use std::net::SocketAddr;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::net::handshake::Handshake;
use crate::net::wire::PeerMessage;

pub async fn execute_probe(
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
) -> Result<String> {
    let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .context("probe connection timeout")??;

    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(Duration::from_secs(5), stream.write_all(&local))
        .await
        .context("probe write timeout")??;

    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(Duration::from_secs(5), stream.read_exact(&mut incoming))
        .await
        .context("probe read timeout")??;

    let remote = Handshake::from_bytes(&incoming).context("invalid remote handshake")?;
    if remote.info_hash != info_hash {
        bail!("probe failed: remote info_hash mismatch");
    }

    let first_msg = timeout(Duration::from_secs(5), PeerMessage::read_from(&mut stream))
        .await
        .context("timeout waiting for first wire message")??;

    let status_info = match first_msg {
        PeerMessage::Bitfield(data) => format!("Bitfield ({} bytes)", data.len()),
        PeerMessage::Have(idx) => format!("Have piece {idx}"),
        PeerMessage::Unchoke => "Unchoked immediately".to_string(),
        msg => format!("Received {:?}", msg),
    };

    timeout(Duration::from_secs(5), PeerMessage::send_interested(&mut stream))
        .await
        .context("timeout while sending Interested")??;

    Ok(format!("Handshake OK | {status_info}"))
}
