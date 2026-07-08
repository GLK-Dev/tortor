use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

use crate::core::command::CoreMessage;
use crate::core::disk::DiskWriter;
use crate::core::manager::TorrentManager;

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
) {
    info!("Coordinator task started");
    let _ = ui_sender.send(CoreMessage::GlobalProgress(0.0)).await;

    while let Some(msg) = receiver.recv().await {
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

    info!("Coordinator task stopped");
}
