// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! BFS Direct-IO disk backend; compiled on Haiku / HeOS only (see `core/mod.rs`).

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::core::disk::{is_selected, plan_files};
use crate::core::disk_io::AsyncDiskIO;
use crate::core::heos_fs_ffi::{
    HeosFile, HEOS_FS_CREATE, HEOS_FS_DIRECT_IO, HEOS_FS_READ, HEOS_FS_WRITE,
};
use crate::core::torrent::TorrentFile;

pub struct BfsFileMapping {
    pub path: PathBuf,
    /// `None` for files that are not downloaded: their bytes are discarded.
    pub file: Option<HeosFile>,
    pub start_offset: u64,
    pub end_offset: u64,
    /// Written since the last `flush`.
    pub dirty: bool,
    /// The handle was opened with O_DIRECT. Cleared after a fallback to buffered I/O.
    pub direct: bool,
}

pub struct BfsDirectDisk {
    files: Vec<BfsFileMapping>,
    piece_length: u32,
}

fn open_bfs_preallocated(path: &Path, length: u64) -> Result<HeosFile> {
    if let Some(parent) = path.parent() {
        let _ = HeosFile::create_dir_all(parent);
    }
    let file = HeosFile::open(
        path,
        HEOS_FS_READ | HEOS_FS_WRITE | HEOS_FS_CREATE | HEOS_FS_DIRECT_IO,
    )
    .with_context(|| format!("failed to open BFS Direct-IO file {}", path.display()))?;

    file.preallocate(length)
        .with_context(|| format!("failed to preallocate BFS file {}", path.display()))?;

    info!(
        "[HeOS BFS Direct-IO] Initialized and preallocated {:?} ({} bytes) with O_DIRECT flags",
        path, length
    );

    Ok(file)
}

impl BfsDirectDisk {
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

        let _ = HeosFile::create_dir_all(base_dir);

        let mut files = Vec::new();
        for (index, plan) in plan_files(base_dir, name, is_multi, &torrent_files)?
            .into_iter()
            .enumerate()
        {
            let file = if is_selected(selected, index) {
                let length = plan.end - plan.start;
                Some(open_bfs_preallocated(&plan.path, length)?)
            } else {
                None
            };
            let direct = file.is_some();
            files.push(BfsFileMapping {
                path: plan.path,
                file,
                start_offset: plan.start,
                end_offset: plan.end,
                dirty: false,
                direct,
            });
        }

        info!(
            "[HeOS BFS Direct-IO] Backend ready with {} file mappings, piece_length={} bytes",
            files.len(),
            piece_length
        );

        Ok(Self {
            files,
            piece_length,
        })
    }
}

/// Finds the file that contains the absolute byte `offset`. Files are laid out
/// back to back, so the lookup is a binary search; zero-length files never match.
fn locate(files: &mut [BfsFileMapping], offset: u64) -> Option<&mut BfsFileMapping> {
    let idx = files.partition_point(|m| m.end_offset <= offset);
    files.get_mut(idx).filter(|m| m.start_offset <= offset)
}

/// Reopens the file without O_DIRECT. The new handle is opened before the old one is
/// replaced, so a failure leaves the mapping usable.
fn reopen_buffered(mapping: &mut BfsFileMapping) -> Result<()> {
    let file = HeosFile::open(&mapping.path, HEOS_FS_READ | HEOS_FS_WRITE).with_context(|| {
        format!(
            "failed to reopen {} without O_DIRECT",
            mapping.path.display()
        )
    })?;
    mapping.file = Some(file);
    mapping.direct = false;
    Ok(())
}

/// The SDK falls back to buffered I/O only when `open` fails. A later `pwrite` on an
/// O_DIRECT handle can still fail (for example on alignment), so retry once through a
/// buffered handle instead of failing the whole download.
fn write_with_fallback(mapping: &mut BfsFileMapping, buf: &[u8], offset: u64) -> Result<usize> {
    let first = match mapping.file.as_ref() {
        Some(file) => file.write_at(buf, offset),
        None => bail!("write to a file that is not selected"),
    };
    match first {
        Ok(n) => Ok(n),
        Err(err) if mapping.direct => {
            warn!(
                "[HeOS BFS Direct-IO] O_DIRECT write failed on {:?} ({err:#}); retrying buffered",
                mapping.path
            );
            reopen_buffered(mapping)?;
            match mapping.file.as_ref() {
                Some(file) => file.write_at(buf, offset),
                None => bail!("write to a file that is not selected"),
            }
        }
        Err(err) => Err(err),
    }
}

/// Read counterpart of `write_with_fallback`.
fn read_with_fallback(mapping: &mut BfsFileMapping, buf: &mut [u8], offset: u64) -> Result<usize> {
    let first = match mapping.file.as_ref() {
        Some(file) => file.read_at(buf, offset),
        None => bail!("read from a file that is not selected"),
    };
    match first {
        Ok(n) => Ok(n),
        Err(err) if mapping.direct => {
            warn!(
                "[HeOS BFS Direct-IO] O_DIRECT read failed on {:?} ({err:#}); retrying buffered",
                mapping.path
            );
            reopen_buffered(mapping)?;
            match mapping.file.as_ref() {
                Some(file) => file.read_at(buf, offset),
                None => bail!("read from a file that is not selected"),
            }
        }
        Err(err) => Err(err),
    }
}

#[async_trait(?Send)]
impl AsyncDiskIO for BfsDirectDisk {
    async fn flush(&mut self) -> Result<()> {
        for mapping in self.files.iter_mut().filter(|m| m.dirty) {
            if let Some(file) = mapping.file.as_ref() {
                file.flush()?;
                debug!("[HeOS BFS Direct-IO] Flushed file {:?}", mapping.path);
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
                    let length = mapping.end_offset - mapping.start_offset;
                    let file = open_bfs_preallocated(&mapping.path, length)?;
                    mapping.file = Some(file);
                    mapping.direct = true;
                }
                (false, true) => {
                    // Data already on disk stays there; the handle is just released.
                    if let Some(file) = mapping.file.take() {
                        let _ = file.flush();
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

                if mapping.file.is_some() {
                    let mut file_done = 0;
                    while file_done < to_write {
                        let slice = &data[written + file_done..written + to_write];
                        let n =
                            write_with_fallback(mapping, slice, file_offset + file_done as u64)?;
                        ensure!(n > 0, "write returned zero bytes");
                        file_done += n;
                    }
                    mapping.dirty = true;
                    info!(
                        "[HeOS BFS Direct-IO] Write piece #{} ({} bytes) at BFS offset {} (O_DIRECT: {})",
                        piece_index,
                        to_write,
                        file_offset,
                        mapping.direct
                    );
                }

                written += to_write;
            } else {
                bail!("piece offset out of bounds in BfsDirectDisk::write_piece");
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

                if mapping.file.is_some() {
                    let mut temp_buf = vec![0u8; to_read];
                    let mut file_done = 0;
                    while file_done < to_read {
                        let n = read_with_fallback(
                            mapping,
                            &mut temp_buf[file_done..to_read],
                            file_offset + file_done as u64,
                        )?;
                        ensure!(n > 0, "read returned zero bytes before EOF");
                        file_done += n;
                    }
                    final_buffer.extend_from_slice(&temp_buf);
                } else {
                    final_buffer.resize(final_buffer.len() + to_read, 0);
                }

                read += to_read;
            } else {
                bail!("piece offset out of bounds in BfsDirectDisk::read_piece");
            }
        }

        Ok(final_buffer)
    }
}
