# Q-Music Extension Host — User Flows

Personas: **listener** (richer metadata, lyrics), **operator** (manages providers and
downloads), **extension author** (ships a package and expects it to work).

## 1. First run, no extensions

Rail shows **Extensions** with a dot. Opening it shows the catalogue, not a blank state: 12
registry entries with type badge, version and install button.

Packages whose `requiredRuntimeFeatures` the host lacks appear greyed with the reason inline —
"Requires signedSession@3 (not supported)" — never a silent failure after install.

## 2. Install a metadata provider

Catalogue, Install on *Spotify Web*:

```text
click -> fetch .sflx -> SHA-256 vs registry
      -> archive safety -> manifest validate -> min_app_version
      -> capability negotiation -> extract -> enable -> runtime warm
```

Progress renders on the card, not a modal. Success flips the card to **Installed vX** with a
settings gear. A checksum mismatch shows **Integrity check failed — not installed**; nothing
reaches disk. Wrong app version names both versions. Unsafe archive names the offending entry.

## 3. Configure a provider

Gear opens a sheet rendered **from `settings[]`**: text, number, boolean, select with real
options, and buttons bound to registered actions (`startLogin`, `completeGrant`). No hardcoded
per-extension forms.

Saving calls `initialize(config)` once and persists `storage.json`. Values survive restart.
Secrets use the encrypted credential store, never `storage.json`.

## 4. Search across providers

Typing in the search box shows local library results immediately; provider groups follow under
**From extensions**, each row carrying a source chip.

A slow provider shows a skeleton, then results or a quiet inline "Spotify Web timed out". Local
results are never delayed by a network provider.

## 5. Play a track from an extension result

Result row, play. The player resolves the track: if it exists locally it plays from disk;
otherwise the row offers **Fetch** when a download provider can supply it and the file is
permitted.

The Now panel shows the artwork from `images` and the source chip. Playback itself never
depends on an extension being healthy.

## 6. Download one track

Context menu, **Download**, then a quality picker built from `qualityOptions[]`.

```text
queued -> resolving -> downloading 42% (12.4 MB/s) -> post-processing -> embedding -> done
```

Every state is visible in a queue drawer, including byte counts and current stage. Cancel is
available in every state and leaves no partial file.

Failures are typed, not generic: *not found*, *authentication failed*, *rate limited*. The row
keeps a retry action; the error type maps to a human sentence from the documented table.

## 7. Download an album

Album, **Download album**. Jobs are expanded per track and queued in order, with one active
download and a configurable concurrency. The drawer shows "3 of 12". Cancelling the album
cancels its remaining jobs; already-finished tracks stay.

On completion: tags written, artwork embedded, lyrics fetched from a lyrics provider and
written, plus a sidecar `.lrc`. The library picks the new files up without a manual rescan.

## 8. Set provider priority

Settings, Providers: a reorderable list per capability. The first healthy provider that
declares the capability wins; the rest are fallbacks.

Reordering is immediate and persists. A provider that failed its last three calls shows an
**unhealthy** badge and is skipped until retried.

## 9. Lyrics from an extension

Lyrics pane, no local lyrics. The pane offers **Fetch from extensions** when a lyrics provider
is installed. Result lands as synced or plain, labelled with the provider name.

Saving writes back to the file only on explicit request, honouring the existing editor rules.

## 10. Update or remove

An installed extension with a newer registry version shows **Update** with both versions. An
update is installed only if strictly newer. Remove asks for confirmation, deletes the package
and its data directory, and tears down the runtime.

## 11. Extension author checks a package

Drop a `.sflx` on the Extensions view (or pick a file). The host runs the same pipeline minus
the registry fetch and reports a validation list: archive OK, manifest OK, checksum recorded,
registered surface detected, permissions requested, unsupported features listed.

This is the compatibility adapter's public face: the author sees exactly which functions the
host recognised and which it did not.
