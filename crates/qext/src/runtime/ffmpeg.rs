//! The `ffmpeg` object extensions call from a post-processing hook.
//!
//! Two rules shape this module:
//!
//! - **Only a configured binary runs.** There is no PATH lookup and no
//!   auto-detection: a host that has not been told where ffmpeg lives answers
//!   with a typed `unsupported` error, so a package can report a reason instead
//!   of a mystery failure.
//! - **Nothing goes through a shell.** The argument string a package supplies is
//!   split here and handed to `Command` as separate argv entries, so a quoted
//!   path cannot become a second command. URLs are refused outright: ffmpeg's
//!   own protocol handlers would otherwise be an unchecked network client that
//!   ignores the manifest allowlist.
//!
//! stdout is discarded. ffmpeg writes progress diagnostics to stderr, and the
//! documented `output` field is the tail of that, which is what a failing hook
//! needs in order to say what went wrong.

use serde::Deserialize;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// How long a single ffmpeg invocation may run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// Bytes of stderr kept for the `output` field.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 32 * 1024;
/// How often a running child is polled.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Upper bound on one `-ar` value.
const MAX_SAMPLE_RATE: u32 = 768_000;
/// Upper bound on one `-ac` value.
const MAX_CHANNELS: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for FfmpegLimits {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FfmpegError {
    #[error("ffmpeg is not configured for this host; set an ffmpeg binary path in settings")]
    Unsupported,

    #[error("ffmpeg arguments are unusable: {reason}")]
    InvalidArguments { reason: String },

    #[error("ffmpeg is configured but cannot be started: {message}")]
    Spawn { message: String },

    #[error("ffmpeg timed out after {seconds} seconds: {message}")]
    Timeout { seconds: u64, message: String },

    #[error("ffmpeg exited with {code}: {message}")]
    Exit { code: i32, message: String },
}

impl FfmpegError {
    /// The `error_type` a package sees, matching the platform's failure keys.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::Spawn { .. } => "unavailable",
            Self::Timeout { .. } => "timeout",
            Self::Exit { .. } => "ffmpeg_error",
        }
    }

    /// The text a hook can surface without leaking host internals.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

/// Options accepted by `ffmpeg.convert`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ConvertOptions {
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub bitrate: Option<String>,
    #[serde(default, alias = "sampleRate")]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub channels: Option<u32>,
}

/// One configured ffmpeg binary.
pub struct Ffmpeg {
    binary: PathBuf,
    limits: FfmpegLimits,
}

impl std::fmt::Debug for Ffmpeg {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Ffmpeg")
            .field("binary", &self.binary)
            .field("limits", &self.limits)
            .finish()
    }
}

impl Ffmpeg {
    pub fn new(binary: impl Into<PathBuf>, limits: FfmpegLimits) -> Self {
        Self {
            binary: binary.into(),
            limits,
        }
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Run one raw argument string, e.g. `-i "in.flac" -af loudnorm -y "out.flac"`.
    pub fn execute(&self, arguments: &str) -> Result<String, FfmpegError> {
        let argv = split_arguments(arguments)?;
        self.run(&argv)
    }

    /// Build and run the documented conversion helper.
    pub fn convert(
        &self,
        input: &str,
        output: &str,
        options: &ConvertOptions,
    ) -> Result<String, FfmpegError> {
        if input.trim().is_empty() || output.trim().is_empty() {
            return Err(FfmpegError::InvalidArguments {
                reason: "input and output paths are required".to_string(),
            });
        }

        let mut argv = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            input.to_string(),
        ];

        if let Some(codec) = options.codec.as_deref() {
            argv.push("-c:a".to_string());
            argv.push(check_codec(codec)?);
        }
        if let Some(bitrate) = options.bitrate.as_deref() {
            argv.push("-b:a".to_string());
            argv.push(check_bitrate(bitrate)?);
        }
        if let Some(rate) = options.sample_rate {
            if !(1..=MAX_SAMPLE_RATE).contains(&rate) {
                return Err(FfmpegError::InvalidArguments {
                    reason: format!("sample_rate {rate} is outside 1..={MAX_SAMPLE_RATE}"),
                });
            }
            argv.push("-ar".to_string());
            argv.push(rate.to_string());
        }
        if let Some(channels) = options.channels {
            if !(1..=MAX_CHANNELS).contains(&channels) {
                return Err(FfmpegError::InvalidArguments {
                    reason: format!("channels {channels} is outside 1..={MAX_CHANNELS}"),
                });
            }
            argv.push("-ac".to_string());
            argv.push(channels.to_string());
        }

        argv.push(output.to_string());
        self.run(&argv)
    }

    fn run(&self, argv: &[String]) -> Result<String, FfmpegError> {
        let mut child = Command::new(&self.binary)
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| FfmpegError::Spawn {
                message: format!("{}: {error}", self.binary.display()),
            })?;

        let stderr = child.stderr.take().ok_or_else(|| FfmpegError::Spawn {
            message: "ffmpeg stderr was not captured".to_string(),
        })?;

        // Drained on its own thread: a conversion writes far more diagnostics
        // than a pipe buffer holds, and a full pipe would block ffmpeg forever.
        let limit = self.limits.max_output_bytes;
        let reader = thread::spawn(move || read_tail(stderr, limit));

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let output = join_output(reader);
                    return if status.success() {
                        Ok(output)
                    } else {
                        Err(FfmpegError::Exit {
                            code: status.code().unwrap_or(-1),
                            message: output,
                        })
                    };
                }
                Ok(None) => {
                    if started.elapsed() >= self.limits.timeout {
                        stop(&mut child);
                        let output = join_output(reader);
                        return Err(FfmpegError::Timeout {
                            seconds: self.limits.timeout.as_secs(),
                            message: output,
                        });
                    }
                    thread::sleep(POLL_INTERVAL);
                }
                Err(error) => {
                    stop(&mut child);
                    let _ = reader.join();
                    return Err(FfmpegError::Spawn {
                        message: format!("cannot read ffmpeg status: {error}"),
                    });
                }
            }
        }
    }
}

fn join_output(reader: thread::JoinHandle<String>) -> String {
    reader.join().unwrap_or_default()
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Split an argument string the way a shell would, without involving a shell.
///
/// Supports double quotes (with backslash escapes), single quotes (literal) and
/// whitespace separation. Anything else is taken verbatim.
pub fn split_arguments(input: &str) -> Result<Vec<String>, FfmpegError> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut in_argument = false;
    let mut quote: Option<char> = None;
    let mut characters = input.chars();

    while let Some(character) = characters.next() {
        match quote {
            Some('"') => match character {
                '"' => quote = None,
                // A backslash only escapes a quote or another backslash.
                // Windows paths are written with backslashes and a shell-style
                // escape would silently eat them.
                '\\' => push_escaped(&mut current, &mut characters, &mut in_argument),
                other => current.push(other),
            },
            Some('\'') => match character {
                '\'' => quote = None,
                other => current.push(other),
            },
            Some(_) => current.push(character),
            None => match character {
                '"' | '\'' => {
                    quote = Some(character);
                    in_argument = true;
                }
                '\\' => push_escaped(&mut current, &mut characters, &mut in_argument),
                character if character.is_whitespace() => {
                    if in_argument {
                        arguments.push(std::mem::take(&mut current));
                        in_argument = false;
                    }
                }
                other => {
                    current.push(other);
                    in_argument = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(FfmpegError::InvalidArguments {
            reason: "unterminated quote".to_string(),
        });
    }

    if in_argument {
        arguments.push(current);
    }

    if arguments.is_empty() {
        return Err(FfmpegError::InvalidArguments {
            reason: "no arguments were supplied".to_string(),
        });
    }

    for argument in &arguments {
        if carries_protocol(argument) {
            return Err(FfmpegError::InvalidArguments {
                reason: format!(
                    "{argument:?} names a protocol; ffmpeg may only read local files here"
                ),
            });
        }
    }

    Ok(arguments)
}

/// Protocols that would let a hook use ffmpeg as a second network client.
///
/// Local protocols (`file:`, `pipe:`, `concat:`) stay available: they cannot
/// reach a host the manifest never declared. A Windows drive letter (`C:\media`)
/// is not matched because no scheme is ever one character long.
const NETWORK_SCHEMES: [&str; 12] = [
    "http", "https", "ftp", "ftps", "tcp", "udp", "rtmp", "rtmps", "srt", "gopher", "tls", "quic",
];

fn carries_protocol(argument: &str) -> bool {
    let lowered = argument.to_ascii_lowercase();

    NETWORK_SCHEMES
        .iter()
        .any(|scheme| lowered.starts_with(&format!("{scheme}:")))
}

fn check_codec(codec: &str) -> Result<String, FfmpegError> {
    let trimmed = codec.trim();
    let shaped = !trimmed.is_empty()
        && !trimmed.starts_with('-')
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'));

    if shaped {
        Ok(trimmed.to_string())
    } else {
        Err(FfmpegError::InvalidArguments {
            reason: format!("codec {codec:?} is not a codec name"),
        })
    }
}

fn check_bitrate(bitrate: &str) -> Result<String, FfmpegError> {
    let trimmed = bitrate.trim();
    let digits = trimmed.strip_suffix(['k', 'K', 'm', 'M']).unwrap_or(trimmed);
    let shaped = !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit());

    if shaped {
        Ok(trimmed.to_string())
    } else {
        Err(FfmpegError::InvalidArguments {
            reason: format!("bitrate {bitrate:?} is not a bitrate"),
        })
    }
}

/// Consume one escaped character.
///
/// Only a quote, a backslash or a space is escaped; any other backslash is kept
/// literally so a Windows path survives unchanged.
fn push_escaped(
    current: &mut String,
    characters: &mut std::str::Chars<'_>,
    in_argument: &mut bool,
) {
    match characters.clone().next() {
        Some(next) if matches!(next, '"' | '\'' | '\\' | ' ') => {
            current.push(next);
            characters.next();
        }
        _ => current.push('\\'),
    }
    *in_argument = true;
}

/// Keep the last `limit` bytes of a stream while draining all of it.
fn read_tail(mut stream: impl Read, limit: usize) -> String {
    let limit = limit.max(1);
    let mut kept: VecDeque<u8> = VecDeque::with_capacity(limit);
    let mut chunk = [0u8; 8192];

    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };

        for byte in &chunk[..read] {
            if kept.len() == limit {
                kept.pop_front();
            }
            kept.push_back(*byte);
        }
    }

    String::from_utf8_lossy(&kept.into_iter().collect::<Vec<u8>>())
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_ffmpeg_is_a_typed_unsupported_error() {
        let error = FfmpegError::Unsupported;
        assert_eq!(error.kind(), "unsupported");
        assert!(error.message().contains("not configured"));
    }

    #[test]
    fn quoted_arguments_split_without_a_shell() {
        let argv = split_arguments(r#"-i "C:\Music\a b.flac" -af "loudnorm=I=-16" -y "out.mp3""#)
            .expect("splits");

        assert_eq!(
            argv,
            vec![
                "-i",
                r"C:\Music\a b.flac",
                "-af",
                "loudnorm=I=-16",
                "-y",
                "out.mp3",
            ]
        );
    }

    #[test]
    fn shell_operators_are_plain_arguments() {
        // `&` and `|` must reach ffmpeg as text, never as a command separator.
        let argv = split_arguments(r#"-i in.flac && rm -rf /"#).expect("splits");
        assert_eq!(argv, vec!["-i", "in.flac", "&&", "rm", "-rf", "/"]);
    }

    #[test]
    fn network_urls_are_refused_and_local_protocols_are_not() {
        for arguments in [
            "-i https://example.test/a.flac out.flac",
            "-i http://example.test/a.flac out.flac",
            "-i tcp://example.test:9000 out.flac",
            "-i rtmp://example.test/live out.flac",
        ] {
            let error = split_arguments(arguments).expect_err("refused");
            assert!(
                matches!(error, FfmpegError::InvalidArguments { .. }),
                "{arguments}: {error:?}"
            );
        }

        // A Windows drive is not a protocol, and local protocols are allowed.
        assert!(split_arguments(r#"-i C:\Music\a.flac out.flac"#).is_ok());
        assert!(split_arguments("-i concat:list.txt out.flac").is_ok());
    }

    #[test]
    fn unbalanced_quotes_and_empty_input_are_refused() {
        assert_eq!(
            split_arguments(r#"-i "unterminated"#).unwrap_err(),
            FfmpegError::InvalidArguments {
                reason: "unterminated quote".to_string()
            }
        );
        assert_eq!(
            split_arguments("   ").unwrap_err(),
            FfmpegError::InvalidArguments {
                reason: "no arguments were supplied".to_string()
            }
        );
    }

    #[test]
    fn a_missing_binary_fails_without_panicking() {
        let ffmpeg = Ffmpeg::new("definitely-not-ffmpeg-xyz", FfmpegLimits::default());
        let error = match ffmpeg.execute("-version") {
            Ok(output) => panic!("unexpected success: {output}"),
            Err(error) => error,
        };
        assert!(matches!(error, FfmpegError::Spawn { .. }), "{error:?}");
        assert_eq!(error.kind(), "unavailable");
    }

    #[test]
    fn convert_rejects_flags_disguised_as_values() {
        let ffmpeg = Ffmpeg::new("missing", FfmpegLimits::default());
        let error = ffmpeg
            .convert(
                "in.flac",
                "out.mp3",
                &ConvertOptions {
                    codec: Some("-y".to_string()),
                    ..ConvertOptions::default()
                },
            )
            .expect_err("refused");
        assert!(matches!(error, FfmpegError::InvalidArguments { .. }));

        let error = ffmpeg
            .convert(
                "in.flac",
                "out.mp3",
                &ConvertOptions {
                    channels: Some(9),
                    ..ConvertOptions::default()
                },
            )
            .expect_err("refused");
        assert!(matches!(error, FfmpegError::InvalidArguments { .. }));
    }

    /// Runs only when the developer points the suite at a real binary:
    /// `QEXT_TEST_FFMPEG=C:\path\to\ffmpeg.exe cargo test -p qext ffmpeg::`.
    ///
    /// The default suite never needs ffmpeg installed.
    #[test]
    fn a_configured_binary_runs_when_one_is_provided() {
        let Ok(binary) = std::env::var("QEXT_TEST_FFMPEG") else {
            return;
        };

        let ffmpeg = Ffmpeg::new(binary, FfmpegLimits::default());
        let output = ffmpeg.execute("-hide_banner -version").expect("runs");
        assert!(output.to_lowercase().contains("ffmpeg"));

        let error = ffmpeg
            .execute("-hide_banner -i definitely-missing-input.flac -y out.wav")
            .expect_err("fails");
        assert!(matches!(error, FfmpegError::Exit { .. }), "{error:?}");
        assert!(!error.message().is_empty());
    }
}
