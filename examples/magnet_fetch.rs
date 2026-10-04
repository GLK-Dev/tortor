//! Fetches the metadata of a magnet link and prints a summary.
//!
//! Usage: cargo run --example magnet_fetch -- "magnet:?xt=urn:btih:..."

use std::time::Duration;

use tortor::core::bencode::parse_torrent_metadata_bytes;
use tortor::core::peer_id::generate_peer_id;
use tortor::net::magnet;
use tortor::net::metadata::{fetch_metadata, FetchOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let uri = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: magnet_fetch <magnet-uri>"))?;
    let magnet = magnet::parse(&uri)?;
    println!(
        "info hash: {}, {} tracker(s)",
        hex::encode(magnet.info_hash),
        magnet.trackers.len()
    );

    let started = std::time::Instant::now();
    let bytes = fetch_metadata(
        FetchOptions {
            info_hash: magnet.info_hash,
            peer_id: generate_peer_id(),
            listen_port: 6881,
            trackers: magnet.trackers,
            initial_peers: Vec::new(),
            use_dht: true,
            timeout: Duration::from_secs(120),
        },
        |status| println!("{status}"),
    )
    .await?;

    let meta = parse_torrent_metadata_bytes(&bytes, magnet.info_hash)?;
    println!(
        "got {} bytes in {:.1}s: name={:?} pieces={} piece_length={} size={:?}",
        bytes.len(),
        started.elapsed().as_secs_f32(),
        meta.name,
        meta.pieces_count,
        meta.piece_length,
        meta.total_length
    );
    Ok(())
}
