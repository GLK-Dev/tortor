use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use eframe::egui;

use crate::core::bencode;
use crate::core::peer_id::generate_peer_id;
use crate::core::torrent::TorrentMeta;
use crate::net::tracker;

#[derive(Debug)]
enum CoreMessage {
    Status(String),
    TorrentLoaded(TorrentMeta),
    PeerFound(String),
    TrackerDone(usize),
    Error(String),
}

pub fn run_dashboard(torrent_path: PathBuf, listen_port: u16) -> Result<()> {
    let (tx, rx) = mpsc::channel::<CoreMessage>();

    std::thread::spawn(move || {
        if let Err(err) = background_task(tx.clone(), torrent_path, listen_port) {
            let _ = tx.send(CoreMessage::Error(err.to_string()));
        }
    });

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "TorTor Dashboard",
        native_options,
        Box::new(move |_| Ok(Box::new(TorTorApp::new(rx)))),
    )
    .map_err(|err| anyhow::anyhow!("failed to start GUI: {err}"))?;

    Ok(())
}

fn background_task(
    tx: mpsc::Sender<CoreMessage>,
    torrent_path: PathBuf,
    listen_port: u16,
) -> Result<()> {
    let meta = bencode::parse_torrent_file(&torrent_path)?;
    tx.send(CoreMessage::TorrentLoaded(meta.clone())).ok();

    let tracker_url = meta.announce.clone();
    if !(tracker_url.starts_with("http://") || tracker_url.starts_with("https://")) {
        tx.send(CoreMessage::Status(format!(
            "Tracker is not HTTP/HTTPS, skipping announce: {tracker_url}"
        )))
        .ok();
        return Ok(());
    }

    tx.send(CoreMessage::Status(format!(
        "Announcing to tracker: {tracker_url}"
    )))
    .ok();

    let left = meta
        .total_length
        .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));
    let peer_id = generate_peer_id();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let peers = runtime.block_on(async {
        tracker::announce(&tracker_url, &meta.info_hash, &peer_id, listen_port, left).await
    })?;

    for peer in &peers {
        tx.send(CoreMessage::PeerFound(peer.addr.to_string())).ok();
    }
    tx.send(CoreMessage::TrackerDone(peers.len())).ok();

    Ok(())
}

struct TorTorApp {
    rx: Receiver<CoreMessage>,
    peers: Vec<String>,
    logs: Vec<String>,
    meta: Option<TorrentMeta>,
}

impl TorTorApp {
    fn new(rx: Receiver<CoreMessage>) -> Self {
        Self {
            rx,
            peers: Vec::new(),
            logs: vec!["GUI started. Waiting for core events...".to_string()],
            meta: None,
        }
    }

    fn pump_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                CoreMessage::Status(text) => self.logs.push(text),
                CoreMessage::TorrentLoaded(meta) => {
                    self.logs
                        .push(format!("Loaded torrent: {}", meta.name));
                    self.meta = Some(meta);
                }
                CoreMessage::PeerFound(addr) => self.peers.push(addr),
                CoreMessage::TrackerDone(count) => {
                    self.logs.push(format!("Tracker returned {count} peers"));
                }
                CoreMessage::Error(err) => {
                    self.logs.push(format!("Error: {err}"));
                }
            }
        }
    }
}

impl eframe::App for TorTorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_messages();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("TorTor Core Dashboard");
            ui.separator();

            if let Some(meta) = &self.meta {
                ui.label(format!("Name: {}", meta.name));
                ui.label(format!("Announce: {}", meta.announce));
                ui.label(format!("Pieces: {}", meta.pieces_count));
                ui.label(format!("Info hash: {}", meta.info_hash_hex()));
            } else {
                ui.label("Torrent metadata is loading...");
            }

            ui.separator();
            ui.heading(format!("Peers ({})", self.peers.len()));
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for peer in &self.peers {
                        ui.label(peer);
                    }
                });

            ui.separator();
            ui.heading("Logs");
            egui::ScrollArea::vertical().show(ui, |ui| {
                for line in self.logs.iter().rev().take(20) {
                    ui.label(line);
                }
            });
        });

        ctx.request_repaint();
    }
}
