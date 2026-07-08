use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::Path;

use serde::Deserialize;
use serde_bytes::ByteBuf;

use crate::core::torrent::TorrentMeta;

#[derive(Debug)]
pub enum TorrentParseError {
    Io(std::io::Error),
    Decode(serde_bencode::Error),
    InvalidPiecesLength(usize),
}

impl Display for TorrentParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error while reading torrent: {err}"),
            Self::Decode(err) => write!(f, "Bencode decode error: {err}"),
            Self::InvalidPiecesLength(len) => write!(
                f,
                "Invalid pieces field length: {len}. Length must be a multiple of 20"
            ),
        }
    }
}

impl Error for TorrentParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Decode(err) => Some(err),
            Self::InvalidPiecesLength(_) => None,
        }
    }
}

impl From<std::io::Error> for TorrentParseError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_bencode::Error> for TorrentParseError {
    fn from(value: serde_bencode::Error) -> Self {
        Self::Decode(value)
    }
}

#[derive(Debug, Deserialize)]
struct RawTorrent {
    announce: String,
    info: RawInfo,
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    name: String,
    #[serde(rename = "piece length")]
    piece_length: u32,
    pieces: ByteBuf,
    length: Option<u64>,
}

pub fn parse_torrent_bytes(bytes: &[u8]) -> Result<TorrentMeta, TorrentParseError> {
    let raw: RawTorrent = serde_bencode::from_bytes(bytes)?;

    let pieces_len = raw.info.pieces.len();
    if pieces_len % 20 != 0 {
        return Err(TorrentParseError::InvalidPiecesLength(pieces_len));
    }

    let pieces_count = (pieces_len / 20) as u32;

    Ok(TorrentMeta::new(
        raw.announce,
        raw.info.name,
        raw.info.piece_length,
        pieces_count,
        raw.info.length,
    ))
}

pub fn parse_torrent_file(path: impl AsRef<Path>) -> Result<TorrentMeta, TorrentParseError> {
    let bytes = std::fs::read(path)?;
    parse_torrent_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_file_torrent_minimal() {
        let bytes = b"d8:announce14:http://tracker4:infod6:lengthi12345e4:name8:test.bin12:piece lengthi16384e6:pieces20:12345678901234567890ee";

        let parsed = parse_torrent_bytes(bytes).expect("must parse valid torrent");
        assert_eq!(parsed.announce, "http://tracker");
        assert_eq!(parsed.name, "test.bin");
        assert_eq!(parsed.piece_length, 16384);
        assert_eq!(parsed.pieces_count, 1);
        assert_eq!(parsed.total_length, Some(12345));
    }

    #[test]
    fn reject_invalid_pieces_length() {
        let bytes = b"d8:announce14:http://tracker4:infod6:lengthi10e4:name1:a12:piece lengthi16384e6:pieces21:123456789012345678901ee";

        let err = parse_torrent_bytes(bytes).expect_err("must reject invalid pieces");
        match err {
            TorrentParseError::InvalidPiecesLength(21) => {}
            other => panic!("unexpected error: {other}"),
        }
    }
}
