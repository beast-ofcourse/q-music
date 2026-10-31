# Q-Music

**Quantum Music** — a cross-platform desktop music player built with a Rust native core and a modern web frontend.

![Q-Music](https://img.shields.io/badge/version-0.1.0-blue) ![Rust](https://img.shields.io/badge/rust-1.85%2B-orange) ![Tauri](https://img.shields.io/badge/tauri-2.x-blue) ![React](https://img.shields.io/badge/react-19-61dafb)

## Overview

Q-Music combines a high-performance Rust audio engine with a premium React/Tauri desktop interface. It supports a wide range of audio formats, provides a rich library management experience, and includes a modular extension platform compatible with the Mornye Extension format.

## Architecture

| Layer | Technology |
|-------|-----------|
| Audio decode | Symphonia 0.6.1 |
| Audio output | CPAL 0.18.2 |
| Tags/metadata | Lofty 0.25.5 |
| Database | rusqlite 0.41.1 (bundled) |
| Desktop shell | Tauri 2 |
| Frontend | React 19 + TypeScript + Vite 8 |
| Extension runtime | QuickJS (rquickjs 0.14) |
| HTTP client | ureq 3.5 (rustls + cookies) |

## Features

### Audio Playback
- **Formats**: FLAC, MP3, M4A/AAC, WAV, OGG, OPUS, AIFF, ALAC
- Gapless playback, shuffle, repeat (off/one/all), speed control (tape-style)
- 10-band EQ, device selection, keyboard shortcuts
- OS media controls (souvlaki)
- Low memory footprint, fast startup

### Library Management
- Recursive folder scanning with progress
- Manual and smart playlists
- Search, sorting, filtering by genre/format
- Album/artist browsing, folder navigation
- Favorites, recently played, recently added

### Lyrics
- Embedded + sidecar (.lrc/.txt) with sync
- Manual lyric editing with timing
- Provider lyrics search (Apple Music)

### Extension Platform (Mornye-compatible)
- Modular Rust extension host with sandboxed JS runtime
- Permissions: network, storage, file (scoped)
- Package safety: path traversal, archive bombs, hash verification
- Provider registry with priority ordering and health monitoring
- Download queue with progress, cancel, retry, resume
- Automatic metadata/artwork/lyrics embedding
- 5 integration packages supported
- 2 download providers refused (missing runtime features)

### Security
- Packages treated as untrusted code
- QuickJS sandbox with memory/stack limits
- Path traversal prevention
- Network domain allowlisting
- SHA-256 package integrity verification

## Getting Started

### Prerequisites
- Rust 1.85+
- Node.js 22+
- Tauri CLI

### Development

```bash
# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev

# Run tests
cargo test -p qext --manifest-path crates/qext/Cargo.toml
cargo test -p qcore --manifest-path crates/qcore/Cargo.toml
npx tsc --noEmit
npx vite build
```

### Build

```bash
npm run tauri build
```

## Documentation

- [docs/architecture.md](docs/architecture.md) - System architecture
- [docs/development.md](docs/development.md) - Developer guide
- [docs/extension-development.md](docs/extension-development.md) - Extension development guide
- [EXTENSIONS.md](EXTENSIONS.md) - Extension compatibility matrix
- [SECURITY.md](SECURITY.md) - Security model and threat analysis
- [DESIGN.md](DESIGN.md) - Design system specification
- [PRODUCT.md](PRODUCT.md) - Product requirements
- [SURFACE.md](SURFACE.md) - UI surface documentation

## License

See [LICENSE](LICENSE) for details.