//! The playback engine: one decode thread, one lock-free ring, one output stream.
//!
//! The audio callback never allocates, never blocks and never decodes. It copies
//! frames out of a fixed ring and writes them to the device, so a slow disk or a
//! busy UI cannot cause a dropout. Everything stateful (decode, resample, EQ,
//! volume ramp) happens on the decode thread.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use crossbeam_queue::ArrayQueue;

use crate::deck::Deck;
use crate::eq::Equalizer;
use crate::error::{CoreError, CoreResult};
use crate::model::{EngineSnapshot, EqSettings, OutputDevice, PlaybackStatus};

/// Roughly 1.5 seconds of stereo at 48 kHz: enough headroom for a slow packet read,
/// small enough that a seek is felt immediately.
const RING_SAMPLES: usize = 1 << 17;
const BLOCK_FRAMES: usize = 512;
const DEFAULT_RATE: u32 = 48_000;
const PENDING_SEEK_NONE: u64 = u64::MAX;

#[derive(Debug)]
enum Command {
    Load { path: String, start_ms: u64 },
    Play,
    Pause,
    Stop,
    Seek(u64),
    Volume(f32),
    Speed(f32),
    Eq(EqSettings),
    Next(Option<String>),
    OutputRate(u32),
    Shutdown,
}

struct Shared {
    ring: ArrayQueue<f32>,
    status: AtomicU8,
    written_frames: AtomicU64,
    duration_ms: AtomicU64,
    volume: AtomicU32,
    speed: AtomicU32,
    output_rate: AtomicU32,
    pending_seek: AtomicU64,
    /// Smoothed RMS of the block being sent to the device, 0.0..=1.0. The interface
    /// plots this instead of inventing a waveform, so the meter only moves when
    /// audio is actually leaving the engine.
    level: AtomicU32,
    /// Highest sample magnitude in the last block, for the clipping edge of the plot.
    peak: AtomicU32,
    current: Mutex<Option<String>>,
    ended: Mutex<Vec<String>>,
    error: Mutex<Option<String>>,
    device: Mutex<String>,
    eq: Mutex<EqSettings>,
    next: Mutex<Option<String>>,
}

impl Shared {
    fn new(volume: f32) -> Self {
        Self {
            ring: ArrayQueue::new(RING_SAMPLES),
            status: AtomicU8::new(status_code(PlaybackStatus::Idle)),
            written_frames: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            volume: AtomicU32::new(volume.to_bits()),
            speed: AtomicU32::new(1.0f32.to_bits()),
            output_rate: AtomicU32::new(DEFAULT_RATE),
            pending_seek: AtomicU64::new(PENDING_SEEK_NONE),
            level: AtomicU32::new(0.0f32.to_bits()),
            peak: AtomicU32::new(0.0f32.to_bits()),
            current: Mutex::new(None),
            ended: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            device: Mutex::new(String::new()),
            eq: Mutex::new(EqSettings::flat()),
            next: Mutex::new(None),
        }
    }

    fn status(&self) -> PlaybackStatus {
        status_from_code(self.status.load(Ordering::Relaxed))
    }

    fn set_status(&self, status: PlaybackStatus) {
        self.status.store(status_code(status), Ordering::Relaxed);
    }

    fn set_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(message.into());
        }
        self.set_status(PlaybackStatus::Error);
    }
}

fn status_code(status: PlaybackStatus) -> u8 {
    match status {
        PlaybackStatus::Idle => 0,
        PlaybackStatus::Playing => 1,
        PlaybackStatus::Paused => 2,
        PlaybackStatus::Error => 3,
    }
}

fn status_from_code(code: u8) -> PlaybackStatus {
    match code {
        1 => PlaybackStatus::Playing,
        2 => PlaybackStatus::Paused,
        3 => PlaybackStatus::Error,
        _ => PlaybackStatus::Idle,
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    commands: Sender<Command>,
    stream: Mutex<Option<cpal::Stream>>,
    output_error: Mutex<Option<String>>,
}

impl Engine {
    /// Starts the engine against the named device, or the system default.
    /// A machine with no output device still gets a working engine; it reports the
    /// failure in its snapshot instead of refusing to start.
    pub fn start(device: Option<String>) -> Self {
        let shared = Arc::new(Shared::new(0.85));
        let (commands, receiver) = mpsc::channel();

        match open_stream(&shared, device.as_deref()) {
            Ok((stream, name, rate)) => {
                if let Ok(mut slot) = shared.device.lock() {
                    *slot = name;
                }
                shared.output_rate.store(rate, Ordering::Relaxed);
                if let Err(error) = stream.play() {
                    shared.set_error(format!("audio output could not start: {error}"));
                }
                let engine = Self {
                    shared: shared.clone(),
                    commands,
                    stream: Mutex::new(Some(stream)),
                    output_error: Mutex::new(None),
                };
                engine.spawn_producer(receiver);
                engine
            }
            Err(error) => {
                let engine = Self {
                    shared: shared.clone(),
                    commands,
                    stream: Mutex::new(None),
                    output_error: Mutex::new(Some(error.clone())),
                };
                shared.set_error(error);
                engine.spawn_producer(receiver);
                engine
            }
        }
    }

    /// Test-only engine: no device is opened, a drain thread empties the ring so
    /// position advances exactly as it would with a device attached.
    pub fn silent() -> Self {
        let shared = Arc::new(Shared::new(0.85));
        let (commands, receiver) = mpsc::channel();
        shared.output_rate.store(DEFAULT_RATE, Ordering::Relaxed);
        let drain = shared.clone();
        std::thread::Builder::new()
            .name("qm-drain".into())
            .spawn(move || {
                while drain.status.load(Ordering::Relaxed) != 255 {
                    while drain.ring.pop().is_some() {}
                    std::thread::sleep(Duration::from_millis(4));
                }
            })
            .ok();
        let engine = Self {
            shared: shared.clone(),
            commands,
            stream: Mutex::new(None),
            output_error: Mutex::new(None),
        };
        engine.spawn_producer(receiver);
        engine
    }

    fn spawn_producer(&self, receiver: Receiver<Command>) {
        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("qm-decode".into())
            .spawn(move || run_producer(shared, receiver))
            .ok();
    }

    fn send(&self, command: Command) -> CoreResult<()> {
        self.commands
            .send(command)
            .map_err(|_| CoreError::Device("playback thread stopped".to_string()))
    }

    /// Decodes and starts a file. `start_ms` resumes a previous position.
    pub fn load(&self, path: &str, start_ms: u64) -> CoreResult<()> {
        if let Ok(mut slot) = self.shared.error.lock() {
            *slot = None;
        }
        self.send(Command::Load { path: path.to_string(), start_ms })
    }

    pub fn play(&self) -> CoreResult<()> {
        self.send(Command::Play)
    }

    pub fn pause(&self) -> CoreResult<()> {
        self.send(Command::Pause)
    }

    pub fn stop(&self) -> CoreResult<()> {
        self.send(Command::Stop)
    }

    pub fn seek(&self, position_ms: u64) -> CoreResult<()> {
        self.send(Command::Seek(position_ms))
    }

    pub fn set_volume(&self, volume: f32) -> CoreResult<()> {
        let clamped = volume.clamp(0.0, 1.0);
        self.shared.volume.store(clamped.to_bits(), Ordering::Relaxed);
        self.send(Command::Volume(clamped))
    }

    pub fn set_speed(&self, speed: f32) -> CoreResult<()> {
        let clamped = speed.clamp(0.5, 2.0);
        self.shared.speed.store(clamped.to_bits(), Ordering::Relaxed);
        self.send(Command::Speed(clamped))
    }

    pub fn set_eq(&self, settings: EqSettings) -> CoreResult<()> {
        let settings = settings.normalized();
        if let Ok(mut slot) = self.shared.eq.lock() {
            *slot = settings.clone();
        }
        self.send(Command::Eq(settings))
    }

    /// The next file, decoded back to back so albums do not gap.
    pub fn set_next(&self, path: Option<String>) -> CoreResult<()> {
        self.send(Command::Next(path))
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let buffered_frames = (self.shared.ring.len() / 2) as u64;
        let written = self.shared.written_frames.load(Ordering::Relaxed);
        let audible_frames = written.saturating_sub(buffered_frames);
        let rate = self.shared.output_rate.load(Ordering::Relaxed).max(1) as u64;

        let ended = self
            .shared
            .ended
            .lock()
            .map(|mut queue| {
                let drained: Vec<String> = queue.drain(..).collect();
                drained.last().cloned()
            })
            .ok()
            .flatten();

        EngineSnapshot {
            status: self.shared.status(),
            path: self.shared.current.lock().ok().and_then(|current| current.clone()),
            position_ms: audible_frames.saturating_mul(1000) / rate,
            duration_ms: self.shared.duration_ms.load(Ordering::Relaxed),
            volume: f32::from_bits(self.shared.volume.load(Ordering::Relaxed)),
            speed: f32::from_bits(self.shared.speed.load(Ordering::Relaxed)),
            device: self.shared.device.lock().map(|name| name.clone()).unwrap_or_default(),
            sample_rate: self.shared.output_rate.load(Ordering::Relaxed),
            level: f32::from_bits(self.shared.level.load(Ordering::Relaxed)),
            peak: f32::from_bits(self.shared.peak.load(Ordering::Relaxed)),
            ended_path: ended,
            error: self
                .shared
                .error
                .lock()
                .ok()
                .and_then(|error| error.clone())
                .or_else(|| self.output_error.lock().ok().and_then(|error| error.clone())),
        }
    }

    pub fn devices(&self) -> Vec<OutputDevice> {
        let host = cpal::default_host();
        let default_name = host
            .default_output_device()
            .and_then(|device| device_name(&device))
            .unwrap_or_default();

        let mut names: Vec<String> = host
            .output_devices()
            .map(|devices| devices.filter_map(|device| device_name(&device)).collect())
            .unwrap_or_default();
        names.sort_by_key(|name| name.to_ascii_lowercase());
        names.dedup();

        names
            .into_iter()
            .map(|name| OutputDevice { is_default: name == default_name, name })
            .collect()
    }

    /// Switches output device, keeping playback position and queue.
    pub fn set_device(&self, name: &str) -> CoreResult<()> {
        let snapshot = self.snapshot();
        if let Some(stream) = self.stream.lock().ok().and_then(|mut slot| slot.take()) {
            drop(stream);
        }

        let (stream, device_name, rate) =
            open_stream(&self.shared, Some(name)).map_err(CoreError::Device)?;
        stream.play().map_err(|error| CoreError::Device(error.to_string()))?;
        if let Ok(mut slot) = self.stream.lock() {
            *slot = Some(stream);
        }
        if let Ok(mut slot) = self.shared.device.lock() {
            *slot = device_name;
        }
        if let Ok(mut slot) = self.output_error.lock() {
            *slot = None;
        }
        self.shared.output_rate.store(rate, Ordering::Relaxed);
        self.send(Command::OutputRate(rate))?;

        if let Some(path) = snapshot.path {
            self.load(&path, snapshot.position_ms)?;
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// cpal 0.18 reports a full description; the human-readable name is the piece we
/// store and match on when the user picks a device.
fn device_name(device: &cpal::Device) -> Option<String> {
    device
        .description()
        .ok()
        .map(|description| description.name().to_string())
}

fn open_stream(
    shared: &Arc<Shared>,
    device_name_filter: Option<&str>,
) -> Result<(cpal::Stream, String, u32), String> {
    let host = cpal::default_host();
    let device = match device_name_filter.filter(|name| !name.is_empty()) {
        Some(wanted) => host
            .output_devices()
            .map_err(|error| error.to_string())?
            .find(|device| device_name(device).as_deref() == Some(wanted))
            .ok_or_else(|| format!("output device \"{wanted}\" is not available"))?,
        None => host
            .default_output_device()
            .ok_or_else(|| "no audio output device found".to_string())?,
    };

    let name = device_name(&device).unwrap_or_else(|| "System output".to_string());
    let supported = device.default_output_config().map_err(|error| error.to_string())?;
    let sample_format = supported.sample_format();
    let config = supported.config();
    let rate = config.sample_rate;
    let channels = config.channels as usize;

    let error_state = shared.clone();
    let f32_source = shared.clone();
    let i16_source = shared.clone();
    let u16_source = shared.clone();
    let on_error = move |error| {
        if let Ok(mut slot) = error_state.error.lock() {
            *slot = Some(format!("audio output error: {error}"));
        }
        error_state.set_status(PlaybackStatus::Error);
    };

    let stream = match sample_format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |output: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                copy_block(&f32_source, output, channels);
            },
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |output: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                let mut block = vec![0f32; output.len()];
                copy_block(&i16_source, &mut block, channels);
                for (slot, value) in output.iter_mut().zip(block.iter()) {
                    *slot = (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                }
            },
            on_error,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |output: &mut [u16], _info: &cpal::OutputCallbackInfo| {
                let mut block = vec![0f32; output.len()];
                copy_block(&u16_source, &mut block, channels);
                for (slot, value) in output.iter_mut().zip(block.iter()) {
                    let normalized = value.clamp(-1.0, 1.0) * 0.5 + 0.5;
                    *slot = (normalized * u16::MAX as f32) as u16;
                }
            },
            on_error,
            None,
        ),
        other => return Err(format!("output device sample format {other:?} is not supported")),
    };

    stream.map(|stream| (stream, name, rate)).map_err(|error| error.to_string())
}

fn copy_block(shared: &Arc<Shared>, output: &mut [f32], channels: usize) {
    let playing = shared.status() == PlaybackStatus::Playing;
    let frames = output.len() / channels.max(1);
    for frame in 0..frames {
        let left = if playing { shared.ring.pop().unwrap_or(0.0) } else { 0.0 };
        let right = if playing { shared.ring.pop().unwrap_or(0.0) } else { 0.0 };
        for channel in 0..channels {
            let value = if channels == 1 {
                (left + right) * 0.5
            } else if channel % 2 == 0 {
                left
            } else {
                right
            };
            output[frame * channels + channel] = value;
        }
    }
    // Drain the ring when not playing so stale samples don't accumulate.
    if !playing {
        while shared.ring.pop().is_some() {}
    }
}

fn run_producer(shared: Arc<Shared>, commands: Receiver<Command>) {
    let mut deck: Option<Deck> = None;
    let mut eq = Equalizer::new(shared.output_rate.load(Ordering::Relaxed));
    let mut eq_settings = shared.eq.lock().map(|settings| settings.clone()).unwrap_or_else(|_| EqSettings::flat());
    let mut input: Vec<f32> = Vec::with_capacity(16_384);
    let mut read_index: f64 = 0.0;
    let mut written: u64 = 0;
    let mut smoothed_volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
    let mut meter_level = 0f32;

    loop {
        // Commands first: a seek or a new track must win over buffered audio.
        loop {
            match commands.try_recv() {
                Ok(Command::Shutdown) => return,
                Ok(command) => {
                    apply_command(
                        &shared,
                        command,
                        &mut deck,
                        &mut eq,
                        &mut eq_settings,
                        &mut input,
                        &mut read_index,
                        &mut written,
                    );
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        let status = shared.status();
        if status != PlaybackStatus::Playing || deck.is_none() {
            // Nothing is leaving the engine, so the meter falls to zero instead of
            // freezing at whatever was playing when the user hit pause.
            shared.level.store(0.0f32.to_bits(), Ordering::Relaxed);
            shared.peak.store(0.0f32.to_bits(), Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(6));
            continue;
        }

        let output_rate = shared.output_rate.load(Ordering::Relaxed).max(1);
        let speed = f32::from_bits(shared.speed.load(Ordering::Relaxed)).clamp(0.5, 2.0);
        let target_volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));

        let Some(current_deck) = deck.as_mut() else {
            continue;
        };

        let step = (current_deck.sample_rate() as f64 / output_rate as f64) * speed as f64;
        let mut produced = 0usize;
        let mut sum_squares = 0f64;
        let mut block_peak = 0f32;

        while produced < BLOCK_FRAMES && !shared.ring.is_full() {
            let frames_available = input.len() / 2;
            if read_index as usize + 1 >= frames_available {
                compact(&mut input, &mut read_index);
                let mut pulled = Vec::with_capacity(4_096);
                match current_deck.pull(2_048, &mut pulled) {
                    Ok(0) => {
                        handle_end_of_track(
                            &shared,
                            &mut deck,
                            &mut eq,
                            &mut input,
                            &mut read_index,
                            &mut written,
                        );
                        break;
                    }
                    Ok(_) => input.extend_from_slice(&pulled),
                    Err(error) => {
                        shared.set_error(error.user_message());
                        shared.set_status(PlaybackStatus::Error);
                        break;
                    }
                }
                continue;
            }

            let index = read_index.floor() as usize;
            let fraction = (read_index - index as f64) as f32;
            let left_a = input[index * 2];
            let right_a = input[index * 2 + 1];
            let left_b = input[index * 2 + 2];
            let right_b = input[index * 2 + 3];

            let left = left_a + (left_b - left_a) * fraction;
            let right = right_a + (right_b - right_a) * fraction;

            let (left, right) = eq.process(left, right);
            smoothed_volume += (target_volume - smoothed_volume) * 0.25;
            let left = left * smoothed_volume;
            let right = right * smoothed_volume;
            sum_squares += f64::from(left) * f64::from(left) + f64::from(right) * f64::from(right);
            block_peak = block_peak.max(left.abs()).max(right.abs());
            let _ = shared.ring.push(left);
            let _ = shared.ring.push(right);

            read_index += step;
            written += 1;
            produced += 1;
        }

        shared.written_frames.store(written, Ordering::Relaxed);
        publish_level(shared.as_ref(), &mut meter_level, sum_squares, block_peak, produced);

        if produced == 0 {
            std::thread::sleep(Duration::from_millis(3));
        }
    }
}

/// How fast the level meter falls between blocks. Chosen so the plot breathes at a
/// readable pace instead of flickering with every 20 ms block.
const LEVEL_RELEASE: f32 = 0.94;

/// Publishes meter ballistics: the level jumps up on a transient and falls smoothly,
/// the way a hardware meter does. `meter_level` carries the fall between blocks.
fn publish_level(
    shared: &Shared,
    meter_level: &mut f32,
    sum_squares: f64,
    peak: f32,
    frames: usize,
) {
    if frames == 0 {
        return;
    }

    let samples = (frames * 2).max(1) as f64;
    let rms = (sum_squares / samples).sqrt() as f32;
    *meter_level = if rms > *meter_level {
        rms
    } else {
        *meter_level * LEVEL_RELEASE
    };

    shared
        .level
        .store((*meter_level).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    shared
        .peak
        .store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn compact(input: &mut Vec<f32>, read_index: &mut f64) {    let whole_frames = read_index.floor() as usize;
    if whole_frames > 4_096 {
        let drain = whole_frames * 2;
        input.drain(..drain);
        *read_index -= whole_frames as f64;
    }
}

fn handle_end_of_track(
    shared: &Arc<Shared>,
    deck: &mut Option<Deck>,
    eq: &mut Equalizer,
    input: &mut Vec<f32>,
    read_index: &mut f64,
    written: &mut u64,
) {
    let finished = deck.as_ref().map(|deck| deck.path().display().to_string());
    let next_path = shared.next.lock().ok().and_then(|next| next.clone());

    // The swap completes before the finished track is reported: the UI advances its
    // queue the moment `ended_path` appears, and a snapshot taken then must already
    // name the new track as current.
    match next_path {
        Some(next_path) => match Deck::open(std::path::Path::new(&next_path)) {
            Ok(next_deck) => {
                shared.duration_ms.store(next_deck.duration_ms(), Ordering::Relaxed);
                if let Ok(mut slot) = shared.current.lock() {
                    *slot = Some(next_path);
                }
                *deck = Some(next_deck);
                input.clear();
                *read_index = 0.0;
                *written = 0;
                eq.reset();
            }
            Err(error) => {
                shared.set_error(error.user_message());
                *deck = None;
            }
        },
        None => {
            *deck = None;
            shared.set_status(PlaybackStatus::Idle);
        }
    }

    if let Some(path) = finished {
        if let Ok(mut queue) = shared.ended.lock() {
            queue.push(path);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_command(
    shared: &Arc<Shared>,
    command: Command,
    deck: &mut Option<Deck>,
    eq: &mut Equalizer,
    eq_settings: &mut EqSettings,
    input: &mut Vec<f32>,
    read_index: &mut f64,
    written: &mut u64,
) {
    match command {
        Command::Load { path, start_ms } => {
            while shared.ring.pop().is_some() {}
            match Deck::open(std::path::Path::new(&path)) {
                Ok(mut opened) => {
                    if start_ms > 0 {
                        if let Err(error) = opened.seek_ms(start_ms) {
                            shared.set_error(error.user_message());
                            *deck = None;
                            return;
                        }
                    }
                    shared.duration_ms.store(opened.duration_ms(), Ordering::Relaxed);
                    if let Ok(mut slot) = shared.current.lock() {
                        *slot = Some(path);
                    }
                    if let Ok(mut slot) = shared.error.lock() {
                        *slot = None;
                    }
                    eq.reset();
                    input.clear();
                    *read_index = 0.0;
                    *written = (start_ms * shared.output_rate.load(Ordering::Relaxed) as u64) / 1000;
                    shared.written_frames.store(*written, Ordering::Relaxed);
                    *deck = Some(opened);
                    shared.set_status(PlaybackStatus::Playing);
                }
                Err(error) => {
                    shared.set_error(error.user_message());
                    *deck = None;
                }
            }
        }
        Command::Play => {
            if deck.is_some() {
                shared.set_status(PlaybackStatus::Playing);
            }
        }
        Command::Pause => {
            if shared.status() == PlaybackStatus::Playing {
                shared.set_status(PlaybackStatus::Paused);
            }
        }
        Command::Stop => {
            while shared.ring.pop().is_some() {}
            *deck = None;
            input.clear();
            *read_index = 0.0;
            *written = 0;
            shared.written_frames.store(0, Ordering::Relaxed);
            if let Ok(mut slot) = shared.current.lock() {
                *slot = None;
            }
            shared.set_status(PlaybackStatus::Idle);
        }
        Command::Seek(position_ms) => {
            if let Some(current) = deck.as_mut() {
                if let Err(error) = current.seek_ms(position_ms) {
                    shared.set_error(error.user_message());
                    return;
                }
                while shared.ring.pop().is_some() {}
                input.clear();
                *read_index = 0.0;
                *written = (position_ms * shared.output_rate.load(Ordering::Relaxed) as u64) / 1000;
                shared.written_frames.store(*written, Ordering::Relaxed);
                eq.reset();
            }
            shared.pending_seek.store(PENDING_SEEK_NONE, Ordering::Relaxed);
        }
        Command::Volume(volume) => {
            shared.volume.store(volume.to_bits(), Ordering::Relaxed);
        }
        Command::Speed(speed) => {
            shared.speed.store(speed.to_bits(), Ordering::Relaxed);
        }
        Command::Eq(settings) => {
            *eq_settings = settings.clone();
            eq.set_sample_rate(shared.output_rate.load(Ordering::Relaxed));
            eq.configure(&settings);
        }
        Command::Next(path) => {
            if let Ok(mut slot) = shared.next.lock() {
                *slot = path;
            }
        }
        Command::OutputRate(rate) => {
            shared.output_rate.store(rate, Ordering::Relaxed);
            eq.set_sample_rate(rate);
            eq.configure(eq_settings);
        }
        Command::Shutdown => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn write_test_wav(path: &std::path::Path, seconds: f32) {
        crate::deck::tests_support::write_test_wav(path, seconds, 440.0).expect("write wav");
    }

    fn wait_for(mut predicate: impl FnMut() -> bool, timeout_ms: u128) -> bool {
        let start = Instant::now();
        while start.elapsed().as_millis() < timeout_ms {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn loads_plays_pauses_and_reports_position() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 1.0);

        let engine = Engine::silent();
        engine.load(path.to_str().expect("path"), 0).expect("load");
        assert!(wait_for(|| engine.snapshot().position_ms > 150, 2_000));

        engine.pause().expect("pause");
        std::thread::sleep(Duration::from_millis(120));
        let paused_at = engine.snapshot().position_ms;
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(engine.snapshot().status, PlaybackStatus::Paused);
        assert!(engine.snapshot().position_ms.abs_diff(paused_at) < 60);

        engine.play().expect("play");
        assert!(wait_for(|| engine.snapshot().position_ms > paused_at + 60, 2_000));
    }

    #[test]
    fn seek_moves_the_position() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 4.0);

        let engine = Engine::silent();
        engine.load(path.to_str().expect("path"), 0).expect("load");
        std::thread::sleep(Duration::from_millis(80));
        engine.seek(2_000).expect("seek");
        assert!(wait_for(|| engine.snapshot().position_ms >= 2_000, 1_500));
    }

    #[test]
    fn bad_path_reports_an_error_without_stopping_the_engine() {
        let engine = Engine::silent();
        engine.load("C:/definitely/not/here.wav", 0).expect("command accepted");
        assert!(wait_for(|| engine.snapshot().error.is_some(), 1_000));
        assert_eq!(engine.snapshot().status, PlaybackStatus::Error);
    }

    #[test]
    fn gapless_swap_reports_the_finished_track() {
        let directory = tempfile::tempdir().expect("temp dir");
        let first = directory.path().join("one.wav");
        let second = directory.path().join("two.wav");
        write_test_wav(&first, 0.35);
        write_test_wav(&second, 2.0);

        let engine = Engine::silent();
        engine.set_next(Some(second.display().to_string())).expect("next");
        engine.load(first.to_str().expect("path"), 0).expect("load");

        assert!(wait_for(|| engine.snapshot().ended_path.is_some(), 3_000));
        assert_eq!(
            engine.snapshot().path.as_deref(),
            Some(second.display().to_string().as_str())
        );
        assert_eq!(engine.snapshot().status, PlaybackStatus::Playing);
    }

    #[test]
    fn speed_changes_advance_rate() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("tone.wav");
        write_test_wav(&path, 6.0);

        let engine = Engine::silent();
        engine.set_speed(2.0).expect("speed");
        engine.load(path.to_str().expect("path"), 0).expect("load");
        assert!(wait_for(|| engine.snapshot().position_ms > 400, 2_000));
        // At double speed the reported position is still wall-clock audio time, so
        // the track simply reaches its end sooner; assert the rate is stored.
        assert!((engine.snapshot().speed - 2.0).abs() < f32::EPSILON);
    }
}
