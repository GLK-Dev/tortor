# Changelog - TorTor

## [Unreleased]

### Features / Возможности
- **File selection:** for multi-file torrents the file list with checkboxes (All / None) is shown in the torrent panel; only pieces that overlap the selected files are downloaded, progress and completion refer to the selection, and unselected files are not created. Pieces that straddle a skipped file are verified but never served to other peers. The selection is saved in session.json and can be changed while downloading: files added later are created and the pieces they share with already finished ones are fetched again; released files keep their data on disk. (Magnet links: files are known only after the metadata arrives, so everything is downloaded.)
- **IPv6:** the DHT socket is dual-stack as well (BEP 32: 
odes6, 18-byte IPv6 peer values, family-matched replies). The TCP listener and the QUIC endpoint are dual-stack (one port serves IPv4 and IPv6, falling back to IPv4 only); PEX exchanges IPv6 peers (`added6`/`dropped6`, key names fixed to BEP 11).
- **UPnP:** the peer port (TCP and UDP) is forwarded on a UPnP router, renewed every 30 minutes and removed on exit; the status is shown in the top bar. Routers that only accept permanent leases are handled. NAT-PMP/PCP are not implemented.

### Architecture / Архитектура
- **Shared network engine:** one runtime, one TCP listener, one QUIC endpoint and one DHT node for the whole application (previously one of each per torrent). Torrents register by info hash; inbound peers are routed to the right swarm. One peer id per client.
- **Crash-safe state:** resume data and `session.json` are written atomically (temp file, fsync, rename). The session list now lives in the per-user config directory (`%APPDATA%\TorTor`), resume data in the data directory; old files are migrated/read as before. Resume files use a compact binary bitfield (JSON from older versions is still read).
- **io_uring (Linux):** the backend now supports file selection, deferred flush and binary-search lookup, and handles partial reads/writes. The Linux build is cross-checked (cargo check/clippy --target x86_64-unknown-linux-gnu) but has not been run.
- **Disk:** pieces are no longer fsynced one by one; data is flushed before progress is recorded as durable and on shutdown. Binary-search file lookup. File re-check hashes off the coordinator thread. The coordinator thread is awaited on shutdown so the final flush finishes.
- **Speed limits:** global download/upload limits (token bucket, no busy polling, no `unsafe`) with UI fields in the top bar; persisted in `session.json`.
- **Removed** the decorative SIMD/GPU hashing layer: `sha1`/`sha2` already select SHA-NI/AVX2 at runtime (SHA-1 runs at ~2.2 GiB/s on the test machine, so hashing is not a bottleneck).

### Network / Сеть
- **Piece picking:** per-peer bitfields, rarest-first selection and endgame mode; pieces are only requested from peers that have them. Fixes stalls at 99.9 %.
- **Peer sessions:** one long-lived session per connection that downloads, uploads (seeding), sends keep-alives and drops silent/useless peers; at most 8 upload slots per torrent. Inbound TCP and QUIC connections are accepted.
- **Magnet links (BEP 9/10):** info hash parsing (hex/base32, `dn`, `tr`), metadata download from trackers and DHT with SHA-1 verification.
- **Trackers:** `announce-list` (BEP 12), non-compact peer lists, `peers6`, IPv6 UDP trackers, retries, real `uploaded`/`downloaded`/`left`; peers are used as soon as the first tracker answers. Torrents without trackers work through DHT/PEX.
- **DHT:** answers ping, find_node, get_peers and announce_peer (with tokens), announces itself for active torrents, validates the reply source, expires pending queries, sorted bencode keys, more bootstrap routers.
- **Fixed:** fresh torrents no longer start paused; completed pieces are no longer re-downloaded after a file check; resume state is saved at most every 5 s.

### Security / Безопасность
- **Wire protocol:** peer messages are decoded by a buffered, cancel-safe `MessageDecoder` with a 1 MiB message cap; unknown message ids are skipped instead of dropping the peer. REQUEST messages are validated (length ≤ 32 KiB, within the piece).
- **Torrent parsing:** path components are sanitized (no `..`, separators, NUL, Windows reserved names), piece length / size / piece count are validated, bencode nesting depth and .torrent / metadata / tracker response sizes are limited.
- **Robustness:** fixed `u32` overflow in block bounds, QUIC setup failures no longer panic the swarm task, tracker HTTP client has a timeout and a dynamic User-Agent.
- **Dependencies:** `rustls` 0.23.45, `reqwest` 0.12 (drops vulnerable `h2` 0.3 / `rustls-webpki` 0.101), removed unused `bincode`; CI now runs `rustsec/audit-check`, `cargo fmt` and `clippy -D warnings` pass.

## [1.6.3] - 2026-07-15

### Bug Fixes / Исправления ошибок
- **Endgame Stall Fix:** Implemented a global 120-second piece timeout to automatically drop stalled or choking peers, resolving an issue where the download could freeze at 99.9%. (Реализован глобальный тайм-аут в 120 секунд для кусков, чтобы сбрасывать зависших пиров. Это решает проблему зависания загрузки на 99.9%).
- **Seeding Transition:** Fixed an issue where the coordinator task would stop instead of transitioning into Seeding mode after a download completed. (Исправлена проблема, из-за которой координатор останавливался после 100% загрузки вместо перехода в режим раздачи).
- **Missing Files / Magnet Resume Bug:** Fixed a logic bug where restarting the client with missing files or using Magnet links would erroneously overwrite the `fastresume` state and silently re-download from 0%. The torrent is now correctly paused if files are missing. (Исправлен баг, при котором перезапуск клиента приводил к тихому удалению прогресса и перекачиванию с нуля. Теперь, если файлы перемещены, торрент корректно ставится на паузу).

### Added
- **Local File Verification (Force Recheck):** TorTor теперь проверяет хэши существующих файлов при добавлении торрента. Если файлы уже скачаны, клиент автоматически восстановит прогресс (работает и для Magnet-ссылок).
- **UI State Sync:** Синхронизация кнопки "Пауза/Продолжить" с состоянием ядра. Если загрузка прервана из-за отсутствия файлов, кнопка корректно переключается в "Продолжить" для старта с нуля в один клик.

## [1.5.0-alpha] - 2026-07-14

### Features / Новые функции
- **Peer Exchange (PEX - BEP 11):** Full inbound and outbound PEX implementation. The swarm manager dynamically computes connection deltas and broadcasts peer updates across the network, minimizing tracker dependency. (Полная поддержка входящего и исходящего PEX. Менеджер роя динамически вычисляет дельты соединений и рассылает обновления пиров по сети, минимизируя зависимость от трекера).
- **SessionEvent Channel Refactor:** Migrated internal inter-actor messaging to a unified, strongly-typed SessionEvent bus for seamless global broadcasts. (Миграция внутреннего общения акторов на единую строго-типизированную шину SessionEvent для бесшовных глобальных рассылок).


## [1.4.0] - 2026-07-14

### Core Architecture / Архитектура
- **Magnet Links (BEP 9 / BEP 10):** Full support for the Extension Protocol and metadata downloading. TorTor can now parse magnet links, connect to peers, and download the .torrent file directly from the swarm into memory. (Полная поддержка Magnet-ссылок и протокола расширений. Скачивание .torrent файла напрямую из роя в память).
- **Warm Transition (Горячий переход):** The underlying IO engine dynamically transitions from Metadata Assembly to Data Download without dropping active TCP connections to peers. (Динамическое переключение движка IO с режима метаданных на режим скачивания без разрыва TCP соединений).


## [1.3.0] - 2026-07-14

### Core Architecture
- **Choke/Unchoke State Machine:** Implemented strict peer state management. TorTor now protects the disk pipeline from unbounded requests by only serving pieces to explicitly unchoked peers that have shown interest.

## [1.2.0] - 2026-07-14

### Features & UI
- **ASCII UI Design:** Redesigned the download dashboard with text-based ASCII progress bars (`[██████████░░] 80%`) and emojis for a classic hacker aesthetic.
- **Neon Theme:** Upgraded the application color palette to feature neon blue and bright teal on a dark background.
- **App Icon Integration:** Successfully embedded a custom "Digital Vortex" logo into `tortor.exe` (Windows) and the eframe title bar.

## [1.1.0] - 2026-07-14

### Features
- **Multi-Torrent Manager:** Redesigned the GUI to support downloading and managing multiple torrents simultaneously. 
- **Interactive Progress Bars:** Added clickable, accordion-style progress bars displaying detailed statistics, peers, and individual controls (Start/Cancel/Delete) for each torrent.
- **Independent Sessions:** Each torrent operates in an isolated session state within the same application window.

## [1.0.0] - 2026-07-14

### Features
- **Multi-file Support:** Added full support for multi-file torrents (parsing and disk writing).
- **Default GUI:** Application now runs as a desktop GUI by default without showing the console window. Added an 'About' dialog.
- **Custom Download Location:** Users can specify the output directory for downloaded files via CLI (--output) or GUI will use the selected directory.

## [0.1.0-alpha.1] - 2026-07-08

### Архитектурные достижения
- **Core Pipeline:** Полная реализация Actor Model для управления сессиями без мьютексов.
- **Swarm Manager:** Автономный менеджер роя с авто-пополнением (tracker re-announce) и защитой от медленных пиров (60s no-progress timeout).
- **Data Path:** Надежный сборщик кусков (PieceAssembler) с поддержкой out-of-order блоков и SHA-1 верификацией.
- **Fast Resume:** Автоматическое восстановление прогресса из .fastresume файлов.
- **Graceful Shutdown:** Безопасное завершение задач через broadcast-шину и перехват системных сигналов.

### UX & Interface
- **Desktop-First Flow:** Нативная интеграция выбора файла (
fd).
- **Live Telemetry:** Color-coded индикация здоровья соединений и ProgressBar для кусков.
- **Background Persistence:** Фоновый процесс записи на диск и авто-сохранение состояния.

### Технические детали
- Использование 	okio для всей асинхронности.
- gui для Immediate Mode GUI.
- 
fd для системных диалогов.