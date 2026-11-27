// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashSet;
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

fn configure_share_mode(options: &mut std::fs::OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ (1) | FILE_SHARE_WRITE (2) | FILE_SHARE_DELETE (4)
        // Allows users to rename, move, or delete files even while handles exist.
        options.share_mode(0x7);
    }
    #[cfg(not(windows))]
    let _ = options;
}

/// Creates the file (and its directories) and sets it to `length` bytes.
pub(crate) fn open_preallocated(path: &Path, length: u64) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    configure_share_mode(&mut options);

    let file = options
        .open(path)
        .with_context(|| format!("failed to open file {}", path.display()))?;

    if file.metadata()?.len() != length {
        file.set_len(length)
            .with_context(|| format!("failed to preallocate {}", path.display()))?;
    }
    Ok(file)
}

/// Opens an existing file read-only without creating or resizing it.
fn open_readonly(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    configure_share_mode(&mut options);
    options
        .open(path)
        .with_context(|| format!("failed to open file {} for reading", path.display()))
}

/// `selected[i]` for file `i`; files beyond the slice count as selected.
pub(crate) fn is_selected(selected: Option<&[bool]>, index: usize) -> bool {
    selected
        .and_then(|flags| flags.get(index))
        .copied()
        .unwrap_or(true)
}

/// Filters `completed` pieces from fastresume against actual files on disk.
///
/// Any piece overlapping a selected non-zero file that is missing or has an
/// unexpected byte length is removed. Returns `(valid_pieces, invalidated_any)`.
pub fn filter_completed_against_disk(
    base_dir: &Path,
    name: &str,
    total_size: u64,
    piece_length: u32,
    files_meta: Option<&Vec<TorrentFile>>,
    selected: Option<&[bool]>,
    completed: &[u32],
) -> Result<(Vec<u32>, bool)> {
    if completed.is_empty() {
        return Ok((Vec::new(), false));
    }

    let is_multi = files_meta.is_some();
    let torrent_files = files_meta.cloned().unwrap_or_else(|| {
        vec![TorrentFile {
            length: total_size,
            path: vec![name.to_string()],
        }]
    });
    let plans = plan_files(base_dir, name, is_multi, &torrent_files)?;
    let piece_len = (piece_length.max(1)) as u64;

    let mut invalid_pieces: HashSet<u32> = HashSet::new();
    for (index, plan) in plans.iter().enumerate() {
        let expected_len = plan.end.saturating_sub(plan.start);
        if expected_len == 0 || !is_selected(selected, index) {
            continue;
        }
        let valid = match std::fs::metadata(&plan.path) {
            Ok(meta) => meta.is_file() && meta.len() == expected_len,
            Err(_) => false,
        };
        if !valid {
            let first = (plan.start / piece_len) as u32;
            let last = ((plan.end - 1) / piece_len) as u32;
            for piece in first..=last {
                invalid_pieces.insert(piece);
            }
        }
    }

    if invalid_pieces.is_empty() {
        return Ok((completed.to_vec(), false));
    }

    let valid: Vec<u32> = completed
        .iter()
        .copied()
        .filter(|piece| !invalid_pieces.contains(piece))
        .collect();
    let invalidated_any = valid.len() < completed.len();
    Ok((valid, invalidated_any))
}

/// Returns `true` if at least one selected non-zero file already exists on disk.
pub fn any_selected_file_exists(
    base_dir: &Path,
    name: &str,
    total_size: u64,
    files_meta: Option<&Vec<TorrentFile>>,
    selected: Option<&[bool]>,
) -> Result<bool> {
    let is_multi = files_meta.is_some();
    let torrent_files = files_meta.cloned().unwrap_or_else(|| {
        vec![TorrentFile {
            length: total_size,
            path: vec![name.to_string()],
        }]
    });
    let plans = plan_files(base_dir, name, is_multi, &torrent_files)?;
    Ok(plans.iter().enumerate().any(|(index, plan)| {
        let len = plan.end.saturating_sub(plan.start);
        len > 0 && is_selected(selected, index) && plan.path.is_file()
    }))
}

pub struct FileMapping {
    path: PathBuf,
    selected: bool,
    /// Cached read/write handle while actively downloading; released when
    /// completed, paused, or idle so external apps/users can freely use files.
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
        Self::init_with_options(
            base_dir,
            total_size,
            piece_length,
            files_meta,
            name,
            selected,
            true,
        )
        .await
    }

    /// Initializes the disk mapping. When `create_files` is `false`, missing
    /// files are not created until a piece is actually written to them, and
    /// existing files are opened read-only on demand without holding locks.
    pub async fn init_with_options(
        base_dir: impl AsRef<Path>,
        total_size: u64,
        piece_length: u32,
        files_meta: Option<&Vec<TorrentFile>>,
        name: &str,
        selected: Option<&[bool]>,
        create_files: bool,
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
            move || -> Result<Vec<(FilePlan, bool, Option<std::fs::File>)>> {
                if create_files {
                    std::fs::create_dir_all(&base_dir).context("failed to create base dir")?;
                }

                plan_files(&base_dir, &name, is_multi, &torrent_files)?
                    .into_iter()
                    .enumerate()
                    .map(|(index, plan)| {
                        let keep = is_selected(selected.as_deref(), index);
                        let file = if keep && create_files {
                            Some(open_preallocated(&plan.path, plan.end - plan.start)?)
                        } else {
                            None
                        };
                        Ok((plan, keep, file))
                    })
                    .collect()
            },
        )
        .await??;

        let files = opened
            .into_iter()
            .map(|(plan, keep, file)| FileMapping {
                path: plan.path,
                selected: keep,
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

    async fn release_handles(&mut self) -> Result<()> {
        self.flush().await?;
        for mapping in &mut self.files {
            mapping.file.take();
        }
        Ok(())
    }

    async fn set_selection(&mut self, selected: &[bool]) -> Result<()> {
        for (index, mapping) in self.files.iter_mut().enumerate() {
            let keep = is_selected(Some(selected), index);
            mapping.selected = keep;
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

                if mapping.selected {
                    if mapping.file.is_none() {
                        let path = mapping.path.clone();
                        let length = mapping.end_offset - mapping.start_offset;
                        let opened =
                            tokio::task::spawn_blocking(move || open_preallocated(&path, length))
                                .await??;
                        mapping.file = Some(File::from_std(opened));
                    }
                    if let Some(file) = mapping.file.as_mut() {
                        file.seek(SeekFrom::Start(file_offset)).await?;
                        file.write_all(&data[written..written + to_write]).await?;
                        mapping.dirty = true;
                    }
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
                if mapping.selected {
                    if let Some(file) = mapping.file.as_mut() {
                        file.seek(SeekFrom::Start(file_offset)).await?;
                        file.read_exact(&mut buffer[read..read + to_read]).await?;
                    } else {
                        // Open read-only just for this read so seeding/checking never locks files.
                        let path = mapping.path.clone();
                        let mut temp = File::from_std(
                            tokio::task::spawn_blocking(move || open_readonly(&path)).await??,
                        );
                        temp.seek(SeekFrom::Start(file_offset)).await?;
                        temp.read_exact(&mut buffer[read..read + to_read]).await?;
                    }
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

    #[tokio::test]
    async fn release_handles_unlocks_files_and_allows_reads_or_deletion() {
        let dir = temp("release");
        let data: Vec<u8> = (0..32u8).collect();
        let mut disk = StandardDisk::init_with_options(
            &dir,
            32,
            16,
            None,
            "movie.bin",
            None,
            false,
        )
        .await
        .unwrap();

        assert!(!dir.join("movie.bin").exists());
        disk.write_piece(0, data[0..16].to_vec()).await.unwrap();
        disk.write_piece(1, data[16..32].to_vec()).await.unwrap();
        disk.release_handles().await.unwrap();

        // Reads still work on demand without leaving handles open.
        assert_eq!(disk.read_piece(0, 0, 16).await.unwrap(), data[0..16]);

        // External rename & deletion succeed immediately without killing the process.
        let renamed = dir.join("movie-renamed.bin");
        std::fs::rename(dir.join("movie.bin"), &renamed).unwrap();
        assert!(disk.read_piece(0, 0, 16).await.is_err());
        assert!(!dir.join("movie.bin").exists());
        std::fs::remove_file(&renamed).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_completed_drops_pieces_of_deleted_or_resized_files() {
        let dir = temp("validate");
        std::fs::create_dir_all(dir.join("bundle")).unwrap();
        std::fs::write(dir.join("bundle").join("a.bin"), vec![1u8; 15]).unwrap();
        std::fs::write(dir.join("bundle").join("b.bin"), vec![2u8; 25]).unwrap();

        let files = vec![file(15, &["a.bin"]), file(25, &["b.bin"])];
        let (valid, changed) =
            filter_completed_against_disk(&dir, "bundle", 40, 10, Some(&files), None, &[0, 1, 2, 3])
                .unwrap();
        assert_eq!(valid, vec![0, 1, 2, 3]);
        assert!(!changed);

        // Delete b.bin (covers pieces 1, 2, 3; piece 0 belongs only to a.bin).
        std::fs::remove_file(dir.join("bundle").join("b.bin")).unwrap();
        let (valid, changed) =
            filter_completed_against_disk(&dir, "bundle", 40, 10, Some(&files), None, &[0, 1, 2, 3])
                .unwrap();
        assert_eq!(valid, vec![0]);
        assert!(changed);

        let _ = std::fs::remove_dir_all(&dir);
    }
}