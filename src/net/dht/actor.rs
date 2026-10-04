use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::net::dht::krpc::{KrpcMessage, QueryArgs, ResponseArgs};
use crate::net::dht::routing::{Contact, NodeId, RoutingTable};
use crate::net::dht::search::DhtSearch;
use crate::net::dht::server::{canonical, DhtCommand, DhtServer};
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
        /// Receives `DhtPeersReceived` events for this search.
        peers_tx: mpsc::UnboundedSender<SwarmEvent>,
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
    peer_store: PeerStore,
    tokens: Tokens,
    port: u16,
}

/// Placeholder id for bootstrap routers, whose real id is not known yet.
fn pseudo_id(addr: &SocketAddr) -> NodeId {
    let mut id = [0u8; 20];
    id.copy_from_slice(&Sha1::digest(addr.to_string().as_bytes()));
    NodeId(id)
}

/// Splits contacts into the compact IPv4 list (26 bytes per node) and the
/// compact IPv6 list of BEP 32 (38 bytes per node).
fn encode_nodes(contacts: &[Contact]) -> (Vec<u8>, Vec<u8>) {
    let (mut v4, mut v6) = (Vec::new(), Vec::new());
    for contact in contacts {
        match canonical(contact.addr) {
            SocketAddr::V4(addr) => {
                v4.extend_from_slice(&contact.id.0);
                v4.extend_from_slice(&addr.ip().octets());
                v4.extend_from_slice(&addr.port().to_be_bytes());
            }
            SocketAddr::V6(addr) => {
                v6.extend_from_slice(&contact.id.0);
                v6.extend_from_slice(&addr.ip().octets());
                v6.extend_from_slice(&addr.port().to_be_bytes());
            }
        }
    }
    (v4, v6)
}

/// One `values` entry: 6 bytes for an IPv4 peer, 18 bytes for an IPv6 peer.
fn encode_value(addr: &SocketAddr) -> Option<serde_bytes::ByteBuf> {
    let (v4, v6) = crate::net::pex::split_compact(std::slice::from_ref(addr));
    let raw = if v4.is_empty() { v6 } else { v4 };
    (!raw.is_empty()).then(|| serde_bytes::ByteBuf::from(raw))
}

impl DhtManager {
    pub async fn new(port: u16) -> anyhow::Result<(Self, mpsc::Sender<DhtManagerCommand>)> {
        let local_id = NodeId(rand::random());

        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let (server, server_cmd_tx) = DhtServer::new(port, incoming_tx).await?;
        let bound_port = server.local_addr().port();
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
            peer_store: PeerStore::default(),
            tokens: Tokens::new(),
            port: bound_port,
        };

        Ok((manager, cmd_tx))
    }

    /// The UDP port the DHT socket is bound to.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn run(mut self) {
        info!("DhtManager started with node ID {:?}", self.local_id);
        let mut maintenance = tokio::time::interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(DhtManagerCommand::StartSearch { info_hash, announce_port, peers_tx }) => {
                        info!("DhtManager starting recursive search for {:?}", info_hash);
                        let search = DhtSearch::new(
                            info_hash,
                            self.local_id,
                            self.routing_table.closest(&info_hash, K),
                            self.server_cmd_tx.clone(),
                            peers_tx,
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

    /// The nodes to put in a reply: IPv4 contacts for an IPv4 requester and
    /// IPv6 contacts (`nodes6`) for an IPv6 requester (BEP 32).
    fn nodes_for(&self, target: &NodeId, requester: &SocketAddr) -> (Vec<u8>, Vec<u8>) {
        let want_v6 = requester.is_ipv6();
        let contacts = self
            .routing_table
            .closest_where(target, K, |c| canonical(c.addr).is_ipv6() == want_v6);
        encode_nodes(&contacts)
    }

    /// Answers a query from another node (BEP 5, BEP 32): ping, find_node,
    /// get_peers and announce_peer. Returns the reply to send, if any.
    fn handle_query(&mut self, src: SocketAddr, msg: KrpcMessage) -> Option<KrpcMessage> {
        let src = canonical(src);
        let tid = msg.t.clone();
        let (Some(name), Some(args)) = (msg.q.as_deref(), msg.a.as_ref()) else {
            return Some(KrpcMessage::error(tid, 203, "Protocol Error"));
        };
        let Ok(sender_id) = <[u8; 20]>::try_from(args.id.as_slice()) else {
            return Some(KrpcMessage::error(tid, 203, "Invalid node id"));
        };

        if crate::net::dht::server::is_valid_node_addr(&src) {
            self.routing_table.insert(Contact {
                id: NodeId(sender_id),
                addr: src,
            });
        }

        let reply = |extra: ResponseArgs| KrpcMessage::response(tid.clone(), extra);
        let base = ResponseArgs {
            id: self.local_id.0.to_vec(),
            nodes: vec![],
            nodes6: vec![],
            token: vec![],
            values: vec![],
        };

        match name {
            "ping" => Some(reply(base)),
            "find_node" => {
                let target = <[u8; 20]>::try_from(args.target.as_slice()).ok()?;
                let (nodes, nodes6) = self.nodes_for(&NodeId(target), &src);
                Some(reply(ResponseArgs {
                    nodes,
                    nodes6,
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
                let (nodes, nodes6) = if values.is_empty() {
                    self.nodes_for(&NodeId(info_hash), &src)
                } else {
                    (vec![], vec![])
                };
                Some(reply(ResponseArgs {
                    nodes,
                    nodes6,
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
        Ok(Ok(addrs)) => addrs.collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn manager() -> DhtManager {
        // Port 0 picks a free UDP port; bootstrap lookups may fail offline, which is fine.
        let (manager, _cmd) = DhtManager::new(0).await.unwrap();
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
    async fn ipv6_requesters_get_nodes6_and_ipv6_peers() {
        let mut dht = manager().await;
        let v4: SocketAddr = "10.1.2.3:6881".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::2]:6881".parse().unwrap();

        // One IPv4 and one IPv6 node introduce themselves.
        dht.handle_query(v4, query("ping", QueryArgs::new(vec![7; 20])));
        dht.handle_query(v6, query("ping", QueryArgs::new(vec![8; 20])));

        let mut args = QueryArgs::new(vec![9; 20]);
        args.target = vec![1; 20];
        let reply = dht
            .handle_query(
                "[2001:db8::3]:6881".parse().unwrap(),
                query("find_node", args),
            )
            .unwrap()
            .r
            .unwrap();
        assert!(
            reply.nodes.is_empty(),
            "an IPv6 requester gets no IPv4 nodes"
        );
        assert!(reply.nodes6.chunks(38).any(|c| c[..20] == [8u8; 20]));
        assert!(!reply.nodes6.chunks(38).any(|c| c[..20] == [7u8; 20]));

        // An IPv6 peer announces itself and is returned as an 18-byte value.
        let info_hash = vec![5u8; 20];
        let mut get = QueryArgs::new(vec![8; 20]);
        get.info_hash = info_hash.clone();
        let token = dht
            .handle_query(v6, query("get_peers", get.clone()))
            .unwrap()
            .r
            .unwrap()
            .token;
        let mut announce = QueryArgs::new(vec![8; 20]);
        announce.info_hash = info_hash;
        announce.port = Some(51413);
        announce.token = token;
        assert_eq!(
            dht.handle_query(v6, query("announce_peer", announce))
                .unwrap()
                .y,
            "r"
        );

        let values = dht
            .handle_query(v4, query("get_peers", get))
            .unwrap()
            .r
            .unwrap()
            .values;
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].len(), 18);
        assert_eq!(
            crate::net::pex::decode_compact_peers(&values[0]),
            vec!["[2001:db8::2]:51413".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn mapped_ipv4_sources_are_treated_as_ipv4() {
        let mut dht = manager().await;
        let mapped: SocketAddr = "[::ffff:10.1.2.3]:6881".parse().unwrap();
        dht.handle_query(mapped, query("ping", QueryArgs::new(vec![7; 20])));

        let mut args = QueryArgs::new(vec![9; 20]);
        args.target = vec![1; 20];
        let reply = dht
            .handle_query("10.9.9.9:1".parse().unwrap(), query("find_node", args))
            .unwrap()
            .r
            .unwrap();
        assert!(reply.nodes6.is_empty());
        assert!(reply
            .nodes
            .chunks(26)
            .any(|c| c[..20] == [7u8; 20] && c[20..24] == [10, 1, 2, 3]));
    }

    #[tokio::test]
    async fn answers_over_real_sockets_on_both_families() {
        use crate::net::dht::server::DhtServer;

        let (manager, _cmd) = DhtManager::new(0).await.unwrap();
        let port = manager.port();
        tokio::spawn(manager.run());

        let (incoming_tx, _incoming_rx) = mpsc::channel(8);
        let (server, client) = DhtServer::new(0, incoming_tx).await.unwrap();
        tokio::spawn(server.run());

        let mut targets: Vec<SocketAddr> = vec![SocketAddr::from(([127, 0, 0, 1], port))];
        if std::net::UdpSocket::bind("[::1]:0").is_ok() {
            targets.push(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)));
        }

        for target in targets {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            client
                .send(DhtCommand::SendQuery {
                    target,
                    msg: KrpcMessage::new_ping_query(vec![], vec![4; 20]),
                    reply: reply_tx,
                })
                .await
                .unwrap();
            let reply = tokio::time::timeout(Duration::from_secs(5), reply_rx)
                .await
                .unwrap_or_else(|_| panic!("no answer from {target}"))
                .unwrap()
                .unwrap();
            assert_eq!(reply.y, "r", "{target}");
            assert_eq!(reply.r.unwrap().id.len(), 20);
        }
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
