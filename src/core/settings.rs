// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Persistent user settings and lock-free runtime tuning parameters.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Persistent fine-tuning settings saved inside `session.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClientSettings {
    /// Default download directory for new torrents (None = current working directory / CLI -o).
    pub default_output_dir: Option<PathBuf>,
    /// Start newly added torrents in paused state.
    pub start_paused: bool,
    /// Use compact UI density (0.8x spacing; applies cleanly on restart).
    pub compact_ui: bool,
    /// TCP + QUIC listener port (0 = pick any free port; takes effect on restart).
    pub listen_port: u16,
    /// Enable Kademlia DHT node (takes effect on restart).
    pub enable_dht: bool,
    /// Enable UPnP automatic port forwarding (takes effect on restart).
    pub enable_port_mapping: bool,
    /// Enable Peer Exchange (BEP 11) (applies immediately).
    pub enable_pex: bool,
    /// Enable outbound QUIC/TLS peer dialing alongside TCP (applies immediately).
    pub enable_quic: bool,
    /// Max active peer connections per torrent (1..=250, applies immediately).
    pub max_peers_per_torrent: usize,
    /// Max global inbound peer connections (1..=500, takes effect on restart).
    pub max_inbound_peers: usize,
    /// Max concurrent upload slots per torrent (1..=64, applies immediately).
    pub upload_slots: usize,
    /// Max in-flight 16 KiB block requests per peer (2..=128, applies immediately).
    pub pipeline_depth: usize,
    /// Outbound peer connect/handshake timeout in seconds (2..=30, applies immediately).
    pub connect_timeout_secs: u64,
    /// Fast-resume disk flush interval in seconds (1..=60, applies immediately).
    pub resume_interval_secs: u64,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            default_output_dir: None,
            start_paused: false,
            compact_ui: false,
            listen_port: 6881,
            enable_dht: true,
            enable_port_mapping: true,
            enable_pex: true,
            enable_quic: true,
            max_peers_per_torrent: 30,
            max_inbound_peers: 100,
            upload_slots: 8,
            pipeline_depth: 16,
            connect_timeout_secs: 5,
            resume_interval_secs: 5,
        }
    }
}

impl ClientSettings {
    /// Clamps every numeric parameter to a safe operational range.
    pub fn clamped(mut self) -> Self {
        self.max_peers_per_torrent = self.max_peers_per_torrent.clamp(1, 250);
        self.max_inbound_peers = self.max_inbound_peers.clamp(1, 500);
        self.upload_slots = self.upload_slots.clamp(1, 64);
        self.pipeline_depth = self.pipeline_depth.clamp(2, 128);
        self.connect_timeout_secs = self.connect_timeout_secs.clamp(2, 30);
        self.resume_interval_secs = self.resume_interval_secs.clamp(1, 60);
        if self
            .default_output_dir
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            self.default_output_dir = None;
        }
        self
    }
}

static MAX_ACTIVE_PEERS: AtomicUsize = AtomicUsize::new(30);
static MAX_INBOUND_PEERS: AtomicUsize = AtomicUsize::new(100);
static UPLOAD_SLOTS: AtomicUsize = AtomicUsize::new(8);
static PIPELINE_DEPTH: AtomicUsize = AtomicUsize::new(16);
static CONNECT_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(5);
static RESUME_INTERVAL_SECS: AtomicU64 = AtomicU64::new(5);
static ENABLE_PEX: AtomicBool = AtomicBool::new(true);
static ENABLE_QUIC: AtomicBool = AtomicBool::new(true);

/// Applies validated runtime-tunable settings immediately without restarting swarms.
pub fn apply_runtime_settings(settings: &ClientSettings) {
    let s = settings.clone().clamped();
    MAX_ACTIVE_PEERS.store(s.max_peers_per_torrent, Ordering::Relaxed);
    MAX_INBOUND_PEERS.store(s.max_inbound_peers, Ordering::Relaxed);
    UPLOAD_SLOTS.store(s.upload_slots, Ordering::Relaxed);
    PIPELINE_DEPTH.store(s.pipeline_depth, Ordering::Relaxed);
    CONNECT_TIMEOUT_SECS.store(s.connect_timeout_secs, Ordering::Relaxed);
    RESUME_INTERVAL_SECS.store(s.resume_interval_secs, Ordering::Relaxed);
    ENABLE_PEX.store(s.enable_pex, Ordering::Relaxed);
    ENABLE_QUIC.store(s.enable_quic, Ordering::Relaxed);
}

pub fn max_active_peers() -> usize {
    MAX_ACTIVE_PEERS.load(Ordering::Relaxed).clamp(1, 250)
}

pub fn max_inbound_peers() -> usize {
    MAX_INBOUND_PEERS.load(Ordering::Relaxed).clamp(1, 500)
}

pub fn upload_slots() -> usize {
    UPLOAD_SLOTS.load(Ordering::Relaxed).clamp(1, 64)
}

pub fn pipeline_depth() -> usize {
    PIPELINE_DEPTH.load(Ordering::Relaxed).clamp(2, 128)
}

pub fn connect_timeout() -> Duration {
    Duration::from_secs(CONNECT_TIMEOUT_SECS.load(Ordering::Relaxed).clamp(2, 30))
}

pub fn resume_interval() -> Duration {
    Duration::from_secs(RESUME_INTERVAL_SECS.load(Ordering::Relaxed).clamp(1, 60))
}

pub fn pex_enabled() -> bool {
    ENABLE_PEX.load(Ordering::Relaxed)
}

pub fn quic_enabled() -> bool {
    ENABLE_QUIC.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_applies_runtime_settings() {
        let s = ClientSettings {
            max_peers_per_torrent: 0,
            max_inbound_peers: 9999,
            upload_slots: 0,
            pipeline_depth: 1,
            connect_timeout_secs: 100,
            resume_interval_secs: 0,
            enable_pex: false,
            enable_quic: false,
            ..ClientSettings::default()
        }
        .clamped();

        assert_eq!(s.max_peers_per_torrent, 1);
        assert_eq!(s.max_inbound_peers, 500);
        assert_eq!(s.upload_slots, 1);
        assert_eq!(s.pipeline_depth, 2);
        assert_eq!(s.connect_timeout_secs, 30);
        assert_eq!(s.resume_interval_secs, 1);

        apply_runtime_settings(&s);
        assert_eq!(max_active_peers(), 1);
        assert_eq!(max_inbound_peers(), 500);
        assert_eq!(upload_slots(), 1);
        assert_eq!(pipeline_depth(), 2);
        assert_eq!(connect_timeout(), Duration::from_secs(30));
        assert_eq!(resume_interval(), Duration::from_secs(1));
        assert!(!pex_enabled());
        assert!(!quic_enabled());

        // Restore defaults for parallel tests
        apply_runtime_settings(&ClientSettings::default());
    }
}