# Q-Music Extension Host — Tasks

Execute in order. Each task is one fresh subagent's whole brief. A task is done only when its
**Verify.** line passes.

Fixture set in `crates/qext/fixtures/`: `spotify-web.sflx` (35681 bytes, sha256
`5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad`), `amazon.sflx`,
`tidal-web.sflx`, their unpacked `index.js` + `manifest.json`, and `registry.json`.
The three `index.js` files are the behavioural oracle — they are what actually ships.

Source of truth order: **package bytes > this plan > upstream docs.** Where the guide and a
package disagree, follow the package and record the divergence.

---

## P0 — Baseline and foundation

### T-0 Baseline gate

**Build.** Record the starting state of the tree. No product code.

**Acceptance criteria.**
- `cargo test -p qcore` passes with the known 40-test count.
- `cargo check --manifest-path src-tauri/Cargo.toml` is clean.
- `npx tsc --noEmit` and `npx vite build` are green.
- Any pre-existing failure is written into `plans/baseline.md` with the exact command and first
  failing assertion.

**Verify.** All four commands above, results recorded in `plans/baseline.md`. Red baseline stops
the loop unless this task is the fix.

### T-1 `qext` crate and manifest schema

**Build.** Create `crates/qext` with `manifest.rs`. Model every field observed across the three
real manifests: `name, displayName, version, description, author, homepage, icon, type[],
permissions{network[],storage,file}, settings[], qualityOptions[], capabilities, serviceHealth[],
searchBehavior, urlHandler, trackMatching, postProcessing, signedSession,
requiredRuntimeFeatures[], minAppVersion, skipMetadataEnrichment`.

Unknown fields are captured via `#[serde(flatten)] extra` and never rejected — forward
compatibility with newer packages is a requirement, not a nicety. Validation returns typed
errors naming the field.

**Acceptance criteria.**
- All three fixture manifests parse without warnings.
- A manifest missing `name` or `version` is rejected with an error naming the field.
- A manifest with an unknown top-level key parses and round-trips it unchanged.
- `type[]` accepts `metadata_provider`, `download_provider`, `lyrics_provider`; an unknown type
  string is preserved rather than panicking.
- No `unwrap()` on parsed input paths.

**Verify.** `cargo test -p qext manifest::`

### T-2 Package reader with archive safety

**Build.** `package.rs`: open `.sflx` / `.spotiflac-ext`, compute SHA-256, validate entry names,
extract to a staging directory, require `manifest.json` and `index.js` at root.

Reject: absolute paths, drive prefixes, `..` after normalisation, symlink entries, duplicate
normalised names, any entry over 64 MiB, total over 256 MiB, compression ratio over 200:1.
Limits are named constants, not literals.

**Acceptance criteria.**
- Real `spotify-web.sflx` extracts; computed SHA-256 equals `5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad`.
- Each rejection rule has its own synthesised archive fixture and its own test.
- A rejected archive leaves no partial staging directory behind.
- Extraction never follows a symlink out of the destination.

**Verify.** `cargo test -p qext package::`

### T-3 Contract types

**Build.** `contract.rs`: `Track, Album, Artist, LyricsResult, LyricsLine, DownloadResult,
QualityOption, CheckAvailability`. Field names match the shipped snake_case (`album_name`,
`duration_ms`, `images`, `error_message`).

Tolerant parsing where packages vary: `images` and `artists` accept string or array;
`duration_ms` accepts number or numeric string; missing optional fields default.

**Acceptance criteria.**
- Each type deserialises from a real response sample copied out of the fixture `index.js`.
- `images: "url"` and `images: ["url"]` both land as a one-element vector.
- Malformed input yields a typed error, never a panic.
- `DownloadResult` accepts `error`, `error_message`, and `message` as the failure text.

**Verify.** `cargo test -p qext contract::`

---

## P1 — Runtime

### T-4 QuickJS worker runtime

**Build.** `runtime/mod.rs`: one OS thread per extension owning `rquickjs::Runtime` +
`Context`. `set_memory_limit(128 MiB)`, `set_max_stack_size(4 MiB)`, `set_interrupt_handler`
enforcing a per-call deadline. Channel API: `Call { function, args, deadline }` / `Reply`.

Contexts are created lazily on first use and torn down on disable or uninstall. Nothing is
created at app boot.

**Acceptance criteria.**
- All three fixture `index.js` files load and report the registered key sets recorded in
  `plans/project-overview.md`.
- No network call fires during load.
- An infinite loop in extension code is interrupted at the deadline with a typed timeout error.
- A 512 MiB allocation attempt fails inside the sandbox without killing the host process.
- Creating and dropping a runtime releases its thread.

**Verify.** `cargo test -p qext runtime::`

### T-5 Permission enforcement

**Build.** `runtime/permissions.rs`: network allowlist with exact-host and `*.suffix` matching,
plus sandboxed path resolution. Every denial names the permission and the offending value.

**Acceptance criteria.**
- `tidal.com` allowed where declared; `evil.tidal.com` denied; `*.audio.tidal.com` allows
  `a.audio.tidal.com` and denies `audio.tidal.com`.
- Wildcard matching never widens to a different registrable domain (`*.tidal.com` must not allow
  `tidal.com.evil.net`).
- `../../etc/passwd`, `C:\Windows\x`, `\\server\share`, and `..%2f` all denied.
- A relative path resolves inside the extension data dir; a canonicalised prefix check catches
  escapes after symlink resolution.
- An absolute download target is allowed only when the host issued it explicitly.

**Verify.** `cargo test -p qext permissions::`

### T-6 Host HTTP object

**Build.** `runtime/http.rs`: blocking client, per-extension cookie jar, redirect following
(max 10, each hop re-checked against the allowlist), timeout from
`capabilities.networkTimeoutSeconds` (default 15 s). Response object exactly
`{statusCode, status, ok, body, headers}` with multi-value headers as arrays. `http.request`
supports any method, object bodies serialise to JSON, `clearCookies()` empties the jar.

**Acceptance criteria.**
- 200 JSON, 404, and a redirect chain each return the final status.
- A redirect to a non-allowlisted host is refused with the host named.
- Cookie set on request 1 is echoed on request 2 and absent after `clearCookies()`.
- A slow endpoint raises the typed timeout, not a hang.
- Response headers expose repeated `Set-Cookie` values as an array.

**Verify.** `cargo test -p qext http::` against the local fixture server.

### T-7 Polyfills and utilities

**Build.** `runtime/polyfills.rs` and `runtime/crypto.rs`: synchronous `fetch` returning
`{ok,status,statusText,url,headers,text(),json(),arrayBuffer()}`; `atob`, `btoa`, `TextEncoder`,
`TextDecoder`, `URL`, `URLSearchParams`.

`utils`: `parseJSON, stringifyJSON, base64Encode, base64Decode, md5, sha256, hmacSHA256,
hmacSHA256Base64, hmacSHA1, encrypt, decrypt, generateKey`.
`storage`, `file`, `log`, `gobackend` (`getLocalTime` with a real IANA timezone,
`sanitizeFilename`, `buildFilename`, `getAudioQuality`), `matching`
(`compareStrings`, `compareDuration`, `normalizeString`).

**Acceptance criteria.**
- `sha256("abc")`, `md5("abc")` and an HMAC-SHA256 vector match published test vectors.
- `btoa`/`atob` round-trip across non-ASCII input.
- `URLSearchParams` serialises the way the guide's OAuth example expects.
- The guide's TOTP example produces the same six-digit code as an independent computation.
- `gobackend.getLocalTime()` returns the machine's real timezone name, not `"Local"` and not a
  zero offset.
- `storage` persists across runtime teardown and reload.

**Verify.** `cargo test -p qext utils:: polyfills::`

### T-8 Compatibility adapter

**Build.** `adapter.rs`: dispatch `download` by `Function.length` so a 2-argument implementation
still works; read `error` / `error_message` / `message`; coerce `images` / `artists` to arrays;
coerce lyric times to `u64`; expose only functions a package actually registered.

**Acceptance criteria.**
- A 5-argument `download` is called with `(trackID, quality, outputPath, onProgress, options)`.
- A 2-argument `download` is called with `(request, progressCallback)` and still succeeds.
- Each failure key shape produces the same normalised error.
- An extension registering only `initialize` reports no search capability instead of erroring.
- Adapter unit tests cover every registered key set found in the three fixtures.

**Verify.** `cargo test -p qext adapter::`

---

## P2 — Providers

### T-9 Capability negotiation

**Build.** A static host feature table compared against `requiredRuntimeFeatures` at install.
Unmet features return a list, each named. `signedSession@3`, `sessionGrant@1` and
`downloadSegments@1` are deliberately absent from the table and must be named individually in
the refusal.

**Acceptance criteria.**
- `tidal-web` fixture is refused with exactly those three feature names.
- `spotify-web` and `apple-music` install with no refusal.
- A future unknown feature string is refused and reported verbatim, not silently ignored.
- The refusal reason is machine-readable, so the UI can render it without string matching.

**Verify.** `cargo test -p qext negotiate::`

### T-10 Provider registry, priority and quality

**Build.** `providers.rs`: installed providers grouped by capability, priority persisted in
settings, health tracking (3 consecutive failures marks unhealthy), `qualityOptions[]` surfaced
with per-quality settings overrides, `settings[]` defaults resolved.

**Acceptance criteria.**
- Selection is deterministic and respects priority order.
- An unhealthy provider is skipped; a retry clears the health flag.
- Reordering priority changes the winner without a restart.
- A quality-specific setting override merges without clobbering global settings.
- A provider with no `qualityOptions` yields a single default option.

**Verify.** `cargo test -p qext providers::`

### T-11 Metadata, search and lyrics calls

**Build.** Rust API over `searchTracks` / `customSearch`, `getTrack` / `getAlbum` / `getArtist` /
`getPlaylist`, `getHomeFeed`, `getBrowseCategories`, `handleUrl`, `enrichTrack`, `matchTrack`,
`fetchLyrics`. Results cached in SQLite keyed by provider plus query. Failures return typed
errors and never poison the cache.

**Acceptance criteria.**
- Every call round-trips through a fixture provider returning canned JSON.
- A cache hit avoids a second runtime call.
- A throwing provider yields its error and does not affect a second provider in the same query.
- `fetchLyrics` normalises synced, unsynced and instrumental results into one type.
- Results carry their provider id so the UI can render a source chip.

**Verify.** `cargo test -p qext calls::`

---

## P3 — Downloads and embedding

### T-12 Queue

**Build.** `queue.rs`: job model with states `queued | resolving | downloading |
post_processing | embedding | done | failed | cancelled`, job id, optional parent album id, byte
counters, typed error. Cancel is honoured in every state and removes the partial file.
Concurrency is configurable, default 2.

**Acceptance criteria.**
- Every transition is covered by a state-machine test.
- Cancel mid-download leaves no file and no orphan temp file.
- Cancelling an album job cancels its remaining children and keeps completed tracks.
- Progress events are throttled to 10 Hz, not one per chunk.
- A failed job records its typed error and does not block the queue.

**Verify.** `cargo test -p qext queue::`

### T-13 Streamed download executor

**Build.** Chunked HTTP into a `.part` file, atomic rename on completion, throttled progress
callbacks, resume from byte offset when the provider reports a known size.

**Acceptance criteria.**
- A 10 MB fixture payload completes with monotonic progress.
- Cancel at 50% leaves nothing on disk.
- A server dropping mid-stream fails with `download_error` and removes the partial.
- Resume from a supplied offset yields a byte-identical file.
- Concurrent jobs never write the same temp path.

**Verify.** `cargo test -p qext download::`

### T-14 Post-processing hooks

**Build.** `postProcess` / `postProcessV2` invocation after download and before embedding.
`ffmpeg` proxies to a configured binary only; when absent, a call returns a typed `unsupported`
error instead of crashing.

**Acceptance criteria.**
- A fixture package performing a no-op post-process completes and reports success.
- `ffmpeg.execute` without a configured binary returns the typed error and the job fails cleanly.
- Post-process failure leaves the downloaded file on disk and marks the job failed with the
  stage recorded.

**Verify.** `cargo test -p qext postprocess::`

### T-15 Embedding

**Build.** `crates/qcore/src/embed.rs`: write title, artist, album, album_artist, track, disc,
year, genre and isrc via Lofty for FLAC, MP3, M4A, OGG and OPUS; embed artwork; embed synced
lyrics (FLAC `LYRICS`/`UNSYNCEDLYRICS`, ID3 `USLT`/`SYLT`); write a sidecar `.lrc` when synced.

**Acceptance criteria.**
- Round-trip per format: write, then read back with the existing tag reader, values equal.
- Artwork byte length matches the source.
- `.lrc` output parses with the existing LRC parser and line count matches.
- Writing to a read-only file returns a typed error, not a panic.
- `cargo test -p qcore` stays green, including the original 40 tests.

**Verify.** `cargo test -p qcore embed::` plus the full `cargo test -p qcore`.

---

## P4 — Store and lifecycle

### T-16 Registry client

**Build.** `registry.rs`: fetch `registry.json`, parse semver, compare `minAppVersion` against
the host version, expose entries with supported / unsupported status and reason.

**Acceptance criteria.**
- The fixture registry parses all 12 entries with ids, versions and SHA-256s intact.
- `min_app_version` 9.0.0 refuses; equal version refuses an update; a higher version offers one.
- An unparseable version string is reported against the entry, not silently dropped.
- Network failure returns a typed error and leaves the previous cached registry usable.

**Verify.** `cargo test -p qext registry::`

### T-17 Install, update and uninstall

**Build.** `store.rs`: install from registry or local file, staged extraction, atomic enable,
downgrade refusal, update only when strictly newer, uninstall clearing package and data dir,
disable without uninstall.

**Acceptance criteria.**
- Install then uninstall leaves no directory behind.
- Downgrading an installed 1.10.4 to 1.10.3 is refused with a typed error.
- An interrupted install leaves no half-enabled extension; the prior version stays active.
- Uninstall tears the runtime down before deleting files.
- No path outside the extensions root is ever written.

**Verify.** `cargo test -p qext store::`

---

## P5 — UI

### T-18 Extensions view

**Build.** Catalogue grid from the registry with status, refusal reason, and install progress on
the card. Local file install. Empty and error states in the existing design language.

**Acceptance criteria.**
- Renders all 12 fixture entries with type badge and version.
- Unsupported entries show the named missing feature inline.
- Install progress animates on the card, not in a modal.
- Works from the demo backend with no Rust host present.
- Screenshots at 1440x900 and 960x620 in all three themes; no text below 4.5:1 contrast.

**Verify.** `npx tsc --noEmit`, `npx vite build`, `node scripts/shoot.mjs`, plus contrast check.

### T-19 Settings sheet from `settings[]`

**Build.** Generated controls for text, number, boolean, select and button; per-quality
overrides; save calls `initialize`. No per-extension code paths.

**Acceptance criteria.**
- Each setting type renders from a synthetic manifest.
- Save round-trips through the demo backend and survives reload.
- A button setting invokes its named registered action and surfaces success or failure.
- Keyboard-only operation reaches every control in order.
- Screenshots both widths.

**Verify.** `npx tsc --noEmit`, screenshots at 1440x900 and 960x620.

### T-20 Provider priority and health

**Build.** Reorderable per-capability list, unhealthy badge, retry action.

**Acceptance criteria.**
- Reorder persists across reload in the demo backend.
- Keyboard reordering works and announces the new position.
- Unhealthy badge clears after a successful retry.
- Screenshots both widths.

**Verify.** `npx tsc --noEmit`, screenshots.

### T-21 Queue drawer

**Build.** Live jobs with stage, percent, bytes, speed, cancel, typed error text and retry.

**Acceptance criteria.**
- The demo backend drives one job through every state and the drawer reflects each.
- Cancel removes the row and its partial state.
- `prefers-reduced-motion` suppresses the progress animation.
- Screenshots both widths.

**Verify.** `npx tsc --noEmit`, screenshots.

### T-22 Search and lyrics integration

**Build.** Provider results merged into the search view under a source chip, local results
first. A lyrics provider reachable from the Lyrics pane.

**Acceptance criteria.**
- The demo backend returns mixed local and provider results; local renders first.
- A provider timeout renders a quiet inline message without delaying local results.
- Lyrics fetched from a provider render with the provider name attached.
- Screenshots both widths.

**Verify.** `npx tsc --noEmit`, screenshots.

---

## P6 — Verification and documentation

### T-23 Compatibility matrix

**Build.** `EXTENSIONS.md`: all 12 registry entries with status (supported / refused with named
reason / unimplemented feature), the documented-versus-shipped divergences, and the exact
function surface recognised per package.

**Acceptance criteria.**
- Every claim traces to a fixture file or a named test.
- The four known divergences are listed with both shapes.
- The unsupported-feature section states plainly what is not implemented and why.
- No version numbers or hashes are typed from memory — each is quoted from a fetched artefact.

**Verify.** `EXTENSIONS.md` exists; every cited test name exists in the tree.

### T-24 End-to-end with real packages

**Build.** Load all five integration packages from real bytes, run `initialize`, and exercise
offline paths (`handleUrl` parsing, adapter dispatch) with no network. Live network calls stay in
a separate opt-in test, never in the default suite.

**Acceptance criteria.**
- `cargo test -p qext --features real-packages` passes with the network disabled.
- All five packages register the expected key sets.
- A package whose `requiredRuntimeFeatures` are unmet is refused, not loaded.
- The default `cargo test -p qext` stays network-free and green.

**Verify.** `cargo test -p qext`, then `cargo test -p qext --features real-packages`.

### T-25 Security documentation

**Build.** `SECURITY.md`: the threat table from `project-overview.md` expanded with the concrete
control and the test that proves it.

**Acceptance criteria.**
- Every row cites a test name that exists and passes.
- The extension trust model is stated explicitly: packages are untrusted code.
- The residual-risk section names what the host deliberately does not defend against.
- No claim in the document exceeds what the tests demonstrate.

**Verify.** `SECURITY.md` exists; every cited test name resolves.

---

## Out of scope, recorded

`signedSession@3`, `sessionGrant@1`, `downloadSegments@1`, and the seven `download`-category
commercial-service rippers. Not implemented, not exercised against live services, reported as
unsupported in T-23.
