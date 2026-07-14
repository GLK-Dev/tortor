use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};
use std::path::PathBuf;

use crate::core::command::CoreMessage;
use crate::core::disk_io::AsyncDiskIO;
use crate::core::manager::TorrentManager;
use crate::core::resume::{save_fastresume, FastResumeState};

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
}

pub async fn run_coordinator(
    mut receiver: mpsc::Receiver<CoordinatorMsg>,
    ui_sender: mpsc::Sender<CoreMessage>,
    mut manager: TorrentManager,
    mut disk_writer: Box<dyn AsyncDiskIO>,
    resume_path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<()>,
    announce_tx: broadcast::Sender<u32>,
) {
    info!("Coordinator task started");
    let _ = ui_sender
        .send(CoreMessage::GlobalProgress(manager.progress()))
        .await;

    loop {
        let msg = tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("coordinator received shutdown signal");
                break;
            }
            incoming = receiver.recv() => incoming,
        };

        let Some(msg) = msg else {
            break;
        };

        match msg {
            CoordinatorMsg::RequestWork(reply) => {
                let work = manager.get_next_work();
                let _ = reply.send(work);
            }
            CoordinatorMsg::PieceDownloaded(index, data) => {
                if let Err(err) = disk_writer.write_piece(index, data).await {
                    error!("disk write failed for piece {}: {}", index, err);
                    manager.return_work(index);
                    continue;
                }

                manager.mark_completed(index);
                let _ = announce_tx.send(index);
                let progress = manager.progress();
                let _ = ui_sender.send(CoreMessage::GlobalProgress(progress)).await;

                if let Err(err) = persist_resume(&resume_path, &manager).await {
                    error!("failed to persist fastresume: {}", err);
                }

                if manager.is_done() {
                    info!("torrent download complete");
                    let _ = ui_sender.send(CoreMessage::DownloadComplete).await;
                    break;
                }
            }
            CoordinatorMsg::PieceFailed(index) => {
                manager.return_work(index);
            }
            CoordinatorMsg::GetCompletedPieces(reply) => {
                let _ = reply.send(manager.completed_pieces());
            }
            CoordinatorMsg::ReadPiece {
                index,
                begin,
                length,
                reply,
            } => {
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
            }
        }
    }

    if let Err(err) = persist_resume(&resume_path, &manager).await {
        error!("failed to persist final fastresume: {}", err);
    }

    info!("Coordinator task stopped");
}

async fn persist_resume(resume_path: &PathBuf, manager: &TorrentManager) -> anyhow::Result<()> {
    let state = FastResumeState::from_manager(manager);
    save_fastresume(resume_path, &state).await
}
