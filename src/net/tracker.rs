use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use tracing::{debug, info};
use url::form_urlencoded::byte_serialize;

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
) -> Result<Vec<PeerInfo>> {
    let info_hash_encoded: String = byte_serialize(info_hash).collect();
    let peer_id_encoded: String = byte_serialize(peer_id).collect();

    let separator = if tracker_url.contains('?') { '&' } else { '?' };
    let request_url = format!(
        "{tracker_url}{separator}info_hash={info_hash_encoded}&peer_id={peer_id_encoded}&port={port}&uploaded=0&downloaded=0&left={left}&compact=1&event=started"
    );

    debug!("Sending tracker announce: {request_url}");

    let client = reqwest::Client::builder()
        .user_agent("TorTor/0.1.0")
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
