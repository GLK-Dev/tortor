// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Torrent session lifecycle and communication with coordinator/swarm workers.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{broadcast, mpsc as tokio_mpsc};

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
use crate::core::resume::{load_fastresume, save_fastresume, FastResumeState};
use crate::core::selection::Selection;
use crate::core::session_store::{SessionStore, TorrentSource};
use crate::core::torrent::TorrentMeta;
use crate::net::engine::Engine;
use crate::net::swarm;
use crate::net::tracker;

#[derive(Debug, Clone)]
pub enum ProbeState {
    Idle,
    Queued,
    Probing,
    Success(String),
    Failed(String),
}

impl ProbeState {
    pub fn label(&self) -> String {
        match self {
            ProbeState::Idle => "ожидание".to_string(),
            ProbeState::Queued => "в очереди".to_string(),
            ProbeState::Probing => "подключение...".to_string(),
            ProbeState::Success(msg) => format!("активен: {msg}"),
            ProbeState::Failed(err) => format!("ошибка: {err}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerRow {
    pub addr: SocketAddr,
    pub state: ProbeState,
    pub telemetry: Option<SessionTelemetry>,
}

pub struct TorrentSessionState {
    pub id: usize,
    pub output_dir: PathBuf,
    pub source: TorrentSource,
    pub command_tx: tokio_mpsc::UnboundedSender<CoreCommand>,
    pub peers: Vec<PeerRow>,
    pub logs: Vec<String>,
    pub meta: Option<TorrentMeta>,
    pub global_progress: f32,
    pub status: String,
    pub is_shutting_down: bool,
    pub delete_requested: bool,
    pub swarm_started: bool,
    pub selection: Arc<Mutex<Option<Vec<bool>>>>,
    pub tag: Option<String>,
    pub is_paused: bool,
    pub has_error: bool,
    pub error_text: Option<String>,
    pub remove_requested: bool,
    pub accumulated_rx: usize,
    pub accumulated_tx: usize,
    pub total_rx: u64,
    pub total_tx: u64,
    pub download_speed: f64,
    pub upload_speed: f64,
    pub last_speed_update: Instant,
    pub completed_pieces: u32,
    pub total_pieces: u32,
    pub piece_buckets: Vec<u8>,
}

impl TorrentSessionState {
    pub fn display_name(&self) -> &str {
        if let Some(meta) = &self.meta {
            &meta.name
        } else {
            match &self.source {
                TorrentSource::File(p) => p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Загрузка..."),
                TorrentSource::Magnet(_) => "Загрузка метаданных...",
            }
        }
    }

    pub fn active_peers_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| matches!(p.state, ProbeState::Probing | ProbeState::Success(_)))
            .count()
    }

    pub fn ratio(&self) -> f64 {
        let base = self.total_rx.max(
            self.meta
                .as_ref()
                .and_then(|m| m.total_length)
                .map(|t| (t as f64 * self.global_progress as f64) as u64)
                .unwrap_or(0),
        );
        if base == 0 {
            0.0
        } else {
            self.total_tx as f64 / base as f64
        }
    }
}

pub fn apply_speed_limits(store: &SessionStore) {
    crate::net::shaper::GlobalShaper::set_limits(
        store.download_limit_kib * 1024,
        store.upload_limit_kib * 1024,
    );
}

pub fn spawn_session(
    id: usize,
    tx: Sender<(usize, CoreMessage)>,
    torrent_source: TorrentSource,
    output_dir: PathBuf,
    selected_files: Option<Vec<bool>>,
    tag: Option<String>,
    start_paused: bool,
    engine: Arc<Engine>,
    runtime: tokio::runtime::Handle,
) -> TorrentSessionState {
    let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel::<CoreCommand>();
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
        logs: vec![format!("[ENGINE] Загрузка: {:?}", torrent_source)],
        meta: None,
        global_progress: 0.0,
        status: if start_paused {
            "Пауза".to_string()
        } else {
            "Запуск...".to_string()
        },
        is_shutting_down: false,
        delete_requested: false,
        swarm_started: !start_paused,
        selection: selection.clone(),
        tag,
        is_paused: start_paused,
        has_error: false,
        error_text: None,
        remove_requested: false,
        accumulated_rx: 0,
        accumulated_tx: 0,
        total_rx: 0,
        total_tx: 0,
        download_speed: 0.0,
        upload_speed: 0.0,
        last_speed_update: Instant::now(),
        completed_pieces: 0,
        total_pieces: 0,
        piece_buckets: Vec::new(),
    };

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

    session
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

#[allow(clippy::too_many_arguments)]
fn background_task(
    session_id: usize,
    tx: Sender<(usize, CoreMessage)>,
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
                    CoreMessage::Error(format!("Некорректная magnet-ссылка: {e}")),
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
                    CoreMessage::Error(format!("Не удалось прочитать .torrent: {e}")),
                ));
                return Err(e.into());
            }
        },
        (_, Some(magnet)) => TorrentMeta::new(
            "",
            magnet
                .name
                .clone()
                .unwrap_or_else(|| "Magnet-загрузка".to_string()),
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
        tx.send((
            session_id,
            CoreMessage::Status("Поиск метаданных в сети...".to_string()),
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
                    CoreMessage::Error(format!("Ошибка загрузки метаданных: {e:#}")),
                ))
                .ok();
                return Err(e);
            }
        }
    }

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

    let total_size = meta
        .total_length
        .unwrap_or((meta.piece_length as u64) * (meta.pieces_count as u64));
    let left = file_selection.wanted_bytes(meta.piece_length, total_size);

    let peers = if meta.trackers.is_empty() {
        tx.send((
            session_id,
            CoreMessage::Status("Без трекеров: поиск через DHT и PEX".to_string()),
        ))
        .ok();
        Vec::new()
    } else {
        tx.send((
            session_id,
            CoreMessage::Status(format!("Запрос к трекерам ({})", meta.trackers.len())),
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
                        "Трекер недоступен ({e:#}); продолжаем через DHT и PEX"
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

    if meta.pieces.is_empty() {
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

    let resume_path = crate::core::fsutil::resume_file(&meta.info_hash);
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
                let mgr = TorrentManager::with_selection(
                    meta.pieces_count,
                    &valid_completed,
                    file_selection.clone(),
                );
                if valid_completed.is_empty() {
                    let _ = std::fs::remove_file(&resume_path);
                    let _ = std::fs::remove_file(&legacy_resume_path);
                } else {
                    let cleaned = FastResumeState::from_manager(&mgr);
                    let _ = runtime.block_on(save_fastresume(&resume_path, &cleaned));
                }
                tx.send((session_id, CoreMessage::GlobalProgress(mgr.progress())))
                    .ok();
                tx.send((
                    session_id,
                    CoreMessage::Error("файлы не найдены на диске".to_string()),
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
                        "Восстановлено {} из {} частей",
                        mgr.completed_count(),
                        meta.pieces_count
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
                CoreMessage::Status(format!("Ошибка чтения fastresume: {err}")),
            ))
            .ok();
            TorrentManager::with_selection(meta.pieces_count, &[], file_selection.clone())
        }
    };

    let (coord_tx, coord_rx) = tokio_mpsc::channel::<CoordinatorMsg>(2048);
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
                            .send(CoreMessage::Error(format!("Ошибка диска: {e}")))
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
                            .send(CoreMessage::Error(format!("Ошибка BFS: {e}")))
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
                            .send(CoreMessage::Error(format!("Ошибка io_uring: {e}")))
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
    let swarm_engine = engine;
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
                let requested = selection.lock().unwrap().clone().filter(|flags| {
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
                let _ = shutdown_tx.send(());
                let _ = coord_done_rx.recv_timeout(Duration::from_secs(3));
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