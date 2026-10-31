# Development Guide

## Prerequisites

- Rust 1.85+ (with `cargo`)
- Node.js 22+ (with `npm`)
- Tauri CLI (`cargo install tauri-cli`)

## Setup

```bash
# Clone the repository
git clone https://github.com/beast-ofcourse/q-music.git
cd q-music

# Install frontend dependencies
npm install

# Verify Rust toolchain
cargo --version
rustc --version
```

## Development Workflow

### Running in Development Mode

```bash
# Start the Tauri dev server (frontend + Rust)
npm run tauri dev
```

This starts:
- Vite dev server on port 5183
- Tauri window with hot-reload for frontend
- Rust recompilation on source changes

### Browser-Only Mode

For UI development without the Rust backend:

```bash
npm run dev
```

Opens at `http://localhost:5173`. Uses the demo backend with synthetic data.

## Testing

### Rust Tests

```bash
# qext (extension host) — 267 tests
cargo test -p qext --manifest-path crates/qext/Cargo.toml

# qext with real packages — 6 additional tests
cargo test -p qext --manifest-path crates/qext/Cargo.toml --features real-packages

# qcore (audio engine) — 49 tests
cargo test -p qcore --manifest-path crates/qcore/Cargo.toml

# Clippy
cargo clippy -p qext --manifest-path crates/qext/Cargo.toml --all-targets -- -D warnings
```

### Frontend Tests

```bash
# TypeScript type check
npx tsc --noEmit

# Build (catches build errors)
npx vite build
```

### Screenshot Testing

```bash
# Start preview server
npx vite preview --port 4173

# Start Chrome with remote debugging
chrome --headless=new --remote-debugging-port=9222 --user-data-dir=/tmp/qmusic-chrome http://localhost:4173/

# Take screenshots
node scripts/shot.mjs --name my-screenshot --width 1440 --height 900 --theme carbon
```

## Project Structure

### Key Directories

| Directory | Purpose |
|-----------|---------|
| `crates/qcore/` | Audio engine, library, tags, LRC parser |
| `crates/qext/` | Extension host, runtime, providers, queue |
| `src/` | React frontend (components, views, lib, state) |
| `src-tauri/` | Tauri desktop shell, commands, state |
| `scripts/` | Build, dev, screenshot, probe tools |
| `docs/` | Documentation |

### Key Files

| File | Purpose |
|------|---------|
| `src/lib/ipc.ts` | Backend interface (Tauri + demo implementations) |
| `src/lib/extensions.ts` | Extension platform contract + demo backend |
| `src/state/store.tsx` | Application state (React Context + reducer) |
| `src-tauri/src/lib.rs` | Tauri app setup and command registration |
| `src-tauri/src/commands.rs` | Core commands (library, engine, playlists) |
| `src-tauri/src/ext_commands.rs` | Extension platform commands |

## Coding Conventions

### Rust
- Error handling: every error is a typed enum, never `unwrap()` in production
- No `unsafe` code
- Clippy clean (`-D warnings`)
- Tests in `#[cfg(test)]` modules within source files

### TypeScript
- Strict mode enabled
- No `any` types
- Components use functional components with hooks
- CSS uses custom properties from `tokens.css`

### CSS
- Design tokens in `src/styles/tokens.css`
- Component styles in feature-specific files (`extensions-*.css`)
- `app.css` is the shared stylesheet — don't modify for feature work

## Building for Production

```bash
# Build the Tauri app
npm run tauri build

# Output: src-tauri/target/release/bundle/
```

## CI/CD

GitHub Actions workflow (`.github/workflows/ci.yml`):
- **Rust**: Tests qext (267), qext real-packages (6), qcore (49), clippy
- **Frontend**: TypeScript check + Vite build
- **Release**: Tauri build on version tags

## Debugging

### Rust
```bash
# Run with backtrace
RUST_BACKTRACE=1 cargo test -p qext --manifest-path crates/qext/Cargo.toml

# Run specific test
cargo test -p qext --manifest-path crates/qext/Cargo.toml test_name
```

### Frontend
- Browser DevTools (F12) for React component inspection
- Network tab for IPC calls
- Console for error messages

### Tauri
- DevTools available in debug builds (right-click → Inspect)
- Logs in `%APPDATA%/q-music/logs/` (Windows)
