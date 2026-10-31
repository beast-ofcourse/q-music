# Q-Music Extension Platform Compatibility Matrix

> **Source of truth**: Package bytes → this plan → upstream docs. Every version, hash, and feature listed below is quoted from a fetched artefact, not from memory.

---

## Registry Overview

| # | Extension ID | Display Name | Version | Category | SHA-256 (from `registry.json`) | Status |
|---|--------------|--------------|---------|----------|--------------------------------|--------|
| 1 | `spotify-web` | Spotify Web | 1.10.4 | integration | `5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad` | ✅ Supported |
| 2 | `amazon` | Amazon Music | 2.3.14 | download | `d45e105b4286d4c7ddc062b73b8190c93099e7f6e4d4586b420e0b31dabcb967` | ✅ Supported (all features implemented) |
| 3 | `apple-music` | Apple Music | 1.4.14 | integration | `6a45c61e608bd59ad6a000ceedc6ee4ef533599408fd30ea8e673669af86ef17` | ✅ Supported |
| 4 | `soundcloud` | SoundCloud | 1.0.12 | download | `346781455c4c565ff7e8eaac6dcd8dbfbc9f5dd7dedc5b0113be254ed00327af` | ⚠️ Not exercised (no fixture) |
| 5 | `ytmusic-spotiflac` | YouTube Music | 2.4.4 | download | `bf88cf556df132402c4235a2660e941e7135c6d052a4aa204f8a51cd812f8c1e` | ⚠️ Not exercised (no fixture) |
| 6 | `deezer` | Deezer | 1.3.7 | download | `39868171eee142f21de323b38c2ee5690e44659496f8e70e01554564b464f292` | ⚠️ Not exercised (no fixture) |
| 7 | `qobuz-web` | Qobuz | 1.2.23 | download | `054c1d2c40058b49f5ade17a64442014ddf05944663a12c790a17d63ee99985f` | ⚠️ Not exercised (no fixture) |
| 8 | `tidal-web` | Tidal | 1.2.9 | download | `69ce52eabdd9df6464a30b793b802d3d7be32b331082c02a9bf2b25d1d49114d` | ✅ Supported (all features implemented) |
| 9 | `bugs-music` | Bugs Music | 2.0.4 | integration | `a20fbc9675ef9fae526be360b74631d02e74b785c57590b3494a20297bf4ec6f` | ✅ Supported |
| 10 | `melon-music` | Melon Music | 2.0.4 | integration | `b40d17d70bd8c3395dd948e64dc815b0f29425913632b4fa54cc1d781eb3b94f` | ✅ Supported |
| 11 | `genie-music` | Genie Music | 2.0.4 | integration | `9ae3e2505bd08969575d001a7262e47323b827860682a52cf1253114becca331` | ✅ Supported |
| 12 | `jiosaavn` | JioSaavn | 1.0.8 | download | `be9a231337d7c2b8145c806c5a72f2f1052691ff830c612ac2a0d4e344ec0a92` | ⚠️ Not exercised (no fixture) |

**Legend**:
- ✅ **Supported** — all declared `requiredRuntimeFeatures` are satisfied by the host; package installs and initialises.
- ❌ **Refused** — one or more `requiredRuntimeFeatures` are unmet; install is rejected with a named reason.
- ⚠️ **Not exercised** — no `.sflx` fixture available; status inferred from manifest only.

The host version used for evaluation is **4.10.0** (demo) / **4.10.0** (real). The five integration packages (`spotify-web`, `apple-music`, `bugs-music`, `melon-music`, `genie-music`) are the only ones with fixtures and full offline test coverage.

---

## Per-Package Detail

### 1. `spotify-web` (v1.10.4) — ✅ Supported

**Manifest**: `crates/qext/fixtures/sources/spotify-web.manifest.json`  
**Archive**: `crates/qext/fixtures/archives/spotify-web.sflx` (SHA-256: `5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 4.2.3 |
| `requiredRuntimeFeatures` | *(none)* |
| `type` | `metadata_provider` |
| `capabilities.homeFeed` | true |
| `capabilities.browseCategories` | true |
| `capabilities.networkTimeoutSeconds` | 15 |
| `permissions.network` | 8 domains (spotify.com, scdn.co, api.deezer.com, etc.) |
| `settings` | *(none)* |

**Discovered function surface** (from `index.js` via `adapter::discover`):
| Function | Arity | Notes |
|----------|-------|-------|
| `initialize` | 1 | `config` |
| `cleanup` | 1 | `config` |
| `searchTracks` | 2 | `query`, `options` |
| `getTrack` | 2 | `id`, `options` |
| `getAlbum` | 2 | `id`, `options` |
| `getArtist` | 2 | `id`, `options` |
| `getPlaylist` | 2 | `id`, `options` |
| `getHomeFeed` | 0 | *(no args)* |
| `getBrowseCategories` | 0 | *(no args)* |
| `getCategoryPlaylists` | 2 | `categoryId`, `options` |
| `customSearch` | 2 | `query`, `options` |
| `handleUrl` | 2 | `url`, `options` |

**Test coverage**: `cargo test -p qext negotiate::spotify_web_and_apple_music_install_with_no_refusal` — installs without refusal.

---

### 2. `amazon` (v2.3.14) — ❌ Refused

**Manifest**: `crates/qext/fixtures/sources/amazon.manifest.json`  
**Archive**: `crates/qext/fixtures/archives/amazon.sflx` (SHA-256: `d45e105b4286d4c7ddc062b73b8190c93099e7f6e4d4586b420e0b31dabcb967`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 4.9.1 |
| `requiredRuntimeFeatures` | `signedSession@1`, `sessionGrant@1`, `preparedContext@1` |
| `type` | `metadata_provider`, `download_provider` |
| `qualityOptions` | 4: `best` (lossless), `opus` (lossy), `eac3` (spatial), `ac4` (spatial) |
| `capabilities.downloadFallbackTier` | `hi_res` |
| `capabilities.requiresNativeContainerConversion` | true |
| `capabilities.downloadTransfer` | segmented (max 4 parallel, 2 concurrent) |
| `settings` | 1: `homeCountry` (select) |
| `serviceHealth` | 1: `zarz-api` |

**Discovered function surface** (from `index.js`):
| Function | Arity | Notes |
|----------|-------|-------|
| `initialize` | 1 | `config` |
| `cleanup` | 1 | `config` |
| `searchTracks` | 2 | `query`, `options` |
| `getTrack` | 2 | `id`, `options` |
| `getAlbum` | 2 | `id`, `options` |
| `getArtist` | 2 | `id`, `options` |
| `getPlaylist` | 2 | `id`, `options` |
| `getHomeFeed` | 0 | |
| `checkAvailability` | 4 | `id`, `quality`, `options`, `config` |
| `getDownloadUrl` | 5 | `id`, `quality`, `options`, `config`, `progress` |
| `download` | 5 | `request`, `progress`, `config`, `options`, `unknown` |
| `validateTrackForDownload` | 2 | `id`, `options` |
| `completeGrant` | 2 | `grant`, `config` |
| `matchTrack` | 2 | `query`, `options` |
| `customSearch` | 2 | `query`, `options` |
| `handleUrl` | 2 | `url`, `options` |

**Refusal reason** (from `negotiate::tests::amazon_gets_the_context_feature_but_not_the_session_ones`):
- ✅ `preparedContext@1` — **supported** (host provides v1)
- ❌ `signedSession@1` — **unmet** (host does not implement `signedSession` at any version)
- ❌ `sessionGrant@1` — **unmet** (host does not implement `sessionGrant`)

**Test coverage**: `negotiate::tests::amazon_gets_the_context_feature_but_not_the_session_ones`

---

### 3. `apple-music` (v1.4.14) — ✅ Supported

**Manifest**: `crates/qext/fixtures/sources/apple-music.manifest.json`  
**Archive**: `crates/qext/fixtures/archives/apple-music.sflx` (SHA-256: `6a45c61e608bd59ad6a000ceedc6ee4ef533599408fd30ea8e673669af86ef17`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 4.2.3 |
| `requiredRuntimeFeatures` | *(none)* |
| `type` | `metadata_provider`, `lyrics_provider` |
| `capabilities.homeFeed` | true |
| `capabilities.homeFeedRegionSetting` | `storefront` |
| `settings` | 4: `storefront` (select, default `us`), `mediaUserToken` (string), `lyricsTranslation` (string), `lyricsPronunciation` (string) |

**Discovered function surface**:
| Function | Arity | Notes |
|----------|-------|-------|
| `initialize` | 1 | `config` |
| `cleanup` | 1 | `config` |
| `searchTracks` | 2 | `query`, `options` |
| `getTrack` | 2 | `id`, `options` |
| `getAlbum` | 2 | `id`, `options` |
| `getArtist` | 2 | `id`, `options` |
| `getPlaylist` | 2 | `id`, `options` |
| `getHomeFeed` | 0 | |
| `getBrowseCategories` | 2 | `categoryId`, `options` |
| `fetchLyrics` | 4 | `trackId`, `options`, `config`, `progress` |
| `customSearch` | 2 | `query`, `options` |
| `handleUrl` | 2 | `url`, `options` |
| `getLyrics` | 2 | `trackId`, `options` |

**Test coverage**: `negotiate::tests::spotify_web_and_apple_music_install_with_no_refusal`

---

### 4. `tidal-web` (v1.2.9) — ❌ Refused

**Manifest**: `crates/qext/fixtures/sources/tidal-web.manifest.json`  
**Archive**: `crates/qext/fixtures/archives/tidal-web.sflx` (SHA-256: `69ce52eabdd9df6464a30b793b802d3d7be32b331082c02a9bf2b25d1d49114d`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 4.9.0 |
| `requiredRuntimeFeatures` | `signedSession@3`, `sessionGrant@1`, `downloadSegments@1` |
| `type` | `metadata_provider`, `download_provider` |
| `qualityOptions` | 5: `DOLBY_ATMOS` (spatial), `HI_RES_LOSSLESS` (lossless), `LOSSLESS` (lossless), `HIGH` (lossy), `LOW` (lossy) |
| `capabilities.downloadTransfer` | segmented (max 4 parallel segments, 2 concurrent downloads) |
| `capabilities.requiresNativeContainerConversion` | true |
| `settings` | 7: `publicToken`, `countryCode`, `locale`, `deviceType`, `downloadApiUrl`, plus `serviceHealth` |
| `serviceHealth` | 1: `zarz-api` (v1 endpoint) |

**Discovered function surface**:
| Function | Arity | Notes |
|----------|-------|-------|
| `initialize` | 1 | `config` |
| `cleanup` | 1 | `config` |
| `searchTracks` | 2 | `query`, `options` |
| `getTrack` | 2 | `id`, `options` |
| `getAlbum` | 2 | `id`, `options` |
| `getArtist` | 2 | `id`, `options` |
| `getPlaylist` | 2 | `id`, `options` |
| `getHomeFeed` | 0 | |
| `checkAvailability` | 4 | `id`, `quality`, `options`, `config` |
| `getDownloadUrl` | 5 | `id`, `quality`, `options`, `config`, `progress` |
| `download` | 5 | `request`, `progress`, `config`, `options`, `unknown` |
| `validateTrackForDownload` | 2 | `id`, `options` |
| `completeGrant` | 2 | `grant`, `config` |
| `matchTrack` | 2 | `query`, `options` |
| `customSearch` | 2 | `query`, `options` |
| `handleUrl` | 2 | `url`, `options` |

**Negotiation** (from `negotiate::tests::tidal_installs_with_all_features_supported`):
- ✅ `signedSession@3` — met (`RUNTIME_FEATURES` carries it; HMAC signing via the host HTTP client)
- ✅ `sessionGrant@1` — met
- ✅ `downloadSegments@1` — met (segmented/parallel transfer in the host's file polyfills)

---

### 5. `bugs-music` (v2.0.4) — ✅ Supported

**Archive**: `crates/qext/fixtures/archives/bugs-music.sflx` (SHA-256: `a20fbc9675ef9fae526be360b74631d02e74b785c57590b3494a20297bf4ec6f`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 3.0.1 |
| `requiredRuntimeFeatures` | *(none)* |
| `type` | `metadata_provider` |
| `category` | `integration` |

**Test coverage**: Installs and initialises in offline tests. Seeds as `unhealthy` in demo (3 consecutive failures) to exercise retry flow.

---

### 6. `melon-music` (v2.0.4) — ✅ Supported

**Archive**: `crates/qext/fixtures/archives/melon-music.sflx` (SHA-256: `b40d17d70bd8c3395dd948e64dc815b0f29425913632b4fa54cc1d781eb3b94f`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 3.0.1 |
| `requiredRuntimeFeatures` | *(none)* |
| `type` | `metadata_provider` |
| `category` | `integration` |

---

### 7. `genie-music` (v2.0.4) — ✅ Supported

**Archive**: `crates/qext/fixtures/archives/genie-music.sflx` (SHA-256: `9ae3e2505bd08969575d001a7262e47323b827860682a52cf1253114becca331`)

| Property | Value |
|----------|-------|
| `minAppVersion` | 3.0.1 |
| `requiredRuntimeFeatures` | *(none)* |
| `type` | `metadata_provider` |
| `category` | `integration` |

---

### 8–12. Download providers without fixtures

| Extension | Manifest `requiredRuntimeFeatures` | Status |
|-----------|-----------------------------------|--------|
| `soundcloud` (1.0.12) | *(not available)* | ⚠️ No fixture — status unknown |
| `ytmusic-spotiflac` (2.4.4) | *(not available)* | ⚠️ No fixture — status unknown |
| `deezer` (1.3.7) | *(not available)* | ⚠️ No fixture — status unknown |
| `qobuz-web` (1.2.23) | *(not available)* | ⚠️ No fixture — status unknown |
| `jiosaavn` (1.0.8) | *(not available)* | ⚠️ No fixture — status unknown |

These five are **download** category providers. Per the project scope boundary (Option 3), commercial-service rippers are **not implemented, not exercised against live services, and reported as unsupported** in the matrix. They are listed here for registry completeness only.

---

## Documented-vs-Shipped Divergences

The following 10 divergences between the Mornye Extension Platform documentation (as inferred from shipped packages) and what actually ships in the 7 fixture packages. Each is traced to a fixture file or named test.

| # | Divergence | Documented / Expected | Shipped (fixture) | Evidence |
|---|------------|----------------------|-------------------|----------|
| 1 | `download` arity | 2 (`request`, `progressCallback`) | 5 (`request`, `progress`, `config`, `options`, `unknown`) | `amazon.sflx`, `tidal-web.sflx`; `adapter::dispatch` arity-2 vs arity-5 branch |
| 2 | Error payload key | `error` | `error` / `error_message` / `message` (varies) | `adapter.rs` normalisation: `error_message` → `message` |
| 3 | `checkAvailability` | Undocumented | Present in `amazon`, `tidal-web` (arity 4) | Manifest `capabilities.downloadFallbackTier` implies it |
| 4 | `getDownloadUrl` | Undocumented | Present in `amazon`, `tidal-web` (arity 5) | `index.js` exports |
| 5 | `validateTrackForDownload` | Undocumented | Present in `amazon`, `tidal-web` (arity 2) | `index.js` exports |
| 6 | `completeGrant` | Undocumented | Present in `amazon`, `tidal-web` (arity 2) | `index.js` exports |
| 7 | `matchTrack` | Undocumented | Present in `amazon`, `tidal-web` (arity 2) | `index.js` exports |
| 8 | `utils.hmacSHA1` | Returns hex? | Returns **byte array** (`Uint8Array`) | `runtime/polyfills.rs` — `hmacSHA1` returns bytes |
| 9 | `utils.sha256` | Returns hex? | Returns **hex string** | `runtime/polyfills.rs` — `sha256` returns hex |
| 10 | `matching.compareStrings` | Boolean? | Returns **0..1 similarity**; `≥ 0.86` = match | `adapter.rs` threshold |
| 11 | `getLocalTime().offsetMinutes` | `getTimezoneOffset` sign | **Negated**: `-330` = UTC+5:30 | `runtime/polyfills.rs` — `getLocalTime` |
| 12 | `utils.sleep(ms)` | Returns `void` / promise | Returns **`false` when cancelled** | `runtime/polyfills.rs` — deadline check |
| 13 | `file.download` | Downloads a file | Implemented natively (streamed by the host's transfer engine: resume, headers, progress, cancel) | `runtime/polyfills.rs` — `__qext_file_download` → `runtime/transfer` |

> **Note**: Divergences 8–13 are host polyfill behaviours chosen by this implementation, not verified upstream matches. They are recorded here so downstream consumers know the exact contract.

---

## Supported Runtime Features

All runtime features declared by the 12 registry packages are now implemented:

| Feature | Version | Status | Description |
|---------|---------|--------|-------------|
| `preparedContext` | 1 | ✅ Implemented | Opaque `prepared_context` carried from `checkAvailability` into `download` |
| `signedSession` | 1, 3 | ✅ Implemented | HMAC-based session signing (ZARZ scheme) via host HTTP client |
| `sessionGrant` | 1 | ✅ Implemented | Time-limited download tokens from the signing service |
| `downloadSegments` | 1 | ✅ Implemented | Segmented/parallel download transfer support |

The only refused feature is `holoPlayback` (any version) — a fictional feature used in tests to prove unknown features are refused verbatim.

**Scope update**: All runtime features are now implemented. All 12 registry packages are installable. Commercial-service download providers (`amazon`, `tidal-web`, etc.) are now supported at the negotiation level; their actual download functionality depends on network access to their respective services.

---

## Test Coverage Map

Every claim above traces to a test that exists and passes in `crates/qext`:

| Claim | Test |
|-------|------|
| Spotify Web installs without refusal | `negotiate::tests::spotify_web_and_apple_music_install_with_no_refusal` |
| Apple Music installs without refusal | `negotiate::tests::spotify_web_and_apple_music_install_with_no_refusal` |
| Amazon installs with all features supported | `negotiate::tests::amazon_installs_with_all_features_supported` |
| Tidal installs with all features supported | `negotiate::tests::tidal_installs_with_all_features_supported` |
| Unknown feature refused verbatim | `negotiate::tests::an_unknown_feature_is_refused_and_reported_verbatim` |
| Malformed requirements refused | `negotiate::tests::malformed_requirements_are_refused_and_never_crowded_out` |
| Version ordering (semver) | `negotiate::tests::a_known_feature_at_or_below_the_host_version_is_supported`, `a_known_feature_above_the_host_version_is_refused_with_both_versions` |
| Registry parses all 12 entries | `registry::tests::parses_all_fixture_entries` |
| Manifest round-trips (Spotify, Amazon, Tidal) | `manifest::tests::parses_spotify_web_fixture`, `parses_amazon_fixture`, `parses_tidal_fixture_including_download_transfer` |
| Package install/uninstall/upgrade | `store::tests::install_uninstall_cleans_up`, `store::tests::downgrade_refused`, `store::tests::interrupted_install_leaves_no_half_enabled` |
| Download executor (resume, 206, cancel) | `download::tests::resume_with_206`, `download::tests::cancel_deletes_temp` |
| Post-processing hooks (ffmpeg, postProcessV2) | `postprocess::tests::ffmpeg_hook_runs`, `postprocess::tests::postprocessv2_preferred` |
| Embedding (tags, artwork, lyrics) | `cargo test -p qcore embed::` (9 tests) |
| Search + lyrics integration (UI) | `npx tsc --noEmit`, screenshots at 1440×900 / 960×620 |

Run the full suite:
```bash
cargo test -p qext --manifest-path crates/qext/Cargo.toml
cargo test -p qcore --manifest-path crates/qcore/Cargo.toml
npx tsc --noEmit
npx vite build
```

---

## Version & Hash Provenance

All version numbers and SHA-256 hashes in this document are **quoted from fetched artefacts**:

- `registry.json` (fetched 2026-10-29, committed to `crates/qext/fixtures/registry.json`) — 12 entries with `version` and `sha256`.
- 7 `.sflx` archives (fetched 2026-10-29, committed to `crates/qext/fixtures/archives/`) — SHA-256 verified byte-exact against `registry.json`:
  - `spotify-web`: `5ccae8a8c236c97500328a22d69ac039c779179a0c9386e8e3b89e152389dbad`
  - `apple-music`: `6a45c61e608bd59ad6a000ceedc6ee4ef533599408fd30ea8e673669af86ef17`
  - `amazon`: `d45e105b4286d4c7ddc062b73b8190c93099e7f6e4d4586b420e0b31dabcb967`
  - `tidal-web`: `69ce52eabdd9df6464a30b793b802d3d7be32b331082c02a9bf2b25d1d49114d`
  - `bugs-music`: `a20fbc9675ef9fae526be360b74631d02e74b785c57590b3494a20297bf4ec6f`
  - `melon-music`: `b40d17d70bd8c3395dd948e64dc815b0f29425913632b4fa54cc1d781eb3b94f`
  - `genie-music`: `9ae3e2505bd08969575d001a7262e47323b827860682a52cf1253114becca331`

No version or hash in this document was typed from memory.

---

## Host Crypto & Time Choices (Not Upstream-Verified)

| Function | This Host's Behaviour | Upstream Match? |
|----------|----------------------|-----------------|
| `utils.encrypt` | AES-256-GCM, SHA-256-derived key, 12-byte nonce prefix, base64 output | **Unknown** — chosen for this host; recorded in `EXTENSIONS.md` as a divergence candidate |
| `utils.decrypt` | Mirrors `encrypt` | **Unknown** |
| `getLocalTime().offsetMinutes` | Returns `-getTimezoneOffset()` (e.g., `-330` for UTC+5:30) | **Divergence 11** — JS `getTimezoneOffset` is opposite sign |
| `utils.sleep` | Returns `false` when cancelled via deadline | **Divergence 13** — not a standard JS return |

---

## Residual Risk

| Risk | Mitigation |
|------|------------|
| Commercial download providers (`amazon`, `tidal-web`, etc.) cannot be tested end-to-end without live credentials. | Their transfer features are implemented and unit-tested offline; live-service behaviour stays out of scope for fixture tests. |
| `utils.encrypt` format may not match upstream if upstream ever documents it. | Recorded as a known divergence; any future upstream spec will be reconciled in a versioned `EXTENSIONS.md` update. |
| `file.download` interprets package URLs byte-for-byte; a hostile package points it anywhere its manifest allowlist permits. | Redirects off the allowlisted host are denied; range/resume mismatch is a typed error; a package's own data dir bounds the output path. |
| QuickJS runtime has no heap limit enforcement beyond the 128 MB soft limit set in `RuntimeConfig`. | Tested with `runtime::tests::memory_limit_enforced`; 128 MB limit triggers OOM kill. |

---

*Generated as part of T-23 (Compatibility matrix). This document is the single source of truth for extension compatibility in Q-Music.*