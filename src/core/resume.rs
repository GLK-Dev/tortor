use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::core::bitfield::Bitfield;
use crate::core::fsutil;
use crate::core::manager::TorrentManager;

const MAGIC: &[u8; 4] = b"TTRS";
const FORMAT_VERSION: u8 = 2;
const HEADER_LEN: usize = 4 + 1 + 4;
/// Sanity bound: a torrent with more pieces than this is not plausible.
const MAX_PIECES: u32 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastResumeState {
    pub version: u8,
    pub total_pieces: u32,
    pub completed: Vec<u32>,
}

impl FastResumeState {
    pub fn from_manager(manager: &TorrentManager) -> Self {
        Self {
            version: FORMAT_VERSION,
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

    /// Compact binary form: magic, version, piece count, then a piece bitfield.
    pub fn encode(&self) -> Vec<u8> {
        let bitfield =
            Bitfield::from_indices(self.total_pieces as usize, self.completed.iter().copied());
        let mut out = Vec::with_capacity(HEADER_LEN + bitfield.as_bytes().len());
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&self.total_pieces.to_le_bytes());
        out.extend_from_slice(bitfield.as_bytes());
        out
    }

    /// Reads the binary format, or the JSON written by older versions.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(MAGIC) {
            if bytes.len() < HEADER_LEN {
                bail!("resume file is truncated");
            }
            if bytes[4] != FORMAT_VERSION {
                bail!("unsupported resume format version {}", bytes[4]);
            }
            let total_pieces = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
            if total_pieces > MAX_PIECES {
                bail!("resume file claims {total_pieces} pieces");
            }
            let bitfield = Bitfield::from_wire(total_pieces as usize, &bytes[HEADER_LEN..])
                .context("resume bitfield has the wrong size")?;
            return Ok(Self {
                version: FORMAT_VERSION,
                total_pieces,
                completed: bitfield.iter_ones().map(|i| i as u32).collect(),
            });
        }

        serde_json::from_slice(bytes).context("resume file is neither binary nor JSON")
    }
}

pub async fn load_fastresume(path: &Path) -> Result<Option<FastResumeState>> {
    let content = match fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read fastresume file {}", path.display()))
        }
    };

    FastResumeState::decode(&content)
        .with_context(|| format!("failed to parse fastresume file {}", path.display()))
        .map(Some)
}

pub async fn save_fastresume(path: &Path, state: &FastResumeState) -> Result<()> {
    fsutil::atomic_write_async(path.to_path_buf(), state.encode())
        .await
        .with_context(|| format!("failed to write fastresume {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_roundtrip_keeps_completed_pieces() {
        let state = FastResumeState {
            version: FORMAT_VERSION,
            total_pieces: 21,
            completed: vec![0, 3, 7, 8, 20],
        };
        let bytes = state.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 3);

        let decoded = FastResumeState::decode(&bytes).unwrap();
        assert_eq!(decoded.total_pieces, 21);
        assert_eq!(decoded.completed, vec![0, 3, 7, 8, 20]);
    }

    #[test]
    fn legacy_json_is_still_readable() {
        let json = br#"{"version":1,"total_pieces":4,"completed":[1,2]}"#;
        let decoded = FastResumeState::decode(json).unwrap();
        assert_eq!(decoded.completed, vec![1, 2]);
    }

    #[test]
    fn corrupted_files_are_rejected() {
        let mut bytes = FastResumeState {
            version: FORMAT_VERSION,
            total_pieces: 16,
            completed: vec![1],
        }
        .encode();
        assert!(FastResumeState::decode(&bytes[..6]).is_err());
        bytes.pop();
        assert!(FastResumeState::decode(&bytes).is_err());
        assert!(FastResumeState::decode(b"garbage").is_err());

        let mut huge = MAGIC.to_vec();
        huge.push(FORMAT_VERSION);
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(FastResumeState::decode(&huge).is_err());
    }

    #[tokio::test]
    async fn save_then_load() {
        let path =
            std::env::temp_dir().join(format!("tortor-resume-{}.fastresume", std::process::id()));
        let state = FastResumeState {
            version: FORMAT_VERSION,
            total_pieces: 9,
            completed: vec![2, 4],
        };
        save_fastresume(&path, &state).await.unwrap();
        let loaded = load_fastresume(&path).await.unwrap().unwrap();
        assert_eq!(loaded.completed, vec![2, 4]);
        let _ = std::fs::remove_file(&path);
        assert!(load_fastresume(&path).await.unwrap().is_none());
    }
}
