#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub announce: String,
    pub name: String,
    pub piece_length: u32,
    pub pieces_count: u32,
    pub total_length: Option<u64>,
}

impl TorrentMeta {
    pub fn new(
        announce: impl Into<String>,
        name: impl Into<String>,
        piece_length: u32,
        pieces_count: u32,
        total_length: Option<u64>,
    ) -> Self {
        Self {
            announce: announce.into(),
            name: name.into(),
            piece_length,
            pieces_count,
            total_length,
        }
    }
}
