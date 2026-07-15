use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use tokio::sync::{broadcast, mpsc as tokio_mpsc};
use tokio::time::{sleep, Duration};

use crate::core::bencode;
use crate::core::command::{CoreCommand, CoreMessage, SessionTelemetry};
use crate::core::coordinator::{self, CoordinatorMsg};
use crate::core::session_store::TorrentSource;
use crate::core::disk::StandardDisk;
#[cfg(target_os = "linux")]
use crate::core::disk_uring::UringDisk;
use crate::core::manager::TorrentManager;
use crate::core::peer_id::generate_peer_id;
use crate::core::resume::load_fastresume;
use crate::core::torrent::TorrentMeta;
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

fn ascii_progress_bar(progress: f32, width: usize) -> String {
    let p = progress.clamp(0.0, 1.0);
    let filled = (p * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    let filled_str = "█".repeat(filled);
    let empty_str = "░".repeat(empty);
    format!("[{}{}] {}%", filled_str, empty_str, (p * 100.0) as u32)
}

pub fn run_dashboard(initial_torrent_path: Option<PathBuf>, listen_port: u16, output_dir: PathBuf) -> Result<()> {
    let mut native_options = eframe::NativeOptions::default();
    
    // Load window icon
    let icon_data = include_bytes!("../../Images/tortor_icon.png");
    if let Ok(image) = image::load_from_memory(icon_data) {
        let image = image.into_rgba8();
        let (width, height) = image.dimensions();
        native_options.viewport = egui::ViewportBuilder::default()
            .with_icon(Arc::new(egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }));
    }

    eframe::run_native(
        "TorTor Download Manager",
        native_options,
        Box::new(move |_| {
            let mut app = TorTorApp::new(listen_port);
            
            // Resume saved sessions
            let entries = app.session_store.entries.clone();
            for entry in entries {
                app.start_core(entry.source, entry.output_dir);
            }
            
            // Start CLI-provided torrent if any
            if let Some(path) = initial_torrent_path {
                app.start_core(crate::core::session_store::TorrentSource::File(path), output_dir);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("failed to start GUI: {err}"))?;

    Ok(())
}

fn background_task(
    session_id: usize,
    tx: mpsc::Sender<(usize, CoreMessage)>,
    command_rx: Receiver<CoreCommand>,
    torrent_source: TorrentSource,
    listen_port: u16,
    output_dir: PathBuf,
) -> Result<()> {
    let meta = match &torrent_source {
        TorrentSource::File(path) => match bencode::parse_torrent_file(path) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send((session_id, CoreMessage::Error(format!("Failed to parse torrent: {e}"))));
                return Err(e.into());
            }
        },
        TorrentSource::Magnet(uri) => {
            // Very basic Magnet URI parse for now (we just need info_hash)
            let mut info_hash = [0u8; 20];
            if let Some(hash_str) = uri.split("urn:btih:").nth(1).map(|s| s.split('&').next().unwrap_or(s)) {
                if hash_str.len() == 40 {
                    if let Ok(bytes) = hex::decode(hash_str) {
                        info_hash.copy_from_slice(&bytes);
                    }
                }
            }
            TorrentMeta {
                info_hash,
                name: "Magnet Download".to_string(),
                piece_length: 256 * 1024,
                pieces_count: 0,
                total_length: None,
                announce: "".to_string(),
                pieces: vec![],
                files: None,
            }
        }
    };
    tx.send((session_id, CoreMessage::TorrentLoaded(meta.clone()))).ok();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    tx.send((session_id, CoreMessage::Status(
        "Ready to start. Press Start Swarm to begin allocation and download.".to_string(),
    )))
    .ok();

    // Wait for the user to press "Start Swarm" or "StopAll"
    let mut started = false;
    while let Ok(command) = command_rx.recv() {
        match command {
            CoreCommand::StartSwarm => {
                started = true;
                break;
            }
            CoreCommand::StopAll => {
                tx.send((session_id, CoreMessage::ShutdownComplete)).ok();
                return Ok(());
            }
            _ => {}
        }
    }

    if !started {
        return Ok(());
    }

    let tracker_url = meta.announce.clone();
    if !(tracker_url.starts_with("http://") || tracker_url.starts_with("https://") || tracker_url.starts_with("udp://")) {
        tx.send((session_id, CoreMessage::Status(format!(
            "Tracker is not HTTP/HTTPS/UDP, skipping announce: {tracker_url}"
        ))))
        .ok();
        return Ok(());
    }

    tx.send((session_id, CoreMessage::Status(format!(
        "Announcing to tracker: {tracker_url}"
    ))))
    .ok();

    let left = meta
        .total_length
        .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));
    let peer_id = generate_peer_id();

    let peers = runtime.block_on(async {
        tracker::announce(&tracker_url, &meta.info_hash, &peer_id, listen_port, left).await
    })?;

    for peer in &peers {
        tx.send((session_id, CoreMessage::PeerFound(peer.addr))).ok();
    }
    tx.send((session_id, CoreMessage::TrackerDone(peers.len()))).ok();
    tx.send((session_id, CoreMessage::Status(
        "Core worker is ready for probe commands".to_string(),
    )))
    .ok();

    if meta.pieces.is_empty() {
        tx.send((session_id, CoreMessage::Status(
            "Torrent has no piece hashes; download data-path disabled".to_string(),
        )))
        .ok();
        return Ok(());
    }

    let (ui_async_tx, mut ui_async_rx) = tokio_mpsc::channel::<CoreMessage>(1024);
    let (shutdown_tx, _) = broadcast::channel::<()>(16);
    let (announce_tx, _) = broadcast::channel::<crate::core::command::SessionEvent>(64);
    let ui_bridge_tx = tx.clone();
    runtime.spawn(async move {
        while let Some(msg) = ui_async_rx.recv().await {
            let _ = ui_bridge_tx.send((session_id, msg));
        }
    });

    let total_size = meta
        .total_length
        .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));
    
    let resume_path = match &torrent_source {
        TorrentSource::File(path) => path.with_extension("fastresume"),
        TorrentSource::Magnet(_) => output_dir.join(format!("{}.fastresume", hex::encode(meta.info_hash))),
    };

    let target_path = output_dir.join(&meta.name);
    let mut is_valid = target_path.exists();
    if is_valid && target_path.is_file() {
        if let Ok(metadata) = std::fs::metadata(&target_path) {
            if metadata.len() == 0 {
                is_valid = false;
            }
        }
    }
    
    let manager = match runtime.block_on(load_fastresume(&resume_path)) {
        Ok(Some(state)) if is_valid => {
            let mgr = state.clone().into_manager(meta.pieces_count);
            tx.send((session_id, CoreMessage::Status(format!(
                "Fast resume loaded: {} completed pieces",
                mgr.completed_count()
            ))))
            .ok();
            mgr
        }
        Ok(Some(_)) => {
             tracing::warn!("Файлы для торрента {} не найдены на диске! Остановка загрузки.", meta.name);
             let mgr = TorrentManager::new(meta.pieces_count);
             tx.send((session_id, CoreMessage::Status("[ERROR: Missing]".to_string()))).ok();
             tx.send((session_id, CoreMessage::Error("Files missing".to_string()))).ok(); // signal error state
             mgr
        }
        Ok(None) => TorrentManager::new(meta.pieces_count),
        Err(err) => {
            tx.send((session_id, CoreMessage::Status(format!(
                "Failed to load fast resume: {err}"
            ))))
            .ok();
            TorrentManager::new(meta.pieces_count)
        }
    };
    let (coord_tx, coord_rx) = tokio_mpsc::channel::<CoordinatorMsg>(2048);

    #[cfg(not(target_os = "linux"))]
    {
        let output_dir_c = output_dir.clone();
        let meta_files = meta.files.clone();
        let meta_name = meta.name.clone();
        let meta_piece_length = meta.piece_length;
        
        let ui_async_tx_c = ui_async_tx.clone();
        let shutdown_rx_c = shutdown_tx.subscribe();
        let announce_tx_c = announce_tx.clone();
        let resume_path_c = resume_path.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                match StandardDisk::init(
                    &output_dir_c,
                    total_size,
                    meta_piece_length,
                    meta_files.as_ref(),
                    &meta_name,
                ).await {
                    Ok(disk_writer) => {
                        let disk_writer = Box::new(disk_writer);
                        let state = coordinator::CoordinatorState::DownloadingData { manager, disk_writer, paused: false };
                        coordinator::run_coordinator(
                            coord_rx,
                            ui_async_tx_c,
                            state,
                            resume_path_c,
                            shutdown_rx_c,
                            announce_tx_c,
                        ).await;
                    }
                    Err(e) => {
                        let _ = ui_async_tx_c.send(CoreMessage::Error(format!("Failed to init StandardDisk: {}", e))).await;
                    }
                }
            });
        });
    }

    #[cfg(target_os = "linux")]
    {
        let output_dir_c = output_dir.clone();
        let meta_files = meta.files.clone();
        let meta_name = meta.name.clone();
        let meta_piece_length = meta.piece_length;
        
        let ui_async_tx_c = ui_async_tx.clone();
        let shutdown_rx_c = shutdown_tx.subscribe();
        let announce_tx_c = announce_tx.clone();
        let resume_path_c = resume_path.clone();

        std::thread::spawn(move || {
            tokio_uring::start(async move {
                match UringDisk::init(
                    &output_dir_c,
                    total_size,
                    meta_piece_length,
                    meta_files.as_ref(),
                    &meta_name,
                ).await {
                    Ok(disk_writer) => {
                        let disk_writer = Box::new(disk_writer);
                        let state = coordinator::CoordinatorState::DownloadingData { manager, disk_writer, paused: false };
                        coordinator::run_coordinator(
                            coord_rx,
                            ui_async_tx_c,
                            state,
                            resume_path_c,
                            shutdown_rx_c,
                            announce_tx_c,
                        ).await;
                    }
                    Err(e) => {
                        let _ = ui_async_tx_c.send(CoreMessage::Error(format!("Failed to init UringDisk: {}", e))).await;
                    }
                }
            });
        });
    }

    let expected_hashes = Arc::new(meta.pieces.clone());
    let piece_length = meta.piece_length;
    let total_length = meta.total_length;
    let available_peers: std::collections::VecDeque<SocketAddr> =
        peers.iter().map(|p| p.addr).collect();

    let info_hash = meta.info_hash;
    let swarm_peer_id = peer_id;

    let shutdown_tx_for_swarm = shutdown_tx.clone();
    let coord_tx_for_cmd = coord_tx.clone();
    runtime.spawn(async move {
        swarm::run_swarm_manager(
            available_peers,
            tracker_url,
            info_hash,
            swarm_peer_id,
            listen_port,
            left,
            expected_hashes,
            piece_length,
            total_length,
            ui_async_tx,
            coord_tx,
            shutdown_tx_for_swarm,
            announce_tx,
        )
        .await;
    });

    while let Ok(command) = command_rx.recv() {
        match command {
            CoreCommand::StopAll => {
                tx.send((session_id, CoreMessage::Status("Stop requested: shutting down workers".to_string()))).ok();
                let _ = shutdown_tx.send(());
                runtime.block_on(async {
                    sleep(Duration::from_millis(800)).await;
                });
                tx.send((session_id, CoreMessage::ShutdownComplete)).ok();
                break;
            }
            CoreCommand::Pause => {
                let _ = coord_tx_for_cmd.try_send(CoordinatorMsg::Pause);
            }
            CoreCommand::Resume => {
                let _ = coord_tx_for_cmd.try_send(CoordinatorMsg::Resume);
            }
            _ => {}
        }
    }

    Ok(())
}

struct TorrentSessionState {
    id: usize,
    output_dir: PathBuf,
    source: TorrentSource,
    command_tx: Sender<CoreCommand>,
    peers: Vec<PeerRow>,
    logs: Vec<String>,
    meta: Option<TorrentMeta>,
    global_progress: f32,
    status: String,
    is_shutting_down: bool,
    delete_requested: bool,
    swarm_started: bool,
    is_paused: bool,
    expanded: bool,
    has_error: bool,
    remove_requested: bool,
}

struct TorTorApp {
    show_about: bool,
    show_link_input: bool,
    link_input_buffer: String,
    tx: Sender<(usize, CoreMessage)>,
    rx: Receiver<(usize, CoreMessage)>,
    sessions: HashMap<usize, TorrentSessionState>,
    next_id: usize,
    listen_port: u16,
    session_store: crate::core::session_store::SessionStore,
    quit_requested: bool,
}

impl TorTorApp {
    fn new(listen_port: u16) -> Self {
        let (tx, rx) = mpsc::channel();
        let session_store = crate::core::session_store::SessionStore::load(&std::path::PathBuf::from("session.json")).unwrap_or_default();
        
        Self {
            show_about: false,
            show_link_input: false,
            link_input_buffer: String::new(),
            tx,
            rx,
            sessions: HashMap::new(),
            next_id: 1,
            listen_port,
            session_store,
            quit_requested: false,
        }
    }

    fn start_core(&mut self, torrent_source: TorrentSource, output_dir: PathBuf) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<CoreCommand>();
        let id = self.next_id;
        self.next_id += 1;

        let session = TorrentSessionState {
            id,
            output_dir: output_dir.clone(),
            source: torrent_source.clone(),
            command_tx: cmd_tx,
            peers: Vec::new(),
            logs: vec![format!("Loading torrent: {:?}", torrent_source)],
            meta: None,
            global_progress: 0.0,
            status: format!("Starting core for {:?}", torrent_source),
            is_shutting_down: false,
            delete_requested: false,
            swarm_started: false,
            is_paused: false,
            expanded: true,
            has_error: false,
            remove_requested: false,
        };

        self.sessions.insert(id, session);

        let tx = self.tx.clone();
        let listen_port = self.listen_port;

        std::thread::spawn(move || {
            if let Err(err) = background_task(id, tx.clone(), cmd_rx, torrent_source, listen_port, output_dir) {
                let _ = tx.send((id, CoreMessage::Error(err.to_string())));
            }
        });
    }

    fn update_peer_state(session: &mut TorrentSessionState, addr: SocketAddr, state: ProbeState) {
        if let Some(row) = session.peers.iter_mut().find(|row| row.addr == addr) {
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

    fn pump_messages(&mut self) -> Vec<usize> {
        let mut completed_shutdowns = Vec::new();

        while let Ok((id, msg)) = self.rx.try_recv() {
            let Some(session) = self.sessions.get_mut(&id) else { continue; };
            match msg {
                CoreMessage::Status(text) => {
                    session.status = text.clone();
                    session.logs.push(text);
                }
                CoreMessage::TorrentLoaded(meta) => {
                    session.logs.push(format!("Loaded torrent: {}", meta.name));
                    session.meta = Some(meta);
                }
                CoreMessage::MetadataReady(meta) => {
                    session.logs.push(format!("Metadata downloaded: {}", meta.name));
                    session.meta = Some((*meta).clone());
                }
                CoreMessage::GlobalProgress(progress) => {
                    session.global_progress = progress.clamp(0.0, 1.0);
                }
                CoreMessage::DownloadComplete => {
                    session.global_progress = 1.0;
                    session.status = "Download complete".to_string();
                    session.logs.push("All pieces were downloaded".to_string());
                }
                CoreMessage::ShutdownComplete => {
                    completed_shutdowns.push(id);
                }
                CoreMessage::PeerFound(addr) => session.peers.push(PeerRow {
                    addr,
                    state: ProbeState::Idle,
                    telemetry: None,
                }),
                CoreMessage::TrackerDone(count) => {
                    session.logs.push(format!("Tracker returned {count} peers"));
                }
                CoreMessage::ProbeQueued(addr) => {
                    Self::update_peer_state(session, addr, ProbeState::Queued);
                }
                CoreMessage::ProbeStarted(addr) => {
                    Self::update_peer_state(session, addr, ProbeState::Probing);
                }
                CoreMessage::ProbeSucceeded(addr, status) => {
                    Self::update_peer_state(session, addr, ProbeState::Success(status));
                }
                CoreMessage::ProbeFailed(addr, err) => {
                    Self::update_peer_state(session, addr, ProbeState::Failed(err));
                }
                CoreMessage::TelemetryUpdate(addr, telemetry) => {
                    if let Some(row) = session.peers.iter_mut().find(|row| row.addr == addr) {
                        row.telemetry = Some(telemetry);
                    }
                }
                CoreMessage::Error(err) => {
                    session.logs.push(format!("Error: {err}"));
                    session.has_error = true;
                }
            }
        }

        completed_shutdowns
    }
}

impl eframe::App for TorTorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = Color32::from_rgb(10, 15, 26);
        visuals.panel_fill = Color32::from_rgb(10, 15, 26);
        visuals.selection.bg_fill = Color32::from_rgb(0, 210, 255);
        visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(0, 255, 209));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(15, 25, 40);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(25, 40, 60);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(0, 210, 255);
        visuals.widgets.active.bg_fill = Color32::from_rgb(0, 255, 209);
        visuals.override_text_color = Some(Color32::from_rgb(230, 240, 255));
        ctx.set_visuals(visuals);
        
        let mut about_open = self.show_about;
        egui::Window::new("About TorTor")
            .open(&mut about_open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("🌀 TorTor").size(36.0).strong().color(Color32::from_rgb(0, 210, 255)));
                    ui.label(RichText::new("Version 1.6.1").size(14.0).color(Color32::from_rgb(0, 255, 209)));
                    ui.add_space(10.0);
                    ui.label(RichText::new("High-performance BitTorrent client").italics().color(Color32::LIGHT_GRAY));
                    ui.add_space(15.0);
                });
                
                ui.group(|ui| {
                    ui.label(RichText::new("🚀 Key Features:").strong().color(Color32::WHITE));
                    ui.add_space(5.0);
                    let features = [
                        "⚡ Dynamic SIMD dispatch (AVX2/SSE4.1)",
                        "📂 Multi-file torrent & Session isolation",
                        "🛡️ Memory-safe piece assembler",
                        "🔄 Zero-copy I/O with Tokio",
                        "🌐 Kademlia DHT & PEX Support",
                    ];
                    for f in features {
                        ui.label(RichText::new(f).color(Color32::from_rgb(200, 220, 255)));
                    }
                });
                
                ui.add_space(15.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Created by: mjojo <GLK Dev>").size(12.0).color(Color32::from_rgb(100, 150, 200)));
                    ui.add_space(5.0);
                });
            });
        self.show_about = about_open;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });
        });

        // Graceful shutdown on window close
        if ctx.input(|i| i.viewport().close_requested()) {
            self.quit_requested = true;
            for session in self.sessions.values_mut() {
                if !session.is_shutting_down {
                    session.is_shutting_down = true;
                    let _ = session.command_tx.send(CoreCommand::StopAll);
                }
            }
            if !self.sessions.is_empty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        let completed_shutdowns = self.pump_messages();
        for id in completed_shutdowns {
            if let Some(session) = self.sessions.remove(&id) {
                if session.remove_requested || session.delete_requested {
                    self.session_store.entries.retain(|e| e.source != session.source);
                    let _ = self.session_store.save(&std::path::PathBuf::from("session.json"));
                }
                
                if session.delete_requested {
                    if let Some(meta) = &session.meta {
                        let target_path = session.output_dir.join(&meta.name);
                        let _ = std::fs::remove_dir_all(&target_path);
                        let _ = std::fs::remove_file(&target_path);
                    }
                    if let crate::core::session_store::TorrentSource::File(path) = &session.source {
                        let _ = std::fs::remove_file(path.with_extension("fastresume"));
                        let _ = std::fs::remove_file(path.with_extension("download.part"));
                    }
                }
            }
        }

        if self.quit_requested && self.sessions.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("TorTor Download Manager");
                if ui.button(egui::RichText::new("[ Добавить файл ]").family(egui::FontFamily::Monospace)).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Torrent Files", &["torrent"])
                        .pick_file()
                    {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Select Download Directory")
                            .pick_folder()
                        {
                            let source = crate::core::session_store::TorrentSource::File(path);
                            self.session_store.entries.push(crate::core::session_store::SessionEntry {
                                source: source.clone(),
                                output_dir: dir.clone(),
                                is_paused: false,
                            });
                            let _ = self.session_store.save(&std::path::PathBuf::from("session.json"));
                            self.start_core(source, dir);
                        }
                    }
                }
                
                if ui.button(egui::RichText::new("[ Добавить ссылку ]").family(egui::FontFamily::Monospace)).clicked() {
                    self.show_link_input = !self.show_link_input;
                }
            });

            if self.show_link_input {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("> URL/Magnet:").family(egui::FontFamily::Monospace).color(egui::Color32::GREEN));
                    ui.text_edit_singleline(&mut self.link_input_buffer);
                    
                    if ui.button(egui::RichText::new("[ OK ]").family(egui::FontFamily::Monospace)).clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Select Download Directory")
                            .pick_folder()
                        {
                            let source = crate::core::session_store::TorrentSource::Magnet(self.link_input_buffer.clone());
                            self.session_store.entries.push(crate::core::session_store::SessionEntry {
                                source: source.clone(),
                                output_dir: dir.clone(),
                                is_paused: false,
                            });
                            let _ = self.session_store.save(&std::path::PathBuf::from("session.json"));
                            self.start_core(source, dir);
                            
                            self.link_input_buffer.clear();
                            self.show_link_input = false;
                        }
                    }
                });
            }
            ui.separator();

            if self.sessions.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No active downloads. Click '+ Add Torrent' to start.");
                });
                return;
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut ids: Vec<usize> = self.sessions.keys().copied().collect();
                ids.sort_unstable(); // Keep order consistent

                for id in ids {
                    let session = self.sessions.get_mut(&id).unwrap();
                    let name = session.meta.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| "Loading...".to_string());
                    
                    let bg_color = if session.global_progress >= 1.0 {
                        Color32::from_rgb(10, 70, 50) // completed: dark green-teal
                    } else if session.is_shutting_down {
                        Color32::from_rgb(70, 20, 30)
                    } else {
                        Color32::from_rgb(20, 35, 55) // downloading: dark blue slate
                    };

                    let frame = egui::Frame::none()
                        .fill(bg_color)
                        .rounding(8.0)
                        .inner_margin(12.0)
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0, 150, 200).linear_multiply(0.3)));

                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let icon = if session.expanded { "▼" } else { "▶" };
                            let ascii_bar = ascii_progress_bar(session.global_progress, 15);
                            let title = format!("{} {}  {}", icon, name, ascii_bar);
                            
                            let text_color = if session.has_error {
                                Color32::from_rgb(255, 50, 80)
                            } else if session.global_progress >= 1.0 {
                                Color32::from_rgb(0, 255, 209)
                            } else {
                                Color32::from_rgb(0, 210, 255)
                            };

                            // The clickable bar
                            let btn = ui.add_sized(
                                [ui.available_width(), 35.0],
                                egui::Button::new(RichText::new(title).size(18.0).monospace().color(text_color))
                                    .fill(Color32::TRANSPARENT)
                            );

                            if btn.clicked() {
                                session.expanded = !session.expanded;
                            }
                        });

                        if session.expanded {
                            ui.add_space(8.0);
                            let status_color = if session.global_progress >= 1.0 { Color32::from_rgb(0, 255, 209) } else { Color32::from_rgb(200, 220, 255) };
                            
                            ui.label(RichText::new(format!("📁 Path: {}", session.output_dir.display())).color(Color32::WHITE));
                            ui.label(RichText::new(format!("🔗 Status: {}", session.status)).color(status_color));
                            ui.label(RichText::new(format!("👥 Peers: {}", session.peers.len())).color(Color32::WHITE));
                            
                            if let Some(meta) = &session.meta {
                                ui.label(RichText::new(format!("📦 Pieces: {}", meta.pieces_count)).color(Color32::LIGHT_GRAY));
                            }
                            
                            ui.add_space(8.0);
                            
                            ui.horizontal(|ui| {
                                if ui.add_enabled(!session.swarm_started && !session.is_shutting_down && !session.has_error, egui::Button::new("▶ Start Swarm")).clicked() {
                                    session.swarm_started = true;
                                    let _ = session.command_tx.send(CoreCommand::StartSwarm);
                                }
                                
                                if session.swarm_started {
                                    if !session.is_paused {
                                        if ui.add_enabled(!session.is_shutting_down, egui::Button::new("⏸ Пауза")).clicked() {
                                            session.is_paused = true;
                                            let _ = session.command_tx.send(CoreCommand::Pause);
                                        }
                                    } else {
                                        if ui.add_enabled(!session.is_shutting_down, egui::Button::new("▶ Возобновить")).clicked() {
                                            session.is_paused = false;
                                            let _ = session.command_tx.send(CoreCommand::Resume);
                                        }
                                    }
                                }
                                
                                if ui.add_enabled(!session.is_shutting_down, egui::Button::new("❌ Remove")).clicked() {
                                    session.remove_requested = true;
                                    session.is_shutting_down = true;
                                    let _ = session.command_tx.send(CoreCommand::StopAll);
                                }
                                
                                if ui.add_enabled(!session.is_shutting_down, egui::Button::new("🗑 Remove + Files")).clicked() {
                                    session.remove_requested = true;
                                    session.delete_requested = true;
                                    session.is_shutting_down = true;
                                    let _ = session.command_tx.send(CoreCommand::StopAll);
                                }
                            });
                            
                            ui.separator();
                            
                            egui::ScrollArea::vertical().id_salt(id).max_height(100.0).show(ui, |ui| {
                                for row in &session.peers {
                                    ui.horizontal(|ui| {
                                        ui.monospace(row.addr.to_string());
                                        ui.label(Self::status_label(&row.state));
                                        if let Some(tel) = &row.telemetry {
                                            ui.label(format!("| In-flight: {}", tel.in_flight_requests));
                                            ui.label(format!("| Drops: {}", tel.unexpected_blocks + tel.duplicate_blocks));
                                        }
                                    });
                                }
                            });
                        }
                    });
                    ui.add_space(8.0);
                }
            });
        });

        ctx.request_repaint();
    }
}
