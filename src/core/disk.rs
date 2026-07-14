use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

pub struct FileMapping {
    pub file: File,
    pub start_offset: u64,
    pub end_offset: u64,
}

pub struct DiskWriter {
    files: Vec<FileMapping>,
    piece_length: u32,
}

impl DiskWriter {
    pub async fn init(
        base_dir: impl AsRef<Path>,
        total_size: u64,
        piece_length: u32,
        files_meta: Option<&Vec<crate::core::torrent::TorrentFile>>,
        name: &str,
    ) -> Result<Self> {
        let base_dir = base_dir.as_ref();
        tokio::fs::create_dir_all(base_dir).await.context("failed to create base dir")?;

        let mut mappings = Vec::new();
        let mut current_offset = 0u64;

        let torrent_files = files_meta.cloned().unwrap_or_else(|| {
            vec![crate::core::torrent::TorrentFile {
                length: total_size,
                path: vec![name.to_string()],
            }]
        });

        for tf in torrent_files {
            let mut file_path = base_dir.to_path_buf();
            // If it's a multi-file torrent, usually the 'name' is the root directory
            if files_meta.is_some() {
                file_path.push(name);
            }
            for p in &tf.path {
                file_path.push(p);
            }

            if let Some(parent) = file_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&file_path)
                .await
                .with_context(|| format!("failed to open file {}", file_path.display()))?;

            let metadata = file.metadata().await?;
            if metadata.len() != tf.length {
                file.set_len(tf.length)
                    .await
                    .with_context(|| format!("failed to preallocate {}", file_path.display()))?;
                tracing::info!("Preallocated file {} to {} bytes", file_path.display(), tf.length);
            }

            mappings.push(FileMapping {
                file,
                start_offset: current_offset,
                end_offset: current_offset + tf.length,
            });

            current_offset += tf.length;
        }

        Ok(Self {
            files: mappings,
            piece_length,
        })
    }

    pub async fn write_piece(&mut self, piece_index: u32, data: &[u8]) -> Result<()> {
        let piece_offset = (piece_index as u64) * (self.piece_length as u64);
        let mut written = 0;

        while written < data.len() {
            let current_abs_offset = piece_offset + written as u64;
            
            if let Some(mapping) = self.files.iter_mut().find(|m| current_abs_offset >= m.start_offset && current_abs_offset < m.end_offset) {
                let file_offset = current_abs_offset - mapping.start_offset;
                let available_in_file = mapping.end_offset - current_abs_offset;
                let to_write = std::cmp::min(data.len() - written, available_in_file as usize);

                mapping.file.seek(SeekFrom::Start(file_offset)).await?;
                mapping.file.write_all(&data[written..written + to_write]).await?;
                mapping.file.sync_data().await?;
                
                written += to_write;
            } else {
                anyhow::bail!("piece offset out of bounds");
            }
        }

        Ok(())
    }

    pub async fn read_piece(&mut self, piece_index: u32, offset: u32, len: u32) -> Result<Vec<u8>> {
        let piece_offset = (piece_index as u64) * (self.piece_length as u64);
        let absolute_offset = piece_offset + offset as u64;

        let mut buffer = vec![0u8; len as usize];
        let mut read = 0;

        while read < len as usize {
            let current_abs_offset = absolute_offset + read as u64;

            if let Some(mapping) = self.files.iter_mut().find(|m| current_abs_offset >= m.start_offset && current_abs_offset < m.end_offset) {
                let file_offset = current_abs_offset - mapping.start_offset;
                let available_in_file = mapping.end_offset - current_abs_offset;
                let to_read = std::cmp::min((len as usize) - read, available_in_file as usize);

                mapping.file.seek(SeekFrom::Start(file_offset)).await?;
                mapping.file.read_exact(&mut buffer[read..read + to_read]).await?;
                
                read += to_read;
            } else {
                anyhow::bail!("piece offset out of bounds for read");
            }
        }

        Ok(buffer)
    }
}
