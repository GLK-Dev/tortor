use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::command::CoreMessage;
use crate::core::disk_io::AsyncDiskIO;
use crate::core::manager::TorrentManager;
use crate::core::resume::{save_fastresume, FastResumeState};
use crate::core::metadata_assembler::MetadataAssembler;
use crate::core::bencode::parse_torrent_metadata_bytes;

pub enum CoordinatorMsg {
    RequestWork(oneshot::Sender<Option<u32>>),
    PieceDownloaded(u32, Vec<u8>),
    PieceFailed(u32),
    GetCompletedPieces(oneshot::Sender<Vec<u32>>),
    ReadPiece {
        index: u32,
        begin: u32,
        length: u32,
        reply: oneshot::Sender<Option<Vec<u8>>>,
    },
    // BEP 9/10
    RequestMetadataWork(oneshot::Sender<Option<u32>>),
    MetadataPieceDownloaded(u32, Vec<u8>),
    MetadataPieceFailed(u32),
    Pause,
    Resume,
}

pub enum CoordinatorState {
    DownloadingMetadata {
        assembler: MetadataAssembler,
        info_hash: [u8; 20],
        output_dir: PathBuf,
    },
    CheckingFiles {
        manager: TorrentManager,
        disk_writer: Box<dyn AsyncDiskIO>,
        expected_hashes: std::sync::Arc<Vec<[u8; 20]>>,
        piece_length: u32,
        total_length: u64,
        next_piece: u32,
    },
    DownloadingData {
        manager: TorrentManager,
        disk_writer: Box<dyn AsyncDiskIO>,
        paused: bool,
        has_completed: bool,
    }
}

struct DummyDisk;
#[async_trait::async_trait(?Send)]
impl AsyncDiskIO for DummyDisk {
    async fn write_piece(&mut self, _index: u32, _data: Vec<u8>) -> anyhow::Result<()> { Ok(()) }
    async fn read_piece(&mut self, _index: u32, _offset: u32, _len: u32) -> anyhow::Result<Vec<u8>> { Ok(vec![]) }
}

pub async fn run_coordinator(
    mut receiver: mpsc::Receiver<CoordinatorMsg>,
    ui_sender: mpsc::Sender<CoreMessage>,
    mut state: CoordinatorState,
    resume_path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<()>,
    announce_tx: broadcast::Sender<crate::core::command::SessionEvent>,
) {
    info!("Coordinator task started");

    let mut check_interval = tokio::time::interval(tokio::time::Duration::from_millis(1));

    loop {
        let msg = tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("coordinator received shutdown signal");
                break;
            }
            incoming = receiver.recv() => incoming,
            _ = check_interval.tick(), if matches!(state, CoordinatorState::CheckingFiles { .. }) => {
                let mut transition = false;
                if let CoordinatorState::CheckingFiles { manager, disk_writer, expected_hashes, piece_length, total_length, next_piece } = &mut state {
                    let index = *next_piece;
                    let total_pieces = manager.total_pieces;
                    if index >= total_pieces {
                        transition = true;
                    } else {
                        let length = if index == total_pieces - 1 {
                            let rem = (*total_length % *piece_length as u64) as u32;
                            if rem == 0 { *piece_length } else { rem }
                        } else {
                            *piece_length
                        };

                        if let Ok(data) = disk_writer.read_piece(index, 0, length).await {
                            use sha1::{Sha1, Digest};
                            let mut hasher = Sha1::new();
                            hasher.update(&data);
                            let hash = hasher.finalize();
                            if hash.as_slice() == expected_hashes[index as usize].as_slice() {
                                manager.mark_completed(index);
                                let _ = announce_tx.send(crate::core::command::SessionEvent::PieceCompleted(index));
                            }
                        }

                        *next_piece += 1;
                        if *next_piece % 10 == 0 || *next_piece == total_pieces {
                            let pct = (*next_piece as f32 / total_pieces as f32) * 100.0;
                            let _ = ui_sender.send(CoreMessage::Status(format!("Checking files: {:.1}%", pct))).await;
                            let _ = ui_sender.send(CoreMessage::GlobalProgress(manager.progress())).await;
                        }
                        
                        if *next_piece >= total_pieces {
                            transition = true;
                        }
                    }
                }

                if transition {
                    let dummy_state = CoordinatorState::DownloadingData {
                        manager: TorrentManager::new(0),
                        disk_writer: Box::new(DummyDisk),
                        paused: false,
                        has_completed: false,
                    };
                    if let CoordinatorState::CheckingFiles { manager, disk_writer, .. } = std::mem::replace(&mut state, dummy_state) {
                        let has_completed = manager.is_done();
                        if has_completed {
                            info!("torrent download complete (from local files), entering seeding mode");
                            let _ = ui_sender.send(CoreMessage::DownloadComplete).await;
                        }
                        let _ = ui_sender.send(CoreMessage::Status("File check complete".to_string())).await;
                        state = CoordinatorState::DownloadingData { manager, disk_writer, paused: false, has_completed };
                    }
                }
                continue;
            }
        };

        let Some(msg) = msg else {
            break;
        };

        match msg {
            CoordinatorMsg::RequestWork(reply) => {
                if let CoordinatorState::DownloadingData { manager, paused, .. } = &mut state {
                    if *paused {
                        let _ = reply.send(None);
                    } else {
                        let work = manager.get_next_work();
                        let _ = reply.send(work);
                    }
                } else {
                    let _ = reply.send(None);
                }
            }
            CoordinatorMsg::PieceDownloaded(index, data) => {
                if let CoordinatorState::DownloadingData { manager, disk_writer, has_completed, .. } = &mut state {
                    if let Err(err) = disk_writer.write_piece(index, data).await {
                        error!("disk write failed for piece {}: {}", index, err);
                        manager.return_work(index);
                        continue;
                    }

                    manager.mark_completed(index);
                    let _ = announce_tx.send(crate::core::command::SessionEvent::PieceCompleted(index));
                    let progress = manager.progress();
                    let _ = ui_sender.send(CoreMessage::GlobalProgress(progress)).await;

                    if let Err(err) = persist_resume(&resume_path, manager).await {
                        error!("failed to persist fastresume: {}", err);
                    }

                    if manager.is_done() {
                        if !*has_completed {
                            info!("torrent download complete, entering seeding mode");
                            let _ = ui_sender.send(CoreMessage::DownloadComplete).await;
                            *has_completed = true;
                        }
                    }
                }
            }
            CoordinatorMsg::PieceFailed(index) => {
                if let CoordinatorState::DownloadingData { manager, .. } = &mut state {
                    manager.return_work(index);
                }
            }
            CoordinatorMsg::GetCompletedPieces(reply) => {
                if let CoordinatorState::DownloadingData { manager, .. } = &mut state {
                    let _ = reply.send(manager.completed_pieces());
                } else {
                    let _ = reply.send(vec![]);
                }
            }
            CoordinatorMsg::ReadPiece { index, begin, length, reply } => {
                if let CoordinatorState::DownloadingData { manager, disk_writer, paused: _, .. } = &mut state {
                    let should_serve = matches!(manager.piece_state(index), Some(crate::core::manager::PieceState::Downloaded));

                    let data = if should_serve {
                        match disk_writer.read_piece(index, begin, length).await {
                            Ok(block) => Some(block),
                            Err(err) => {
                                error!("failed to read piece {} from disk: {}", index, err);
                                None
                            }
                        }
                    } else {
                        None
                    };

                    let _ = reply.send(data);
                } else {
                    let _ = reply.send(None);
                }
            }
            CoordinatorMsg::RequestMetadataWork(reply) => {
                if let CoordinatorState::DownloadingMetadata { assembler, .. } = &mut state {
                    let _ = reply.send(assembler.next_missing_piece());
                } else {
                    let _ = reply.send(None);
                }
            }
            CoordinatorMsg::MetadataPieceDownloaded(index, data) => {
                let mut transition_to_data = None;

                if let CoordinatorState::DownloadingMetadata { assembler, info_hash, output_dir } = &mut state {
                    match assembler.add_piece(index, &data) {
                        Ok(true) => {
                            info!("Metadata download complete! Verifying SHA-1...");
                            let buffer = assembler.get_buffer();
                            match parse_torrent_metadata_bytes(buffer, *info_hash) {
                                Ok(meta) => {
                                    info!("Metadata parsed successfully! Transitioning to DownloadingData...");
                                    transition_to_data = Some((meta, output_dir.clone()));
                                }
                                Err(err) => {
                                    error!("Failed to parse metadata: {}", err);
                                    // Could reset the assembler here, but let's just abort for now
                                }
                            }
                        }
                        Ok(false) => {
                            // Still downloading metadata
                        }
                        Err(e) => {
                            error!("Failed to add metadata piece: {}", e);
                        }
                    }
                }

                if let Some((meta, output_dir)) = transition_to_data {
                    let meta_arc = Arc::new(meta.clone());
                    let _ = ui_sender.send(CoreMessage::MetadataReady(meta_arc)).await;

                    let manager = TorrentManager::new(meta.pieces.len() as u32);
                    let total_size = meta.total_length.unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));

                    #[cfg(target_os = "linux")]
                    let disk_writer_res = crate::core::disk_uring::UringDisk::init(
                        &output_dir, total_size, meta.piece_length, meta.files.as_ref(), &meta.name
                    ).await.map(|d| Box::new(d) as Box<dyn AsyncDiskIO>);

                    #[cfg(not(target_os = "linux"))]
                    let disk_writer_res = crate::core::disk::StandardDisk::init(
                        &output_dir, total_size, meta.piece_length, meta.files.as_ref(), &meta.name
                    ).await.map(|d| Box::new(d) as Box<dyn AsyncDiskIO>);

                    match disk_writer_res {
                        Ok(disk_writer) => {
                            let target_path = output_dir.join(&meta.name);
                            if target_path.exists() {
                                let expected_hashes = Arc::new(meta.pieces.clone());
                                state = CoordinatorState::CheckingFiles {
                                    manager,
                                    disk_writer,
                                    expected_hashes,
                                    piece_length: meta.piece_length,
                                    total_length: total_size,
                                    next_piece: 0,
                                };
                                info!("Target path exists, transitioning to CheckingFiles...");
                            } else {
                                state = CoordinatorState::DownloadingData { manager, disk_writer, paused: false, has_completed: false };
                            }
                        }
                        Err(err) => {
                            error!("Failed to initialize disk for downloaded metadata: {}", err);
                        }
                    }
                }
            }
            CoordinatorMsg::MetadataPieceFailed(_index) => {
                // Next tick will just request it again
            }
            CoordinatorMsg::Pause => {
                if let CoordinatorState::DownloadingData { paused, .. } = &mut state {
                    *paused = true;
                    let _ = ui_sender.send(CoreMessage::PausedState(true)).await;
                }
            }
            CoordinatorMsg::Resume => {
                if let CoordinatorState::DownloadingData { paused, .. } = &mut state {
                    *paused = false;
                    let _ = ui_sender.send(CoreMessage::PausedState(false)).await;
                }
            }
        }
    }

    if let CoordinatorState::DownloadingData { manager, .. } = &state {
        if let Err(err) = persist_resume(&resume_path, manager).await {
            error!("failed to persist final fastresume: {}", err);
        }
    }

    info!("Coordinator task stopped");
}

async fn persist_resume(resume_path: &PathBuf, manager: &TorrentManager) -> anyhow::Result<()> {
    let state = FastResumeState::from_manager(manager);
    save_fastresume(resume_path, &state).await
}
