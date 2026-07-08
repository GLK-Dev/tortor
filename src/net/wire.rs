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
