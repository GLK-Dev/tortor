// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! TorTor desktop dashboard powered by the Hey declarative MVU framework.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use hey_core::{Callback, Insets, Key};
use hey_i18n::{Arg, I18n};
use hey_layout::{Align, Justify, Style};
use hey_shell::{open_file, pick_folder, App, Command, WindowConfig};
use hey_style::Variant;
use hey_widgets::{Button, Checkbox, Label, TextInput, View};

use crate::core::command::{CoreCommand, CoreMessage};
use crate::core::session_store::{SessionEntry, SessionStore, TorrentSource};
use crate::net::engine::{Engine, EngineOptions};
use crate::net::portmap::PortMapStatus;

use super::backend::{
    apply_speed_limits, spawn_session, PeerRow, ProbeState, TorrentSessionState,
};
use super::i18n::{build_i18n, format_eta, format_size, format_speed};
use super::theme::{
    tortor_theme, ACCENT, CYAN, GREEN, INSPECTOR_BG, LINE, RED, SIDEBAR_BG, STATUSBAR_BG, TEXT,
    TEXT_DIM, TEXT_MUTED, WINDOW_BG,
};
use super::widgets::{
    ActionButton, DesktopBackdrop, FilterInput, InspectorTabs, ModalBackdrop, PieceMapBar,
    SectionLabel, SidebarHeader, SidebarItem, StatusBarWidget, StyledLabel, Surface, TableHeader,
    TorrentRow, TorrentRowProps, WindowTab,
};

const TAGS: [&str; 3] = ["system", "video", "data"];
const MIN_LIMIT_KIB: u64 = 4;

fn tag_ftl_key(tag: &str) -> &'static str {
    match tag {
        "system" | "Система" => "tag-system",
        "video" | "Видео" => "tag-video",
        "data" | "Данные" => "tag-data",
        _ => "tag-data",
    }
}

fn tag_matches(stored: Option<&str>, tag: &str) -> bool {
    stored.is_some_and(|s| tag_ftl_key(s) == tag_ftl_key(tag))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

    fn key(self) -> &'static str {
        match self {
            SidebarFilter::All => "cat-all",
            SidebarFilter::Downloading => "cat-downloading",
            SidebarFilter::Seeding => "cat-seeding",
            SidebarFilter::Completed => "cat-completed",
            SidebarFilter::Paused => "cat-paused",
            SidebarFilter::Error => "cat-error",
        }
    }

    fn matches(self, s: &TorrentSessionState) -> bool {
        match self {
            SidebarFilter::All => true,
            SidebarFilter::Downloading => !s.is_paused && !s.has_error && s.global_progress < 1.0,
            SidebarFilter::Seeding => {
                !s.is_paused && !s.has_error && s.global_progress >= 1.0 && s.upload_speed >= 1.0
            }
            SidebarFilter::Completed => !s.has_error && s.global_progress >= 1.0,
            SidebarFilter::Paused => s.is_paused && !s.has_error,
            SidebarFilter::Error => s.has_error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InspectorTab {
    General,
    Files,
    Peers,
    Trackers,
    Logs,
}

#[derive(Clone, Debug)]
enum ActiveModal {
    AddTorrent {
        magnet_input: String,
        picked_file: Option<PathBuf>,
        output_dir: PathBuf,
        tag: Option<String>,
        error: Option<String>,
    },
    ConfirmDelete(usize),
    SpeedLimits {
        dl_text: String,
        ul_text: String,
    },
}

#[derive(Debug)]
enum Msg {
    Tick,
    SelectCategory(SidebarFilter),
    ToggleTagFilter(&'static str),
    FilterChanged(String),
    SelectTorrent(usize),
    SelectTab(usize),
    PauseSelected,
    ResumeSelected,
    OpenAddModal,
    PickTorrentFile,
    PickedTorrentFile(Option<PathBuf>),
    PickOutputDir,
    PickedOutputDir(Option<PathBuf>),
    AddMagnetText(String),
    SetAddModalTag(Option<&'static str>),
    ConfirmAddTorrent,
    OpenDeleteModal,
    RemoveFromList(usize),
    DeleteWithFiles(usize),
    OpenLimitsModal,
    DlLimitInput(String),
    UlLimitInput(String),
    SaveLimits,
    CloseModal,
    ToggleFile(usize, usize, bool),
    SelectAllFiles(usize, bool),
    SetSessionTag(usize, Option<&'static str>),
    ToggleLanguage,
}

struct Model {
    sidebar_filter: SidebarFilter,
    tag_filter: Option<&'static str>,
    filter_text: String,
    selected_id: Option<usize>,
    active_tab: InspectorTab,
    modal: Option<ActiveModal>,
    default_output_dir: PathBuf,
    tx: Sender<(usize, CoreMessage)>,
    rx: Receiver<(usize, CoreMessage)>,
    sessions: HashMap<usize, TorrentSessionState>,
    next_id: usize,
    engine: Arc<Engine>,
    runtime: Arc<tokio::runtime::Runtime>,
    session_store: SessionStore,
    i18n: I18n,
    tick_gen: u64,
}

impl Model {
    fn start_core(
        &mut self,
        source: TorrentSource,
        output_dir: PathBuf,
        selected_files: Option<Vec<bool>>,
        tag: Option<String>,
        start_paused: bool,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        if self.selected_id.is_none() {
            self.selected_id = Some(id);
        }
        let session = spawn_session(
            id,
            self.tx.clone(),
            source,
            output_dir,
            selected_files,
            tag,
            start_paused,
            self.engine.clone(),
            self.runtime.handle().clone(),
        );
        self.sessions.insert(id, session);
    }

    fn pump_messages(&mut self) {
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
                    session.status = if paused { "Пауза" } else { "Загрузка" }.to_string();
                }
                CoreMessage::TorrentLoaded(meta) => {
                    session.total_pieces = meta.pieces_count;
                    session.logs.push(format!("[ENGINE] Загружен: {}", meta.name));
                    session.meta = Some(meta);
                }
                CoreMessage::MetadataReady(meta) => {
                    session.total_pieces = meta.pieces_count;
                    session
                        .logs
                        .push(format!("[ENGINE] Метаданные получены: {}", meta.name));
                    session.meta = Some((*meta).clone());
                }
                CoreMessage::GlobalProgress(progress) => {
                    session.global_progress = progress.clamp(0.0, 1.0);
                }
                CoreMessage::PieceMap {
                    completed,
                    total,
                    buckets,
                } => {
                    session.completed_pieces = completed;
                    session.total_pieces = total;
                    session.piece_buckets = buckets;
                }
                CoreMessage::DownloadComplete => {
                    session.global_progress = 1.0;
                    session.status = "Готово".to_string();
                    session.logs.push("[ENGINE] Загрузка завершена.".to_string());
                }
                CoreMessage::ShutdownComplete => completed_shutdowns.push(id),
                CoreMessage::PeerFound(addr) => {
                    if !session.peers.iter().any(|p| p.addr == addr) {
                        session.peers.push(PeerRow {
                            addr,
                            state: ProbeState::Idle,
                            telemetry: None,
                        });
                    }
                }
                CoreMessage::TrackerDone(count) => {
                    session
                        .logs
                        .push(format!("[TRACKER] Получено пиров: {count}"));
                }
                CoreMessage::ProbeQueued(addr) => update_peer_state(session, addr, ProbeState::Queued),
                CoreMessage::ProbeStarted(addr) => {
                    update_peer_state(session, addr, ProbeState::Probing)
                }
                CoreMessage::ProbeSucceeded(addr, st) => {
                    update_peer_state(session, addr, ProbeState::Success(st))
                }
                CoreMessage::ProbeFailed(addr, err) => {
                    update_peer_state(session, addr, ProbeState::Failed(err))
                }
                CoreMessage::TelemetryUpdate(addr, telemetry) => {
                    if let Some(row) = session.peers.iter_mut().find(|r| r.addr == addr) {
                        row.telemetry = Some(telemetry);
                    }
                }
                CoreMessage::Error(err) => {
                    session.logs.push(format!("[ERROR] {err}"));
                    session.has_error = true;
                    session.error_text = Some(err);
                }
                CoreMessage::BytesTransferred(rx_bytes, tx_bytes) => {
                    session.accumulated_rx += rx_bytes;
                    session.accumulated_tx += tx_bytes;
                    session.total_rx = session.total_rx.saturating_add(rx_bytes as u64);
                    session.total_tx = session.total_tx.saturating_add(tx_bytes as u64);
                }
            }
        }

        let now = Instant::now();
        for session in self.sessions.values_mut() {
            let elapsed = now.duration_since(session.last_speed_update).as_secs_f64();
            if elapsed >= 1.0 {
                session.download_speed = session.accumulated_rx as f64 / elapsed;
                session.upload_speed = session.accumulated_tx as f64 / elapsed;
                session.accumulated_rx = 0;
                session.accumulated_tx = 0;
                session.last_speed_update = now;
            }
        }

        for id in completed_shutdowns {
            if let Some(session) = self.sessions.remove(&id) {
                if session.remove_requested || session.delete_requested {
                    self.session_store
                        .entries
                        .retain(|e| e.source != session.source);
                    let _ = self.session_store.save_default();
                }
                if session.delete_requested {
                    if let Some(meta) = &session.meta {
                        let target_path = session.output_dir.join(&meta.name);
                        let _ = std::fs::remove_dir_all(&target_path);
                        let _ = std::fs::remove_file(&target_path);
                        let _ =
                            std::fs::remove_file(crate::core::fsutil::resume_file(&meta.info_hash));
                    }
                    if let TorrentSource::File(path) = &session.source {
                        let _ = std::fs::remove_file(path.with_extension("fastresume"));
                        let _ = std::fs::remove_file(path.with_extension("download.part"));
                    }
                }
                if self.selected_id == Some(id) {
                    let mut keys: Vec<usize> = self.sessions.keys().copied().collect();
                    keys.sort();
                    self.selected_id = keys.first().copied();
                }
            }
        }
    }
}

fn update_peer_state(session: &mut TorrentSessionState, addr: std::net::SocketAddr, state: ProbeState) {
    if let Some(row) = session.peers.iter_mut().find(|row| row.addr == addr) {
        row.state = state;
    }
}

fn schedule_tick() -> Command<Msg> {
    Command::perform(
        || {
            std::thread::sleep(Duration::from_millis(120));
            Msg::Tick
        },
        |m| m,
    )
}
fn update(m: &mut Model, msg: Msg) -> Command<Msg> {
    match msg {
        Msg::Tick => {
            m.pump_messages();
            m.tick_gen = m.tick_gen.wrapping_add(1);
            return schedule_tick();
        }
        Msg::SelectCategory(cat) => {
            m.sidebar_filter = cat;
        }
        Msg::ToggleTagFilter(tag) => {
            m.tag_filter = if m.tag_filter == Some(tag) {
                None
            } else {
                Some(tag)
            };
        }
        Msg::FilterChanged(text) => {
            m.filter_text = text;
        }
        Msg::SelectTorrent(id) => {
            m.selected_id = Some(id);
        }
        Msg::SelectTab(idx) => {
            m.active_tab = match idx {
                0 => InspectorTab::General,
                1 => InspectorTab::Files,
                2 => InspectorTab::Peers,
                3 => InspectorTab::Trackers,
                _ => InspectorTab::Logs,
            };
        }
        Msg::PauseSelected => {
            if let Some(sid) = m.selected_id {
                if let Some(s) = m.sessions.get_mut(&sid) {
                    if !s.is_shutting_down && !s.is_paused {
                        s.is_paused = true;
                        let _ = s.command_tx.send(CoreCommand::Pause);
                        let src = s.source.clone();
                        if let Some(e) = m.session_store.entries.iter_mut().find(|e| e.source == src) {
                            e.is_paused = true;
                            let _ = m.session_store.save_default();
                        }
                    }
                }
            }
        }
        Msg::ResumeSelected => {
            if let Some(sid) = m.selected_id {
                if let Some(s) = m.sessions.get_mut(&sid) {
                    if !s.is_shutting_down && (s.is_paused || s.has_error) {
                        s.is_paused = false;
                        s.has_error = false;
                        s.error_text = None;
                        if !s.swarm_started {
                            s.swarm_started = true;
                            let _ = s.command_tx.send(CoreCommand::StartSwarm);
                        } else {
                            let _ = s.command_tx.send(CoreCommand::Resume);
                        }
                        let src = s.source.clone();
                        if let Some(e) = m.session_store.entries.iter_mut().find(|e| e.source == src) {
                            e.is_paused = false;
                            let _ = m.session_store.save_default();
                        }
                    }
                }
            }
        }
        Msg::OpenAddModal => {
            m.modal = Some(ActiveModal::AddTorrent {
                magnet_input: String::new(),
                picked_file: None,
                output_dir: m.default_output_dir.clone(),
                tag: None,
                error: None,
            });
        }
        Msg::PickTorrentFile => {
            let title = m.i18n.tr("btn-pick-torrent");
            return Command::perform(
                move || open_file(&title, &[("Torrent", &["torrent"])]),
                Msg::PickedTorrentFile,
            );
        }
        Msg::PickedTorrentFile(Some(path)) => {
            if let Some(ActiveModal::AddTorrent {
                picked_file, error, ..
            }) = &mut m.modal
            {
                *picked_file = Some(path);
                *error = None;
            }
        }
        Msg::PickedTorrentFile(None) => {}
        Msg::PickOutputDir => {
            let title = m.i18n.tr("btn-folder");
            return Command::perform(move || pick_folder(&title), Msg::PickedOutputDir);
        }
        Msg::PickedOutputDir(Some(dir)) => {
            m.default_output_dir = dir.clone();
            if let Some(ActiveModal::AddTorrent { output_dir, .. }) = &mut m.modal {
                *output_dir = dir;
            }
        }
        Msg::PickedOutputDir(None) => {}
        Msg::AddMagnetText(text) => {
            if let Some(ActiveModal::AddTorrent {
                magnet_input,
                error,
                ..
            }) = &mut m.modal
            {
                *magnet_input = text;
                *error = None;
            }
        }
        Msg::SetAddModalTag(tag) => {
            if let Some(ActiveModal::AddTorrent { tag: cur, .. }) = &mut m.modal {
                *cur = tag.map(str::to_string);
            }
        }
        Msg::ConfirmAddTorrent => {
            let Some(ActiveModal::AddTorrent {
                magnet_input,
                picked_file,
                output_dir,
                tag,
                ..
            }) = m.modal.clone()
            else {
                return Command::none();
            };

            let mag = magnet_input.trim().to_string();
            let source = if !mag.is_empty() {
                if let Err(err) = crate::net::magnet::parse(&mag) {
                    let msg = m
                        .i18n
                        .tr_args("err-bad-magnet", &[("err", Arg::Str(err.to_string()))]);
                    if let Some(ActiveModal::AddTorrent { error, .. }) = &mut m.modal {
                        *error = Some(msg);
                    }
                    return Command::none();
                }
                TorrentSource::Magnet(mag)
            } else if let Some(path) = picked_file {
                TorrentSource::File(path)
            } else {
                let msg = m.i18n.tr("err-no-source");
                if let Some(ActiveModal::AddTorrent { error, .. }) = &mut m.modal {
                    *error = Some(msg);
                }
                return Command::none();
            };

            m.session_store.entries.push(SessionEntry {
                source: source.clone(),
                output_dir: output_dir.clone(),
                is_paused: false,
                selected_files: None,
                tag: tag.clone(),
            });
            let _ = m.session_store.save_default();
            m.start_core(source, output_dir, None, tag, false);
            m.modal = None;
        }
        Msg::OpenDeleteModal => {
            if let Some(sid) = m.selected_id {
                if m.sessions.get(&sid).is_some_and(|s| !s.is_shutting_down) {
                    m.modal = Some(ActiveModal::ConfirmDelete(sid));
                }
            }
        }
        Msg::RemoveFromList(sid) => {
            if let Some(s) = m.sessions.get_mut(&sid) {
                s.remove_requested = true;
                s.is_shutting_down = true;
                let _ = s.command_tx.send(CoreCommand::StopAll);
            }
            m.modal = None;
        }
        Msg::DeleteWithFiles(sid) => {
            if let Some(s) = m.sessions.get_mut(&sid) {
                s.delete_requested = true;
                s.is_shutting_down = true;
                let _ = s.command_tx.send(CoreCommand::StopAll);
            }
            m.modal = None;
        }
        Msg::OpenLimitsModal => {
            m.modal = Some(ActiveModal::SpeedLimits {
                dl_text: m.session_store.download_limit_kib.to_string(),
                ul_text: m.session_store.upload_limit_kib.to_string(),
            });
        }
        Msg::DlLimitInput(v) => {
            if let Some(ActiveModal::SpeedLimits { dl_text, .. }) = &mut m.modal {
                *dl_text = v;
            }
        }
        Msg::UlLimitInput(v) => {
            if let Some(ActiveModal::SpeedLimits { ul_text, .. }) = &mut m.modal {
                *ul_text = v;
            }
        }
        Msg::SaveLimits => {
            if let Some(ActiveModal::SpeedLimits { dl_text, ul_text }) = m.modal.take() {
                let mut dl = dl_text.trim().parse::<u64>().unwrap_or(0);
                let mut ul = ul_text.trim().parse::<u64>().unwrap_or(0);
                if dl > 0 {
                    dl = dl.max(MIN_LIMIT_KIB);
                }
                if ul > 0 {
                    ul = ul.max(MIN_LIMIT_KIB);
                }
                m.session_store.download_limit_kib = dl;
                m.session_store.upload_limit_kib = ul;
                apply_speed_limits(&m.session_store);
                let _ = m.session_store.save_default();
            }
        }
        Msg::CloseModal => {
            m.modal = None;
        }
        Msg::ToggleFile(sid, idx, checked) => {
            if let Some(s) = m.sessions.get_mut(&sid) {
                if let Some(files) = s.meta.as_ref().and_then(|mt| mt.files.as_ref()) {
                    let mut flags = s
                        .selection
                        .lock()
                        .unwrap()
                        .clone()
                        .filter(|f| f.len() == files.len())
                        .unwrap_or_else(|| vec![true; files.len()]);
                    if idx < flags.len() {
                        flags[idx] = checked;
                        if flags.iter().any(|k| *k) {
                            *s.selection.lock().unwrap() = Some(flags.clone());
                            if s.swarm_started {
                                let _ = s.command_tx.send(CoreCommand::SelectionChanged);
                            }
                            let src = s.source.clone();
                            if let Some(e) =
                                m.session_store.entries.iter_mut().find(|e| e.source == src)
                            {
                                e.selected_files = Some(flags);
                                let _ = m.session_store.save_default();
                            }
                        }
                    }
                }
            }
        }
        Msg::SelectAllFiles(sid, all) => {
            if let Some(s) = m.sessions.get_mut(&sid) {
                if let Some(files) = s.meta.as_ref().and_then(|mt| mt.files.as_ref()) {
                    if !files.is_empty() {
                        let mut flags = vec![all; files.len()];
                        if !all {
                            flags[0] = true;
                        }
                        *s.selection.lock().unwrap() = Some(flags.clone());
                        if s.swarm_started {
                            let _ = s.command_tx.send(CoreCommand::SelectionChanged);
                        }
                        let src = s.source.clone();
                        if let Some(e) =
                            m.session_store.entries.iter_mut().find(|e| e.source == src)
                        {
                            e.selected_files = Some(flags);
                            let _ = m.session_store.save_default();
                        }
                    }
                }
            }
        }
        Msg::SetSessionTag(sid, tag) => {
            if let Some(s) = m.sessions.get_mut(&sid) {
                s.tag = tag.map(str::to_string);
                let src = s.source.clone();
                if let Some(e) = m.session_store.entries.iter_mut().find(|e| e.source == src) {
                    e.tag = s.tag.clone();
                    let _ = m.session_store.save_default();
                }
            }
        }
        Msg::ToggleLanguage => {
            let next = if m.i18n.locale().starts_with("ru") {
                "en"
            } else {
                "ru"
            };
            m.i18n.set_locale(next);
            m.session_store.language = Some(next.to_string());
            let _ = m.session_store.save_default();
        }
    }
    Command::none()
}
fn row_props(i18n: &I18n, s: &TorrentSessionState, selected: bool) -> TorrentRowProps {
    let total_bytes = s
        .meta
        .as_ref()
        .and_then(|m| m.total_length)
        .unwrap_or(0);
    let size_text = if total_bytes > 0 {
        format_size(i18n, total_bytes)
    } else {
        "—".to_string()
    };

    let (status_kind, subtitle) = if s.has_error {
        let msg = s.error_text.as_deref().unwrap_or("error").to_string();
        (4u8, i18n.tr_args("st-error", &[("err", Arg::Str(msg))]))
    } else if s.is_paused {
        (3u8, i18n.tr("st-paused"))
    } else if s.global_progress >= 1.0 {
        if s.upload_speed >= 1.0 {
            (1u8, i18n.tr("st-seeding"))
        } else {
            (2u8, i18n.tr("st-done"))
        }
    } else if s.meta.is_none() {
        (0u8, i18n.tr("st-metadata"))
    } else {
        (0u8, i18n.tr("st-downloading"))
    };

    let remaining = if total_bytes > 0 {
        ((total_bytes as f64) * (1.0 - s.global_progress as f64).max(0.0)) as u64
    } else {
        0
    };
    let eta_text = format_eta(
        i18n,
        remaining,
        s.download_speed,
        s.global_progress >= 1.0,
        s.is_paused || s.has_error,
    );

    let pct = (s.global_progress * 100.0).round().clamp(0.0, 100.0) as u32;
    TorrentRowProps {
        name: s.display_name().to_string(),
        subtitle,
        status_kind,
        size_text,
        progress_milli: (s.global_progress.clamp(0.0, 1.0) * 1000.0).round() as u32,
        pct_text: format!("{pct}%"),
        down_text: format_speed(i18n, s.download_speed),
        up_text: format_speed(i18n, s.upload_speed),
        eta_text,
        selected,
    }
}

fn view_sidebar(m: &Model) -> View<Msg> {
    let mut items = vec![View::leaf("sb_header", (), SidebarHeader::add)];

    for cat in SidebarFilter::ALL {
        let count = m.sessions.values().filter(|s| cat.matches(s)).count();
        let active = m.sidebar_filter == cat && m.tag_filter.is_none();
        let label = m.i18n.tr(cat.key());
        items.push(View::leaf(
            cat.key(),
            (active, count, label.clone()),
            move |ui, p| {
                SidebarItem::add(
                    ui,
                    p,
                    &label,
                    &count.to_string(),
                    active,
                    Callback::new(move |_| Msg::SelectCategory(cat)),
                )
            },
        ));
    }

    items.push(View::container(
        "sb_tags_gap",
        Style::column().height(14.0),
        vec![],
    ));
    let tags_hdr = m.i18n.tr("sec-tags");
    items.push(View::leaf("sb_tags_hdr", tags_hdr.clone(), move |ui, p| {
        SectionLabel::add(ui, p, &tags_hdr, 16.0)
    }));
    for tag in TAGS {
        let active = m.tag_filter == Some(tag);
        let count = m
            .sessions
            .values()
            .filter(|s| tag_matches(s.tag.as_deref(), tag))
            .count();
        let right = if count > 0 {
            count.to_string()
        } else {
            String::new()
        };
        let label = m.i18n.tr(tag_ftl_key(tag));
        items.push(View::leaf(
            tag,
            (active, count, label.clone()),
            move |ui, p| {
                SidebarItem::add(
                    ui,
                    p,
                    &label,
                    &right,
                    active,
                    Callback::new(move |_| Msg::ToggleTagFilter(tag)),
                )
            },
        ));
    }

    items.push(View::container("sb_spacer", Style::spacer(), vec![]));
    let lim_hdr = m.i18n.tr("sec-limits");
    items.push(View::leaf("sb_limit_hdr", lim_hdr.clone(), move |ui, p| {
        SectionLabel::add(ui, p, &lim_hdr, 16.0)
    }));
    let dl = m.session_store.download_limit_kib;
    let ul = m.session_store.upload_limit_kib;
    let (lim_label, lim_badge) = if dl == 0 && ul == 0 {
        (m.i18n.tr("limit-speed"), m.i18n.tr("limit-off"))
    } else {
        (
            m.i18n.tr_args(
                "limit-summary",
                &[("dl", Arg::Num(dl as f64)), ("ul", Arg::Num(ul as f64))],
            ),
            m.i18n.tr("limit-on"),
        )
    };
    items.push(View::leaf(
        "sb_limit_btn",
        (lim_label.clone(), lim_badge.clone()),
        move |ui, p| {
            SidebarItem::add(
                ui,
                p,
                &lim_label,
                &lim_badge,
                false,
                Callback::new(|_| Msg::OpenLimitsModal),
            )
        },
    ));
    items.push(View::container(
        "sb_bottom_pad",
        Style::column().height(8.0),
        vec![],
    ));

    Surface::container(
        "sidebar_wrap",
        Style::column().width(210.0),
        Style::column(),
        Surface {
            fill: SIDEBAR_BG,
            border_top: None,
            border_right: Some(LINE),
            border_bottom: None,
            border_left: None,
            chamfer: 0.0,
        },
        items,
    )
}

fn view_inspector_body(m: &Model) -> Vec<View<Msg>> {
    let Some(sid) = m.selected_id else {
        let txt = m.i18n.tr("insp-empty");
        return vec![View::leaf("insp_empty", txt.clone(), move |ui, p| {
            StyledLabel::add(ui, p, &txt, 12.5, TEXT_MUTED)
        })];
    };
    let Some(s) = m.sessions.get(&sid) else {
        return vec![];
    };

    match m.active_tab {
        InspectorTab::General => {
            let left_txt = m.i18n.tr_args(
                "piece-map",
                &[
                    ("done", Arg::Num(s.completed_pieces as f64)),
                    ("total", Arg::Num(s.total_pieces as f64)),
                ],
            );
            let active_peers = s.active_peers_count();
            let total_peers = s.peers.len();
            let ratio_txt = m.i18n.format_number(s.ratio(), 2);
            let right_txt = m.i18n.tr_args(
                "swarm-stats",
                &[
                    ("active", Arg::Num(active_peers as f64)),
                    ("peers", Arg::Num(total_peers as f64)),
                    ("ratio", Arg::Str(ratio_txt)),
                ],
            );
            let buckets = s.piece_buckets.clone();
            let done_all = s.global_progress >= 1.0;

            let info_line = m.i18n.tr_args(
                "info-line",
                &[
                    ("dir", Arg::Str(s.output_dir.display().to_string())),
                    ("down", Arg::Str(format_size(&m.i18n, s.total_rx))),
                    ("up", Arg::Str(format_size(&m.i18n, s.total_tx))),
                ],
            );
            let cur_tag = s.tag.clone();

            let tag_lbl = m.i18n.tr("tag-label");
            let none_lbl = m.i18n.tr("tag-none");
            let mut tag_btns = vec![View::leaf("tag_lbl", tag_lbl.clone(), move |ui, p| {
                StyledLabel::add(ui, p, &tag_lbl, 12.0, TEXT_MUTED)
            })];
            let none_var = if cur_tag.is_none() {
                Variant::Primary
            } else {
                Variant::Secondary
            };
            tag_btns.push(View::leaf(
                "tag_none",
                (none_var as u8, none_lbl.clone()),
                move |ui, p| {
                    Button::add(
                        ui,
                        p,
                        &none_lbl,
                        none_var,
                        Callback::new(move |_| Msg::SetSessionTag(sid, None)),
                    )
                },
            ));
            for tag in TAGS {
                let var = if tag_matches(cur_tag.as_deref(), tag) {
                    Variant::Primary
                } else {
                    Variant::Secondary
                };
                let t_lbl = m.i18n.tr(tag_ftl_key(tag));
                tag_btns.push(View::leaf(
                    Key::Name(format!("set_tag_{tag}")),
                    (var as u8, t_lbl.clone()),
                    move |ui, p| {
                        Button::add(
                            ui,
                            p,
                            &t_lbl,
                            var,
                            Callback::new(move |_| Msg::SetSessionTag(sid, Some(tag))),
                        )
                    },
                ));
            }

            vec![
                View::leaf(
                    "piece_map",
                    (
                        left_txt.clone(),
                        right_txt.clone(),
                        buckets.clone(),
                        done_all,
                    ),
                    move |ui, p| PieceMapBar::add(ui, p, left_txt, right_txt, buckets, done_all),
                ),
                View::leaf("insp_info_line", info_line.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &info_line, 12.0, TEXT_MUTED)
                }),
                View::container(
                    "insp_tags_row",
                    Style::row().gap(6.0).align_items(Align::Center),
                    tag_btns,
                ),
            ]
        }
        InspectorTab::Files => {
            if let Some(meta) = &s.meta {
                if let Some(files) = &meta.files {
                    let flags = s
                        .selection
                        .lock()
                        .unwrap()
                        .clone()
                        .filter(|f| f.len() == files.len())
                        .unwrap_or_else(|| vec![true; files.len()]);
                    let all_lbl = m.i18n.tr("btn-select-all");
                    let min_lbl = m.i18n.tr("btn-first-only");
                    let mut rows = vec![View::container(
                        "files_actions",
                        Style::row().gap(8.0),
                        vec![
                            View::leaf("f_all", (sid, all_lbl.clone()), move |ui, p| {
                                Button::add(
                                    ui,
                                    p,
                                    &all_lbl,
                                    Variant::Secondary,
                                    Callback::new(move |_| Msg::SelectAllFiles(sid, true)),
                                )
                            }),
                            View::leaf("f_min", (sid, min_lbl.clone()), move |ui, p| {
                                Button::add(
                                    ui,
                                    p,
                                    &min_lbl,
                                    Variant::Secondary,
                                    Callback::new(move |_| Msg::SelectAllFiles(sid, false)),
                                )
                            }),
                        ],
                    )];
                    for (idx, file) in files.iter().enumerate() {
                        let checked = flags.get(idx).copied().unwrap_or(true);
                        let lbl = format!(
                            "{}  ({})",
                            file.path.join("/"),
                            format_size(&m.i18n, file.length)
                        );
                        rows.push(View::leaf(
                            Key::Name(format!("file_{sid}_{idx}")),
                            (checked, lbl.clone()),
                            move |ui, p| {
                                Checkbox::add(
                                    ui,
                                    p,
                                    &lbl,
                                    checked,
                                    Callback::new(move |v| Msg::ToggleFile(sid, idx, v)),
                                )
                            },
                        ));
                    }
                    rows
                } else {
                    let line = m.i18n.tr_args(
                        "single-file",
                        &[
                            ("name", Arg::Str(meta.name.clone())),
                            (
                                "size",
                                Arg::Str(format_size(&m.i18n, meta.total_length.unwrap_or(0))),
                            ),
                        ],
                    );
                    vec![View::leaf("single_file", line.clone(), move |ui, p| {
                        StyledLabel::add(ui, p, &line, 12.5, TEXT)
                    })]
                }
            } else {
                let txt = m.i18n.tr("waiting-meta");
                vec![View::leaf("files_wait", txt.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &txt, 12.5, ACCENT)
                })]
            }
        }
        InspectorTab::Peers => {
            if s.peers.is_empty() {
                let txt = m.i18n.tr("no-peers");
                return vec![View::leaf("peers_none", txt.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &txt, 12.5, TEXT_MUTED)
                })];
            }
            s.peers
                .iter()
                .take(80)
                .enumerate()
                .map(|(i, peer)| {
                    let st_txt = match &peer.state {
                        ProbeState::Idle => m.i18n.tr("peer-idle"),
                        ProbeState::Queued => m.i18n.tr("peer-queued"),
                        ProbeState::Probing => m.i18n.tr("peer-probing"),
                        ProbeState::Success(msg) => {
                            m.i18n.tr_args("peer-active", &[("msg", Arg::Str(msg.clone()))])
                        }
                        ProbeState::Failed(err) => {
                            m.i18n.tr_args("peer-failed", &[("err", Arg::Str(err.clone()))])
                        }
                    };
                    let line = if let Some(t) = &peer.telemetry {
                        m.i18n.tr_args(
                            "peer-stats",
                            &[
                                ("addr", Arg::Str(peer.addr.to_string())),
                                ("state", Arg::Str(st_txt)),
                                ("inflight", Arg::Num(t.in_flight_requests as f64)),
                                ("rx", Arg::Str(format_size(&m.i18n, t.downloaded_bytes as u64))),
                                ("retries", Arg::Num(t.retries as f64)),
                            ],
                        )
                    } else {
                        format!("{:<22}  {st_txt}", peer.addr)
                    };
                    let col = match peer.state {
                        ProbeState::Success(_) => GREEN,
                        ProbeState::Failed(_) => RED,
                        ProbeState::Probing => CYAN,
                        _ => TEXT_MUTED,
                    };
                    View::leaf(
                        Key::Name(format!("peer_{sid}_{i}")),
                        line.clone(),
                        move |ui, p| StyledLabel::add(ui, p, &line, 12.0, col),
                    )
                })
                .collect()
        }
        InspectorTab::Trackers => {
            let port = m.engine.port;
            let port_txt = m
                .i18n
                .tr_args("listen-port", &[("port", Arg::Num(port as f64))]);
            let mut rows = vec![View::leaf("tr_port", port_txt.clone(), move |ui, p| {
                StyledLabel::add(ui, p, &port_txt, 12.0, CYAN)
            })];
            if let Some(meta) = &s.meta {
                for (i, url) in meta.trackers.iter().enumerate() {
                    let u = url.clone();
                    rows.push(View::leaf(
                        Key::Name(format!("tr_url_{i}")),
                        u.clone(),
                        move |ui, p| StyledLabel::add(ui, p, &u, 12.0, TEXT),
                    ));
                }
            }
            for (i, line) in s
                .logs
                .iter()
                .filter(|l| l.contains("[TRACKER]"))
                .rev()
                .take(15)
                .enumerate()
            {
                let txt = line.clone();
                rows.push(View::leaf(
                    Key::Name(format!("tr_log_{i}")),
                    txt.clone(),
                    move |ui, p| StyledLabel::add(ui, p, &txt, 12.0, TEXT_MUTED),
                ));
            }
            rows
        }
        InspectorTab::Logs => s
            .logs
            .iter()
            .rev()
            .take(60)
            .rev()
            .enumerate()
            .map(|(i, line)| {
                let txt = line.clone();
                let col = if txt.contains("[ERROR]") {
                    RED
                } else if txt.contains("[TRACKER]") {
                    CYAN
                } else {
                    TEXT_MUTED
                };
                View::leaf(
                    Key::Name(format!("log_{sid}_{i}")),
                    txt.clone(),
                    move |ui, p| StyledLabel::add(ui, p, &txt, 12.0, col),
                )
            })
            .collect(),
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        for session in self.sessions.values_mut() {
            if !session.is_shutting_down {
                session.is_shutting_down = true;
                let _ = session.command_tx.send(CoreCommand::StopAll);
            }
        }
    }
}

fn view_modal(m: &Model, modal: &ActiveModal) -> View<Msg> {
    let dialog_children = match modal {
        ActiveModal::AddTorrent {
            magnet_input,
            picked_file,
            output_dir,
            tag,
            error,
        } => {
            let file_txt = picked_file
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| {
                    m.i18n.tr_args(
                        "file-selected",
                        &[("name", Arg::Str(n.to_string_lossy().into_owned()))],
                    )
                })
                .unwrap_or_else(|| m.i18n.tr("file-none"));
            let dir_txt = m.i18n.tr_args(
                "folder-label",
                &[("dir", Arg::Str(output_dir.display().to_string()))],
            );
            let cur_tag = tag.clone();
            let err_opt = error.clone();
            let mag_val = magnet_input.clone();

            let tag_lbl = m.i18n.tr("tag-label");
            let none_lbl = m.i18n.tr("tag-none");
            let mut tag_row = vec![View::leaf("add_tag_lbl", tag_lbl.clone(), move |ui, p| {
                StyledLabel::add(ui, p, &tag_lbl, 12.0, TEXT_MUTED)
            })];
            let none_var = if cur_tag.is_none() {
                Variant::Primary
            } else {
                Variant::Secondary
            };
            tag_row.push(View::leaf(
                "add_tag_none",
                (none_var as u8, none_lbl.clone()),
                move |ui, p| {
                    Button::add(
                        ui,
                        p,
                        &none_lbl,
                        none_var,
                        Callback::new(|_| Msg::SetAddModalTag(None)),
                    )
                },
            ));
            for t in TAGS {
                let var = if cur_tag.as_deref() == Some(t) {
                    Variant::Primary
                } else {
                    Variant::Secondary
                };
                let t_lbl = m.i18n.tr(tag_ftl_key(t));
                tag_row.push(View::leaf(
                    Key::Name(format!("add_tag_{t}")),
                    (var as u8, t_lbl.clone()),
                    move |ui, p| {
                        Button::add(
                            ui,
                            p,
                            &t_lbl,
                            var,
                            Callback::new(move |_| Msg::SetAddModalTag(Some(t))),
                        )
                    },
                ));
            }

            let title_lbl = m.i18n.tr("modal-add-title");
            let pick_t_lbl = m.i18n.tr("btn-pick-torrent");
            let mag_lbl = m.i18n.tr("or-magnet");
            let dir_btn_lbl = m.i18n.tr("btn-folder");
            let cancel_lbl = m.i18n.tr("btn-cancel");
            let ok_lbl = m.i18n.tr("btn-confirm-add");

            let mut body = vec![
                View::leaf("add_title", title_lbl.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &title_lbl, 16.0, TEXT)
                }),
                View::container(
                    "add_file_row",
                    Style::row().gap(10.0).align_items(Align::Center),
                    vec![
                        View::leaf("pick_torrent", pick_t_lbl.clone(), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &pick_t_lbl,
                                Variant::Secondary,
                                Callback::new(|_| Msg::PickTorrentFile),
                            )
                        }),
                        View::leaf("picked_lbl", file_txt.clone(), move |ui, p| {
                            StyledLabel::add(ui, p, &file_txt, 12.0, TEXT_MUTED)
                        }),
                    ],
                ),
                View::leaf("add_mag_lbl", mag_lbl.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &mag_lbl, 12.0, TEXT_MUTED)
                }),
                View::leaf("add_mag_in", mag_val.is_empty(), move |ui, p| {
                    TextInput::add(
                        ui,
                        p,
                        &mag_val,
                        440.0,
                        Callback::new(Msg::AddMagnetText),
                    )
                }),
                View::container(
                    "add_dir_row",
                    Style::row().gap(10.0).align_items(Align::Center),
                    vec![
                        View::leaf("pick_dir", dir_btn_lbl.clone(), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &dir_btn_lbl,
                                Variant::Secondary,
                                Callback::new(|_| Msg::PickOutputDir),
                            )
                        }),
                        View::leaf("dir_lbl", dir_txt.clone(), move |ui, p| {
                            StyledLabel::add(ui, p, &dir_txt, 12.0, TEXT_MUTED)
                        }),
                    ],
                ),
                View::container(
                    "add_tags_row",
                    Style::row().gap(6.0).align_items(Align::Center),
                    tag_row,
                ),
            ];
            if let Some(err) = err_opt {
                body.push(View::leaf("add_err", err.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &err, 12.0, RED)
                }));
            }
            body.push(View::container(
                "add_btns",
                Style::row().gap(8.0).justify(Justify::End),
                vec![
                    View::leaf("add_cancel", cancel_lbl.clone(), move |ui, p| {
                        Button::add(
                            ui,
                            p,
                            &cancel_lbl,
                            Variant::Secondary,
                            Callback::new(|_| Msg::CloseModal),
                        )
                    }),
                    View::leaf("add_ok", ok_lbl.clone(), move |ui, p| {
                        Button::add(
                            ui,
                            p,
                            &ok_lbl,
                            Variant::Primary,
                            Callback::new(|_| Msg::ConfirmAddTorrent),
                        )
                    }),
                ],
            ));
            body
        }
        ActiveModal::ConfirmDelete(sid) => {
            let sid = *sid;
            let name = m
                .sessions
                .get(&sid)
                .map(|s| s.display_name().to_string())
                .unwrap_or_default();
            let title_lbl = m.i18n.tr("modal-del-title");
            let msg = m
                .i18n
                .tr_args("modal-del-msg", &[("name", Arg::Str(name))]);
            let cancel_lbl = m.i18n.tr("btn-cancel");
            let list_lbl = m.i18n.tr("btn-remove-list");
            let disk_lbl = m.i18n.tr("btn-delete-disk");
            vec![
                View::leaf("del_title", title_lbl.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &title_lbl, 16.0, TEXT)
                }),
                View::leaf("del_msg", msg.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &msg, 13.0, TEXT_MUTED)
                }),
                View::container(
                    "del_btns",
                    Style::row().gap(8.0).justify(Justify::End),
                    vec![
                        View::leaf("del_cancel", cancel_lbl.clone(), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &cancel_lbl,
                                Variant::Secondary,
                                Callback::new(|_| Msg::CloseModal),
                            )
                        }),
                        View::leaf("del_list", (sid, list_lbl.clone()), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &list_lbl,
                                Variant::Secondary,
                                Callback::new(move |_| Msg::RemoveFromList(sid)),
                            )
                        }),
                        View::leaf("del_disk", (sid, disk_lbl.clone()), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &disk_lbl,
                                Variant::Danger,
                                Callback::new(move |_| Msg::DeleteWithFiles(sid)),
                            )
                        }),
                    ],
                ),
            ]
        }
        ActiveModal::SpeedLimits { dl_text, ul_text } => {
            let dl_v = dl_text.clone();
            let ul_v = ul_text.clone();
            let title_lbl = m.i18n.tr("modal-lim-title");
            let dl_lbl = m.i18n.tr("lbl-dl-lim");
            let ul_lbl = m.i18n.tr("lbl-ul-lim");
            let cancel_lbl = m.i18n.tr("btn-cancel");
            let save_lbl = m.i18n.tr("btn-save");
            vec![
                View::leaf("lim_title", title_lbl.clone(), move |ui, p| {
                    StyledLabel::add(ui, p, &title_lbl, 15.0, TEXT)
                }),
                View::container(
                    "lim_dl_row",
                    Style::row().gap(10.0).align_items(Align::Center),
                    vec![
                        View::leaf("lim_dl_lbl", dl_lbl.clone(), move |ui, p| {
                            Label::add(ui, p, &dl_lbl)
                        }),
                        View::leaf("lim_dl_in", (), move |ui, p| {
                            TextInput::add(ui, p, &dl_v, 160.0, Callback::new(Msg::DlLimitInput))
                        }),
                    ],
                ),
                View::container(
                    "lim_ul_row",
                    Style::row().gap(10.0).align_items(Align::Center),
                    vec![
                        View::leaf("lim_ul_lbl", ul_lbl.clone(), move |ui, p| {
                            Label::add(ui, p, &ul_lbl)
                        }),
                        View::leaf("lim_ul_in", (), move |ui, p| {
                            TextInput::add(ui, p, &ul_v, 160.0, Callback::new(Msg::UlLimitInput))
                        }),
                    ],
                ),
                View::container(
                    "lim_btns",
                    Style::row().gap(8.0).justify(Justify::End),
                    vec![
                        View::leaf("lim_cancel", cancel_lbl.clone(), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &cancel_lbl,
                                Variant::Secondary,
                                Callback::new(|_| Msg::CloseModal),
                            )
                        }),
                        View::leaf("lim_save", save_lbl.clone(), move |ui, p| {
                            Button::add(
                                ui,
                                p,
                                &save_lbl,
                                Variant::Primary,
                                Callback::new(|_| Msg::SaveLimits),
                            )
                        }),
                    ],
                ),
            ]
        }
    };

    let card = Surface::container(
        "modal_card",
        Style::column().width(500.0),
        Style::column().padding(Insets::all(20.0)).gap(12.0),
        Surface {
            fill: INSPECTOR_BG,
            border_top: Some(LINE),
            border_right: Some(LINE),
            border_bottom: Some(LINE),
            border_left: Some(LINE),
            chamfer: 12.0,
        },
        dialog_children,
    );
    ModalBackdrop::container("modal_backdrop", Callback::new(|_| Msg::CloseModal), card)
}

fn view(m: &Model) -> Vec<View<Msg>> {
    let sel_session = m.selected_id.and_then(|id| m.sessions.get(&id));
    let can_pause = sel_session.is_some_and(|s| !s.is_shutting_down && !s.is_paused);
    let can_resume =
        sel_session.is_some_and(|s| !s.is_shutting_down && (s.is_paused || s.has_error));
    let can_delete = sel_session.is_some_and(|s| !s.is_shutting_down);

    let add_lbl = m.i18n.tr("btn-add");
    let filter_ph = m.i18n.tr("filter-placeholder");
    let lang_code = m.i18n.tr("lang-code");
    let q = m.filter_text.clone();
    let toolbar = Surface::container(
        "toolbar",
        Style::column().height(54.0),
        Style::row()
            .padding(Insets::symmetric(18.0, 10.0))
            .gap(8.0)
            .align_items(Align::Center),
        Surface {
            fill: WINDOW_BG,
            border_top: None,
            border_right: None,
            border_bottom: Some(LINE),
            border_left: None,
            chamfer: 0.0,
        },
        vec![
            View::leaf("btn_add", add_lbl.clone(), move |ui, p| {
                ActionButton::add(
                    ui,
                    p,
                    "plus",
                    Some(&add_lbl),
                    ACCENT,
                    true,
                    Callback::new(|_| Msg::OpenAddModal),
                )
            }),
            View::leaf("btn_pause", can_pause, move |ui, p| {
                let id = ActionButton::add(
                    ui,
                    p,
                    "pause",
                    None,
                    TEXT,
                    false,
                    Callback::new(|_| Msg::PauseSelected),
                );
                ui.set_disabled(id, !can_pause);
                id
            }),
            View::leaf("btn_play", can_resume, move |ui, p| {
                let id = ActionButton::add(
                    ui,
                    p,
                    "play",
                    None,
                    TEXT,
                    false,
                    Callback::new(|_| Msg::ResumeSelected),
                );
                ui.set_disabled(id, !can_resume);
                id
            }),
            View::leaf("btn_trash", can_delete, move |ui, p| {
                let id = ActionButton::add(
                    ui,
                    p,
                    "trash",
                    None,
                    TEXT_MUTED,
                    false,
                    Callback::new(|_| Msg::OpenDeleteModal),
                );
                ui.set_disabled(id, !can_delete);
                id
            }),
            View::container("tb_spacer", Style::spacer(), vec![]),
            View::leaf("filter_input", filter_ph.clone(), move |ui, p| {
                FilterInput::add(
                    ui,
                    p,
                    &q,
                    &filter_ph,
                    220.0,
                    Callback::new(Msg::FilterChanged),
                )
            }),
            View::leaf("btn_lang", lang_code.clone(), move |ui, p| {
                Button::add(
                    ui,
                    p,
                    &lang_code,
                    Variant::Secondary,
                    Callback::new(|_| Msg::ToggleLanguage),
                )
            }),
        ],
    );

    let q_lower = m.filter_text.trim().to_ascii_lowercase();
    let mut ids: Vec<usize> = m.sessions.keys().copied().collect();
    ids.sort();
    let visible_rows: Vec<View<Msg>> = ids
        .into_iter()
        .filter(|id| {
            let s = &m.sessions[id];
            if let Some(tag) = m.tag_filter {
                if !tag_matches(s.tag.as_deref(), tag) {
                    return false;
                }
            } else if !m.sidebar_filter.matches(s) {
                return false;
            }
            if q_lower.is_empty() {
                return true;
            }
            s.display_name().to_ascii_lowercase().contains(&q_lower)
        })
        .map(|id| {
            let s = &m.sessions[&id];
            let props = row_props(&m.i18n, s, m.selected_id == Some(id));
            View::leaf(Key::Name(format!("row_{id}")), props.clone(), move |ui, p| {
                TorrentRow::add(ui, p, props, Callback::new(move |_| Msg::SelectTorrent(id)))
            })
        })
        .collect();

    let rows_list = if visible_rows.is_empty() {
        let empty_txt = m.i18n.tr("empty-list");
        vec![View::container(
            "empty_wrap",
            Style::column().padding(Insets::all(24.0)),
            vec![View::leaf("empty_lbl", empty_txt.clone(), move |ui, p| {
                StyledLabel::add(ui, p, &empty_txt, 13.0, TEXT_DIM)
            })],
        )]
    } else {
        visible_rows
    };

    let (files_cnt, peers_cnt) = match sel_session {
        Some(s) => (
            s.meta
                .as_ref()
                .and_then(|mt| mt.files.as_ref())
                .map(|f| f.len())
                .unwrap_or_else(|| usize::from(s.meta.is_some())),
            s.peers.len(),
        ),
        None => (0, 0),
    };
    let tab_labels = vec![
        m.i18n.tr("tab-general"),
        m.i18n
            .tr_args("tab-files", &[("count", Arg::Num(files_cnt as f64))]),
        m.i18n
            .tr_args("tab-peers", &[("count", Arg::Num(peers_cnt as f64))]),
        m.i18n.tr("tab-trackers"),
        m.i18n.tr("tab-logs"),
    ];
    let active_tab_idx = match m.active_tab {
        InspectorTab::General => 0,
        InspectorTab::Files => 1,
        InspectorTab::Peers => 2,
        InspectorTab::Trackers => 3,
        InspectorTab::Logs => 4,
    };
    let insp_children = view_inspector_body(m);
    let inspector = Surface::container(
        "inspector_panel",
        Style::column().height(178.0),
        Style::column(),
        Surface {
            fill: INSPECTOR_BG,
            border_top: Some(LINE),
            border_right: None,
            border_bottom: None,
            border_left: None,
            chamfer: 0.0,
        },
        vec![
            View::leaf(
                "insp_tabs",
                (tab_labels.clone(), active_tab_idx),
                move |ui, p| {
                    InspectorTabs::add(
                        ui,
                        p,
                        tab_labels,
                        active_tab_idx,
                        Callback::new(Msg::SelectTab),
                    )
                },
            ),
            View::container(
                "insp_body",
                Style::column()
                    .grow(1.0)
                    .padding(Insets::symmetric(18.0, 10.0))
                    .gap(8.0)
                    .scroll(false, true),
                insp_children,
            ),
        ],
    );

    let total_down: f64 = m.sessions.values().map(|s| s.download_speed).sum();
    let total_up: f64 = m.sessions.values().map(|s| s.upload_speed).sum();
    let down_str = format!("↓ {}", format_speed(&m.i18n, total_down));
    let up_str = format!("↑ {}", format_speed(&m.i18n, total_up));
    let (dl_lim, ul_lim) = (
        m.session_store.download_limit_kib,
        m.session_store.upload_limit_kib,
    );
    let limit_str = if dl_lim == 0 && ul_lim == 0 {
        m.i18n.tr("limit-none")
    } else {
        m.i18n.tr_args(
            "limit-status",
            &[("dl", Arg::Num(dl_lim as f64)), ("ul", Arg::Num(ul_lim as f64))],
        )
    };
    let dht_nodes = *m.engine.dht_nodes.borrow();
    let port_arg = [("port", Arg::Num(m.engine.port as f64))];
    let port_desc = match &*m.engine.port_map.borrow() {
        PortMapStatus::Mapped { .. } => m.i18n.tr_args("port-mapped", &port_arg),
        PortMapStatus::Searching => m.i18n.tr_args("port-searching", &port_arg),
        PortMapStatus::Unavailable(_) => m.i18n.tr_args("port-unavailable", &port_arg),
        PortMapStatus::Disabled => m.i18n.tr_args("port-disabled", &port_arg),
    };
    let right_status = m.i18n.tr_args(
        "dht-status",
        &[
            ("nodes", Arg::Num(dht_nodes as f64)),
            ("port", Arg::Str(port_desc)),
        ],
    );

    let status_bar = Surface::container(
        "status_bar_wrap",
        Style::column().height(32.0),
        Style::column(),
        Surface {
            fill: STATUSBAR_BG,
            border_top: Some(LINE),
            border_right: None,
            border_bottom: None,
            border_left: None,
            chamfer: 0.0,
        },
        vec![View::leaf(
            "status_widget",
            (
                down_str.clone(),
                up_str.clone(),
                limit_str.clone(),
                right_status.clone(),
            ),
            move |ui, p| {
                StatusBarWidget::add(
                    ui,
                    p,
                    down_str,
                    up_str,
                    limit_str,
                    right_status,
                    Callback::new(|_| Msg::OpenLimitsModal),
                )
            },
        )],
    );

    let cols = [
        m.i18n.tr("col-name"),
        m.i18n.tr("col-size"),
        m.i18n.tr("col-progress"),
        m.i18n.tr("col-eta"),
    ];
    let main_right = View::container(
        "main_right",
        Style::column().grow(1.0),
        vec![
            toolbar,
            View::leaf("tbl_hdr", cols.clone(), move |ui, p| {
                TableHeader::add(ui, p, cols)
            }),
            View::container(
                "tbl_scroll",
                Style::column().grow(1.0).scroll(false, true),
                rows_list,
            ),
            inspector,
            status_bar,
        ],
    );

    let window_card = Surface::container(
        "window_card",
        Style::column().grow(1.0),
        Style::row().grow(1.0),
        Surface {
            fill: WINDOW_BG,
            border_top: Some(LINE),
            border_right: Some(LINE),
            border_bottom: Some(LINE),
            border_left: Some(LINE),
            chamfer: 0.0,
        },
        vec![view_sidebar(m), main_right],
    );

    let shell_frame = DesktopBackdrop::container(
        "desktop_bg",
        Style::column().grow(1.0),
        Style::column().padding(Insets {
            left: 20.0,
            top: 14.0,
            right: 20.0,
            bottom: 16.0,
        }),
        vec![
            View::leaf("win_tab", (), |ui, p| WindowTab::add(ui, p, "Tortor")),
            window_card,
        ],
    );

    let mut stack_children = vec![shell_frame];
    if let Some(modal) = &m.modal {
        stack_children.push(view_modal(m, modal));
    }

    vec![View::container("root", Style::stack(), stack_children)]
}

pub fn run_dashboard(
    initial_torrent_path: Option<PathBuf>,
    initial_magnet: Option<String>,
    listen_port: u16,
    output_dir: PathBuf,
) -> Result<()> {
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

    let (tx, rx) = mpsc::channel();
    let session_store = SessionStore::load_default();
    apply_speed_limits(&session_store);
    let i18n = build_i18n(session_store.language.as_deref().unwrap_or("ru"));

    let mut model = Model {
        sidebar_filter: SidebarFilter::All,
        tag_filter: None,
        filter_text: String::new(),
        selected_id: None,
        active_tab: InspectorTab::General,
        modal: None,
        default_output_dir: output_dir.clone(),
        tx,
        rx,
        sessions: HashMap::new(),
        next_id: 1,
        engine: engine.clone(),
        runtime: runtime.clone(),
        session_store,
        i18n,
        tick_gen: 0,
    };

    let entries = model.session_store.entries.clone();
    for entry in entries {
        model.start_core(
            entry.source,
            entry.output_dir,
            entry.selected_files,
            entry.tag,
            entry.is_paused,
        );
    }

    if let Some(path) = initial_torrent_path {
        model.start_core(TorrentSource::File(path), output_dir.clone(), None, None, false);
    }
    if let Some(uri) = initial_magnet {
        model.start_core(TorrentSource::Magnet(uri), output_dir, None, None, false);
    }

    App::new(model)
        .update(update)
        .view(view)
        .theme(tortor_theme())
        .window(WindowConfig {
            title: "TorTor Download Manager".to_string(),
            size: (1180.0, 740.0),
            clear: WINDOW_BG,
            vsync: true,
        })
        .init(schedule_tick())
        .run()
        .map_err(|err| anyhow::anyhow!("failed to start Hey GUI: {err}"))?;

    let _ = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), engine.shutdown_graceful()).await
    });
    drop(engine);
    if let Ok(rt) = Arc::try_unwrap(runtime) {
        rt.shutdown_timeout(Duration::from_secs(1));
    }
    Ok(())
}