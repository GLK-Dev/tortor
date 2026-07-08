use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

use crate::net::handshake::Handshake;

pub async fn start_listener(port: u16, info_hash: [u8; 20], peer_id: [u8; 20]) -> Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!("TorTor listening for incoming peers on {addr}");

    loop {
        match listener.accept().await {
            Ok((socket, remote_addr)) => {
                info!("Incoming peer connection from {remote_addr}");
                tokio::spawn(async move {
                    if let Err(err) = handle_peer(socket, remote_addr, info_hash, peer_id).await {
                        error!("Peer session error for {remote_addr}: {err}");
                    }
                });
            }
            Err(err) => error!("Error while accepting peer connection: {err}"),
        }
    }
}

async fn handle_peer(
    mut socket: tokio::net::TcpStream,
    remote_addr: std::net::SocketAddr,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
) -> Result<()> {
    let local_handshake = Handshake::new(info_hash, peer_id).as_bytes();
    socket.write_all(&local_handshake).await?;
    debug!("Handshake sent to {remote_addr}");

    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    socket.read_exact(&mut incoming).await?;
    let remote_handshake = Handshake::from_bytes(&incoming)?;

    if remote_handshake.info_hash != info_hash {
        warn!("Peer {remote_addr} sent handshake for another swarm");
    } else {
        info!("Handshake completed with peer {remote_addr}");
    }

    Ok(())
}
