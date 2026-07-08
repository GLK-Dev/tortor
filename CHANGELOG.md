# Changelog - TorTor v0.1.0-alpha.1

## [0.1.0-alpha.1] - 2026-07-08

### Архитектурные достижения
- **Core Pipeline:** Полная реализация Actor Model для управления сессиями без мьютексов.
- **Swarm Manager:** Автономный менеджер роя с авто-пополнением (tracker re-announce) и защитой от медленных пиров (60s no-progress timeout).
- **Data Path:** Надежный сборщик кусков (PieceAssembler) с поддержкой out-of-order блоков и SHA-1 верификацией.
- **Fast Resume:** Автоматическое восстановление прогресса из `.fastresume` файлов.
- **Graceful Shutdown:** Безопасное завершение задач через broadcast-шину и перехват системных сигналов.

### UX & Interface
- **Desktop-First Flow:** Нативная интеграция выбора файла (`rfd`).
- **Live Telemetry:** Color-coded индикация здоровья соединений и ProgressBar для кусков.
- **Background Persistence:** Фоновый процесс записи на диск и авто-сохранение состояния.

### Технические детали
- Использование `tokio` для всей асинхронности.
- `egui` для Immediate Mode GUI.
- `rfd` для системных диалогов.
