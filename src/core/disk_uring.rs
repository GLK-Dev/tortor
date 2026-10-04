//! io_uring disk backend; compiled on Linux only (see `core/mod.rs`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio_uring::buf::IoBuf;
use tokio_uring::fs::{File, OpenOptions};

use crate::core::disk::{is_selected, open_preallocated, plan_files};
use crate::core::disk_io::AsyncDiskIO;
use crate::core::torrent::TorrentFile;

pub struct UringFileMapping {
    path: PathBuf,
    /// `None` for files that are not downloaded: their bytes are discarded.
    file: Option<File>,
    start_offset: u64,
    end_offset: u64,
    /// Written since the last `flush`.
    dirty: bool,
}

pub struct UringDisk {
    files: Vec<UringFileMapping>,
    piece_length: u32,
}

/// io_uring writes may be partial, so keep going until everything is written.
async fn write_fully(file: &File, mut buf: Vec<u8>, pos: u64) -> Result<()> {
    let len = buf.len();
    let mut done = 0usize;
    while done < len {
        let (result, slice) = file.write_at(buf.slice(done..), pos + done as u64).await;
        let written = result?;
        anyhow::ensure!(written > 0, "write returned zero bytes");
        done += written;
        buf = slice.into_inner();
    }
    Ok(())
}

/// Reads exactly `len` bytes starting at `pos`.
async fn read_fully(file: &File, len: usize, pos: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut done = 0usize;
    while done < len {
        let (result, slice) = file.read_at(buf.slice(done..), pos + done as u64).await;
        let read = result?;
        anyhow::ensure!(read > 0, "unexpected end of file");
        done += read;
        buf = slice.into_inner();
    }
    Ok(buf)
}

async fn open_uring(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("failed to open file {}", path.display()))
}

impl UringDisk {
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
        let base_dir = base_dir.as_ref();
        let is_multi = files_meta.is_some();
        let torrent_files = files_meta.cloned().unwrap_or_else(|| {
            vec![TorrentFile {
                length: total_size,
                path: vec![name.to_string()],
            }]
        });

        std::fs::create_dir_all(base_dir).context("failed to create base dir")?;

        let mut files = Vec::new();
        for (index, plan) in plan_files(base_dir, name, is_multi, &torrent_files)?
            .into_iter()
            .enumerate()
        {
            let file = if is_selected(selected, index) {
                // Create and size the file with plain std calls, then reopen it for io_uring.
                open_preallocated(&plan.path, plan.end - plan.start)?;
                Some(open_uring(&plan.path).await?)
            } else {
                None
            };
            files.push(UringFileMapping {
                path: plan.path,
                file,
                start_offset: plan.start,
                end_offset: plan.end,
                dirty: false,
            });
        }

        Ok(Self {
            files,
            piece_length,
        })
    }
}

/// Finds the file that contains the absolute byte `offset`. Files are laid out
/// back to back, so the lookup is a binary search; zero-length files never match.
fn locate(files: &mut [UringFileMapping], offset: u64) -> Option<&mut UringFileMapping> {
    let idx = files.partition_point(|m| m.end_offset <= offset);
    files.get_mut(idx).filter(|m| m.start_offset <= offset)
}

#[async_trait(?Send)]
impl AsyncDiskIO for UringDisk {
    async fn flush(&mut self) -> Result<()> {
        for mapping in self.files.iter_mut().filter(|m| m.dirty) {
            if let Some(file) = mapping.file.as_ref() {
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
                    open_preallocated(&mapping.path, mapping.end_offset - mapping.start_offset)?;
                    mapping.file = Some(open_uring(&mapping.path).await?);
                }
                (false, true) => {
                    // Data already on disk stays there; the handle is just released.
                    if let Some(file) = mapping.file.take() {
                        file.sync_data().await?;
                        file.close().await?;
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

                if let Some(file) = mapping.file.as_ref() {
                    let slice = data[written..written + to_write].to_vec();
                    write_fully(file, slice, file_offset).await?;
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

        let mut final_buffer = Vec::with_capacity(len as usize);
        let mut read = 0;

        while read < len as usize {
            let current_abs_offset = absolute_offset + read as u64;

            if let Some(mapping) = locate(&mut self.files, current_abs_offset) {
                let file_offset = current_abs_offset - mapping.start_offset;
                let available_in_file = mapping.end_offset - current_abs_offset;
                let to_read = std::cmp::min((len as usize) - read, available_in_file as usize);

                // Files that are not downloaded read as zeros.
                match mapping.file.as_ref() {
                    Some(file) => {
                        let buffer = read_fully(file, to_read, file_offset).await?;
                        final_buffer.extend_from_slice(&buffer);
                    }
                    None => final_buffer.resize(final_buffer.len() + to_read, 0),
                }
                read += to_read;
            } else {
                anyhow::bail!("piece offset out of bounds for read");
            }
        }

        Ok(final_buffer)
    }
}
