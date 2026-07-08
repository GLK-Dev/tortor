use anyhow::{bail, Context, Result};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{timeout, Duration};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

use crate::core::assembler::{AssemblerState, BlockClass, PieceAssembler};
use crate::core::coordinator::CoordinatorMsg;
use crate::core::command::{CoreMessage, SessionTelemetry};
use crate::crypto::dispatch::{hash_piece, HashAlgorithm};
use crate::net::wire::PeerMessage;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TICK: Duration = Duration::from_millis(1500);
const REQUEST_RETRY_TIMEOUT: Duration = Duration::from_secs(3);
const PIPELINE_DEPTH: usize = 5;

#[derive(Debug, Clone)]
struct PeerState {
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
}

impl PeerState {
    fn new() -> Self {
        Self {
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
        }
    }
}

pub async fn run_download_session(
    stream: &mut TcpStream,
    expected_hashes: Arc<Vec<[u8; 20]>>,
    piece_length: u32,
    total_length: Option<u64>,
    peer_addr: SocketAddr,
    ui_sender: mpsc::Sender<CoreMessage>,
    coord_sender: mpsc::Sender<CoordinatorMsg>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let mut state = PeerState::new();

    timeout(IO_TIMEOUT, PeerMessage::send_interested(stream))
        .await
        .context("timeout while sending Interested")??;
    state.am_interested = true;

    loop {
        let (work_tx, work_rx) = oneshot::channel();
        if coord_sender
            .send(CoordinatorMsg::RequestWork(work_tx))
            .await
            .is_err()
        {
            break;
        }

        let target_piece_index = match tokio::select! {
            _ = shutdown_rx.recv() => None,
            reply = work_rx => reply.ok().flatten(),
        } {
            Some(index) => index,
            None => break,
        };

        let expected_hash = match expected_hashes.get(target_piece_index as usize) {
            Some(hash) => *hash,
            None => {
                let _ = coord_sender
                    .send(CoordinatorMsg::PieceFailed(target_piece_index))
                    .await;
                bail!("invalid piece index requested: {}", target_piece_index);
            }
        };

        let target_piece_length = piece_len_at(target_piece_index, piece_length, total_length)
            .ok_or_else(|| anyhow::anyhow!("invalid piece length for piece {}", target_piece_index))?;

        let piece_result = download_piece(
            stream,
            &mut state,
            target_piece_index,
            target_piece_length,
            expected_hash,
            peer_addr,
            &ui_sender,
            &mut shutdown_rx,
        )
        .await;

        match piece_result {
            Ok(PieceOutcome::Completed(buffer)) => {
                if coord_sender
                    .send(CoordinatorMsg::PieceDownloaded(target_piece_index, buffer))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(PieceOutcome::Shutdown) => {
                let _ = coord_sender
                    .send(CoordinatorMsg::PieceFailed(target_piece_index))
                    .await;
                break;
            }
            Err(err) => {
                let _ = coord_sender
                    .send(CoordinatorMsg::PieceFailed(target_piece_index))
                    .await;
                return Err(err);
            }
        }
    }

    Ok(())
}

enum PieceOutcome {
    Completed(Vec<u8>),
    Shutdown,
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

async fn download_piece(
    stream: &mut TcpStream,
    state: &mut PeerState,
    target_piece_index: u32,
    target_piece_length: u32,
    expected_hash: [u8; 20],
    peer_addr: SocketAddr,
    ui_sender: &mpsc::Sender<CoreMessage>,
    shutdown_rx: &mut broadcast::Receiver<()>,
) -> Result<PieceOutcome> {
    let mut assembler = PieceAssembler::new(target_piece_index, target_piece_length);
    let mut telemetry = SessionTelemetry::default();
    let session_start = Instant::now();

    loop {
        if !state.peer_choking {
            while assembler.in_flight_count(REQUEST_RETRY_TIMEOUT) < PIPELINE_DEPTH {
                if let Some((begin, len, is_retry)) = assembler.next_request(REQUEST_RETRY_TIMEOUT) {
                    timeout(
                        IO_TIMEOUT,
                        PeerMessage::send_request(stream, target_piece_index, begin, len),
                    )
                    .await
                    .context("timeout while sending Request")??;

                    if is_retry {
                        telemetry.retries += 1;
                    }
                    telemetry.in_flight_requests = assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                    let _ = ui_sender
                        .send(CoreMessage::TelemetryUpdate(peer_addr, telemetry.clone()))
                        .await;
                } else {
                    break;
                }
            }
        }

        let read_result = tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("download session for {} received shutdown signal", peer_addr);
                return Ok(PieceOutcome::Shutdown);
            }
            result = timeout(READ_TICK, PeerMessage::read_from(stream)) => result,
        };
        match read_result {
            Ok(Ok(msg)) => match msg {
                PeerMessage::KeepAlive => {}
                PeerMessage::Choke => state.peer_choking = true,
                PeerMessage::Unchoke => state.peer_choking = false,
                PeerMessage::Interested => state.peer_interested = true,
                PeerMessage::NotInterested => state.peer_interested = false,
                PeerMessage::Have(_) | PeerMessage::Bitfield(_) | PeerMessage::Request { .. } => {}
                PeerMessage::Piece {
                    index,
                    begin,
                    block,
                } => {
                    if index != assembler.piece_index {
                        telemetry.unexpected_blocks += 1;
                        let _ = ui_sender
                            .send(CoreMessage::TelemetryUpdate(peer_addr, telemetry.clone()))
                            .await;
                        continue;
                    }

                    match assembler.classify_block(begin, block.len() as u32) {
                        BlockClass::Duplicate => {
                            telemetry.duplicate_blocks += 1;
                            telemetry.in_flight_requests =
                                assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                            let _ = ui_sender.send(CoreMessage::TelemetryUpdate(
                                peer_addr,
                                telemetry.clone(),
                            ))
                            .await;
                            continue;
                        }
                        BlockClass::Unexpected => {
                            telemetry.unexpected_blocks += 1;
                            telemetry.in_flight_requests =
                                assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                            let _ = ui_sender.send(CoreMessage::TelemetryUpdate(
                                peer_addr,
                                telemetry.clone(),
                            ))
                            .await;
                            continue;
                        }
                        BlockClass::ExpectedNew => {}
                    }

                    match assembler.add_block(begin, &block) {
                        AssemblerState::InProgress => {
                            telemetry.downloaded_bytes = assembler.received_bytes();
                            telemetry.in_flight_requests =
                                assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                            let _ = ui_sender.send(CoreMessage::TelemetryUpdate(
                                peer_addr,
                                telemetry.clone(),
                            ))
                            .await;
                        }
                        AssemblerState::Error(err) => bail!("assembler error: {err}"),
                        AssemblerState::Complete(buffer) => {
                            telemetry.downloaded_bytes = assembler.received_bytes();
                            telemetry.in_flight_requests =
                                assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                            telemetry.time_to_first_piece_ms =
                                Some(session_start.elapsed().as_millis() as u64);
                            let _ = ui_sender.send(CoreMessage::TelemetryUpdate(
                                peer_addr,
                                telemetry.clone(),
                            ))
                            .await;

                            let actual_hash = hash_piece(&buffer, HashAlgorithm::Sha1);
                            if actual_hash.as_slice() == expected_hash.as_slice() {
                                return Ok(PieceOutcome::Completed(buffer));
                            }
                            bail!("piece hash mismatch for piece {}", target_piece_index);
                        }
                    }
                }
            },
            Ok(Err(err)) => bail!("wire read error: {err}"),
            Err(_) => {
                // Read tick timeout is expected. It lets the loop re-run and retry timed-out requests.
                telemetry.in_flight_requests = assembler.in_flight_count(REQUEST_RETRY_TIMEOUT);
                let _ = ui_sender
                    .send(CoreMessage::TelemetryUpdate(peer_addr, telemetry.clone()))
                    .await;
            }
        }
    }
}
