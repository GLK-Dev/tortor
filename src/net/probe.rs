use std::net::SocketAddr;
use std::sync::mpsc::Sender;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::net::handshake::Handshake;
use crate::net::session;
use crate::core::command::CoreMessage;

pub async fn execute_probe(
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    target_piece_index: u32,
    target_piece_length: u32,
    expected_piece_hash: [u8; 20],
    ui_sender: &Sender<CoreMessage>,
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

    session::run_probe_session(
        &mut stream,
        target_piece_index,
        target_piece_length,
        expected_piece_hash,
        addr,
        ui_sender,
    )
    .await
}
