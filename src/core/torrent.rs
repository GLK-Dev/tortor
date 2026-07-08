#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub announce: String,
    pub piece_length: u32,
}

impl TorrentMeta {
    pub fn new(announce: impl Into<String>, piece_length: u32) -> Self {
        Self {
            announce: announce.into(),
            piece_length,
        }
    }
}
