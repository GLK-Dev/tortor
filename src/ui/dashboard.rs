use std::path::PathBuf;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use tokio::sync::{broadcast, mpsc as tokio_mpsc};
use tokio::time::{sleep, Duration};

use crate::core::bencode;
use crate::core::command::{CoreCommand, CoreMessage, SessionTelemetry};
use crate::core::coordinator::{self, CoordinatorMsg};
use crate::core::disk::DiskWriter;
use crate::core::manager::TorrentManager;
use crate::core::peer_id::generate_peer_id;
use crate::core::resume::load_fastresume;
use crate::core::torrent::TorrentMeta;
use crate::net::probe;
use crate::net::swarm;
use crate::net::tracker;

#[derive(Debug, Clone)]
enum ProbeState {
    Idle,
    Queued,
    Probing,
    Success(String),
    Failed(String),
}

#[derive(Debug, Clone)]
struct PeerRow {
    addr: SocketAddr,
    state: ProbeState,
    telemetry: Option<SessionTelemetry>,
}

pub fn run_dashboard(torrent_path: PathBuf, listen_port: u16) -> Result<()> {
    let (msg_tx, msg_rx) = mpsc::channel::<CoreMessage>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<CoreCommand>();

    std::thread::spawn(move || {
        if let Err(err) = background_task(msg_tx.clone(), cmd_rx, torrent_path, listen_port) {
            let _ = msg_tx.send(CoreMessage::Error(err.to_string()));
        }
    });

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "TorTor Dashboard",
        native_options,
        Box::new(move |_| Ok(Box::new(TorTorApp::new(msg_rx, cmd_tx.clone())))),
    )
    .map_err(|err| anyhow::anyhow!("failed to start GUI: {err}"))?;

    Ok(())
}

fn background_task(
    tx: mpsc::Sender<CoreMessage>,
    command_rx: Receiver<CoreCommand>,
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

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let peers = runtime.block_on(async {
        tracker::announce(&tracker_url, &meta.info_hash, &peer_id, listen_port, left).await
    })?;

    for peer in &peers {
        tx.send(CoreMessage::PeerFound(peer.addr)).ok();
    }
    tx.send(CoreMessage::TrackerDone(peers.len())).ok();
    tx.send(CoreMessage::Status(
        "Core worker is ready for probe commands".to_string(),
    ))
    .ok();

    if meta.pieces.is_empty() {
        tx.send(CoreMessage::Status(
            "Torrent has no piece hashes; download data-path disabled".to_string(),
        ))
        .ok();
        return Ok(());
    }

    let (ui_async_tx, mut ui_async_rx) = tokio_mpsc::channel::<CoreMessage>(1024);
    let (shutdown_tx, _) = broadcast::channel::<()>(16);
    let ui_bridge_tx = tx.clone();
    runtime.spawn(async move {
        while let Some(msg) = ui_async_rx.recv().await {
            let _ = ui_bridge_tx.send(msg);
        }
    });

    let total_size = meta
        .total_length
        .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));
    let output_path = torrent_path.with_extension("download.part");
    let disk_writer = runtime.block_on(DiskWriter::init(
        output_path,
        total_size,
        meta.piece_length,
    ))?;
    let resume_path = torrent_path.with_extension("fastresume");
    let manager = match runtime.block_on(load_fastresume(&resume_path)) {
        Ok(Some(state)) => {
            let mgr = state.clone().into_manager(meta.pieces_count);
            tx.send(CoreMessage::Status(format!(
                "Fast resume loaded: {} completed pieces",
                mgr.completed_count()
            )))
            .ok();
            mgr
        }
        Ok(None) => TorrentManager::new(meta.pieces_count),
        Err(err) => {
            tx.send(CoreMessage::Status(format!(
                "Fast resume load failed, starting fresh: {}",
                err
            )))
            .ok();
            TorrentManager::new(meta.pieces_count)
        }
    };
    let (coord_tx, coord_rx) = tokio_mpsc::channel::<CoordinatorMsg>(2048);
    runtime.spawn(coordinator::run_coordinator(
        coord_rx,
        ui_async_tx.clone(),
        manager,
        disk_writer,
        resume_path,
        shutdown_tx.subscribe(),
    ));

    let expected_hashes = Arc::new(meta.pieces.clone());
    let piece_length = meta.piece_length;
    let total_length = meta.total_length;
    let available_peers: std::collections::VecDeque<SocketAddr> =
        peers.iter().map(|p| p.addr).collect();
    let mut swarm_started = false;

    tx.send(CoreMessage::Status(format!(
        "Swarm is ready. Press Start Swarm to begin autonomous download"
    )))
    .ok();

    while let Ok(command) = command_rx.recv() {
        match command {
            CoreCommand::StartSwarm => {
                if swarm_started {
                    tx.send(CoreMessage::Status("Swarm is already running".to_string()))
                        .ok();
                    continue;
                }

                swarm_started = true;
                let info_hash = meta.info_hash;
                let swarm_peer_id = peer_id;
                let expected_hashes = Arc::clone(&expected_hashes);
                let ui_async_tx = ui_async_tx.clone();
                let coord_tx = coord_tx.clone();
                let shutdown_tx = shutdown_tx.clone();
                let available = available_peers.clone();

                runtime.spawn(async move {
                    swarm::run_swarm_manager(
                        available,
                        info_hash,
                        swarm_peer_id,
                        expected_hashes,
                        piece_length,
                        total_length,
                        ui_async_tx,
                        coord_tx,
                        shutdown_tx,
                    )
                    .await;
                });
            }
            CoreCommand::ProbePeer(addr) => {
                if swarm_started {
                    tx.send(CoreMessage::Status(
                        "Ignoring manual worker start while swarm is running".to_string(),
                    ))
                    .ok();
                    continue;
                }

                tx.send(CoreMessage::ProbeQueued(addr)).ok();

                let tx_clone = tx.clone();
                let info_hash = meta.info_hash;
                let probe_peer_id = peer_id;
                let expected_hashes = Arc::clone(&expected_hashes);
                let ui_async_tx = ui_async_tx.clone();
                let coord_tx = coord_tx.clone();
                let shutdown_rx = shutdown_tx.subscribe();

                runtime.spawn(async move {
                    let _ = tx_clone.send(CoreMessage::ProbeStarted(addr));
                    let result = probe::execute_probe(
                        addr,
                        info_hash,
                        probe_peer_id,
                        expected_hashes,
                        piece_length,
                        total_length,
                        ui_async_tx,
                        coord_tx,
                        shutdown_rx,
                        None,
                    )
                    .await;

                    match result {
                        Ok(status) => {
                            let _ = tx_clone.send(CoreMessage::ProbeSucceeded(addr, status));
                        }
                        Err(err) => {
                            let _ = tx_clone.send(CoreMessage::ProbeFailed(addr, err.to_string()));
                        }
                    }
                });
            }
            CoreCommand::StopAll => {
                tx.send(CoreMessage::Status("Stop requested: shutting down workers".to_string()))
                    .ok();
                let _ = shutdown_tx.send(());
                runtime.block_on(async {
                    sleep(Duration::from_millis(800)).await;
                });
                tx.send(CoreMessage::ShutdownComplete).ok();
                break;
            }
        }
    }

    Ok(())
}

struct TorTorApp {
    rx: Receiver<CoreMessage>,
    command_tx: Sender<CoreCommand>,
    peers: Vec<PeerRow>,
    logs: Vec<String>,
    meta: Option<TorrentMeta>,
    global_progress: f32,
    status: String,
    is_shutting_down: bool,
    swarm_started: bool,
}

impl TorTorApp {
    fn new(rx: Receiver<CoreMessage>, command_tx: Sender<CoreCommand>) -> Self {
        Self {
            rx,
            command_tx,
            peers: Vec::new(),
            logs: vec!["GUI started. Waiting for core events...".to_string()],
            meta: None,
            global_progress: 0.0,
            status: "Ready".to_string(),
            is_shutting_down: false,
            swarm_started: false,
        }
    }

    fn request_shutdown(&mut self) {
        if self.is_shutting_down {
            return;
        }

        self.is_shutting_down = true;
        self.status = "Stopping workers and persisting state...".to_string();
        self.logs
            .push("Shutdown requested: waiting for graceful stop".to_string());
        let _ = self.command_tx.send(CoreCommand::StopAll);
    }

    fn update_peer_state(&mut self, addr: SocketAddr, state: ProbeState) {
        if let Some(row) = self.peers.iter_mut().find(|row| row.addr == addr) {
            row.state = state;
        }
    }

    fn status_label(state: &ProbeState) -> String {
        match state {
            ProbeState::Idle => "idle".to_string(),
            ProbeState::Queued => "queued".to_string(),
            ProbeState::Probing => "probing...".to_string(),
            ProbeState::Success(msg) => format!("ok: {msg}"),
            ProbeState::Failed(err) => format!("failed: {err}"),
        }
    }

    fn pump_messages(&mut self) -> bool {
        let mut shutdown_complete = false;

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                CoreMessage::Status(text) => {
                    self.status = text.clone();
                    self.logs.push(text);
                }
                CoreMessage::TorrentLoaded(meta) => {
                    self.logs
                        .push(format!("Loaded torrent: {}", meta.name));
                    self.meta = Some(meta);
                }
                CoreMessage::GlobalProgress(progress) => {
                    self.global_progress = progress.clamp(0.0, 1.0);
                }
                CoreMessage::DownloadComplete => {
                    self.global_progress = 1.0;
                    self.status = "Download complete".to_string();
                    self.logs.push("All pieces were downloaded".to_string());
                }
                CoreMessage::ShutdownComplete => {
                    self.status = "Shutdown complete".to_string();
                    self.logs.push("All workers stopped safely".to_string());
                    shutdown_complete = true;
                }
                CoreMessage::PeerFound(addr) => self.peers.push(PeerRow {
                    addr,
                    state: ProbeState::Idle,
                    telemetry: None,
                }),
                CoreMessage::TrackerDone(count) => {
                    self.logs.push(format!("Tracker returned {count} peers"));
                }
                CoreMessage::ProbeQueued(addr) => {
                    self.update_peer_state(addr, ProbeState::Queued);
                    self.logs.push(format!("Probe queued: {addr}"));
                }
                CoreMessage::ProbeStarted(addr) => {
                    self.update_peer_state(addr, ProbeState::Probing);
                    self.logs.push(format!("Probe started: {addr}"));
                }
                CoreMessage::ProbeSucceeded(addr, status) => {
                    self.update_peer_state(addr, ProbeState::Success(status.clone()));
                    self.logs.push(format!("Probe succeeded: {addr} | {status}"));
                }
                CoreMessage::ProbeFailed(addr, err) => {
                    self.update_peer_state(addr, ProbeState::Failed(err.clone()));
                    self.logs.push(format!("Probe failed for {addr}: {err}"));
                }
                CoreMessage::TelemetryUpdate(addr, telemetry) => {
                    if let Some(row) = self.peers.iter_mut().find(|row| row.addr == addr) {
                        row.telemetry = Some(telemetry);
                    }
                }
                CoreMessage::Error(err) => {
                    self.logs.push(format!("Error: {err}"));
                }
            }
        }

        shutdown_complete
    }
}

impl eframe::App for TorTorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.is_shutting_down {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_shutdown();
        }

        let shutdown_complete = self.pump_messages();
        if shutdown_complete {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if self.is_shutting_down {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.heading("Stopping modules and saving state...");
                    ui.add_space(12.0);
                    ui.spinner();
                });
            });
            ctx.request_repaint();
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("TorTor Core Dashboard");
            ui.label(format!("Status: {}", self.status));
            if ui
                .add_enabled(!self.swarm_started, egui::Button::new("Start Swarm"))
                .clicked()
            {
                self.swarm_started = true;
                let _ = self.command_tx.send(CoreCommand::StartSwarm);
            }
            if ui.button("Stop All").clicked() {
                self.request_shutdown();
            }
            ui.separator();

            ui.heading("TorTor Global Progress");
            ui.add(egui::ProgressBar::new(self.global_progress).show_percentage());
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
                    for row in &self.peers {
                        let addr = row.addr;
                        let state_text = Self::status_label(&row.state);
                        let can_probe = !matches!(row.state, ProbeState::Probing);
                        let piece_len = self
                            .meta
                            .as_ref()
                            .and_then(|m| m.piece_len_at(0))
                            .unwrap_or(1);

                        ui.horizontal(|ui| {
                            ui.monospace(addr.to_string());
                            ui.label(state_text);

                            if ui
                                .add_enabled(can_probe && !self.swarm_started, egui::Button::new("Start Worker"))
                                .clicked()
                            {
                                let _ = self.command_tx.send(CoreCommand::ProbePeer(addr));
                            }
                        });

                        if let Some(tel) = &row.telemetry {
                            let progress = (tel.downloaded_bytes as f32 / piece_len as f32)
                                .clamp(0.0, 1.0);
                            let drops = tel.unexpected_blocks + tel.duplicate_blocks;

                            let drop_color = if drops > 10 {
                                Color32::RED
                            } else if drops > 0 {
                                Color32::YELLOW
                            } else {
                                Color32::GREEN
                            };

                            let retry_color = if tel.retries > 5 {
                                Color32::YELLOW
                            } else {
                                Color32::LIGHT_GRAY
                            };

                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(180.0)
                                        .text(format!(
                                            "{} / {} B",
                                            tel.downloaded_bytes, piece_len
                                        )),
                                );

                                let ttfp = tel
                                    .time_to_first_piece_ms
                                    .map(|v| format!(" | TTFP: {} ms", v))
                                    .unwrap_or_default();

                                ui.label(
                                    RichText::new(format!("In-flight: {}", tel.in_flight_requests))
                                        .color(Color32::LIGHT_BLUE),
                                );
                                ui.label("|");
                                ui.label(
                                    RichText::new(format!("Retries: {}", tel.retries))
                                        .color(retry_color),
                                );
                                ui.label("|");
                                ui.label(RichText::new(format!("Drops: {}", drops)).color(drop_color));
                                if !ttfp.is_empty() {
                                    ui.label(ttfp);
                                }
                            });
                        }

                        ui.separator();
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
