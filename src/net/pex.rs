use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PexMessage {
    #[serde(with = "serde_bytes", default)]
    pub added: Vec<u8>,
    #[serde(with = "serde_bytes", default)]
    pub added_f: Vec<u8>,
    #[serde(with = "serde_bytes", default)]
    pub dropped: Vec<u8>,
}

impl PexMessage {
    pub fn decode_added_ipv4(&self) -> Vec<SocketAddr> {
        decode_compact_ipv4(&self.added)
    }
}

pub fn decode_compact_ipv4(bytes: &[u8]) -> Vec<SocketAddr> {
    let mut addrs = Vec::with_capacity(bytes.len() / 6);
    
    for chunk in bytes.chunks_exact(6) {
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        addrs.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
    }
    
    addrs
}

pub fn encode_compact_ipv4(peers: &[SocketAddr]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(peers.len() * 6);
    for peer in peers {
        if let SocketAddr::V4(addr) = peer {
            buf.extend_from_slice(&addr.ip().octets());
            buf.extend_from_slice(&addr.port().to_be_bytes());
        }
    }
    buf
}
