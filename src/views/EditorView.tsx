import { useCallback, useEffect, useState } from "react";

import { backend } from "../lib/ipc";
import { techLine } from "../lib/format";
import { toLrc } from "../lib/lrc";
import type { TagEdit, Track } from "../lib/types";
import { describe, useStore } from "../state/store";

interface TagFields {
  title: string;
  artist: string;
  albumArtist: string;
  album: string;
  genre: string;
  year: string;
  trackNo: string;
  discNo: string;
}

function fieldsOf(track: Track): TagFields {
  return {
    title: track.title,
    artist: track.artist,
    albumArtist: track.albumArtist,
    album: track.album,
    genre: track.genre,
    year: track.year === null ? "" : String(track.year),
    trackNo: track.trackNo === null ? "" : String(track.trackNo),
    discNo: track.discNo === null ? "" : String(track.discNo),
  };
}

function numberOrUndefined(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : undefined;
}

export function EditorView() {
  const { selectedTrack, currentTrack, replaceTrack, notify, loadLyrics } = useStore();
  const track = selectedTrack ?? currentTrack;

  const [fields, setFields] = useState<TagFields | null>(track ? fieldsOf(track) : null);
  const [lyricText, setLyricText] = useState("");
  const [savingTags, setSavingTags] = useState(false);
  const [savingLyrics, setSavingLyrics] = useState(false);

  const trackId = track?.id ?? null;
  const trackPath = track?.path ?? null;

  const loadLyricText = useCallback(async () => {
    if (trackId === null) {
      setLyricText("");
      return;
    }
    try {
      const lyrics = await backend.lyricsFor(trackId);
      setLyricText(lyrics.lines.length > 0 ? toLrc(lyrics.lines) : "");
    } catch {
      setLyricText("");
    }
  }, [trackId]);

  useEffect(() => {
    setFields(track ? fieldsOf(track) : null);
  }, [track]);

  useEffect(() => {
    void loadLyricText();
  }, [loadLyricText]);

  function update<K extends keyof TagFields>(key: K, value: string) {
    setFields((previous) => (previous ? { ...previous, [key]: value } : previous));
  }

  async function saveTags() {
    if (!track || !fields) return;
    setSavingTags(true);
    try {
      const edit: TagEdit = {
        title: fields.title,
        artist: fields.artist,
        albumArtist: fields.albumArtist,
        album: fields.album,
        genre: fields.genre,
      };
      const year = numberOrUndefined(fields.year);
      const trackNo = numberOrUndefined(fields.trackNo);
      const discNo = numberOrUndefined(fields.discNo);
      if (year !== undefined) edit.year = year;
      if (trackNo !== undefined) edit.trackNo = trackNo;
      if (discNo !== undefined) edit.discNo = discNo;

      const updated = await backend.updateTags(track.path, edit);
      replaceTrack(updated);
      notify(`Wrote the tags for ${updated.title || "this track"}`);
      await loadLyrics(updated.id);
    } catch (error) {
      notify(describe(error), "error");
    } finally {
      setSavingTags(false);
    }
  }

  async function saveLyrics() {
    if (trackId === null) return;
    setSavingLyrics(true);
    try {
      await backend.saveLyrics(trackId, lyricText);
      await loadLyrics(trackId);
      notify("Lyrics saved beside the audio file");
    } catch (error) {
      notify(describe(error), "error");
    } finally {
      setSavingLyrics(false);
    }
  }

  async function estimate() {
    if (trackId === null) return;
    try {
      const lyrics = await backend.estimateLyrics(trackId);
      setLyricText(toLrc(lyrics.lines));
      notify("Timings estimated — check them against the recording");
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function importFile() {
    if (trackId === null) return;
    try {
      const lyrics = await backend.importLyrics(trackId);
      if (!lyrics) return;
      setLyricText(toLrc(lyrics.lines));
      await loadLyrics(trackId);
      notify("Imported the lyric file");
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function exportFile() {
    if (!track || trackId === null) return;
    try {
      const suggested = `${track.artist || "unknown"} - ${track.title || "untitled"}.lrc`;
      const written = await backend.exportLyrics(trackId, suggested);
      if (written) notify(`Wrote ${written}`);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  async function reveal() {
    if (!trackPath) return;
    try {
      await backend.revealTrack(trackPath);
    } catch (error) {
      notify(describe(error), "error");
    }
  }

  if (!track || !fields) {
    return (
      <>
        <div className="view-head">
          <h1 className="view-title">Tag editor</h1>
        </div>
        <div className="view-body">
          <div className="empty">
            <p className="empty-title">No track selected</p>
            <p className="empty-body">
              Select a row in the library, or start something playing, and its tags and lyrics
              load here.
            </p>
          </div>
        </div>
      </>
    );
  }

  const tech = techLine(track);

  return (
    <>
      <div className="view-head">
        <h1 className="view-title">Tag editor</h1>
        <p className="view-sub" title={track.path}>
          {track.path}
        </p>
        <div className="view-tools">
          <span className="view-meta">
            {tech.length > 0 ? tech.join(" · ") : track.format.toUpperCase()}
          </span>
          <button type="button" className="button button-quiet" onClick={() => void reveal()}>
            Show file
          </button>
        </div>
      </div>

      <div className="view-body">
        <div className="panel">
          <section className="panel-section">
            <h2 className="panel-title">Tags</h2>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Title</span>
                <input
                  className="input"
                  value={fields.title}
                  onChange={(event) => update("title", event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">Artist</span>
                <input
                  className="input"
                  value={fields.artist}
                  onChange={(event) => update("artist", event.target.value)}
                />
              </label>
            </div>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Album artist</span>
                <input
                  className="input"
                  value={fields.albumArtist}
                  onChange={(event) => update("albumArtist", event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">Album</span>
                <input
                  className="input"
                  value={fields.album}
                  onChange={(event) => update("album", event.target.value)}
                />
              </label>
            </div>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Genre</span>
                <input
                  className="input"
                  value={fields.genre}
                  onChange={(event) => update("genre", event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">Year</span>
                <input
                  className="input"
                  inputMode="numeric"
                  value={fields.year}
                  onChange={(event) => update("year", event.target.value)}
                />
              </label>
            </div>

            <div className="field-row">
              <label className="field">
                <span className="field-label">Track number</span>
                <input
                  className="input"
                  inputMode="numeric"
                  value={fields.trackNo}
                  onChange={(event) => update("trackNo", event.target.value)}
                />
              </label>
              <label className="field">
                <span className="field-label">Disc number</span>
                <input
                  className="input"
                  inputMode="numeric"
                  value={fields.discNo}
                  onChange={(event) => update("discNo", event.target.value)}
                />
              </label>
            </div>

            <div className="actions">
              <button
                type="button"
                className="button button-primary"
                disabled={savingTags}
                onClick={() => void saveTags()}
              >
                {savingTags ? "Writing…" : "Save tags"}
              </button>
              <span className="hint">
                Tags are written into the file itself, not a side database. Numbers left blank
                keep their current value.
              </span>
            </div>
          </section>

          <section className="panel-section">
            <h2 className="panel-title">Lyrics</h2>

            <label className="field">
              <span className="sr-only">Lyrics text</span>
              <textarea
                className="textarea"
                rows={8}
                value={lyricText}
                spellCheck={false}
                onChange={(event) => setLyricText(event.target.value)}
              />
            </label>

            <div className="actions">
              <button
                type="button"
                className="button button-primary"
                disabled={savingLyrics}
                onClick={() => void saveLyrics()}
              >
                {savingLyrics ? "Saving…" : "Save lyrics"}
              </button>
              <button type="button" className="button" onClick={() => void estimate()}>
                Estimate timings
              </button>
              <button type="button" className="button" onClick={() => void importFile()}>
                Import file
              </button>
              <button type="button" className="button" onClick={() => void exportFile()}>
                Export .lrc
              </button>
            </div>

            <p className="hint">
              Plain lines work; timestamp them like [00:12.40] and the now panel follows the
              track. Saving writes a `.lrc` beside the audio file so other players read it too.
              Estimating spreads the lines across the track's length — a first pass, not a
              transcription.
            </p>
          </section>
        </div>
      </div>
    </>
  );
}
