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
pub const PLAYER_ENV: &str = "VGMSTUDIO_REF_PLAYER";
/// Optional: extra arguments, space-separated, inserted before the input path.
///
/// Different builds of the same player disagree about their flags, so the
/// invocation is configuration rather than a guess baked into this file.
pub const ARGS_ENV: &str = "VGMSTUDIO_REF_ARGS";
/// Optional: where to keep rendered reference WAVs between runs.
pub const CACHE_ENV: &str = "VGMSTUDIO_PARITY_CACHE";
/// Optional: a settings file staged beside the player before each run.
///
/// For VGMPlay this is its `VGMPlay.ini`, which carries the loop count, the
/// fade and the per-chip core selection -- everything that decides what the
/// reference *is*. Pinning it is what makes a comparison reproducible.
/// See [`Reference::stage`] for why the player is copied rather than pointed at.
pub const CONFIG_ENV: &str = "VGMSTUDIO_REF_CONFIG";

/// An external player, and the record of which one it was.
#[derive(Debug, Clone)]
pub struct Reference {
    executable: PathBuf,
    extra_args: Vec<String>,
    cache: Option<PathBuf>,
    config: Option<PathBuf>,
    rate: Option<u32>,
}

/// Why a reference render did not happen.
#[derive(Debug)]
pub enum ReferenceError {
    /// No `VGMSTUDIO_REF_PLAYER`, or it does not point at a file. Not a failure:
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
            config: std::env::var_os(CONFIG_ENV).map(PathBuf::from),
            rate: None,
        })
    }

    /// The same reference, asked to render at `rate`.
    ///
    /// **Rendering both sides at a chip's native rate is what makes a
    /// correlation mean the core rather than the resampler.** Left to
    /// themselves the two sides convert 49716 Hz down to 44100 by different
    /// arithmetic, and the difference is neither small nor a fault: it cost the
    /// OPL control group -- whose core is proven bit-identical to the
    /// reference's -- around fifteen points of correlation.
    #[must_use]
    pub fn at_rate(&self, rate: u32) -> Self {
        Self {
            rate: Some(rate),
            ..self.clone()
        }
    }

    /// What was run, for the record a re-run years later compares against.
    #[must_use]
    pub fn describe(&self) -> String {
        let size = std::fs::metadata(&self.executable)
            .map(|meta| meta.len())
            .unwrap_or(0);
        format!(
            "{} ({size} bytes){}{}",
            self.executable.display(),
            if self.extra_args.is_empty() {
                String::new()
            } else {
                format!(" args={:?}", self.extra_args)
            },
            self.config
                .as_ref()
                .map_or(String::new(), |path| format!(" config={}", path.display())),
        ) + &self
            .rate
            .map_or(String::new(), |rate| format!(" rate={rate}"))
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
        // A stale WAV from an earlier run would be picked up as this one's, so
        // the directory starts empty.
        clear_wavs(work_dir);
        let staged = self.stage(work_dir)?;

        let mut command = std::process::Command::new(&staged);
        command.args(&self.extra_args).arg(input);
        command.current_dir(staged.parent().unwrap_or(work_dir));
        // No console to read from: the player must run headless or not at all.
        command.stdin(std::process::Stdio::null());
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
        // The player names its own output after the input, so it is found
        // rather than dictated. Anything else would make the runner specific to
        // one player's command line.
        let output = sole_wav(work_dir).ok_or_else(|| {
            ReferenceError::Failed(format!(
                "the player exited cleanly but wrote no .wav into {}. Check that \
                 the pinned config sets LogSound to 1.",
                work_dir.display()
            ))
        })?;
        let bytes = std::fs::read(&output)?;
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
    /// Everything downstream assumes the reference is a fixed point; a player
    /// that dithers would make every threshold noise, and the symptom would look
    /// like flaky cores rather than a flaky reference.
    ///
    /// # Errors
    /// [`ReferenceError::NotDeterministic`] if the two renders differ, or
    /// whatever [`render`](Self::render) failed with.
    pub fn self_check(&self, input: &Path, work_dir: &Path) -> Result<(), ReferenceError> {
        // Deliberately bypasses the cache: a cache hit would compare a file
        // with itself and prove nothing.
        let bare = Self {
            cache: None,
            ..self.clone()
        };
        let first = bare.render(input, &work_dir.join("determinism-a"))?;
        let second = bare.render(input, &work_dir.join("determinism-b"))?;
        if first != second {
            return Err(ReferenceError::NotDeterministic);
        }
        Ok(())
    }

    /// Copies the player and its pinned config into `work_dir`, and returns the
    /// staged executable.
    ///
    /// Two behaviours of VGMPlay force this, both established by experiment
    /// rather than read off a manual:
    ///
    /// 1. **It reads `VGMPlay.ini` from its own directory, not the working
    ///    directory.** Dropping a pinned config next to where the process was
    ///    started has no effect at all -- the run silently uses whatever the
    ///    installation happens to hold, which is exactly the unreproducibility
    ///    pinning exists to prevent. The only way to make the pinned settings
    ///    win without editing someone's installed copy is to stand up our own.
    /// 2. **An empty `LogPath` means "beside the input file".** Left empty, a
    ///    render writes an eight-megabyte WAV into the corpus directory it is
    ///    reading from. So `LogPath` is rewritten to point here, and the rest of
    ///    the pinned file is copied through untouched.
    fn stage(&self, work_dir: &Path) -> Result<PathBuf, ReferenceError> {
        let staged_dir = work_dir.join("player");
        std::fs::create_dir_all(&staged_dir)?;
        let source_dir = self.executable.parent().ok_or_else(|| {
            ReferenceError::Failed(format!("{} has no directory", self.executable.display()))
        })?;
        // The player's neighbours matter: VGMPlay needs its zlib beside it.
        let neighbours = std::fs::read_dir(source_dir).map_err(|error| {
            ReferenceError::Failed(format!("reading {}: {error}", source_dir.display()))
        })?;
        for entry in neighbours.flatten() {
            let from = entry.path();
            if !from.is_file() {
                continue;
            }
            let to = staged_dir.join(entry.file_name());
            // Copying is one-time; later renders reuse the staged tree.
            if !to.exists() {
                std::fs::copy(&from, &to)?;
            }
        }
        if let Some(config) = &self.config
            && let Some(name) = config.file_name()
        {
            let pinned = std::fs::read_to_string(config)?;
            std::fs::write(
                staged_dir.join(name),
                settings_for(&pinned, work_dir, self.rate),
            )?;
        }
        let name = self.executable.file_name().ok_or_else(|| {
            ReferenceError::Failed(format!("{} has no file name", self.executable.display()))
        })?;
        Ok(staged_dir.join(name))
    }

    /// Where a cached render of `input` would live. Keyed by the input's name
    /// and size, which is enough to tell corpus files apart without hashing
    /// every one of them -- **and by the rate**, because the same file rendered
    /// at two rates is two different answers and serving one for the other
    /// would silently reintroduce the resampler the rate exists to avoid.
    fn cached_path(&self, input: &Path) -> Option<PathBuf> {
        let cache = self.cache.as_ref()?;
        let name = input.file_name()?.to_string_lossy();
        let size = std::fs::metadata(input).ok()?.len();
        let rate = self
            .rate
            .map_or_else(|| "default".to_owned(), |r| r.to_string());
        Some(cache.join(format!("{name}.{size}.{rate}.wav")))
    }
}

/// Returns `config` with its `LogPath` pointed at `dir` and, if `rate` is
/// given, its `SampleRate` set to it.
///
/// Everything else passes through byte for byte -- the pinned file stays the
/// authority on loop count, fade, core selection and everything else that
/// decides what the reference *is*. Only the two settings the pinned file
/// cannot know are written: where this run's output goes, and what rate the
/// caller wants to compare at. The second exists because comparing at a chip's
/// **native rate** takes both resamplers out of the measurement, and which rate
/// that is depends on the chip and on the clock in the file's header.
fn settings_for(config: &str, dir: &Path, rate: Option<u32>) -> String {
    // A trailing separator, because the setting is documented as a directory
    // and is concatenated with the file name: without it the render lands in
    // the *parent* under a run-together name.
    let mut target = dir.display().to_string();
    if !target.ends_with(std::path::MAIN_SEPARATOR) {
        target.push(std::path::MAIN_SEPARATOR);
    }
    let mut out = String::with_capacity(config.len() + target.len());
    for line in config.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        // A comment or a section header has no `=`, and must not be mistaken
        // for a setting whose name happens to match.
        let key = match body.split_once('=') {
            Some((name, _)) => name.trim(),
            None => "",
        };
        let replacement = if key.eq_ignore_ascii_case("LogPath") {
            Some(format!("LogPath = {target}"))
        } else if key.eq_ignore_ascii_case("SampleRate") {
            rate.map(|rate| format!("SampleRate = {rate}"))
        } else {
            None
        };
        match replacement {
            Some(text) => {
                out.push_str(&text);
                out.push_str(&line[body.len()..]);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Removes any WAV already in `dir`, so the next run's output is unambiguous.
fn clear_wavs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The one WAV in `dir`, or `None` if there is not exactly one.
///
/// Exactly one, not the newest: two would mean the previous run was not
/// cleared, and quietly taking one of them is how a comparison ends up
/// measuring the wrong file.
fn sole_wav(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();
    (found.len() == 1).then(|| found.remove(0))
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
            config: None,
            rate: None,
        };
        let error = missing
            .render(
                Path::new("in.vgm"),
                &std::env::temp_dir().join("vgmstudio-parity-test"),
            )
            .expect_err("it cannot have run");
        assert!(
            matches!(error, ReferenceError::Failed(_)),
            "got {error} -- a missing binary is a failure to run, not a panic"
        );
    }

    /// An empty `LogPath` makes VGMPlay write its render *beside the input* --
    /// which, for a corpus test, means depositing megabytes of WAV into the
    /// user's music collection. Discovered the hard way. The rewrite is the
    /// only thing standing between the harness and that behaviour, so it is
    /// tested rather than trusted.
    #[test]
    fn the_log_path_is_redirected_and_nothing_else_is_touched() {
        let pinned = "; a comment\r\nLogSound = 1\r\nLogPath =\r\nSampleRate = 44100\r\n";
        let rewritten = settings_for(pinned, Path::new("X:/work/dir"), None);

        let line = rewritten
            .lines()
            .find(|line| line.starts_with("LogPath"))
            .expect("the setting survives");
        let target = line.trim_start_matches("LogPath =").trim();
        assert!(
            target.ends_with(std::path::MAIN_SEPARATOR),
            "{line} -- a directory concatenated with a file name needs its \
             separator, or the render lands in the parent"
        );
        assert!(
            target.contains("dir"),
            "{line} -- it points at the work dir"
        );

        assert!(rewritten.contains("LogSound = 1"), "other settings survive");
        assert!(rewritten.contains("; a comment"), "comments survive");
        assert_eq!(
            rewritten.lines().count(),
            pinned.lines().count(),
            "the file keeps its shape"
        );
        assert!(rewritten.contains("\r\n"), "and its line endings");
        assert!(
            rewritten.contains("SampleRate = 44100"),
            "an unasked-for rate leaves the pinned one alone"
        );

        let native = settings_for(pinned, Path::new("X:/work/dir"), Some(49_716));
        assert!(
            native.contains("SampleRate = 49716"),
            "asking for a rate rewrites it: {native}"
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
