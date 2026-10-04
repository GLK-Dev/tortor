//! Application-wide networking: one TCP listener, one QUIC endpoint and one DHT
//! node shared by every torrent. Torrents register themselves by info hash so
//! inbound connections reach the right swarm.

use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch, Semaphore};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::core::command::SessionEvent;
use crate::core::peer_id::generate_peer_id;
use crate::net::dht::actor::{DhtManager, DhtManagerCommand};
use crate::net::inbound;
use crate::net::portmap::{self, PortMapStatus};
use crate::net::session::PeerContext;

const MAX_INBOUND_PEERS: usize = 100;

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// First port to try; TCP and QUIC share it. 0 picks any free port.
    pub listen_port: u16,
    pub enable_dht: bool,
    /// Ask a UPnP router to forward the peer port.
    pub enable_port_mapping: bool,
}

/// What the engine needs to serve inbound peers of one torrent.
#[derive(Clone)]
pub struct TorrentRegistration {
    pub ctx: PeerContext,
    pub shutdown_tx: broadcast::Sender<()>,
    pub announce_tx: broadcast::Sender<SessionEvent>,
}

pub struct Engine {
    pub peer_id: [u8; 20],
    /// Port of the TCP listener and the QUIC endpoint.
    pub port: u16,
    /// The listeners also accept IPv6 peers (dual-stack sockets).
    pub ipv6: bool,
    pub quic_endpoint: Arc<quinn::Endpoint>,
    pub dht: Option<mpsc::Sender<DhtManagerCommand>>,
    /// State of the UPnP port forwarding.
    pub port_map: watch::Receiver<PortMapStatus>,
    portmap_task: Mutex<Option<JoinHandle<()>>>,
    torrents: RwLock<HashMap<[u8; 20], TorrentRegistration>>,
    shutdown_tx: broadcast::Sender<()>,
}

fn next_port(port: u16) -> u16 {
    if port == 0 {
        0
    } else {
        port.saturating_add(2)
    }
}

/// Binds a TCP listener that serves IPv4 and IPv6 on one socket; falls back
/// to IPv4 only when the host has no IPv6. The flag tells which one it got.
fn bind_tcp(port: u16) -> std::io::Result<(TcpListener, bool)> {
    use socket2::{Domain, Protocol, Socket, Type};

    let dual = (|| {
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_only_v6(false)?;
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
        socket.listen(1024)?;
        socket.set_nonblocking(true)?;
        TcpListener::from_std(socket.into())
    })();

    match dual {
        Ok(listener) => Ok((listener, true)),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => Err(err),
        Err(_) => {
            let socket = std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))?;
            socket.set_nonblocking(true)?;
            Ok((TcpListener::from_std(socket)?, false))
        }
    }
}

fn bind_quic(
    server_config: &quinn::ServerConfig,
    port: u16,
    dual_stack: bool,
) -> std::io::Result<quinn::Endpoint> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket: std::net::UdpSocket = if dual_stack {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_only_v6(false)?;
        socket.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
        socket.into()
    } else {
        std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port)))?
    };

    quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config.clone()),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
}

impl Engine {
    pub async fn start(options: EngineOptions) -> Result<Arc<Self>> {
        let (server_config, client_config) =
            crate::crypto::tls::configure_quic().context("failed to configure QUIC")?;

        let mut candidate = options.listen_port;
        let mut bound = None;
        for _ in 0..100 {
            let (tcp, dual_stack) = match bind_tcp(candidate) {
                Ok(bound) => bound,
                Err(err) => {
                    warn!("TCP port {candidate} is busy ({err}). Trying next...");
                    candidate = next_port(candidate);
                    continue;
                }
            };
            let port = tcp.local_addr()?.port();
            match bind_quic(&server_config, port, dual_stack) {
                Ok(quic) => {
                    bound = Some((tcp, quic, port, dual_stack));
                    break;
                }
                Err(err) => {
                    warn!("UDP port {port} is busy ({err}). Trying next...");
                    candidate = next_port(candidate);
                }
            }
        }
        let (tcp, mut quic, port, ipv6) = bound.context("no free port for the peer listeners")?;
        quic.set_default_client_config(client_config);
        info!(
            "Peer listeners bound to port {port} (TCP and QUIC, {})",
            if ipv6 { "IPv4 + IPv6" } else { "IPv4 only" }
        );

        let dht = if options.enable_dht {
            start_dht(port).await
        } else {
            None
        };

        let (shutdown_tx, _) = broadcast::channel(4);
        let (map_tx, port_map) = watch::channel(if options.enable_port_mapping {
            PortMapStatus::Searching
        } else {
            PortMapStatus::Disabled
        });
        let portmap_task = options.enable_port_mapping.then(|| {
            tokio::spawn(portmap::run_upnp(
                port,
                Arc::new(map_tx),
                shutdown_tx.subscribe(),
            ))
        });

        let engine = Arc::new(Self {
            peer_id: generate_peer_id(),
            port,
            ipv6,
            quic_endpoint: Arc::new(quic),
            dht,
            port_map,
            portmap_task: Mutex::new(portmap_task),
            torrents: RwLock::new(HashMap::new()),
            shutdown_tx,
        });

        let limit = Arc::new(Semaphore::new(MAX_INBOUND_PEERS));
        spawn_tcp_acceptor(engine.clone(), tcp, limit.clone());
        spawn_quic_acceptor(engine.clone(), limit);
        Ok(engine)
    }

    pub fn register(&self, info_hash: [u8; 20], registration: TorrentRegistration) {
        self.torrents
            .write()
            .unwrap()
            .insert(info_hash, registration);
    }

    pub fn unregister(&self, info_hash: &[u8; 20]) {
        self.torrents.write().unwrap().remove(info_hash);
    }

    pub fn lookup(&self, info_hash: &[u8; 20]) -> Option<TorrentRegistration> {
        self.torrents.read().unwrap().get(info_hash).cloned()
    }

    /// Like [`Engine::shutdown`], then waits briefly for the UPnP mappings to be removed.
    pub async fn shutdown_graceful(&self) {
        self.shutdown();
        let task = self.portmap_task.lock().unwrap().take();
        if let Some(task) = task {
            let _ = tokio::time::timeout(Duration::from_secs(4), task).await;
        }
    }

    /// Stops accepting connections. Torrent sessions are stopped through their own shutdown signal.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
        self.quic_endpoint.close(0u32.into(), b"shutdown");
    }
}

/// The DHT uses the port after the peer port; falls back to any free port.
async fn start_dht(peer_port: u16) -> Option<mpsc::Sender<DhtManagerCommand>> {
    for port in [peer_port.saturating_add(1), 0] {
        match DhtManager::new(port).await {
            Ok((manager, cmd_tx)) => {
                tokio::spawn(manager.run());
                return Some(cmd_tx);
            }
            Err(err) => warn!("DHT could not bind port {port}: {err}"),
        }
    }
    None
}

fn spawn_tcp_acceptor(engine: Arc<Engine>, listener: TcpListener, limit: Arc<Semaphore>) {
    tokio::spawn(async move {
        let mut stop = engine.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                _ = stop.recv() => break,
                accepted = listener.accept() => match accepted {
                    Ok((socket, addr)) => {
                        let Ok(permit) = limit.clone().try_acquire_owned() else {
                            continue;
                        };
                        let engine = engine.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(err) = inbound::serve_tcp(socket, addr, engine).await {
                                debug!("inbound TCP peer {addr} ended: {err}");
                            }
                        });
                    }
                    Err(err) => {
                        warn!("failed to accept TCP peer: {err}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
            }
        }
    });
}

fn spawn_quic_acceptor(engine: Arc<Engine>, limit: Arc<Semaphore>) {
    tokio::spawn(async move {
        let mut stop = engine.shutdown_tx.subscribe();
        loop {
            tokio::select! {
                _ = stop.recv() => break,
                incoming = engine.quic_endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    let Ok(permit) = limit.clone().try_acquire_owned() else {
                        incoming.refuse();
                        continue;
                    };
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        if let Err(err) = inbound::serve_quic(incoming, engine).await {
                            debug!("inbound QUIC peer ended: {err}");
                        }
                    });
                }
            }
        }
    });
}
