use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::crypto::dispatch::{hash_piece, HashAlgorithm};
use crate::net::wire::PeerMessage;

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MESSAGES: usize = 64;
const REQUEST_BLOCK_LEN: u32 = 16 * 1024;

#[derive(Debug, Clone)]
struct PeerState {
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
    requested_first_block: bool,
}

impl PeerState {
    fn new() -> Self {
        Self {
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
            requested_first_block: false,
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
    let mut piece_buffer = Vec::with_capacity(target_piece_length as usize);
    let mut current_begin = 0u32;

    timeout(READ_TIMEOUT, PeerMessage::send_interested(stream))
        .await
        .context("timeout while sending Interested")??;
    state.am_interested = true;

    for _ in 0..MAX_MESSAGES {
        let msg = timeout(READ_TIMEOUT, PeerMessage::read_from(stream))
            .await
            .context("timeout waiting for wire message")??;

        match msg {
            PeerMessage::KeepAlive => continue,
            PeerMessage::Choke => state.peer_choking = true,
            PeerMessage::Unchoke => {
                state.peer_choking = false;
                if !state.requested_first_block && current_begin < target_piece_length {
                    let req_len = std::cmp::min(REQUEST_BLOCK_LEN, target_piece_length - current_begin);
                    timeout(
                        READ_TIMEOUT,
                        PeerMessage::send_request(
                            stream,
                            target_piece_index,
                            current_begin,
                            req_len,
                        ),
                    )
                    .await
                    .context("timeout while sending Request")??;
                    state.requested_first_block = true;
                }
            }
            PeerMessage::Interested => state.peer_interested = true,
            PeerMessage::NotInterested => state.peer_interested = false,
            PeerMessage::Have(_) | PeerMessage::Bitfield(_) | PeerMessage::Request { .. } => {}
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => {
                if index != target_piece_index || begin != current_begin {
                    anyhow::bail!(
                        "unexpected piece block coordinates index={}, begin={} (expected index={}, begin={})",
                        index,
                        begin,
                        target_piece_index,
                        current_begin
                    );
                }

                if block.is_empty() {
                    anyhow::bail!("peer sent empty piece block");
                }

                let remaining = target_piece_length
                    .checked_sub(current_begin)
                    .ok_or_else(|| anyhow::anyhow!("piece offset overflow"))? as usize;
                if block.len() > remaining {
                    anyhow::bail!(
                        "peer sent oversized block: got {}, remaining {}",
                        block.len(),
                        remaining
                    );
                }

                piece_buffer.extend_from_slice(&block);
                current_begin += block.len() as u32;

                if current_begin == target_piece_length {
                    let actual_hash = hash_piece(&piece_buffer, HashAlgorithm::Sha1);
                    if actual_hash.as_slice() == expected_hash.as_slice() {
                        return Ok(format!(
                            "Handshake OK | Piece {} verified ({} bytes)",
                            target_piece_index,
                            target_piece_length
                        ));
                    }
                    anyhow::bail!("piece hash mismatch for piece {}", target_piece_index);
                }

                if !state.peer_choking {
                    let req_len =
                        std::cmp::min(REQUEST_BLOCK_LEN, target_piece_length - current_begin);
                    timeout(
                        READ_TIMEOUT,
                        PeerMessage::send_request(
                            stream,
                            target_piece_index,
                            current_begin,
                            req_len,
                        ),
                    )
                    .await
                    .context("timeout while sending next Request")??;
                }
            }
        }
    }

    Ok(format!(
        "Handshake OK | no verified piece within {} messages (am_choking={}, peer_choking={}, downloaded={}/{})",
        MAX_MESSAGES,
        state.am_choking,
        state.peer_choking,
        current_begin,
        target_piece_length
    ))
}
