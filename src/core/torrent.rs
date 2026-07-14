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
            files.as_ref().map(|fs| fs.iter().map(|f| f.length).sum())
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
