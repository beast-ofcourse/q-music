//! One decoded file, read packet by packet and handed out as stereo f32 frames.

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::audio::sample::Sample;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::error::{CoreError, CoreResult};

/// Duration from the container's timebase when it has one, otherwise from the frame
/// count and sample rate. A missing duration is reported as zero rather than guessed.
fn track_duration_ms(track: &symphonia::core::formats::Track, sample_rate: u32) -> u64 {
    if let (Some(duration), Some(time_base)) = (track.duration, track.time_base) {
        let ticks = u128::from(duration.get());
        let numer = u128::from(time_base.numer.get());
        let denom = u128::from(time_base.denom.get());
        return (ticks * numer * 1000 / denom).min(u128::from(u64::MAX)) as u64;
    }

    track
        .num_frames
        .map(|frames| frames.saturating_mul(1000) / u64::from(sample_rate.max(1)))
        .unwrap_or(0)
}

pub struct Deck {
    path: PathBuf,
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    duration_ms: u64,
    /// Interleaved f32 samples of the packet currently being consumed.
    packet: Vec<f32>,
    cursor: usize,
    exhausted: bool,
}

impl Deck {
    pub fn open(path: &Path) -> CoreResult<Self> {
        let file = File::open(path).map_err(|error| CoreError::io(path, error))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
            hint.with_extension(extension);
        }

        // A failed probe means the container is not one Symphonia recognises; the
        // decoder error below carries the detail for files it does recognise.
        let reader = symphonia::default::get_probe()
            .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
            .map_err(|_| CoreError::UnsupportedFormat { path: path.to_path_buf() })?;

        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| CoreError::UnsupportedFormat { path: path.to_path_buf() })?;

        let track_id = track.id;
        let parameters = track
            .codec_params
            .as_ref()
            .and_then(|parameters| parameters.audio())
            .ok_or_else(|| CoreError::UnsupportedFormat { path: path.to_path_buf() })?;

        let sample_rate = parameters.sample_rate.unwrap_or(44_100);
        let channels = parameters
            .channels
            .as_ref()
            .map(|channels| channels.count())
            .unwrap_or(2)
            .max(1);

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(parameters, &AudioDecoderOptions::default())
            .map_err(|error| CoreError::Decode {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;

        let duration_ms = track_duration_ms(track, sample_rate);

        Ok(Self {
            path: path.to_path_buf(),
            reader,
            decoder,
            track_id,
            sample_rate,
            channels,
            duration_ms,
            packet: Vec::new(),
            cursor: 0,
            exhausted: false,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends up to `max_frames` stereo frames. Returns the number of frames written;
    /// zero means the file ended.
    pub fn pull(&mut self, max_frames: usize, out: &mut Vec<f32>) -> CoreResult<usize> {
        let mut frames_written = 0;

        while frames_written < max_frames {
            if self.cursor >= self.packet.len() {
                match self.fill_packet()? {
                    true => continue,
                    false => break,
                }
            }

            let remaining_frames = (self.packet.len() - self.cursor) / self.channels;
            let wanted = (max_frames - frames_written).min(remaining_frames);
            if wanted == 0 {
                break;
            }

            for _ in 0..wanted {
                let (left, right) = self.read_stereo_frame();
                out.push(left);
                out.push(right);
            }
            frames_written += wanted;
        }

        Ok(frames_written)
    }

    fn read_stereo_frame(&mut self) -> (f32, f32) {
        match self.channels {
            1 => {
                let mono = self.packet[self.cursor];
                self.cursor += 1;
                (mono, mono)
            }
            2 => {
                let left = self.packet[self.cursor];
                let right = self.packet[self.cursor + 1];
                self.cursor += 2;
                (left, right)
            }
            channels => {
                let base = self.cursor;
                let left = self.packet[base];
                let right = self.packet[base + 1];
                // Surround mixes fold down: front pair at full gain, the rest summed
                // quietly so dialogue and rear channels remain audible.
                let mut extra = 0.0;
                for channel in 2..channels {
                    extra += self.packet[base + channel];
                }
                self.cursor += channels;
                let extra = extra / (channels.saturating_sub(2)).max(1) as f32 * 0.5;
                (left + extra, right + extra)
            }
        }
    }

    /// Decodes the next packet for our track. Returns false at end of stream.
    fn fill_packet(&mut self) -> CoreResult<bool> {
        if self.exhausted {
            return Ok(false);
        }

        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => {
                    self.exhausted = true;
                    return Ok(false);
                }
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.exhausted = true;
                    return Ok(false);
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.exhausted = true;
                    return Ok(false);
                }
                Err(error) => {
                    return Err(CoreError::Decode {
                        path: self.path.clone(),
                        message: error.to_string(),
                    })
                }
            };

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    let sample_count = buffer.samples_interleaved();
                    self.packet.clear();
                    self.packet.resize(sample_count, f32::MID);
                    buffer.copy_to_slice_interleaved(self.packet.as_mut_slice());
                    self.cursor = 0;
                    if self.sample_rate == 0 {
                        self.sample_rate = buffer.spec().rate();
                    }
                    return Ok(!self.packet.is_empty());
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(SymphoniaError::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.exhausted = true;
                    return Ok(false);
                }
                Err(error) => {
                    return Err(CoreError::Decode {
                        path: self.path.clone(),
                        message: error.to_string(),
                    })
                }
            }
        }
    }

    pub fn seek_ms(&mut self, position_ms: u64) -> CoreResult<()> {
        self.reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from_millis_u64(position_ms),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|error| CoreError::Decode {
                path: self.path.clone(),
                message: format!("seek failed: {error}"),
            })?;
        self.decoder.reset();
        self.packet.clear();
        self.cursor = 0;
        self.exhausted = false;
        Ok(())
    }
}

/// Test fixtures shared by the engine, scanner and library suites.
#[cfg(test)]
pub mod tests_support {
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    /// Writes a small 16-bit stereo WAV so the decoder has something real to read.
    pub fn write_test_wav(path: &Path, seconds: f32, frequency: f32) -> std::io::Result<()> {
        let sample_rate = 44_100u32;
        let frames = (sample_rate as f32 * seconds) as u32;
        let mut file = File::create(path)?;
        let data_bytes = frames * 4;

        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data_bytes).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&2u16.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&(sample_rate * 4).to_le_bytes())?;
        file.write_all(&4u16.to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_bytes.to_le_bytes())?;

        for frame in 0..frames {
            let phase = frame as f32 / sample_rate as f32 * frequency * std::f32::consts::TAU;
            let sample = (phase.sin() * 0.4 * i16::MAX as f32) as i16;
            file.write_all(&sample.to_le_bytes())?;
            file.write_all(&sample.to_le_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::write_test_wav;
    use super::*;

    #[test]
    fn decodes_real_pcm_and_reports_rate() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 0.5, 440.0).expect("write wav");

        let mut deck = Deck::open(&path).expect("open");
        assert_eq!(deck.sample_rate(), 44_100);
        assert_eq!(deck.channels(), 2);

        let mut out = Vec::new();
        let mut total = 0;
        loop {
            let written = deck.pull(1024, &mut out).expect("pull");
            if written == 0 {
                break;
            }
            total += written;
        }
        // 0.5s at 44.1 kHz, allowing for container rounding.
        assert!(total >= 22_000, "decoded {total} frames");
        assert!(total <= 22_100, "decoded {total} frames");
    }

    #[test]
    fn seek_repositions_and_keeps_decoding() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 2.0, 220.0).expect("write wav");

        let mut deck = Deck::open(&path).expect("open");
        let mut out = Vec::new();
        deck.pull(512, &mut out).expect("pull");
        deck.seek_ms(1_000).expect("seek");
        out.clear();
        let written = deck.pull(256, &mut out).expect("pull after seek");
        assert!(written > 0);
    }

    #[test]
    fn unsupported_file_is_reported_not_panicked() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("notes.txt");
        std::fs::write(&path, b"not audio").expect("write");
        assert!(Deck::open(&path).is_err());
    }
}
