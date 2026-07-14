# Changelog - TorTor

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
- **Desktop-First Flow:** Нативная интеграция выбора файла (fd).
- **Live Telemetry:** Color-coded индикация здоровья соединений и ProgressBar для кусков.
- **Background Persistence:** Фоновый процесс записи на диск и авто-сохранение состояния.

### Технические детали
- Использование 	okio для всей асинхронности.
- gui для Immediate Mode GUI.
- fd для системных диалогов.