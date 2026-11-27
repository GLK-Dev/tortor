// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HeOS 3-theme design tokens, English-first localization, and real disk probes for TorTor UI.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeosThemeId {
    IndustrialDark,
    NightOps,
    ZetaRemastered,
}

impl HeosThemeId {
    pub const ALL: [HeosThemeId; 3] = [
        HeosThemeId::IndustrialDark,
        HeosThemeId::NightOps,
        HeosThemeId::ZetaRemastered,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HeosThemeId::IndustrialDark => "Industrial Dark",
            HeosThemeId::NightOps => "Night Ops",
            HeosThemeId::ZetaRemastered => "ZETA Remastered",
        }
    }

    pub fn next(self) -> Self {
        match self {
            HeosThemeId::IndustrialDark => HeosThemeId::NightOps,
            HeosThemeId::NightOps => HeosThemeId::ZetaRemastered,
            HeosThemeId::ZetaRemastered => HeosThemeId::IndustrialDark,
        }
    }

    pub fn palette(self) -> HeosPalette {
        match self {
            HeosThemeId::IndustrialDark => HeosPalette {
                dark_mode: true,
                desktop: Color32::from_rgb(0x0B, 0x0C, 0x10),
                panel: Color32::from_rgb(0x18, 0x1A, 0x20),
                document: Color32::from_rgb(0x14, 0x16, 0x1B),
                control: Color32::from_rgb(0x20, 0x23, 0x2B),
                control_hover: Color32::from_rgb(0x27, 0x2B, 0x34),
                line: Color32::from_rgb(0x2C, 0x30, 0x3A),
                accent: Color32::from_rgb(0xF5, 0xA6, 0x23),
                on_accent: Color32::from_rgb(0x14, 0x16, 0x1B),
                focus: Color32::from_rgb(0x00, 0xE5, 0xFF),
                text: Color32::from_rgb(0xF2, 0xF4, 0xF8),
                text_muted: Color32::from_rgb(0x9A, 0xA3, 0xB2),
                success: Color32::from_rgb(0x00, 0xFF, 0x66),
                warning: Color32::from_rgb(0xF5, 0xA6, 0x23),
                error: Color32::from_rgb(0xFF, 0x33, 0x66),
                media: Color32::from_rgb(0xB0, 0x26, 0xFF),
                selection: Color32::from_rgb(0x2A, 0x2E, 0x39),
            },
            HeosThemeId::NightOps => HeosPalette {
                dark_mode: true,
                desktop: Color32::from_rgb(0x07, 0x0A, 0x0F),
                panel: Color32::from_rgb(0x11, 0x16, 0x1E),
                document: Color32::from_rgb(0x0D, 0x11, 0x17),
                control: Color32::from_rgb(0x19, 0x21, 0x2D),
                control_hover: Color32::from_rgb(0x21, 0x2C, 0x3C),
                line: Color32::from_rgb(0x24, 0x30, 0x41),
                accent: Color32::from_rgb(0x00, 0xE5, 0xFF),
                on_accent: Color32::from_rgb(0x09, 0x0D, 0x12),
                focus: Color32::from_rgb(0x00, 0xE5, 0xFF),
                text: Color32::from_rgb(0xE8, 0xEE, 0xF5),
                text_muted: Color32::from_rgb(0x88, 0x99, 0xAE),
                success: Color32::from_rgb(0x39, 0xFF, 0x14),
                warning: Color32::from_rgb(0xFF, 0xB0, 0x00),
                error: Color32::from_rgb(0xFF, 0x45, 0x60),
                media: Color32::from_rgb(0x9D, 0x5C, 0xFF),
                selection: Color32::from_rgb(0x18, 0x2A, 0x3A),
            },
            HeosThemeId::ZetaRemastered => HeosPalette {
                dark_mode: false,
                desktop: Color32::from_rgb(0x33, 0x66, 0x99),
                panel: Color32::from_rgb(0xE8, 0xEA, 0xEE),
                document: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                control: Color32::from_rgb(0xD8, 0xDC, 0xE2),
                control_hover: Color32::from_rgb(0xE5, 0xE9, 0xEE),
                line: Color32::from_rgb(0xB4, 0xBA, 0xC4),
                accent: Color32::from_rgb(0xFF, 0x8C, 0x00),
                on_accent: Color32::from_rgb(0x14, 0x18, 0x20),
                focus: Color32::from_rgb(0x2C, 0x7B, 0xE5),
                text: Color32::from_rgb(0x1E, 0x22, 0x29),
                text_muted: Color32::from_rgb(0x55, 0x5E, 0x6B),
                success: Color32::from_rgb(0x1A, 0x99, 0x48),
                warning: Color32::from_rgb(0xD9, 0x77, 0x06),
                error: Color32::from_rgb(0xD9, 0x26, 0x44),
                media: Color32::from_rgb(0x7C, 0x3A, 0xED),
                selection: Color32::from_rgb(0xCD, 0xE0, 0xF5),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct HeosPalette {
    pub dark_mode: bool,
    pub desktop: Color32,
    pub panel: Color32,
    pub document: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub line: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub focus: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub media: Color32,
    pub selection: Color32,
}

impl HeosPalette {
    pub fn apply_to_ctx(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.window_fill = self.panel;
        visuals.panel_fill = self.panel;
        visuals.extreme_bg_color = self.document;
        visuals.faint_bg_color = self.control;
        visuals.selection.bg_fill = self.selection;
        visuals.selection.stroke = Stroke::new(1.0_f32, self.accent);
        visuals.window_stroke = Stroke::new(1.0_f32, self.line);
        visuals.window_rounding = egui::Rounding::ZERO;

        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.rounding = egui::Rounding::ZERO;
            w.bg_stroke = Stroke::new(1.0_f32, self.line);
        }
        visuals.widgets.noninteractive.bg_fill = self.panel;
        visuals.widgets.inactive.bg_fill = self.control;
        visuals.widgets.hovered.bg_fill = self.control_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.focus);
        visuals.widgets.active.bg_fill = self.accent;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.on_accent);
        visuals.override_text_color = Some(self.text);

        ctx.set_visuals(visuals);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeosLang {
    En,
    Ru,
}

impl HeosLang {
    pub fn code(self) -> &'static str {
        match self {
            HeosLang::En => "EN",
            HeosLang::Ru => "RU",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            HeosLang::En => HeosLang::Ru,
            HeosLang::Ru => HeosLang::En,
        }
    }
}

pub fn tr(lang: HeosLang, src: &'static str) -> &'static str {
    if lang == HeosLang::En {
        return src;
    }
    match src {
        "+ Torrent" => "+ Торрент",
        "+ Magnet" => "+ Магнет",
        "Resume" => "Старт",
        "Pause" => "Пауза",
        "Remove" => "Удалить",
        "Delete" => "Удалить с диска",
        "Prefs" => "Настр.",
        "About" => "О программе",
        "Filter..." => "Фильтр...",
        "CATEGORIES" => "КАТЕГОРИИ",
        "All Torrents" => "Все торренты",
        "Downloading" => "Загрузка",
        "Seeding" => "Раздача",
        "Completed" => "Завершено",
        "Paused" => "Пауза",
        "Error" => "Ошибка",
        "Storage" => "Хранилище",
        "Name" => "Имя",
        "Size" => "Размер",
        "State" => "Статус",
        "Peers" => "Пиры",
        "Down" => "Приём",
        "Up" => "Отдача",
        "Progress" => "Прогресс",
        "General" => "Общее",
        "Trackers" => "Трекеры",
        "Files" => "Файлы",
        "Speed Graph" => "График скорости",
        "Logs" => "Журнал",
        "No active torrents. Use + Torrent or + Magnet." => "Нет активных торрентов. Используйте + Торрент или + Магнет.",
        "Select a torrent to inspect transfer details." => "Выберите торрент для просмотра деталей.",
        "Global Speed Limits (KiB/s, 0 = unlimited)" => "Лимиты скорости (КиБ/с, 0 = безлимит)",
        "Download KiB/s" => "Приём КиБ/с",
        "Upload KiB/s" => "Отдача КиБ/с",
        "Cancel" => "Отмена",
        "Confirm" => "Применить",
        "Magnet / URL" => "Magnet / URL",
        _ => src,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageProbe {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub available: bool,
}

pub fn probe_storage_volume(path: &Path) -> StorageProbe {
    let probe = existing_ancestor(path);
    #[cfg(unix)]
    {
        if let Some((total, free)) = probe_unix(&probe) {
            return StorageProbe {
                total_bytes: total,
                free_bytes: free,
                used_bytes: total.saturating_sub(free),
                available: total > 0,
            };
        }
    }
    #[cfg(windows)]
    {
        if let Some((total, free)) = probe_windows(&probe) {
            return StorageProbe {
                total_bytes: total,
                free_bytes: free,
                used_bytes: total.saturating_sub(free),
                available: total > 0,
            };
        }
    }
    StorageProbe::default()
}

fn existing_ancestor(path: &Path) -> PathBuf {
    let mut cur = path.to_path_buf();
    loop {
        if cur.exists() {
            return cur;
        }
        if !cur.pop() {
            return PathBuf::from(".");
        }
    }
}

#[cfg(unix)]
fn probe_unix(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct StatVfsRaw {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        _pad: [u64; 8],
    }
    extern "C" {
        fn statvfs(path: *const i8, buf: *mut StatVfsRaw) -> i32;
    }
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = StatVfsRaw {
        f_bsize: 0,
        f_frsize: 0,
        f_blocks: 0,
        f_bfree: 0,
        f_bavail: 0,
        f_files: 0,
        f_ffree: 0,
        f_favail: 0,
        f_fsid: 0,
        f_flag: 0,
        f_namemax: 0,
        _pad: [0; 8],
    };
    if unsafe { statvfs(cpath.as_ptr(), &mut buf as *mut StatVfsRaw) } != 0 {
        return None;
    }
    let unit = if buf.f_frsize > 0 { buf.f_frsize } else { buf.f_bsize };
    Some((buf.f_blocks.saturating_mul(unit), buf.f_bavail.saturating_mul(unit)))
}

#[cfg(windows)]
fn probe_windows(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lpDirectoryName: *const u16,
            lpFreeBytesAvailableToCaller: *mut u64,
            lpTotalNumberOfBytes: *mut u64,
            lpTotalNumberOfFreeBytes: *mut u64,
        ) -> i32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return None;
    }
    Some((total, avail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_themes_cycle_and_localize() {
        assert_eq!(HeosThemeId::IndustrialDark.next(), HeosThemeId::NightOps);
        assert_eq!(HeosThemeId::NightOps.next(), HeosThemeId::ZetaRemastered);
        assert_eq!(HeosThemeId::ZetaRemastered.next(), HeosThemeId::IndustrialDark);

        assert!(HeosThemeId::IndustrialDark.palette().dark_mode);
        assert!(HeosThemeId::NightOps.palette().dark_mode);
        assert!(!HeosThemeId::ZetaRemastered.palette().dark_mode);

        assert_eq!(tr(HeosLang::En, "+ Magnet"), "+ Magnet");
        assert_eq!(tr(HeosLang::Ru, "+ Magnet"), "+ Магнет");

        let st = probe_storage_volume(Path::new("."));
        assert!(st.available && st.total_bytes > 0);
    }
}