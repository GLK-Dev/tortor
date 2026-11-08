/! BFS Direct-IO disk backend; compiled on Haiku / HeOS only (see `core/mod.rs`).

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use heos_fs::{HeosFile, HEOS_FS_CREATE, HEOS_FS_DIRECT_IO, HEOS_FS_READ, HEOS_FS_WRITE};

use crate::core::disk::{is_selected, plan_files};
use crate::core::disk_io::AsyncDiskIO;
use crate::core::torrent::TorrentFile;

pub struct BfsFileMapping {
    pub path: PathBuf,
    /// `None` for files that are not downloaded: their bytes are discarded.
    pub file: Option<HeosFile>,
    pub start_offset: u64,
    pub end_offset: u64,
    /// Written since the last `flush`.
    pub dirty: bool,
}

pub struct BfsDirectDisk {
    files: Vec<BfsFileMapping>,
    piece_length: u32,
}

fn open_bfs_preallocated(path: &Path, length: u64) -> Result<HeosFile> {
    if let Some(parent) = path.parent() {
        let _ = heos_fs::HeosFile::create_dir_all(parent);
    }
    let file = HeosFile::open(
        path,
        HEOS_FS_READ | HEOS_FS_WRITE | HEOS_FS_CREATE | HEOS_FS_DIRECT_IO,
    )
    .with_context(|| format!("failed to open BFS Direct-IO file {}", path.display()))?;

    file.preallocate(length)
        .with_context(|| format!("failed to preallocate BFS file {}", path.display()))?;

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

        let _ = heos_fs::HeosFile::create_dir_all(base_dir);

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
            files.push(BfsFileMapping {
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
fn locate(files: &mut [BfsFileMapping], offset: u64) -> Option<&mut BfsFileMapping> {
    let idx = files.partition_point(|m| m.end_offset <= offset);
    files.get_mut(idx).filter(|m| m.start_offset <= offset)
}

#[async_trait(?Send)]
impl AsyncDiskIO for BfsDirectDisk {
    async fn flush(&mut self) -> Result<()> {
        for mapping in self.files.iter_mut().filter(|m| m.dirty) {
            if let Some(file) = mapping.file.as_ref() {
                file.flush()?;
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

                if let Some(file) = mapping.file.as_ref() {
                    let mut file_done = 0;
                    while file_done < to_write {
                        let slice = &data[written + file_done..written + to_write];
                        let n = file.write_at(slice, file_offset + file_done as u64)?;
                        ensure!(n > 0, "write returned zero bytes");
                        file_done += n;
                    }
                    mapping.dirty = true;
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

                // Files that are not downloaded read as zeros.
                match mapping.file.as_ref() {
                    Some(file) => {
                        let mut chunk = vec![0u8; to_read];
                        let mut chunk_done = 0;
                        while chunk_done < to_read {
                            let n = file.read_at(
                                &mut chunk[chunk_done..to_read],
                                file_offset + chunk_done as u64,
                            )?;
                            ensure!(n > 0, "unexpected end of file");
                            chunk_done += n;
                        }
                        final_buffer.extend_from_slice(&chunk);
                    }
                    None => {
                        final_buffer.resize(final_buffer.len() + to_read, 0);
                    }
                }
                read += to_read;
            } else {
                bail!("piece offset out of bounds for read in BfsDirectDisk");
            }
        }

        Ok(final_buffer)
    }
}