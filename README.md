<div align="center">

<img src="Images/tortor_icon.png" alt="TorTor Logo" width="112" height="112">

# TorTor

**High-performance cross-platform BitTorrent client written in Rust — built with native [Hey](https://github.com/GLK-Dev/Hey) GPU UI, Linux `io_uring`, and `QUIC + IPv6`.**

[![CI](https://github.com/GLK-Dev/tortor/actions/workflows/ci.yml/badge.svg)](https://github.com/GLK-Dev/tortor/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/GLK-Dev/tortor?color=f5a623&label=release)](https://github.com/GLK-Dev/tortor/releases/latest)
[![Pages](https://img.shields.io/badge/website-GitHub%20Pages-00e5ff)](https://glk-dev.github.io/tortor/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-00ff66)](LICENSE)

[**Website**](https://glk-dev.github.io/tortor/) · [**Download Releases**](https://github.com/GLK-Dev/tortor/releases/latest) · [**Hey UI Framework**](https://github.com/GLK-Dev/Hey) · [**Русский**](#русский)

</div>

---

## English

**TorTor** is a lightweight, crash-safe BitTorrent desktop client built from the ground up in **Rust** by [Vitaly Golik (mjojo)](https://github.com/mjojo) / [GLK Dev](https://github.com/GLK-Dev). Instead of bundling Electron or a web browser, TorTor uses our custom pure-Rust **[Hey](https://github.com/GLK-Dev/Hey)** MVU interface rendered directly with `wgpu` (and automatic `softbuffer` CPU fallback).

### Highlights

| Area | Capabilities |
| --- | --- |
| **Desktop GUI** | Native **Windows**, **Linux**, and **macOS** interface powered by **Hey** (`wgpu` / `softbuffer`), HiDPI & system `TextScaleFactor` scaling, compact density mode, live **RU / EN** Fluent localization (`hey-i18n`). |
| **Networking** | Multi-torrent swarms, `.torrent` files & `magnet:` URIs (BEP 9/10), HTTP/UDP multi-tracker `announce-list`, Kademlia DHT (BEP 5 + IPv6 BEP 32), PEX (BEP 11), automatic UPnP forwarding, dual-stack IPv4/IPv6 TCP + TorTor-to-TorTor **QUIC (TLS 1.3)**. |
| **Disk & Integrity** | Linux **`io_uring`** (`tokio-uring`), Windows `FILE_SHARE_DELETE` (files stay movable/deletable), per-piece SHA-1 verification before write, crash-safe atomic `fastresume`, disk re-check on existing files, and selective multi-file download. |
| **Fine-Grained Control** | 4-tab **Settings** modal (Default folder, start-paused, compact density, listen port, DHT/UPnP/PEX/QUIC toggles, active/inbound peer caps, upload slots, block pipeline depth, connect timeout, resume interval) + instant global speed caps. |
| **Security & Hardening** | Strict message size & bencode nesting limits, path traversal sanitization, validated torrent metadata, and automated `rustsec` dependency auditing in CI. |

### Download Ready GUI Builds

Prebuilt desktop archives are published automatically on every release tag via [`.github/workflows/release.yml`](.github/workflows/release.yml):

| Platform | Package | Notes |
| --- | --- | --- |
| **Windows (x86_64)** | [`tortor_v*_windows_x86_64.zip`](https://github.com/GLK-Dev/tortor/releases/latest) | Standalone `tortor.exe` with embedded multi-resolution `.ico` icon & Windows `TextScaleFactor` support. |
| **Linux (x86_64)** | [`tortor_v*_linux_x86_64.tar.gz`](https://github.com/GLK-Dev/tortor/releases/latest) | ELF GUI binary (`io_uring`, Wayland/X11, GTK3 XDG portals) + `./install.sh` & `.desktop` launcher. |
| **macOS (Universal)** | [`tortor_v*_macos_universal.zip`](https://github.com/GLK-Dev/tortor/releases/latest) | Universal `TorTor.app` bundle (`arm64` Apple Silicon + `x86_64` Intel) with `.icns`, Retina HiDPI, `.torrent` & `magnet:` handlers. |

### Build from Source

Requires a recent stable Rust toolchain ([rustup](https://rustup.rs)). `Hey` crates are fetched automatically from GitHub:

```bash
git clone https://github.com/GLK-Dev/tortor.git
cd tortor
cargo run --release
```

- **Windows:** you can also run `build.bat` (`1` = debug, `2` = release).
- **Linux:** requires a C compiler (`gcc`) and desktop dev packages (`pkg-config libxkbcommon-dev libwayland-dev libxcb1-dev libxcb-render0-dev libxcb-shm0-dev libxcb-xfixes0-dev libgtk-3-dev`). Run `./packaging/linux/install.sh` to install the binary, icon, and `.desktop` launcher into `~/.local`. Headless build (no GUI): `cargo build --release --no-default-features`.
- **macOS:** requires Xcode Command Line Tools (`Metal` / Cocoa). After `cargo build --release`, run `./packaging/macos/build_app.sh` to package `TorTor.app`.
- **Haiku:** links the HeOS SDK (`libheos_fs`, `libheos_net`); set `HEOS_SDK_LIB_DIR` to `<haiku-tree>/sdk/lib` (defaults to `/boot/system/develop/lib` and `/boot/system/lib`).

### CLI Reference

```text
tortor [OPTIONS] [MAGNET_OR_TORRENT]

  -t, --torrent <FILE>        open a .torrent file at start (positional .torrent paths also work)
  -o, --output <DIR>          download directory (GUI defaults to OS Downloads; CLI defaults to .)
      --listen-port <PORT>    first port for TCP + QUIC (default: 6881; DHT uses next port)
  -v, --verbose               enable debug logging
      --cli                   run without GUI: inspect .torrent, query tracker (--announce-tracker), or listen
```

> Open TCP and UDP port `6881` (or your configured port) in your router/firewall if UPnP is unavailable so inbound peers can connect.

### Storage Locations

| Data | Windows | Linux | macOS |
| --- | --- | --- | --- |
| Session list & settings | `%APPDATA%\TorTor\session.json` | `~/.config/TorTor/session.json` | `~/Library/Application Support/TorTor/session.json` |
| Fast-resume state | `%APPDATA%\TorTor\resume\` | `~/.local/share/TorTor/resume/` | `~/Library/Application Support/TorTor/resume/` |

### Architecture & Development

- `src/core` — torrent parser (`bencode`), piece & selection manager, async disk backends (`StandardDisk`, Linux `UringDisk`, Haiku `BfsDirectDisk`), atomic `fastresume`, `SessionStore`, and runtime `ClientSettings`.
- `src/net` — dual-stack TCP/QUIC `Engine`, peer wire protocol & swarm scheduler, HTTP/UDP trackers, Kademlia DHT, magnet fetcher, UPnP (`igd-next`), and global bandwidth shaper.
- `src/ui` — Hey MVU dashboard, Industrial Dark theme tokens, custom chamfered widgets (`PieceMapBar`, `TableHeader`, `TorrentRow`), Fluent EN/RU i18n, and background session coordinator.
- `tests/` — loopback integration tests for magnet metadata fetch and TCP/QUIC/IPv4/IPv6 transfers.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo bench --bench hash_bench
```

See [CHANGELOG.md](CHANGELOG.md) for the release history. Not yet supported: uTP (BEP 29), BitTorrent v2/hybrid (BEP 52), MSE/PE encryption, Fast Extension (BEP 6), LPD (BEP 14), NAT-PMP/PCP, full tit-for-tat choking, sequential mode, and downloading in `--cli` mode.

---

### Built with Hey by GLK Dev

TorTor is developed by **[Vitaly Golik (mjojo)](https://github.com/mjojo)** under **[GLK Dev](https://github.com/GLK-Dev)** and showcases **[Hey](https://github.com/GLK-Dev/Hey)** — our pure-Rust cross-platform desktop UI framework featuring GPU/CPU rendering, Flexbox/Taffy layout, `fluent-bundle` localization, and zero WebView/Electron overhead.

---

<a id="русский"></a>
## Русский

**TorTor** — быстрый, надёжный и лёгкий кроссплатформенный BitTorrent-клиент, написанный с нуля на **Rust** ([Виталий Голик / mjojo](https://github.com/mjojo), **[GLK Dev](https://github.com/GLK-Dev)**). Вместо тяжёлого Electron/Chromium используется наш собственный нативный GUI-фреймворк **[Hey](https://github.com/GLK-Dev/Hey)** с аппаратным рендерингом `wgpu` и автоматическим fallback на `softbuffer`.

### Ключевые возможности

| Раздел | Возможности |
| --- | --- |
| **Десктопный GUI** | Нативный интерфейс для **Windows**, **Linux** и **macOS** на базе **Hey**: HiDPI, системный масштаб `TextScaleFactor`, компактный режим, переключение **RU / EN** на лету (`hey-i18n`). |
| **Сеть и пиры** | Несколько торрентов в одном окне, `.torrent` и `magnet:` (BEP 9/10), HTTP/UDP-трекеры и `announce-list`, Kademlia DHT (BEP 5 + IPv6 BEP 32), PEX (BEP 11), UPnP, dual-stack IPv4/IPv6 TCP + **QUIC (TLS 1.3)** между клиентами TorTor. |
| **Диск и целостность** | **`io_uring`** (`tokio-uring`) в Linux, режим `FILE_SHARE_DELETE` в Windows (файлы не блокируются от переноса/удаления), SHA-1 проверка каждого куска, атомарный `fastresume`, автопроверка существующих файлов и выборочное скачивание. |
| **Тонкая настройка** | Окно **Настройки** (4 вкладки: папка загрузок, старт на паузе, компактность, порт, DHT/UPnP/PEX/QUIC, лимиты пиров, слоты отдачи, глубина конвейера, таймауты, интервал сохранения) + мгновенные лимиты скорости. |
| **Безопасность** | Лимиты размера сообщений и глубины bencode, защита от path traversal, строгая валидация метаданных и аудит зависимостей через `rustsec`. |

### Готовые сборки и установка из исходников

Готовые архивы для **Windows (`.zip`)**, **Linux (`.tar.gz` + `install.sh` + `.desktop`)** и **macOS Universal (`TorTor.app` для Apple Silicon + Intel)** доступны на странице [Releases](https://github.com/GLK-Dev/tortor/releases/latest).

Сборка из исходного кода (требуется стабильный Rust):

```bash
git clone https://github.com/GLK-Dev/tortor.git
cd tortor
cargo run --release
```

- **Windows:** можно использовать скрипт `build.bat`.
- **Linux:** установите `gcc`, `pkg-config`, `libxkbcommon-dev`, `libwayland-dev`, `libxcb*-dev`, `libgtk-3-dev`. После сборки `./packaging/linux/install.sh` установит приложение, иконку и `.desktop`-ярлык. Без GUI: `cargo build --release --no-default-features`.
- **macOS:** требуются Xcode Command Line Tools (`Metal`/Cocoa). После `cargo build --release` скрипт `./packaging/macos/build_app.sh` соберёт готовый бандл `TorTor.app`.

### Где хранятся данные

| Данные | Windows | Linux | macOS |
| --- | --- | --- | --- |
| Сессии и настройки | `%APPDATA%\TorTor\session.json` | `~/.config/TorTor/session.json` | `~/Library/Application Support/TorTor/session.json` |
| Данные возобновления | `%APPDATA%\TorTor\resume\` | `~/.local/share/TorTor/resume/` | `~/Library/Application Support/TorTor/resume/` |

---

<div align="center">

Dual-licensed under **MIT** or **Apache-2.0** · See [LICENSE](LICENSE) · Crafted by [**mjojo**](https://github.com/mjojo) at [**GLK Dev**](https://github.com/GLK-Dev)

</div>