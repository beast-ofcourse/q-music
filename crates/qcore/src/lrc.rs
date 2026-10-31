//! LRC parsing and serialisation.
//!
//! The format is simple enough that a hand parser beats a dependency: `[mm:ss.xx]`
//! stamps may appear several times on one line (a repeated chorus), plain `.txt`
//! lyrics have no stamps at all, and users expect the editor to keep whatever they
//! typed. Anything ambiguous is reported as a warning instead of being dropped.

use crate::model::{LyricLine, Lyrics, LyricsSource};

/// Metadata tag keys that LRC files may contain. These are stripped from display.
const METADATA_KEYS: &[&str] = &[
    "ar", "ti", "al", "au", "by", "offset", "re", "ve", "length", "tool", "encoder",
];

/// Parses lyric file contents, accepting both `.lrc` and plain text.
/// Handles standard LRC, enhanced LRC (inline word timestamps), and metadata tags.
pub fn parse(track_id: i64, source: LyricsSource, content: &str) -> Lyrics {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut stray_warnings = 0usize;

    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Skip metadata tags like [ar:Artist], [ti:Title], [al:Album], etc.
        if is_metadata_tag(trimmed) {
            continue;
        }

        let mut stamps: Vec<u64> = Vec::new();
        let mut rest = trimmed;

        while let Some((stamp, remainder)) = take_timestamp(rest) {
            stamps.push(stamp);
            rest = remainder.trim_start();
        }

        // Strip inline enhanced timestamps (<mm:ss.xx>) from the text.
        let text = strip_enhanced_timestamps(rest).trim().to_string();

        // Remove any stray timestamp-like patterns that leaked through.
        let text = strip_stray_timestamps(&text);

        // Skip metadata/credit lines.
        if is_metadata_line(&text) {
            continue;
        }

        if stamps.is_empty() {
            if text.is_empty() {
                continue;
            }
            lines.push(LyricLine { time_ms: None, text });
            continue;
        }

        if text.is_empty() {
            stray_warnings += 1;
        }

        for stamp in stamps {
            lines.push(LyricLine { time_ms: Some(stamp), text: text.clone() });
        }
    }

    lines.sort_by_key(|line| line.time_ms.unwrap_or(u64::MAX));

    let synced = !lines.is_empty() && lines.iter().all(|line| line.time_ms.is_some());
    let warning = if stray_warnings > 0 {
        Some(format!(
            "{} timestamped line(s) had no words and were kept as blank cues",
            stray_warnings
        ))
    } else {
        None
    };

    Lyrics { track_id, source, lines, synced, warning }
}

/// Check if a line is a metadata tag like [ar:Artist] or [ti:Title].
fn is_metadata_tag(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') {
        return false;
    }
    let close = match trimmed.find(']') {
        Some(c) => c,
        None => return false,
    };
    let inside = &trimmed[1..close];
    // Metadata tags have a colon separator: [key:value]
    if let Some(colon_pos) = inside.find(':') {
        let key = &inside[..colon_pos];
        if METADATA_KEYS.contains(&key.trim().to_lowercase().as_str()) {
            return true;
        }
    }
    false
}

/// Strip enhanced LRC inline timestamps like <00:32.116> or <32.116>.
fn strip_enhanced_timestamps(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // Try to consume a timestamp until '>'
            let mut buf = String::new();
            let mut found_close = false;
            for ch in chars.by_ref() {
                if ch == '>' {
                    found_close = true;
                    break;
                }
                buf.push(ch);
            }
            // If it looks like a timestamp, skip it; otherwise keep the text.
            if found_close && is_timestamp_like(&buf) {
                // Skip — it's a timestamp
            } else {
                result.push('<');
                result.push_str(&buf);
                if found_close {
                    result.push('>');
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Check if a string looks like a timestamp (mm:ss.xx or ss.xx).
fn is_timestamp_like(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // Match patterns like 00:32.116, 32.116, 00:32, 32
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        1 => {
            // Could be seconds or seconds.fraction
            let p = parts[0];
            let (whole, frac) = if let Some(dot) = p.find('.') {
                (&p[..dot], &p[dot + 1..])
            } else {
                (p, "")
            };
            whole.chars().all(|c| c.is_ascii_digit()) && frac.chars().all(|c| c.is_ascii_digit())
        }
        2 => {
            let m = parts[0];
            let s = parts[1];
            m.chars().all(|c| c.is_ascii_digit())
                && {
                    let (whole, frac) = if let Some(dot) = s.find('.') {
                        (&s[..dot], &s[dot + 1..])
                    } else {
                        (s, "")
                    };
                    whole.chars().all(|c| c.is_ascii_digit()) && frac.chars().all(|c| c.is_ascii_digit())
                }
        }
        _ => false,
    }
}

/// Check if a line is a metadata/credit line that should not be displayed as lyrics.
fn is_metadata_line(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return true;
    }
    // Skip URLs
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    // Skip common credit/metadata patterns
    let credit_prefixes = [
        "produced", "written", "composed", "arranged", "mixed", "mastered",
        "engineered", "recorded", "published", "copyright", "©", "℗",
    ];
    for prefix in &credit_prefixes {
        if lower.starts_with(prefix) {
            return true;
        }
    }
    // Skip version-only markers
    if lower.starts_with('v') {
        let rest = &lower[1..];
        let mut end = 0;
        for c in rest.chars() {
            if c.is_ascii_digit() {
                end += 1;
            } else {
                break;
            }
        }
        if end > 0 && rest[end..].starts_with(':') && rest[end + 1..].trim().is_empty() {
            return true;
        }
    }
    false
}

/// Remove any stray timestamp patterns that leaked through.
fn strip_stray_timestamps(text: &str) -> String {
    let mut result = text.to_string();
    // Remove any remaining <...> that look like timestamps
    while let Some(start) = result.find('<') {
        if let Some(end) = result[start..].find('>') {
            let inner = &result[start + 1..start + end];
            if is_timestamp_like(inner) {
                result = format!("{}{}", &result[..start], &result[start + end + 1..]);
            } else {
                break;
            }
        } else {
            break;
        }
    }
    // Remove version markers like v1:, v2:, etc.
    if let Some(rest) = result.strip_prefix('v') {
        // Check if it's v<number>:
        let mut end_idx = 0;
        for (i, c) in rest.char_indices() {
            if c.is_ascii_digit() {
                end_idx = i + 1;
            } else {
                break;
            }
        }
        if end_idx > 0 && rest[end_idx..].starts_with(':') {
            result = rest[end_idx + 1..].trim().to_string();
        }
    }
    result.trim().to_string()
}

/// Reads `[mm:ss]`, `[mm:ss.xx]`, `[mm:ss.xxx]` or `[hh:mm:ss]` and returns
/// milliseconds plus the remainder of the line.
fn take_timestamp(input: &str) -> Option<(u64, &str)> {
    let rest = input.strip_prefix('[')?;
    let close = rest.find(']')?;
    let inside = &rest[..close];
    let remainder = &rest[close + 1..];

    let parts: Vec<&str> = inside.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [m, s] => (0u64, parse_number(m)?, parse_seconds(s)?),
        [h, m, s] => (parse_number(h)?, parse_number(m)?, parse_seconds(s)?),
        _ => return None,
    };
    Some((hours * 3_600_000 + minutes * 60_000 + seconds, remainder))
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Seconds with an optional fractional part, in any of 1, 2 or 3 decimals.
fn parse_seconds(text: &str) -> Option<u64> {
    let mut split = text.splitn(2, '.');
    let whole = parse_number(split.next()?)?;
    let fraction_ms = match split.next() {
        None => 0,
        Some(fraction) => {
            if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let digits: String = fraction.chars().take(3).collect();
            let padded = format!("{digits:0<3}");
            padded.parse::<u64>().ok()?
        }
    };
    if whole >= 60 {
        return None;
    }
    Some(whole * 1000 + fraction_ms)
}

/// Writes lyrics back as an LRC document, keeping unsynced lines as plain text.
pub fn to_lrc(lines: &[LyricLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if line.text.trim().is_empty() && line.time_ms.is_none() {
            continue;
        }
        match line.time_ms {
            Some(ms) => {
                let minutes = ms / 60_000;
                let seconds = (ms % 60_000) / 1000;
                let hundredths = (ms % 1000) / 10;
                out.push_str(&format!("[{minutes:02}:{seconds:02}.{hundredths:02}]{}\n", line.text));
            }
            None => {
                out.push_str(&line.text);
                out.push('\n');
            }
        }
    }
    out
}

/// Index of the active line for a playback position, or the last line whose
/// stamp has passed. Returns `None` before the first line.
pub fn active_line(lines: &[LyricLine], position_ms: u64) -> Option<usize> {
    let mut active = None;
    for (index, line) in lines.iter().enumerate() {
        match line.time_ms {
            Some(stamp) if stamp <= position_ms => active = Some(index),
            Some(_) => break,
            None => continue,
        }
    }
    active
}

/// Falls back to evenly spaced timings for a plain-text lyric sheet so the
/// player can still follow along; the editor marks the result as estimated.
pub fn estimate_timings(lines: &[LyricLine], duration_ms: u64) -> Vec<LyricLine> {
    let text_lines: Vec<&str> = lines
        .iter()
        .map(|line| line.text.trim())
        .filter(|text| !text.is_empty())
        .collect();

    if text_lines.is_empty() {
        return Vec::new();
    }

    let weights: Vec<f64> = text_lines
        .iter()
        .map(|text| (text.chars().count() as f64 + 12.0).min(80.0))
        .collect();
    let total: f64 = weights.iter().sum();

    let mut cursor = 0f64;
    let mut timed = Vec::with_capacity(text_lines.len());
    for (index, text) in text_lines.iter().enumerate() {
        let share = weights[index] / total;
        let start = (cursor * duration_ms as f64) as u64;
        timed.push(LyricLine { time_ms: Some(start), text: (*text).to_string() });
        cursor += share;
    }
    timed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stamps_with_hundredths_and_repeats() {
        let lyrics = parse(1, LyricsSource::Sidecar, "[00:12.50]first line\n[00:20.00][01:05.25]chorus\n");
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.lines[0].time_ms, Some(12_500));
        assert_eq!(lyrics.lines[2].time_ms, Some(65_250));
        assert!(lyrics.synced);
    }

    #[test]
    fn parses_hours_and_plain_text() {
        let timed = parse(1, LyricsSource::Sidecar, "[01:02:03.004]late");
        assert_eq!(timed.lines[0].time_ms, Some(3_723_004));

        let plain = parse(1, LyricsSource::Manual, "no stamps here\n\nsecond line");
        assert_eq!(plain.lines.len(), 2);
        assert!(!plain.synced);
        assert!(plain.lines[0].time_ms.is_none());
    }

    #[test]
    fn rounds_trip_through_lrc() {
        let original = parse(1, LyricsSource::Sidecar, "[00:05.10]one\n[00:09.99]two");
        let text = to_lrc(&original.lines);
        let again = parse(1, LyricsSource::Sidecar, &text);
        assert_eq!(original.lines, again.lines);
    }

    #[test]
    fn active_line_follows_position() {
        let lyrics = parse(1, LyricsSource::Sidecar, "[00:01.00]a\n[00:03.00]b\n[00:06.00]c");
        assert_eq!(active_line(&lyrics.lines, 500), None);
        assert_eq!(active_line(&lyrics.lines, 3_100), Some(1));
        assert_eq!(active_line(&lyrics.lines, 60_000), Some(2));
    }

    #[test]
    fn estimates_keep_order_and_end_inside_duration() {
        let plain = parse(1, LyricsSource::Manual, "a\nbb\nccc");
        let timed = estimate_timings(&plain.lines, 30_000);
        assert_eq!(timed.len(), 3);
        assert!(timed[0].time_ms.unwrap() < timed[1].time_ms.unwrap());
        assert!(timed[2].time_ms.unwrap() < 30_000);
    }
}
