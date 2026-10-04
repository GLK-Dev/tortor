//! Magnet-link metadata download (BEP 9 over the BEP 10 extension protocol).

use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{interval, timeout, Duration};
use tracing::{debug, warn};

use crate::core::bencode::bencode_value_len;
use crate::core::metadata_assembler::MetadataAssembler;
use crate::crypto::core::hash_sha1;
use crate::net::handshake::Handshake;
use crate::net::shaper::ShapedStream;
use crate::net::swarm::SwarmEvent;
use crate::net::tracker::{self, AnnounceParams};
use crate::net::transport::PeerStream;
use crate::net::wire::{ExtendedHandshakeDict, MessageDecoder, PeerMessage};

const MAX_PARALLEL_PEERS: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PIECE_TIMEOUT: Duration = Duration::from_secs(15);
const EXT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(60);
/// The id we ask peers to use when they send us ut_metadata messages.
const LOCAL_UT_METADATA_ID: u8 = 1;

const MSG_REQUEST: u8 = 0;
const MSG_DATA: u8 = 1;
const MSG_REJECT: u8 = 2;

#[derive(Debug, Deserialize)]
struct MetadataHeader {
    msg_type: u8,
    piece: u32,
}

#[derive(Default)]
struct Inner {
    assembler: Option<MetadataAssembler>,
    in_flight: HashSet<u32>,
    result: Option<Vec<u8>>,
}

struct Shared {
    info_hash: [u8; 20],
    inner: Mutex<Inner>,
}

impl Shared {
    fn is_done(&self) -> bool {
        self.inner.lock().unwrap().result.is_some()
    }

    fn take_result(&self) -> Option<Vec<u8>> {
        self.inner.lock().unwrap().result.take()
    }

    /// Records the metadata size a peer announced; peers that disagree are rejected.
    fn init_size(&self, size: usize) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        match &inner.assembler {
            Some(assembler) if assembler.size() != size => {
                bail!("peer announced a different metadata size")
            }
            Some(_) => Ok(()),
            None => {
                inner.assembler = Some(MetadataAssembler::new(size)?);
                Ok(())
            }
        }
    }

    fn claim_piece(&self) -> Option<u32> {
        let mut inner = self.inner.lock().unwrap();
        let piece = {
            let assembler = inner.assembler.as_ref()?;
            assembler.next_piece_where(|p| inner.in_flight.contains(&p))?
        };
        inner.in_flight.insert(piece);
        Some(piece)
    }

    fn release_piece(&self, piece: u32) {
        self.inner.lock().unwrap().in_flight.remove(&piece);
    }

    fn add_piece(&self, piece: u32, data: &[u8]) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.in_flight.remove(&piece);
        let Some(assembler) = inner.assembler.as_mut() else {
            bail!("metadata piece arrived before the metadata size was known");
        };
        if !assembler.add_piece(piece, data)? {
            return Ok(());
        }

        let buffer = assembler.get_buffer().to_vec();
        if hash_sha1(&buffer) == self.info_hash {
            inner.result = Some(buffer);
        } else {
            warn!("assembled metadata does not match the info hash, starting over");
            let size = assembler.size();
            inner.assembler = Some(MetadataAssembler::new(size)?);
            inner.in_flight.clear();
        }
        Ok(())
    }
}

pub struct FetchOptions {
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub listen_port: u16,
    pub trackers: Vec<String>,
    /// Peers to try before any tracker or DHT result arrives.
    pub initial_peers: Vec<SocketAddr>,
    pub use_dht: bool,
    pub timeout: Duration,
}

/// Downloads and verifies the info dictionary for a magnet link. Peers come
/// from the trackers, the DHT and `initial_peers`.
pub async fn fetch_metadata(
    options: FetchOptions,
    status: impl Fn(String) + Send,
) -> Result<Vec<u8>> {
    let shared = Arc::new(Shared {
        info_hash: options.info_hash,
        inner: Mutex::new(Inner::default()),
    });

    let (found_tx, mut found_rx) = mpsc::unbounded_channel::<SwarmEvent>();
    let mut dht_task = None;
    if options.use_dht {
        match crate::net::dht::actor::DhtManager::new(0).await {
            Ok((manager, cmd_tx)) => {
                dht_task = Some(tokio::spawn(manager.run()));
                let _ = cmd_tx
                    .send(crate::net::dht::actor::DhtManagerCommand::StartSearch {
                        info_hash: crate::net::dht::routing::NodeId(options.info_hash),
                        announce_port: None,
                        peers_tx: found_tx.clone(),
                    })
                    .await;
            }
            Err(err) => debug!("DHT unavailable for metadata lookup: {err}"),
        }
    }

    let tracker_task = (!options.trackers.is_empty()).then(|| {
        let trackers = options.trackers.clone();
        let (info_hash, peer_id, port) = (options.info_hash, options.peer_id, options.listen_port);
        let tx = found_tx.clone();
        tokio::spawn(async move {
            let mut event = Some("started");
            loop {
                let params = AnnounceParams {
                    info_hash: &info_hash,
                    peer_id: &peer_id,
                    port,
                    uploaded: 0,
                    downloaded: 0,
                    left: 1,
                    event,
                };
                let announced = tracker::announce_all(&trackers, &params, |peers| {
                    let _ = tx.send(SwarmEvent::TrackerPeersReceived(
                        peers.iter().map(|p| p.addr).collect(),
                    ));
                })
                .await;
                if announced.is_ok() {
                    event = None;
                }
                tokio::time::sleep(REANNOUNCE_INTERVAL).await;
            }
        })
    });
    drop(found_tx);

    let mut queue: VecDeque<SocketAddr> = VecDeque::new();
    let mut seen: HashSet<SocketAddr> = HashSet::new();
    for addr in options.initial_peers {
        if seen.insert(addr) {
            queue.push_back(addr);
        }
    }

    let mut peers_tasks: JoinSet<()> = JoinSet::new();
    let mut tick = interval(Duration::from_millis(250));
    let deadline = Instant::now() + options.timeout;
    let mut reported = 0usize;

    let result = loop {
        tokio::select! {
            Some(event) = found_rx.recv() => {
                if let SwarmEvent::TrackerPeersReceived(addrs) | SwarmEvent::DhtPeersReceived(addrs) = event {
                    for addr in addrs {
                        if addr.port() != 0 && seen.insert(addr) {
                            queue.push_back(addr);
                        }
                    }
                }
            }
            _ = peers_tasks.join_next(), if !peers_tasks.is_empty() => {}
            _ = tick.tick() => {}
        }

        if let Some(bytes) = shared.take_result() {
            break Ok(bytes);
        }
        if Instant::now() >= deadline {
            break Err(anyhow::anyhow!(
                "timed out while fetching metadata ({} peers tried)",
                seen.len()
            ));
        }

        while peers_tasks.len() < MAX_PARALLEL_PEERS {
            let Some(addr) = queue.pop_front() else { break };
            let shared = shared.clone();
            let (info_hash, peer_id) = (options.info_hash, options.peer_id);
            peers_tasks.spawn(async move {
                if let Err(err) = fetch_from_peer(addr, info_hash, peer_id, &shared).await {
                    debug!("metadata peer {addr}: {err:#}");
                }
            });
        }

        if seen.len() != reported && seen.len().is_multiple_of(5) {
            reported = seen.len();
            status(format!("Looking for metadata: {reported} peers found"));
        }
    };

    peers_tasks.abort_all();
    if let Some(task) = tracker_task {
        task.abort();
    }
    if let Some(task) = dht_task {
        task.abort();
    }
    result
}

async fn fetch_from_peer(
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    shared: &Shared,
) -> Result<()> {
    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .context("connect timeout")??;
    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(CONNECT_TIMEOUT, stream.write_all(&local)).await??;
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(CONNECT_TIMEOUT, stream.read_exact(&mut incoming)).await??;
    let remote = Handshake::from_bytes(&incoming)?;
    if remote.info_hash != info_hash {
        bail!("info hash mismatch");
    }
    if remote.peer_id == peer_id {
        bail!("connected to ourselves");
    }
    if !remote.supports_extension_protocol() {
        bail!("peer does not support the extension protocol");
    }

    let mut peer_stream = PeerStream::Tcp(stream);
    let mut shaped = ShapedStream::new(&mut peer_stream);

    let mut m = std::collections::HashMap::new();
    m.insert("ut_metadata".to_string(), LOCAL_UT_METADATA_ID);
    let ours = serde_bencode::to_bytes(&ExtendedHandshakeDict {
        m,
        metadata_size: None,
    })?;
    PeerMessage::send_extended(&mut shaped, 0, &ours).await?;

    let mut claimed = None;
    let result = peer_loop(&mut shaped, shared, &mut claimed).await;
    if let Some(piece) = claimed {
        shared.release_piece(piece);
    }
    result
}

async fn peer_loop(
    shaped: &mut ShapedStream<&mut PeerStream>,
    shared: &Shared,
    claimed: &mut Option<u32>,
) -> Result<()> {
    let mut decoder = MessageDecoder::new();
    let mut remote_id: Option<u8> = None;
    let started = Instant::now();
    let mut requested_at = Instant::now();

    loop {
        if shared.is_done() {
            return Ok(());
        }

        if let (Some(id), None) = (remote_id, *claimed) {
            if let Some(piece) = shared.claim_piece() {
                let request = format!("d8:msg_typei{MSG_REQUEST}e5:piecei{piece}ee");
                PeerMessage::send_extended(shaped, id, request.as_bytes()).await?;
                *claimed = Some(piece);
                requested_at = Instant::now();
            }
        }

        let message = match timeout(Duration::from_millis(500), decoder.next(&mut *shaped)).await {
            Ok(message) => message?,
            Err(_) => {
                if claimed.is_some() && requested_at.elapsed() > PIECE_TIMEOUT {
                    bail!("peer is too slow");
                }
                if remote_id.is_none() && started.elapsed() > EXT_HANDSHAKE_TIMEOUT {
                    bail!("no extended handshake");
                }
                continue;
            }
        };

        let PeerMessage::Extended { id, payload } = message else {
            continue;
        };

        if id == 0 {
            if crate::core::bencode::check_bencode_depth(&payload).is_err() {
                bail!("malformed extended handshake");
            }
            let dict: ExtendedHandshakeDict =
                serde_bencode::from_bytes(&payload).context("malformed extended handshake")?;
            let ut_metadata = *dict
                .m
                .get("ut_metadata")
                .context("peer does not offer ut_metadata")?;
            let size = dict
                .metadata_size
                .context("peer did not send metadata_size")?;
            if ut_metadata == 0 {
                bail!("peer disabled ut_metadata");
            }
            shared.init_size(size)?;
            remote_id = Some(ut_metadata);
        } else if id == LOCAL_UT_METADATA_ID {
            let header_len = bencode_value_len(&payload)?;
            let header: MetadataHeader = serde_bencode::from_bytes(&payload[..header_len])?;
            match header.msg_type {
                MSG_DATA => {
                    shared.add_piece(header.piece, &payload[header_len..])?;
                    if *claimed == Some(header.piece) {
                        *claimed = None;
                    }
                }
                MSG_REJECT => bail!("peer rejected the metadata request"),
                MSG_REQUEST => {
                    if let Some(id) = remote_id {
                        let reject = format!("d8:msg_typei{MSG_REJECT}e5:piecei{}ee", header.piece);
                        PeerMessage::send_extended(shaped, id, reject.as_bytes()).await?;
                    }
                }
                _ => {}
            }
        }
    }
}
