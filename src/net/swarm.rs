use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{interval, Duration};
use tracing::warn;

use crate::core::command::CoreMessage;
use crate::core::coordinator::CoordinatorMsg;
use crate::net::probe;

const MAX_ACTIVE_PEERS: usize = 30;
const SWARM_TICK_SECS: u64 = 5;
const PEER_IDLE_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub enum SwarmEvent {
    PeerProgress(SocketAddr, u32),
    PeerExited(SocketAddr),
}

struct ActivePeer {
    downloaded_bytes: u64,
    last_progress: Instant,
    handle: JoinHandle<()>,
}

pub async fn run_swarm_manager(
    mut available_peers: VecDeque<SocketAddr>,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    expected_hashes: Arc<Vec<[u8; 20]>>,
    piece_length: u32,
    total_length: Option<u64>,
    ui_sender: mpsc::Sender<CoreMessage>,
    coord_sender: mpsc::Sender<CoordinatorMsg>,
    shutdown_tx: broadcast::Sender<()>,
) {
    let mut active: HashMap<SocketAddr, ActivePeer> = HashMap::new();
    let mut tick = interval(Duration::from_secs(SWARM_TICK_SECS));
    let mut shutdown_rx = shutdown_tx.subscribe();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<SwarmEvent>();

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

                    let expected_hashes_cloned = Arc::clone(&expected_hashes);
                    let ui_sender_cloned = ui_sender.clone();
                    let coord_sender_cloned = coord_sender.clone();
                    let event_tx_cloned = event_tx.clone();
                    let local_shutdown_rx = shutdown_tx.subscribe();

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
    }

    for (_, peer) in active {
        peer.handle.abort();
    }

    let _ = ui_sender
        .send(CoreMessage::Status("Swarm manager stopped".to_string()))
        .await;
}
