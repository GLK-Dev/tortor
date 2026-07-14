use anyhow::{anyhow, bail, Result};

#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl Handshake {
    pub const PROTOCOL_STRING: [u8; 19] = *b"BitTorrent protocol";
    pub const HANDSHAKE_LEN: usize = 68;

    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        let mut reserved = [0u8; 8];
        reserved[5] |= 0x10; // BEP 10 Extension Protocol
        Self { reserved, info_hash, peer_id }
    }

    pub fn supports_extension_protocol(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn as_bytes(&self) -> [u8; Self::HANDSHAKE_LEN] {
        let mut buffer = [0u8; Self::HANDSHAKE_LEN];
        buffer[0] = Self::PROTOCOL_STRING.len() as u8;
        buffer[1..20].copy_from_slice(&Self::PROTOCOL_STRING);
        buffer[20..28].copy_from_slice(&self.reserved);
        buffer[28..48].copy_from_slice(&self.info_hash);
        buffer[48..68].copy_from_slice(&self.peer_id);
        buffer
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::HANDSHAKE_LEN {
            bail!(
                "invalid handshake length: expected {}, got {}",
                Self::HANDSHAKE_LEN,
                bytes.len()
            );
        }

        if bytes[0] as usize != Self::PROTOCOL_STRING.len() {
            bail!("invalid protocol string length byte in handshake");
        }

        if bytes[1..20] != Self::PROTOCOL_STRING {
            return Err(anyhow!("invalid protocol string in handshake"));
        }

        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&bytes[20..28]);

        let mut info_hash = [0u8; 20];
        info_hash.copy_from_slice(&bytes[28..48]);

        let mut peer_id = [0u8; 20];
        peer_id.copy_from_slice(&bytes[48..68]);

        Ok(Self { reserved, info_hash, peer_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_then_parse_roundtrip() {
        let hs = Handshake::new([1u8; 20], [2u8; 20]);
        let raw = hs.as_bytes();
        let parsed = Handshake::from_bytes(&raw).expect("must parse valid handshake");
        assert_eq!(parsed.info_hash, [1u8; 20]);
        assert_eq!(parsed.peer_id, [2u8; 20]);
        assert!(parsed.supports_extension_protocol());
    }
}
