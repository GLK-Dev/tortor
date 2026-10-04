use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

use crate::core::disk_io::AsyncDiskIO;

pub struct FileMapping {
    /// `None` for files that are not downloaded: their bytes are discarded.
    pub file: Option<File>,
    pub start_offset: u64,
    pub end_offset: u64,
    /// Written since the last `flush`.
    dirty: bool,
}

pub struct StandardDisk {
    files: Vec<FileMapping>,
    piece_length: u32,
}

impl StandardDisk {
    pub async fn init(
        base_dir: impl AsRef<Path>,
        total_size: u64,
        piece_length: u32,
        files_meta: Option<&Vec<crate::core::torrent::TorrentFile>>,
        name: &str,
    ) -> Result<Self> {
        Self::init_with_selection(base_dir, total_size, piece_length, files_meta, name, None).await
    }

    /// Like `init`, but files whose entry in `selected` is `false` are not
    /// created: data belonging to them is dropped on write and reads as zeros.
    pub async fn init_with_selection(
        base_dir: impl AsRef<Path>,
        total_size: u64,
        piece_length: u32,
        files_meta: Option<&Vec<crate::core::torrent::TorrentFile>>,
        name: &str,
        selected: Option<&[bool]>,
    ) -> Result<Self> {
        let selected: Option<Vec<bool>> = selected.map(<[bool]>::to_vec);
        let base_dir = base_dir.as_ref().to_path_buf();
        let name = name.to_string();
        let is_multi = files_meta.is_some();
        let torrent_files = files_meta.cloned().unwrap_or_else(|| {
            vec![crate::core::torrent::TorrentFile {
                length: total_size,
                path: vec![name.clone()],
            }]
        });

        let mappings_data = tokio::task::spawn_blocking(
            move || -> Result<Vec<(Option<std::fs::File>, u64, u64)>> {
                std::fs::create_dir_all(&base_dir).context("failed to create base dir")?;

                let mut mappings = Vec::new();
                let mut current_offset = 0u64;

                for (index, tf) in torrent_files.into_iter().enumerate() {
                    let keep = selected
                        .as_ref()
                        .and_then(|flags| flags.get(index))
                        .copied()
                        .unwrap_or(true);
                    if !keep {
                        mappings.push((None, current_offset, current_offset + tf.length));
                        current_offset += tf.length;
                        continue;
                    }

                    let file_path =
                        crate::core::torrent::build_file_path(&base_dir, &name, is_multi, &tf.path)
                            .map_err(anyhow::Error::msg)?;

                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }

                    let std_file = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&file_path)
                        .with_context(|| format!("failed to open file {}", file_path.display()))?;

                    let metadata = std_file.metadata()?;
                    if metadata.len() != tf.length {
                        std_file.set_len(tf.length).with_context(|| {
                            format!("failed to preallocate {}", file_path.display())
                        })?;
                    }

                    mappings.push((Some(std_file), current_offset, current_offset + tf.length));
                    current_offset += tf.length;
                }

                Ok(mappings)
            },
        )
        .await??;

        let files = mappings_data
            .into_iter()
            .map(|(std_file, start_offset, end_offset)| FileMapping {
                file: std_file.map(File::from_std),
                start_offset,
                end_offset,
                dirty: false,
            })
            .collect();

        Ok(Self {
            files,
            piece_length,
        })
    }
}

/// Finds the file that contains the absolute byte `offset`. Files are laid out
/// back to back, so the lookup is a binary search; zero-length files never match.
fn locate(files: &mut [FileMapping], offset: u64) -> Option<&mut FileMapping> {
    let idx = files.partition_point(|m| m.end_offset <= offset);
    files.get_mut(idx).filter(|m| m.start_offset <= offset)
}

#[async_trait(?Send)]
impl AsyncDiskIO for StandardDisk {
    async fn flush(&mut self) -> Result<()> {
        for mapping in self.files.iter_mut().filter(|m| m.dirty) {
            if let Some(file) = mapping.file.as_mut() {
                file.sync_data().await?;
            }
            mapping.dirty = false;
        }
        Ok(())
    }

    async fn write_piece(&mut self, piece_index: u32, data: Vec<u8>) -> Result<()> {
        let piece_offset = (piece_index as u64) * (self.piece_length as u64);
        let mut written = 0;

        while written < data.len() {
            let current_abs_offset = piece_offset + written as u64;

            if let Some(mapping) = locate(&mut self.files, current_abs_offset) {
                let file_offset = current_abs_offset - mapping.start_offset;
                let available_in_file = mapping.end_offset - current_abs_offset;
                let to_write = std::cmp::min(data.len() - written, available_in_file as usize);

                if let Some(file) = mapping.file.as_mut() {
                    file.seek(SeekFrom::Start(file_offset)).await?;
                    file.write_all(&data[written..written + to_write]).await?;
                    mapping.dirty = true;
                }

                written += to_write;
            } else {
                anyhow::bail!("piece offset out of bounds");
            }
        }

        Ok(())
    }

    async fn read_piece(&mut self, piece_index: u32, offset: u32, len: u32) -> Result<Vec<u8>> {
        let piece_offset = (piece_index as u64) * (self.piece_length as u64);
        let absolute_offset = piece_offset + offset as u64;

        let mut buffer = vec![0u8; len as usize];
        let mut read = 0;

        while read < len as usize {
            let current_abs_offset = absolute_offset + read as u64;

            if let Some(mapping) = locate(&mut self.files, current_abs_offset) {
                let file_offset = current_abs_offset - mapping.start_offset;
                let available_in_file = mapping.end_offset - current_abs_offset;
                let to_read = std::cmp::min((len as usize) - read, available_in_file as usize);

                // Files that are not downloaded read as zeros.
                if let Some(file) = mapping.file.as_mut() {
                    file.seek(SeekFrom::Start(file_offset)).await?;
                    file.read_exact(&mut buffer[read..read + to_read]).await?;
                }

                read += to_read;
            } else {
                anyhow::bail!("piece offset out of bounds for read");
            }
        }

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::torrent::TorrentFile;

    #[tokio::test]
    async fn pieces_span_files_and_survive_flush() {
        let dir = std::env::temp_dir().join(format!("tortor-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Files of 10, 0, 25 and 5 bytes; pieces of 16 bytes cross every boundary.
        let files = vec![
            TorrentFile {
                length: 10,
                path: vec!["a.bin".into()],
            },
            TorrentFile {
                length: 0,
                path: vec!["empty.bin".into()],
            },
            TorrentFile {
                length: 25,
                path: vec!["sub".into(), "b.bin".into()],
            },
            TorrentFile {
                length: 5,
                path: vec!["c.bin".into()],
            },
        ];
        let data: Vec<u8> = (0..40u8).collect();
        let mut disk = StandardDisk::init(&dir, 40, 16, Some(&files), "t")
            .await
            .unwrap();

        disk.write_piece(0, data[0..16].to_vec()).await.unwrap();
        disk.write_piece(1, data[16..32].to_vec()).await.unwrap();
        disk.write_piece(2, data[32..40].to_vec()).await.unwrap();
        disk.flush().await.unwrap();

        assert_eq!(disk.read_piece(0, 0, 16).await.unwrap(), data[0..16]);
        assert_eq!(disk.read_piece(1, 4, 12).await.unwrap(), data[20..32]);
        assert_eq!(disk.read_piece(2, 0, 8).await.unwrap(), data[32..40]);
        assert!(disk.read_piece(3, 0, 1).await.is_err());

        assert_eq!(
            std::fs::read(dir.join("t").join("a.bin")).unwrap(),
            data[0..10]
        );
        assert_eq!(
            std::fs::read(dir.join("t").join("sub").join("b.bin")).unwrap(),
            data[10..35]
        );
        assert_eq!(
            std::fs::read(dir.join("t").join("c.bin")).unwrap(),
            data[35..40]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unselected_files_are_not_created() {
        let dir = std::env::temp_dir().join(format!("tortor-disk-sel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let files = vec![
            TorrentFile {
                length: 10,
                path: vec!["skip.bin".into()],
            },
            TorrentFile {
                length: 20,
                path: vec!["keep.bin".into()],
            },
        ];
        let data: Vec<u8> = (100..130u8).collect();
        let mut disk = StandardDisk::init_with_selection(
            &dir,
            30,
            15,
            Some(&files),
            "t",
            Some(&[false, true]),
        )
        .await
        .unwrap();

        // Piece 0 straddles both files; the skipped part is discarded.
        disk.write_piece(0, data[0..15].to_vec()).await.unwrap();
        disk.write_piece(1, data[15..30].to_vec()).await.unwrap();
        disk.flush().await.unwrap();

        assert!(!dir.join("t").join("skip.bin").exists());
        assert_eq!(
            std::fs::read(dir.join("t").join("keep.bin")).unwrap(),
            data[10..30]
        );

        let first = disk.read_piece(0, 0, 15).await.unwrap();
        assert_eq!(first[..10], [0u8; 10]);
        assert_eq!(first[10..], data[10..15]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
