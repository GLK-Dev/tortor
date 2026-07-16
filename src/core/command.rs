use std::net::SocketAddr;

use crate::core::torrent::TorrentMeta;

#[derive(Debug, Clone, Copy)]
pub enum CoreCommand {
    ProbePeer(SocketAddr),
    StartSwarm,
    StopAll,
    Pause,
    Resume,
    Remove(bool),
}

#[derive(Debug, Clone, Default)]
pub struct SessionTelemetry {
    pub in_flight_requests: usize,
    pub retries: u32,
    pub duplicate_blocks: u32,
    pub unexpected_blocks: u32,
    pub downloaded_bytes: u32,
    pub time_to_first_piece_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum CoreMessage {
    Status(String),
    MetadataReady(std::sync::Arc<crate::core::torrent::TorrentMeta>),
    TorrentLoaded(TorrentMeta),
    GlobalProgress(f32),
    DownloadComplete,
    ShutdownComplete,
    PeerFound(SocketAddr),
    TrackerDone(usize),
    ProbeQueued(SocketAddr),
    ProbeStarted(SocketAddr),
    ProbeSucceeded(SocketAddr, String),
    ProbeFailed(SocketAddr, String),
    TelemetryUpdate(SocketAddr, SessionTelemetry),
    BytesTransferred(usize, usize),
    Error(String),
    PausedState(bool),
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    PieceCompleted(u32),
    ActivePeersSnapshot(Vec<std::net::SocketAddr>),
    DownloadComplete,
}
