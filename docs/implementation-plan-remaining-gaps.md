# Implementation Plan — Close the Remaining 10% (Mornye Compatibility)

Source audit: `docs/research-mornye-docs.md` (fetched 2026-10-31).
Scope: implement the gaps the audit identified so Q-Music's `qext` host matches
the documented Mornye extension contract. Phases are ordered by risk and
dependency; each subtask is independently verifiable. Nothing here is committed
until you say so (standing rule).

Conventions: every subtask ends with a verification command that must pass before
the next subtask starts. `cargo` commands need `--manifest-path crates/qext/Cargo.toml`
or `src-tauri/Cargo.toml`. Frontend changes go through `swe-frontend` (browser
verification), not inline.

---

## Phase 0 — Baseline (gate)

- [ ] **0.1** Confirm green baseline before any edit.
  - Verify: `cargo test --manifest-path crates/qext/Cargo.toml --features real-packages` → 273 + 6 pass; `cargo check --manifest-path src-tauri/Cargo.toml` clean; `npx tsc --noEmit` 0 errors.
  - Stop if red.

---

## Phase 1 — Contract fixes (low risk, no new behaviour)

Goal: align documented constants/vocabulary with the host so packages written
against the docs behave as their authors expect.

### 1.1 HTTP timeout
- [ ] **1.1.1** Change `DEFAULT_TIMEOUT_SECONDS` from 15 to 30 in `crates/qext/src/runtime/http.rs:33`.
- [ ] **1.1.2** Update the test that asserts the 250 ms transport timeout (http.rs:732) — that one is a *test* timeout and must stay short; do not touch it. Only the production default changes.
- [ ] **1.1.3** Update `EXTENSIONS.md` "HTTP Timeout" note and `docs/audit-report.md` 5.3 if present.
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib http::` green; `cargo clippy --manifest-path crates/qext/Cargo.toml --lib` clean.

### 1.2 Standard error_type vocabulary
- [ ] **1.2.1** Add variants to `DownloadFailureKind` (`adapter.rs:343-361`): `GeoBlocked`, `StreamError`, `FormatError`, `QuotaExceeded`, `PremiumRequired`, `ServerError`. Each gets an `as_str()` arm matching the doc's snake_case code (`geo_blocked`, `stream_error`, `format_error`, `quota_exceeded`, `premium_required`, `server_error`).
- [ ] **1.2.2** Add matching arms to `from_hint` (adapter.rs:362-383) so packages returning those codes map to the typed variant instead of `Other`.
- [ ] **1.2.3** Update `docs/audit-report.md` 5.3 error-vocabulary note.
- Verify: add a unit test in adapter.rs asserting `from_hint(Some("geo_blocked")) == DownloadFailureKind::GeoBlocked` (and the other five); `cargo test --manifest-path crates/qext/Cargo.toml --lib adapter::` green.

### 1.3 Manifest field `skipBuiltInFallback`
- [ ] **1.3.1** Add `pub skip_built_in_fallback: bool` to `Manifest` in `crates/qext/src/manifest.rs` (next to `skip_metadata_enrichment:78`), with `#[serde(default, rename = "skipBuiltInFallback")]`.
- [ ] **1.3.2** Add a parse test in manifest tests (mirror the existing skipMetadataEnrichment test).
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib manifest::` green.

---

## Phase 2 — Desktop download enrichment (medium risk)

Goal: run the documented `enrichTrack` flow so extension tracks can resolve to
tidal_id/qobuz_id/deezer_id/isrc before download.

- [ ] **2.1** In `src-tauri/src/ext_commands.rs::run_job`, after `activate_for_download` succeeds and before building `DownloadRequest`, call `store.with_runtime(&provider, |runtime| calls.enrich(...))` when the manifest declares `enrichTrack` (capability check via `Capabilities::from_runtime(runtime).has("enrichTrack")`). Merge the returned `Track` fields into the request metadata.
- [ ] **2.2** Extend `DownloadRequest::with_metadata` usage: pass the enriched track (not the raw job) so `request.metadata` reflects ISRC/tidal_id/qobuz_id.
- [ ] **2.3** Add a `--features`-gated integration test in `crates/qext/tests/` exercising `Calls::enrich` end-to-end (mirror existing real-packages test style) — or, if no fixture package implements enrichTrack, add a unit test in calls.rs with a synthetic `registerExtension({ enrichTrack: ... })` source.
- [ ] **2.4** Document in `EXTENSIONS.md` that enrichment now runs in desktop mode.
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib calls::` green; `cargo check --manifest-path src-tauri/Cargo.toml` clean; `cargo clippy --manifest-path src-tauri/Cargo.toml` clean.

---

## Phase 3 — Host-side dispatch for declared package functions

Goal: packages that declare `postProcessing`, `handleUrl`, `getHomeFeed`,
`getBrowseCategories`, `matchTrack` get a host that actually calls them.

### 3.1 postProcess / postProcessV2
- [ ] **3.1.1** Add `Calls::post_process(&self, input, metadata, hook_id) -> Result<Option<Resolved<Value>>, CallError>` in `calls.rs`, supporting both `postProcessV2` (preferred) and `postProcess` (fallback), mirroring the lyrics arity handling.
- [ ] **3.1.2** In `run_job`, after a successful download+relocate and before tagging, if the manifest's `post_processing.is_active()`, iterate enabled hooks whose `supports(format)` and call `post_process`; on success use `new_file_path` for the subsequent embed/relocate step.
- [ ] **3.1.3** Add unit tests in calls.rs (synthetic source) for both v1 and v2 dispatch.
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib calls::` green.

### 3.2 handleUrl
- [ ] **3.2.1** Add Tauri command `extensions_handle_url(ext_state, url) -> Result<Value, String>` in `ext_commands.rs` that scans installed enabled providers, calls `calls.handle_url(url)` on the first that implements it, returns the resolved link.
- [ ] **3.2.2** Register the command in `lib.rs` invoke_handler.
- [ ] **3.2.3** (Frontend, via `swe-frontend`) wire a route/handler so pasted URLs dispatch to it. Verify in a real browser.
- Verify: `cargo check src-tauri` clean; `npx tsc --noEmit` 0; browser smoke test.

### 3.3 getHomeFeed / getBrowseCategories
- [ ] **3.3.1** Add Tauri commands `provider_home_feed(ext_state, id)` and `provider_browse_categories(ext_state, id)`.
- [ ] **3.3.2** Register in `lib.rs`.
- [ ] **3.3.3** (Frontend, via `swe-frontend`) render home-feed sections when an extension declares `capabilities.homeFeed`. Browser verification.
- Verify: same as 3.2.

### 3.4 matchTrack in the download path
- [ ] **3.4.1** Where the desktop download flow resolves a track (currently the worker receives `trackId` from the queued job), if the provider implements `matchTrack`, call it to refine the id before `getDownloadUrl`. Gate behind manifest `trackMatching.customMatching`.
- [ ] **3.4.2** Unit test in calls.rs.
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib calls::` green.

---

## Phase 4 — Stubbed host APIs (high effort, optional)

These are documented but currently `__qext_unavailable`. Each is a self-contained
sub-project; tackle only if a package you ship needs it.

### 4.1 credentials (encrypted storage)
- [ ] **4.1.1** Design: where does the encryption key live? (OS keyring via `keyring` crate, or a key derived from a machine id + a salt in the ext root.) Document the choice in an ADR before coding.
- [ ] **4.1.2** Implement `credentials.store/get/has/remove` in polyfills.rs backed by AES-GCM over `<ext_data>/.credentials.enc`, matching the docs' "unlimited, encrypted" contract.
- [ ] **4.1.3** Add tests: round-trip, wrong-key failure, tamper detection.
- Verify: `cargo test --manifest-path crates/qext/Cargo.toml --lib` green.

### 4.2 auth (OAuth/PKCE)
- [ ] **4.2.1** Design: the desktop app already has an OAuth callback route? Check `src-tauri` for deep-link handling. If absent, this is a multi-week feature — defer.
- [ ] **4.2.2** Implement `auth.openAuthUrl/getAuthCode/setAuthCode/isAuthenticated/getTokens/clearAuth` + PKCE helpers (`generatePKCE`, `startOAuthWithPKCE`, `exchangeCodeWithPKCE`) backed by the desktop's HTTP client and a callback channel.
- [ ] **4.2.3** Tests: PKCE verifier/challenge round-trip, token expiry.
- Verify: manual OAuth flow against a test provider.

### 4.3 ffmpeg.getInfo
- [ ] **4.3.1** Implement via `ffprobe` (if bundled) or a minimal container parser for FLAC/MP3/M4A (bit_depth, sample_rate, duration). Document which.
- [ ] **4.3.2** Tests with fixture files.

### 4.4 gobackend.getAudioQuality / getLyricsLRC / checkISRCExists
- [ ] **4.4.1** `getAudioQuality` → delegate to 4.3's parser (or return the same `{error}` if unavailable, documented).
- [ ] **4.4.2** `getLyricsLRC` and `checkISRCExists` → decide: implement against a lyrics/ISRC service, or document as intentionally unavailable (current behaviour). Recommend documenting as unavailable unless a concrete package needs them.

---

## Phase 5 — Verification & docs

- [ ] **5.1** Re-run full suite: `cargo test --manifest-path crates/qext/Cargo.toml --features real-packages` (expect 273+ + new), `cargo test --manifest-path src-tauri/Cargo.toml` (if any), `npx tsc --noEmit`, `cargo clippy` both crates all-targets.
- [ ] **5.2** Update `EXTENSIONS.md`: HTTP timeout 30 s, new error_type codes, enrichment runs in desktop mode, postProcess dispatch, handleUrl/homeFeed wiring, stubbed-API status.
- [ ] **5.3** Update `docs/audit-report.md` 5.3: move closed items out of "Remaining Gaps"; keep `extensions_invoke_action` as the one open command.
- [ ] **5.4** Browser verification (via `swe-frontend`) of any new UI: handleUrl route, home-feed screen. Screenshots required as evidence.
- [ ] **5.5** Ask before commit/push (standing rule).

---

## Out of scope (explicitly)

- `extensions_invoke_action` has no package-side counterpart (manifest defines no named actions) — leave as "noted" unless the manifest spec gains an `actions` field.
- Commercial-service providers (amazon, tidal-web) cannot be tested end-to-end without live credentials; transfer features are covered by offline unit tests only.
- Cross-platform (mobile) host — this plan targets the desktop `qext` + Tauri layer only.
