# TorTor

TorTor is a next-generation, high-performance BitTorrent client written in Rust.
It combines memory safety with low-level speed through dynamic CPU dispatch and a modular architecture designed for zero-copy I/O evolution.

## Key Features

- Dynamic SIMD dispatch: runtime CPU detection for AVX2 and SSE4.1 with a portable fallback path.
- Memory-safe core: piece verification and protocol logic in safe Rust by default.
- Optional GPU path: feature-gated scaffold for future compute-based hashing.
- Clear module boundaries: separate `core`, `crypto`, and `net` layers.

## Installation and Build

Ensure you have the latest stable Rust toolchain installed.

```bash
git clone https://github.com/GLK-Dev/tortor.git
cd tortor
cargo build --release
```

Run tests:

```bash
cargo test
```

Run benches:

```bash
cargo bench
```

Run GUI dashboard (egui) with tracker peers view:

```bash
cargo run --features gui -- --torrent <file.torrent> --gui
```

## Architecture

TorTor separates responsibilities into focused modules:

- `src/net`: async network primitives and listener scaffolding.
- `src/core`: protocol/domain layer for torrent metadata and state.
- `src/crypto`: hash implementations, SIMD targets, and runtime dispatch.

Current runtime hashing flow:

1. Detect CPU features (`AVX2` -> `SSE4.1` -> portable).
2. Dispatch SHA-1/SHA-256 piece verification to the best available backend.
3. Fall back safely on unsupported hardware.

## Roadmap

| Feature | Status | Target |
| --- | --- | --- |
| Basic TCP listener and protocol scaffolding | In Progress | v0.1.0 |
| Dynamic SIMD hashing (AVX2 / SSE4.1) | In Progress | v0.2.0 |
| io_uring disk pipeline (Linux) | Planned | v0.3.0 |
| QUIC/WebTransport transport experiments | Planned | v0.4.0 |
| GPU hashing backend | Research | v1.0.0 |

## Author

Created and maintained by Vitaliy Golik ([mjojo](https://github.com/mjojo)) under [GLK Dev](https://github.com/GLK-Dev).

## License

Dual-licensed under MIT or Apache-2.0. See the LICENSE file for details.
