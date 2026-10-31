# Q-Music v0.2.0 — Release Notes

**Release date:** 2026-10-31
**Branch:** `feature/lyrics-redesign`
**Tag:** `v0.2.0`

## Summary

v0.2.0 is the first feature-complete release of Q-Music's **extension platform** and
**online discovery** stack. The Rust extension host (`qext`) now ships a real download
executor, persisted settings, provider health tracking, registry installs, and native
file transfers, while the desktop shell gains encrypted credentials, ffmpeg introspection,
and the Phase 1–4 online command surface. The frontend adds a Home feed, resolved track
view, browse-categories screen, and a URL-paste handler. The synced-lyrics experience is
rebuilt end to end.

## Added

### Extension platform (Rust, `qext` + `src-tauri`)
- **Real download executor** — `spawn_download_worker`, queue cancel/retry, and tag
  embedding for downloaded tracks.
- **Extension settings + provider priority persistence** — atomic writes to `state.json`.
- **Provider health tracking (desktop mode)** — a provider becomes unhealthy after
  `>= 2` consecutive failures; a successful retry clears the state.
- **Registry installs** — `extensions_install` from the Mornye registry with `sha256`
  digest and extension-id verification before activation.
- **Native file transfers** — `file.download` / `file.downloadSegments` routed through
  the `qext` transfer engine.
- **Phase 1 online command surface** — 30 s HTTP timeout, 6 new `error_type` codes,
  and parsed `skipBuiltInFallback`.
- **Phase 2** — desktop `enrichTrack` flow.
- **Phase 3** — `postProcess` / `postProcessV2` dispatch, plus `handleUrl`,
  `homeFeed`, and `browseCategories` commands.
- **Phase 4** — encrypted credentials store, `ffmpeg.getInfo`, and
  `gobackend.getAudioQuality`.
- Tauri commands: `providers_list`, `queue_jobs`, `queue_cancel`, `queue_retry`,
  `queue_enqueue`, `provider_lyrics`, `extensions_settings`, `save_settings`,
  `invoke_action`, `set_enabled`, `uninstall`, `providers_set_priority`,
  `providers_retry`.

### Frontend
- Home feed screen (`HomeFeedView`), resolved track view (`ResolvedView`), and
  browse-categories screen (`BrowseCategoriesView`).
- URL-paste handler wired into the main app shell.
- Two-column synced lyrics layout (artwork 35% / lyrics 65%) with auto-scroll and
  metadata filtering.
- Five new themes: `apple`, `glass`, `catppuccin-mocha`, `sky-catppuccin`,
  `babypink-catppuccin`.

### Documentation
- `docs/audit-report.md` (phases 1–8 status).
- `EXTENSIONS.md` (extension authoring + runtime contract).
- `docs/research-mornye-docs.md`.

## Changed
- Lyrics view rebuilt: centered, larger text, consistent spacing, no overlays/glassmorphism.
- Provider priority and enabled state now persist across restarts.
- Online search merges multiple providers in priority order with deduplication.
- HTTP client uses a 30 s timeout and surfaces typed errors (`timed_out`, etc.).

## Fixed
- Removed duplicate queue command definitions (compile conflict).
- Lyrics parser fixes (synced LRC parsing, metadata filtering).
- Interrupted install swaps now restore the previous version.
- Downgrades of an installed extension are refused with a typed error.
- Extension data directory is created outside the package directory.
- Uninstall stops the runtime before deleting files.

## Verification

| Check | Result |
|---|---|
| `cargo test --manifest-path crates/qext/Cargo.toml` | 286 passed, 0 failed |
| `cargo test --manifest-path crates/qext/Cargo.toml --features real-packages` | 6 passed, 0 failed (real-package integration) |
| `cargo test --manifest-path crates/qcore/Cargo.toml` | 49 passed, 0 failed |
| `cargo check --manifest-path src-tauri/Cargo.toml` | 0 errors (6 warnings, pre-existing) |
| `npx tsc --noEmit` | 0 errors |
| `npm run tauri:build` | see build artifacts below |

## Build artifacts

Produced by `npm run tauri:build` (Tauri bundle targets: `all`).

- `src-tauri/target/release/q-music.exe` (release binary)
- `src-tauri/target/release/bundle/msi/Q-Music_0.2.0_x64_en-US.msi` (8.50 MiB)
- `src-tauri/target/release/bundle/nsis/Q-Music_0.2.0_x64-setup.exe` (6.06 MiB)

## Migration notes

- No breaking API changes for end users; extension authors should consult `EXTENSIONS.md`
  for the new `file.download` / `file.downloadSegments` and credential APIs.
- `state.json` is written atomically; existing installs are migrated on first run.

## Known limitations

- Provider health tracking is desktop-mode only.
- `real-packages` integration tests require the `real-packages` feature flag.
- Warnings in `online_commands.rs` (unused imports/dead code) are pre-existing and
  non-blocking.

---

_Generated for release v0.2.0._
