use std::net::SocketAddr;

use anyhow::{bail, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};

use crate::core::command::SessionEvent;
use crate::net::handshake::Handshake;
use crate::net::session::{self, PeerContext};
use crate::net::transport::PeerStream;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn check_remote(remote: &Handshake, info_hash: &[u8; 20], peer_id: &[u8; 20]) -> Result<bool> {
    if remote.info_hash != *info_hash {
        bail!("inbound peer asked for another swarm");
    }
    if remote.peer_id == *peer_id {
        bail!("connected to ourselves");
    }
    Ok(remote.supports_extension_protocol())
}

/// Handles a TCP connection accepted by the swarm listener.
pub async fn serve_tcp(
    mut stream: TcpStream,
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    ctx: PeerContext,
    shutdown_rx: broadcast::Receiver<()>,
    announce_rx: broadcast::Receiver<SessionEvent>,
) -> Result<()> {
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut incoming)).await??;
    let remote = Handshake::from_bytes(&incoming)?;
    let supports_ext = check_remote(&remote, &info_hash, &peer_id)?;

    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(HANDSHAKE_TIMEOUT, stream.write_all(&local)).await??;

    run(
        PeerStream::Tcp(stream),
        addr,
        ctx,
        shutdown_rx,
        announce_rx,
        supports_ext,
    )
    .await
}

/// Handles a QUIC connection accepted by the swarm endpoint.
pub async fn serve_quic(
    incoming: quinn::Incoming,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    ctx: PeerContext,
    shutdown_rx: broadcast::Receiver<()>,
    announce_rx: broadcast::Receiver<SessionEvent>,
) -> Result<()> {
    let conn = timeout(HANDSHAKE_TIMEOUT, incoming).await??;
    let addr = conn.remote_address();
    let (mut send_stream, mut recv_stream) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi()).await??;

    let mut buf = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(HANDSHAKE_TIMEOUT, recv_stream.read_exact(&mut buf)).await??;
    let remote = Handshake::from_bytes(&buf)?;
    let supports_ext = check_remote(&remote, &info_hash, &peer_id)?;

    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(HANDSHAKE_TIMEOUT, send_stream.write_all(&local)).await??;

    run(
        PeerStream::Quic(send_stream, recv_stream),
        addr,
        ctx,
        shutdown_rx,
        announce_rx,
        supports_ext,
    )
    .await
}

async fn run(
    mut stream: PeerStream,
    addr: SocketAddr,
    ctx: PeerContext,
    shutdown_rx: broadcast::Receiver<()>,
    announce_rx: broadcast::Receiver<SessionEvent>,
    supports_ext: bool,
) -> Result<()> {
    let mut shaped = crate::net::shaper::ShapedStream::new(&mut stream);
    session::run_peer_session(
        &mut shaped,
        &ctx,
        addr,
        shutdown_rx,
        announce_rx,
        supports_ext,
    )
    .await
}
