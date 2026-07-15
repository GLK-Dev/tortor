use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use tracing::{debug, info, warn};
use url::form_urlencoded::byte_serialize;
use url::Url;
use tokio::net::UdpSocket;

#[derive(Debug, Clone, Copy)]
pub struct PeerInfo {
    pub addr: SocketAddr,
}

#[derive(Debug, Deserialize)]
struct TrackerResponse {
    peers: Option<ByteBuf>,
    interval: Option<u64>,
    #[serde(rename = "failure reason")]
    failure_reason: Option<String>,
    #[serde(rename = "warning message")]
    warning_message: Option<String>,
}

pub async fn announce(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    left: u64,
    event: Option<&str>,
) -> Result<Vec<PeerInfo>> {
    if tracker_url.starts_with("udp://") {
        return announce_udp(tracker_url, info_hash, peer_id, port, left, event).await;
    }

    let info_hash_encoded: String = byte_serialize(info_hash).collect();
    let peer_id_encoded: String = byte_serialize(peer_id).collect();

    let separator = if tracker_url.contains('?') { '&' } else { '?' };
    let event_param = match event {
        Some(e) => format!("&event={}", e),
        None => "".to_string(),
    };
    
    let request_url = format!(
        "{tracker_url}{separator}info_hash={info_hash_encoded}&peer_id={peer_id_encoded}&port={port}&uploaded=0&downloaded=0&left={left}&compact=1{event_param}"
    );

    debug!("Sending tracker announce: {request_url}");

    let client = reqwest::Client::builder()
        .user_agent("TorTor/1.0.0")
        .build()
        .context("failed to build HTTP client")?;

    let response_bytes = client
        .get(&request_url)
        .send()
        .await
        .context("failed to connect to tracker")?
        .error_for_status()
        .context("tracker returned non-success HTTP status")?
        .bytes()
        .await
        .context("failed to read tracker response body")?;

    let response: TrackerResponse =
        serde_bencode::from_bytes(&response_bytes).context("failed to decode tracker bencode")?;

    if let Some(reason) = response.failure_reason {
        bail!("tracker failure reason: {reason}");
    }

    if let Some(warn) = response.warning_message {
        debug!("Tracker warning: {warn}");
    }

    let peers_blob = response
        .peers
        .as_deref()
        .context("tracker response does not contain compact peers list")?;

    let peers = decode_compact_peers(peers_blob)?;

    if let Some(interval) = response.interval {
        info!(
            "Tracker announce succeeded: {} peers, interval={}s",
            peers.len(),
            interval
        );
    } else {
        info!("Tracker announce succeeded: {} peers", peers.len());
    }

    Ok(peers)
}

async fn announce_udp(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    port: u16,
    left: u64,
    event: Option<&str>,
) -> Result<Vec<PeerInfo>> {
    debug!("Sending UDP tracker announce: {}", tracker_url);

    let url = Url::parse(tracker_url).context("invalid udp tracker url")?;
    let host = url.host_str().context("missing host")?;
    let tracker_port = url.port().unwrap_or(80);
    
    let addr_str = format!("{}:{}", host, tracker_port);
    let socket = UdpSocket::bind("0.0.0.0:0").await.context("failed to bind udp socket")?;
    socket.connect(&addr_str).await.context("failed to connect udp socket")?;
    
    let transaction_id: u32 = rand::random();
    
    // Connect request
    let mut connect_req = Vec::with_capacity(16);
    connect_req.extend_from_slice(&0x41727101980u64.to_be_bytes());
    connect_req.extend_from_slice(&0u32.to_be_bytes()); // action = 0
    connect_req.extend_from_slice(&transaction_id.to_be_bytes());
    
    socket.send(&connect_req).await?;
    
    let mut buf = [0u8; 65536];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
        .await
        .context("udp tracker connect timeout")?
        .context("failed to receive connect response")?;
        
    if len < 16 {
        bail!("invalid connect response length");
    }
    
    let action = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    if action == 3 {
        let err_msg = String::from_utf8_lossy(&buf[8..len]);
        bail!("tracker error: {}", err_msg);
    }
    if action != 0 {
        bail!("unexpected action in connect response: {}", action);
    }
    
    let resp_tx_id = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    if resp_tx_id != transaction_id {
        bail!("transaction id mismatch");
    }
    
    let connection_id = u64::from_be_bytes(buf[8..16].try_into().unwrap());
    
    // Announce request
    let announce_tx_id: u32 = rand::random();
    let mut announce_req = Vec::with_capacity(98);
    announce_req.extend_from_slice(&connection_id.to_be_bytes());
    announce_req.extend_from_slice(&1u32.to_be_bytes()); // action = 1
    announce_req.extend_from_slice(&announce_tx_id.to_be_bytes());
    announce_req.extend_from_slice(info_hash);
    announce_req.extend_from_slice(peer_id);
    announce_req.extend_from_slice(&0u64.to_be_bytes()); // downloaded
    announce_req.extend_from_slice(&left.to_be_bytes()); // left
    announce_req.extend_from_slice(&0u64.to_be_bytes()); // uploaded
    
    let event_num = match event {
        Some("completed") => 1u32,
        Some("started") => 2u32,
        Some("stopped") => 3u32,
        _ => 0u32,
    };
    announce_req.extend_from_slice(&event_num.to_be_bytes()); // event
    
    announce_req.extend_from_slice(&0u32.to_be_bytes()); // IP
    announce_req.extend_from_slice(&rand::random::<u32>().to_be_bytes()); // key
    announce_req.extend_from_slice(&(-1i32).to_be_bytes()); // num_want
    announce_req.extend_from_slice(&port.to_be_bytes());
    
    socket.send(&announce_req).await?;
    
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buf))
        .await
        .context("udp tracker announce timeout")?
        .context("failed to receive announce response")?;
        
    if len < 20 {
        bail!("invalid announce response length");
    }
    
    let action = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    if action == 3 {
        let err_msg = String::from_utf8_lossy(&buf[8..len]);
        bail!("tracker error: {}", err_msg);
    }
    if action != 1 {
        bail!("unexpected action in announce response: {}", action);
    }
    
    let resp_tx_id = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    if resp_tx_id != announce_tx_id {
        bail!("transaction id mismatch in announce");
    }
    
    let interval = u32::from_be_bytes(buf[8..12].try_into().unwrap());
    let _leechers = u32::from_be_bytes(buf[12..16].try_into().unwrap());
    let _seeders = u32::from_be_bytes(buf[16..20].try_into().unwrap());
    
    let peers_data = &buf[20..len];
    let peers = decode_compact_peers(peers_data)?;
    
    info!("UDP Tracker announce succeeded: {} peers, interval={}s", peers.len(), interval);
    
    Ok(peers)
}

fn decode_compact_peers(peers_data: &[u8]) -> Result<Vec<PeerInfo>> {
    if peers_data.len() % 6 != 0 {
        bail!(
            "invalid compact peers payload length: {} (must be multiple of 6)",
            peers_data.len()
        );
    }

    let mut peers = Vec::with_capacity(peers_data.len() / 6);
    for chunk in peers_data.chunks_exact(6) {
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        peers.push(PeerInfo {
            addr: SocketAddr::V4(SocketAddrV4::new(ip, port)),
        });
    }

    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_compact_peers_ok() {
        let raw = [127, 0, 0, 1, 0x1A, 0xE1]; // 6881
        let peers = decode_compact_peers(&raw).expect("must decode compact peers");
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].addr.to_string(), "127.0.0.1:6881");
    }

    #[test]
    fn decode_compact_peers_invalid_length() {
        let err = decode_compact_peers(&[1, 2, 3]).expect_err("must fail");
        assert!(
            err.to_string().contains("multiple of 6"),
            "unexpected error: {err}"
        );
    }
}
