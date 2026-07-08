#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub announce: String,
    pub name: String,
    pub piece_length: u32,
    pub pieces_count: u32,
    pub total_length: Option<u64>,
    pub info_hash: [u8; 20],
}

impl TorrentMeta {
    pub fn new(
        announce: impl Into<String>,
        name: impl Into<String>,
        piece_length: u32,
        pieces_count: u32,
        total_length: Option<u64>,
        info_hash: [u8; 20],
    ) -> Self {
        Self {
            announce: announce.into(),
            name: name.into(),
            piece_length,
            pieces_count,
            total_length,
            info_hash,
        }
    }

    pub fn info_hash_hex(&self) -> String {
        hex::encode(self.info_hash)
    }
}
