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
    announce_rx: broadcast::Receiver<crate::core::command::SessionEvent>,
    quic_endpoint: Arc<quinn::Endpoint>,
) -> Result<String> {


    // Optimistic Dialing: Race between TCP and QUIC
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    
    let tx_tcp = tx.clone();
    let expected_hashes_tcp = Arc::clone(&expected_hashes);
    tokio::spawn(async move {
        let res = async {
            let stream = timeout(Duration::from_secs(5), TcpStream::connect(addr)).await??;
            let mut stream = stream;
            let local = Handshake::new(info_hash, peer_id).as_bytes();
            timeout(Duration::from_secs(5), stream.write_all(&local)).await??;
            let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
            timeout(Duration::from_secs(5), stream.read_exact(&mut incoming)).await??;
            let remote = Handshake::from_bytes(&incoming)?;
            if remote.info_hash != info_hash {
                bail!("probe failed: remote info_hash mismatch");
            }
            Ok::<(crate::net::transport::PeerStream, bool), anyhow::Error>((crate::net::transport::PeerStream::Tcp(stream), remote.supports_extension_protocol()))
        }.await;
        if let Ok(val) = res {
            let _ = tx_tcp.send(Ok(val));
        } else {
            let _ = tx_tcp.send(Err(res.unwrap_err()));
        }
    });

    let tx_quic = tx.clone();
    let quic_endpoint_cloned = Arc::clone(&quic_endpoint);
    tokio::spawn(async move {
        let res = async {
            let conn = quic_endpoint_cloned.connect(addr, "tortor.local")?.await?;
            let (mut send_stream, mut recv_stream) = conn.open_bi().await?;
            let local = Handshake::new(info_hash, peer_id).as_bytes();
            timeout(Duration::from_secs(5), send_stream.write_all(&local)).await??;
            let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
            timeout(Duration::from_secs(5), recv_stream.read_exact(&mut incoming)).await??;
            let remote = Handshake::from_bytes(&incoming)?;
            if remote.info_hash != info_hash {
                bail!("probe failed: remote info_hash mismatch");
            }
            Ok::<(crate::net::transport::PeerStream, bool), anyhow::Error>((crate::net::transport::PeerStream::Quic(send_stream, recv_stream), remote.supports_extension_protocol()))
        }.await;
        if let Ok(val) = res {
            let _ = tx_quic.send(Ok(val));
        } else {
            let _ = tx_quic.send(Err(res.unwrap_err()));
        }
    });

        let mut first_err = None;
    let (mut peer_stream, supports_ext) = loop {
        match rx.recv().await {
            Some(Ok(val)) => break val,
            Some(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                } else {
                    return Err(first_err.unwrap().context(format!("Both TCP and QUIC failed, last err: {}", e)));
                }
            }
            None => return Err(anyhow::anyhow!("Channels closed unexpectedly")),
        }
    };
    let mut shaped_stream = crate::net::shaper::ShapedStream::new(&mut peer_stream);
    session::run_download_session(
        &mut shaped_stream,
        expected_hashes,
        piece_length,
        total_length,
        addr,
        ui_sender,
        coord_sender,
        shutdown_rx,
        swarm_event_tx,
        announce_rx,
        supports_ext,
    )
    .await?;

    Ok("Download worker finished".to_string())
}
