//! End-to-end transfer tests: real coordinators and peer sessions talking over
//! loopback TCP, with in-memory disks.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};

use tortor::core::command::{CoreMessage, SessionEvent};
use tortor::core::coordinator::{run_coordinator, CoordinatorMsg, CoordinatorState};
use tortor::core::disk_io::AsyncDiskIO;
use tortor::core::manager::TorrentManager;
use tortor::crypto::core::hash_sha1;
use tortor::net::session::{
    run_peer_session, PeerContext, TransferStats, UploadSlots, MAX_UPLOAD_SLOTS,
};
use tortor::net::shaper::ShapedStream;
use tortor::net::transport::PeerStream;

const PIECE_LENGTH: u32 = 32 * 1024;
const PIECES: u32 = 6;

fn test_data() -> Vec<u8> {
    let total = (PIECES as usize - 1) * PIECE_LENGTH as usize + 10_000;
    (0..total).map(|i| (i * 31 % 251) as u8).collect()
}

fn piece_hashes(data: &[u8]) -> Vec<[u8; 20]> {
    data.chunks(PIECE_LENGTH as usize).map(hash_sha1).collect()
}

struct MemDisk {
    data: Arc<Mutex<Vec<u8>>>,
}

#[async_trait(?Send)]
impl AsyncDiskIO for MemDisk {
    async fn write_piece(&mut self, piece_index: u32, data: Vec<u8>) -> anyhow::Result<()> {
        let offset = piece_index as usize * PIECE_LENGTH as usize;
        self.data.lock().unwrap()[offset..offset + data.len()].copy_from_slice(&data);
        Ok(())
    }

    async fn read_piece(
        &mut self,
        piece_index: u32,
        offset: u32,
        len: u32,
    ) -> anyhow::Result<Vec<u8>> {
        let start = piece_index as usize * PIECE_LENGTH as usize + offset as usize;
        Ok(self.data.lock().unwrap()[start..start + len as usize].to_vec())
    }
}

struct Node {
    ctx: PeerContext,
    announce_tx: broadcast::Sender<SessionEvent>,
    ui_rx: mpsc::Receiver<CoreMessage>,
    disk: Arc<Mutex<Vec<u8>>>,
    thread: std::thread::JoinHandle<()>,
    resume_path: std::path::PathBuf,
}

/// Starts a coordinator that already owns the pieces in `have`.
fn spawn_node(name: &str, data: &[u8], have: &[u32], shutdown_tx: &broadcast::Sender<()>) -> Node {
    let hashes = Arc::new(piece_hashes(data));
    let mut initial = vec![0u8; data.len()];
    for &piece in have {
        let start = piece as usize * PIECE_LENGTH as usize;
        let end = (start + PIECE_LENGTH as usize).min(data.len());
        initial[start..end].copy_from_slice(&data[start..end]);
    }
    let disk = Arc::new(Mutex::new(initial));

    let (coord_tx, coord_rx) = mpsc::channel::<CoordinatorMsg>(1024);
    let (ui_tx, ui_rx) = mpsc::channel::<CoreMessage>(4096);
    let (announce_tx, _) = broadcast::channel::<SessionEvent>(256);

    let resume_path = std::env::temp_dir().join(format!(
        "tortor-test-{}-{name}.fastresume",
        std::process::id()
    ));

    let thread = {
        let ui_tx = ui_tx.clone();
        let announce_tx = announce_tx.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        let resume_path = resume_path.clone();
        let disk = disk.clone();
        let have = have.to_vec();
        std::thread::spawn(move || {
            // The disk trait object is not `Send`, so the state is built on this thread.
            let state = CoordinatorState::DownloadingData {
                manager: TorrentManager::from_completed(PIECES, &have),
                disk_writer: Box::new(MemDisk { data: disk }),
                paused: false,
                has_completed: false,
            };
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(run_coordinator(
                    coord_rx,
                    ui_tx,
                    state,
                    resume_path,
                    shutdown_rx,
                    announce_tx,
                ));
        })
    };

    let ctx = PeerContext {
        expected_hashes: hashes,
        piece_length: PIECE_LENGTH,
        total_length: Some(data.len() as u64),
        ui_sender: ui_tx,
        coord_sender: coord_tx,
        swarm_event_tx: None,
        upload_slots: UploadSlots::new(MAX_UPLOAD_SLOTS),
        stats: Arc::new(TransferStats::default()),
    };

    Node {
        ctx,
        announce_tx,
        ui_rx,
        disk,
        thread,
        resume_path,
    }
}

/// Connects two nodes with a loopback TCP connection and runs a session on each end.
async fn connect(a: &Node, b: &Node, shutdown_tx: &broadcast::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let (client, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
    let client = client.unwrap();
    let (server, server_addr) = accepted.unwrap();

    for (stream, ctx, announce_tx, peer_addr) in [
        (client, a.ctx.clone(), a.announce_tx.clone(), addr),
        (server, b.ctx.clone(), b.announce_tx.clone(), server_addr),
    ] {
        let shutdown_rx = shutdown_tx.subscribe();
        let announce_rx = announce_tx.subscribe();
        tokio::spawn(async move {
            let mut stream = PeerStream::Tcp(stream);
            let mut shaped = ShapedStream::new(&mut stream);
            let _ = run_peer_session(&mut shaped, &ctx, peer_addr, shutdown_rx, announce_rx, true)
                .await;
        });
    }
}

async fn wait_for_complete(node: &mut Node) {
    let wait = async {
        while let Some(msg) = node.ui_rx.recv().await {
            if matches!(msg, CoreMessage::DownloadComplete) {
                return;
            }
        }
        panic!("coordinator stopped before the download completed");
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("download did not finish in time");
}

fn finish(nodes: Vec<Node>, shutdown_tx: broadcast::Sender<()>) {
    let _ = shutdown_tx.send(());
    for node in nodes {
        drop(node.ctx);
        node.thread.join().unwrap();
        let _ = std::fs::remove_file(&node.resume_path);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leecher_downloads_everything_from_a_seeder() {
    let data = test_data();
    let (shutdown_tx, _) = broadcast::channel::<()>(4);

    let seeder = spawn_node(
        "seed-full",
        &data,
        &(0..PIECES).collect::<Vec<_>>(),
        &shutdown_tx,
    );
    let mut leecher = spawn_node("leech-full", &data, &[], &shutdown_tx);
    connect(&leecher, &seeder, &shutdown_tx).await;

    wait_for_complete(&mut leecher).await;
    assert_eq!(*leecher.disk.lock().unwrap(), data);

    finish(vec![seeder, leecher], shutdown_tx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leecher_combines_pieces_from_two_partial_seeders() {
    let data = test_data();
    let (shutdown_tx, _) = broadcast::channel::<()>(4);

    let even: Vec<u32> = (0..PIECES).filter(|p| p % 2 == 0).collect();
    let odd: Vec<u32> = (0..PIECES).filter(|p| p % 2 == 1).collect();
    let seeder_even = spawn_node("seed-even", &data, &even, &shutdown_tx);
    let seeder_odd = spawn_node("seed-odd", &data, &odd, &shutdown_tx);
    let mut leecher = spawn_node("leech-two", &data, &[], &shutdown_tx);
    connect(&leecher, &seeder_even, &shutdown_tx).await;
    connect(&leecher, &seeder_odd, &shutdown_tx).await;

    wait_for_complete(&mut leecher).await;
    assert_eq!(*leecher.disk.lock().unwrap(), data);

    finish(vec![seeder_even, seeder_odd, leecher], shutdown_tx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn corrupted_pieces_are_rejected_and_never_written() {
    let data = test_data();
    let mut corrupted = data.clone();
    corrupted[PIECE_LENGTH as usize + 5] ^= 0xFF; // piece 1

    let (shutdown_tx, _) = broadcast::channel::<()>(4);
    let evil_node = spawn_node(
        "evil",
        &corrupted,
        &(0..PIECES).collect::<Vec<_>>(),
        &shutdown_tx,
    );
    let mut leecher = spawn_node("leech-evil", &data, &[], &shutdown_tx);

    // The seeder advertises the genuine hashes but serves corrupted bytes.
    let mut evil_ctx = evil_node.ctx.clone();
    evil_ctx.expected_hashes = leecher.ctx.expected_hashes.clone();
    let evil = Node {
        ctx: evil_ctx,
        ..evil_node
    };
    connect(&leecher, &evil, &shutdown_tx).await;

    // Piece 1 can never verify, so the download must not complete.
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(msg) = leecher.ui_rx.recv().await {
            if matches!(msg, CoreMessage::DownloadComplete) {
                return true;
            }
        }
        false
    })
    .await;
    assert!(
        !matches!(result, Ok(true)),
        "corrupted data must not complete"
    );

    let disk = leecher.disk.lock().unwrap().clone();
    let start = PIECE_LENGTH as usize;
    assert!(disk[start..start + PIECE_LENGTH as usize]
        .iter()
        .all(|&b| b == 0));

    finish(vec![evil, leecher], shutdown_tx);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_listener_handshakes_and_seeds() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tortor::net::handshake::Handshake;

    let data = test_data();
    let (shutdown_tx, _) = broadcast::channel::<()>(4);
    let seeder = spawn_node(
        "seed-inbound",
        &data,
        &(0..PIECES).collect::<Vec<_>>(),
        &shutdown_tx,
    );
    let mut leecher = spawn_node("leech-inbound", &data, &[], &shutdown_tx);

    let info_hash = [7u8; 20];
    let seeder_id = [1u8; 20];
    let leecher_id = [2u8; 20];

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    {
        let ctx = seeder.ctx.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        let announce_rx = seeder.announce_tx.subscribe();
        tokio::spawn(async move {
            let (socket, remote) = listener.accept().await.unwrap();
            let _ = tortor::net::inbound::serve_tcp(
                socket,
                remote,
                info_hash,
                seeder_id,
                ctx,
                shutdown_rx,
                announce_rx,
            )
            .await;
        });
    }

    let mut socket = TcpStream::connect(addr).await.unwrap();
    socket
        .write_all(&Handshake::new(info_hash, leecher_id).as_bytes())
        .await
        .unwrap();
    let mut reply = [0u8; Handshake::HANDSHAKE_LEN];
    socket.read_exact(&mut reply).await.unwrap();
    let remote = Handshake::from_bytes(&reply).unwrap();
    assert_eq!(remote.info_hash, info_hash);
    assert_eq!(remote.peer_id, seeder_id);

    let ctx = leecher.ctx.clone();
    let shutdown_rx = shutdown_tx.subscribe();
    let announce_rx = leecher.announce_tx.subscribe();
    tokio::spawn(async move {
        let mut stream = PeerStream::Tcp(socket);
        let mut shaped = ShapedStream::new(&mut stream);
        let _ = run_peer_session(&mut shaped, &ctx, addr, shutdown_rx, announce_rx, true).await;
    });

    wait_for_complete(&mut leecher).await;
    assert_eq!(*leecher.disk.lock().unwrap(), data);
    finish(vec![seeder, leecher], shutdown_tx);
}

#[tokio::test]
async fn inbound_listener_rejects_wrong_info_hash() {
    use tokio::io::AsyncWriteExt;
    use tortor::net::handshake::Handshake;

    let data = test_data();
    let (shutdown_tx, _) = broadcast::channel::<()>(4);
    let seeder = spawn_node("seed-reject", &data, &[0], &shutdown_tx);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ctx = seeder.ctx.clone();
    let shutdown_rx = shutdown_tx.subscribe();
    let announce_rx = seeder.announce_tx.subscribe();
    let server = tokio::spawn(async move {
        let (socket, remote) = listener.accept().await.unwrap();
        tortor::net::inbound::serve_tcp(
            socket,
            remote,
            [7u8; 20],
            [1u8; 20],
            ctx,
            shutdown_rx,
            announce_rx,
        )
        .await
    });

    let mut socket = TcpStream::connect(addr).await.unwrap();
    socket
        .write_all(&Handshake::new([9u8; 20], [2u8; 20]).as_bytes())
        .await
        .unwrap();
    assert!(server.await.unwrap().is_err());

    finish(vec![seeder], shutdown_tx);
}
