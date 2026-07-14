use std::path::PathBuf;
use std::fs;
use serde::{Serialize, Deserialize};

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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStore {
    pub entries: Vec<SessionEntry>,
}

impl SessionStore {
    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(path)?;
        let store = serde_json::from_str(&data)?;
        Ok(store)
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)?;
        Ok(())
    }
}
