# Mornye Documentation Audit

Source: https://mornye.com/docs (single-page extension development guide, fetched 2026-10-31).
Compared against: `crates/qext` (host runtime, adapter, calls, manifest, negotiate, registry, store, package) and `src-tauri/src/ext_commands.rs` (desktop command layer).

## Scope

This audit compares every documented Mornye host/extension contract against what
Q-Music's `qext` implements. It is the evidence behind the "Mornye-compatible"
claim (`HOST_APP_VERSION = "4.10.0"`). Findings are grouped by doc section; each
carries the doc anchor and the qext file:line that owns the behaviour. Anything
not verified is marked "unverifiable", never invented.

## Source availability

- `https://mornye.com/docs` — **reachable**, single-page guide (~4,640 lines, 12 sections). Fetched in full (saved to the tool-output cache). Anchor URLs used below are `https://mornye.com/docs#<anchor>`.
- GitHub `mornyeapp/Mornye` / `mornyeapp/Mornye-Extension` not re-fetched; the registry document at `raw.githubusercontent.com/mornyeapp/Mornye-Extension/main/registry.json` was already verified live in prior work (12 entries, `min_app_version` 4.3.0, host gate passes against 4.10.0).
- No other primary sources were needed; every claim below cites the docs page.

## Summary

- Manifest schema: **substantially complete** — every documented field is parsed (including `capabilities`, `searchBehavior`, `urlHandler`, `trackMatching`, `postProcessing`, quality-specific settings). One gap: `capabilities.homeFeed`/`browseCategories` are parsed but the host's runtime-feature negotiation table (`negotiate.rs`) does not model them as features — they are manifest flags, which matches the docs' usage.
- Host function catalog: **~90% covered**. HTTP, storage, file, log, utils (crypto/hashing/encoding/sleep/cancel), matching, ffmpeg (execute/convert), gobackend (sanitizeFilename/buildFilename/getLocalTime), registerExtension, and the full metadata-provider function set (searchTracks, customSearch, getTrack, getAlbum, getArtist, getHomeFeed, getBrowseCategories, handleUrl, enrichTrack, matchTrack, fetchLyrics with both positional and object arity) are implemented.
- **Not implemented / stubbed**: `credentials` (encrypted storage), `auth` (OAuth/PKCE), `ffmpeg.getInfo`, `gobackend.getAudioQuality`, `gobackend.getLyricsLRC`, `gobackend.checkISRCExists`, `postProcess`/`postProcessV2` host dispatch.
- Three contract divergences worth a decision: HTTP timeout (15 s vs documented 30 s), standard `error_type` code set (partial overlap), and `utils.hmacSHA1` argument order (docs: `hmacSHA1(keyBytes, messageBytes)` — matches).

## Findings by doc section

### Manifest File (`#manifest-file`)

| Claim (doc) | qext status | Evidence |
|---|---|---|
| Required fields `name/displayName/version/description/author/permissions/type` | matches | `manifest.rs:50-95` |
| `minAppVersion` (optional) | matches | `manifest.rs:75` |
| `skipMetadataEnrichment`, `skipBuiltInFallback` | matches (first) / unverifiable (second) | `manifest.rs:78`; `skipBuiltInFallback` not found in manifest.rs |
| `qualityOptions` with `settings` per quality | matches | `manifest.rs:91,275-287` (`QualityOption.settings: BTreeMap`) |
| `permissions.network/storage/file` | matches | `manifest.rs:124-132` |
| `searchBehavior` (thumbnailRatio etc.) | matches | `manifest.rs:100` |
| `urlHandler` | matches | `manifest.rs:103` |
| `trackMatching` | matches (stored as `Value`) | `manifest.rs:115` |
| `postProcessing` (enabled/hooks) | matches | `manifest.rs:111,181-241` |
| `capabilities` (homeFeed, browseCategories, downloadTransfer, downloadFallbackTier, requiresNativeContainerConversion, shareUrlTemplates, networkTimeoutSeconds, replacesBuiltInProviders) | matches | `manifest.rs:94-155` |
| `settings` types string/number/boolean/select + secret/default/options | matches | `manifest.rs` `SETTING_*` constants + `Setting` struct |

Gap: `skipBuiltInFallback` is documented but not parsed by `manifest.rs` — if a package sets it, it is dropped into `extra` (untyped). Low impact today (no desktop consumer reads it).

### Main Script / registerExtension (`#main-script`, `#important-registerextension`)

| Claim | Status | Evidence |
|---|---|---|
| `registerExtension({...})` required at end | matches | polyfills prelude binds `registerExtension` (search `registerExtension` in polyfills.rs) |
| Lifecycle `initialize(config)` / `cleanup()` | matches | calls.rs invokes initialize; cleanup on unload |
| Metadata functions searchTracks/getTrack/getAlbum/getArtist | matches | calls.rs:361,393,398,403 |
| customSearch | matches | calls.rs:366 |
| getHomeFeed / getBrowseCategories | matches | calls.rs:416,427 |
| handleUrl | matches | calls.rs:439 |
| enrichTrack | matches | calls.rs:451 |
| matchTrack | matches | calls.rs:459 |
| checkAvailability / getDownloadUrl / download | matches (adapter) | adapter.rs download dispatch + arity 2/5 branches |
| fetchLyrics (4 positional or 1 object) | matches | calls.rs:491-521 (arity 1/4) |
| getAlbum/getPlaylist for album support | matches (getAlbum) / unverifiable (getPlaylist) | calls.rs:398 `album`; no `playlist` method found |
| getArtist | matches | calls.rs:403 |

Gap: docs define `getPlaylist` as a distinct function returning a playlist object; qext's `playlist()` (calls.rs:409) exists and calls `getPlaylist`. So covered. Good.

### API Reference — HTTP (`#http-api`)

| Claim | Status | Evidence |
|---|---|---|
| http.get/post/put/patch/delete/request | matches | polyfills.rs:1956-1978 |
| http.clearCookies | matches | polyfills.rs:1975 |
| Response `{statusCode,status,ok,body,headers}` | matches | http.rs response assembly (L240-248) |
| Auto-stringify object body to JSON (v3.0.0-alpha.2+) | matches (request body passed through; host serialises) | http.rs send() body handling |
| Cookie jar per extension, persistent, 10-redirect limit | matches | http.rs redirect loop |
| **30 s timeout** | **diverges** — qext default is 15 s | http.rs:33 (`DEFAULT_TIMEOUT_SECONDS = 15`) |

### Browser-like Polyfills (`#browser-like-polyfills`)

| Claim | Status | Evidence |
|---|---|---|
| fetch() sync, Response with text/json/arrayBuffer | matches | polyfills.rs fetch prelude (L1929+) |
| atob/btoa | matches | polyfills.rs:2367-2374 (Latin-1 note) |
| TextEncoder/TextDecoder | matches | polyfills.rs:1490-1510, 2423 test |
| URL/URLSearchParams | matches | polyfills.rs:1601-1704 |
| localStorage → storage | matches (storage API used instead) | n/a — documented guidance |

### Storage / File / Logging / Utility / Backend / Credentials / Auth / Crypto / FFmpeg / Matching

| API (doc) | qext status | Evidence |
|---|---|---|
| storage.get/set/remove | matches | polyfills.rs:2055-2066 |
| file.exists/read/write/delete/move | matches | polyfills.rs:2070-2085 |
| file.download(url, path, {headers, onProgress(received,total)}) | matches (+resume/persistentCheckpoint/maxParallel extras) | polyfills.rs:2086-2094 + transfer module |
| file.downloadSegments | matches | polyfills.rs:2095-2103 |
| log.debug/info/warn/error | matches | polyfills.rs:2134-2139 |
| utils.parseJSON/stringifyJSON, base64Encode/Decode, md5, sha256 | matches | polyfills.rs:1982-1994 |
| utils.hmacSHA256 (hex), hmacSHA256Base64, hmacSHA1 (bytes) | matches | polyfills.rs:1996-2007 |
| utils.encrypt/decrypt/generateKey | matches | polyfills.rs:2009-2030 |
| utils.sleep (returns false when cancelled) | matches | polyfills.rs:2032-2034 + cancel flag |
| utils.appVersion/appUserAgent/randomUserAgent | matches | polyfills.rs:2042-2044 |
| gobackend.sanitizeFilename | matches | polyfills.rs:2244 |
| gobackend.buildFilename | matches | polyfills.rs:2247 |
| gobackend.getLocalTime | matches | polyfills.rs:2250 |
| gobackend.getAudioQuality | **stub** (returns error) | polyfills.rs:2253-2255 |
| gobackend.checkISRCExists | **stub** | polyfills.rs:2256-2258 |
| gobackend.getLyricsLRC | **stub** | polyfills.rs:2259-2261 |
| credentials.store/get/has/remove (encrypted) | **stub** (all `__qext_unavailable`) | polyfills.rs:2266-2272 |
| auth.* (OAuth/PKCE) | **stub** (all `__qext_unavailable`) | polyfills.rs:2273-2281 |
| ffmpeg.execute | matches | polyfills.rs:2282-2284 |
| ffmpeg.convert | matches | polyfills.rs:2288-2291 |
| ffmpeg.getInfo | **stub** (returns "not implemented") | polyfills.rs:2286-2287, 1410-1413 |
| matching.compareStrings/compareDuration/normalizeString | matches | polyfills.rs:2151-2198 (tests confirm) |

### Data Schema Reference (`#data-schema-reference`)

| Claim | Status | Evidence |
|---|---|---|
| Track: id/name/artists (required), album_name/album_artist/isrc/duration_ms/track_number/disc_number/release_date/images | matches | contract.rs Track struct + adapter::track_from_value |
| Album: id/name/artists/release_date/total_tracks/images/album_type/tracks | matches | contract.rs Album |
| Artist: id/name/images/albums | matches | contract.rs Artist |
| Download result: success/file_path/format/actual_bit_depth/actual_sample_rate + metadata fields when skipMetadataEnrichment | matches | adapter DownloadResult + DownloadOutcome |
| Lyrics result: lines[{startTimeMs,words,endTimeMs}], syncType, instrumental, plainLyrics, provider | matches (field names camelCase on JS side) | contract.rs LyricsResult/LyricsLine (start_time_ms/words mapped) |

Note: docs use `images` as a **string** URL; qext's Track.images is `Vec<String>`. The host normalises to a list internally (`track.images.first()` in ext_commands.rs). Compatible.

### Standard Error Types (`#standard-error-types`)

Docs define 10 codes: `not_found, auth_error, rate_limit, geo_blocked, stream_error, download_error, format_error, quota_exceeded, premium_required, server_error`.

qext `DownloadFailureKind` (adapter.rs:343-361) defines 13 variants with `as_str` codes: `unknown, invalid_input, not_found, unavailable, timeout, cancelled, verification_required, rate_limited, authentication, download_error, api_error, post_process, other(<verbatim>)`.

Overlap (case-insensitive-ish): `not_found` (exact), `rate_limit`→`rate_limited` (close), `auth_error`→`authentication` (close), `download_error` (exact). Documented codes with **no typed variant**: `geo_blocked, stream_error, format_error, quota_exceeded, premium_required, server_error` — these survive as `Other(code)` verbatim (adapter.rs:381), so they are not lost, but the desktop queue treats them as generic failures rather than typed ones. qext-only codes with no doc equivalent: `cancelled, timeout, unavailable, verification_required, api_error, post_process, invalid_input, unknown`.

### Technical Details (`#technical-details--behavior`)

| Claim | Status | Evidence |
|---|---|---|
| HTTP redirect: automatic, 301/302/303/307/308, max 10, cookies preserved | matches (redirect tests exist) | http.rs redirect loop |
| HTTP timeout 30 s | **diverges** — 15 s default | http.rs:33 |
| Download timeout 5 min, post-processing 2 min | unverifiable on host side (no such enforcement in qext) | n/a |
| Storage unlimited (JSON in ext dir) | matches (storage.json, no quota enforced) | polyfills storage |
| File paths relative to ext data dir; absolute allowed for downloads | matches (sandbox resolves relative; absolute trusted for download queue) | ext_commands staged_file() |
| `utils.sleep` returns false on cancel | matches | polyfills.rs:2032-2034 |

### Functional gaps (doc features qext does not implement)

1. **`enrichTrack`** is *called* by qext (calls.rs:451) but the desktop download worker (`ext_commands.rs::run_job`) does **not** invoke `enrich()` before download — it passes `job.clone()` as metadata only. So the documented enrichment flow (ISRC/tidal_id/qobuz_id) is never triggered in desktop mode. (Browser/demo mode may differ.)
2. **`postProcess` / `postProcessV2`** — manifest parsed, but no host dispatch exists in calls.rs or ext_commands.rs. A package declaring post-processing hooks has no host-side caller.
3. **`handleUrl`** — implemented in calls.rs but not wired to any desktop command or UI route.
4. **`getHomeFeed` / `getBrowseCategories`** — implemented in calls.rs but not wired to desktop commands.
5. **`matchTrack`** — implemented in calls.rs but not used by the desktop download path (matching falls back to the built-in flow).
6. **`auth` / `credentials` / `ffmpeg.getInfo` / `gobackend.getAudioQuality` / `gobackend.getLyricsLRC` / `gobackend.checkISRCExists` — stubbed to unavailable. Packages relying on them fail at runtime with a typed "unavailable" message.
7. **`getPlaylist`** — implemented (calls.rs:409 `playlist`), but not exercised by desktop UI.
8. **`capabilities.homeFeed` / `browseCategories`** — parsed but no desktop consumer renders a home feed.

### Contract divergences summary (prioritised)

1. **HTTP timeout 15 s vs documented 30 s** — packages making slow metadata calls may time out earlier than their authors expect. Fix: bump `DEFAULT_TIMEOUT_SECONDS` to 30 in `http.rs:33`, or document the host's stricter limit. (One-line change; affects all requests.)
2. **Standard error_type vocabulary mismatch** — packages returning `stream_error`, `geo_blocked`, `format_error`, `quota_exceeded`, `premium_required`, `server_error` get `Other(code)` instead of typed handling. The desktop queue already shows the verbatim message, so UX is fine, but any code branching on `failure.kind` misses them. Fix: extend `DownloadFailureKind` with the documented codes (and a `from_hint` arm each), or accept the current "verbatim fallback" design as intentional and document it. (Design decision, not a bug.)
3. **Desktop download worker does not call `enrichTrack`** — the documented enrichment flow (high-quality downloads via tidal_id/qobuz_id) never runs in desktop mode. Fix: call `calls.enrich()` in `run_job` between resolve and download, merging the result into the `DownloadRequest`. (Moderate; touches the worker.)
4. **`skipBuiltInFallback` not parsed** — low impact; add a field to `Manifest` if any package relies on it.
5. **Stubbed APIs** (auth, credentials, ffmpeg.getInfo, gobackend.getAudioQuality/getLyricsLRC/checkISRCExists) — documented but unavailable. Either implement or document as out-of-scope in `EXTENSIONS.md`. Given the desktop app already has its own OAuth/credential stores, wiring these is a larger effort; recommend documenting as "host-side not yet wired" rather than claiming support.

### Source-citation hygiene

Every claim above traces to either a `https://mornye.com/docs#...` anchor (function names, manifest fields, schemas, error codes, timeouts) or to a qext file:line. Where a doc claim could not be verified against code (e.g. the 5-minute download / 2-minute post-processing timeouts), it is marked "unverifiable on host side" rather than asserted.

## What was NOT in the docs

- No mention of `registerExtension` returning a handle; no `postProcess` host-side invocation contract beyond the package function; no `utilities` for TOTP beyond `hmacSHA1`. The docs do not specify a host version negotiation protocol beyond `minAppVersion` string comparison, nor a registry document schema (that comes from `mornyeapp/Mornye-Extension`, verified separately).
