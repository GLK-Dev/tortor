// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use super::theme::{probe_storage_volume, tr, HeosLang, HeosPalette, HeosThemeId, StorageProbe};
use eframe::egui::{self, Color32, RichText};
use tokio::sync::{broadcast, mpsc as tokio_mpsc};
use tokio::time::Duration;

use crate::core::bencode;
use crate::core::command::{CoreCommand, CoreMessage, SessionTelemetry};
use crate::core::coordinator::{self, CoordinatorMsg};
#[cfg(not(any(target_os = "linux", target_os = "haiku")))]
use crate::core::disk::StandardDisk;
#[cfg(target_os = "haiku")]
use crate::core::disk_bfs::BfsDirectDisk;
use crate::core::disk_io::AsyncDiskIO;
#[cfg(target_os = "linux")]
use crate::core::disk_uring::UringDisk;
use crate::core::manager::TorrentManager;
use crate::core::resume::load_fastresume;
use crate::core::selection::Selection;
use crate::core::session_store::TorrentSource;
use crate::core::torrent::TorrentMeta;
use crate::net::engine::{Engine, EngineOptions};
use crate::net::portmap::PortMapStatus;
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

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Collapsible file list with checkboxes. The choice is editable until the
/// swarm starts. Returns the new flags when the user changed them.
fn file_picker(
    ui: &mut egui::Ui,
    id: usize,
    files: &[crate::core::torrent::TorrentFile],
    session: &TorrentSessionState,
) -> Option<Vec<bool>> {
    let editable = !session.is_shutting_down;
    let mut flags = session
        .selection
        .lock()
        .unwrap()
        .clone()
        .filter(|flags| flags.len() == files.len())
        .unwrap_or_else(|| vec![true; files.len()]);
    let before = flags.clone();

    egui::CollapsingHeader::new(format!(
        "🗂 Files: {} of {} selected",
        flags.iter().filter(|keep| **keep).count(),
        files.len()
    ))
    .id_salt(("torrent-files", id))
    .show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal(|ui| {
                if ui.button("All").clicked() {
                    flags.fill(true);
                }
                if ui.button("None").clicked() {
                    flags.fill(false);
                }
            });
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .show(ui, |ui| {
                    for (index, file) in files.iter().enumerate() {
                        let label =
                            format!("{}  ({})", file.path.join("/"), human_size(file.length));
                        ui.checkbox(&mut flags[index], label);
                    }
                });
        });
        if session.swarm_started {
            ui.label("Files added now are downloaded from the start; pieces shared with them are fetched again.");
        }
    });

    // At least one file must stay selected.
    if flags != before && flags.iter().any(|keep| *keep) {
        *session.selection.lock().unwrap() = Some(flags.clone());
        Some(flags)
    } else {
        None
    }
}

#[allow(dead_code)]
fn ascii_progress_bar(progress: f32, width: usize) -> String {
    let p = progress.clamp(0.0, 1.0);
    let filled = (p * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    let filled_str = "█".repeat(filled);
    let empty_str = "░".repeat(empty);
    format!("[{}{}] {}%", filled_str, empty_str, (p * 100.0) as u32)
}

pub fn run_dashboard(
    initial_torrent_path: Option<PathBuf>,
    initial_magnet: Option<String>,
    listen_port: u16,
    output_dir: PathBuf,
) -> Result<()> {
    let mut native_options = eframe::NativeOptions::default();

    // Load window icon
    let icon_data = include_bytes!("../../Images/tortor_icon.png");
    if let Ok(image) = image::load_from_memory(icon_data) {
        let image = image.into_rgba8();
        let (width, height) = image.dimensions();
        native_options.viewport =
            egui::ViewportBuilder::default().with_icon(Arc::new(egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }));
    }

    // One runtime and one network engine (listeners, DHT) serve every torrent.
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );
    let engine = runtime.block_on(Engine::start(EngineOptions {
        listen_port,
        enable_dht: true,
        enable_port_mapping: true,
    }))?;

    let app_engine = engine.clone();
    let app_runtime = runtime.clone();
    eframe::run_native(
        "TorTor Download Manager",
        native_options,
        Box::new(move |_| {
            let mut app = TorTorApp::new(app_engine, app_runtime);

            // Resume saved sessions
            let entries = app.session_store.entries.clone();
            for entry in entries {
                app.start_core(
                    entry.source,
                    entry.output_dir,
                    entry.selected_files,
                    entry.is_paused,
                );
            }

            // Start CLI-provided torrent if any
            if let Some(path) = initial_torrent_path {
                app.start_core(
                    crate::core::session_store::TorrentSource::File(path),
                    output_dir.clone(),
                    None,
                    false,
                );
            }
            if let Some(uri) = initial_magnet {
                app.start_core(
                    crate::core::session_store::TorrentSource::Magnet(uri),
                    output_dir,
                    None,
                    false,
                );
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("failed to start GUI: {err}"))?;

    let _ = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), engine.shutdown_graceful()).await
    });
    drop(engine);
    if let Ok(rt) = Arc::try_unwrap(runtime) {
        rt.shutdown_timeout(Duration::from_secs(1));
    }
    Ok(())
}

struct DoneOnDrop(mpsc::Sender<()>);

impl Drop for DoneOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

fn initial_coordinator_state(
    manager: TorrentManager,
    disk_writer: Box<dyn AsyncDiskIO>,
    requires_check: bool,
    pieces: Vec<[u8; 20]>,
    piece_length: u32,
    total_length: u64,
    start_paused: bool,
) -> coordinator::CoordinatorState {
    if requires_check && !pieces.is_empty() {
        coordinator::CoordinatorState::CheckingFiles {
            manager,
            disk_writer,
            expected_hashes: Arc::new(pieces),
            piece_length,
            total_length,
            next_piece: 0,
        }
    } else {
        coordinator::CoordinatorState::DownloadingData {
            manager,
            disk_writer,
            paused: start_paused,
            has_completed: false,
        }
    }
}

fn background_task(
    session_id: usize,
    tx: mpsc::Sender<(usize, CoreMessage)>,
    mut command_rx: tokio_mpsc::UnboundedReceiver<CoreCommand>,
    torrent_source: TorrentSource,
    engine: Arc<Engine>,
    runtime: tokio::runtime::Handle,
    output_dir: PathBuf,
    selection: Arc<Mutex<Option<Vec<bool>>>>,
) -> Result<()> {
    let listen_port = engine.port;
    let magnet = match &torrent_source {
        TorrentSource::Magnet(uri) => match crate::net::magnet::parse(uri) {
            Ok(magnet) => Some(magnet),
            Err(e) => {
                let _ = tx.send((
                    session_id,
                    CoreMessage::Error(format!("Invalid magnet link: {e}")),
                ));
                return Err(e);
            }
        },
        TorrentSource::File(_) => None,
    };

    let mut meta = match (&torrent_source, &magnet) {
        (TorrentSource::File(path), _) => match bencode::parse_torrent_file(path) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send((
                    session_id,
                    CoreMessage::Error(format!("Failed to parse torrent: {e}")),
                ));
                return Err(e.into());
            }
        },
        (_, Some(magnet)) => TorrentMeta::new(
            "",
            magnet
                .name
                .clone()
                .unwrap_or_else(|| "Magnet Download".to_string()),
            256 * 1024,
            0,
            vec![],
            None,
            None,
            magnet.info_hash,
        )
        .with_extra_trackers(magnet.trackers.clone()),
        _ => unreachable!("a magnet source always yields a parsed magnet"),
    };
    tx.send((session_id, CoreMessage::TorrentLoaded(meta.clone())))
        .ok();

    tx.send((
        session_id,
        CoreMessage::Status(
            "Ready to start. Press Start Swarm to begin allocation and download.".to_string(),
        ),
    ))
    .ok();

    // Wait for StartSwarm or StopAll
    let mut started = false;
    while let Some(command) = command_rx.blocking_recv() {
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

    let peer_id = engine.peer_id;

    if meta.pieces.is_empty() {
        // Magnet link: fetch the info dictionary from the swarm first.
        tx.send((
            session_id,
            CoreMessage::Status("Fetching metadata from peers...".to_string()),
        ))
        .ok();

        let status_tx = tx.clone();
        let options = crate::net::metadata::FetchOptions {
            info_hash: meta.info_hash,
            peer_id,
            listen_port,
            trackers: meta.trackers.clone(),
            initial_peers: Vec::new(),
            use_dht: true,
            timeout: Duration::from_secs(300),
        };
        let mut fetch = runtime.spawn(crate::net::metadata::fetch_metadata(options, move |text| {
            let _ = status_tx.send((session_id, CoreMessage::Status(text)));
        }));

        let fetched_res = runtime.block_on(async {
            loop {
                tokio::select! {
                    res = &mut fetch => break Some(res),
                    cmd = command_rx.recv() => match cmd {
                        Some(CoreCommand::StopAll) | None => {
                            fetch.abort();
                            break None;
                        }
                        Some(_) => {}
                    }
                }
            }
        });

        let Some(fetch_join) = fetched_res else {
            tx.send((session_id, CoreMessage::ShutdownComplete)).ok();
            return Ok(());
        };

        let fetched = fetch_join
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
            .and_then(|bytes| {
                bencode::parse_torrent_metadata_bytes(&bytes, meta.info_hash)
                    .map_err(anyhow::Error::from)
            });
        match fetched {
            Ok(fetched) => {
                meta = fetched.with_extra_trackers(meta.trackers.clone());
                tx.send((
                    session_id,
                    CoreMessage::MetadataReady(Arc::new(meta.clone())),
                ))
                .ok();
            }
            Err(e) => {
                tx.send((
                    session_id,
                    CoreMessage::Error(format!("Failed to fetch metadata: {e:#}")),
                ))
                .ok();
                return Err(e);
            }
        }
    }

    // Which files to download. Only meaningful for multi-file torrents; a
    // selection that covers everything is stored as `None`.
    let chosen_files: Option<Vec<bool>> = selection.lock().unwrap().clone().filter(|flags| {
        meta.files
            .as_ref()
            .is_some_and(|files| files.len() == flags.len())
            && flags.iter().any(|keep| *keep)
    });
    let file_selection = match (&meta.files, &chosen_files) {
        (Some(files), Some(flags)) => {
            Selection::from_files(files, flags, meta.piece_length, meta.pieces.len())
        }
        _ => Selection::all(meta.pieces.len()),
    };
    let chosen_files = chosen_files.filter(|_| !file_selection.is_all());

    let left = file_selection.wanted_bytes(
        meta.piece_length,
        meta.total_length
            .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64)),
    );

    let peers = if meta.trackers.is_empty() {
        tx.send((
            session_id,
            CoreMessage::Status("No trackers in torrent; relying on DHT and PEX".to_string()),
        ))
        .ok();
        Vec::new()
    } else {
        tx.send((
            session_id,
            CoreMessage::Status(format!("Announcing to {} tracker(s)", meta.trackers.len())),
        ))
        .ok();
        let params = tracker::AnnounceParams {
            info_hash: &meta.info_hash,
            peer_id: &peer_id,
            port: listen_port,
            uploaded: 0,
            downloaded: 0,
            left,
            event: Some("started"),
        };
        let announce_res = runtime.block_on(async {
            let fut = tracker::announce_all(&meta.trackers, &params, |_| {});
            tokio::pin!(fut);
            loop {
                tokio::select! {
                    res = &mut fut => break Some(res),
                    cmd = command_rx.recv() => match cmd {
                        Some(CoreCommand::StopAll) | None => break None,
                        Some(_) => {}
                    }
                }
            }
        });
        match announce_res {
            Some(Ok(peers)) => peers,
            Some(Err(e)) => {
                tx.send((
                    session_id,
                    CoreMessage::Status(format!(
                        "Tracker announce failed ({e:#}); continuing with DHT and PEX"
                    )),
                ))
                .ok();
                Vec::new()
            }
            None => {
                tx.send((session_id, CoreMessage::ShutdownComplete)).ok();
                return Ok(());
            }
        }
    };

    for peer in &peers {
        tx.send((session_id, CoreMessage::PeerFound(peer.addr)))
            .ok();
    }
    tx.send((session_id, CoreMessage::TrackerDone(peers.len())))
        .ok();
    tx.send((
        session_id,
        CoreMessage::Status("Core worker is ready for probe commands".to_string()),
    ))
    .ok();

    if meta.pieces.is_empty() {
        tx.send((
            session_id,
            CoreMessage::Status(
                "Torrent has no piece hashes; download data-path disabled".to_string(),
            ),
        ))
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

    let resume_path = crate::core::fsutil::resume_file(&meta.info_hash);
    // Older versions kept the resume file next to the torrent or the download.
    let legacy_resume_path = match &torrent_source {
        TorrentSource::File(path) => path.with_extension("fastresume"),
        TorrentSource::Magnet(_) => {
            output_dir.join(format!("{}.fastresume", hex::encode(meta.info_hash)))
        }
    };

    let mut requires_check = false;
    let mut start_paused = false;

    let loaded_resume = match runtime.block_on(load_fastresume(&resume_path)) {
        Ok(None) => runtime.block_on(load_fastresume(&legacy_resume_path)),
        other => other,
    };
    let manager = match loaded_resume {
        Ok(Some(state)) => {
            let (valid_completed, missing_any) = crate::core::disk::filter_completed_against_disk(
                &output_dir,
                &meta.name,
                total_size,
                meta.piece_length,
                meta.files.as_ref(),
                chosen_files.as_deref(),
                &state.completed,
            )
            .unwrap_or_else(|_| (Vec::new(), !state.completed.is_empty()));

            if missing_any {
                tracing::warn!(
                    "Downloaded files for {} are missing or modified on disk; pausing torrent.",
                    meta.name
                );
                let mgr = TorrentManager::with_selection(
                    meta.pieces_count,
                    &valid_completed,
                    file_selection.clone(),
                );
                if valid_completed.is_empty() {
                    let _ = std::fs::remove_file(&resume_path);
                    let _ = std::fs::remove_file(&legacy_resume_path);
                } else {
                    let cleaned = crate::core::resume::FastResumeState::from_manager(&mgr);
                    let _ = runtime.block_on(crate::core::resume::save_fastresume(
                        &resume_path,
                        &cleaned,
                    ));
                }
                tx.send((session_id, CoreMessage::GlobalProgress(mgr.progress())))
                    .ok();
                tx.send((
                    session_id,
                    CoreMessage::Status("[ERROR: Missing]".to_string()),
                ))
                .ok();
                tx.send((
                    session_id,
                    CoreMessage::Error("Files missing. Torrent paused.".to_string()),
                ))
                .ok();
                tx.send((session_id, CoreMessage::PausedState(true))).ok();
                start_paused = true;
                mgr
            } else {
                let mgr = TorrentManager::with_selection(
                    meta.pieces_count,
                    &valid_completed,
                    file_selection.clone(),
                );
                tx.send((session_id, CoreMessage::GlobalProgress(mgr.progress())))
                    .ok();
                tx.send((
                    session_id,
                    CoreMessage::Status(format!(
                        "Fast resume loaded: {} completed pieces",
                        mgr.completed_count()
                    )),
                ))
                .ok();
                mgr
            }
        }
        Ok(None) => {
            if crate::core::disk::any_selected_file_exists(
                &output_dir,
                &meta.name,
                total_size,
                meta.files.as_ref(),
                chosen_files.as_deref(),
            )
            .unwrap_or(false)
            {
                requires_check = true;
            }
            TorrentManager::with_selection(meta.pieces_count, &[], file_selection.clone())
        }
        Err(err) => {
            if crate::core::disk::any_selected_file_exists(
                &output_dir,
                &meta.name,
                total_size,
                meta.files.as_ref(),
                chosen_files.as_deref(),
            )
            .unwrap_or(false)
            {
                requires_check = true;
            }
            tx.send((
                session_id,
                CoreMessage::Status(format!("Failed to load fast resume: {err}")),
            ))
            .ok();
            TorrentManager::with_selection(meta.pieces_count, &[], file_selection.clone())
        }
    };
    let (coord_tx, coord_rx) = tokio_mpsc::channel::<CoordinatorMsg>(2048);
    // Signalled when the coordinator thread ends, i.e. after its final flush.
    let (coord_done_tx, coord_done_rx) = mpsc::channel::<()>();

    #[cfg(not(any(target_os = "linux", target_os = "haiku")))]
    {
        let output_dir_c = output_dir.clone();
        let meta_files = meta.files.clone();
        let meta_name = meta.name.clone();
        let meta_piece_length = meta.piece_length;
        let meta_pieces_c = meta.pieces.clone();
        let chosen_files_c = chosen_files.clone();

        let ui_async_tx_c = ui_async_tx.clone();
        let shutdown_rx_c = shutdown_tx.subscribe();
        let announce_tx_c = announce_tx.clone();
        let resume_path_c = resume_path.clone();

        std::thread::spawn(move || {
            let _done = DoneOnDrop(coord_done_tx);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                match StandardDisk::init_with_options(
                    &output_dir_c,
                    total_size,
                    meta_piece_length,
                    meta_files.as_ref(),
                    &meta_name,
                    chosen_files_c.as_deref(),
                    false,
                )
                .await
                {
                    Ok(disk_writer) => {
                        let disk_writer = Box::new(disk_writer) as Box<dyn AsyncDiskIO>;
                        let state = initial_coordinator_state(
                            manager,
                            disk_writer,
                            requires_check,
                            meta_pieces_c,
                            meta_piece_length,
                            total_size,
                            start_paused,
                        );
                        coordinator::run_coordinator(
                            coord_rx,
                            ui_async_tx_c,
                            state,
                            resume_path_c,
                            shutdown_rx_c,
                            announce_tx_c,
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = ui_async_tx_c
                            .send(CoreMessage::Error(format!(
                                "Failed to init StandardDisk: {}",
                                e
                            )))
                            .await;
                    }
                }
            });
        });
    }

    #[cfg(target_os = "haiku")]
    {
        let output_dir_c = output_dir.clone();
        let meta_files = meta.files.clone();
        let meta_name = meta.name.clone();
        let meta_piece_length = meta.piece_length;
        let meta_pieces_c = meta.pieces.clone();
        let chosen_files_c = chosen_files.clone();

        let ui_async_tx_c = ui_async_tx.clone();
        let shutdown_rx_c = shutdown_tx.subscribe();
        let announce_tx_c = announce_tx.clone();
        let resume_path_c = resume_path.clone();

        std::thread::spawn(move || {
            let _done = DoneOnDrop(coord_done_tx);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                match BfsDirectDisk::init_with_selection(
                    &output_dir_c,
                    total_size,
                    meta_piece_length,
                    meta_files.as_ref(),
                    &meta_name,
                    chosen_files_c.as_deref(),
                )
                .await
                {
                    Ok(disk_writer) => {
                        let disk_writer = Box::new(disk_writer) as Box<dyn AsyncDiskIO>;
                        let state = initial_coordinator_state(
                            manager,
                            disk_writer,
                            requires_check,
                            meta_pieces_c,
                            meta_piece_length,
                            total_size,
                            start_paused,
                        );
                        coordinator::run_coordinator(
                            coord_rx,
                            ui_async_tx_c,
                            state,
                            resume_path_c,
                            shutdown_rx_c,
                            announce_tx_c,
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = ui_async_tx_c
                            .send(CoreMessage::Error(format!(
                                "Failed to init BfsDirectDisk: {}",
                                e
                            )))
                            .await;
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
        let meta_pieces_c = meta.pieces.clone();
        let chosen_files_c = chosen_files.clone();

        let ui_async_tx_c = ui_async_tx.clone();
        let shutdown_rx_c = shutdown_tx.subscribe();
        let announce_tx_c = announce_tx.clone();
        let resume_path_c = resume_path.clone();

        std::thread::spawn(move || {
            let _done = DoneOnDrop(coord_done_tx);
            tokio_uring::start(async move {
                match UringDisk::init_with_selection(
                    &output_dir_c,
                    total_size,
                    meta_piece_length,
                    meta_files.as_ref(),
                    &meta_name,
                    chosen_files_c.as_deref(),
                )
                .await
                {
                    Ok(disk_writer) => {
                        let disk_writer = Box::new(disk_writer) as Box<dyn AsyncDiskIO>;
                        let state = initial_coordinator_state(
                            manager,
                            disk_writer,
                            requires_check,
                            meta_pieces_c,
                            meta_piece_length,
                            total_size,
                            start_paused,
                        );
                        coordinator::run_coordinator(
                            coord_rx,
                            ui_async_tx_c,
                            state,
                            resume_path_c,
                            shutdown_rx_c,
                            announce_tx_c,
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = ui_async_tx_c
                            .send(CoreMessage::Error(format!(
                                "Failed to init UringDisk: {}",
                                e
                            )))
                            .await;
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
    let tracker_urls = meta.trackers.clone();
    let swarm_engine = engine.clone();

    let shutdown_tx_for_swarm = shutdown_tx.clone();
    let coord_tx_for_cmd = coord_tx.clone();
    runtime.spawn(async move {
        swarm::run_swarm_manager(
            available_peers,
            tracker_urls,
            info_hash,
            left,
            expected_hashes,
            piece_length,
            total_length,
            ui_async_tx,
            coord_tx,
            shutdown_tx_for_swarm,
            announce_tx,
            swarm_engine,
        )
        .await;
    });

    // The selection the coordinator currently works with.
    let mut current_flags: Vec<bool> = chosen_files
        .clone()
        .or_else(|| meta.files.as_ref().map(|files| vec![true; files.len()]))
        .unwrap_or_default();

    while let Some(command) = command_rx.blocking_recv() {
        match command {
            CoreCommand::SelectionChanged => {
                let Some(files) = meta.files.as_ref() else {
                    continue;
                };
                let requested =
                    selection.lock().unwrap().clone().filter(|flags| {
                        flags.len() == files.len() && flags.iter().any(|keep| *keep)
                    });
                let Some(after) = requested else { continue };
                if after == current_flags {
                    continue;
                }

                let pieces = meta.pieces.len();
                let refetch = Selection::pieces_to_refetch(
                    files,
                    &current_flags,
                    &after,
                    meta.piece_length,
                    pieces,
                );
                let new_selection = Selection::from_files(files, &after, meta.piece_length, pieces);
                let _ = coord_tx_for_cmd.blocking_send(CoordinatorMsg::SetSelection {
                    selection: new_selection,
                    selected_files: after.clone(),
                    refetch,
                });
                current_flags = after;
            }
            CoreCommand::StopAll => {
                tx.send((
                    session_id,
                    CoreMessage::Status("Stop requested: shutting down workers".to_string()),
                ))
                .ok();
                let _ = shutdown_tx.send(());
                let _ = coord_done_rx.recv_timeout(Duration::from_secs(5));
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
    #[allow(dead_code)]
    id: usize,
    output_dir: PathBuf,
    source: TorrentSource,
    command_tx: tokio_mpsc::UnboundedSender<CoreCommand>,
    peers: Vec<PeerRow>,
    logs: Vec<String>,
    meta: Option<TorrentMeta>,
    global_progress: f32,
    status: String,
    is_shutting_down: bool,
    delete_requested: bool,
    swarm_started: bool,
    /// Files of a multi-file torrent to download (`None` = all); read when the swarm starts.
    selection: Arc<Mutex<Option<Vec<bool>>>>,
    is_paused: bool,
    has_error: bool,
    remove_requested: bool,
    accumulated_rx: usize,
    accumulated_tx: usize,
    download_speed: f64,
    upload_speed: f64,
    last_speed_update: std::time::Instant,
    speed_history: Vec<(f32, f32)>,
}

/// Smallest non-zero limit: a 16 KiB block must still go out within the I/O timeout.
const MIN_LIMIT_KIB: u64 = 4;

fn apply_speed_limits(store: &crate::core::session_store::SessionStore) {
    crate::net::shaper::GlobalShaper::set_limits(
        store.download_limit_kib * 1024,
        store.upload_limit_kib * 1024,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarFilter {
    All,
    Downloading,
    Seeding,
    Completed,
    Paused,
    Error,
}

impl SidebarFilter {
    const ALL: [SidebarFilter; 6] = [
        SidebarFilter::All,
        SidebarFilter::Downloading,
        SidebarFilter::Seeding,
        SidebarFilter::Completed,
        SidebarFilter::Paused,
        SidebarFilter::Error,
    ];

    fn label_key(self) -> &'static str {
        match self {
            SidebarFilter::All => "All Torrents",
            SidebarFilter::Downloading => "Downloading",
            SidebarFilter::Seeding => "Seeding",
            SidebarFilter::Completed => "Completed",
            SidebarFilter::Paused => "Paused",
            SidebarFilter::Error => "Error",
        }
    }

    fn matches(self, s: &TorrentSessionState) -> bool {
        match self {
            SidebarFilter::All => true,
            SidebarFilter::Downloading => !s.is_paused && !s.has_error && s.global_progress < 1.0,
            SidebarFilter::Seeding => !s.is_paused && s.global_progress >= 1.0,
            SidebarFilter::Completed => s.global_progress >= 1.0,
            SidebarFilter::Paused => s.is_paused,
            SidebarFilter::Error => s.has_error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorTab {
    General,
    Trackers,
    Peers,
    Files,
    SpeedGraph,
    Logs,
}

impl InspectorTab {
    const ALL: [InspectorTab; 6] = [
        InspectorTab::General,
        InspectorTab::Trackers,
        InspectorTab::Peers,
        InspectorTab::Files,
        InspectorTab::SpeedGraph,
        InspectorTab::Logs,
    ];

    fn label_key(self) -> &'static str {
        match self {
            InspectorTab::General => "General",
            InspectorTab::Trackers => "Trackers",
            InspectorTab::Peers => "Peers",
            InspectorTab::Files => "Files",
            InspectorTab::SpeedGraph => "Speed Graph",
            InspectorTab::Logs => "Logs",
        }
    }
}

struct TorTorApp {
    show_about: bool,
    show_prefs: bool,
    show_link_input: bool,
    link_input_buffer: String,
    filter_text: String,
    theme_id: HeosThemeId,
    lang: HeosLang,
    sidebar_filter: SidebarFilter,
    active_tab: InspectorTab,
    selected_id: Option<usize>,
    storage: StorageProbe,
    last_storage_probe: std::time::Instant,
    tx: Sender<(usize, CoreMessage)>,
    rx: Receiver<(usize, CoreMessage)>,
    sessions: HashMap<usize, TorrentSessionState>,
    next_id: usize,
    engine: Arc<Engine>,
    runtime: Arc<tokio::runtime::Runtime>,
    session_store: crate::core::session_store::SessionStore,
    quit_requested: bool,
    shutdown_deadline: Option<std::time::Instant>,
    limits_dirty: bool,
}

impl Drop for TorTorApp {
    fn drop(&mut self) {
        for session in self.sessions.values_mut() {
            if !session.is_shutting_down {
                session.is_shutting_down = true;
                let _ = session.command_tx.send(CoreCommand::StopAll);
            }
        }
    }
}

impl TorTorApp {
    fn new(engine: Arc<Engine>, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        let (tx, rx) = mpsc::channel();
        let session_store = crate::core::session_store::SessionStore::load_default();
        apply_speed_limits(&session_store);
        let probe_dir = session_store
            .entries
            .first()
            .map(|e| e.output_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            show_about: false,
            show_prefs: false,
            show_link_input: false,
            link_input_buffer: String::new(),
            filter_text: String::new(),
            theme_id: HeosThemeId::IndustrialDark,
            lang: HeosLang::En,
            sidebar_filter: SidebarFilter::All,
            active_tab: InspectorTab::General,
            selected_id: None,
            storage: probe_storage_volume(&probe_dir),
            last_storage_probe: std::time::Instant::now(),
            tx,
            rx,
            sessions: HashMap::new(),
            next_id: 1,
            engine,
            runtime,
            session_store,
            quit_requested: false,
            shutdown_deadline: None,
            limits_dirty: false,
        }
    }

    fn start_core(
        &mut self,
        torrent_source: TorrentSource,
        output_dir: PathBuf,
        selected_files: Option<Vec<bool>>,
        start_paused: bool,
    ) {
        let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<CoreCommand>();
        let id = self.next_id;
        self.next_id += 1;
        if self.selected_id.is_none() {
            self.selected_id = Some(id);
        }

        if !start_paused {
            let _ = cmd_tx.send(CoreCommand::StartSwarm);
        }

        let selection = Arc::new(Mutex::new(selected_files));
        let session = TorrentSessionState {
            id,
            output_dir: output_dir.clone(),
            source: torrent_source.clone(),
            command_tx: cmd_tx,
            peers: Vec::new(),
            logs: vec![format!("[ENGINE] Loading torrent: {:?}", torrent_source)],
            meta: None,
            global_progress: 0.0,
            status: format!("Starting core for {:?}", torrent_source),
            is_shutting_down: false,
            delete_requested: false,
            swarm_started: !start_paused,
            selection: selection.clone(),
            is_paused: start_paused,
            has_error: false,
            remove_requested: false,
            accumulated_rx: 0,
            accumulated_tx: 0,
            download_speed: 0.0,
            upload_speed: 0.0,
            last_speed_update: std::time::Instant::now(),
            speed_history: Vec::with_capacity(120),
        };

        self.sessions.insert(id, session);

        let tx = self.tx.clone();
        let engine = self.engine.clone();
        let runtime = self.runtime.handle().clone();

        std::thread::spawn(move || {
            if let Err(err) = background_task(
                id,
                tx.clone(),
                cmd_rx,
                torrent_source,
                engine,
                runtime,
                output_dir,
                selection,
            ) {
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
            let Some(session) = self.sessions.get_mut(&id) else {
                continue;
            };
            match msg {
                CoreMessage::Status(text) => {
                    session.status = text.clone();
                    session.logs.push(format!("[ENGINE] {text}"));
                }
                CoreMessage::PausedState(paused) => {
                    session.is_paused = paused;
                    session.status = if paused { "Paused" } else { "Resumed" }.to_string();
                }
                CoreMessage::TorrentLoaded(meta) => {
                    session.logs.push(format!("[ENGINE] Loaded torrent: {}", meta.name));
                    session.meta = Some(meta);
                }
                CoreMessage::MetadataReady(meta) => {
                    session.logs.push(format!("[ENGINE] Metadata ready: {}", meta.name));
                    session.meta = Some((*meta).clone());
                }
                CoreMessage::GlobalProgress(progress) => {
                    session.global_progress = progress.clamp(0.0, 1.0);
                }
                CoreMessage::DownloadComplete => {
                    session.global_progress = 1.0;
                    session.status = "Seeding".to_string();
                    session.logs.push("[ENGINE] Download complete; seeding.".to_string());
                }
                CoreMessage::ShutdownComplete => completed_shutdowns.push(id),
                CoreMessage::PeerFound(addr) => session.peers.push(PeerRow {
                    addr,
                    state: ProbeState::Idle,
                    telemetry: None,
                }),
                CoreMessage::TrackerDone(count) => {
                    session.logs.push(format!("[TRACKER] Returned {count} peers"));
                }
                CoreMessage::ProbeQueued(addr) => Self::update_peer_state(session, addr, ProbeState::Queued),
                CoreMessage::ProbeStarted(addr) => Self::update_peer_state(session, addr, ProbeState::Probing),
                CoreMessage::ProbeSucceeded(addr, st) => Self::update_peer_state(session, addr, ProbeState::Success(st)),
                CoreMessage::ProbeFailed(addr, err) => Self::update_peer_state(session, addr, ProbeState::Failed(err)),
                CoreMessage::TelemetryUpdate(addr, telemetry) => {
                    if let Some(row) = session.peers.iter_mut().find(|r| r.addr == addr) {
                        row.telemetry = Some(telemetry);
                    }
                }
                CoreMessage::Error(err) => {
                    session.logs.push(format!("[ERROR] {err}"));
                    session.has_error = true;
                }
                CoreMessage::BytesTransferred(rx_bytes, tx_bytes) => {
                    session.accumulated_rx += rx_bytes;
                    session.accumulated_tx += tx_bytes;
                }
            }
        }

        let now = std::time::Instant::now();
        for session in self.sessions.values_mut() {
            let elapsed = now.duration_since(session.last_speed_update).as_secs_f64();
            if elapsed >= 1.0 {
                session.download_speed = session.accumulated_rx as f64 / elapsed;
                session.upload_speed = session.accumulated_tx as f64 / elapsed;
                session.accumulated_rx = 0;
                session.accumulated_tx = 0;
                session.last_speed_update = now;
                if session.speed_history.len() >= 120 {
                    session.speed_history.remove(0);
                }
                session.speed_history.push((
                    (session.download_speed / 1024.0) as f32,
                    (session.upload_speed / 1024.0) as f32,
                ));
            }
        }
        completed_shutdowns
    }
}
impl TorTorApp {
    fn render_toolbar(&mut self, ui: &mut egui::Ui, pal: &HeosPalette) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            ui.label(RichText::new("TorTor").strong().size(15.0).color(pal.accent));
            ui.separator();

            if ui.add(egui::Button::new(RichText::new(tr(lang, "+ Torrent")).color(pal.on_accent)).fill(pal.accent)).clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Torrent", &["torrent"]).pick_file() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Select Download Directory").pick_folder() {
                        let source = TorrentSource::File(path);
                        self.session_store.entries.push(crate::core::session_store::SessionEntry {
                            source: source.clone(),
                            output_dir: dir.clone(),
                            is_paused: false,
                            selected_files: None,
                        });
                        let _ = self.session_store.save_default();
                        self.start_core(source, dir, None, false);
                    }
                }
            }

            if ui.button(tr(lang, "+ Magnet")).clicked() {
                self.show_link_input = !self.show_link_input;
            }

            let mut pause_update: Option<(TorrentSource, bool)> = None;
            if let Some(sid) = self.selected_id {
                if let Some(s) = self.sessions.get_mut(&sid) {
                    if s.is_paused {
                        if ui.add_enabled(!s.is_shutting_down, egui::Button::new(tr(lang, "Resume"))).clicked() {
                            s.is_paused = false;
                            s.has_error = false;
                            if !s.swarm_started {
                                s.swarm_started = true;
                                let _ = s.command_tx.send(CoreCommand::StartSwarm);
                            } else {
                                let _ = s.command_tx.send(CoreCommand::Resume);
                            }
                            pause_update = Some((s.source.clone(), false));
                        }
                    } else if ui.add_enabled(!s.is_shutting_down, egui::Button::new(tr(lang, "Pause"))).clicked() {
                        s.is_paused = true;
                        let _ = s.command_tx.send(CoreCommand::Pause);
                        pause_update = Some((s.source.clone(), true));
                    }
                    if ui.add_enabled(!s.is_shutting_down, egui::Button::new(tr(lang, "Remove"))).clicked() {
                        s.remove_requested = true;
                        s.is_shutting_down = true;
                        let _ = s.command_tx.send(CoreCommand::StopAll);
                    }
                    if ui.add_enabled(!s.is_shutting_down, egui::Button::new(tr(lang, "Delete"))).clicked() {
                        s.delete_requested = true;
                        s.is_shutting_down = true;
                        let _ = s.command_tx.send(CoreCommand::StopAll);
                    }
                }
            }
            if let Some((source, paused)) = pause_update {
                if let Some(entry) = self.session_store.entries.iter_mut().find(|e| e.source == source) {
                    entry.is_paused = paused;
                    let _ = self.session_store.save_default();
                }
            }

            if ui.button(tr(lang, "Prefs")).clicked() {
                self.show_prefs = !self.show_prefs;
            }
            if ui.button(tr(lang, "About")).clicked() {
                self.show_about = !self.show_about;
            }

            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.filter_text)
                    .desired_width(140.0)
                    .hint_text(tr(lang, "Filter...")),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(lang.code()).clicked() {
                    self.lang = self.lang.toggle();
                }
                if ui.button(self.theme_id.label()).clicked() {
                    self.theme_id = self.theme_id.next();
                }
            });
        });

        if self.show_link_input {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr(lang, "Magnet / URL")).color(pal.focus));
                ui.text_edit_singleline(&mut self.link_input_buffer);
                if ui.button(tr(lang, "Confirm")).clicked() && !self.link_input_buffer.trim().is_empty() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Select Download Directory").pick_folder() {
                        let source = TorrentSource::Magnet(self.link_input_buffer.trim().to_string());
                        self.session_store.entries.push(crate::core::session_store::SessionEntry {
                            source: source.clone(),
                            output_dir: dir.clone(),
                            is_paused: false,
                            selected_files: None,
                        });
                        let _ = self.session_store.save_default();
                        self.start_core(source, dir, None, false);
                        self.link_input_buffer.clear();
                        self.show_link_input = false;
                    }
                }
                if ui.button(tr(lang, "Cancel")).clicked() {
                    self.show_link_input = false;
                }
            });
        }
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui, pal: &HeosPalette) {
        let lang = self.lang;
        ui.label(RichText::new(tr(lang, "CATEGORIES")).monospace().size(11.0).color(pal.text_muted));
        ui.add_space(4.0);
        for f in SidebarFilter::ALL {
            let cnt = self.sessions.values().filter(|s| f.matches(s)).count();
            let text = format!("{} ({cnt})", tr(lang, f.label_key()));
            let active = self.sidebar_filter == f;
            let btn = egui::Button::new(RichText::new(text).color(if active { pal.accent } else { pal.text }))
                .fill(if active { pal.selection } else { Color32::TRANSPARENT })
                .min_size(egui::vec2(ui.available_width(), 24.0));
            if ui.add(btn).clicked() {
                self.sidebar_filter = f;
            }
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            let port_txt = match &*self.engine.port_map.borrow() {
                PortMapStatus::Disabled => format!("TCP :{}", self.engine.port),
                PortMapStatus::Searching => format!("TCP :{} (UPnP...)", self.engine.port),
                PortMapStatus::Mapped { .. } => format!("TCP :{} (UPnP OK)", self.engine.port),
                PortMapStatus::Unavailable(_) => format!("TCP :{} (No UPnP)", self.engine.port),
            };
            ui.label(RichText::new(port_txt).monospace().size(11.0).color(pal.text_muted));

            if self.storage.available && self.storage.total_bytes > 0 {
                let frac = (self.storage.used_bytes as f32 / self.storage.total_bytes as f32).clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(frac).text(format!("{} free", human_size(self.storage.free_bytes))));
            } else {
                ui.label(RichText::new(format!("{}: N/A", tr(lang, "Storage"))).monospace().size(11.0).color(pal.text_muted));
            }
            ui.label(RichText::new(tr(lang, "Storage")).size(11.0).color(pal.text_muted));
            ui.separator();
        });
    }

    fn render_table(&mut self, ui: &mut egui::Ui, pal: &HeosPalette) {
        let lang = self.lang;
        let q = self.filter_text.trim().to_ascii_lowercase();
        let mut ids: Vec<usize> = self.sessions.keys().copied().collect();
        ids.sort();
        let visible: Vec<usize> = ids
            .into_iter()
            .filter(|id| {
                let s = &self.sessions[id];
                if !self.sidebar_filter.matches(s) {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                let name = s.meta.as_ref().map(|m| m.name.as_str()).unwrap_or(&s.status);
                name.to_ascii_lowercase().contains(&q)
            })
            .collect();

        if visible.is_empty() {
            ui.label(RichText::new(tr(lang, "No active torrents. Use + Torrent or + Magnet.")).color(pal.text_muted));
            return;
        }

        egui::Grid::new("tortor_main_table")
            .num_columns(7)
            .striped(true)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for h in ["Name", "Size", "State", "Peers", "Down", "Up", "Progress"] {
                    ui.label(RichText::new(tr(lang, h)).monospace().size(11.0).color(pal.text_muted));
                }
                ui.end_row();

                for id in visible {
                    let s = &self.sessions[&id];
                    let selected = self.selected_id == Some(id);
                    let name = s.meta.as_ref().map(|m| m.name.as_str()).unwrap_or("Loading metadata...");
                    let total_sz = s.meta.as_ref().map(|m| human_size(m.total_length.unwrap_or(0))).unwrap_or_else(|| "--".into());
                    let state_str = if s.has_error { "Error" } else if s.is_paused { "Paused" } else if s.global_progress >= 1.0 { "Seeding" } else { "Downloading" };
                    let state_col = if s.has_error { pal.error } else if s.is_paused { pal.text_muted } else if s.global_progress >= 1.0 { pal.success } else { pal.focus };

                    if ui.selectable_label(selected, RichText::new(name).strong()).clicked() {
                        self.selected_id = Some(id);
                    }
                    ui.monospace(total_sz);
                    ui.label(RichText::new(tr(lang, state_str)).color(state_col).monospace());
                    ui.monospace(format!("{}", s.peers.len()));
                    ui.monospace(format!("{}/s", human_size(s.download_speed as u64)));
                    ui.monospace(format!("{}/s", human_size(s.upload_speed as u64)));
                    ui.add(egui::ProgressBar::new(s.global_progress).desired_width(120.0).show_percentage());
                    ui.end_row();
                }
            });
    }
}
impl TorTorApp {
    fn render_inspector(&mut self, ui: &mut egui::Ui, pal: &HeosPalette) {
        let lang = self.lang;
        ui.horizontal(|ui| {
            for tab in InspectorTab::ALL {
                let active = self.active_tab == tab;
                let btn = egui::Button::new(RichText::new(tr(lang, tab.label_key())).color(if active { pal.accent } else { pal.text }))
                    .fill(if active { pal.selection } else { pal.control });
                if ui.add(btn).clicked() {
                    self.active_tab = tab;
                }
            }
        });
        ui.separator();

        let Some(sid) = self.selected_id else {
            ui.label(RichText::new(tr(lang, "Select a torrent to inspect transfer details.")).color(pal.text_muted));
            return;
        };
        let Some(session) = self.sessions.get_mut(&sid) else {
            ui.label(RichText::new(tr(lang, "Select a torrent to inspect transfer details.")).color(pal.text_muted));
            return;
        };

        let mut selection_update: Option<(TorrentSource, Vec<bool>)> = None;
        match self.active_tab {
            InspectorTab::General => {
                ui.monospace(format!("Save Path: {}", session.output_dir.display()));
                ui.monospace(format!("Status: {}", session.status));
                if let Some(meta) = &session.meta {
                    ui.monospace(format!(
                        "Pieces: {}  |  Progress: {:.1}%  |  DL {}/s  |  UL {}/s",
                        meta.pieces_count,
                        session.global_progress * 100.0,
                        human_size(session.download_speed as u64),
                        human_size(session.upload_speed as u64),
                    ));
                }
            }
            InspectorTab::Trackers => {
                ui.monospace(format!("Listening TCP port: {}", self.engine.port));
                for line in session.logs.iter().filter(|l| l.contains("[TRACKER]")).rev().take(12) {
                    ui.monospace(RichText::new(line).color(pal.focus));
                }
            }
            InspectorTab::Peers => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for peer in &session.peers {
                        let txt = if let Some(t) = &peer.telemetry {
                            format!(
                                "{:<22} {} | inflight={} rx={} retries={} dup={}",
                                peer.addr,
                                Self::status_label(&peer.state),
                                t.in_flight_requests,
                                human_size(t.downloaded_bytes as u64),
                                t.retries,
                                t.duplicate_blocks,
                            )
                        } else {
                            format!("{:<22} {}", peer.addr, Self::status_label(&peer.state))
                        };
                        ui.monospace(txt);
                    }
                });
            }
            InspectorTab::Files => {
                if let Some(meta) = &session.meta {
                    if let Some(files) = &meta.files {
                        if let Some(flags) = file_picker(ui, sid, files, session) {
                            selection_update = Some((session.source.clone(), flags));
                            if session.swarm_started {
                                let _ = session.command_tx.send(CoreCommand::SelectionChanged);
                            }
                        }
                    } else {
                        ui.monospace(format!("Single file: {} ({})", meta.name, human_size(meta.total_length.unwrap_or(0))));
                    }
                } else {
                    ui.label(RichText::new("Waiting for torrent metadata...").color(pal.warning));
                }
            }
            InspectorTab::SpeedGraph => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 140.0), egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, pal.document);
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, pal.line));
                let max_val = session
                    .speed_history
                    .iter()
                    .flat_map(|(d, u)| [*d, *u])
                    .fold(10.0_f32, f32::max);
                let n = session.speed_history.len();
                if n > 1 {
                    let dl_pts: Vec<egui::Pos2> = session
                        .speed_history
                        .iter()
                        .enumerate()
                        .map(|(i, (d, _))| {
                            let x = rect.left() + 6.0 + (i as f32 / (n - 1) as f32) * (rect.width() - 12.0);
                            let y = rect.bottom() - 6.0 - (*d / max_val) * (rect.height() - 12.0);
                            egui::pos2(x, y)
                        })
                        .collect();
                    let ul_pts: Vec<egui::Pos2> = session
                        .speed_history
                        .iter()
                        .enumerate()
                        .map(|(i, (_, u))| {
                            let x = rect.left() + 6.0 + (i as f32 / (n - 1) as f32) * (rect.width() - 12.0);
                            let y = rect.bottom() - 6.0 - (*u / max_val) * (rect.height() - 12.0);
                            egui::pos2(x, y)
                        })
                        .collect();
                    painter.add(egui::Shape::line(dl_pts, egui::Stroke::new(2.0_f32, pal.focus)));
                    painter.add(egui::Shape::line(ul_pts, egui::Stroke::new(2.0_f32, pal.accent)));
                }
                ui.monospace(format!("Peak scale: {:.1} KiB/s", max_val));
            }
            InspectorTab::Logs => {
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for line in session.logs.iter().rev().take(80).rev() {
                        let col = if line.contains("[ERROR]") {
                            pal.error
                        } else if line.contains("[TRACKER]") {
                            pal.focus
                        } else {
                            pal.text
                        };
                        ui.label(RichText::new(line).monospace().color(col));
                    }
                });
            }
        }

        if let Some((source, flags)) = selection_update {
            if let Some(entry) = self.session_store.entries.iter_mut().find(|e| e.source == source) {
                entry.selected_files = Some(flags);
                let _ = self.session_store.save_default();
            }
        }
    }
}

impl eframe::App for TorTorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let pal = self.theme_id.palette();
        pal.apply_to_ctx(ctx);

        if self.last_storage_probe.elapsed().as_secs() >= 5 {
            let probe_dir = self
                .sessions
                .values()
                .next()
                .map(|s| s.output_dir.clone())
                .unwrap_or_else(|| PathBuf::from("."));
            self.storage = probe_storage_volume(&probe_dir);
            self.last_storage_probe = std::time::Instant::now();
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.quit_requested = true;
            self.shutdown_deadline
                .get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_secs(3));
            for session in self.sessions.values_mut() {
                if !session.is_shutting_down {
                    session.is_shutting_down = true;
                    let _ = session.command_tx.send(CoreCommand::StopAll);
                }
            }
            let deadline_reached = self
                .shutdown_deadline
                .is_some_and(|d| std::time::Instant::now() >= d);
            if !self.sessions.is_empty() && !deadline_reached {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        let completed_shutdowns = self.pump_messages();
        for id in completed_shutdowns {
            if let Some(session) = self.sessions.remove(&id) {
                if session.remove_requested || session.delete_requested {
                    self.session_store.entries.retain(|e| e.source != session.source);
                    let _ = self.session_store.save_default();
                }
                if session.delete_requested {
                    if let Some(meta) = &session.meta {
                        let target_path = session.output_dir.join(&meta.name);
                        let _ = std::fs::remove_dir_all(&target_path);
                        let _ = std::fs::remove_file(&target_path);
                        let _ = std::fs::remove_file(crate::core::fsutil::resume_file(&meta.info_hash));
                    }
                    if let TorrentSource::File(path) = &session.source {
                        let _ = std::fs::remove_file(path.with_extension("fastresume"));
                        let _ = std::fs::remove_file(path.with_extension("download.part"));
                    }
                }
                if self.selected_id == Some(id) {
                    self.selected_id = self.sessions.keys().copied().next();
                }
            }
        }
        let deadline_reached = self
            .shutdown_deadline
            .is_some_and(|d| std::time::Instant::now() >= d);
        if self.quit_requested && (self.sessions.is_empty() || deadline_reached) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let mut about_open = self.show_about;
        egui::Window::new("About TorTor").open(&mut about_open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("TorTor BitTorrent").size(22.0).strong().color(pal.accent));
            ui.monospace(concat!("Version ", env!("CARGO_PKG_VERSION")));
            ui.label("Copyright (c) 2026 Vitaly Golik (mjojo) · MIT / Apache-2.0");
        });
        self.show_about = about_open;

        let mut prefs_open = self.show_prefs;
        egui::Window::new(tr(self.lang, "Prefs")).open(&mut prefs_open).resizable(false).show(ctx, |ui| {
            ui.label(tr(self.lang, "Global Speed Limits (KiB/s, 0 = unlimited)"));
            let store = &mut self.session_store;
            let dl_changed = ui.horizontal(|ui| {
                ui.label(tr(self.lang, "Download KiB/s"));
                ui.add(egui::DragValue::new(&mut store.download_limit_kib).speed(10).range(0..=1_000_000)).changed()
            }).inner;
            let ul_changed = ui.horizontal(|ui| {
                ui.label(tr(self.lang, "Upload KiB/s"));
                ui.add(egui::DragValue::new(&mut store.upload_limit_kib).speed(10).range(0..=1_000_000)).changed()
            }).inner;
            if dl_changed || ul_changed {
                for v in [&mut store.download_limit_kib, &mut store.upload_limit_kib] {
                    if *v > 0 { *v = (*v).max(MIN_LIMIT_KIB); }
                }
                apply_speed_limits(store);
                self.limits_dirty = true;
            }
        });
        self.show_prefs = prefs_open;
        if self.limits_dirty && !ctx.input(|i| i.pointer.any_down()) {
            let _ = self.session_store.save_default();
            self.limits_dirty = false;
        }

        egui::TopBottomPanel::top("tortor_toolbar").show(ctx, |ui| {
            self.render_toolbar(ui, &pal);
        });
        egui::SidePanel::left("tortor_sidebar").exact_width(196.0).show(ctx, |ui| {
            self.render_sidebar(ui, &pal);
        });
        egui::TopBottomPanel::bottom("tortor_inspector").exact_height(216.0).show(ctx, |ui| {
            self.render_inspector(ui, &pal);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                self.render_table(ui, &pal);
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}