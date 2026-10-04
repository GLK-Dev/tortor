//! Magnet metadata download against a scripted fake peer (BEP 9 / BEP 10).

use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use tortor::core::bencode::parse_torrent_metadata_bytes;
use tortor::crypto::core::hash_sha1;
use tortor::net::handshake::Handshake;
use tortor::net::metadata::{fetch_metadata, FetchOptions};
use tortor::net::wire::{MessageDecoder, PeerMessage};

#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    Honest,
    Corrupt,
    Reject,
}

#[derive(Deserialize)]
struct Request {
    msg_type: u8,
    piece: u32,
}

/// An info dictionary larger than one 16 KiB metadata piece.
fn info_dict() -> Vec<u8> {
    let pieces = 1000usize;
    let mut info = format!(
        "d6:lengthi{}e4:name5:m.bin12:piece lengthi16384e6:pieces{}:",
        pieces * 16384,
        pieces * 20
    )
    .into_bytes();
    info.extend((0..pieces * 20).map(|i| (i % 253) as u8));
    info.push(b'e');
    info
}

async fn write_extended(stream: &mut TcpStream, ext_id: u8, payload: &[u8]) {
    let mut msg = Vec::new();
    msg.extend_from_slice(&(2 + payload.len() as u32).to_be_bytes());
    msg.push(20);
    msg.push(ext_id);
    msg.extend_from_slice(payload);
    stream.write_all(&msg).await.unwrap();
}

async fn serve(mut stream: TcpStream, info: Vec<u8>, info_hash: [u8; 20], behavior: Behavior) {
    let mut incoming = [0u8; Handshake::HANDSHAKE_LEN];
    stream.read_exact(&mut incoming).await.unwrap();
    stream
        .write_all(&Handshake::new(info_hash, [9u8; 20]).as_bytes())
        .await
        .unwrap();

    let handshake = format!("d1:md11:ut_metadatai3ee13:metadata_sizei{}ee", info.len());
    write_extended(&mut stream, 0, handshake.as_bytes()).await;

    let mut decoder = MessageDecoder::new();
    let mut buf = [0u8; 4096];
    loop {
        let msg = loop {
            match decoder.decode() {
                Ok(Some(msg)) => break msg,
                Ok(None) => {}
                Err(_) => return,
            }
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => decoder.feed(&buf[..n]),
            }
        };

        let PeerMessage::Extended { id: 3, payload } = msg else {
            continue;
        };
        let request: Request = serde_bencode::from_bytes(&payload).unwrap();
        assert_eq!(request.msg_type, 0);

        if behavior == Behavior::Reject {
            let reject = format!("d8:msg_typei2e5:piecei{}ee", request.piece);
            write_extended(&mut stream, 1, reject.as_bytes()).await;
            continue;
        }

        let start = request.piece as usize * 16384;
        let end = (start + 16384).min(info.len());
        let mut data = info[start..end].to_vec();
        if behavior == Behavior::Corrupt {
            data[0] ^= 0xFF;
        }
        let mut reply = format!(
            "d8:msg_typei1e5:piecei{}e10:total_sizei{}ee",
            request.piece,
            info.len()
        )
        .into_bytes();
        reply.extend_from_slice(&data);
        write_extended(&mut stream, 1, &reply).await;
    }
}

async fn spawn_peer(info: Vec<u8>, info_hash: [u8; 20], behavior: Behavior) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream, info.clone(), info_hash, behavior));
        }
    });
    addr
}

fn options(info_hash: [u8; 20], peers: Vec<SocketAddr>, timeout: Duration) -> FetchOptions {
    FetchOptions {
        info_hash,
        peer_id: [5u8; 20],
        listen_port: 6881,
        trackers: Vec::new(),
        initial_peers: peers,
        use_dht: false,
        timeout,
    }
}

#[tokio::test]
async fn downloads_and_verifies_metadata_from_peers() {
    let info = info_dict();
    let info_hash = hash_sha1(&info);
    let honest = spawn_peer(info.clone(), info_hash, Behavior::Honest).await;
    let rejecting = spawn_peer(info.clone(), info_hash, Behavior::Reject).await;

    let fetched = fetch_metadata(
        options(info_hash, vec![rejecting, honest], Duration::from_secs(20)),
        |_| {},
    )
    .await
    .expect("metadata must be fetched");
    assert_eq!(fetched, info);

    let meta = parse_torrent_metadata_bytes(&fetched, info_hash).unwrap();
    assert_eq!(meta.name, "m.bin");
    assert_eq!(meta.pieces_count, 1000);
}

#[tokio::test]
async fn corrupted_metadata_is_never_accepted() {
    let info = info_dict();
    let info_hash = hash_sha1(&info);
    let evil = spawn_peer(info, info_hash, Behavior::Corrupt).await;

    let result = fetch_metadata(
        options(info_hash, vec![evil], Duration::from_secs(3)),
        |_| {},
    )
    .await;
    assert!(result.is_err(), "hash mismatch must prevent completion");
}
