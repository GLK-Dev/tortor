// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Minimal FFI to the HeOS BFS Direct-IO SDK (`libheos_fs`).
//!
//! The declarations live in Tortor on purpose: the HeOS source tree is not a Cargo
//! dependency, so Tortor builds without a fixed folder layout. The library itself is
//! linked by `build.rs` on Haiku (search path from `HEOS_SDK_LIB_DIR`).
//! Compiled on Haiku only (see `core/mod.rs`).
//!
//! Keep these signatures in sync with `os/sdk/heos_fs/include/heos_fs.h`.

use std::ffi::CString;
use std::os::raw::c_char;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

pub type HeosStatus = i32;
pub type HeosFileHandle = i32;

pub const HEOS_OK: HeosStatus = 0;
const HEOS_INVALID_HANDLE: HeosFileHandle = -999;

pub const HEOS_FS_READ: u32 = 1 << 0;
pub const HEOS_FS_WRITE: u32 = 1 << 1;
pub const HEOS_FS_CREATE: u32 = 1 << 2;
pub const HEOS_FS_TRUNCATE: u32 = 1 << 3;
pub const HEOS_FS_DIRECT_IO: u32 = 1 << 4;

extern "C" {
    fn heos_fs_create_dir_all(path: *const c_char) -> HeosStatus;
    fn heos_fs_open(path: *const c_char, flags: u32, out_handle: *mut HeosFileHandle) -> HeosStatus;
    fn heos_fs_read(handle: HeosFileHandle, buffer: *mut u8, offset: u64, len: usize) -> isize;
    fn heos_fs_write(handle: HeosFileHandle, buffer: *const u8, offset: u64, len: usize) -> isize;
    fn heos_fs_flush(handle: HeosFileHandle) -> HeosStatus;
    fn heos_fs_preallocate(handle: HeosFileHandle, length: u64) -> HeosStatus;
    fn heos_fs_close(handle: HeosFileHandle) -> HeosStatus;
}

/// RAII wrapper around a native HeOS file handle; closes it on drop.
#[derive(Debug)]
pub struct HeosFile {
    handle: HeosFileHandle,
}

impl HeosFile {
    pub fn open(path: impl AsRef<Path>, flags: u32) -> Result<Self> {
        let p_str = path.as_ref().to_str().context("invalid UTF-8 path")?;
        let c_path = CString::new(p_str).context("null byte in path")?;

        let mut handle = HEOS_INVALID_HANDLE;
        let status = unsafe { heos_fs_open(c_path.as_ptr(), flags, &mut handle) };
        if status != HEOS_OK || handle == HEOS_INVALID_HANDLE {
            bail!("heos_fs_open failed with status {} for '{}'", status, p_str);
        }
        Ok(Self { handle })
    }

    pub fn create_dir_all(path: impl AsRef<Path>) -> Result<()> {
        let p_str = path.as_ref().to_str().context("invalid UTF-8 path")?;
        let c_path = CString::new(p_str).context("null byte in path")?;

        let status = unsafe { heos_fs_create_dir_all(c_path.as_ptr()) };
        ensure!(status == HEOS_OK, "heos_fs_create_dir_all failed ({})", status);
        Ok(())
    }

    pub fn preallocate(&self, length: u64) -> Result<()> {
        let status = unsafe { heos_fs_preallocate(self.handle, length) };
        ensure!(status == HEOS_OK, "heos_fs_preallocate failed ({})", status);
        Ok(())
    }

    /// Returns the number of bytes written; may be less than `buf.len()`.
    pub fn write_at(&self, buf: &[u8], offset: u64) -> Result<usize> {
        let written = unsafe { heos_fs_write(self.handle, buf.as_ptr(), offset, buf.len()) };
        if written < 0 {
            bail!("heos_fs_write failed with code {}", written);
        }
        Ok(written as usize)
    }

    /// Returns the number of bytes read; may be less than `buf.len()`.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        let read = unsafe { heos_fs_read(self.handle, buf.as_mut_ptr(), offset, buf.len()) };
        if read < 0 {
            bail!("heos_fs_read failed with code {}", read);
        }
        Ok(read as usize)
    }

    pub fn flush(&self) -> Result<()> {
        let status = unsafe { heos_fs_flush(self.handle) };
        ensure!(status == HEOS_OK, "heos_fs_flush failed ({})", status);
        Ok(())
    }
}

impl Drop for HeosFile {
    fn drop(&mut self) {
        if self.handle != HEOS_INVALID_HANDLE {
            unsafe {
                heos_fs_close(self.handle);
            }
            self.handle = HEOS_INVALID_HANDLE;
        }
    }
}
