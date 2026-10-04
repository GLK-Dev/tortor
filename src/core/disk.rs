use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

use crate::core::disk_io::AsyncDiskIO;
use crate::core::torrent::TorrentFile;

/// Where one torrent file lives on disk and which byte range it covers.
#[derive(Debug, Clone)]
pub(crate) struct FilePlan {
    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
}

/// Lays the torrent's files out back to back below `base_dir`.
pub(crate) fn plan_files(
    base_dir: &Path,
    name: &str,
    is_multi: bool,
    files: &[TorrentFile],
) -> Result<Vec<FilePlan>> {
    let mut plans = Vec::with_capacity(files.len());
    let mut offset = 0u64;
    for file in files {
        let path = crate::core::torrent::build_file_path(base_dir, name, is_multi, &file.path)
            .map_err(anyhow::Error::msg)?;
        plans.push(FilePlan {
            path,
            start: offset,
            end: offset + file.length,
        });
        offset += file.length;
    }
    Ok(plans)
}

/// Creates the file (and its directories) and sets it to `length` bytes.
pub(crate) fn open_preallocated(path: &Path, length: u64) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("failed to open file {}", path.display()))?;

    if file.metadata()?.len() != length {
        file.set_len(length)
            .with_context(|| format!("failed to preallocate {}", path.display()))?;
    }
    Ok(file)
}

/// `selected[i]` for file `i`; files beyond the slice count as selected.
pub(crate) fn is_selected(selected: Option<&[bool]>, index: usize) -> bool {
    selected
        .and_then(|flags| flags.get(index))
        .copied()
        .unwrap_or(true)
}

pub struct FileMapping {
    path: PathBuf,
    /// `None` for files that are not downloaded: their bytes are discarded.
    file: Option<File>,
    start_offset: u64,
    end_offset: u64,
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
        files_meta: Option<&Vec<TorrentFile>>,
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
        files_meta: Option<&Vec<TorrentFile>>,
        name: &str,
        selected: Option<&[bool]>,
    ) -> Result<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        let name = name.to_string();
        let is_multi = files_meta.is_some();
        let torrent_files = files_meta.cloned().unwrap_or_else(|| {
            vec![TorrentFile {
                length: total_size,
                path: vec![name.clone()],
            }]
        });
        let selected: Option<Vec<bool>> = selected.map(<[bool]>::to_vec);

        let opened = tokio::task::spawn_blocking(
            move || -> Result<Vec<(FilePlan, Option<std::fs::File>)>> {
                std::fs::create_dir_all(&base_dir).context("failed to create base dir")?;

                plan_files(&base_dir, &name, is_multi, &torrent_files)?
                    .into_iter()
                    .enumerate()
                    .map(|(index, plan)| {
                        let file = is_selected(selected.as_deref(), index)
                            .then(|| open_preallocated(&plan.path, plan.end - plan.start))
                            .transpose()?;
                        Ok((plan, file))
                    })
                    .collect()
            },
        )
        .await??;

        let files = opened
            .into_iter()
            .map(|(plan, file)| FileMapping {
                path: plan.path,
                file: file.map(File::from_std),
                start_offset: plan.start,
                end_offset: plan.end,
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

    async fn set_selection(&mut self, selected: &[bool]) -> Result<()> {
        for (index, mapping) in self.files.iter_mut().enumerate() {
            let keep = is_selected(Some(selected), index);
            match (keep, mapping.file.is_some()) {
                (true, false) => {
                    let (path, length) = (
                        mapping.path.clone(),
                        mapping.end_offset - mapping.start_offset,
                    );
                    let file =
                        tokio::task::spawn_blocking(move || open_preallocated(&path, length))
                            .await??;
                    mapping.file = Some(File::from_std(file));
                }
                (false, true) => {
                    // Data already on disk stays there; the handle is just released.
                    if let Some(file) = mapping.file.take() {
                        file.sync_data().await?;
                    }
                    mapping.dirty = false;
                }
                _ => {}
            }
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

    fn file(length: u64, path: &[&str]) -> TorrentFile {
        TorrentFile {
            length,
            path: path.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tortor-disk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn pieces_span_files_and_survive_flush() {
        let dir = temp("span");

        // Files of 10, 0, 25 and 5 bytes; pieces of 16 bytes cross every boundary.
        let files = vec![
            file(10, &["a.bin"]),
            file(0, &["empty.bin"]),
            file(25, &["sub", "b.bin"]),
            file(5, &["c.bin"]),
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
        let dir = temp("unselected");

        let files = vec![file(10, &["skip.bin"]), file(20, &["keep.bin"])];
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

    #[tokio::test]
    async fn selection_can_change_while_running() {
        let dir = temp("switch");

        let files = vec![file(10, &["a.bin"]), file(10, &["b.bin"])];
        let data: Vec<u8> = (1..=20u8).collect();
        let mut disk = StandardDisk::init_with_selection(
            &dir,
            20,
            10,
            Some(&files),
            "t",
            Some(&[true, false]),
        )
        .await
        .unwrap();

        disk.write_piece(0, data[0..10].to_vec()).await.unwrap();
        disk.write_piece(1, data[10..20].to_vec()).await.unwrap(); // discarded
        assert!(!dir.join("t").join("b.bin").exists());

        // Select the second file and drop the first: b appears, a keeps its data.
        disk.set_selection(&[false, true]).await.unwrap();
        assert!(dir.join("t").join("b.bin").exists());
        disk.write_piece(1, data[10..20].to_vec()).await.unwrap();
        disk.flush().await.unwrap();

        assert_eq!(
            std::fs::read(dir.join("t").join("b.bin")).unwrap(),
            data[10..20]
        );
        assert_eq!(
            std::fs::read(dir.join("t").join("a.bin")).unwrap(),
            data[0..10]
        );
        // The released file reads as zeros now.
        assert_eq!(disk.read_piece(0, 0, 10).await.unwrap(), [0u8; 10]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
