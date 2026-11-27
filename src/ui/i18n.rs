// Copyright (c) Vitaly Golik (mjojo)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fluent-based localization (`hey-i18n`) for English and Russian UI.

use hey_i18n::{Arg, I18n};

const EN_FTL: &str = r#"
cat-all = All
cat-downloading = Downloading
cat-seeding = Seeding
cat-completed = Completed
cat-paused = Paused
cat-error = Error

sec-tags = TAGS
sec-limits = LIMITS
tag-system = System
tag-video = Video
tag-data = Data
tag-none = None
tag-label = Tag:

limit-speed = Speed
limit-on = on
limit-off = off
limit-none = unlimited
limit-summary = ↓ { $dl } · ↑ { $ul } KiB/s
limit-status = limit ↓ { $dl } · ↑ { $ul } KiB/s

btn-add = Add
filter-placeholder = Filter
lang-code = EN

col-name = NAME
col-size = SIZE
col-progress = PROGRESS
col-eta = LEFT

st-seeding = Seeding
st-done = Done
st-downloading = Downloading
st-metadata = Fetching metadata
st-paused = Paused
st-error = Error: { $err }

tab-general = General
tab-files = Files · { $count }
tab-peers = Peers · { $count }
tab-trackers = Trackers
tab-logs = Logs

empty-list = No torrents to show. Click "+ Add".
insp-empty = Select a torrent above to inspect details.
piece-map = Piece map · { $done } of { $total }
swarm-stats = Active { $active } · peers { $peers } · ratio { $ratio }
info-line = Folder: { $dir }  ·  Down: { $down }  ·  Up: { $up }

btn-select-all = Select all
btn-first-only = First only
single-file = Single file: { $name }  ({ $size })
waiting-meta = Waiting for metadata...
no-peers = No peers found yet.
listen-port = Listening port (TCP + QUIC): { $port }
peer-stats = { $addr }  { $state }  ·  inflight: { $inflight }  ·  rx: { $rx }  ·  retries: { $retries }

peer-idle = idle
peer-queued = queued
peer-probing = probing...
peer-active = active: { $msg }
peer-failed = failed: { $err }

dht-status = DHT: { $nodes } nodes · { $port }
port-mapped = port { $port } open
port-searching = port { $port } (UPnP...)
port-unavailable = port { $port } (no UPnP)
port-disabled = port { $port }

modal-add-title = Add torrent or magnet link
btn-pick-torrent = Choose .torrent...
file-selected = File: { $name }
file-none = No .torrent file selected
or-magnet = Or paste a magnet link:
btn-folder = Folder...
folder-label = Folder: { $dir }
err-bad-magnet = Invalid magnet link: { $err }
err-no-source = Choose a .torrent file or paste a magnet link
btn-cancel = Cancel
btn-confirm-add = Add

modal-del-title = Remove torrent
modal-del-msg = Remove "{ $name }"?
btn-remove-list = Remove from list
btn-delete-disk = Delete with files

modal-lim-title = Global speed limits (KiB/s, 0 = unlimited)
lbl-dl-lim = Download (↓):
lbl-ul-lim = Upload (↑):
btn-save = Save

modal-about-title = About TorTor
about-version = Version { $ver }
about-desc = High-performance BitTorrent client written in Rust with Hey GUI. Supports TCP, QUIC, IPv6, DHT, PEX, UPnP, and selective file downloads.
about-author = Author: Vitaly Golik (mjojo / GLK Dev)
about-license = License: MIT OR Apache-2.0
about-repo = Repository: https://github.com/GLK-Dev/tortor
btn-close = Close

unit-b = B
unit-kb = KB
unit-mb = MB
unit-gb = GB
unit-tb = TB
eta-sec = { $n }s
eta-min = { $n }m
eta-hour = { $n }h
eta-day = { $n }d
"#;

const RU_FTL: &str = r#"
cat-all = Все
cat-downloading = Загружаются
cat-seeding = Раздаются
cat-completed = Готово
cat-paused = На паузе
cat-error = С ошибкой

sec-tags = МЕТКИ
sec-limits = ЛИМИТ
tag-system = Система
tag-video = Видео
tag-data = Данные
tag-none = Нет
tag-label = Метка:

limit-speed = Скорость
limit-on = вкл
limit-off = выкл
limit-none = без лимита
limit-summary = ↓{ $dl } ↑{ $ul } КБ/с
limit-status = лимит ↓ { $dl } · ↑ { $ul } КБ/с

btn-add = Добавить
filter-placeholder = Фильтр
lang-code = RU

col-name = ИМЯ
col-size = РАЗМЕР
col-progress = ГОТОВО
col-eta = ОСТАЛОСЬ

st-seeding = Раздаётся
st-done = Готово
st-downloading = Загрузка
st-metadata = Загрузка метаданных
st-paused = Пауза
st-error = Ошибка: { $err }

tab-general = Общее
tab-files = Файлы · { $count }
tab-peers = Пиры · { $count }
tab-trackers = Трекеры
tab-logs = Логи

empty-list = Нет торрентов для отображения. Нажмите «+ Добавить».
insp-empty = Выберите торрент в таблице выше, чтобы увидеть детали.
piece-map = Карта частей · { $done } из { $total }
swarm-stats = Активные { $active } · пиры { $peers } · рейтинг { $ratio }
info-line = Папка: { $dir }  ·  Скачано: { $down }  ·  Отдано: { $up }

btn-select-all = Выбрать все
btn-first-only = Только первый
single-file = Один файл: { $name }  ({ $size })
waiting-meta = Ожидание метаданных...
no-peers = Пиры пока не найдены.
listen-port = Порт входящих соединений (TCP + QUIC): { $port }
peer-stats = { $addr }  { $state }  ·  в работе: { $inflight }  ·  получено: { $rx }  ·  повторы: { $retries }

peer-idle = ожидание
peer-queued = в очереди
peer-probing = подключение...
peer-active = активен: { $msg }
peer-failed = ошибка: { $err }

dht-status = DHT: { $nodes } узлов · { $port }
port-mapped = порт { $port } открыт
port-searching = порт { $port } (UPnP...)
port-unavailable = порт { $port } (без UPnP)
port-disabled = порт { $port }

modal-add-title = Добавить торрент или magnet-ссылку
btn-pick-torrent = Выбрать .torrent...
file-selected = Файл: { $name }
file-none = Файл .torrent не выбран
or-magnet = Или вставьте magnet-ссылку:
btn-folder = Папка...
folder-label = Папка: { $dir }
err-bad-magnet = Некорректная magnet-ссылка: { $err }
err-no-source = Выберите .torrent файл или вставьте magnet-ссылку
btn-cancel = Отмена
btn-confirm-add = Добавить

modal-del-title = Удаление торрента
modal-del-msg = Удалить «{ $name }»?
btn-remove-list = Убрать из списка
btn-delete-disk = Удалить с диска

modal-lim-title = Глобальные лимиты скорости (КБ/с, 0 = без лимита)
lbl-dl-lim = Загрузка (↓):
lbl-ul-lim = Отдача (↑):
btn-save = Сохранить

modal-about-title = О программе TorTor
about-version = Версия { $ver }
about-desc = Высокопроизводительный BitTorrent-клиент на Rust с графическим интерфейсом Hey. Поддерживает TCP, QUIC, IPv6, DHT, PEX, UPnP и выборочную загрузку файлов.
about-author = Автор: Виталий Голик (mjojo / GLK Dev)
about-license = Лицензия: MIT OR Apache-2.0
about-repo = Репозиторий: https://github.com/GLK-Dev/tortor
btn-close = Закрыть

unit-b = Б
unit-kb = КБ
unit-mb = МБ
unit-gb = ГБ
unit-tb = ТБ
eta-sec = { $n } с
eta-min = { $n } мин
eta-hour = { $n } ч
eta-day = { $n } д
"#;

pub fn build_i18n(locale: &str) -> I18n {
    let mut i18n = I18n::new("en");
    let _ = i18n.add_ftl("en", EN_FTL);
    let _ = i18n.add_ftl("ru", RU_FTL);
    let target = match locale {
        "en" | "en-US" => "en",
        _ => "ru",
    };
    i18n.set_locale(target);
    i18n
}

pub fn format_size(i18n: &I18n, bytes: u64) -> String {
    const KEYS: [&str; 5] = ["unit-b", "unit-kb", "unit-mb", "unit-gb", "unit-tb"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < KEYS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let unit_str = i18n.tr(KEYS[unit]);
    if unit == 0 {
        format!("{bytes} {unit_str}")
    } else if value >= 100.0 || ((value * 10.0).round() as u64).is_multiple_of(10) {
        format!("{} {unit_str}", i18n.format_number(value, 0))
    } else {
        format!("{} {unit_str}", i18n.format_number(value, 1))
    }
}

pub fn format_speed(i18n: &I18n, bytes_per_sec: f64) -> String {
    if bytes_per_sec < 1.0 {
        return "—".to_string();
    }
    let suffix = if i18n.locale().starts_with("ru") {
        "/с"
    } else {
        "/s"
    };
    format!("{}{suffix}", format_size(i18n, bytes_per_sec as u64))
}

pub fn format_eta(
    i18n: &I18n,
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
        i18n.tr_args("eta-sec", &[("n", Arg::Num(secs as f64))])
    } else if secs < 3600 {
        i18n.tr_args("eta-min", &[("n", Arg::Num((secs / 60) as f64))])
    } else if secs < 86400 {
        i18n.tr_args("eta-hour", &[("n", Arg::Num((secs / 3600) as f64))])
    } else {
        i18n.tr_args("eta-day", &[("n", Arg::Num((secs / 86400) as f64))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_between_en_and_ru_cleanly() {
        let mut i18n = build_i18n("ru");
        assert_eq!(i18n.tr("btn-add"), "Добавить");
        assert_eq!(i18n.tr("col-name"), "ИМЯ");
        assert_eq!(format_size(&i18n, 640 * 1024 * 1024), "640 МБ");

        i18n.set_locale("en");
        assert_eq!(i18n.tr("btn-add"), "Add");
        assert_eq!(i18n.tr("col-name"), "NAME");
        assert_eq!(format_size(&i18n, 640 * 1024 * 1024), "640 MB");
    }
}