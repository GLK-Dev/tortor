#![cfg_attr(all(not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]


use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn, Level};

use tortor::core::bencode;
use tortor::core::peer_id::generate_peer_id;
use tortor::net::listener;
use tortor::net::tracker;

use tortor::ui::dashboard;

/// TorTor - High-performance BitTorrent client
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Directory to download files to
    #[arg(short, long, default_value = ".")]
    output: PathBuf,

    /// Path to .torrent file
    #[arg(short, long)]
    torrent: Option<PathBuf>,

    /// Enable verbose logging (debug level)
    #[arg(short, long, default_value_t = false)]
    verbose: bool,

    /// Start incoming peer listener on the given port
    #[arg(long)]
    listen_port: Option<u16>,

    /// Query HTTP tracker and print returned peers
    #[arg(long, default_value_t = false)]
    announce_tracker: bool,

    /// Run without GUI
    #[arg(long, default_value_t = false)]
    cli: bool,

    /// Optional magnet link
    #[arg(index = 1)]
    magnet: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let run_gui = !args.cli;

    let log_level = if args.verbose { Level::DEBUG } else { Level::INFO };
    tracing_subscriber::fmt().with_max_level(log_level).init();

    if run_gui {
        let port = args.listen_port.unwrap_or(6881);
        return dashboard::run_dashboard(args.torrent.clone(), port, args.output);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to initialize tokio runtime")?;
    runtime.block_on(run_cli(args))
}

async fn run_cli(args: Args) -> Result<()> {
    let torrent_path = args
        .torrent
        .clone()
        .context("CLI mode requires --torrent <path-to-file.torrent>")?;

    info!("Starting TorTor CLI");
    info!("Reading torrent file: {:?}", torrent_path);

    let meta = bencode::parse_torrent_file(&torrent_path)
        .with_context(|| format!("Failed to parse torrent file: {:?}", torrent_path))?;

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

    let peer_id = generate_peer_id();
    let port = args.listen_port.unwrap_or(6881);

    if args.announce_tracker {
        if meta.announce.starts_with("http://") || meta.announce.starts_with("https://") {
            let left = meta
                .total_length
                .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));

            let peers = tracker::announce(&meta.announce, &meta.info_hash, &peer_id, port, left)
                .await
                .context("tracker announce failed")?;

            println!("Peers from tracker: {}", peers.len());
            for peer in peers.iter().take(20) {
                println!("  {}", peer.addr);
            }
            if peers.len() > 20 {
                println!("  ... and {} more", peers.len() - 20);
            }
        } else {
            warn!(
                "Skipping tracker announce: only HTTP/HTTPS trackers are supported right now ({})",
                meta.announce
            );
        }
    }

    if let Some(port) = args.listen_port {
        info!("Starting listener mode on port {port}");
        info!("Local peer id: {}", String::from_utf8_lossy(&peer_id));
        listener::start_listener(port, meta.info_hash, peer_id).await?;
    }

    Ok(())
}


