use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub enum PeerMessage {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have(u32),
    Bitfield(Vec<u8>),
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, block: Vec<u8> },
}

impl PeerMessage {
    pub async fn read_from(stream: &mut TcpStream) -> Result<Self> {
        let len = stream
            .read_u32()
            .await
            .context("failed to read message length")?;

        if len == 0 {
            return Ok(PeerMessage::KeepAlive);
        }

        let id = stream
            .read_u8()
            .await
            .context("failed to read message id")?;
        let payload_len = (len - 1) as usize;

        match id {
            0 => {
                drain_payload(stream, payload_len).await?;
                Ok(PeerMessage::Choke)
            }
            1 => {
                drain_payload(stream, payload_len).await?;
                Ok(PeerMessage::Unchoke)
            }
            2 => {
                drain_payload(stream, payload_len).await?;
                Ok(PeerMessage::Interested)
            }
            3 => {
                drain_payload(stream, payload_len).await?;
                Ok(PeerMessage::NotInterested)
            }
            4 => {
                if payload_len != 4 {
                    bail!("invalid HAVE payload length: {payload_len}");
                }
                let piece_index = stream
                    .read_u32()
                    .await
                    .context("failed to read HAVE payload")?;
                Ok(PeerMessage::Have(piece_index))
            }
            5 => {
                let mut bitfield = vec![0u8; payload_len];
                stream
                    .read_exact(&mut bitfield)
                    .await
                    .context("failed to read BITFIELD payload")?;
                Ok(PeerMessage::Bitfield(bitfield))
            }
            6 => {
                if payload_len != 12 {
                    bail!("invalid REQUEST payload length: {payload_len}");
                }
                let index = stream
                    .read_u32()
                    .await
                    .context("failed to read REQUEST index")?;
                let begin = stream
                    .read_u32()
                    .await
                    .context("failed to read REQUEST begin")?;
                let length = stream
                    .read_u32()
                    .await
                    .context("failed to read REQUEST length")?;
                Ok(PeerMessage::Request {
                    index,
                    begin,
                    length,
                })
            }
            7 => {
                if payload_len < 8 {
                    bail!("invalid PIECE payload length: {payload_len}");
                }
                let index = stream
                    .read_u32()
                    .await
                    .context("failed to read PIECE index")?;
                let begin = stream
                    .read_u32()
                    .await
                    .context("failed to read PIECE begin")?;
                let block_len = payload_len - 8;
                let mut block = vec![0u8; block_len];
                stream
                    .read_exact(&mut block)
                    .await
                    .context("failed to read PIECE block")?;
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    block,
                })
            }
            _ => {
                drain_payload(stream, payload_len).await?;
                bail!("unknown peer message id: {id}")
            }
        }
    }

    pub async fn send_interested(stream: &mut TcpStream) -> Result<()> {
        let msg = [0u8, 0, 0, 1, 2];
        stream
            .write_all(&msg)
            .await
            .context("failed to send Interested message")?;
        Ok(())
    }

    pub async fn send_request(
        stream: &mut TcpStream,
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

    pub async fn send_have(stream: &mut TcpStream, piece_index: u32) -> Result<()> {
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

    pub async fn send_bitfield(stream: &mut TcpStream, bitfield: &[u8]) -> Result<()> {
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
        stream: &mut TcpStream,
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
}

async fn drain_payload(stream: &mut TcpStream, payload_len: usize) -> Result<()> {
    if payload_len == 0 {
        return Ok(());
    }

    let mut dump = vec![0u8; payload_len];
    stream
        .read_exact(&mut dump)
        .await
        .context("failed to drain payload")?;
    Ok(())
}
