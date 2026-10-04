use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_bytes::ByteBuf;
use tokio::net::UdpSocket;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};
use url::form_urlencoded::byte_serialize;
use url::Url;

const MAX_TRACKER_RESPONSE: usize = 2 * 1024 * 1024;
const UDP_ATTEMPTS: usize = 2;
const UDP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct PeerInfo {
    pub addr: SocketAddr,
}

/// Fields shared by every announce request.
#[derive(Debug, Clone, Copy)]
pub struct AnnounceParams<'a> {
    pub info_hash: &'a [u8; 20],
    pub peer_id: &'a [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct DictPeer {
    ip: String,
    port: u16,
}

/// `peers` is either a compact byte string (BEP 23) or a list of dictionaries.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PeersField {
    Compact(ByteBuf),
    Dicts(Vec<DictPeer>),
}

#[derive(Debug, Deserialize)]
struct TrackerResponse {
    peers: Option<PeersField>,
    peers6: Option<ByteBuf>,
    interval: Option<u64>,
    #[serde(rename = "failure reason")]
    failure_reason: Option<String>,
    #[serde(rename = "warning message")]
    warning_message: Option<String>,
}

pub fn is_supported_tracker(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("udp://")
}

/// Announces to every tracker concurrently and merges the peers. Fails only
/// when no tracker answered.
pub async fn announce_all(
    tracker_urls: &[String],
    params: &AnnounceParams<'_>,
    mut on_peers: impl FnMut(&[PeerInfo]),
) -> Result<Vec<PeerInfo>> {
    if tracker_urls.is_empty() {
        bail!("no trackers to announce to");
    }

    let mut tasks = JoinSet::new();
    for url in tracker_urls {
        let url = url.clone();
        let info_hash = *params.info_hash;
        let peer_id = *params.peer_id;
        let (port, uploaded, downloaded, left) =
            (params.port, params.uploaded, params.downloaded, params.left);
        let event = params.event.map(str::to_string);
        tasks.spawn(async move {
            let params = AnnounceParams {
                info_hash: &info_hash,
                peer_id: &peer_id,
                port,
                uploaded,
                downloaded,
                left,
                event: event.as_deref(),
            };
            (url.clone(), announce(&url, &params).await)
        });
    }

    let mut merged: Vec<PeerInfo> = Vec::new();
    let mut last_err = None;
    let mut any_ok = false;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_, Ok(peers))) => {
                any_ok = true;
                on_peers(&peers);
                for peer in peers {
                    if !merged.iter().any(|p| p.addr == peer.addr) {
                        merged.push(peer);
                    }
                }
            }
            Ok((url, Err(err))) => {
                warn!("tracker {url} failed: {err:#}");
                last_err = Some(err);
            }
            Err(err) => warn!("tracker task failed: {err}"),
        }
    }

    if any_ok {
        Ok(merged)
    } else {
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all trackers failed")))
    }
}

pub async fn announce(tracker_url: &str, params: &AnnounceParams<'_>) -> Result<Vec<PeerInfo>> {
    if tracker_url.starts_with("udp://") {
        return announce_udp(tracker_url, params).await;
    }

    let info_hash_encoded: String = byte_serialize(params.info_hash).collect();
    let peer_id_encoded: String = byte_serialize(params.peer_id).collect();

    let separator = if tracker_url.contains('?') { '&' } else { '?' };
    let event_param = match params.event {
        Some(e) => format!("&event={e}"),
        None => String::new(),
    };

    let request_url = format!(
        "{tracker_url}{separator}info_hash={info_hash_encoded}&peer_id={peer_id_encoded}&port={}&uploaded={}&downloaded={}&left={}&compact=1{event_param}",
        params.port, params.uploaded, params.downloaded, params.left
    );

    // The URL may carry a private tracker passkey, so only the tracker itself is logged.
    debug!("Sending tracker announce to {tracker_url}");

    let client = reqwest::Client::builder()
        .user_agent(concat!("TorTor/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build HTTP client")?;

    let mut response = client
        .get(&request_url)
        .send()
        .await
        .context("failed to connect to tracker")?
        .error_for_status()
        .context("tracker returned non-success HTTP status")?;

    let mut response_bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed to read tracker response body")?
    {
        if response_bytes.len() + chunk.len() > MAX_TRACKER_RESPONSE {
            bail!("tracker response exceeds {MAX_TRACKER_RESPONSE} bytes");
        }
        response_bytes.extend_from_slice(&chunk);
    }

    let (peers, interval) = parse_http_response(&response_bytes)?;
    match interval {
        Some(interval) => info!(
            "Tracker announce succeeded: {} peers, interval={interval}s",
            peers.len()
        ),
        None => info!("Tracker announce succeeded: {} peers", peers.len()),
    }
    Ok(peers)
}

fn parse_http_response(bytes: &[u8]) -> Result<(Vec<PeerInfo>, Option<u64>)> {
    crate::core::bencode::check_bencode_depth(bytes)?;
    let response: TrackerResponse =
        serde_bencode::from_bytes(bytes).context("failed to decode tracker bencode")?;

    if let Some(reason) = response.failure_reason {
        bail!("tracker failure reason: {reason}");
    }
    if let Some(warn) = response.warning_message {
        debug!("Tracker warning: {warn}");
    }

    let mut peers = match response.peers {
        Some(PeersField::Compact(blob)) => decode_compact_peers(&blob)?,
        Some(PeersField::Dicts(list)) => list
            .into_iter()
            .filter_map(|p| {
                p.ip.parse::<IpAddr>().ok().map(|ip| PeerInfo {
                    addr: SocketAddr::new(ip, p.port),
                })
            })
            .collect(),
        None => Vec::new(),
    };
    if let Some(blob) = response.peers6 {
        peers.extend(decode_compact_peers6(&blob)?);
    }

    if peers.is_empty() && response.interval.is_none() {
        bail!("tracker response contains neither peers nor an interval");
    }
    Ok((peers, response.interval))
}

async fn udp_exchange(socket: &UdpSocket, request: &[u8], buf: &mut [u8]) -> Result<usize> {
    for _ in 0..UDP_ATTEMPTS {
        socket.send(request).await?;
        if let Ok(received) = tokio::time::timeout(UDP_TIMEOUT, socket.recv(buf)).await {
            return received.context("failed to receive udp tracker response");
        }
    }
    bail!("udp tracker timed out")
}

async fn announce_udp(tracker_url: &str, params: &AnnounceParams<'_>) -> Result<Vec<PeerInfo>> {
    debug!("Sending UDP tracker announce: {tracker_url}");

    let url = Url::parse(tracker_url).context("invalid udp tracker url")?;
    let host = url.host_str().context("missing host")?;
    let tracker_port = url.port().context("udp tracker url has no port")?;

    let target = tokio::net::lookup_host((host, tracker_port))
        .await
        .context("failed to resolve udp tracker")?
        .next()
        .context("udp tracker resolved to no address")?;
    let bind_addr = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_addr)
        .await
        .context("failed to bind udp socket")?;
    socket
        .connect(target)
        .await
        .context("failed to connect udp socket")?;

    let transaction_id: u32 = rand::random();

    let mut connect_req = Vec::with_capacity(16);
    connect_req.extend_from_slice(&0x41727101980u64.to_be_bytes());
    connect_req.extend_from_slice(&0u32.to_be_bytes()); // action = connect
    connect_req.extend_from_slice(&transaction_id.to_be_bytes());

    let mut buf = vec![0u8; 8192];
    let len = udp_exchange(&socket, &connect_req, &mut buf).await?;
    if len < 16 {
        bail!("invalid connect response length");
    }

    let action = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    if action == 3 {
        bail!("tracker error: {}", String::from_utf8_lossy(&buf[8..len]));
    }
    if action != 0 {
        bail!("unexpected action in connect response: {action}");
    }
    if u32::from_be_bytes(buf[4..8].try_into().unwrap()) != transaction_id {
        bail!("transaction id mismatch");
    }
    let connection_id = u64::from_be_bytes(buf[8..16].try_into().unwrap());

    let announce_tx_id: u32 = rand::random();
    let mut announce_req = Vec::with_capacity(98);
    announce_req.extend_from_slice(&connection_id.to_be_bytes());
    announce_req.extend_from_slice(&1u32.to_be_bytes()); // action = announce
    announce_req.extend_from_slice(&announce_tx_id.to_be_bytes());
    announce_req.extend_from_slice(params.info_hash);
    announce_req.extend_from_slice(params.peer_id);
    announce_req.extend_from_slice(&params.downloaded.to_be_bytes());
    announce_req.extend_from_slice(&params.left.to_be_bytes());
    announce_req.extend_from_slice(&params.uploaded.to_be_bytes());

    let event_num = match params.event {
        Some("completed") => 1u32,
        Some("started") => 2u32,
        Some("stopped") => 3u32,
        _ => 0u32,
    };
    announce_req.extend_from_slice(&event_num.to_be_bytes());
    announce_req.extend_from_slice(&0u32.to_be_bytes()); // IP
    announce_req.extend_from_slice(&rand::random::<u32>().to_be_bytes()); // key
    announce_req.extend_from_slice(&(-1i32).to_be_bytes()); // num_want
    announce_req.extend_from_slice(&params.port.to_be_bytes());

    let len = udp_exchange(&socket, &announce_req, &mut buf).await?;
    if len < 20 {
        bail!("invalid announce response length");
    }

    let action = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    if action == 3 {
        bail!("tracker error: {}", String::from_utf8_lossy(&buf[8..len]));
    }
    if action != 1 {
        bail!("unexpected action in announce response: {action}");
    }
    if u32::from_be_bytes(buf[4..8].try_into().unwrap()) != announce_tx_id {
        bail!("transaction id mismatch in announce");
    }

    let interval = u32::from_be_bytes(buf[8..12].try_into().unwrap());
    let peers = if target.is_ipv4() {
        decode_compact_peers(&buf[20..len])?
    } else {
        decode_compact_peers6(&buf[20..len])?
    };

    info!(
        "UDP Tracker announce succeeded: {} peers, interval={interval}s",
        peers.len()
    );
    Ok(peers)
}

fn decode_compact_peers(peers_data: &[u8]) -> Result<Vec<PeerInfo>> {
    if !peers_data.len().is_multiple_of(6) {
        bail!(
            "invalid compact peers payload length: {} (must be multiple of 6)",
            peers_data.len()
        );
    }

    Ok(peers_data
        .chunks_exact(6)
        .map(|chunk| PeerInfo {
            addr: SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]),
                u16::from_be_bytes([chunk[4], chunk[5]]),
            )),
        })
        .collect())
}

fn decode_compact_peers6(peers_data: &[u8]) -> Result<Vec<PeerInfo>> {
    if !peers_data.len().is_multiple_of(18) {
        bail!(
            "invalid compact peers6 payload length: {} (must be multiple of 18)",
            peers_data.len()
        );
    }

    Ok(peers_data
        .chunks_exact(18)
        .map(|chunk| {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[..16]);
            PeerInfo {
                addr: SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(octets),
                    u16::from_be_bytes([chunk[16], chunk[17]]),
                    0,
                    0,
                )),
            }
        })
        .collect())
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

    #[test]
    fn decode_compact_peers6_ok() {
        let mut raw = [0u8; 18];
        raw[15] = 1; // ::1
        raw[16] = 0x1A;
        raw[17] = 0xE1;
        let peers = decode_compact_peers6(&raw).unwrap();
        assert_eq!(peers[0].addr.to_string(), "[::1]:6881");
        assert!(decode_compact_peers6(&[0u8; 17]).is_err());
    }

    #[test]
    fn parses_compact_response_with_peers6() {
        let mut body = b"d8:intervali900e5:peers6:".to_vec();
        body.extend_from_slice(&[10, 0, 0, 1, 0x1A, 0xE1]);
        body.extend_from_slice(b"6:peers618:");
        body.extend_from_slice(&[0u8; 15]);
        body.extend_from_slice(&[1, 0x1A, 0xE1]);
        body.push(b'e');
        let (peers, interval) = parse_http_response(&body).unwrap();
        assert_eq!(interval, Some(900));
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].addr.to_string(), "10.0.0.1:6881");
    }

    #[test]
    fn parses_dictionary_peers_response() {
        let body = b"d8:intervali60e5:peersld2:ip9:127.0.0.14:porti6881eed2:ip3:::14:porti1eeee";
        let (peers, _) = parse_http_response(body).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].addr.to_string(), "127.0.0.1:6881");
        assert_eq!(peers[1].addr.to_string(), "[::1]:1");
    }

    #[test]
    fn failure_reason_is_an_error() {
        assert!(parse_http_response(b"d14:failure reason6:no waye").is_err());
    }

    #[test]
    fn supported_schemes() {
        assert!(is_supported_tracker("udp://tracker.example:80/announce"));
        assert!(is_supported_tracker("https://t.example/announce"));
        assert!(!is_supported_tracker("wss://t.example"));
    }
}
