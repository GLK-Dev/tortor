use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, Level};

use tortor::core::bencode;

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
}

fn main() -> Result<()> {
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
    match meta.total_length {
        Some(total) => println!("Total size   : {} bytes", total),
        None => println!("Total size   : multi-file mode (not yet summarized)"),
    }

    Ok(())
}
