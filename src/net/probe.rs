use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};

use crate::core::command::SessionEvent;
use crate::net::handshake::Handshake;
use crate::net::session::{self, PeerContext};
use crate::net::transport::PeerStream;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

type Dialed = (PeerStream, bool);

fn check_remote(remote: &Handshake, info_hash: &[u8; 20], peer_id: &[u8; 20]) -> Result<bool> {
    if remote.info_hash != *info_hash {
        bail!("remote info_hash mismatch");
    }
    if remote.peer_id == *peer_id {
        bail!("connected to ourselves");
    }
    Ok(remote.supports_extension_protocol())
}

async fn dial_tcp(addr: SocketAddr, info_hash: [u8; 20], peer_id: [u8; 20]) -> Result<Dialed> {
    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await??;
    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(CONNECT_TIMEOUT, stream.write_all(&local)).await??;
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(CONNECT_TIMEOUT, stream.read_exact(&mut incoming)).await??;
    let remote = Handshake::from_bytes(&incoming)?;
    let supports_ext = check_remote(&remote, &info_hash, &peer_id)?;
    Ok((PeerStream::Tcp(stream), supports_ext))
}

async fn dial_quic(
    endpoint: Arc<quinn::Endpoint>,
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
) -> Result<Dialed> {
    let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr, "tortor.local")?).await??;
    let (mut send_stream, mut recv_stream) = conn.open_bi().await?;
    let local = Handshake::new(info_hash, peer_id).as_bytes();
    timeout(CONNECT_TIMEOUT, send_stream.write_all(&local)).await??;
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(CONNECT_TIMEOUT, recv_stream.read_exact(&mut incoming)).await??;
    let remote = Handshake::from_bytes(&incoming)?;
    let supports_ext = check_remote(&remote, &info_hash, &peer_id)?;
    Ok((PeerStream::Quic(send_stream, recv_stream), supports_ext))
}

/// Dials a peer over TCP and QUIC at the same time, keeps whichever
/// connection completes the handshake first and runs the peer session on it.
pub async fn execute_probe(
    addr: SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    ctx: PeerContext,
    shutdown_rx: broadcast::Receiver<()>,
    announce_rx: broadcast::Receiver<SessionEvent>,
    quic_endpoint: Arc<quinn::Endpoint>,
) -> Result<String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let tx_tcp = tx.clone();
    tokio::spawn(async move {
        let _ = tx_tcp.send(dial_tcp(addr, info_hash, peer_id).await);
    });
    tokio::spawn(async move {
        let _ = tx.send(dial_quic(quic_endpoint, addr, info_hash, peer_id).await);
    });

    let mut first_err = None;
    let (mut peer_stream, supports_ext) = loop {
        match rx.recv().await {
            Some(Ok(dialed)) => break dialed,
            Some(Err(e)) => match first_err.take() {
                None => first_err = Some(e),
                Some(first) => {
                    return Err(first.context(format!("Both TCP and QUIC failed, last err: {e}")));
                }
            },
            None => bail!("Channels closed unexpectedly"),
        }
    };

    let mut shaped_stream = crate::net::shaper::ShapedStream::new(&mut peer_stream);
    session::run_peer_session(
        &mut shaped_stream,
        &ctx,
        addr,
        shutdown_rx,
        announce_rx,
        supports_ext,
    )
    .await?;

    Ok("Peer session finished".to_string())
}
