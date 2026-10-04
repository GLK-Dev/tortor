use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedHandshakeDict {
    #[serde(default)]
    pub m: std::collections::HashMap<String, u8>,
    pub metadata_size: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    Extended {
        id: u8,
        payload: Vec<u8>,
    },
}

/// Largest accepted message (length prefix excluded). Covers a bitfield for ~8M pieces.
pub const MAX_MESSAGE_LEN: usize = 1024 * 1024;
/// Largest block a peer may request from us in a single REQUEST.
pub const MAX_BLOCK_REQUEST: u32 = 32 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const COMPACT_THRESHOLD: usize = 64 * 1024;

/// Incremental, cancel-safe peer message decoder: bytes already read are kept
/// in the internal buffer, so dropping `next()` mid-message never loses data.
#[derive(Debug, Default, Clone)]
pub struct MessageDecoder {
    buf: Vec<u8>,
    pos: usize,
}

impl MessageDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Decodes the next complete message from the buffer. Messages with an
    /// unknown id are skipped; oversized messages are an error.
    pub fn decode(&mut self) -> Result<Option<PeerMessage>> {
        loop {
            let available = &self.buf[self.pos..];
            if available.len() < 4 {
                self.compact();
                return Ok(None);
            }

            let len = u32::from_be_bytes([available[0], available[1], available[2], available[3]])
                as usize;
            if len > MAX_MESSAGE_LEN {
                bail!("peer message too large: {len} bytes");
            }
            if available.len() < 4 + len {
                self.compact();
                return Ok(None);
            }

            let parsed = if len == 0 {
                Some(PeerMessage::KeepAlive)
            } else {
                PeerMessage::parse(available[4], &available[5..4 + len])?
            };
            self.pos += 4 + len;
            if let Some(msg) = parsed {
                return Ok(Some(msg));
            }
        }
    }

    pub async fn next<S>(&mut self, stream: &mut S) -> Result<PeerMessage>
    where
        S: AsyncRead + Unpin,
    {
        let mut chunk = [0u8; READ_CHUNK];
        loop {
            if let Some(msg) = self.decode()? {
                return Ok(msg);
            }
            let n = stream
                .read(&mut chunk)
                .await
                .context("failed to read from peer")?;
            if n == 0 {
                bail!("peer closed the connection");
            }
            self.feed(&chunk[..n]);
        }
    }

    fn compact(&mut self) {
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos >= COMPACT_THRESHOLD {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }
}

impl PeerMessage {
    fn parse(id: u8, payload: &[u8]) -> Result<Option<Self>> {
        let be32 = |offset: usize| {
            u32::from_be_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ])
        };

        let msg = match id {
            0 => PeerMessage::Choke,
            1 => PeerMessage::Unchoke,
            2 => PeerMessage::Interested,
            3 => PeerMessage::NotInterested,
            4 => {
                if payload.len() != 4 {
                    bail!("invalid HAVE payload length: {}", payload.len());
                }
                PeerMessage::Have(be32(0))
            }
            5 => PeerMessage::Bitfield(payload.to_vec()),
            6 => {
                if payload.len() != 12 {
                    bail!("invalid REQUEST payload length: {}", payload.len());
                }
                PeerMessage::Request {
                    index: be32(0),
                    begin: be32(4),
                    length: be32(8),
                }
            }
            7 => {
                if payload.len() < 8 {
                    bail!("invalid PIECE payload length: {}", payload.len());
                }
                PeerMessage::Piece {
                    index: be32(0),
                    begin: be32(4),
                    block: payload[8..].to_vec(),
                }
            }
            20 => {
                if payload.is_empty() {
                    bail!("invalid EXTENDED payload length: 0");
                }
                PeerMessage::Extended {
                    id: payload[0],
                    payload: payload[1..].to_vec(),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(msg))
    }

    pub async fn send_keepalive(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
    ) -> Result<()> {
        stream
            .write_all(&[0u8; 4])
            .await
            .context("failed to send KeepAlive")?;
        Ok(())
    }

    pub async fn send_not_interested(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
    ) -> Result<()> {
        stream
            .write_all(&[0u8, 0, 0, 1, 3])
            .await
            .context("failed to send NotInterested")?;
        Ok(())
    }

    pub async fn send_choke(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
    ) -> Result<()> {
        let msg = [0u8, 0, 0, 1, 0];
        stream
            .write_all(&msg)
            .await
            .context("failed to send Choke")?;
        Ok(())
    }

    pub async fn send_unchoke(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
    ) -> Result<()> {
        let msg = [0u8, 0, 0, 1, 1];
        stream
            .write_all(&msg)
            .await
            .context("failed to send Unchoke")?;
        Ok(())
    }

    pub async fn send_interested(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
    ) -> Result<()> {
        let msg = [0u8, 0, 0, 1, 2];
        stream
            .write_all(&msg)
            .await
            .context("failed to send Interested message")?;
        Ok(())
    }

    pub async fn send_request(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
        index: u32,
        begin: u32,
        length: u32,
    ) -> Result<()> {
        let mut msg = [0u8; 17];
        msg[0..4].copy_from_slice(&13u32.to_be_bytes());
        msg[4] = 6;
        msg[5..9].copy_from_slice(&index.to_be_bytes());
        msg[9..13].copy_from_slice(&begin.to_be_bytes());
        msg[13..17].copy_from_slice(&length.to_be_bytes());

        stream
            .write_all(&msg)
            .await
            .context("failed to send Request message")?;
        Ok(())
    }

    pub async fn send_have(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
        piece_index: u32,
    ) -> Result<()> {
        let mut msg = [0u8; 9];
        msg[0..4].copy_from_slice(&5u32.to_be_bytes());
        msg[4] = 4;
        msg[5..9].copy_from_slice(&piece_index.to_be_bytes());

        stream
            .write_all(&msg)
            .await
            .context("failed to send Have message")?;
        Ok(())
    }

    pub async fn send_bitfield(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
        bitfield: &[u8],
    ) -> Result<()> {
        let len = 1u32 + bitfield.len() as u32;
        stream
            .write_u32(len)
            .await
            .context("failed to send Bitfield length")?;
        stream
            .write_u8(5)
            .await
            .context("failed to send Bitfield id")?;
        stream
            .write_all(bitfield)
            .await
            .context("failed to send Bitfield payload")?;
        Ok(())
    }

    pub async fn send_piece(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
        index: u32,
        begin: u32,
        block: &[u8],
    ) -> Result<()> {
        let len = 9u32 + block.len() as u32;

        stream
            .write_u32(len)
            .await
            .context("failed to send Piece length")?;
        stream
            .write_u8(7)
            .await
            .context("failed to send Piece id")?;
        stream
            .write_u32(index)
            .await
            .context("failed to send Piece index")?;
        stream
            .write_u32(begin)
            .await
            .context("failed to send Piece begin")?;
        stream
            .write_all(block)
            .await
            .context("failed to send Piece block")?;

        Ok(())
    }

    pub async fn send_extended(
        stream: &mut crate::net::shaper::ShapedStream<&mut crate::net::transport::PeerStream>,
        extended_id: u8,
        payload: &[u8],
    ) -> Result<()> {
        let len = 2u32 + payload.len() as u32;
        stream.write_u32(len).await?;
        stream.write_u8(20).await?;
        stream.write_u8(extended_id).await?;
        stream.write_all(payload).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_message_split_across_feeds() {
        let mut dec = MessageDecoder::new();
        let raw = [0u8, 0, 0, 5, 4, 0, 0, 0, 7];
        dec.feed(&raw[..3]);
        assert!(dec.decode().unwrap().is_none());
        dec.feed(&raw[3..7]);
        assert!(dec.decode().unwrap().is_none());
        dec.feed(&raw[7..]);
        assert!(matches!(dec.decode().unwrap(), Some(PeerMessage::Have(7))));
        assert!(dec.decode().unwrap().is_none());
    }

    #[test]
    fn rejects_oversized_message() {
        let mut dec = MessageDecoder::new();
        dec.feed(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(dec.decode().is_err());
    }

    #[test]
    fn skips_unknown_message_ids() {
        let mut dec = MessageDecoder::new();
        dec.feed(&[0, 0, 0, 3, 9, 1, 2, 0, 0, 0, 1, 1]);
        assert!(matches!(dec.decode().unwrap(), Some(PeerMessage::Unchoke)));
    }

    #[test]
    fn decodes_request_and_piece() {
        let mut dec = MessageDecoder::new();
        let mut raw = vec![0, 0, 0, 13, 6];
        raw.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0x40, 0]);
        raw.extend_from_slice(&[0, 0, 0, 11, 7, 0, 0, 0, 1, 0, 0, 0, 2, 0xAA, 0xBB]);
        dec.feed(&raw);
        match dec.decode().unwrap() {
            Some(PeerMessage::Request {
                index: 1,
                begin: 2,
                length: 16384,
            }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match dec.decode().unwrap() {
            Some(PeerMessage::Piece {
                index: 1,
                begin: 2,
                block,
            }) => assert_eq!(block, vec![0xAA, 0xBB]),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_fixed_size_messages() {
        let mut dec = MessageDecoder::new();
        dec.feed(&[0, 0, 0, 2, 4, 0]);
        assert!(dec.decode().is_err());
    }

    #[tokio::test]
    async fn next_is_cancel_safe() {
        use tokio::io::AsyncWriteExt;
        let (mut a, mut b) = tokio::io::duplex(64);
        let mut dec = MessageDecoder::new();
        a.write_all(&[0, 0, 0, 5, 4, 0]).await.unwrap();
        let res =
            tokio::time::timeout(std::time::Duration::from_millis(50), dec.next(&mut b)).await;
        assert!(res.is_err());
        a.write_all(&[0, 0, 9]).await.unwrap();
        assert!(matches!(
            dec.next(&mut b).await.unwrap(),
            PeerMessage::Have(9)
        ));
    }
}
