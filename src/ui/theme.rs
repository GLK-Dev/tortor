// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! HeOS Industrial Dark palette tokens and Russian formatting helpers for TorTor.

use hey_core::Color;
use hey_style::Theme;

pub const WINDOW_BG: Color = Color::hex(0x14161B);
pub const SIDEBAR_BG: Color = Color::hex(0x101216);
pub const INSPECTOR_BG: Color = Color::hex(0x181A20);
pub const STATUSBAR_BG: Color = Color::hex(0x101216);
pub const CONTROL_BG: Color = Color::hex(0x20232B);
pub const CONTROL_HOVER: Color = Color::hex(0x272B34);
pub const ROW_SELECTED: Color = Color::hex(0x20232B);
pub const ROW_HOVER: Color = Color::hex(0x1A1D24);
pub const INPUT_BG: Color = Color::hex(0x0E1014);
pub const LINE: Color = Color::hex(0x2C303A);
pub const ROW_DIVIDER: Color = Color::hex(0x1E222A);

pub const ACCENT: Color = Color::hex(0xF5A623);
pub const ACCENT_DARK: Color = Color::hex(0xB26A00);
pub const CYAN: Color = Color::hex(0x00E5FF);
pub const GREEN: Color = Color::hex(0x00FF66);
pub const RED: Color = Color::hex(0xFF3366);
pub const TEXT: Color = Color::hex(0xF2F4F8);
pub const TEXT_MUTED: Color = Color::hex(0x9AA3B2);
pub const TEXT_DIM: Color = Color::hex(0x5E6676);
pub const BAR_TRACK: Color = Color::hex(0x2A2E38);

pub fn tortor_theme() -> Theme {
    Theme::dark()
}

/// Formats a byte count with Russian units and decimal comma (e.g. `2,1 ГБ`, `640 МБ`).
pub fn human_size_ru(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} Б")
    } else if value >= 100.0 || ((value * 10.0).round() as u64).is_multiple_of(10) {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// Formats bytes/sec speed (e.g. `3,8 МБ/с`, `612 КБ/с`).
pub fn human_speed_ru(bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        return "—".to_string();
    }
    format!("{}/с", human_size_ru(bytes_per_sec as u64))
}

/// Formats estimated time remaining (e.g. `3 мин`, `7 ч`, `∞`, `—`).
pub fn human_eta_ru(
    remaining_bytes: u64,
    speed_bps: f64,
    is_done: bool,
    is_paused: bool,
) -> String {
    if is_done && !is_paused {
        return "∞".to_string();
    }
    if is_paused || speed_bps < 1.0 || remaining_bytes == 0 {
        return "—".to_string();
    }
    let secs = (remaining_bytes as f64 / speed_bps).ceil() as u64;
    if secs < 60 {
        format!("{secs} с")
    } else if secs < 3600 {
        format!("{} мин", secs / 60)
    } else if secs < 86400 {
        format!("{} ч", secs / 3600)
    } else {
        format!("{} д", secs / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_russian_sizes_and_eta() {
        assert_eq!(human_size_ru(640 * 1024 * 1024), "640 МБ");
        assert_eq!(human_speed_ru(0.0), "—");
        assert_eq!(human_eta_ru(0, 0.0, true, false), "∞");
        assert_eq!(human_eta_ru(1024, 0.0, false, false), "—");
    }
}
