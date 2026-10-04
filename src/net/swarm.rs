use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::{error, info, warn};

use crate::core::command::CoreMessage;
use crate::core::coordinator::CoordinatorMsg;
use crate::net::engine::{Engine, TorrentRegistration};
use crate::net::probe;
use crate::net::session::{PeerContext, TransferStats, UploadSlots, MAX_UPLOAD_SLOTS};
use crate::net::tracker;

const MAX_ACTIVE_PEERS: usize = 30;
const SWARM_TICK_SECS: u64 = 5;
/// Backstop only: sessions drop silent or useless peers on their own.
const PEER_IDLE_TIMEOUT_SECS: u64 = 300;
const MAX_QUEUED_PEERS: usize = 2000;
const PEER_THRESHOLD: usize = 5;
const MIN_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub enum SwarmEvent {
    PeerProgress(SocketAddr, u32),
    PeerExited(SocketAddr),
    TrackerPeersReceived(Vec<SocketAddr>),
    TrackerAnnounceFailed(String),
    PexPeersReceived(Vec<SocketAddr>),
    DhtPeersReceived(Vec<SocketAddr>),
}

struct ActivePeer {
    downloaded_bytes: u64,
    last_progress: Instant,
    handle: JoinHandle<()>,
}

struct SwarmState {
    last_announce: Option<Instant>,
    announce_in_progress: bool,
    tracker_urls: Vec<String>,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    listen_port: u16,
    left_hint: u64,
    event: Option<String>,
    stats: Arc<TransferStats>,
}

/// Runs the peer swarm of one torrent: dials peers from trackers, DHT and
/// PEX, re-announces, and serves inbound peers the engine routes to it.
pub async fn run_swarm_manager(
    mut available_peers: VecDeque<SocketAddr>,
    tracker_urls: Vec<String>,
    info_hash: [u8; 20],
    left_hint: u64,
    expected_hashes: Arc<Vec<[u8; 20]>>,
    piece_length: u32,
    total_length: Option<u64>,
    ui_sender: mpsc::Sender<CoreMessage>,
    coord_sender: mpsc::Sender<CoordinatorMsg>,
    shutdown_tx: broadcast::Sender<()>,
    announce_tx: broadcast::Sender<crate::core::command::SessionEvent>,
    engine: Arc<Engine>,
) {
    let mut active: HashMap<SocketAddr, ActivePeer> = HashMap::new();
    let mut tick = interval(Duration::from_secs(SWARM_TICK_SECS));
    let mut pex_tick = interval(Duration::from_secs(60));
    let mut shutdown_rx = shutdown_tx.subscribe();
    let mut announce_rx = announce_tx.subscribe();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwarmEvent>();

    let peer_id = engine.peer_id;
    let quic_endpoint = engine.quic_endpoint.clone();

    let peer_ctx = PeerContext {
        expected_hashes,
        piece_length,
        total_length,
        ui_sender: ui_sender.clone(),
        coord_sender,
        swarm_event_tx: Some(event_tx.clone()),
        upload_slots: UploadSlots::new(MAX_UPLOAD_SLOTS),
        stats: Arc::new(TransferStats::default()),
    };
    engine.register(
        info_hash,
        TorrentRegistration {
            ctx: peer_ctx.clone(),
            shutdown_tx: shutdown_tx.clone(),
            announce_tx: announce_tx.clone(),
        },
    );

    if let Some(dht) = &engine.dht {
        let _ = dht
            .send(crate::net::dht::actor::DhtManagerCommand::StartSearch {
                info_hash: crate::net::dht::routing::NodeId(info_hash),
                announce_port: Some(engine.port),
                peers_tx: event_tx.clone(),
            })
            .await;
    }
    let mut swarm_state = SwarmState {
        last_announce: None,
        announce_in_progress: false,
        tracker_urls,
        info_hash,
        peer_id,
        listen_port: engine.port,
        left_hint,
        event: Some("started".to_string()),
        stats: peer_ctx.stats.clone(),
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
                            swarm_state.event = None; // clear event after successful announce
                            let added = enqueue_peers(&mut available_peers, &active, addrs);

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
                            let added = enqueue_peers(&mut available_peers, &active, addrs);
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
                        SwarmEvent::DhtPeersReceived(addrs) => {
                            let added = enqueue_peers(&mut available_peers, &active, addrs);
                            if added > 0 {
                                info!("DHT discovered {} new peers", added);
                                let _ = ui_sender
                                    .send(CoreMessage::Status(format!(
                                        "DHT added {} peers | queued: {}",
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
            Ok(event) = announce_rx.recv() => {
                match event {
                    crate::core::command::SessionEvent::ActivePeersSnapshot(_) => {} // Handled elsewhere or not needed here
                    crate::core::command::SessionEvent::PieceCompleted(_) => {}
                    crate::core::command::SessionEvent::DownloadComplete => {
                        tracing::info!("Swarm received DownloadComplete. Forcing tracker announce with event=completed");
                        swarm_state.event = Some("completed".to_string());
                        swarm_state.left_hint = 0;
                        start_reannounce(&mut swarm_state, event_tx.clone());
                    }
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

                    let ctx = peer_ctx.clone();
                    let quic_endpoint_cloned = quic_endpoint.clone();
                    let event_tx_cloned = event_tx.clone();
                    let local_shutdown_rx = shutdown_tx.subscribe();
                    let local_announce_rx = announce_tx.subscribe();
                    let ui_sender_cloned = ui_sender.clone();

                    let handle = tokio::spawn(async move {
                        let _ = ui_sender_cloned.send(CoreMessage::ProbeStarted(addr)).await;
                        let result = probe::execute_probe(
                            addr,
                            info_hash,
                            peer_id,
                            ctx,
                            local_shutdown_rx,
                            local_announce_rx,
                            quic_endpoint_cloned,
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
    engine.unregister(&info_hash);

    if !swarm_state.tracker_urls.is_empty() {
        tracing::info!("Graceful shutdown: Sending event=stopped tracker announce...");
        let params = swarm_state.announce_params(Some("stopped"));
        let announce_future = tracker::announce_all(&swarm_state.tracker_urls, &params, |_| {});
        let _ = tokio::time::timeout(Duration::from_secs(2), announce_future).await;
    }
    tracing::info!("Swarm network interfaces closed. Exit.");

    let _ = ui_sender
        .send(CoreMessage::Status("Swarm manager stopped".to_string()))
        .await;
}

impl SwarmState {
    fn announce_params(&self, event: Option<&'static str>) -> tracker::AnnounceParams<'_> {
        let downloaded = self.stats.downloaded.load(Ordering::Relaxed);
        let left = if self.event.as_deref() == Some("completed") {
            0
        } else {
            self.left_hint.saturating_sub(downloaded)
        };
        tracker::AnnounceParams {
            info_hash: &self.info_hash,
            peer_id: &self.peer_id,
            port: self.listen_port,
            uploaded: self.stats.uploaded.load(Ordering::Relaxed),
            downloaded,
            left,
            event,
        }
    }
}

fn should_reannounce(state: &SwarmState, available_len: usize, active_len: usize) -> bool {
    if state.tracker_urls.is_empty() || state.announce_in_progress {
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

    let tracker_urls = state.tracker_urls.clone();
    let info_hash = state.info_hash;
    let peer_id = state.peer_id;
    let listen_port = state.listen_port;
    let stats = state.stats.clone();
    let left_hint = state.left_hint;
    let completed = state.event.as_deref() == Some("completed");
    let event_str = state.event.clone();

    tokio::spawn(async move {
        let downloaded = stats.downloaded.load(Ordering::Relaxed);
        let params = tracker::AnnounceParams {
            info_hash: &info_hash,
            peer_id: &peer_id,
            port: listen_port,
            uploaded: stats.uploaded.load(Ordering::Relaxed),
            downloaded,
            left: if completed {
                0
            } else {
                left_hint.saturating_sub(downloaded)
            },
            event: event_str.as_deref(),
        };
        let delivered = event_tx.clone();
        let result = tracker::announce_all(&tracker_urls, &params, |peers| {
            // Hand peers over as soon as each tracker answers instead of waiting for the slowest one.
            let addrs = peers.iter().map(|p| p.addr).collect();
            let _ = delivered.send(SwarmEvent::TrackerPeersReceived(addrs));
        })
        .await;
        if let Err(err) = result {
            let _ = event_tx.send(SwarmEvent::TrackerAnnounceFailed(err.to_string()));
        }
    });
}

/// Queues new dialable peers (bounded) and returns how many were added.
fn enqueue_peers(
    queue: &mut VecDeque<SocketAddr>,
    active: &HashMap<SocketAddr, ActivePeer>,
    addrs: Vec<SocketAddr>,
) -> usize {
    let mut added = 0usize;
    for addr in addrs {
        if queue.len() >= MAX_QUEUED_PEERS {
            break;
        }
        let dialable = addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast();
        if !dialable || active.contains_key(&addr) || queue.contains(&addr) {
            continue;
        }
        queue.push_back(addr);
        added += 1;
    }
    added
}
