//! Headless download of a magnet link for a limited time; used to check the
//! network stack against real swarms.
//!
//! Usage: cargo run --release --example download -- "<magnet>" <output-dir> [seconds]

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};

use tortor::core::bencode::parse_torrent_metadata_bytes;
use tortor::core::command::{CoreMessage, SessionEvent};
use tortor::core::coordinator::{run_coordinator, CoordinatorMsg, CoordinatorState};
use tortor::core::disk::StandardDisk;
use tortor::core::disk_io::AsyncDiskIO;
use tortor::core::manager::TorrentManager;
use tortor::core::peer_id::generate_peer_id;
use tortor::net::metadata::{fetch_metadata, FetchOptions};
use tortor::net::{magnet, swarm};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let mut args = std::env::args().skip(1);
    let uri = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing magnet"))?;
    let output = std::path::PathBuf::from(args.next().unwrap_or_else(|| "download-test".into()));
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let magnet = magnet::parse(&uri)?;
    let peer_id = generate_peer_id();
    let port = 6881;

    let bytes = fetch_metadata(
        FetchOptions {
            info_hash: magnet.info_hash,
            peer_id,
            listen_port: port,
            trackers: magnet.trackers.clone(),
            initial_peers: Vec::new(),
            use_dht: true,
            timeout: Duration::from_secs(120),
        },
        |status| println!("{status}"),
    )
    .await?;
    let meta = parse_torrent_metadata_bytes(&bytes, magnet.info_hash)?
        .with_extra_trackers(magnet.trackers.clone());
    let total = meta.total_length.unwrap_or(0);
    println!(
        "metadata: {:?}, {} pieces of {} bytes, {} MiB",
        meta.name,
        meta.pieces_count,
        meta.piece_length,
        total / (1024 * 1024)
    );

    let (coord_tx, coord_rx) = mpsc::channel::<CoordinatorMsg>(2048);
    let (ui_tx, mut ui_rx) = mpsc::channel::<CoreMessage>(1024);
    let (shutdown_tx, _) = broadcast::channel::<()>(16);
    let (announce_tx, _) = broadcast::channel::<SessionEvent>(64);

    let coordinator = {
        let (ui_tx, shutdown_rx, announce_tx) =
            (ui_tx.clone(), shutdown_tx.subscribe(), announce_tx.clone());
        let (output, meta) = (output.clone(), meta.clone());
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let disk = StandardDisk::init(
                        &output,
                        meta.total_length.unwrap_or(0),
                        meta.piece_length,
                        meta.files.as_ref(),
                        &meta.name,
                    )
                    .await
                    .expect("disk init");
                    let state = CoordinatorState::DownloadingData {
                        manager: TorrentManager::new(meta.pieces_count),
                        disk_writer: Box::new(disk) as Box<dyn AsyncDiskIO>,
                        paused: false,
                        has_completed: false,
                    };
                    run_coordinator(
                        coord_rx,
                        ui_tx,
                        state,
                        output.join("download.fastresume"),
                        shutdown_rx,
                        announce_tx,
                    )
                    .await;
                });
        })
    };

    let hashes = Arc::new(meta.pieces.clone());
    let swarm_task = tokio::spawn(swarm::run_swarm_manager(
        Default::default(),
        meta.trackers.clone(),
        meta.info_hash,
        peer_id,
        port,
        total,
        hashes,
        meta.piece_length,
        meta.total_length,
        ui_tx,
        coord_tx,
        shutdown_tx.clone(),
        announce_tx,
    ));

    let started = Instant::now();
    let (mut down, mut up) = (0usize, 0usize);
    let mut progress = 0.0f32;
    let mut last_report = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        if let Ok(Some(msg)) = tokio::time::timeout(Duration::from_millis(500), ui_rx.recv()).await
        {
            match msg {
                CoreMessage::BytesTransferred(rx, tx) => {
                    down += rx;
                    up += tx;
                }
                CoreMessage::GlobalProgress(p) => progress = p,
                CoreMessage::Error(e) => println!("error: {e}"),
                CoreMessage::DownloadComplete => {
                    println!("download complete");
                    break;
                }
                _ => {}
            }
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            last_report = Instant::now();
            println!(
                "[{:>3}s] progress {:.2}% | down {:.2} MiB | up {:.2} MiB",
                started.elapsed().as_secs(),
                progress * 100.0,
                down as f64 / 1_048_576.0,
                up as f64 / 1_048_576.0
            );
        }
    }

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(5), swarm_task).await;
    let _ = coordinator.join();
    println!(
        "done: progress {:.2}% | down {:.2} MiB | up {:.2} MiB",
        progress * 100.0,
        down as f64 / 1_048_576.0,
        up as f64 / 1_048_576.0
    );
    Ok(())
}
