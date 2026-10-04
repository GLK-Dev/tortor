use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::fsutil;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TorrentSource {
    File(PathBuf),
    Magnet(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub source: TorrentSource,
    pub output_dir: PathBuf,
    pub is_paused: bool,
    /// Which files of a multi-file torrent are downloaded; `None` means all.
    #[serde(default)]
    pub selected_files: Option<Vec<bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStore {
    pub entries: Vec<SessionEntry>,
    /// Global speed limits in KiB/s; 0 means unlimited.
    #[serde(default)]
    pub download_limit_kib: u64,
    #[serde(default)]
    pub upload_limit_kib: u64,
}

impl SessionStore {
    /// Loads the store from the per-user config directory. A `session.json`
    /// left in the working directory by older versions is migrated once.
    pub fn load_default() -> Self {
        let path = fsutil::session_file();
        let legacy = PathBuf::from("session.json");

        if !path.exists() && legacy.exists() {
            if let Ok(store) = Self::load(&legacy) {
                if store.save(&path).is_ok() {
                    let _ = fs::remove_file(&legacy);
                }
                return store;
            }
        }
        Self::load(&path).unwrap_or_default()
    }

    pub fn save_default(&self) -> anyhow::Result<()> {
        self.save(&fsutil::session_file())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        fsutil::atomic_write(path, data.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults_for_old_files() {
        let dir = std::env::temp_dir().join(format!("tortor-session-{}", std::process::id()));
        let path = dir.join("session.json");

        let store = SessionStore {
            entries: vec![SessionEntry {
                source: TorrentSource::Magnet("magnet:?xt=urn:btih:abc".into()),
                output_dir: PathBuf::from("downloads"),
                is_paused: true,
                selected_files: Some(vec![true, false]),
            }],
            download_limit_kib: 512,
            upload_limit_kib: 64,
        };
        store.save(&path).unwrap();
        let loaded = SessionStore::load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.download_limit_kib, 512);
        assert_eq!(loaded.entries[0].selected_files, Some(vec![true, false]));

        // A file written before limits existed still loads.
        fs::write(&path, r#"{"entries":[]}"#).unwrap();
        let old = SessionStore::load(&path).unwrap();
        assert_eq!(old.upload_limit_kib, 0);

        let _ = fs::remove_dir_all(&dir);
    }
}
