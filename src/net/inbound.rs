use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

use crate::net::engine::{Engine, TorrentRegistration};
use crate::net::handshake::Handshake;
use crate::net::session;
use crate::net::transport::PeerStream;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Looks the remote's info hash up among the registered torrents.
fn route(engine: &Engine, remote: &Handshake) -> Result<(TorrentRegistration, bool)> {
    if remote.peer_id == engine.peer_id {
        bail!("connected to ourselves");
    }
    let registration = engine
        .lookup(&remote.info_hash)
        .context("inbound peer asked for a torrent we do not serve")?;
    Ok((registration, remote.supports_extension_protocol()))
}

/// Handles a TCP connection accepted by the engine listener.
pub async fn serve_tcp(mut stream: TcpStream, addr: SocketAddr, engine: Arc<Engine>) -> Result<()> {
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(HANDSHAKE_TIMEOUT, stream.read_exact(&mut incoming)).await??;
    let remote = Handshake::from_bytes(&incoming)?;
    let (registration, supports_ext) = route(&engine, &remote)?;

    let local = Handshake::new(remote.info_hash, engine.peer_id).as_bytes();
    timeout(HANDSHAKE_TIMEOUT, stream.write_all(&local)).await??;

    run(PeerStream::Tcp(stream), addr, registration, supports_ext).await
}

/// Handles a QUIC connection accepted by the engine endpoint.
pub async fn serve_quic(incoming: quinn::Incoming, engine: Arc<Engine>) -> Result<()> {
    let conn = timeout(HANDSHAKE_TIMEOUT, incoming).await??;
    let addr = conn.remote_address();
    let (mut send_stream, mut recv_stream) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi()).await??;

    let mut buf = [0u8; Handshake::HANDSHAKE_LEN];
    timeout(HANDSHAKE_TIMEOUT, recv_stream.read_exact(&mut buf)).await??;
    let remote = Handshake::from_bytes(&buf)?;
    let (registration, supports_ext) = route(&engine, &remote)?;

    let local = Handshake::new(remote.info_hash, engine.peer_id).as_bytes();
    timeout(HANDSHAKE_TIMEOUT, send_stream.write_all(&local)).await??;

    run(
        PeerStream::Quic(send_stream, recv_stream),
        addr,
        registration,
        supports_ext,
    )
    .await
}

async fn run(
    mut stream: PeerStream,
    addr: SocketAddr,
    registration: TorrentRegistration,
    supports_ext: bool,
) -> Result<()> {
    let mut shaped = crate::net::shaper::ShapedStream::new(&mut stream);
    session::run_peer_session(
        &mut shaped,
        &registration.ctx,
        addr,
        registration.shutdown_tx.subscribe(),
        registration.announce_tx.subscribe(),
        supports_ext,
    )
    .await
}
