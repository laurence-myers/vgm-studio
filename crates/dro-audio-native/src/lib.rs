// SPDX-License-Identifier: GPL-2.0-or-later
//! Native audio output: a cpal stream driven by the pull-based [`PlayerEngine`].
//!
//! The engine lives *inside* the cpal callback -- OPL emulation is far faster
//! than real time, so there is no separate render thread and no ring buffer of
//! PCM to underrun. Control (seek, mute, rewind) reaches the callback through a
//! lock-free SPSC queue drained at the top of each callback, and the playback
//! position flows back through atomics. Nothing locks in the audio path.
//!
//! Native only: `cpal` cannot target `wasm32-unknown-unknown`. The web build
//! plays through an `AudioWorkletProcessor` instead, calling the same
//! `PlayerEngine::render`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use dro_core::Song;
use dro_core::config::AudioConfig;
use dro_synth::vgm_engine::VgmEngine;
use dro_synth::{
    AudioSource, BoostLimiter, ChipMuting, ChipPanning, LoopConfig, Muting, OplChip, Panning,
    PlayerEngine, Position,
};

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
///
/// Not `Copy`: the chip mute/pan variants own a small `Vec` (one per chip
/// instance). Toggling a channel is rare and user-driven, so that move is off
/// the per-buffer hot path; the OPL `Muting`/`Panning` variants stay
/// allocation-free.
#[derive(Debug, Clone)]
enum Command {
    SeekMs(u32),
    SeekPos(usize),
    SetMuting(Muting),
    SetPanning(Panning),
    /// Any-chip channel mutes, for the generic engine (a no-op on the OPL arm,
    /// which has its own [`Muting`]).
    SetChipMuting(ChipMuting),
    /// Any-chip channel pans, likewise.
    SetChipPanning(ChipPanning),
    SetBoost(f32),
    SetLoop(Option<LoopConfig>),
    Rewind,
}

/// Playback state the audio callback publishes for the UI thread to poll.
#[derive(Debug, Default)]
struct SharedState {
    frames_rendered: AtomicU64,
    next_instruction: AtomicUsize,
    finished: AtomicBool,
    /// Loop repeats taken since the last seek, for the "loop 2/5" readout.
    loop_iteration: AtomicU32,
    /// Loudest post-limiter |sample| per channel since the UI last took them.
    /// The callback raises them with `fetch_max`; the UI consumes with
    /// `swap(0)`, so a transient between two UI polls is never missed.
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    /// The lowest boost at which the limiter has engaged (clamped an overshoot)
    /// since this stream opened, held as `f32` bits; `0.0` bits means "never
    /// engaged". The callback lowers it (it is the sole writer) as quieter boosts
    /// still clip, and a new song opens a fresh stream that resets it. The UI
    /// reads it as the volume ceiling, which therefore ratchets down to the
    /// lowest level that clips.
    min_engaged_boost: AtomicU32,
    /// Whether the limiter has engaged since the UI last looked. Set by the
    /// callback, cleared by the read, so a clip in any buffer between two UI
    /// polls is reported exactly once -- unlike `min_engaged_boost`, which is
    /// sticky and says nothing about *when* it last clipped.
    limited: AtomicBool,
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
    /// device's default rate (both engines resample either way). Playback starts
    /// paused; call [`Self::play`].
    ///
    /// # Errors
    /// If there is no output device, its configuration cannot be read, its sample
    /// format is neither f32 nor i16, or the stream cannot be built.
    pub fn new(source: &AudioSource, config: &AudioConfig) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let default_config = device.default_output_config()?;

        let sample_rate = supported_rate(&device, config.frequency)?
            .unwrap_or_else(|| default_config.sample_rate());
        // Wire the configured buffer size, clamped to the device's supported
        // range; fall back to the host default if the device advertises no range
        // (or later rejects the fixed size -- WASAPI can). The pull engine is
        // buffer-size agnostic (its chunk-invariance test proves it), so this
        // changes only the callback size / latency, never the rendered audio.
        let sample_format = default_config.sample_format();
        let buffer_size = resolve_buffer_size(&device, sample_rate, config.buffer_size);
        let (stream, commands, shared) = match Self::build(
            &device,
            sample_format,
            sample_rate,
            source,
            config,
            buffer_size,
        ) {
            Ok(parts) => parts,
            Err(error) if matches!(buffer_size, cpal::BufferSize::Fixed(_)) => {
                log::warn!("device rejected a fixed buffer size ({error}); using the host default");
                Self::build(
                    &device,
                    sample_format,
                    sample_rate,
                    source,
                    config,
                    cpal::BufferSize::Default,
                )?
            }
            Err(error) => return Err(error),
        };

        Ok(Self {
            stream,
            commands,
            shared,
            sample_rate,
        })
    }

    /// Builds the output stream, its command producer, and shared state for a
    /// given negotiated rate and buffer size.
    fn build(
        device: &cpal::Device,
        sample_format: SampleFormat,
        sample_rate: u32,
        source: &AudioSource,
        config: &AudioConfig,
        buffer_size: cpal::BufferSize,
    ) -> Result<(cpal::Stream, rtrb::Producer<Command>, Arc<SharedState>), AudioError> {
        let engine = match source {
            AudioSource::Opl(song) => {
                // The chosen core, or the registry's default if this build
                // lacks it (a config naming `retrowave` reaches here when the
                // board could not be opened; falling back beats refusing to
                // play). The registry-side choice is asked first, since it runs
                // ahead of the config while the Settings picker auditions a core.
                let registry_choice =
                    dro_synth::registry::core_choice(dro_core::vgm::ChipKind::Ymf262);
                let chip = dro_synth::registry::registry()
                    .build_opl(
                        registry_choice
                            .as_deref()
                            .or_else(|| config.core(dro_core::config::OPL_SLOT)),
                        sample_rate,
                    )
                    .unwrap_or_else(|| Box::new(dro_synth::DefaultOplChip::new(sample_rate)));
                Engine::Opl(Box::new(PlayerEngine::with_chip(
                    Arc::clone(song),
                    chip,
                    sample_rate,
                )))
            }
            AudioSource::Vgm(file) => {
                // Realtime cores only: a chosen offline-tier core (the LLE
                // die sims) would underrun the callback, so the transport
                // substitutes the chip's realtime default. The WAV render
                // keeps the choice as made -- it has all the time in the
                // world.
                let mut engine = VgmEngine::with_cores(Arc::clone(file), sample_rate, |kind| {
                    dro_synth::core_for_realtime(kind)
                });
                // The config's slug, with an unknown spelling falling back to
                // the accurate default -- same policy as an unknown core name.
                engine.set_resample_mode(
                    dro_synth::resample::ResampleMode::from_slug(&config.resampling)
                        .unwrap_or_default(),
                );
                Engine::Vgm(Box::new(engine))
            }
        };
        // Boost rides the existing `&AudioConfig`, and the limiter's release is
        // derived from the *actual* negotiated rate, not the configured one.
        let limiter = BoostLimiter::new(sample_rate, config.boost);
        let (commands, consumer) = rtrb::RingBuffer::<Command>::new(64);
        let shared = Arc::new(SharedState::default());
        // Pre-size the callback scratch so the real-time path never allocates: a
        // fixed buffer's frame count, else a generous cap for the host default.
        // The in-callback resize stays as a never-hit fallback.
        let scratch_frames = match buffer_size {
            cpal::BufferSize::Fixed(frames) => frames as usize,
            cpal::BufferSize::Default => DEFAULT_SCRATCH_FRAMES,
        };
        let stream_config = StreamConfig {
            channels: 2,
            sample_rate,
            buffer_size,
        };
        let stream = match sample_format {
            SampleFormat::F32 => build_stream(
                device,
                &stream_config,
                engine,
                consumer,
                Arc::clone(&shared),
                limiter,
                scratch_frames,
                |sample| f32::from(sample) / 32768.0,
            )?,
            SampleFormat::I16 => build_stream(
                device,
                &stream_config,
                engine,
                consumer,
                Arc::clone(&shared),
                limiter,
                scratch_frames,
                |sample| sample,
            )?,
            other => return Err(AudioError::UnsupportedFormat(format!("{other:?}"))),
        };
        Ok((stream, commands, shared))
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

    /// Replaces the any-chip channel mutes (the generic engine's).
    pub fn set_chip_muting(&mut self, muting: ChipMuting) {
        self.send(Command::SetChipMuting(muting));
    }

    /// Replaces the any-chip channel pans (the generic engine's).
    pub fn set_chip_panning(&mut self, panning: ChipPanning) {
        self.send(Command::SetChipPanning(panning));
    }

    /// Changes the live playback volume boost. The limiter keeps the boosted
    /// signal from clipping; this never touches a WAV render.
    pub fn set_boost(&mut self, boost: f32) {
        self.send(Command::SetBoost(boost));
    }

    /// Sets (or clears) the region playback loops over.
    ///
    /// Takes effect at the next loop boundary, so changing the repeat count
    /// mid-playback does not interrupt the pass in progress. Build the config
    /// with `LoopConfig::for_song` -- it precomputes the frame position the
    /// callback cannot afford to derive.
    pub fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.send(Command::SetLoop(config));
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
        Position::looping(
            frames,
            self.sample_rate,
            self.shared.next_instruction.load(Ordering::Relaxed),
            self.shared.loop_iteration.load(Ordering::Relaxed),
        )
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

    /// Whether the limiter engaged since the last call: a clip happened, and the
    /// meter should hold its marker to show it. A destructive read.
    #[must_use]
    pub fn take_limited(&self) -> bool {
        self.shared.limited.swap(false, Ordering::Relaxed)
    }

    /// The lowest boost at which the limiter has engaged since this stream opened,
    /// or `None` if it has not clipped. Reset per song (a new song is a new
    /// stream); the UI uses it as the volume ceiling.
    #[must_use]
    pub fn min_engaged_boost(&self) -> Option<f32> {
        let boost = f32::from_bits(self.shared.min_engaged_boost.load(Ordering::Relaxed));
        (boost > 0.0).then_some(boost)
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

/// Whichever engine the callback is driving.
///
/// The callback needs six things from it -- render, seek, rewind, loop, where it
/// is, and whether it has finished -- and everything else it can be told is OPL
/// register policy, which only one of them has. Those are no-ops on the other
/// rather than an error: a mute command arriving for a Mega Drive rip means the
/// UI has not caught up, not that anything is wrong.
enum Engine {
    /// Boxed `dyn OplChip` rather than a concrete core: which OPL emulator plays
    /// is the user's choice, made in Settings and resolved from the registry at
    /// `load()`. A core swap applies to the next load, never to a running stream.
    Opl(Box<PlayerEngine<Arc<Song>, Box<dyn OplChip>>>),
    Vgm(Box<VgmEngine>),
}

impl Engine {
    fn render(&mut self, out: &mut [i16]) {
        match self {
            Self::Opl(engine) => {
                engine.render(out);
            }
            Self::Vgm(engine) => {
                engine.render(out);
            }
        }
    }

    fn seek_to_ms(&mut self, ms: u32) {
        match self {
            Self::Opl(engine) => engine.seek_to_ms(ms),
            Self::Vgm(engine) => engine.seek_to_ms(ms),
        }
    }

    fn seek_to_pos(&mut self, pos: usize) {
        match self {
            Self::Opl(engine) => engine.seek_to_pos(pos),
            Self::Vgm(engine) => engine.seek_to_row(pos),
        }
    }

    fn rewind(&mut self) {
        match self {
            Self::Opl(engine) => engine.rewind(),
            Self::Vgm(engine) => engine.rewind(),
        }
    }

    fn set_muting(&mut self, muting: Muting) {
        if let Self::Opl(engine) = self {
            engine.set_muting(muting);
        }
    }

    fn set_panning(&mut self, panning: Panning) {
        if let Self::Opl(engine) = self {
            engine.set_panning(panning);
        }
    }

    fn set_chip_muting(&mut self, muting: ChipMuting) {
        if let Self::Vgm(engine) = self {
            engine.set_muting(muting);
        }
    }

    fn set_chip_panning(&mut self, panning: ChipPanning) {
        if let Self::Vgm(engine) = self {
            engine.set_panning(panning);
        }
    }

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        match self {
            Self::Opl(engine) => engine.set_loop(config),
            Self::Vgm(engine) => engine.set_loop(config),
        }
    }

    fn position(&self) -> Position {
        match self {
            Self::Opl(engine) => engine.position(),
            Self::Vgm(engine) => engine.position(),
        }
    }

    fn is_finished(&self) -> bool {
        match self {
            Self::Opl(engine) => engine.is_finished(),
            Self::Vgm(engine) => engine.is_finished(),
        }
    }
}

/// Builds the output stream for one sample type, converting the engine's i16
/// frames with `convert`.
// A private stream-builder wiring together the callback's owned pieces; bundling
// them into a struct would only add indirection.
#[allow(clippy::too_many_arguments)]
fn build_stream<T, F>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut engine: Engine,
    mut commands: rtrb::Consumer<Command>,
    shared: Arc<SharedState>,
    mut limiter: BoostLimiter,
    scratch_frames: usize,
    convert: F,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(i16) -> T + Send + 'static,
{
    // Pre-sized so the real-time callback never allocates on its first run.
    let mut scratch: Vec<i16> = vec![0; scratch_frames * 2];
    device.build_output_stream(
        *config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            while let Ok(command) = commands.pop() {
                match command {
                    Command::SeekMs(ms) => engine.seek_to_ms(ms),
                    Command::SeekPos(pos) => engine.seek_to_pos(pos),
                    Command::SetMuting(muting) => engine.set_muting(muting),
                    Command::SetPanning(panning) => engine.set_panning(panning),
                    Command::SetChipMuting(muting) => engine.set_chip_muting(muting),
                    Command::SetChipPanning(panning) => engine.set_chip_panning(panning),
                    Command::SetBoost(boost) => limiter.set_boost(boost),
                    Command::SetLoop(config) => engine.set_loop(config),
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
            // formats (f32 and i16) hear the identical limited signal. When it
            // clamps, record the lowest boost that has clipped so the UI can cap
            // the volume there, ratcheting the cap down as quieter boosts still
            // clip.
            if limiter.process(&mut scratch[..frames * 2]) {
                shared.limited.store(true, Ordering::Relaxed);
                let boost = limiter.boost();
                let prev = f32::from_bits(shared.min_engaged_boost.load(Ordering::Relaxed));
                if prev == 0.0 || boost < prev {
                    shared
                        .min_engaged_boost
                        .store(boost.to_bits(), Ordering::Relaxed);
                }
            }
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
                .loop_iteration
                .store(position.loop_iteration, Ordering::Relaxed);
            shared
                .finished
                .store(engine.is_finished(), Ordering::Relaxed);
        },
        |error| log::error!("audio output stream error: {error}"),
        None,
    )
}

/// A generous scratch pre-size for the host-default buffer path, where the exact
/// callback size isn't known up front. A larger callback still works (the
/// in-callback resize handles it), it just isn't allocation-free that once.
const DEFAULT_SCRATCH_FRAMES: usize = 4096;

/// The buffer size to request: the configured `frames` clamped into the device's
/// supported range for a stereo stream at `sample_rate`, or the host default when
/// the device advertises no fixed-size range.
fn resolve_buffer_size(device: &cpal::Device, sample_rate: u32, frames: u32) -> cpal::BufferSize {
    let supported = device
        .supported_output_configs()
        .into_iter()
        .flatten()
        .find(|config| {
            config.channels() == 2
                && config.min_sample_rate() <= sample_rate
                && sample_rate <= config.max_sample_rate()
        })
        .map(|config| *config.buffer_size());
    match supported {
        Some(range) => clamp_buffer_size(range, frames),
        None => cpal::BufferSize::Default,
    }
}

/// Clamps `frames` into a device's supported buffer-size range, or falls back to
/// the host default when the range is unknown.
fn clamp_buffer_size(supported: cpal::SupportedBufferSize, frames: u32) -> cpal::BufferSize {
    match supported {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed(frames.clamp(min, max))
        }
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
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
    use std::sync::Mutex;

    use dro_core::vgm::ChipKind;
    use dro_synth::ChipCore;

    use super::*;

    /// A minimal one-chip VGM: an SN76489 clocked, one write, one wait, end.
    fn sn_vgm() -> Arc<dro_core::VgmFile> {
        fn put(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put(&mut bytes, 0x08, 0x151);
        put(&mut bytes, 0x34, 0x100 - 0x34);
        put(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        bytes.extend_from_slice(&[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x66]);
        let eof = bytes.len();
        put(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(dro_core::vgm::file::read("t.vgm", &bytes).expect("a walkable VGM"))
    }

    /// The `Engine::Vgm` arm forwards chip muting to its voices; the wrapper's
    /// job is only to pick the arm, but that pick is what the callback relies
    /// on.
    #[test]
    fn the_vgm_arm_forwards_chip_muting() {
        let mutes: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

        struct Tap(Arc<Mutex<Vec<u32>>>);
        impl ChipCore for Tap {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
            fn set_channel_mutes(&mut self, muted: u32) {
                self.0.lock().expect("not poisoned").push(muted);
            }
        }

        let mutes_for_factory = Arc::clone(&mutes);
        let engine = VgmEngine::with_cores(sn_vgm(), 44_100, move |_| {
            Some(Box::new(Tap(Arc::clone(&mutes_for_factory))))
        });
        let mut engine = Engine::Vgm(Box::new(engine));

        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Sn76489, 0, 0b0010);
        engine.set_chip_muting(muting);
        assert!(
            mutes.lock().expect("not poisoned").contains(&0b0010),
            "the mask reached the voice"
        );

        // And the OPL-only command is a no-op on this arm rather than a panic.
        engine.set_muting(Muting::all());
    }

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

    #[test]
    fn clamp_buffer_size_honours_the_device_range() {
        let range = cpal::SupportedBufferSize::Range {
            min: 128,
            max: 1024,
        };
        assert!(matches!(
            clamp_buffer_size(range, 512),
            cpal::BufferSize::Fixed(512)
        ));
        assert!(
            matches!(clamp_buffer_size(range, 16), cpal::BufferSize::Fixed(128)),
            "clamped up to the minimum"
        );
        assert!(
            matches!(
                clamp_buffer_size(range, 8192),
                cpal::BufferSize::Fixed(1024)
            ),
            "clamped down to the maximum"
        );
        assert!(matches!(
            clamp_buffer_size(cpal::SupportedBufferSize::Unknown, 512),
            cpal::BufferSize::Default
        ));
    }
}
