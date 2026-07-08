use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

pub struct DiskWriter {
    file: File,
    piece_length: u32,
}

impl DiskWriter {
    pub async fn init(path: PathBuf, total_size: u64, piece_length: u32) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .await
            .context("failed to open or create download file")?;

        let metadata = file.metadata().await?;
        if metadata.len() != total_size {
            file.set_len(total_size)
                .await
                .context("failed to preallocate file size")?;
            tracing::info!("Preallocated file {} to {} bytes", path.display(), total_size);
        }

        Ok(Self { file, piece_length })
    }

    pub async fn write_piece(&mut self, piece_index: u32, data: &[u8]) -> Result<()> {
        let offset = (piece_index as u64) * (self.piece_length as u64);

        self.file.seek(SeekFrom::Start(offset)).await?;
        self.file
            .write_all(data)
            .await
            .context("failed to write piece data")?;
        self.file.sync_data().await?;

        Ok(())
    }

    pub async fn read_piece(&mut self, piece_index: u32, offset: u32, len: u32) -> Result<Vec<u8>> {
        let piece_offset = (piece_index as u64) * (self.piece_length as u64);
        let absolute_offset = piece_offset + offset as u64;

        self.file.seek(SeekFrom::Start(absolute_offset)).await?;

        let mut buffer = vec![0u8; len as usize];
        self.file
            .read_exact(&mut buffer)
            .await
            .context("failed to read piece data from disk")?;

        Ok(buffer)
    }
}
