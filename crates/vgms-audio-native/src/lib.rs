// SPDX-License-Identifier: GPL-2.0-or-later
//! Native audio output: a cpal stream driven by the pull-based [`VgmEngine`].
//!
//! The engine lives *inside* the cpal callback -- chip emulation is far faster
//! than real time, so there is no separate render thread and no ring buffer of
//! PCM to underrun. Control (seek, mute, rewind) reaches the callback through a
//! lock-free SPSC queue drained at the top of each callback, and the playback
//! position flows back through atomics. Nothing locks in the audio path.
//!
//! Every document plays through [`VgmEngine`]: a multichip VGM directly, and an
//! OPL document over a VGM projection of its register stream (ou-2), so OPL
//! muting, panning and splitting ride the same per-chip path every other chip
//! does. The OPL panel still speaks its own [`Muting`]/[`Panning`] vocabulary;
//! [`Engine`] translates it to the generic [`ChipMuting`]/[`ChipPanning`] for an
//! OPL document (see [`vgms_synth::opl_chip_muting`]).
//!
//! Native only: `cpal` cannot target `wasm32-unknown-unknown`. The web build
//! plays through an `AudioWorkletProcessor` instead, calling the same
//! `VgmEngine::render`.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use vgms_core::OplType;
use vgms_core::config::AudioConfig;
use vgms_synth::vgm_engine::VgmEngine;
use vgms_synth::{
    AudioSource, BoostLimiter, ChipMuting, ChipPanning, ChipTrims, LoopConfig, Muting, Panning,
    Position, opl_chip_muting, opl_chip_panning,
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
    #[error("could not project the OPL document for playback: {0}")]
    Projection(String),
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
    /// Per-chip listening trims, for the generic engine (a no-op on the OPL
    /// arm, which has no trim vocabulary).
    SetChipTrims(ChipTrims),
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
    /// Set when the stream has stopped for good (a device fault), so the
    /// transport can leave "playing" instead of showing a frozen cursor.
    /// Written only by cpal's error callback, never the data callback.
    stopped: AtomicBool,
    /// The first stream error, waiting to be shown once. Behind a `Mutex`, but
    /// touched only by the error callback -- never the real-time data callback --
    /// so the "nothing locks in the audio path" promise holds.
    error: Mutex<Option<String>>,
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
        // Realtime cores only: a chosen offline-tier core (the LLE die sims)
        // would underrun the callback, so the transport substitutes the chip's
        // realtime default. The OPL core choice rides the same registry choice --
        // the app seeds it from the config at startup -- so the Settings OPL
        // picker still applies. The WAV render keeps a chosen offline core as
        // made; it has all the time in the world.
        let build_vgm = |file: Arc<vgms_core::VgmFile>| {
            let mut engine =
                VgmEngine::with_cores(file, sample_rate, vgms_synth::core_for_realtime);
            // The config's slug, with an unknown spelling falling back to the
            // accurate default -- same policy as an unknown core name.
            engine.set_resample_mode(
                vgms_synth::resample::ResampleMode::from_slug(&config.resampling)
                    .unwrap_or_default(),
            );
            engine
        };
        let engine = match source {
            AudioSource::Opl(song) => {
                // ou-2: an OPL document plays through the generic engine, over a
                // VGM projection of its register stream, so its muting, panning
                // and split ride the same per-chip path every other chip does.
                // The projection (a serialise + re-read, as `convert_to_vgm`
                // makes) is built here, off the audio thread. `opl` carries the
                // document's OPL type so the panel's `Muting`/`Panning` translate.
                let file = vgms_core::convert::opl_song_to_vgm_file(song)
                    .map_err(|error| AudioError::Projection(error.to_string()))?;
                Engine {
                    inner: Box::new(build_vgm(Arc::new(file))),
                    opl: Some(song.opl_type),
                }
            }
            AudioSource::Vgm(file) => Engine {
                inner: Box::new(build_vgm(Arc::clone(file))),
                opl: None,
            },
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

    /// Replaces the per-chip listening trims (the generic engine's).
    pub fn set_chip_trims(&mut self, trims: ChipTrims) {
        self.send(Command::SetChipTrims(trims));
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

    /// Whether the song has played to the end, or the stream stopped on a fault.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed) || self.shared.stopped.load(Ordering::Relaxed)
    }

    /// Takes the stream's first error, if it hit one (a device unplugged
    /// mid-song, say). Reported once.
    pub fn take_error(&mut self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
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

/// The [`VgmEngine`] the callback is driving, with the vocabulary bridge an OPL
/// document needs.
///
/// Every document plays through `VgmEngine` now (ou-2): a multichip VGM directly,
/// an OPL document over its VGM projection. `opl` is `Some(type)` for the latter,
/// which is the only thing that still differs -- the OPL panel speaks
/// [`Muting`]/[`Panning`], so those are translated to the generic mutes/pans
/// (keyed on the OPL type's projected chips) before reaching the engine, and the
/// any-chip [`set_chip_muting`](Self::set_chip_muting) command is ignored. For a
/// plain VGM it is the reverse: the OPL commands are the no-ops. A command for
/// the wrong vocabulary means the UI has not caught up, not that anything is
/// wrong.
struct Engine {
    inner: Box<VgmEngine>,
    opl: Option<OplType>,
}

impl Engine {
    fn render(&mut self, out: &mut [i16]) {
        self.inner.render(out);
    }

    fn seek_to_ms(&mut self, ms: u32) {
        self.inner.seek_to_ms(ms);
    }

    fn seek_to_pos(&mut self, pos: usize) {
        // A row index addresses a VGM command. It is 1:1 for a real VGM (and an
        // OPL VGM's projection); for a DRO the projected command indices differ,
        // so the UI seeks OPL documents by time instead (ou-2) and this stays
        // exact for the VGM callers that still use it.
        self.inner.seek_to_row(pos);
    }

    fn rewind(&mut self) {
        self.inner.rewind();
    }

    fn set_muting(&mut self, muting: Muting) {
        if let Some(opl_type) = self.opl {
            self.inner.set_muting(opl_chip_muting(&muting, opl_type));
        }
    }

    fn set_panning(&mut self, panning: Panning) {
        if let Some(opl_type) = self.opl {
            self.inner.set_panning(opl_chip_panning(&panning, opl_type));
        }
    }

    fn set_chip_muting(&mut self, muting: ChipMuting) {
        if self.opl.is_none() {
            self.inner.set_muting(muting);
        }
    }

    fn set_chip_panning(&mut self, panning: ChipPanning) {
        if self.opl.is_none() {
            self.inner.set_panning(panning);
        }
    }

    fn set_chip_trims(&mut self, trims: ChipTrims) {
        // Unlike the mutes/pans, forwarded on both arms: a trim is keyed by chip
        // kind and applied as the engine's own gain, and an OPL document's
        // projection carries the projected chip's kind, so the OPL device's trim
        // (keyed to that kind) reaches its voice here. There is no OPL-vocabulary
        // counterpart to gate against.
        self.inner.set_trims(trims);
    }

    fn set_loop(&mut self, config: Option<LoopConfig>) {
        self.inner.set_loop(config);
    }

    fn position(&self) -> Position {
        self.inner.position()
    }

    fn is_finished(&self) -> bool {
        self.inner.is_finished()
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
    // A second handle for the error callback (the data callback moves `shared`).
    let error_shared = Arc::clone(&shared);
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
                    Command::SetChipTrims(trims) => engine.set_chip_trims(trims),
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
        move |error| record_stream_error(&error_shared, error.to_string()),
        None,
    )
}

/// Records a stream error into the shared state: the first error wins, and the
/// `stopped` flag lets the transport leave "playing". Extracted so it is testable
/// without opening a device. Runs on cpal's error callback -- a separate callback
/// from the real-time data one -- so taking the lock here does not break the
/// "nothing locks in the audio path" promise.
fn record_stream_error(shared: &SharedState, message: String) {
    log::error!("audio output stream error: {message}");
    if let Ok(mut slot) = shared.error.lock() {
        slot.get_or_insert(message);
    }
    shared.stopped.store(true, Ordering::Relaxed);
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

    use vgms_core::vgm::ChipKind;
    use vgms_synth::ChipCore;

    use super::*;

    /// A minimal one-chip VGM: an SN76489 clocked, one write, one wait, end.
    fn sn_vgm() -> Arc<vgms_core::VgmFile> {
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
        Arc::new(vgms_core::vgm::file::read("t.vgm", &bytes).expect("a walkable VGM"))
    }

    #[test]
    fn a_stream_error_is_recorded_once_and_marks_the_stream_stopped() {
        let shared = SharedState::default();
        record_stream_error(&shared, "device unplugged".to_owned());
        record_stream_error(&shared, "a later error".to_owned());

        assert!(
            shared.stopped.load(Ordering::Relaxed),
            "the transport can leave the playing state"
        );
        // First error wins, reported exactly once.
        assert_eq!(
            shared.error.lock().unwrap().take().as_deref(),
            Some("device unplugged")
        );
        assert!(
            shared.error.lock().unwrap().is_none(),
            "the error is taken only once"
        );
    }

    /// A plain-VGM `Engine` (`opl: None`) forwards chip muting to its voices and
    /// treats the OPL-only command as a no-op; the wrapper's job is only to route
    /// by vocabulary, but that routing is what the callback relies on.
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
        let mut engine = Engine {
            inner: Box::new(engine),
            opl: None,
        };

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

    /// A trim never reaches a core (it is the engine's own gain), so the VGM
    /// arm forwarding it is observed in the render: a 0% trim silences.
    #[test]
    fn the_vgm_arm_forwards_chip_trims() {
        struct Constant;
        impl ChipCore for Constant {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(1000);
            }
        }

        let engine = VgmEngine::with_cores(sn_vgm(), 44_100, |_| Some(Box::new(Constant)));
        let mut engine = Engine {
            inner: Box::new(engine),
            opl: None,
        };
        let mut out = vec![0i16; 256];
        engine.render(&mut out);
        assert!(out.iter().any(|&s| s != 0), "sanity: the constant sounds");

        let mut trims = ChipTrims::new();
        trims.set(ChipKind::Sn76489, 0, 0);
        engine.set_chip_trims(trims);
        let mut out = vec![0i16; 256];
        engine.render(&mut out);
        assert!(
            out.iter().all(|&s| s == 0),
            "a 0% trim reached the voice on the VGM arm"
        );
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
