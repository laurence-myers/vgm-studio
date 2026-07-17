//! Native audio output: a cpal stream driven by the pull-based [`PlayerEngine`].
//!
//! Replaces PyAudio. The engine lives *inside* the cpal callback -- OPL emulation
//! is far faster than real time, so there is no separate render thread and no
//! ring buffer of PCM to underrun. Control (seek, mute, rewind) reaches the
//! callback through a lock-free SPSC queue drained at the top of each callback,
//! and the playback position flows back through atomics. Nothing locks in the
//! audio path.
//!
//! Native only: `cpal` cannot target `wasm32-unknown-unknown`. The web build
//! plays through an `AudioWorkletProcessor` instead (Step 9), calling the same
//! `PlayerEngine::render`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use dro_core::Song;
use dro_core::config::AudioConfig;
use dro_synth::{BoostLimiter, Muting, Panning, PlayerEngine, Position};

/// What can go wrong opening or driving the audio device.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no output audio device is available")]
    NoDevice,
    #[error("the device's sample format {0} is not supported (expected f32 or i16)")]
    UnsupportedFormat(String),
    #[error("audio device error: {0}")]
    Cpal(#[from] cpal::Error),
}

/// A control message posted from the UI thread into the audio callback.
#[derive(Debug, Clone, Copy)]
enum Command {
    SeekMs(u32),
    SeekPos(usize),
    SetMuting(Muting),
    SetPanning(Panning),
    SetBoost(f32),
    Rewind,
}

/// Playback state the audio callback publishes for the UI thread to poll.
#[derive(Debug, Default)]
struct SharedState {
    frames_rendered: AtomicU64,
    next_instruction: AtomicUsize,
    finished: AtomicBool,
    /// Loudest post-limiter |sample| per channel since the UI last took them.
    /// The callback raises them with `fetch_max`; the UI consumes with
    /// `swap(0)`, so a transient between two UI polls is never missed.
    peak_left: AtomicU32,
    peak_right: AtomicU32,
}

/// An open output stream playing one song.
///
/// Dropping it stops playback. Because a cpal `Stream` is not `Send` on some
/// platforms, keep this on the thread that created it (the UI thread).
pub struct NativeAudio {
    // The stream must outlive the callback; dropping it ends playback.
    stream: cpal::Stream,
    commands: rtrb::Producer<Command>,
    shared: Arc<SharedState>,
    sample_rate: u32,
}

impl NativeAudio {
    /// Opens the default output device and prepares to play `song`.
    ///
    /// Renders at `config.frequency` if the device supports it, otherwise at the
    /// device's default rate (the OPL core resamples either way). Playback starts
    /// paused; call [`Self::play`].
    ///
    /// # Errors
    /// If there is no output device, its configuration cannot be read, its sample
    /// format is neither f32 nor i16, or the stream cannot be built.
    pub fn new(song: Arc<Song>, config: &AudioConfig) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let default_config = device.default_output_config()?;

        let sample_rate = supported_rate(&device, config.frequency)?
            .unwrap_or_else(|| default_config.sample_rate());
        let stream_config = StreamConfig {
            channels: 2,
            sample_rate,
            // The pull engine is buffer-size agnostic (proven by its chunk-
            // invariance test), so let cpal pick whatever the device prefers.
            buffer_size: cpal::BufferSize::Default,
        };

        let engine = PlayerEngine::new(Arc::clone(&song), sample_rate, config.chip_write_delay);
        // Boost rides the existing `&AudioConfig`, and the limiter's release is
        // derived from the *actual* negotiated rate, not the configured one.
        let limiter = BoostLimiter::new(sample_rate, config.boost);
        let (commands, consumer) = rtrb::RingBuffer::<Command>::new(64);
        let shared = Arc::new(SharedState::default());

        let stream = match default_config.sample_format() {
            SampleFormat::F32 => build_stream(
                &device,
                &stream_config,
                engine,
                consumer,
                Arc::clone(&shared),
                limiter,
                |sample| f32::from(sample) / 32768.0,
            )?,
            SampleFormat::I16 => build_stream(
                &device,
                &stream_config,
                engine,
                consumer,
                Arc::clone(&shared),
                limiter,
                |sample| sample,
            )?,
            other => return Err(AudioError::UnsupportedFormat(format!("{other:?}"))),
        };

        Ok(Self {
            stream,
            commands,
            shared,
            sample_rate,
        })
    }

    /// Starts (or resumes) playback.
    ///
    /// # Errors
    /// If the device rejects starting the stream.
    pub fn play(&self) -> Result<(), AudioError> {
        self.stream.play()?;
        Ok(())
    }

    /// Pauses playback, holding the current position.
    ///
    /// # Errors
    /// If the device rejects pausing the stream.
    pub fn pause(&self) -> Result<(), AudioError> {
        self.stream.pause()?;
        Ok(())
    }

    /// Seeks to the instruction playing at `ms`.
    pub fn seek_ms(&mut self, ms: u32) {
        self.send(Command::SeekMs(ms));
    }

    /// Seeks to instruction `pos`.
    pub fn seek_pos(&mut self, pos: usize) {
        self.send(Command::SeekPos(pos));
    }

    /// Returns to the start of the song.
    pub fn rewind(&mut self) {
        self.send(Command::Rewind);
    }

    /// Replaces the channel/percussion muting.
    pub fn set_muting(&mut self, muting: Muting) {
        self.send(Command::SetMuting(muting));
    }

    /// Replaces the per-channel panning.
    pub fn set_panning(&mut self, panning: Panning) {
        self.send(Command::SetPanning(panning));
    }

    /// Changes the live playback volume boost. The limiter keeps the boosted
    /// signal from clipping; this never touches a WAV render.
    pub fn set_boost(&mut self, boost: f32) {
        self.send(Command::SetBoost(boost));
    }

    /// The rate the stream actually renders at: `config.frequency` if the
    /// device supported it, otherwise the device's default rate.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The most recent playback position published by the audio callback.
    #[must_use]
    pub fn position(&self) -> Position {
        let frames = self.shared.frames_rendered.load(Ordering::Relaxed);
        Position {
            frames_rendered: frames,
            elapsed_ms: u32::try_from(frames * 1000 / u64::from(self.sample_rate))
                .unwrap_or(u32::MAX),
            next_instruction: self.shared.next_instruction.load(Ordering::Relaxed),
        }
    }

    /// The loudest post-limiter output peak per channel (left, right) since
    /// the last call, normalized to `0.0..=1.0` -- what the listener actually
    /// hears, boost included. A destructive read: each peak is reported once.
    #[must_use]
    pub fn take_peaks(&self) -> [f32; 2] {
        [&self.shared.peak_left, &self.shared.peak_right]
            .map(|peak| peak.swap(0, Ordering::Relaxed) as f32 / 32_768.0)
    }

    /// Whether the song has played to the end.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }

    fn send(&mut self, command: Command) {
        if self.commands.push(command).is_err() {
            log::warn!("audio command queue is full; dropping a control command");
        }
    }
}

impl fmt::Debug for NativeAudio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeAudio")
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

/// Builds the output stream for one sample type, converting the engine's i16
/// frames with `convert`.
fn build_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut engine: PlayerEngine<Arc<Song>>,
    mut commands: rtrb::Consumer<Command>,
    shared: Arc<SharedState>,
    mut limiter: BoostLimiter,
    convert: F,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(i16) -> T + Send + 'static,
{
    let mut scratch: Vec<i16> = Vec::new();
    device.build_output_stream(
        *config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            while let Ok(command) = commands.pop() {
                match command {
                    Command::SeekMs(ms) => engine.seek_to_ms(ms),
                    Command::SeekPos(pos) => engine.seek_to_pos(pos),
                    Command::SetMuting(muting) => engine.set_muting(muting),
                    Command::SetPanning(panning) => engine.set_panning(panning),
                    Command::SetBoost(boost) => limiter.set_boost(boost),
                    Command::Rewind => engine.rewind(),
                }
            }

            let frames = out.len() / 2;
            if scratch.len() < frames * 2 {
                scratch.resize(frames * 2, 0);
            }
            // `render` zeroes its own tail once the song ends, so the whole slice
            // is valid to convert.
            engine.render(&mut scratch[..frames * 2]);
            // Boost + limit the i16 frames before conversion, so both device
            // formats (f32 and i16) hear the identical limited signal.
            limiter.process(&mut scratch[..frames * 2]);
            // Publish the post-limiter peaks for the UI's meter. `fetch_max`,
            // not `store`: a transient in a buffer between UI polls survives.
            let (peak_l, peak_r) = channel_peaks(&scratch[..frames * 2]);
            shared.peak_left.fetch_max(peak_l, Ordering::Relaxed);
            shared.peak_right.fetch_max(peak_r, Ordering::Relaxed);
            for (dst, &src) in out.iter_mut().zip(&scratch[..frames * 2]) {
                *dst = convert(src);
            }

            let position = engine.position();
            shared
                .frames_rendered
                .store(position.frames_rendered, Ordering::Relaxed);
            shared
                .next_instruction
                .store(position.next_instruction, Ordering::Relaxed);
            shared
                .finished
                .store(engine.is_finished(), Ordering::Relaxed);
        },
        |error| log::error!("audio output stream error: {error}"),
        None,
    )
}

/// Whether `device` supports stereo output at `rate`, returning it if so.
fn supported_rate(device: &cpal::Device, rate: u32) -> Result<Option<u32>, cpal::Error> {
    for config in device.supported_output_configs()? {
        if config.channels() == 2
            && config.min_sample_rate() <= rate
            && rate <= config.max_sample_rate()
        {
            return Ok(Some(rate));
        }
    }
    Ok(None)
}

/// Per-channel maximum absolute sample over interleaved stereo `samples`.
/// `unsigned_abs` keeps `i16::MIN` exact (32768) instead of overflowing.
fn channel_peaks(samples: &[i16]) -> (u32, u32) {
    let mut left = 0u32;
    let mut right = 0u32;
    for frame in samples.chunks_exact(2) {
        left = left.max(u32::from(frame[0].unsigned_abs()));
        right = right.max(u32::from(frame[1].unsigned_abs()));
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_peaks_takes_the_max_abs_per_channel() {
        assert_eq!(channel_peaks(&[]), (0, 0));
        assert_eq!(channel_peaks(&[100, -50]), (100, 50));
        // Left peaks on a negative sample; right's i16::MIN does not overflow.
        assert_eq!(
            channel_peaks(&[100, -50, -3000, 2000, 5, i16::MIN]),
            (3000, 32_768)
        );
    }
}
