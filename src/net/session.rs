use anyhow::{bail, Context, Result};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::core::assembler::{AssemblerState, PieceAssembler};
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

pub async fn run_probe_session(
    stream: &mut TcpStream,
    target_piece_index: u32,
    target_piece_length: u32,
    expected_hash: [u8; 20],
) -> Result<String> {
    let mut state = PeerState::new();
    let mut assembler = PieceAssembler::new(target_piece_index, target_piece_length);

    timeout(IO_TIMEOUT, PeerMessage::send_interested(stream))
        .await
        .context("timeout while sending Interested")??;
    state.am_interested = true;

    loop {
        if !state.peer_choking {
            while assembler.in_flight_count(REQUEST_RETRY_TIMEOUT) < PIPELINE_DEPTH {
                if let Some((begin, len)) = assembler.next_request(REQUEST_RETRY_TIMEOUT) {
                    timeout(
                        IO_TIMEOUT,
                        PeerMessage::send_request(stream, target_piece_index, begin, len),
                    )
                    .await
                    .context("timeout while sending Request")??;
                } else {
                    break;
                }
            }
        }

        let read_result = timeout(READ_TICK, PeerMessage::read_from(stream)).await;
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
                        continue;
                    }

                    match assembler.add_block(begin, &block) {
                        AssemblerState::InProgress => {}
                        AssemblerState::Error(err) => bail!("assembler error: {err}"),
                        AssemblerState::Complete(buffer) => {
                            let actual_hash = hash_piece(&buffer, HashAlgorithm::Sha1);
                            if actual_hash.as_slice() == expected_hash.as_slice() {
                                return Ok(format!(
                                    "Handshake OK | Piece {} verified ({} bytes)",
                                    target_piece_index,
                                    target_piece_length
                                ));
                            }
                            bail!("piece hash mismatch for piece {}", target_piece_index);
                        }
                    }
                }
            },
            Ok(Err(err)) => bail!("wire read error: {err}"),
            Err(_) => {
                // Read tick timeout is expected. It lets the loop re-run and retry timed-out requests.
            }
        }
    }
}
