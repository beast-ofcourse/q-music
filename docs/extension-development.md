# Extension Development Guide

## Overview

Q-Music supports Mornye-compatible extension packages (`.sflx`). Extensions provide metadata, search, lyrics, and download capabilities from external music services.

## Package Format

An `.sflx` file is a ZIP archive containing:

```
package.sflx
├── manifest.json    # Extension manifest
└── index.js         # JavaScript implementation
```

### Manifest Schema

```json
{
  "name": "my-provider",
  "displayName": "My Provider",
  "version": "1.0.0",
  "description": "A metadata provider",
  "type": ["metadata_provider"],
  "minAppVersion": "4.0.0",
  "requiredRuntimeFeatures": [],
  "permissions": {
    "network": ["api.example.com"],
    "storage": true,
    "file": false
  },
  "settings": [],
  "capabilities": {
    "homeFeed": false,
    "networkTimeoutSeconds": 15
  }
}
```

### Required Runtime Features

The host evaluates `requiredRuntimeFeatures` against its capability set. If any feature is unmet, the package is refused at install.

| Feature | Host Support | Description |
|---------|-------------|-------------|
| `preparedContext@1` | ✅ | Prepared context for download requests |
| `signedSession@1` | ❌ | Centralized signing service (ZARZ) |
| `signedSession@3` | ❌ | Same, v3 |
| `sessionGrant@1` | ❌ | Time-limited download tokens |
| `downloadSegments@1` | ❌ | Segmented/parallel download transfer |

### Function Surface

Extensions must export functions that the host calls. The adapter discovers functions from the runtime and builds a capability map.

#### Metadata Provider

| Function | Arity | Description |
|----------|-------|-------------|
| `initialize` | 1 | `config` — called on load |
| `cleanup` | 1 | `config` — called on unload |
| `searchTracks` | 2 | `query`, `options` |
| `getTrack` | 2 | `id`, `options` |
| `getAlbum` | 2 | `id`, `options` |
| `getArtist` | 2 | `id`, `options` |
| `getPlaylist` | 2 | `id`, `options` |
| `getHomeFeed` | 0 | Home feed (if `capabilities.homeFeed`) |
| `handleUrl` | 2 | `url`, `options` — URL parsing |
| `customSearch` | 2 | `query`, `options` |

#### Download Provider

| Function | Arity | Description |
|----------|-------|-------------|
| `checkAvailability` | 4 | `id`, `quality`, `options`, `config` |
| `getDownloadUrl` | 5 | `id`, `quality`, `options`, `config`, `progress` |
| `download` | 2 or 5 | `request`, `progressCallback` (or full signature) |
| `validateTrackForDownload` | 2 | `id`, `options` |
| `completeGrant` | 2 | `grant`, `config` |
| `matchTrack` | 2 | `query`, `options` |

#### Lyrics Provider

| Function | Arity | Description |
|----------|-------|-------------|
| `fetchLyrics` | 4 | `trackId`, `options`, `config`, `progress` |
| `getLyrics` | 2 | `trackId`, `options` |

## Host Polyfills

The host provides JavaScript polyfills for common operations:

| Polyfill | Description |
|----------|-------------|
| `utils.encrypt` | AES-256-GCM, SHA-256-derived key, 12-byte nonce, base64 output |
| `utils.decrypt` | Mirrors `encrypt` |
| `utils.sha256` | Returns hex string |
| `utils.hmacSHA1` | Returns byte array (Uint8Array) |
| `utils.sleep(ms)` | Returns `false` when cancelled via deadline |
| `getLocalTime()` | Returns `{ offsetMinutes: -getTimezoneOffset() }` |
| `file.download` | **Unimplemented** — throws `unsupported` |
| `file.move` | Moves a file (post-processing) |
| `ffmpeg.*` | FFmpeg hook execution |

## Installing Extensions

### From File
1. Open Q-Music → Extensions view
2. Click "Install from file..."
3. Select a `.sflx` or `.spotiflac-ext` file
4. The package is validated (SHA-256, manifest, structure) and installed

### From Registry
Extensions can also be installed from the published registry. The host fetches `registry.json` and displays available packages.

## Testing Extensions

```bash
# Unit tests for the extension host
cargo test -p qext --manifest-path crates/qext/Cargo.toml

# Real package integration tests
cargo test -p qext --manifest-path crates/qext/Cargo.toml --features real-packages
```

## Known Divergences

See `EXTENSIONS.md` for the full list of 13 documented divergences between the Mornye platform documentation and what actually ships.
