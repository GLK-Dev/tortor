use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// BEP 11 peer exchange message. Fields are declared in key order because
/// bencode dictionaries must be sorted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PexMessage {
    #[serde(with = "serde_bytes", default)]
    pub added: Vec<u8>,
    #[serde(
        rename = "added.f",
        with = "serde_bytes",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub added_f: Vec<u8>,
    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub added6: Vec<u8>,
    #[serde(
        rename = "added6.f",
        with = "serde_bytes",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub added6_f: Vec<u8>,
    #[serde(with = "serde_bytes", default)]
    pub dropped: Vec<u8>,
    #[serde(with = "serde_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub dropped6: Vec<u8>,
}

impl PexMessage {
    /// Builds a message from address lists, splitting them by IP family.
    pub fn from_peers(added: &[SocketAddr], dropped: &[SocketAddr]) -> Self {
        let (added4, added6) = split_compact(added);
        let (dropped4, dropped6) = split_compact(dropped);
        Self {
            added_f: vec![0; added4.len() / 6],
            added6_f: vec![0; added6.len() / 18],
            added: added4,
            added6,
            dropped: dropped4,
            dropped6,
        }
    }

    pub fn decode_added_ipv4(&self) -> Vec<SocketAddr> {
        decode_compact_ipv4(&self.added)
    }

    /// Every added peer, IPv4 and IPv6.
    pub fn decode_added(&self) -> Vec<SocketAddr> {
        let mut peers = decode_compact_ipv4(&self.added);
        peers.extend(decode_compact_ipv6(&self.added6));
        peers
    }
}

pub fn decode_compact_ipv4(bytes: &[u8]) -> Vec<SocketAddr> {
    bytes
        .chunks_exact(6)
        .map(|chunk| {
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]),
                u16::from_be_bytes([chunk[4], chunk[5]]),
            ))
        })
        .collect()
}

/// One DHT `values` entry: 6 bytes for an IPv4 peer, 18 for an IPv6 peer.
/// Several concatenated IPv4 peers are accepted as well.
pub fn decode_compact_peers(bytes: &[u8]) -> Vec<SocketAddr> {
    if bytes.len() == 18 {
        decode_compact_ipv6(bytes)
    } else {
        decode_compact_ipv4(bytes)
    }
}

pub fn decode_compact_ipv6(bytes: &[u8]) -> Vec<SocketAddr> {
    bytes
        .chunks_exact(18)
        .map(|chunk| {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[..16]);
            SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(octets),
                u16::from_be_bytes([chunk[16], chunk[17]]),
                0,
                0,
            ))
        })
        .collect()
}

pub fn encode_compact_ipv4(peers: &[SocketAddr]) -> Vec<u8> {
    split_compact(peers).0
}

/// Compact IPv4 (6 bytes each) and IPv6 (18 bytes each) encodings of `peers`.
pub fn split_compact(peers: &[SocketAddr]) -> (Vec<u8>, Vec<u8>) {
    let (mut v4, mut v6) = (Vec::new(), Vec::new());
    for peer in peers {
        // An IPv4-mapped IPv6 address (dual-stack sockets) is really an IPv4 peer.
        let ip = match peer.ip() {
            IpAddr::V6(addr) => addr
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(addr)),
            other => other,
        };
        match ip {
            IpAddr::V4(ip) => {
                v4.extend_from_slice(&ip.octets());
                v4.extend_from_slice(&peer.port().to_be_bytes());
            }
            IpAddr::V6(ip) => {
                v6.extend_from_slice(&ip.octets());
                v6.extend_from_slice(&peer.port().to_be_bytes());
            }
        }
    }
    (v4, v6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_mixed_families() {
        let peers: Vec<SocketAddr> = vec![
            "10.0.0.1:6881".parse().unwrap(),
            "[2001:db8::1]:51413".parse().unwrap(),
            "192.168.1.2:1".parse().unwrap(),
        ];
        let msg = PexMessage::from_peers(&peers, &[]);
        assert_eq!(msg.added.len(), 12);
        assert_eq!(msg.added6.len(), 18);

        let bytes = serde_bencode::to_bytes(&msg).unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(
            text.contains("7:added.f") && text.contains("6:added6"),
            "{text}"
        );
        assert!(text.find("5:added").unwrap() < text.find("7:dropped").unwrap());

        let parsed: PexMessage = serde_bencode::from_bytes(&bytes).unwrap();
        let mut decoded = parsed.decode_added();
        decoded.sort();
        let mut expected = peers.clone();
        expected.sort();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn mapped_ipv4_counts_as_ipv4() {
        let mapped: SocketAddr = "[::ffff:10.1.2.3]:6881".parse().unwrap();
        let (v4, v6) = split_compact(&[mapped]);
        assert_eq!(v4, vec![10, 1, 2, 3, 0x1A, 0xE1]);
        assert!(v6.is_empty());
    }

    #[test]
    fn old_messages_without_ipv6_fields_still_parse() {
        let parsed: PexMessage =
            serde_bencode::from_bytes(b"d5:added6:\x0a\x00\x00\x01\x1a\xe17:dropped0:e").unwrap();
        assert_eq!(parsed.decode_added().len(), 1);
    }
}
