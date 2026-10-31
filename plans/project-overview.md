# Q-Music Extension Host — Project Overview

## Context

Q-Music (Tauri 2 + React 19 + Rust) is built and verified: 48 Tauri commands, `qcore` green
(40 tests), UI complete across three themes, 18 verification screenshots.

This plan covers the second system: an **extension host** that makes Q-Music a compatible
implementation of the Mornye extension platform — metadata, search, lyrics and download
providers — without modifying upstream packages.

## Verified contract (evidence, not assumption)

| Claim | Evidence |
| --- | --- |
| Package is a plain ZIP named `.sflx` (legacy `.spotiflac-ext`) | `spotify-web.sflx` opened: 5 entries |
| Root contains `manifest.json` + `index.js`; licences alongside | `manifest.json`, `index.js`, `LICENSE`, `COPYING`, `NOTICE` |
| Registry SHA-256 verifies byte-exact | computed `5ccae8a8c236c975…` == registry value |
| JS runtime is QuickJS | docs, and `getTimezoneOffset()` caveat |
| **Real packages execute in `rquickjs` 0.14 on this machine** | probe: all 3 packages evaluated, `registerExtension()` captured |
| No network at load time | probe stub threw on any HTTP call; none fired |
| Registry: 12 entries, 7 `download`, 5 `integration` | `registry.json` fetched |
| `apple-music.sflx` (39286 bytes) matches the registry | computed `6a45c61e608bd59a…` == registry `sha256`, downloaded 2026-10-30 |

### Registered function surface — actual, from shipped packages

`spotify-web`
`initialize, cleanup, customSearch, handleUrl, getTrack, getAlbum, getArtist, getPlaylist,
searchTracks, enrichTrack, getHomeFeed, getBrowseCategories`

`apple-music`
`initialize, cleanup, customSearch, handleUrl, getTrack, getAlbum, getArtist, getConcert,
getPlaylist, getHomeFeed, searchTracks, enrichTrack, fetchLyrics`

`amazon`
`initialize, completeGrant, handleUrl, getTrack, getAlbum, getArtist, getPlaylist,
enrichTrack, getHomeFeed, customSearch, checkAvailability, download, getDownloadUrl,
matchTrack, validateTrackForDownload, cleanup`

`tidal-web`
`initialize, cleanup, completeGrant, customSearch, checkAvailability, download, handleUrl,
getTrack, getAlbum, getArtist, getPlaylist, enrichTrack, searchTracks`

`getConcert` is registered by `apple-music` and has no host route; the adapter reports it as
unrecognised rather than dropping it.

### Documented-vs-shipped divergences (the reason a compatibility adapter is required)

1. `download` — docs give `download(request, progressCallback)`; **every shipped package uses
   `download(trackID, quality, outputPath, onProgress, options)`**. Adapter dispatches on arity.
2. Error key — docs say `error`; `amazon` returns `error_message`. Adapter reads both.
3. `checkAvailability` — undocumented in the guide body; real signature is
   `checkAvailability(isrc, trackName, artistName, options) → { available, track_id, prepared_context }`.
4. `getDownloadUrl`, `validateTrackForDownload`, `completeGrant`, `matchTrack` appear in shipped
   packages but not in the guide's function list.

Found while implementing the host surface (T-7). Each is a call shape taken from the fixture
`index.js`, not from the guide:

5. `utils.hmacSHA1(key, message)` takes **byte arrays** (plain number arrays) and returns a byte
   array the caller indexes (`hmac[hmac.length - 1]`, `hmac[offset]`). Not strings, not hex.
   `utils.sha256(string)` returns lowercase hex.
6. `matching.compareStrings(a, b)` returns a **0..1 similarity**, because the shipped caller
   writes `Number(matching.compareStrings(expected, actual)) >= 0.86`. A boolean would make every
   fuzzy match fail; a Levenshtein ratio on normalised titles reproduces the intent.
7. `gobackend.getLocalTime().offsetMinutes` uses the `Date.getTimezoneOffset()` sign convention
   (**negative east of UTC**: `-330` is UTC+5:30), and the guide's own fallback table is keyed that
   way. The guide also warns QuickJS may report a zero offset, so the value is computed in Rust.
8. `utils.sleep(ms)` returns `false` when the download was cancelled, and `spotify-web` throws
   `"Spotify request cancelled"` on that return value; `utils.isDownloadCancelled()` and
   `utils.getResolutionRemainingMs()` exist alongside it (retry-loop budgeting).
9. `file.download(url, outputPath, { headers, onProgress })` is the only download primitive the
   packages call; `file.downloadSegments(...)` is called by `tidal-web` behind a feature check.
10. `gobackend.getAudioQuality(path)` returns `{ bitDepth, sampleRate, totalSamples }` or
    `{ error }`; `gobackend.checkISRCExists(dir, isrc)` returns `{ exists, filePath }`.

Rule: the **package is the spec**. Where docs and bytes disagree, follow the bytes and record it.

## Scope boundary

Build the complete host. Do not implement or exercise commercial-service ripping.

- **In scope**: all host systems; the 5 `integration` packages; a lawful reference download
  provider (user-owned Subsonic/Navidrome/Jellyfin server); generic streamed download execution
  tested against local fixtures only.
- **Out of scope**: `signedSession@3` (HMAC session exchange against `api.zarz.moe`, a
  third-party Tidal stream proxy), `downloadSegments@1`, Apple token harvesting, and verifying
  the 7 `download`-category rippers against live paid endpoints.
- Extensions requiring those features fail **at install** with a specific reason, never
  half-work. The compatibility matrix states this per entry.

## Crate layout

```
crates/qext/                    new — the host, no Tauri dependency
  manifest.rs                   schema, defaults, validation, capability negotiation
  package.rs                    .sflx read, limits, SHA-256, safe extraction
  registry.rs                   registry client, semver, min_app_version
  store.rs                      on-disk layout, install/update/uninstall, data dirs
  contract.rs                   Track / Album / Artist / Lyrics / DownloadResult
  adapter.rs                    surface normalisation (arity + key divergence)
  providers.rs                  registry of live providers, priority, quality selection
  queue.rs                      download jobs, progress, cancel, atomic write
  runtime/mod.rs                worker thread owning one QuickJS context
  runtime/{http,storage,file,crypto,auth,backend,polyfills}.rs
  runtime/permissions.rs        network allowlist, sandbox path resolution
crates/qcore/src/embed.rs       NEW — tag/artwork/lyrics write-back via Lofty
src-tauri/src/ext_commands.rs   NEW — ~20 commands over qext
src/lib/extensions.ts           NEW — typed client
```

`qext` owns JavaScript and network. `qcore` owns audio and tags. No dependency from `qcore`
back to `qext`.

## Security model

| Threat | Control |
| --- | --- |
| Zip bomb | Per-entry uncompressed cap, total cap, compression-ratio cap |
| Path traversal | Reject absolute, `..`, drive prefixes, backslash-normalised traversal |
| Symlink escape | Reject entries with `S_IFLNK` mode bits |
| Duplicate paths | Reject duplicate normalised names |
| Unbounded extension | One OS thread per runtime; `set_memory_limit`; per-call timeout; interrupt handler |
| Arbitrary network | Allowlist with `*` wildcards, checked on **host** side per request, including redirect hops |
| Filesystem escape | Relative paths resolved inside the extension data dir; canonicalise + prefix check; absolute paths rejected except host-issued download targets passed in explicitly |
| Secret leakage | `credentials` encrypted at rest (AES-GCM, key in OS keystore path); never logged |
| Downgrade attack | Version comparison refuses lower installs |
| Supply chain | Registry SHA-256 mandatory; mismatch aborts install |

Extensions are untrusted code. The host never hands a raw filesystem path or an unvalidated URL
to JavaScript.

## Data flow — a lyrics lookup

```text
React LyricsPane
  → invoke("ext_fetch_lyrics", {trackId})
    → qext::providers::fetch_lyrics
      → pick provider by type + priority + enabled
      → runtime.call("fetchLyrics") on worker thread
        → JS: http.get(...)                      [host checks allowlist]
        → host returns {statusCode, ok, body, headers}
      → adapter.normalise_lyrics(result)          [accepts both shapes]
    → qext::embed / qcore::lrc                     [cache, then optional write-back]
  → Lyrics struct to the pane
```

Nothing blocks the UI thread: commands are `async` and dispatch work onto the runtime thread.

## Compatibility strategy

One adapter, no per-extension branches.

1. **Capability negotiation** — `requiredRuntimeFeatures` is compared against a static host
   feature table. Unmet → install refused with the missing feature named.
2. **Arity dispatch** — `download` is invoked with 5 arguments. A 2-argument implementation is
   detected via `Function.length` and called with the documented pair instead.
3. **Result normalisation** — `error`/`error_message`/`message`, `images` string-or-array,
   `artists` string-or-array, lyric `startTimeMs` numeric coercion.
4. **Feature presence** — providers report only the functions they registered; UI asks
   "can this provider do X" rather than assuming.
5. **Failure isolation** — a throwing provider is marked unhealthy and skipped, not fatal.

### Callbacks across the JSON bridge (host mechanism, T-8)

A function cannot be serialised, but `download` and `file.download` both take progress
callbacks. The adapter passes `{"__qext_callback__": "<kind>"}`; the prelude revives it into a
JavaScript function that forwards `{kind, args}` to `__qext_progress`, which the host records for
the active job. The marker is a host convention, not an upstream feature — no fixture sends one.

## Performance budget

| Path | Target | Method |
| --- | --- | --- |
| App startup impact | 0 ms | runtimes created lazily on first use, never at boot |
| Runtime creation | < 30 ms | one context per extension, reused |
| Idle memory per extension | < 8 MB | memory limit + context teardown when disabled |
| Metadata call | network-bound | no polling; results cached in SQLite |

## Unverified / open

- Whether manifests carry inter-extension `dependencies` — **not present** in the three
  manifests read. Do not invent a schema; treat `requiredRuntimeFeatures` as the only
  declared host dependency and add package dependencies only if a real package uses them.
- Exact `auth.*` and `crypto.*` signatures beyond the guide's examples — the guide is the only
  source; real packages are the tiebreaker when one uses them.
- `reqwest` 0.13 cookie feature name — confirm with `cargo add --dry-run` before pinning.

## Definition of done

Host ships when: all 12 registry packages can be *inspected and installed or refused with a
named reason*; the 5 integration packages load and execute against real bytes; the download
pipeline passes end to end against a fixture server and a lawful provider; embedding writes
verify with a round-trip tag read; the compatibility matrix documents every divergence; and
`cargo test`, `tsc --noEmit` and `vite build` are green.

## Call layer (T-11)

`calls.rs` wraps one provider runtime plus a SQLite cache. Every method normalises results and
stamps them with the provider id, so the UI source chip never guesses. Cache rows are keyed by
`(provider, function, canonical arguments)` with an optional TTL; a failed call writes nothing,
so a second provider answering the same query is unaffected.
