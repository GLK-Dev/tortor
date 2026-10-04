#[derive(Debug, Clone)]
pub struct TorrentFile {
    pub length: u64,
    pub path: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub announce: String,
    pub name: String,
    pub piece_length: u32,
    pub pieces_count: u32,
    pub pieces: Vec<[u8; 20]>,
    pub total_length: Option<u64>,
    pub files: Option<Vec<TorrentFile>>,
    pub info_hash: [u8; 20],
}

impl TorrentMeta {
    pub fn new(
        announce: impl Into<String>,
        name: impl Into<String>,
        piece_length: u32,
        pieces_count: u32,
        pieces: Vec<[u8; 20]>,
        total_length: Option<u64>,
        files: Option<Vec<TorrentFile>>,
        info_hash: [u8; 20],
    ) -> Self {
        let total_length = total_length.or_else(|| {
            files
                .as_ref()
                .map(|fs| fs.iter().fold(0u64, |acc, f| acc.saturating_add(f.length)))
        });

        Self {
            announce: announce.into(),
            name: name.into(),
            piece_length,
            pieces_count,
            pieces,
            total_length,
            files,
            info_hash,
        }
    }

    pub fn info_hash_hex(&self) -> String {
        hex::encode(self.info_hash)
    }

    pub fn piece_hash(&self, index: usize) -> Option<[u8; 20]> {
        self.pieces.get(index).copied()
    }

    pub fn piece_len_at(&self, index: usize) -> Option<u32> {
        if index >= self.pieces.len() {
            return None;
        }

        if let Some(total) = self.total_length {
            let piece_len = self.piece_length as u64;
            let start = (index as u64).checked_mul(piece_len)?;
            if start >= total {
                return None;
            }

            let remaining = total - start;
            return Some(std::cmp::min(piece_len, remaining) as u32);
        }

        Some(self.piece_length)
    }
}

/// Maximum accepted length of a single path component.
const MAX_COMPONENT_LEN: usize = 255;

const WINDOWS_RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validates one path component coming from untrusted torrent metadata and
/// returns a name that is safe to join onto the download directory.
///
/// Components that could escape the directory (`..`, separators, NUL, empty)
/// are rejected on every platform. On Windows, characters and device names
/// that the filesystem forbids are replaced instead of rejected.
pub fn sanitize_path_component(component: &str) -> Result<String, String> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(format!("invalid path component {component:?}"));
    }
    if component.len() > MAX_COMPONENT_LEN {
        return Err("path component is too long".to_string());
    }
    if component.contains(['/', '\\', '\0']) {
        return Err(format!(
            "path component {component:?} contains a separator or NUL"
        ));
    }

    if cfg!(windows) {
        let mut cleaned: String = component
            .chars()
            .map(|c| {
                if c.is_control() || "<>:\"|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        while cleaned.ends_with('.') || cleaned.ends_with(' ') {
            cleaned.pop();
        }
        if cleaned.is_empty() {
            return Err(format!("invalid path component {component:?}"));
        }
        let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
        if WINDOWS_RESERVED_NAMES.contains(&stem.as_str()) {
            cleaned.insert(0, '_');
        }
        return Ok(cleaned);
    }

    Ok(component.to_string())
}

/// Builds the on-disk path of a torrent file below `base_dir`.
pub fn build_file_path(
    base_dir: &std::path::Path,
    name: &str,
    is_multi: bool,
    parts: &[String],
) -> Result<std::path::PathBuf, String> {
    if parts.is_empty() {
        return Err("file entry has an empty path".to_string());
    }

    let mut path = base_dir.to_path_buf();
    if is_multi {
        path.push(sanitize_path_component(name)?);
    }
    for part in parts {
        path.push(sanitize_path_component(part)?);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_traversal_components() {
        for bad in ["", ".", "..", "a/b", "a\\b", "x\0y"] {
            assert!(
                sanitize_path_component(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn build_path_stays_inside_base() {
        let base = Path::new("downloads");
        let ok =
            build_file_path(base, "dir", true, &["a".to_string(), "b.txt".to_string()]).unwrap();
        assert!(ok.starts_with(base));
        assert!(
            build_file_path(base, "dir", true, &["..".to_string(), "evil".to_string()]).is_err()
        );
        assert!(build_file_path(base, "..", true, &["f".to_string()]).is_err());
        assert!(build_file_path(base, "dir", true, &["C:\\evil".to_string()]).is_err());
        assert!(build_file_path(base, "dir", true, &[]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_are_made_safe() {
        assert_eq!(sanitize_path_component("a:b?.txt").unwrap(), "a_b_.txt");
        assert_eq!(sanitize_path_component("CON.txt").unwrap(), "_CON.txt");
        assert_eq!(sanitize_path_component("name. ").unwrap(), "name");
    }
}
