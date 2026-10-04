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
    InvalidTorrent(String),
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
            Self::InvalidTorrent(msg) => write!(f, "Invalid torrent: {msg}"),
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
            Self::InvalidTorrent(_) => None,
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
struct RawFile {
    length: u64,
    path: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    name: String,
    #[serde(rename = "piece length")]
    piece_length: u32,
    pieces: ByteBuf,
    length: Option<u64>,
    files: Option<Vec<RawFile>>,
}

/// Upper bound for a .torrent file or ut_metadata blob.
pub const MAX_TORRENT_SIZE: usize = 16 * 1024 * 1024;
/// Maximum container nesting accepted in any bencoded input.
pub const MAX_BENCODE_DEPTH: usize = 64;
const MAX_PIECE_LENGTH: u32 = 128 * 1024 * 1024;

/// Iteratively verifies that `bytes` does not nest lists/dictionaries deeper
/// than `MAX_BENCODE_DEPTH`. Run it before handing untrusted input to a
/// recursive decoder.
pub fn check_bencode_depth(bytes: &[u8]) -> Result<(), TorrentParseError> {
    let mut depth = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'l' | b'd' => {
                depth += 1;
                if depth > MAX_BENCODE_DEPTH {
                    return Err(TorrentParseError::InvalidBencode(
                        "bencode nesting is too deep",
                    ));
                }
                index += 1;
            }
            b'e' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'i' => {
                while index < bytes.len() && bytes[index] != b'e' {
                    index += 1;
                }
                index += 1;
            }
            b'0'..=b'9' => {
                let mut len = 0usize;
                while index < bytes.len() && bytes[index].is_ascii_digit() {
                    len = len
                        .saturating_mul(10)
                        .saturating_add((bytes[index] - b'0') as usize);
                    index += 1;
                }
                // Skip ':' and the string body; saturate so malformed lengths end the scan.
                index = index.saturating_add(1).saturating_add(len);
            }
            _ => return Ok(()),
        }
    }

    Ok(())
}

fn invalid(msg: impl Into<String>) -> TorrentParseError {
    TorrentParseError::InvalidTorrent(msg.into())
}

fn build_meta(
    announce: String,
    info: RawInfo,
    info_hash: [u8; 20],
) -> Result<TorrentMeta, TorrentParseError> {
    let pieces_len = info.pieces.len();
    if !pieces_len.is_multiple_of(20) {
        return Err(TorrentParseError::InvalidPiecesLength(pieces_len));
    }

    if info.piece_length == 0 || info.piece_length > MAX_PIECE_LENGTH {
        return Err(invalid(format!(
            "unsupported piece length {}",
            info.piece_length
        )));
    }

    let name = crate::core::torrent::sanitize_path_component(&info.name).map_err(invalid)?;

    let total_length = match (info.length, &info.files) {
        (Some(length), None) => length,
        (None, Some(files)) => {
            if files.is_empty() {
                return Err(invalid("multi-file torrent has no files"));
            }
            let mut total = 0u64;
            for file in files {
                crate::core::torrent::build_file_path(
                    std::path::Path::new(""),
                    &name,
                    true,
                    &file.path,
                )
                .map_err(invalid)?;
                total = total
                    .checked_add(file.length)
                    .ok_or_else(|| invalid("total size overflows u64"))?;
            }
            total
        }
        _ => {
            return Err(invalid(
                "info must contain exactly one of `length` or `files`",
            ))
        }
    };

    if total_length == 0 {
        return Err(invalid("torrent has no data"));
    }

    let piece_count = total_length.div_ceil(info.piece_length as u64);
    if piece_count != (pieces_len / 20) as u64 {
        return Err(invalid(format!(
            "piece count mismatch: size implies {piece_count}, `pieces` holds {}",
            pieces_len / 20
        )));
    }

    let pieces: Vec<[u8; 20]> = info
        .pieces
        .chunks_exact(20)
        .map(|chunk| {
            let mut hash = [0u8; 20];
            hash.copy_from_slice(chunk);
            hash
        })
        .collect();
    let pieces_count = pieces.len() as u32;

    Ok(TorrentMeta::new(
        announce,
        name,
        info.piece_length,
        pieces_count,
        pieces,
        info.length,
        info.files.map(|files| {
            files
                .into_iter()
                .map(|f| crate::core::torrent::TorrentFile {
                    length: f.length,
                    path: f.path,
                })
                .collect()
        }),
        info_hash,
    ))
}

pub fn parse_torrent_metadata_bytes(
    info_bytes: &[u8],
    expected_info_hash: [u8; 20],
) -> Result<TorrentMeta, TorrentParseError> {
    if info_bytes.len() > MAX_TORRENT_SIZE {
        return Err(TorrentParseError::InvalidBencode("metadata is too large"));
    }
    let actual_hash = crate::crypto::core::hash_sha1(info_bytes);
    if actual_hash != expected_info_hash {
        return Err(TorrentParseError::InvalidBencode(
            "SHA-1 mismatch for metadata",
        ));
    }
    check_bencode_depth(info_bytes)?;

    let raw_info: RawInfo = serde_bencode::from_bytes(info_bytes)?;
    // No announce URL in metadata
    build_meta(String::new(), raw_info, actual_hash)
}

pub fn parse_torrent_bytes(bytes: &[u8]) -> Result<TorrentMeta, TorrentParseError> {
    if bytes.len() > MAX_TORRENT_SIZE {
        return Err(TorrentParseError::InvalidBencode(
            "torrent file is too large",
        ));
    }
    check_bencode_depth(bytes)?;

    let raw: RawTorrent = serde_bencode::from_bytes(bytes)?;
    let info_slice = extract_info_dictionary_slice(bytes)?;
    let info_hash = crate::crypto::core::hash_sha1(info_slice);

    build_meta(raw.announce, raw.info, info_hash)
}

pub fn parse_torrent_file(path: impl AsRef<Path>) -> Result<TorrentMeta, TorrentParseError> {
    let path = path.as_ref();
    if std::fs::metadata(path)?.len() > MAX_TORRENT_SIZE as u64 {
        return Err(TorrentParseError::InvalidBencode(
            "torrent file is too large",
        ));
    }
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
        .ok_or(TorrentParseError::InvalidBencode(
            "byte string length overflow",
        ))?;

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
        assert_eq!(
            parsed.info_hash,
            crate::crypto::core::hash_sha1(expected_info)
        );
    }

    fn torrent_with_info(info: &[u8]) -> Vec<u8> {
        let mut out = b"d8:announce14:http://tracker4:info".to_vec();
        out.extend_from_slice(info);
        out.push(b'e');
        out
    }

    #[test]
    fn rejects_path_traversal_in_multi_file_torrent() {
        let info = b"d5:filesld6:lengthi5e4:pathl2:..5:evil1eee4:name3:dir12:piece lengthi16384e6:pieces20:12345678901234567890e";
        let err = parse_torrent_bytes(&torrent_with_info(info)).expect_err("must reject traversal");
        assert!(matches!(err, TorrentParseError::InvalidTorrent(_)), "{err}");
    }

    #[test]
    fn rejects_traversal_in_name() {
        let info = b"d6:lengthi5e4:name2:..12:piece lengthi16384e6:pieces20:12345678901234567890e";
        assert!(parse_torrent_bytes(&torrent_with_info(info)).is_err());
    }

    #[test]
    fn rejects_zero_piece_length() {
        let info = b"d6:lengthi5e4:name1:a12:piece lengthi0e6:pieces20:12345678901234567890e";
        assert!(matches!(
            parse_torrent_bytes(&torrent_with_info(info)),
            Err(TorrentParseError::InvalidTorrent(_))
        ));
    }

    #[test]
    fn rejects_piece_count_mismatch() {
        let info =
            b"d6:lengthi99999e4:name1:a12:piece lengthi16384e6:pieces20:12345678901234567890e";
        assert!(matches!(
            parse_torrent_bytes(&torrent_with_info(info)),
            Err(TorrentParseError::InvalidTorrent(_))
        ));
    }

    #[test]
    fn rejects_size_overflow() {
        let info = b"d5:filesld6:lengthi9223372036854775807e4:pathl1:aeed6:lengthi9223372036854775807e4:pathl1:beed6:lengthi9223372036854775807e4:pathl1:ceee4:name1:d12:piece lengthi16384e6:pieces20:12345678901234567890e";
        assert!(matches!(
            parse_torrent_bytes(&torrent_with_info(info)),
            Err(TorrentParseError::InvalidTorrent(_))
        ));
    }

    #[test]
    fn rejects_deeply_nested_input() {
        let mut bytes = vec![b'l'; 100_000];
        bytes.extend(std::iter::repeat_n(b'e', 100_000));
        assert!(check_bencode_depth(&bytes).is_err());
        assert!(parse_torrent_bytes(&bytes).is_err());
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
