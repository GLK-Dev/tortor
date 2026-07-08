use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::core::manager::TorrentManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastResumeState {
    pub version: u8,
    pub total_pieces: u32,
    pub completed: Vec<u32>,
}

impl FastResumeState {
    pub fn from_manager(manager: &TorrentManager) -> Self {
        Self {
            version: 1,
            total_pieces: manager.total_pieces,
            completed: manager.completed_pieces(),
        }
    }

    pub fn into_manager(self, fallback_total_pieces: u32) -> TorrentManager {
        let total_pieces = if self.total_pieces == fallback_total_pieces {
            self.total_pieces
        } else {
            fallback_total_pieces
        };

        TorrentManager::from_completed(total_pieces, &self.completed)
    }
}

pub async fn load_fastresume(path: &Path) -> Result<Option<FastResumeState>> {
    let content = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| {
                format!("failed to read fastresume file {}", path.display())
            })
        }
    };

    let state: FastResumeState = serde_json::from_slice(&content)
        .with_context(|| format!("failed to parse fastresume file {}", path.display()))?;

    Ok(Some(state))
}

pub async fn save_fastresume(path: &Path, state: &FastResumeState) -> Result<()> {
    let tmp_path = tmp_path_for(path);
    let bytes = serde_json::to_vec_pretty(state).context("failed to encode fastresume json")?;

    fs::write(&tmp_path, bytes)
        .await
        .with_context(|| format!("failed to write temp fastresume {}", tmp_path.display()))?;

    if fs::metadata(path).await.is_ok() {
        let _ = fs::remove_file(path).await;
    }

    fs::rename(&tmp_path, path)
        .await
        .with_context(|| format!("failed to replace fastresume {}", path.display()))?;

    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let suffix = match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".to_string(),
    };
    tmp.set_extension(suffix);
    tmp
}
