use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::net::dht::krpc::{KrpcMessage, QueryArgs, ResponseArgs};
use crate::net::dht::routing::{Contact, NodeId, RoutingTable};
use crate::net::dht::search::DhtSearch;
use crate::net::dht::server::{DhtCommand, DhtServer};
use crate::net::swarm::SwarmEvent;

const BOOTSTRAP_HOSTS: [&str; 3] = [
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
    "dht.transmissionbt.com:6881",
];
const K: usize = 8;
const MAX_STORED_TORRENTS: usize = 500;
const MAX_PEERS_PER_TORRENT: usize = 100;
const PEER_TTL: Duration = Duration::from_secs(30 * 60);
const TOKEN_ROTATION: Duration = Duration::from_secs(5 * 60);
const MAX_VALUES_PER_REPLY: usize = 50;

pub enum DhtManagerCommand {
    /// Looks up peers for `info_hash`; with `announce_port` the nodes closest
    /// to it are also told that we are a peer on that TCP/UDP port.
    StartSearch {
        info_hash: NodeId,
        announce_port: Option<u16>,
    },
    InsertNode(Contact),
}

/// Peers other nodes announced to us, bounded in count and age.
#[derive(Default)]
struct PeerStore {
    peers: HashMap<[u8; 20], Vec<(SocketAddr, Instant)>>,
}

impl PeerStore {
    fn add(&mut self, info_hash: [u8; 20], addr: SocketAddr) {
        if !self.peers.contains_key(&info_hash) && self.peers.len() >= MAX_STORED_TORRENTS {
            return;
        }
        let entry = self.peers.entry(info_hash).or_default();
        if let Some(existing) = entry.iter_mut().find(|(a, _)| *a == addr) {
            existing.1 = Instant::now();
        } else {
            if entry.len() >= MAX_PEERS_PER_TORRENT {
                entry.remove(0);
            }
            entry.push((addr, Instant::now()));
        }
    }

    fn get(&self, info_hash: &[u8; 20]) -> Vec<SocketAddr> {
        self.peers
            .get(info_hash)
            .map(|list| {
                list.iter()
                    .filter(|(_, seen)| seen.elapsed() < PEER_TTL)
                    .map(|(addr, _)| *addr)
                    .take(MAX_VALUES_PER_REPLY)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn purge(&mut self) {
        self.peers.retain(|_, list| {
            list.retain(|(_, seen)| seen.elapsed() < PEER_TTL);
            !list.is_empty()
        });
    }
}

/// Tokens handed out by get_peers; valid for the current and the previous secret.
struct Tokens {
    current: [u8; 16],
    previous: [u8; 16],
    rotated: Instant,
}

impl Tokens {
    fn new() -> Self {
        Self {
            current: rand::random(),
            previous: rand::random(),
            rotated: Instant::now(),
        }
    }

    fn rotate_if_due(&mut self) {
        if self.rotated.elapsed() >= TOKEN_ROTATION {
            self.previous = self.current;
            self.current = rand::random();
            self.rotated = Instant::now();
        }
    }

    fn make(secret: &[u8; 16], ip: IpAddr) -> Vec<u8> {
        let mut hasher = Sha1::new();
        hasher.update(secret);
        match ip {
            IpAddr::V4(v4) => hasher.update(v4.octets()),
            IpAddr::V6(v6) => hasher.update(v6.octets()),
        }
        hasher.finalize()[..8].to_vec()
    }

    fn issue(&self, ip: IpAddr) -> Vec<u8> {
        Self::make(&self.current, ip)
    }

    fn is_valid(&self, token: &[u8], ip: IpAddr) -> bool {
        token == Self::make(&self.current, ip) || token == Self::make(&self.previous, ip)
    }
}

pub struct DhtManager {
    local_id: NodeId,
    routing_table: RoutingTable,
    cmd_rx: mpsc::Receiver<DhtManagerCommand>,
    cmd_tx: mpsc::Sender<DhtManagerCommand>,
    server_cmd_tx: mpsc::Sender<DhtCommand>,
    incoming_rx: mpsc::Receiver<(SocketAddr, KrpcMessage)>,
    swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
    peer_store: PeerStore,
    tokens: Tokens,
}

/// Placeholder id for bootstrap routers, whose real id is not known yet.
fn pseudo_id(addr: &SocketAddr) -> NodeId {
    let mut id = [0u8; 20];
    id.copy_from_slice(&Sha1::digest(addr.to_string().as_bytes()));
    NodeId(id)
}

fn encode_nodes(contacts: &[Contact]) -> Vec<u8> {
    let mut out = Vec::with_capacity(contacts.len() * 26);
    for contact in contacts {
        if let SocketAddr::V4(v4) = contact.addr {
            out.extend_from_slice(&contact.id.0);
            out.extend_from_slice(&v4.ip().octets());
            out.extend_from_slice(&v4.port().to_be_bytes());
        }
    }
    out
}

fn encode_value(addr: &SocketAddr) -> Option<serde_bytes::ByteBuf> {
    match addr {
        SocketAddr::V4(v4) => {
            let mut raw = v4.ip().octets().to_vec();
            raw.extend_from_slice(&v4.port().to_be_bytes());
            Some(serde_bytes::ByteBuf::from(raw))
        }
        SocketAddr::V6(_) => None,
    }
}

impl DhtManager {
    pub async fn new(
        port: u16,
        swarm_tx: mpsc::UnboundedSender<SwarmEvent>,
    ) -> anyhow::Result<(Self, mpsc::Sender<DhtManagerCommand>)> {
        let local_id = NodeId(rand::random());

        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let (server, server_cmd_tx) = DhtServer::new(port, incoming_tx).await?;
        tokio::spawn(server.run());

        let mut routing_table = RoutingTable::new(local_id);

        let (first, second, third) = tokio::join!(
            lookup_bootstrap(BOOTSTRAP_HOSTS[0]),
            lookup_bootstrap(BOOTSTRAP_HOSTS[1]),
            lookup_bootstrap(BOOTSTRAP_HOSTS[2]),
        );
        for addr in first.into_iter().chain(second).chain(third) {
            routing_table.insert(Contact {
                id: pseudo_id(&addr),
                addr,
            });
        }

        let (cmd_tx, cmd_rx) = mpsc::channel(100);

        let manager = Self {
            local_id,
            routing_table,
            cmd_rx,
            cmd_tx: cmd_tx.clone(),
            server_cmd_tx,
            incoming_rx,
            swarm_tx,
            peer_store: PeerStore::default(),
            tokens: Tokens::new(),
        };

        Ok((manager, cmd_tx))
    }

    pub async fn run(mut self) {
        info!("DhtManager started with node ID {:?}", self.local_id);
        let mut maintenance = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(DhtManagerCommand::StartSearch { info_hash, announce_port }) => {
                        info!("DhtManager starting recursive search for {:?}", info_hash);
                        let search = DhtSearch::new(
                            info_hash,
                            self.local_id,
                            self.routing_table.closest(&info_hash, K),
                            self.server_cmd_tx.clone(),
                            self.swarm_tx.clone(),
                            self.cmd_tx.clone(),
                            announce_port,
                        );
                        tokio::spawn(search.run());
                    }
                    Some(DhtManagerCommand::InsertNode(contact)) => {
                        self.routing_table.insert(contact);
                    }
                    None => {
                        info!("DhtManager command channel closed.");
                        break;
                    }
                },
                Some((src, msg)) = self.incoming_rx.recv() => {
                    if let Some(reply) = self.handle_query(src, msg) {
                        let _ = self
                            .server_cmd_tx
                            .send(DhtCommand::SendReply { target: src, msg: reply })
                            .await;
                    }
                }
                _ = maintenance.tick() => {
                    self.tokens.rotate_if_due();
                    self.peer_store.purge();
                }
            }
        }
    }

    /// Answers a query from another node (BEP 5): ping, find_node, get_peers
    /// and announce_peer. Returns the reply to send, if any.
    fn handle_query(&mut self, src: SocketAddr, msg: KrpcMessage) -> Option<KrpcMessage> {
        let tid = msg.t.clone();
        let (Some(name), Some(args)) = (msg.q.as_deref(), msg.a.as_ref()) else {
            return Some(KrpcMessage::error(tid, 203, "Protocol Error"));
        };
        let Ok(sender_id) = <[u8; 20]>::try_from(args.id.as_slice()) else {
            return Some(KrpcMessage::error(tid, 203, "Invalid node id"));
        };

        if src.is_ipv4() && src.port() != 0 {
            self.routing_table.insert(Contact {
                id: NodeId(sender_id),
                addr: src,
            });
        }

        let reply = |extra: ResponseArgs| KrpcMessage::response(tid.clone(), extra);
        let base = ResponseArgs {
            id: self.local_id.0.to_vec(),
            nodes: vec![],
            token: vec![],
            values: vec![],
        };

        match name {
            "ping" => Some(reply(base)),
            "find_node" => {
                let target = <[u8; 20]>::try_from(args.target.as_slice()).ok()?;
                let nodes = self.routing_table.closest(&NodeId(target), K);
                Some(reply(ResponseArgs {
                    nodes: encode_nodes(&nodes),
                    ..base
                }))
            }
            "get_peers" => {
                let Ok(info_hash) = <[u8; 20]>::try_from(args.info_hash.as_slice()) else {
                    return Some(KrpcMessage::error(tid, 203, "Invalid info_hash"));
                };
                let values: Vec<_> = self
                    .peer_store
                    .get(&info_hash)
                    .iter()
                    .filter_map(encode_value)
                    .collect();
                let nodes = if values.is_empty() {
                    encode_nodes(&self.routing_table.closest(&NodeId(info_hash), K))
                } else {
                    vec![]
                };
                Some(reply(ResponseArgs {
                    nodes,
                    token: self.tokens.issue(src.ip()),
                    values,
                    ..base
                }))
            }
            "announce_peer" => Some(self.handle_announce(src, args, tid, base)),
            _ => Some(KrpcMessage::error(tid, 204, "Method Unknown")),
        }
    }

    fn handle_announce(
        &mut self,
        src: SocketAddr,
        args: &QueryArgs,
        tid: Vec<u8>,
        base: ResponseArgs,
    ) -> KrpcMessage {
        let Ok(info_hash) = <[u8; 20]>::try_from(args.info_hash.as_slice()) else {
            return KrpcMessage::error(tid, 203, "Invalid info_hash");
        };
        if !self.tokens.is_valid(&args.token, src.ip()) {
            return KrpcMessage::error(tid, 203, "Bad token");
        }
        let port = if args.implied_port == Some(1) {
            Some(src.port())
        } else {
            args.port
        };
        let Some(port) = port.filter(|p| *p != 0) else {
            return KrpcMessage::error(tid, 203, "Invalid port");
        };

        debug!(
            "DHT announce_peer from {src} for {}",
            hex::encode(info_hash)
        );
        self.peer_store
            .add(info_hash, SocketAddr::new(src.ip(), port));
        KrpcMessage::response(tid, base)
    }
}

async fn lookup_bootstrap(host: &str) -> Vec<SocketAddr> {
    match tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host(host)).await {
        Ok(Ok(addrs)) => addrs.filter(SocketAddr::is_ipv4).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn manager() -> DhtManager {
        let (tx, _rx) = mpsc::unbounded_channel();
        // Port 0 picks a free UDP port; bootstrap lookups may fail offline, which is fine.
        let (manager, _cmd) = DhtManager::new(0, tx).await.unwrap();
        manager
    }

    fn query(name: &str, args: QueryArgs) -> KrpcMessage {
        KrpcMessage::query(b"tx".to_vec(), name, args)
    }

    #[tokio::test]
    async fn answers_ping_and_find_node() {
        let mut dht = manager().await;
        let src: SocketAddr = "10.1.2.3:6881".parse().unwrap();

        let ping = dht
            .handle_query(src, query("ping", QueryArgs::new(vec![7; 20])))
            .unwrap();
        assert_eq!(ping.r.unwrap().id, dht.local_id.0.to_vec());

        let mut args = QueryArgs::new(vec![7; 20]);
        args.target = vec![9; 20];
        let reply = dht.handle_query(src, query("find_node", args)).unwrap();
        let nodes = reply.r.unwrap().nodes;
        assert_eq!(nodes.len() % 26, 0);
        // The pinging node was added to the routing table and can be returned.
        assert!(nodes.chunks(26).any(|c| c[..20] == [7u8; 20]));
    }

    #[tokio::test]
    async fn announce_requires_a_valid_token_and_is_returned_by_get_peers() {
        let mut dht = manager().await;
        let src: SocketAddr = "10.1.2.3:50000".parse().unwrap();
        let info_hash = vec![5u8; 20];

        let mut get = QueryArgs::new(vec![7; 20]);
        get.info_hash = info_hash.clone();
        let token = dht
            .handle_query(src, query("get_peers", get.clone()))
            .unwrap()
            .r
            .unwrap()
            .token;
        assert!(!token.is_empty());

        let mut bad = QueryArgs::new(vec![7; 20]);
        bad.info_hash = info_hash.clone();
        bad.port = Some(6881);
        bad.token = b"nope".to_vec();
        let rejected = dht.handle_query(src, query("announce_peer", bad)).unwrap();
        assert_eq!(rejected.y, "e");

        let mut good = QueryArgs::new(vec![7; 20]);
        good.info_hash = info_hash.clone();
        good.port = Some(6881);
        good.token = token;
        let accepted = dht.handle_query(src, query("announce_peer", good)).unwrap();
        assert_eq!(accepted.y, "r");

        let values = dht
            .handle_query(src, query("get_peers", get))
            .unwrap()
            .r
            .unwrap()
            .values;
        assert_eq!(values.len(), 1);
        assert_eq!(&values[0][..], &[10, 1, 2, 3, 0x1A, 0xE1]);
    }

    #[tokio::test]
    async fn token_is_bound_to_the_requesting_ip() {
        let dht = manager().await;
        let token = dht.tokens.issue("10.0.0.1".parse().unwrap());
        assert!(dht.tokens.is_valid(&token, "10.0.0.1".parse().unwrap()));
        assert!(!dht.tokens.is_valid(&token, "10.0.0.2".parse().unwrap()));
    }

    #[tokio::test]
    async fn unknown_method_gets_an_error() {
        let mut dht = manager().await;
        let src: SocketAddr = "10.1.2.3:6881".parse().unwrap();
        let reply = dht
            .handle_query(src, query("vote", QueryArgs::new(vec![7; 20])))
            .unwrap();
        assert_eq!(reply.y, "e");
    }
}
