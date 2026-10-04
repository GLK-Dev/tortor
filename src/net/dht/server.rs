use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

use super::krpc::KrpcMessage;

const TRANSACTION_TTL: Duration = Duration::from_secs(20);
const MAX_PENDING: usize = 4096;
const RECV_BUFFER: usize = 4096;

/// Dual-stack sockets report IPv4 peers as IPv4-mapped IPv6 addresses; this
/// turns them back into plain IPv4 so addresses compare and print normally.
pub fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

/// Binds the DHT socket on `port`, serving IPv4 and IPv6 on one dual-stack
/// socket when the host has IPv6 (BEP 32). The flag tells which one it got.
fn bind_socket(port: u16) -> std::io::Result<(UdpSocket, bool)> {
    use socket2::{Domain, Protocol, Socket, Type};

    let dual = (|| {
        let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_only_v6(false)?;
        socket.bind(&SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
        socket.set_nonblocking(true)?;
        UdpSocket::from_std(socket.into())
    })();

    match dual {
        Ok(socket) => Ok((socket, true)),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => Err(err),
        Err(_) => {
            let socket = std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port)))?;
            socket.set_nonblocking(true)?;
            Ok((UdpSocket::from_std(socket)?, false))
        }
    }
}

/// Rejects addresses no real node can have (port 0, 0.0.0.0, multicast).
pub fn is_valid_node_addr(addr: &SocketAddr) -> bool {
    addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast()
}

pub enum DhtCommand {
    SendQuery {
        target: SocketAddr,
        msg: KrpcMessage,
        reply: oneshot::Sender<Result<KrpcMessage>>,
    },
    SendReply {
        target: SocketAddr,
        msg: KrpcMessage,
    },
}

struct Pending {
    target: SocketAddr,
    reply: oneshot::Sender<Result<KrpcMessage>>,
    sent: Instant,
}

pub struct DhtServer {
    socket: Arc<UdpSocket>,
    /// The socket is IPv6, so IPv4 targets must be sent as IPv4-mapped addresses.
    dual_stack: bool,
    transactions: HashMap<Vec<u8>, Pending>,
    next_tid: u32,
    cmd_rx: mpsc::Receiver<DhtCommand>,
    incoming_tx: mpsc::Sender<(SocketAddr, KrpcMessage)>,
}

impl DhtServer {
    /// Binds the DHT socket. Queries from other nodes are forwarded to `incoming_tx`.
    pub async fn new(
        port: u16,
        incoming_tx: mpsc::Sender<(SocketAddr, KrpcMessage)>,
    ) -> Result<(Self, mpsc::Sender<DhtCommand>)> {
        let (socket, dual_stack) = bind_socket(port)?;
        info!(
            "DHT UDP listener bound to {} ({})",
            socket.local_addr()?,
            if dual_stack {
                "IPv4 + IPv6"
            } else {
                "IPv4 only"
            }
        );

        let (cmd_tx, cmd_rx) = mpsc::channel(100);

        let server = Self {
            socket: Arc::new(socket),
            dual_stack,
            transactions: HashMap::new(),
            next_tid: rand::random(),
            cmd_rx,
            incoming_tx,
        };

        Ok((server, cmd_tx))
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
    }

    /// The address as the socket wants it for `send_to`.
    fn wire_addr(&self, addr: SocketAddr) -> SocketAddr {
        match addr {
            SocketAddr::V4(v4) if self.dual_stack => SocketAddr::V6(std::net::SocketAddrV6::new(
                v4.ip().to_ipv6_mapped(),
                v4.port(),
                0,
                0,
            )),
            other => other,
        }
    }

    pub async fn run(mut self) {
        let mut buf = vec![0u8; RECV_BUFFER];
        let mut cleanup = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(DhtCommand::SendQuery { target, mut msg, reply }) => {
                            let target = canonical(target);
                            if !is_valid_node_addr(&target) {
                                let _ = reply.send(Err(anyhow::anyhow!("invalid DHT node address {target}")));
                                continue;
                            }
                            if self.transactions.len() >= MAX_PENDING {
                                let _ = reply.send(Err(anyhow::anyhow!("too many pending DHT queries")));
                                continue;
                            }
                            // Transaction ids are allocated here so concurrent searches never collide.
                            self.next_tid = self.next_tid.wrapping_add(1);
                            msg.t = self.next_tid.to_be_bytes().to_vec();
                            let tid = msg.t.clone();

                            if let Ok(payload) = serde_bencode::to_bytes(&msg) {
                                match self.socket.send_to(&payload, self.wire_addr(target)).await {
                                    Ok(_) => {
                                        self.transactions.insert(tid, Pending { target, reply, sent: Instant::now() });
                                    }
                                    Err(e) => {
                                        debug!("Failed to send DHT packet to {}: {}", target, e);
                                        let _ = reply.send(Err(e.into()));
                                    }
                                }
                            }
                        }
                        Some(DhtCommand::SendReply { target, msg }) => {
                            if let Ok(payload) = serde_bencode::to_bytes(&msg) {
                                let _ = self.socket.send_to(&payload, self.wire_addr(canonical(target))).await;
                            }
                        }
                        None => {
                            info!("DHT server command channel closed, shutting down.");
                            break;
                        }
                    }
                }
                result = self.socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, src)) => self.handle_datagram(&buf[..len], canonical(src)),
                        Err(e) => {
                            // Windows reports ICMP "port unreachable" for earlier sends as a recv error.
                            debug!("DHT socket recv error: {}", e);
                        }
                    }
                }
                _ = cleanup.tick() => {
                    self.transactions.retain(|_, p| p.sent.elapsed() < TRANSACTION_TTL);
                }
            }
        }
    }

    fn handle_datagram(&mut self, data: &[u8], src: SocketAddr) {
        if crate::core::bencode::check_bencode_depth(data).is_err() {
            return;
        }
        let Ok(msg) = serde_bencode::from_bytes::<KrpcMessage>(data) else {
            return;
        };

        match msg.y.as_str() {
            "r" | "e" => {
                // Only the node we asked may answer; otherwise anyone could forge replies.
                let expected = self
                    .transactions
                    .get(&msg.t)
                    .is_some_and(|p| p.target == src);
                if expected {
                    if let Some(pending) = self.transactions.remove(&msg.t) {
                        let _ = pending.reply.send(Ok(msg));
                    }
                }
            }
            "q" => {
                debug!("Received DHT query from {}: {:?}", src, msg.q);
                let _ = self.incoming_tx.try_send((src, msg));
            }
            _ => {}
        }
    }
}
