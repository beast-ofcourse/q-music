# Q-Music Architecture

## Overview

Q-Music is a cross-platform desktop music player with a Rust native core and a web-based frontend. The architecture follows a layered design where the Rust core owns all audio processing, indexing, and storage, while the frontend provides the user interface.

## System Layers

```
┌─────────────────────────────────────────────┐
│              Frontend (React 19)              │
│  Tauri 2 + TypeScript + Vite 8              │
│  ┌─────────┐ ┌──────────┐ ┌──────────────┐  │
│  │  Views  │ │Components│ │  State Store │  │
│  └────┬────┘ └────┬─────┘ └──────┬───────┘  │
│       └───────────┼──────────────┘           │
│                   │ IPC (Tauri commands)      │
├───────────────────┼──────────────────────────┤
│              Tauri Shell                     │
│  ┌────────────────┴──────────────────────┐  │
│  │  AppState: Library, Engine, Media      │  │
│  │  ExtensionState: ExtensionStore        │  │
│  └────────────────┬──────────────────────┘  │
├───────────────────┼──────────────────────────┤
│              Rust Core (qcore)               │
│  ┌────────┐ ┌──────┐ ┌───────┐ ┌────────┐  │
│  │ Engine │ │Library│ │  Tags │ │  LRC   │  │
│  └────────┘ └──────┘ └───────┘ └────────┘  │
│  ┌────────┐ ┌──────┐ ┌───────┐ ┌────────┐  │
│  │   EQ   │ │ Art  │ │ Scan  │ │ Media  │  │
│  └────────┘ └──────┘ └───────┘ └────────┘  │
├───────────────────┼──────────────────────────┤
│           Extension Host (qext)              │
│  ┌────────┐ ┌──────┐ ┌───────┐ ┌────────┐  │
│  │Package │ │Runtime│ │Adapter│ │ Calls  │  │
│  └────────┘ └──────┘ └───────┘ └────────┘  │
│  ┌────────┐ ┌──────┐ ┌───────┐ ┌────────┐  │
│  │ Store  │ │ Reg. │ │ Queue │ │Download│  │
│  └────────┘ └──────┘ └───────┘ └────────┘  │
└─────────────────────────────────────────────┘
```

## Audio Engine

The audio engine uses a producer-consumer pattern:

1. **Decode thread**: Reads audio files using Symphonia, decodes to PCM samples
2. **Sample queue**: `ArrayQueue` (capacity 1<<17) bridges decode and output
3. **Output callback**: CPAL audio callback pulls samples from the queue
4. **UI polling**: Frontend polls `EngineSnapshot` every 120ms

### Playback Speed
Implemented as tape-style resampling (pitch follows speed). The `signalsmith-stretch` crate was rejected in favor of this simpler approach.

### Gapless Playback
The engine pre-loads the next track and seamlessly transitions between tracks.

## Database

SQLite with WAL mode for concurrent access:
- **Main Library handle**: Used by the UI for queries
- **Scan handle**: Separate connection for background scanning (never blocks playback)

Tables: tracks, playlists, playlist_items, settings, call_cache

## Extension Host

### Runtime
- QuickJS (rquickjs 0.14) with one OS thread per extension
- One QuickJS context per extension, created lazily
- JSON bridge (`__qext_call`) for native-JS communication
- Memory limit: 128 MB soft, stack limit configured per context
- Deadline interrupt handler (default 30s)

### Package Format
- `.sflx` files are ZIP archives containing `manifest.json` and `index.js`
- SHA-256 integrity verification against registry
- Safe extraction: path traversal prevention, archive bomb protection, symlink rejection

### Provider System
- Three provider kinds: metadata, download, lyrics
- Priority ordering per capability (user-configurable)
- Health monitoring (3 consecutive failures → unhealthy)
- Call caching with TTL

### Download Queue
- 8-state transition table: queued → resolving → downloading → post_processing → embedding → done/failed/cancelled
- Concurrency: default 2, min 1
- Progress throttled to 10 Hz with forced final emit
- Resume support (Range: bytes=N-)
- Cancel deletes partial file

## Frontend

### State Management
- React Context + useReducer (no external state library)
- Polls engine snapshot every 120ms
- Optimistic updates for provider reordering

### Theming
- Three themes: carbon (default), paper, stage
- CSS custom properties for all colors
- Self-hosted fonts: Archivo (variable) + IBM Plex Mono

### Responsive Design
- 3-column layout at ≥1180px (rail 208px, main, now panel 340px)
- Now panel becomes overlay drawer below 1180px
- Window minimum: 960×620

## File Structure

```
q-music/
├── crates/
│   ├── qcore/          # Audio engine, library, tags, LRC
│   └── qext/           # Extension host
├── src/                # React frontend
│   ├── components/     # UI components
│   ├── views/          # Page views
│   ├── lib/            # Utilities, IPC, types
│   └── state/          # State management
├── src-tauri/          # Tauri desktop shell
├── scripts/            # Build/dev scripts
├── docs/               # Documentation
├── EXTENSIONS.md       # Extension compatibility matrix
├── SECURITY.md         # Security model
├── DESIGN.md           # Design system
└── README.md           # Project overview
```
