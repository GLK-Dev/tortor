use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use async_trait::async_trait;

use crate::core::disk_io::AsyncDiskIO;

pub struct FileMapping {
    pub file: File,
    pub start_offset: u64,
    pub end_offset: u64,
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
        let base_dir = base_dir.as_ref().to_path_buf();
        let name = name.to_string();
        let is_multi = files_meta.is_some();
        let torrent_files = files_meta.cloned().unwrap_or_else(|| {
            vec![crate::core::torrent::TorrentFile {
                length: total_size,
                path: vec![name.clone()],
            }]
        });

        let mappings_data = tokio::task::spawn_blocking(move || -> Result<Vec<(std::fs::File, u64, u64)>> {
            std::fs::create_dir_all(&base_dir).context("failed to create base dir")?;

            let mut mappings = Vec::new();
            let mut current_offset = 0u64;

            for tf in torrent_files {
                let mut file_path = base_dir.clone();
                if is_multi {
                    file_path.push(&name);
                }
                for p in &tf.path {
                    file_path.push(p);
                }

                if let Some(parent) = file_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                let std_file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .open(&file_path)
                    .with_context(|| format!("failed to open file {}", file_path.display()))?;

                let metadata = std_file.metadata()?;
                if metadata.len() != tf.length {
                    std_file.set_len(tf.length)
                        .with_context(|| format!("failed to preallocate {}", file_path.display()))?;
                }

                mappings.push((std_file, current_offset, current_offset + tf.length));
                current_offset += tf.length;
            }

            Ok(mappings)
        }).await??;

        let files = mappings_data.into_iter().map(|(std_file, start_offset, end_offset)| {
            FileMapping {
                file: File::from_std(std_file),
                start_offset,
                end_offset,
            }
        }).collect();

        Ok(Self {
            files,
            piece_length,
        })
    }
}

#[async_trait(?Send)]
impl AsyncDiskIO for StandardDisk {
    async fn write_piece(&mut self, piece_index: u32, data: Vec<u8>) -> Result<()> {
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

    async fn read_piece(&mut self, piece_index: u32, offset: u32, len: u32) -> Result<Vec<u8>> {
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
