# Changelog - TorTor

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