//! Filesystem helpers: crash-safe writes and per-user application directories.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

fn app_dir(base: Option<PathBuf>) -> PathBuf {
    base.unwrap_or_else(|| PathBuf::from(".")).join("TorTor")
}

/// Per-user directory for settings and the session list.
pub fn config_dir() -> PathBuf {
    app_dir(dirs::config_dir())
}

/// Per-user directory for resume data.
pub fn data_dir() -> PathBuf {
    app_dir(dirs::data_dir())
}

pub fn session_file() -> PathBuf {
    config_dir().join("session.json")
}

pub fn resume_file(info_hash: &[u8; 20]) -> PathBuf {
    data_dir()
        .join("resume")
        .join(format!("{}.fastresume", hex::encode(info_hash)))
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Writes `bytes` to a temporary sibling, flushes it to disk and renames it
/// over `path`, so a crash leaves either the old or the new file, never a
/// truncated one.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = tmp_sibling(path);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub async fn atomic_write_async(path: PathBuf, bytes: Vec<u8>) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || atomic_write(&path, &bytes))
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tortor-fsutil-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn atomic_write_creates_parents_and_replaces() {
        let dir = temp_dir("replace");
        let file = dir.join("nested").join("state.bin");

        atomic_write(&file, b"first").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"first");
        atomic_write(&file, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second, longer");
        assert!(!tmp_sibling(&file).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_keeps_the_old_file() {
        let dir = temp_dir("fail");
        let file = dir.join("state.bin");
        atomic_write(&file, b"good").unwrap();

        // A directory in the way of the temporary file makes the write fail.
        std::fs::create_dir(tmp_sibling(&file)).unwrap();
        assert!(atomic_write(&file, b"bad").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"good");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_file_is_named_after_the_info_hash() {
        let path = resume_file(&[0xAB; 20]);
        assert!(path
            .to_string_lossy()
            .ends_with(&format!("{}.fastresume", "ab".repeat(20))));
    }
}
