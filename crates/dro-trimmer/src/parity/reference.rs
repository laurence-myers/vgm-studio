//! Driving an external reference player, and pinning what it was.
//!
//! **The reference is always a separate process.** The same stance the Mesen2
//! and BlastEm oracles take, and for the same reason: its licence never touches
//! anything this workspace distributes, so even a GPL-3 reference would be
//! fine. Nothing here links it; this module spawns it, waits, and reads a WAV
//! off disk.
//!
//! # What a reference has to be able to do
//!
//! Batch render (a file in, a WAV out, no interaction), deterministically, with
//! the loop count and fade fixed, and with its per-chip core selection pinnable
//! in a config file that can be checked in. VGMPlay is the one the acceptance
//! bar has always named; libvgm's player and MAME's `vgmplay` machine are the
//! alternatives, the latter valuable *because* it shares no code with us.
//!
//! # Determinism is checked, not assumed
//!
//! [`Reference::self_check`] renders the same file twice and requires the bytes
//! to match. A reference that dithers, or seeds anything from the clock, would
//! otherwise make every threshold in the harness noise -- and it would look
//! like our cores were flaky.

use std::path::{Path, PathBuf};

/// Where the reference executable is.
pub const PLAYER_ENV: &str = "DROTRIM_REF_PLAYER";
/// Optional: extra arguments, space-separated, inserted before the input path.
///
/// Different builds of the same player disagree about their flags, so the
/// invocation is configuration rather than a guess baked into this file.
pub const ARGS_ENV: &str = "DROTRIM_REF_ARGS";
/// Optional: where to keep rendered reference WAVs between runs.
pub const CACHE_ENV: &str = "DROTRIM_PARITY_CACHE";

/// An external player, and the record of which one it was.
#[derive(Debug, Clone)]
pub struct Reference {
    executable: PathBuf,
    extra_args: Vec<String>,
    cache: Option<PathBuf>,
}

/// Why a reference render did not happen.
#[derive(Debug)]
pub enum ReferenceError {
    /// No `DROTRIM_REF_PLAYER`, or it does not point at a file. Not a failure:
    /// the harness skips, exactly as the corpus tests skip without a corpus.
    NotConfigured(String),
    /// The player ran and failed, or produced no output.
    Failed(String),
    /// The player ran twice on one file and disagreed with itself.
    NotDeterministic,
    Io(std::io::Error),
}

impl std::fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(why) => write!(f, "no reference player: {why}"),
            Self::Failed(why) => write!(f, "the reference player failed: {why}"),
            Self::NotDeterministic => write!(
                f,
                "the reference player rendered one file differently twice; every \
                 threshold here would be noise"
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl From<std::io::Error> for ReferenceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl Reference {
    /// The reference the environment names, or why there is none.
    ///
    /// # Errors
    /// [`ReferenceError::NotConfigured`] when the variable is unset or does not
    /// point at a file.
    pub fn from_env() -> Result<Self, ReferenceError> {
        let Some(executable) = std::env::var_os(PLAYER_ENV) else {
            return Err(ReferenceError::NotConfigured(format!(
                "{PLAYER_ENV} is unset"
            )));
        };
        let executable = PathBuf::from(executable);
        if !executable.is_file() {
            return Err(ReferenceError::NotConfigured(format!(
                "{} is not a file",
                executable.display()
            )));
        }
        Ok(Self {
            executable,
            extra_args: std::env::var(ARGS_ENV)
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            cache: std::env::var_os(CACHE_ENV).map(PathBuf::from),
        })
    }

    /// What was run, for the record a re-run years later compares against.
    #[must_use]
    pub fn describe(&self) -> String {
        let size = std::fs::metadata(&self.executable)
            .map(|meta| meta.len())
            .unwrap_or(0);
        format!(
            "{} ({size} bytes){}",
            self.executable.display(),
            if self.extra_args.is_empty() {
                String::new()
            } else {
                format!(" args={:?}", self.extra_args)
            }
        )
    }

    /// Renders `input` to a WAV and returns its bytes.
    ///
    /// # Errors
    /// If the player cannot be run, exits non-zero, or writes no output.
    pub fn render(&self, input: &Path, work_dir: &Path) -> Result<Vec<u8>, ReferenceError> {
        if let Some(cached) = self.cached_path(input)
            && let Ok(bytes) = std::fs::read(&cached)
        {
            return Ok(bytes);
        }

        std::fs::create_dir_all(work_dir)?;
        let output = work_dir.join("reference.wav");
        let _ = std::fs::remove_file(&output);

        let mut command = std::process::Command::new(&self.executable);
        command.args(&self.extra_args).arg(input).arg(&output);
        command.current_dir(work_dir);
        let status = command
            .output()
            .map_err(|error| ReferenceError::Failed(format!("spawning: {error}")))?;
        if !status.status.success() {
            return Err(ReferenceError::Failed(format!(
                "exit {:?}: {}",
                status.status.code(),
                String::from_utf8_lossy(&status.stderr).trim()
            )));
        }
        let bytes = std::fs::read(&output).map_err(|error| {
            ReferenceError::Failed(format!(
                "the player exited cleanly but wrote no {}: {error}",
                output.display()
            ))
        })?;
        if let Some(cached) = self.cached_path(input) {
            if let Some(parent) = cached.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&cached, &bytes);
        }
        Ok(bytes)
    }

    /// Renders one file twice and requires the results to match.
    ///
    /// **pt-1's acceptance.** Everything downstream assumes the reference is a
    /// fixed point; a player that dithers would make every threshold noise, and
    /// the symptom would look like flaky cores rather than a flaky reference.
    ///
    /// # Errors
    /// [`ReferenceError::NotDeterministic`] if the two renders differ, or
    /// whatever [`render`](Self::render) failed with.
    pub fn self_check(&self, input: &Path, work_dir: &Path) -> Result<(), ReferenceError> {
        // Deliberately bypasses the cache: a cache hit would compare a file
        // with itself and prove nothing.
        let bare = Self {
            executable: self.executable.clone(),
            extra_args: self.extra_args.clone(),
            cache: None,
        };
        let first = bare.render(input, &work_dir.join("determinism-a"))?;
        let second = bare.render(input, &work_dir.join("determinism-b"))?;
        if first != second {
            return Err(ReferenceError::NotDeterministic);
        }
        Ok(())
    }

    /// Where a cached render of `input` would live. Keyed by the input's name
    /// and size, which is enough to tell corpus files apart without hashing
    /// every one of them.
    fn cached_path(&self, input: &Path) -> Option<PathBuf> {
        let cache = self.cache.as_ref()?;
        let name = input.file_name()?.to_string_lossy();
        let size = std::fs::metadata(input).ok()?.len();
        Some(cache.join(format!("{name}.{size}.wav")))
    }
}

/// Reads a 16-bit stereo WAV into interleaved samples.
///
/// Deliberately strict about the shape: a reference configured for mono or
/// 24-bit would otherwise be compared through a silent conversion, and the
/// mismatch would be blamed on a core.
///
/// # Errors
/// If the bytes are not a readable WAV, or not 16-bit stereo.
pub fn read_wav(bytes: &[u8]) -> Result<(Vec<i16>, u32), String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut reader = hound::WavReader::new(cursor).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.channels != 2 {
        return Err(format!("{} channels, expected stereo", spec.channels));
    }
    if spec.bits_per_sample != 16 || spec.sample_format != hound::SampleFormat::Int {
        return Err(format!(
            "{}-bit {:?}, expected 16-bit integer",
            spec.bits_per_sample, spec.sample_format
        ));
    }
    let samples: Result<Vec<i16>, _> = reader.samples::<i16>().collect();
    samples
        .map(|samples| (samples, spec.sample_rate))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absent reference is a *skip*, not a failure -- the same contract the
    /// corpus tests have. A machine without the player still runs the suite.
    #[test]
    fn an_unconfigured_reference_says_so_rather_than_failing() {
        // Whatever the environment holds, a path that cannot exist is refused
        // with an explanation rather than a panic.
        let missing = Reference {
            executable: PathBuf::from("no-such-player-anywhere"),
            extra_args: Vec::new(),
            cache: None,
        };
        let error = missing
            .render(
                Path::new("in.vgm"),
                &std::env::temp_dir().join("drotrim-parity-test"),
            )
            .expect_err("it cannot have run");
        assert!(
            matches!(error, ReferenceError::Failed(_)),
            "got {error} -- a missing binary is a failure to run, not a panic"
        );
    }

    /// The WAV reader refuses a shape it cannot compare, rather than converting
    /// silently and letting a core take the blame.
    #[test]
    fn the_wav_reader_refuses_a_shape_it_cannot_compare() {
        fn wav(channels: u16, bits: u16) -> Vec<u8> {
            let spec = hound::WavSpec {
                channels,
                sample_rate: 44_100,
                bits_per_sample: bits,
                sample_format: hound::SampleFormat::Int,
            };
            let mut bytes = std::io::Cursor::new(Vec::new());
            {
                let mut writer = hound::WavWriter::new(&mut bytes, spec).expect("a writer");
                for _ in 0..16 {
                    if bits == 16 {
                        writer.write_sample(0i16).expect("a sample");
                    } else {
                        writer.write_sample(0i32).expect("a sample");
                    }
                }
                writer.finalize().expect("finalising");
            }
            bytes.into_inner()
        }

        let (samples, rate) = read_wav(&wav(2, 16)).expect("stereo 16-bit is readable");
        assert_eq!(rate, 44_100);
        assert_eq!(samples.len(), 16);

        assert!(read_wav(&wav(1, 16)).is_err(), "mono must be refused");
        assert!(read_wav(&wav(2, 32)).is_err(), "32-bit must be refused");
        assert!(read_wav(b"not a wav").is_err());
    }
}
