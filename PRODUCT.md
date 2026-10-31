# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

## Stack

Greenfield. The brief delegated the choice: **Tauri 2 (Rust) + React 19 + TypeScript + Vite**, with the
audio engine, library index and metadata layer written in Rust. Rationale: the brief requires Rust for
the audio engine and performance-critical systems, while also requiring an interface polished to a
commercial standard. Rust owns decode/output/index/tag-write (Symphonia 0.6 for decoding, CPAL for
output, Lofty for tags, Rusqlite for the index); the webview owns presentation only and never touches
sample buffers. Rejected alternatives: a pure-Rust UI toolkit (Iced/Egui) gives lower memory but a
visibly weaker ceiling for the requested finish, and no way to verify the render in a browser.

## Users

People who keep their own music as files: collectors with FLAC/ALAC rips, listeners who work offline,
and anyone on a laptop where the browser-based players are heavy. They listen on headphones at a desk,
usually in the evening with the room dim, and they already know their library — they open the player to
find one thing quickly and then leave it running. Secondary: people who care about playback quality
(gapless albums, EQ, device choice) and people who read lyrics while listening.

## Product Purpose

A desktop music player that opens fast, stays small in memory, and plays essentially any audio file the
user owns. It exists because local music players either handle few formats, demand a streaming account,
or feel like utilities. Success: from cold start to the right track playing in seconds, with no format
the user has to convert and no library too large to search instantly.

## Positioning

The whole listening path — scan, index, decode, output, tag write, lyric sync — is one native Rust
engine rather than a browser playing back what a server transcoded. That is what makes "any format,
gapless, at custom speed, through a real equalizer, from a 100k-track library" one claim instead of
four compromises.

## Operating Context

- Folder-based library: users point Q-Music at one or more directories; scanning is incremental.
- Format reality: FLAC, MP3, M4A/AAC, ALAC, WAV, OGG/Vorbis, Opus, AIFF, plus whatever Symphonia's
  format/codec matrix covers later; codec support must extend without rewriting the engine.
- Listening sessions are long and unattended; system media keys and OS media overlays are expected.
- Lyrics arrive in three ways: embedded tags, a sidecar `.lrc`, or typed by hand against the waveform.
- Users expect desktop conventions: menu accelerators, multi-select, context menus, drag-and-drop.

## Capabilities and Constraints

Confirmed scope, all of it shipping functionality rather than mockups:

- Library scanning with progress, incremental rescan, folder browsing, and a persistent local index.
- Playlists (manual), queue management, smart playlists (rule-based), favourites, recently played.
- Search, sortable columns, album art extraction and cache, metadata editing written back to files.
- Playback: gapless, shuffle, repeat modes, playback speed, 10-band equalizer, output device selection.
- Lyrics: embedded, `.lrc`/`.txt` import, synchronized playback, manual timestamp editing.
- Keyboard shortcuts, system media controls, configurable themes, persistent settings.
- Reliable error handling: unreadable files, unsupported codecs, missing art, corrupt tags degrade one
  row, never the session.

Undecided and deliberately not invented: online metadata/artwork lookup, cloud sync, mobile builds,
streaming services, plugin marketplace for codecs.

## Brand Commitments

- Name: **Q-Music**, Q standing for Quantum. The wordmark is "Q-Music"; the engine is referred to as
  the Quantum engine internally.
- The brief is binding on taste: no AI-slop aesthetics, no generic dashboard, no decorative gradients,
  no glassmorphism, no template-like layout. It must read as a carefully crafted premium native music
  application.
- No third-party brand marks, artist imagery, or fabricated claims may be presented as real.

## Evidence on Hand

None. There is no logo, no screenshot library, no real audio collection, and no photography. Any album
art, track metadata, lyric text, and listening history in the build are authored demonstration material
and must be labelled as synthetic wherever a user could mistake them for real data.

## Product Principles

1. **The file is the truth.** Play what is on disk; never require an account, a network, or a
   transcode to hear it.
2. **Speed is a feature you can feel.** Cold start, search, and track change are the three moments the
   product is judged; each must be effectively instant.
3. **Playback never lies.** Gapless means no gap, sample-accurate means sample-accurate, and a failure
   is reported precisely instead of silently skipped.
4. **The library is yours.** Local index, local art cache, tag writes the user can undo by hand.
5. **Quiet by default, deep on demand.** The surface shows the current track and what is next; every
   advanced control exists but never competes for attention.

## Accessibility & Inclusion

Keyboard-complete operation (every action reachable without a pointer), visible focus, text contrast at
WCAG AA, no motion required to understand state, and reduced-motion support. Colour is never the only
carrier of playback state.
