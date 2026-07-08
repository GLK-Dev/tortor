use std::path::PathBuf;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;
use eframe::egui;
use tokio::sync::Semaphore;
use tokio::time::{timeout, Duration};

use crate::core::bencode;
use crate::core::command::{CoreCommand, CoreMessage, SessionTelemetry};
use crate::core::peer_id::generate_peer_id;
use crate::core::torrent::TorrentMeta;
use crate::net::probe;
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

const MAX_CONCURRENT_PROBES: usize = 10;
const PROBE_QUEUE_TIMEOUT: Duration = Duration::from_secs(15);

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

    let runtime = tokio::runtime::Builder::new_current_thread()
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

    let target_piece_index = 0u32;
    let expected_piece_hash = match meta.piece_hash(target_piece_index as usize) {
        Some(hash) => hash,
        None => {
            tx.send(CoreMessage::Status(
                "Torrent has no piece hashes; probe data-path disabled".to_string(),
            ))
            .ok();
            return Ok(());
        }
    };
    let target_piece_length = match meta.piece_len_at(target_piece_index as usize) {
        Some(len) if len > 0 => len,
        _ => {
            tx.send(CoreMessage::Status(
                "Unable to determine target piece length; probe data-path disabled".to_string(),
            ))
            .ok();
            return Ok(());
        }
    };

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PROBES));
    tx.send(CoreMessage::Status(format!(
        "Probe limiter enabled: max {MAX_CONCURRENT_PROBES} concurrent probes"
    )))
    .ok();

    while let Ok(command) = command_rx.recv() {
        match command {
            CoreCommand::ProbePeer(addr) => {
                tx.send(CoreMessage::ProbeQueued(addr)).ok();

                let tx_clone = tx.clone();
                let sem_clone = Arc::clone(&semaphore);
                let info_hash = meta.info_hash;
                let probe_peer_id = peer_id;
                let piece_index = target_piece_index;
                let piece_length = target_piece_length;
                let piece_hash = expected_piece_hash;

                runtime.spawn(async move {
                    let permit = match timeout(PROBE_QUEUE_TIMEOUT, sem_clone.acquire_owned()).await {
                        Ok(Ok(permit)) => permit,
                        Ok(Err(_)) => {
                            let _ = tx_clone.send(CoreMessage::ProbeFailed(
                                addr,
                                "probe limiter closed".to_string(),
                            ));
                            return;
                        }
                        Err(_) => {
                            let _ = tx_clone.send(CoreMessage::ProbeFailed(
                                addr,
                                format!(
                                    "queue timeout after {}s",
                                    PROBE_QUEUE_TIMEOUT.as_secs()
                                ),
                            ));
                            return;
                        }
                    };

                    let _ = tx_clone.send(CoreMessage::ProbeStarted(addr));
                    let result = probe::execute_probe(
                        addr,
                        info_hash,
                        probe_peer_id,
                        piece_index,
                        piece_length,
                        piece_hash,
                        &tx_clone,
                    )
                    .await;
                    drop(permit);

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
}

impl TorTorApp {
    fn new(rx: Receiver<CoreMessage>, command_tx: Sender<CoreCommand>) -> Self {
        Self {
            rx,
            command_tx,
            peers: Vec::new(),
            logs: vec!["GUI started. Waiting for core events...".to_string()],
            meta: None,
        }
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

    fn pump_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                CoreMessage::Status(text) => self.logs.push(text),
                CoreMessage::TorrentLoaded(meta) => {
                    self.logs
                        .push(format!("Loaded torrent: {}", meta.name));
                    self.meta = Some(meta);
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
                                .add_enabled(can_probe, egui::Button::new("Start Probe"))
                                .clicked()
                            {
                                let _ = self.command_tx.send(CoreCommand::ProbePeer(addr));
                            }
                        });

                        if let Some(tel) = &row.telemetry {
                            let progress = (tel.downloaded_bytes as f32 / piece_len as f32)
                                .clamp(0.0, 1.0);

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

                                ui.label(format!(
                                    "In-flight: {} | Retries: {} | Drops: {}{}",
                                    tel.in_flight_requests,
                                    tel.retries,
                                    tel.unexpected_blocks + tel.duplicate_blocks,
                                    ttfp
                                ));
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
