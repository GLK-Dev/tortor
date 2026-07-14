use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{timeout, Duration};

use crate::core::coordinator::CoordinatorMsg;
use crate::net::handshake::Handshake;
use crate::net::session;
use crate::core::command::CoreMessage;
use crate::net::swarm::SwarmEvent;

pub async fn execute_probe(
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    expected_hashes: Arc<Vec<[u8; 20]>>,
    piece_length: u32,
    total_length: Option<u64>,
    ui_sender: mpsc::Sender<CoreMessage>,
    coord_sender: mpsc::Sender<CoordinatorMsg>,
    shutdown_rx: broadcast::Receiver<()>,
    swarm_event_tx: Option<mpsc::UnboundedSender<SwarmEvent>>,
    announce_rx: broadcast::Receiver<u32>,
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

    session::run_download_session(
        &mut stream,
        expected_hashes,
        piece_length,
        total_length,
        addr,
        ui_sender,
        coord_sender,
        shutdown_rx,
        swarm_event_tx,
        announce_rx,
        remote.supports_extension_protocol(),
    )
    .await?;

    Ok("Download worker finished".to_string())
}
