use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, Level};

use tortor::core::bencode;
use tortor::net::listener;

/// TorTor - High-performance BitTorrent client
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to .torrent file
    #[arg(short, long)]
    torrent: PathBuf,

    /// Enable verbose logging (debug level)
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Start incoming peer listener on the given port
    #[arg(long)]
    listen_port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = if args.verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt().with_max_level(log_level).init();

    info!("Starting TorTor CLI");
    info!("Reading torrent file: {:?}", args.torrent);

    let meta = bencode::parse_torrent_file(&args.torrent)
        .with_context(|| format!("Failed to parse torrent file: {:?}", args.torrent))?;

    info!("Torrent metadata loaded successfully");
    println!("Name         : {}", meta.name);
    println!("Announce     : {}", meta.announce);
    println!("Piece length : {}", meta.piece_length);
    println!("Pieces count : {}", meta.pieces_count);
    println!("Info hash    : {}", meta.info_hash_hex());
    match meta.total_length {
        Some(total) => println!("Total size   : {} bytes", total),
        None => println!("Total size   : multi-file mode (not yet summarized)"),
    }

    if let Some(port) = args.listen_port {
        let peer_id = generate_peer_id();
        info!("Starting listener mode on port {port}");
        info!("Local peer id: {}", String::from_utf8_lossy(&peer_id));
        listener::start_listener(port, meta.info_hash, peer_id).await?;
    }

    Ok(())
}

fn generate_peer_id() -> [u8; 20] {
    let mut peer_id = [b'0'; 20];
    let prefix = b"-TT0001-";
    peer_id[..prefix.len()].copy_from_slice(prefix);

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
        ^ u64::from(std::process::id());

    let tail = format!("{seed:012x}");
    peer_id[8..20].copy_from_slice(&tail.as_bytes()[..12]);
    peer_id
}

#[cfg(test)]
mod tests {
    use super::generate_peer_id;

    #[test]
    fn peer_id_has_expected_shape() {
        let id = generate_peer_id();
        assert_eq!(id.len(), 20);
        assert_eq!(&id[..8], b"-TT0001-");
    }
}
