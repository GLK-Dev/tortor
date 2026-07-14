# TorTor

TorTor is a next-generation, high-performance BitTorrent client written in Rust.
It combines memory safety with low-level speed through dynamic CPU dispatch and a modular architecture designed for zero-copy I/O evolution.

## Key Features

- Dynamic SIMD dispatch: runtime CPU detection for AVX2 and SSE4.1 with a portable fallback path.
- Memory-safe core: piece verification and protocol logic in safe Rust by default.
- Optional GPU path: feature-gated scaffold for future compute-based hashing.
- Clear module boundaries: separate core, crypto, and 
et layers.
- Multi-file support: Downloads complex torrent structures.
- Modern GUI: Built-in beautiful interface with egui.

## Installation and Build

Ensure you have the latest stable Rust toolchain installed.

`ash
git clone https://github.com/GLK-Dev/tortor.git
cd tortor
cargo build --release
`

Run tests:

`ash
cargo test
`

Run benches:

`ash
cargo bench
`

Run the client (starts the GUI by default):

`ash
cargo run --features gui
`

Or run headless in CLI mode specifying output directory:

`ash
cargo run -- --cli --torrent file.torrent --output ./downloads
`

## Architecture

TorTor separates responsibilities into focused modules:

- src/net: async network primitives and listener scaffolding.
- src/core: protocol/domain layer for torrent metadata and state.
- src/crypto: hash implementations, SIMD targets, and runtime dispatch.

Current runtime hashing flow:

1. Detect CPU features (AVX2 -> SSE4.1 -> portable).
2. Dispatch SHA-1/SHA-256 piece verification to the best available backend.
3. Fall back safely on unsupported hardware.

## Roadmap

| Feature | Status | Target |
| --- | --- | --- |
| Basic TCP listener and protocol scaffolding | Done | v1.0.0 |
| Dynamic SIMD hashing (AVX2 / SSE4.1) | Done | v1.0.0 |
| io_uring disk pipeline (Linux) | Planned | v2.0.0 |
| QUIC/WebTransport transport experiments | Planned | v2.0.0 |
| GPU hashing backend | Research | v2.0.0 |

## Author

Created and maintained by Vitaliy Golik ([mjojo](https://github.com/mjojo)) under [GLK Dev](https://github.com/GLK-Dev).

## License

Dual-licensed under MIT or Apache-2.0. See the LICENSE file for details.
