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
    InvalidBencode(&'static str),
    MissingInfoDictionary,
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
            Self::InvalidBencode(msg) => write!(f, "Invalid bencode layout: {msg}"),
            Self::MissingInfoDictionary => {
                write!(f, "Torrent does not contain top-level info dictionary")
            }
        }
    }
}

impl Error for TorrentParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Decode(err) => Some(err),
            Self::InvalidPiecesLength(_) => None,
            Self::InvalidBencode(_) => None,
            Self::MissingInfoDictionary => None,
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
    let info_slice = extract_info_dictionary_slice(bytes)?;
    let info_hash = crate::crypto::core::hash_sha1(info_slice);

    let pieces_len = raw.info.pieces.len();
    if pieces_len % 20 != 0 {
        return Err(TorrentParseError::InvalidPiecesLength(pieces_len));
    }

    let mut pieces = Vec::with_capacity(pieces_len / 20);
    for chunk in raw.info.pieces.chunks_exact(20) {
        let mut hash = [0u8; 20];
        hash.copy_from_slice(chunk);
        pieces.push(hash);
    }

    let pieces_count = (pieces_len / 20) as u32;

    Ok(TorrentMeta::new(
        raw.announce,
        raw.info.name,
        raw.info.piece_length,
        pieces_count,
        pieces,
        raw.info.length,
        info_hash,
    ))
}

pub fn parse_torrent_file(path: impl AsRef<Path>) -> Result<TorrentMeta, TorrentParseError> {
    let bytes = std::fs::read(path)?;
    parse_torrent_bytes(&bytes)
}

fn extract_info_dictionary_slice(bytes: &[u8]) -> Result<&[u8], TorrentParseError> {
    if bytes.first().copied() != Some(b'd') {
        return Err(TorrentParseError::InvalidBencode(
            "top-level value must be a dictionary",
        ));
    }

    let mut index = 1usize;
    while index < bytes.len() {
        if bytes[index] == b'e' {
            break;
        }

        let (key, next_index) = parse_byte_string(bytes, index)?;
        index = next_index;

        let value_start = index;
        let value_end = skip_bencode_value(bytes, index)?;

        if key == b"info" {
            return Ok(&bytes[value_start..value_end]);
        }

        index = value_end;
    }

    Err(TorrentParseError::MissingInfoDictionary)
}

fn parse_byte_string(bytes: &[u8], start: usize) -> Result<(&[u8], usize), TorrentParseError> {
    if start >= bytes.len() || !bytes[start].is_ascii_digit() {
        return Err(TorrentParseError::InvalidBencode(
            "expected bencode byte string length prefix",
        ));
    }

    let mut index = start;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }

    if index >= bytes.len() || bytes[index] != b':' {
        return Err(TorrentParseError::InvalidBencode(
            "missing ':' after byte string length",
        ));
    }

    let len_str = std::str::from_utf8(&bytes[start..index]).map_err(|_| {
        TorrentParseError::InvalidBencode("byte string length is not valid UTF-8 digits")
    })?;
    let len = len_str
        .parse::<usize>()
        .map_err(|_| TorrentParseError::InvalidBencode("byte string length parse failed"))?;

    let content_start = index + 1;
    let content_end = content_start
        .checked_add(len)
        .ok_or(TorrentParseError::InvalidBencode("byte string length overflow"))?;

    if content_end > bytes.len() {
        return Err(TorrentParseError::InvalidBencode(
            "byte string exceeds input length",
        ));
    }

    Ok((&bytes[content_start..content_end], content_end))
}

fn skip_bencode_value(bytes: &[u8], start: usize) -> Result<usize, TorrentParseError> {
    if start >= bytes.len() {
        return Err(TorrentParseError::InvalidBencode(
            "unexpected end of input while reading value",
        ));
    }

    match bytes[start] {
        b'i' => {
            let mut index = start + 1;
            while index < bytes.len() && bytes[index] != b'e' {
                index += 1;
            }
            if index >= bytes.len() {
                return Err(TorrentParseError::InvalidBencode(
                    "unterminated integer value",
                ));
            }
            Ok(index + 1)
        }
        b'l' => {
            let mut index = start + 1;
            while index < bytes.len() && bytes[index] != b'e' {
                index = skip_bencode_value(bytes, index)?;
            }
            if index >= bytes.len() {
                return Err(TorrentParseError::InvalidBencode("unterminated list value"));
            }
            Ok(index + 1)
        }
        b'd' => {
            let mut index = start + 1;
            while index < bytes.len() && bytes[index] != b'e' {
                let (_, key_end) = parse_byte_string(bytes, index)?;
                index = skip_bencode_value(bytes, key_end)?;
            }
            if index >= bytes.len() {
                return Err(TorrentParseError::InvalidBencode(
                    "unterminated dictionary value",
                ));
            }
            Ok(index + 1)
        }
        b'0'..=b'9' => {
            let (_, end) = parse_byte_string(bytes, start)?;
            Ok(end)
        }
        _ => Err(TorrentParseError::InvalidBencode(
            "unknown bencode type prefix",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_file_torrent_minimal() {
        let bytes = b"d8:announce14:http://tracker4:infod6:lengthi12345e4:name8:test.bin12:piece lengthi16384e6:pieces20:12345678901234567890ee";
        let expected_info = b"d6:lengthi12345e4:name8:test.bin12:piece lengthi16384e6:pieces20:12345678901234567890e";

        let parsed = parse_torrent_bytes(bytes).expect("must parse valid torrent");
        assert_eq!(parsed.announce, "http://tracker");
        assert_eq!(parsed.name, "test.bin");
        assert_eq!(parsed.piece_length, 16384);
        assert_eq!(parsed.pieces_count, 1);
        assert_eq!(parsed.pieces.len(), 1);
        assert_eq!(parsed.piece_hash(0), Some(*b"12345678901234567890"));
        assert_eq!(parsed.piece_len_at(0), Some(12345));
        assert_eq!(parsed.total_length, Some(12345));
        assert_eq!(parsed.info_hash, crate::crypto::core::hash_sha1(expected_info));
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
