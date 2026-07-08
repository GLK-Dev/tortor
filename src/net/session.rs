use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

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

pub async fn run_probe_session(stream: &mut TcpStream) -> Result<String> {
    let mut state = PeerState::new();

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
                if !state.requested_first_block {
                    timeout(
                        READ_TIMEOUT,
                        PeerMessage::send_request(stream, 0, 0, REQUEST_BLOCK_LEN),
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
                return Ok(format!(
                    "Handshake OK | Piece {}:{} ({} bytes)",
                    index,
                    begin,
                    block.len()
                ));
            }
        }
    }

    Ok(format!(
        "Handshake OK | no piece within {} messages (am_choking={}, peer_choking={}, requested={})",
        MAX_MESSAGES, state.am_choking, state.peer_choking, state.requested_first_block
    ))
}
