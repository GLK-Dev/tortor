use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info};
use std::path::PathBuf;

use crate::core::command::CoreMessage;
use crate::core::disk::DiskWriter;
use crate::core::manager::TorrentManager;
use crate::core::resume::{save_fastresume, FastResumeState};

pub enum CoordinatorMsg {
    RequestWork(oneshot::Sender<Option<u32>>),
    PieceDownloaded(u32, Vec<u8>),
    PieceFailed(u32),
}

pub async fn run_coordinator(
    mut receiver: mpsc::Receiver<CoordinatorMsg>,
    ui_sender: mpsc::Sender<CoreMessage>,
    mut manager: TorrentManager,
    mut disk_writer: DiskWriter,
    resume_path: PathBuf,
    mut shutdown_rx: broadcast::Receiver<()>,
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
                if let Err(err) = disk_writer.write_piece(index, &data).await {
                    error!("disk write failed for piece {}: {}", index, err);
                    manager.return_work(index);
                    continue;
                }

                manager.mark_completed(index);
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
