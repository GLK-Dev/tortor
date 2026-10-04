use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, timeout, Duration, MissedTickBehavior};
use tracing::{debug, info};

use crate::core::assembler::{AssemblerState, BlockClass, PieceAssembler};
use crate::core::bitfield::Bitfield;
use crate::core::command::{CoreMessage, SessionEvent, SessionTelemetry};
use crate::core::coordinator::CoordinatorMsg;
use crate::crypto::dispatch::{hash_piece, HashAlgorithm};
use crate::net::swarm::SwarmEvent;
use crate::net::transport::PeerStream;
use crate::net::wire::{ExtendedHandshakeDict, MessageDecoder, PeerMessage, MAX_BLOCK_REQUEST};

type Stream<'a, 'b> = &'a mut crate::net::shaper::ShapedStream<&'b mut PeerStream>;

const IO_TIMEOUT: Duration = Duration::from_secs(10);
const TICK: Duration = Duration::from_secs(1);
const REQUEST_RETRY_TIMEOUT: Duration = Duration::from_secs(5);
const PIPELINE_DEPTH: usize = 16;
const WORK_POLL_INTERVAL: Duration = Duration::from_secs(2);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(90);
const PEER_SILENCE_TIMEOUT: Duration = Duration::from_secs(150);
/// A peer that neither sends us data nor receives data from us for this long is dropped.
const USELESS_TIMEOUT: Duration = Duration::from_secs(120);
/// Unchoked peer that delivers no block for this long is considered stalled.
const BLOCK_STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// A piece reserved while the peer keeps us choked is given back after this long.
const CHOKED_JOB_TIMEOUT: Duration = Duration::from_secs(30);
/// Number of peers a torrent uploads to at the same time.
pub const MAX_UPLOAD_SLOTS: usize = 8;

/// Shared counter that bounds how many peers we leave unchoked.
#[derive(Debug)]
pub struct UploadSlots {
    max: usize,
    used: AtomicUsize,
}

impl UploadSlots {
    pub fn new(max: usize) -> Arc<Self> {
        Arc::new(Self {
            max,
            used: AtomicUsize::new(0),
        })
    }

    pub fn try_acquire(&self) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.max).then_some(used + 1)
            })
            .is_ok()
    }

    pub fn release(&self) {
        let _ = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                Some(used.saturating_sub(1))
            });
    }
}

/// Bytes moved during this run, reported to trackers.
#[derive(Debug, Default)]
pub struct TransferStats {
    pub downloaded: AtomicU64,
    pub uploaded: AtomicU64,
}

/// Everything a peer session needs from its torrent.
#[derive(Clone)]
pub struct PeerContext {
    pub expected_hashes: Arc<Vec<[u8; 20]>>,
    pub piece_length: u32,
    pub total_length: Option<u64>,
    pub ui_sender: mpsc::Sender<CoreMessage>,
    pub coord_sender: mpsc::Sender<CoordinatorMsg>,
    pub swarm_event_tx: Option<mpsc::UnboundedSender<SwarmEvent>>,
    pub upload_slots: Arc<UploadSlots>,
    pub stats: Arc<TransferStats>,
}

struct Job {
    index: u32,
    expected_hash: [u8; 20],
    assembler: PieceAssembler,
    last_block: Instant,
}

struct PeerSession<'a, 'b> {
    stream: Stream<'a, 'b>,
    ctx: &'a PeerContext,
    peer_addr: SocketAddr,
    decoder: MessageDecoder,
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
    holds_upload_slot: bool,
    remote_pex_id: Option<u8>,
    last_sent_peers: HashSet<SocketAddr>,
    have: Bitfield,
    job: Option<Job>,
    telemetry: SessionTelemetry,
    session_start: Instant,
    next_work_poll: Instant,
    last_recv: Instant,
    last_useful: Instant,
    last_keepalive: Instant,
}

/// Runs the full life of one peer connection: downloads the pieces the peer
/// has, serves the pieces it asks for, and exchanges PEX. Returns when the
/// peer is dropped, the connection fails or shutdown is requested.
pub async fn run_peer_session(
    stream: Stream<'_, '_>,
    ctx: &PeerContext,
    peer_addr: SocketAddr,
    mut shutdown_rx: broadcast::Receiver<()>,
    mut announce_rx: broadcast::Receiver<SessionEvent>,
    remote_supports_extensions: bool,
) -> Result<()> {
    let now = Instant::now();
    let mut session = PeerSession {
        stream,
        ctx,
        peer_addr,
        decoder: MessageDecoder::new(),
        am_choking: true,
        am_interested: false,
        peer_choking: true,
        peer_interested: false,
        holds_upload_slot: false,
        remote_pex_id: None,
        last_sent_peers: HashSet::new(),
        have: Bitfield::new(ctx.expected_hashes.len()),
        job: None,
        telemetry: SessionTelemetry::default(),
        session_start: now,
        next_work_poll: now,
        last_recv: now,
        last_useful: now,
        last_keepalive: now,
    };

    let result = session
        .run(
            &mut shutdown_rx,
            &mut announce_rx,
            remote_supports_extensions,
        )
        .await;
    session.finish().await;
    result
}

impl PeerSession<'_, '_> {
    async fn run(
        &mut self,
        shutdown_rx: &mut broadcast::Receiver<()>,
        announce_rx: &mut broadcast::Receiver<SessionEvent>,
        remote_supports_extensions: bool,
    ) -> Result<()> {
        self.send_opening_messages(remote_supports_extensions)
            .await?;

        let mut tick = interval(TICK);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut announce_open = true;

        loop {
            self.maybe_request_work().await?;
            self.fill_pipeline().await?;

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("peer session for {} received shutdown signal", self.peer_addr);
                    return Ok(());
                }
                event = announce_rx.recv(), if announce_open => match event {
                    Ok(event) => self.handle_event(event).await?,
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => announce_open = false,
                },
                msg = self.decoder.next(&mut *self.stream) => {
                    let msg = msg?;
                    self.last_recv = Instant::now();
                    self.handle_message(msg).await?;
                }
                _ = tick.tick() => {
                    if !self.housekeeping().await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn send_opening_messages(&mut self, remote_supports_extensions: bool) -> Result<()> {
        if remote_supports_extensions {
            let mut m = std::collections::HashMap::new();
            m.insert("ut_metadata".to_string(), 1);
            m.insert("ut_pex".to_string(), 2);
            let ext_dict = ExtendedHandshakeDict {
                m,
                metadata_size: None,
            };
            if let Ok(payload) = serde_bencode::to_bytes(&ext_dict) {
                let _ = PeerMessage::send_extended(&mut *self.stream, 0, &payload).await;
            }
        }

        let (tx, rx) = oneshot::channel();
        self.ctx
            .coord_sender
            .send(CoordinatorMsg::GetCompletedPieces(tx))
            .await
            .context("coordinator is gone")?;
        if let Ok(completed) = rx.await {
            if !completed.is_empty() {
                let bitfield = Bitfield::from_indices(self.ctx.expected_hashes.len(), completed);
                timeout(
                    IO_TIMEOUT,
                    PeerMessage::send_bitfield(&mut *self.stream, bitfield.as_bytes()),
                )
                .await
                .context("timeout while sending Bitfield")??;
            }
        }

        timeout(IO_TIMEOUT, PeerMessage::send_interested(&mut *self.stream))
            .await
            .context("timeout while sending Interested")??;
        self.am_interested = true;
        Ok(())
    }

    async fn push_telemetry(&mut self) {
        if let Some(job) = &self.job {
            self.telemetry.in_flight_requests =
                job.assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
            self.telemetry.downloaded_bytes = job.assembler.received_bytes();
        }
        let _ = self
            .ctx
            .ui_sender
            .send(CoreMessage::TelemetryUpdate(
                self.peer_addr,
                self.telemetry.clone(),
            ))
            .await;
    }

    /// Reserves a piece from the coordinator when we are idle, unchoked and
    /// the peer advertises something.
    async fn maybe_request_work(&mut self) -> Result<()> {
        if self.job.is_some()
            || self.peer_choking
            || self.have.count() == 0
            || Instant::now() < self.next_work_poll
        {
            return Ok(());
        }

        let (tx, rx) = oneshot::channel();
        self.ctx
            .coord_sender
            .send(CoordinatorMsg::RequestWork {
                have: self.have.clone(),
                reply: tx,
            })
            .await
            .context("coordinator is gone")?;

        let Some(index) = rx.await.ok().flatten() else {
            self.next_work_poll = Instant::now() + WORK_POLL_INTERVAL;
            return Ok(());
        };

        let hash = self.ctx.expected_hashes.get(index as usize).copied();
        let length = piece_len_at(index, self.ctx.piece_length, self.ctx.total_length);
        let (Some(expected_hash), Some(length)) = (hash, length) else {
            let _ = self
                .ctx
                .coord_sender
                .send(CoordinatorMsg::PieceFailed(index))
                .await;
            bail!("coordinator assigned invalid piece {index}");
        };

        self.job = Some(Job {
            index,
            expected_hash,
            assembler: PieceAssembler::new(index, length),
            last_block: Instant::now(),
        });
        self.telemetry.downloaded_bytes = 0;
        Ok(())
    }

    async fn fill_pipeline(&mut self) -> Result<()> {
        if self.peer_choking {
            return Ok(());
        }
        let Some(job) = self.job.as_mut() else {
            return Ok(());
        };

        while job.assembler.in_flight_count(REQUEST_RETRY_TIMEOUT) < PIPELINE_DEPTH {
            let Some((begin, len, is_retry)) = job.assembler.next_request(REQUEST_RETRY_TIMEOUT)
            else {
                break;
            };
            timeout(
                IO_TIMEOUT,
                PeerMessage::send_request(&mut *self.stream, job.index, begin, len),
            )
            .await
            .context("timeout while sending Request")??;
            if is_retry {
                self.telemetry.retries += 1;
            }
        }
        Ok(())
    }

    async fn handle_event(&mut self, event: SessionEvent) -> Result<()> {
        match event {
            SessionEvent::PieceCompleted(index) => {
                let _ = PeerMessage::send_have(&mut *self.stream, index).await;
                // Another peer finished the piece we were still fetching (endgame).
                if self.job.as_ref().is_some_and(|job| job.index == index) {
                    self.job = None;
                }
            }
            SessionEvent::DownloadComplete => {
                if self.am_interested {
                    let _ = PeerMessage::send_not_interested(&mut *self.stream).await;
                    self.am_interested = false;
                }
            }
            SessionEvent::ActivePeersSnapshot(current_peers) => {
                let Some(remote_id) = self.remote_pex_id else {
                    return Ok(());
                };
                let current_set: HashSet<SocketAddr> = current_peers.into_iter().collect();

                let dropped: Vec<SocketAddr> = self
                    .last_sent_peers
                    .difference(&current_set)
                    .copied()
                    .take(50)
                    .collect();
                let added: Vec<SocketAddr> = current_set
                    .difference(&self.last_sent_peers)
                    .copied()
                    .filter(|addr| *addr != self.peer_addr)
                    .take(50)
                    .collect();

                if !added.is_empty() || !dropped.is_empty() {
                    self.last_sent_peers = current_set;
                    let pex_msg = crate::net::pex::PexMessage {
                        added: crate::net::pex::encode_compact_ipv4(&added),
                        added_f: vec![0; added.len()],
                        dropped: crate::net::pex::encode_compact_ipv4(&dropped),
                    };
                    if let Ok(payload) = serde_bencode::to_bytes(&pex_msg) {
                        let _ = PeerMessage::send_extended(&mut *self.stream, remote_id, &payload)
                            .await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_message(&mut self, msg: PeerMessage) -> Result<()> {
        match msg {
            PeerMessage::KeepAlive => {}
            PeerMessage::Choke => {
                self.peer_choking = true;
                // The peer discards our outstanding requests when it chokes us.
                if let Some(job) = self.job.as_mut() {
                    job.assembler.reset_requests();
                }
            }
            PeerMessage::Unchoke => {
                self.peer_choking = false;
                self.next_work_poll = Instant::now();
            }
            PeerMessage::Interested => {
                self.peer_interested = true;
                self.try_unchoke().await?;
            }
            PeerMessage::NotInterested => {
                self.peer_interested = false;
                self.choke().await?;
            }
            PeerMessage::Have(index) => {
                let index = index as usize;
                if index >= self.have.len() {
                    bail!("peer sent HAVE for out-of-range piece {index}");
                }
                if self.have.set(index) {
                    let _ = self
                        .ctx
                        .coord_sender
                        .send(CoordinatorMsg::PeerHave(index as u32))
                        .await;
                    self.next_work_poll = Instant::now();
                }
            }
            PeerMessage::Bitfield(payload) => {
                let Some(new_have) = Bitfield::from_wire(self.have.len(), &payload) else {
                    bail!("peer sent a malformed BITFIELD ({} bytes)", payload.len());
                };
                let old = std::mem::replace(&mut self.have, new_have);
                if old.count() > 0 {
                    let _ = self
                        .ctx
                        .coord_sender
                        .send(CoordinatorMsg::PeerGone(old))
                        .await;
                }
                let _ = self
                    .ctx
                    .coord_sender
                    .send(CoordinatorMsg::PeerBitfield(self.have.clone()))
                    .await;
                self.next_work_poll = Instant::now();
            }
            PeerMessage::Extended { id, payload } => self.handle_extended(id, payload),
            PeerMessage::Request {
                index,
                begin,
                length,
            } => self.handle_request(index, begin, length).await?,
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => self.handle_piece(index, begin, block).await?,
        }
        Ok(())
    }

    fn handle_extended(&mut self, id: u8, payload: Vec<u8>) {
        if crate::core::bencode::check_bencode_depth(&payload).is_err() {
            return;
        }

        if id == 0 {
            if let Ok(ext_dict) = serde_bencode::from_bytes::<ExtendedHandshakeDict>(&payload) {
                debug!("Extended handshake from {}: {:?}", self.peer_addr, ext_dict);
                if let Some(&remote_pex) = ext_dict.m.get("ut_pex") {
                    self.remote_pex_id = Some(remote_pex);
                }
            }
        } else if Some(id) == self.remote_pex_id {
            if let Ok(pex_msg) = serde_bencode::from_bytes::<crate::net::pex::PexMessage>(&payload)
            {
                let addrs = pex_msg.decode_added_ipv4();
                if !addrs.is_empty() {
                    if let Some(tx) = self.ctx.swarm_event_tx.as_ref() {
                        let _ = tx.send(SwarmEvent::PexPeersReceived(addrs));
                    }
                }
            }
        }
    }

    async fn handle_request(&mut self, index: u32, begin: u32, length: u32) -> Result<()> {
        if self.am_choking {
            return Ok(());
        }

        let valid = length > 0
            && length <= MAX_BLOCK_REQUEST
            && piece_len_at(index, self.ctx.piece_length, self.ctx.total_length)
                .is_some_and(|piece_len| (begin as u64) + (length as u64) <= piece_len as u64);
        if !valid {
            bail!(
                "invalid REQUEST from {}: index={index} begin={begin} length={length}",
                self.peer_addr
            );
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        self.ctx
            .coord_sender
            .send(CoordinatorMsg::ReadPiece {
                index,
                begin,
                length,
                reply: reply_tx,
            })
            .await
            .context("coordinator is gone")?;

        if let Ok(Some(block)) = reply_rx.await {
            timeout(
                IO_TIMEOUT,
                PeerMessage::send_piece(&mut *self.stream, index, begin, &block),
            )
            .await
            .context("timeout while sending Piece")?
            .with_context(|| format!("failed to send piece {index}"))?;
            self.last_useful = Instant::now();
            self.ctx
                .stats
                .uploaded
                .fetch_add(block.len() as u64, Ordering::Relaxed);
            let _ = self
                .ctx
                .ui_sender
                .send(CoreMessage::BytesTransferred(0, block.len()))
                .await;
        }
        Ok(())
    }

    async fn handle_piece(&mut self, index: u32, begin: u32, block: Vec<u8>) -> Result<()> {
        self.ctx
            .stats
            .downloaded
            .fetch_add(block.len() as u64, Ordering::Relaxed);
        let _ = self
            .ctx
            .ui_sender
            .send(CoreMessage::BytesTransferred(block.len(), 0))
            .await;

        let class = match self.job.as_ref() {
            Some(job) if job.index == index => {
                job.assembler.classify_block(begin, block.len() as u32)
            }
            _ => BlockClass::Unexpected,
        };
        match class {
            BlockClass::Duplicate => {
                self.telemetry.duplicate_blocks += 1;
                self.push_telemetry().await;
                return Ok(());
            }
            BlockClass::Unexpected => {
                self.telemetry.unexpected_blocks += 1;
                self.push_telemetry().await;
                return Ok(());
            }
            BlockClass::ExpectedNew => {}
        }

        let Some(job) = self.job.as_mut() else {
            return Ok(());
        };
        job.last_block = Instant::now();
        self.last_useful = job.last_block;

        match job.assembler.add_block(begin, &block) {
            AssemblerState::InProgress => {
                if let Some(tx) = &self.ctx.swarm_event_tx {
                    let _ = tx.send(SwarmEvent::PeerProgress(self.peer_addr, block.len() as u32));
                }
                self.push_telemetry().await;
                Ok(())
            }
            AssemblerState::Error(err) => bail!("assembler error: {err}"),
            AssemblerState::Complete(buffer) => {
                if let Some(tx) = &self.ctx.swarm_event_tx {
                    let _ = tx.send(SwarmEvent::PeerProgress(self.peer_addr, block.len() as u32));
                }
                self.telemetry
                    .time_to_first_piece_ms
                    .get_or_insert(self.session_start.elapsed().as_millis() as u64);
                self.push_telemetry().await;

                let job = self.job.take().context("job disappeared")?;
                let expected = job.expected_hash;
                let (buffer, matches) = tokio::task::spawn_blocking(move || {
                    let actual = hash_piece(&buffer, HashAlgorithm::Sha1);
                    let matches = actual.as_slice() == expected.as_slice();
                    (buffer, matches)
                })
                .await
                .context("hashing task failed")?;

                if !matches {
                    let _ = self
                        .ctx
                        .coord_sender
                        .send(CoordinatorMsg::PieceFailed(job.index))
                        .await;
                    bail!("piece hash mismatch for piece {}", job.index);
                }

                self.ctx
                    .coord_sender
                    .send(CoordinatorMsg::PieceDownloaded(job.index, buffer))
                    .await
                    .context("coordinator is gone")?;
                Ok(())
            }
        }
    }

    async fn try_unchoke(&mut self) -> Result<()> {
        if self.am_choking && self.peer_interested && self.ctx.upload_slots.try_acquire() {
            self.holds_upload_slot = true;
            PeerMessage::send_unchoke(&mut *self.stream).await?;
            self.am_choking = false;
        }
        Ok(())
    }

    async fn choke(&mut self) -> Result<()> {
        if !self.am_choking {
            PeerMessage::send_choke(&mut *self.stream).await?;
            self.am_choking = true;
        }
        self.release_upload_slot();
        Ok(())
    }

    fn release_upload_slot(&mut self) {
        if self.holds_upload_slot {
            self.ctx.upload_slots.release();
            self.holds_upload_slot = false;
        }
    }

    /// Periodic checks. Returns `false` when the session should end.
    async fn housekeeping(&mut self) -> Result<bool> {
        let now = Instant::now();

        if now.duration_since(self.last_recv) > PEER_SILENCE_TIMEOUT {
            bail!("peer sent nothing for {}s", PEER_SILENCE_TIMEOUT.as_secs());
        }

        if let Some(tx) = &self.ctx.swarm_event_tx {
            let _ = tx.send(SwarmEvent::PeerProgress(self.peer_addr, 0));
        }

        if let Some(job) = &self.job {
            let silent = now.duration_since(job.last_block);
            if self.peer_choking && silent > CHOKED_JOB_TIMEOUT {
                let index = job.index;
                self.job = None;
                let _ = self
                    .ctx
                    .coord_sender
                    .send(CoordinatorMsg::PieceFailed(index))
                    .await;
            } else if !self.peer_choking && silent > BLOCK_STALL_TIMEOUT {
                bail!(
                    "peer delivered no data for {}s, dropping it",
                    silent.as_secs()
                );
            }
        }

        if self.peer_interested && self.am_choking {
            self.try_unchoke().await?;
        }

        if now.duration_since(self.last_keepalive) >= KEEPALIVE_INTERVAL {
            PeerMessage::send_keepalive(&mut *self.stream).await?;
            self.last_keepalive = now;
        }

        let idle = self.job.is_none()
            && now.duration_since(self.last_useful) > USELESS_TIMEOUT
            && (!self.peer_interested || self.am_choking);
        if idle {
            info!("dropping idle peer {}", self.peer_addr);
            return Ok(false);
        }

        if self.job.is_some() {
            self.push_telemetry().await;
        }
        Ok(true)
    }

    /// Releases everything the session reserved, whatever way it ended.
    async fn finish(&mut self) {
        if let Some(job) = self.job.take() {
            let _ = self
                .ctx
                .coord_sender
                .send(CoordinatorMsg::PieceFailed(job.index))
                .await;
        }
        if self.have.count() > 0 {
            let len = self.have.len();
            let have = std::mem::replace(&mut self.have, Bitfield::new(len));
            let _ = self
                .ctx
                .coord_sender
                .send(CoordinatorMsg::PeerGone(have))
                .await;
        }
        self.release_upload_slot();
    }
}

fn piece_len_at(index: u32, piece_length: u32, total_length: Option<u64>) -> Option<u32> {
    if let Some(total) = total_length {
        let piece_len = piece_length as u64;
        let start = (index as u64).checked_mul(piece_len)?;
        if start >= total {
            return None;
        }
        let remaining = total - start;
        return Some(std::cmp::min(piece_len, remaining) as u32);
    }

    Some(piece_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_len_handles_last_piece_and_bounds() {
        assert_eq!(piece_len_at(0, 100, Some(250)), Some(100));
        assert_eq!(piece_len_at(2, 100, Some(250)), Some(50));
        assert_eq!(piece_len_at(3, 100, Some(250)), None);
        assert_eq!(piece_len_at(u32::MAX, u32::MAX, Some(10)), None);
    }

    #[test]
    fn upload_slots_are_bounded_and_released() {
        let slots = UploadSlots::new(2);
        assert!(slots.try_acquire());
        assert!(slots.try_acquire());
        assert!(!slots.try_acquire());
        slots.release();
        assert!(slots.try_acquire());
        slots.release();
        slots.release();
        slots.release();
        assert!(slots.try_acquire());
    }
}
