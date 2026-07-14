use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::core::command::CoreMessage;
use crate::core::coordinator::CoordinatorMsg;
use crate::net::probe;
use crate::net::tracker;

const MAX_ACTIVE_PEERS: usize = 30;
const SWARM_TICK_SECS: u64 = 5;
const PEER_IDLE_TIMEOUT_SECS: u64 = 60;
const PEER_THRESHOLD: usize = 5;
const MIN_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub enum SwarmEvent {
    PeerProgress(SocketAddr, u32),
    PeerExited(SocketAddr),
    TrackerPeersReceived(Vec<SocketAddr>),
    TrackerAnnounceFailed(String),
    PexPeersReceived(Vec<SocketAddr>),
}

struct ActivePeer {
    downloaded_bytes: u64,
    last_progress: Instant,
    handle: JoinHandle<()>,
}

struct SwarmState {
    last_announce: Option<Instant>,
    announce_in_progress: bool,
    tracker_url: String,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    listen_port: u16,
    left_hint: u64,
}

pub async fn run_swarm_manager(
    mut available_peers: VecDeque<SocketAddr>,
    tracker_url: String,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    listen_port: u16,
    left_hint: u64,
    expected_hashes: Arc<Vec<[u8; 20]>>,
    piece_length: u32,
    total_length: Option<u64>,
    ui_sender: mpsc::Sender<CoreMessage>,
    coord_sender: mpsc::Sender<CoordinatorMsg>,
    shutdown_tx: broadcast::Sender<()>,
    announce_tx: broadcast::Sender<crate::core::command::SessionEvent>,
) {
    let mut active: HashMap<SocketAddr, ActivePeer> = HashMap::new();
    let mut tick = interval(Duration::from_secs(SWARM_TICK_SECS));
    let mut pex_tick = interval(Duration::from_secs(60));
    let mut shutdown_rx = shutdown_tx.subscribe();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwarmEvent>();
    let mut swarm_state = SwarmState {
        last_announce: None,
        announce_in_progress: false,
        tracker_url,
        info_hash,
        peer_id,
        listen_port,
        left_hint,
    };

    let _ = ui_sender
        .send(CoreMessage::Status(format!(
            "Swarm started: target {} active peers",
            MAX_ACTIVE_PEERS
        )))
        .await;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }
            maybe_event = event_rx.recv() => {
                if let Some(event) = maybe_event {
                    match event {
                        SwarmEvent::PeerProgress(addr, delta) => {
                            if let Some(peer) = active.get_mut(&addr) {
                                peer.downloaded_bytes = peer.downloaded_bytes.saturating_add(delta as u64);
                                peer.last_progress = Instant::now();
                            }
                        }
                        SwarmEvent::PeerExited(addr) => {
                            active.remove(&addr);
                        }
                        SwarmEvent::TrackerPeersReceived(addrs) => {
                            swarm_state.announce_in_progress = false;
                            let mut added = 0usize;

                            for addr in addrs {
                                if active.contains_key(&addr) || available_peers.contains(&addr) {
                                    continue;
                                }
                                available_peers.push_back(addr);
                                added += 1;
                            }

                            info!("tracker re-announce added {} peers", added);
                            let _ = ui_sender
                                .send(CoreMessage::Status(format!(
                                    "Re-announce added {} peers | queued: {}",
                                    added,
                                    available_peers.len()
                                )))
                                .await;
                        }
                        SwarmEvent::PexPeersReceived(addrs) => {
                            let mut added = 0usize;
                            for addr in addrs {
                                if active.contains_key(&addr) || available_peers.contains(&addr) {
                                    continue;
                                }
                                available_peers.push_back(addr);
                                added += 1;
                            }
                            if added > 0 {
                                info!("PEX discovered {} new peers", added);
                                let _ = ui_sender
                                    .send(CoreMessage::Status(format!(
                                        "PEX added {} peers | queued: {}",
                                        added,
                                        available_peers.len()
                                    )))
                                    .await;
                            }
                        }
                        SwarmEvent::TrackerAnnounceFailed(err_msg) => {
                            swarm_state.announce_in_progress = false;
                            error!("tracker re-announce failed: {}", err_msg);
                            let _ = ui_sender
                                .send(CoreMessage::Status(format!(
                                    "Re-announce failed: {}",
                                    err_msg
                                )))
                                .await;
                        }
                    }
                }
            }

            _ = pex_tick.tick() => {
                let current_peers: Vec<SocketAddr> = active.keys().copied().collect();
                if !current_peers.is_empty() {
                    let _ = announce_tx.send(crate::core::command::SessionEvent::ActivePeersSnapshot(current_peers));
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let mut expired = Vec::new();

                for (addr, peer) in &active {
                    if now.duration_since(peer.last_progress).as_secs() > PEER_IDLE_TIMEOUT_SECS {
                        expired.push(*addr);
                    }
                }

                for addr in expired {
                    if let Some(peer) = active.remove(&addr) {
                        peer.handle.abort();
                        warn!("dropping idle peer {}", addr);
                        let _ = ui_sender
                            .send(CoreMessage::ProbeFailed(
                                addr,
                                format!("dropped by swarm after {}s without progress", PEER_IDLE_TIMEOUT_SECS),
                            ))
                            .await;
                    }
                }

                while active.len() < MAX_ACTIVE_PEERS {
                    let Some(addr) = available_peers.pop_front() else {
                        break;
                    };

                    if active.contains_key(&addr) {
                        continue;
                    }

                    let _ = ui_sender.send(CoreMessage::ProbeQueued(addr)).await;

                    let expected_hashes_cloned = Arc::clone(&expected_hashes);
                    let ui_sender_cloned = ui_sender.clone();
                    let coord_sender_cloned = coord_sender.clone();
                    let event_tx_cloned = event_tx.clone();
                    let local_shutdown_rx = shutdown_tx.subscribe();
                    let local_announce_rx = announce_tx.subscribe();

                    let handle = tokio::spawn(async move {
                        let _ = ui_sender_cloned.send(CoreMessage::ProbeStarted(addr)).await;
                        let result = probe::execute_probe(
                            addr,
                            info_hash,
                            peer_id,
                            expected_hashes_cloned,
                            piece_length,
                            total_length,
                            ui_sender_cloned.clone(),
                            coord_sender_cloned,
                            local_shutdown_rx,
                            Some(event_tx_cloned.clone()),
                            local_announce_rx,
                        )
                        .await;

                        match result {
                            Ok(status) => {
                                let _ = ui_sender_cloned
                                    .send(CoreMessage::ProbeSucceeded(addr, status))
                                    .await;
                            }
                            Err(err) => {
                                let _ = ui_sender_cloned
                                    .send(CoreMessage::ProbeFailed(addr, err.to_string()))
                                    .await;
                            }
                        }

                        let _ = event_tx_cloned.send(SwarmEvent::PeerExited(addr));
                    });

                    active.insert(
                        addr,
                        ActivePeer {
                            downloaded_bytes: 0,
                            last_progress: Instant::now(),
                            handle,
                        },
                    );
                }

                let _ = ui_sender
                    .send(CoreMessage::Status(format!(
                        "Swarm active: {} | queued left: {}",
                        active.len(),
                        available_peers.len()
                    )))
                    .await;
            }
        }

        if should_reannounce(&swarm_state, available_peers.len(), active.len()) {
            start_reannounce(&mut swarm_state, event_tx.clone());
            let _ = ui_sender
                .send(CoreMessage::Status(format!(
                    "Peer queue low ({}). Running re-announce...",
                    available_peers.len()
                )))
                .await;
        }
    }

    for (_, peer) in active {
        peer.handle.abort();
    }

    let _ = ui_sender
        .send(CoreMessage::Status("Swarm manager stopped".to_string()))
        .await;
}

fn should_reannounce(state: &SwarmState, available_len: usize, active_len: usize) -> bool {
    if state.announce_in_progress {
        return false;
    }

    if available_len >= PEER_THRESHOLD && active_len >= PEER_THRESHOLD {
        return false;
    }

    state
        .last_announce
        .map(|t| t.elapsed() >= MIN_ANNOUNCE_INTERVAL)
        .unwrap_or(true)
}

fn start_reannounce(state: &mut SwarmState, event_tx: mpsc::UnboundedSender<SwarmEvent>) {
    state.announce_in_progress = true;
    state.last_announce = Some(Instant::now());

    let tracker_url = state.tracker_url.clone();
    let info_hash = state.info_hash;
    let peer_id = state.peer_id;
    let listen_port = state.listen_port;
    let left_hint = state.left_hint;

    tokio::spawn(async move {
        match tracker::announce(&tracker_url, &info_hash, &peer_id, listen_port, left_hint).await {
            Ok(peers) => {
                let addrs = peers.into_iter().map(|p| p.addr).collect();
                let _ = event_tx.send(SwarmEvent::TrackerPeersReceived(addrs));
            }
            Err(err) => {
                let _ = event_tx.send(SwarmEvent::TrackerAnnounceFailed(err.to_string()));
            }
        }
    });
}
