use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

use super::krpc::KrpcMessage;

pub enum DhtCommand {
    SendQuery {
        target: SocketAddr,
        msg: KrpcMessage,
        reply: oneshot::Sender<Result<KrpcMessage>>,
    },
}

pub struct DhtServer {
    socket: Arc<UdpSocket>,
    transactions: HashMap<Vec<u8>, oneshot::Sender<Result<KrpcMessage>>>,
    cmd_rx: mpsc::Receiver<DhtCommand>,
}

impl DhtServer {
    pub async fn new(port: u16) -> Result<(Self, mpsc::Sender<DhtCommand>)> {
        let addr = format!("0.0.0.0:{}", port);
        let socket = UdpSocket::bind(&addr).await?;
        info!("DHT UDP listener bound to {}", addr);

        let (cmd_tx, cmd_rx) = mpsc::channel(100);

        let server = Self {
            socket: Arc::new(socket),
            transactions: HashMap::new(),
            cmd_rx,
        };

        Ok((server, cmd_tx))
    }

    pub async fn run(mut self) {
        let mut buf = vec![0u8; 2048];

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(DhtCommand::SendQuery { target, msg, reply }) => {
                            self.transactions.insert(msg.t.clone(), reply);
                            if let Ok(payload) = serde_bencode::to_bytes(&msg) {
                                if let Err(e) = self.socket.send_to(&payload, target).await {
                                    error!("Failed to send DHT packet to {}: {}", target, e);
                                    self.transactions.remove(&msg.t);
                                }
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
                        Ok((len, src)) => {
                            if let Ok(msg) = serde_bencode::from_bytes::<KrpcMessage>(&buf[..len]) {
                                if msg.y == "r" || msg.y == "e" {
                                    if let Some(reply_tx) = self.transactions.remove(&msg.t) {
                                        let _ = reply_tx.send(Ok(msg));
                                    }
                                } else if msg.y == "q" {
                                    debug!("Received DHT query from {}: {:?}", src, msg.q);
                                    // TODO: Handle incoming queries (Phase 2/3)
                                }
                            }
                        }
                        Err(e) => {
                            error!("DHT socket recv error: {}", e);
                        }
                    }
                }
            }
        }
    }
}

