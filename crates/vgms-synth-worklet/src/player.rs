// SPDX-License-Identifier: GPL-2.0-or-later
//! The safe playback core behind the worklet ABI.
//!
//! [`WebPlayer`] is the exact analogue of `vgms-audio-native`'s cpal callback,
//! minus the device: it owns whichever engine (`PlayerEngine` for an OPL stream,
//! `VgmEngine` for anything else), the [`BoostLimiter`], and the running peak /
//! limiter state, and renders one buffer per call. The `abi` module is a thin
//! `extern "C"` skin over the process-global instance of it; these methods are
//! plain Rust so the native test suite drives the whole thing without a browser.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use vgms_core::Song;
use vgms_core::vgm::ChipKind;
use vgms_synth::resample::ResampleMode;
use vgms_synth::vgm_engine::VgmEngine;
use vgms_synth::{
    AudioSource, BoostLimiter, ChipMuting, ChipPanning, CoreRegistry, DefaultOplChip, LoopConfig,
    LoopCount, Muting, OplChip, Panning, PlayerEngine, Position,
};

/// The process-wide player the ABI drives. `None` until the first successful
/// [`load`]. A `Mutex` because the borrow checker wants one; wasm is
/// single-threaded, so it never actually contends.
static PLAYER: Mutex<Option<WebPlayer>> = Mutex::new(None);

/// The core choices accumulated by [`set_core_choice`], mirrored into
/// `vgms-synth`'s registry each time one arrives (the registry replaces the whole
/// map, so we keep our own copy to add to).
static CHOICES: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// The reason the most recent [`load`] failed, for the ABI to hand back. Empty
/// after a success.
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

/// Installs the web build's core registry: the app tier's providers exactly as
/// `vgms-app::install_cores` links them, minus the native-only RetroWave board.
///
/// libvgm registers first (the default for every chip it serves); Nuked and the
/// GPL die-sims register behind it as picker options, then the three Nuked
/// promotions the native build also makes. Idempotent: a second call finds the
/// registry already installed and logs rather than fails.
///
/// Shared with `vgms-web` (which links this crate as an rlib for its offline
/// Worker renders) so the two wasm modules cannot disagree about which cores
/// exist.
pub fn install_web_cores() {
    let mut registry = CoreRegistry::with_builtins();
    vgms_cores_libvgm::register(&mut registry);
    vgms_cores_nuked::register(&mut registry);
    vgms_cores_gpl::register(&mut registry);
    registry.promote(ChipKind::Ym2612, "ym2612.nuked");
    registry.promote(ChipKind::Ym2151, "ym2151.nuked");
    registry.promote(ChipKind::Ym2413, "ym2413.nuked");
    if vgms_synth::install(registry).is_err() {
        // Only reachable if setup ran twice; the installed registry is already
        // correct, so this is a note, not a failure.
        log::debug!("the core registry was already installed");
    }
}

/// Records the config's `audio.core.<slug> = <id>` choice, applied to any engine
/// built from the next [`load`] on. Accumulates: each call adds to the set the
/// registry consults.
pub(crate) fn set_core_choice(slug: &str, id: &str) {
    let mut choices = CHOICES.lock().expect("choices mutex not poisoned");
    choices.insert(slug.to_owned(), id.to_owned());
    vgms_synth::registry::set_core_choices(choices.clone());
}

/// The [`ResampleMode`] an ABI code names: `1` is Linear, anything else the
/// accurate Sinc default (matching `ResampleMode::default`).
pub(crate) fn resample_from_code(code: u32) -> ResampleMode {
    match code {
        1 => ResampleMode::Linear,
        _ => ResampleMode::Sinc,
    }
}

/// Parses `bytes` (named `name`, which decides the format) and loads it for
/// playback at `sample_rate`, replacing any current song.
///
/// Mirrors the native `read_any_song_from_path`: a `.vgm`/`.vgz` whose every
/// command projects to OPL loads through that projection (so it plays on the OPL
/// engine, bit-for-bit as the desktop build does), anything else on the generic
/// engine; a DRO always on the OPL engine.
///
/// # Errors
/// If `name`'s bytes are not a song `vgms-core` can parse.
pub(crate) fn load(
    name: &str,
    bytes: &[u8],
    sample_rate: u32,
    resample: ResampleMode,
) -> Result<(), String> {
    let source = read_source(name, bytes)?;
    let player = WebPlayer::new(source, sample_rate.max(1), resample);
    *PLAYER.lock().expect("player mutex not poisoned") = Some(player);
    *LAST_ERROR.lock().expect("error mutex not poisoned") = String::new();
    Ok(())
}

/// Records the reason a load failed, for [`last_error`] to hand back.
pub(crate) fn set_last_error(message: String) {
    *LAST_ERROR.lock().expect("error mutex not poisoned") = message;
}

/// The most recent load failure, or an empty string after a success.
pub(crate) fn last_error() -> String {
    LAST_ERROR.lock().expect("error mutex not poisoned").clone()
}

/// Reads `name`'s `bytes` into an [`AudioSource`], choosing the engine the same
/// way the native reader does.
fn read_source(name: &str, bytes: &[u8]) -> Result<AudioSource, String> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vgm") || lower.ends_with(".vgz") {
        let file = vgms_core::vgm::file::read(name, bytes).map_err(|error| error.to_string())?;
        Ok(match file.to_song() {
            Some(song) => AudioSource::Opl(Arc::new(song)),
            None => AudioSource::Vgm(Arc::new(file)),
        })
    } else {
        let song = vgms_core::io::read_song(name, bytes).map_err(|error| error.to_string())?;
        Ok(AudioSource::Opl(Arc::new(song)))
    }
}

/// Runs `f` against the loaded player, or returns `default` when nothing is
/// loaded.
fn with_player<R>(default: R, f: impl FnOnce(&mut WebPlayer) -> R) -> R {
    match PLAYER.lock().expect("player mutex not poisoned").as_mut() {
        Some(player) => f(player),
        None => default,
    }
}

// -- the process-global surface the ABI calls -----------------------------------

pub(crate) fn render(left: &mut [f32], right: &mut [f32]) -> usize {
    with_player(0, |player| player.render(left, right))
}
pub(crate) fn seek_ms(ms: u32) {
    with_player((), |player| player.engine.seek_to_ms(ms));
}
pub(crate) fn seek_pos(pos: usize) {
    with_player((), |player| player.engine.seek_to_pos(pos));
}
pub(crate) fn rewind() {
    with_player((), |player| player.engine.rewind());
}
pub(crate) fn set_boost(boost: f32) {
    with_player((), |player| player.limiter.set_boost(boost));
}
pub(crate) fn set_loop(config: Option<LoopConfig>) {
    with_player((), |player| player.engine.set_loop(config));
}
pub(crate) fn set_muting(muting: Muting) {
    with_player((), |player| player.engine.set_muting(muting));
}
pub(crate) fn set_panning(panning: Panning) {
    with_player((), |player| player.engine.set_panning(panning));
}
pub(crate) fn set_chip_mute(kind: ChipKind, instance: u8, mask: u32) {
    with_player((), |player| player.set_chip_mute(kind, instance, mask));
}
pub(crate) fn set_chip_pan(kind: ChipKind, instance: u8, pans: Vec<i16>) {
    with_player((), |player| player.set_chip_pan(kind, instance, pans));
}
pub(crate) fn position_frames() -> f64 {
    with_player(0.0, |player| {
        player.engine.position().frames_rendered as f64
    })
}
pub(crate) fn position_ms() -> u32 {
    with_player(0, |player| player.engine.position().elapsed_ms)
}
pub(crate) fn position_row() -> u32 {
    with_player(0, |player| {
        u32::try_from(player.engine.position().next_instruction).unwrap_or(u32::MAX)
    })
}
pub(crate) fn loop_iteration() -> u32 {
    with_player(0, |player| player.engine.position().loop_iteration)
}
pub(crate) fn is_finished() -> bool {
    with_player(false, |player| player.engine.is_finished())
}
pub(crate) fn take_peak(channel: u32) -> f32 {
    with_player(0.0, |player| player.take_peak(channel))
}
pub(crate) fn take_limited() -> bool {
    with_player(false, WebPlayer::take_limited)
}
pub(crate) fn min_engaged_boost() -> f32 {
    with_player(0.0, WebPlayer::min_engaged_boost)
}

/// One loaded song and everything the render loop needs around it.
pub(crate) struct WebPlayer {
    engine: Engine,
    limiter: BoostLimiter,
    /// Reused interleaved i16 scratch, grown to the largest quantum seen so the
    /// steady state never allocates.
    scratch: Vec<i16>,
    /// Running post-limiter peak per channel, in `u16` full-scale magnitude;
    /// reset to zero when read (a destructive read, like the native meter).
    peak_left: u16,
    peak_right: u16,
    /// Whether the limiter engaged since the last [`take_limited`]. Reset on read.
    limited: bool,
    /// The lowest boost at which the limiter has engaged since this song loaded,
    /// or `0.0` for "never".
    min_engaged_boost: f32,
    /// The generic engine's mutes/pans, accumulated per chip instance (the ABI
    /// sets one at a time), kept so each change re-applies the whole set.
    chip_muting: ChipMuting,
    chip_panning: ChipPanning,
}

impl WebPlayer {
    pub(crate) fn new(source: AudioSource, sample_rate: u32, resample: ResampleMode) -> Self {
        Self {
            engine: Engine::build(&source, sample_rate, resample),
            // Boost starts at unity; the host flushes the configured boost right
            // after load, exactly as the native service flushes it on play.
            limiter: BoostLimiter::new(sample_rate, 1.0),
            scratch: Vec::new(),
            peak_left: 0,
            peak_right: 0,
            limited: false,
            min_engaged_boost: 0.0,
            chip_muting: ChipMuting::new(),
            chip_panning: ChipPanning::new(),
        }
    }

    /// Renders one quantum into the planar `left`/`right` output buffers the
    /// AudioWorklet supplies, boosting, limiting and metering exactly as the
    /// native callback does. Returns the number of frames the engine sounded
    /// (the tail is zeroed either way).
    pub(crate) fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> usize {
        let frames = left.len().min(right.len());
        if self.scratch.len() < frames * 2 {
            self.scratch.resize(frames * 2, 0);
        }
        let buf = &mut self.scratch[..frames * 2];
        let produced = self.engine.render(buf);

        // Boost + limit the i16 frames, recording the lowest boost that has
        // clipped so the host can cap the volume there -- the native order.
        if self.limiter.process(buf) {
            self.limited = true;
            let boost = self.limiter.boost();
            if self.min_engaged_boost == 0.0 || boost < self.min_engaged_boost {
                self.min_engaged_boost = boost;
            }
        }
        let (peak_l, peak_r) = channel_peaks(buf);
        self.peak_left = self.peak_left.max(peak_l);
        self.peak_right = self.peak_right.max(peak_r);

        for (frame, pair) in buf.chunks_exact(2).enumerate() {
            left[frame] = f32::from(pair[0]) / 32768.0;
            right[frame] = f32::from(pair[1]) / 32768.0;
        }
        produced
    }

    fn set_chip_mute(&mut self, kind: ChipKind, instance: u8, mask: u32) {
        self.chip_muting.set(kind, instance, mask);
        self.engine.set_chip_muting(self.chip_muting.clone());
    }

    fn set_chip_pan(&mut self, kind: ChipKind, instance: u8, pans: Vec<i16>) {
        self.chip_panning.set(kind, instance, pans);
        self.engine.set_chip_panning(self.chip_panning.clone());
    }

    /// The loudest post-limiter peak on `channel` (0 = left, else right) since the
    /// last call, normalised to `0.0..=1.0`. Destructive: reported once.
    fn take_peak(&mut self, channel: u32) -> f32 {
        let peak = if channel == 0 {
            std::mem::take(&mut self.peak_left)
        } else {
            std::mem::take(&mut self.peak_right)
        };
        f32::from(peak) / 32768.0
    }

    fn take_limited(&mut self) -> bool {
        std::mem::take(&mut self.limited)
    }

    fn min_engaged_boost(&mut self) -> f32 {
        self.min_engaged_boost
    }
}

/// The loudest `|sample|` per channel over interleaved stereo `frames`.
fn channel_peaks(frames: &[i16]) -> (u16, u16) {
    let mut left = 0u16;
    let mut right = 0u16;
    for pair in frames.chunks_exact(2) {
        left = left.max(pair[0].unsigned_abs());
        right = right.max(pair[1].unsigned_abs());
    }
    (left, right)
}

/// Whichever engine is driving playback -- the worklet's counterpart of
/// `vgms-audio-native`'s private `Engine`. The six things the render loop needs
/// (render, seek, rewind, loop, position, finished) are answered by both; the
/// register-policy methods are no-ops on the engine that has no such policy.
enum Engine {
    Opl(Box<PlayerEngine<Arc<Song>, Box<dyn OplChip>>>),
    Vgm(Box<VgmEngine>),
}

impl Engine {
    fn build(source: &AudioSource, sample_rate: u32, resample: ResampleMode) -> Self {
        match source {
            AudioSource::Opl(song) => {
                // The chosen OPL core, or the registry default when this build
                // lacks it. The registry-side choice runs ahead of any config,
                // exactly as the native builder consults it.
                let choice = vgms_synth::registry::core_choice(ChipKind::Ymf262);
                let chip = vgms_synth::registry()
                    .build_opl(choice.as_deref(), sample_rate)
                    .unwrap_or_else(|| Box::new(DefaultOplChip::new(sample_rate)));
                Self::Opl(Box::new(PlayerEngine::with_chip(
                    Arc::clone(song),
                    chip,
                    sample_rate,
                )))
            }
            AudioSource::Vgm(file) => {
                // Realtime cores only, as the native transport does -- an offline
                // LLE die-sim would underrun the audio thread.
                let mut engine = VgmEngine::with_cores(Arc::clone(file), sample_rate, |kind| {
                    vgms_synth::core_for_realtime(kind)
                });
                engine.set_resample_mode(resample);
                Self::Vgm(Box::new(engine))
            }
        }
    }

    fn render(&mut self, out: &mut [i16]) -> usize {
        match self {
            Self::Opl(engine) => engine.render(out),
            Self::Vgm(engine) => engine.render(out),
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
}

/// Builds a [`LoopCount`] from the ABI's `(tag, times)` pair: tag `0` is
/// [`LoopCount::Infinite`], any other tag [`LoopCount::Times`].
pub(crate) fn loop_count(tag: u32, times: u32) -> LoopCount {
    match tag {
        0 => LoopCount::Infinite,
        _ => LoopCount::Times(times),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The projectable OPL VGM fixture -- exercises the OPL engine arm.
    const OPL_VGM: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    fn install_once() {
        install_web_cores();
    }

    /// A minimal single-chip SN76489 VGM: latch a tone period, open the volume,
    /// wait, end. Exercises the generic `VgmEngine` arm through a real core.
    fn sn76489_vgm() -> Vec<u8> {
        fn put(bytes: &mut [u8], offset: usize, value: u32) {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut out = vec![0u8; 0x80];
        out[..4].copy_from_slice(b"Vgm ");
        put(&mut out, 0x08, 0x151); // version
        put(&mut out, 0x34, 0x80 - 0x34); // data offset (relative)
        put(&mut out, 0x0C, 3_579_545); // SN76489 clock
        put(&mut out, 0x18, 44_100); // total samples (1 s)
        out.extend_from_slice(&[
            0x50, 0x8E, // latch ch0 tone, low 4 bits of period
            0x50, 0x02, // high 6 bits of period
            0x50, 0x90, // ch0 volume: full
            0x61, 0x44, 0xAC, // wait 44100 samples
            0x66, // end of data
        ]);
        let eof = (out.len() - 4) as u32;
        put(&mut out, 0x04, eof);
        out
    }

    fn peak_of(left: &[f32], right: &[f32]) -> f32 {
        left.iter()
            .chain(right.iter())
            .map(|s| s.abs())
            .fold(0.0f32, f32::max)
    }

    /// Renders `player` for `frames` in 128-sample quanta and returns the loudest
    /// output sample seen -- the "did it sound" measure.
    fn render_peak(player: &mut WebPlayer, frames: usize) -> f32 {
        let mut peak = 0.0f32;
        let mut left = [0.0f32; 128];
        let mut right = [0.0f32; 128];
        let mut done = 0;
        while done < frames {
            player.render(&mut left, &mut right);
            peak = peak.max(peak_of(&left, &right));
            done += 128;
        }
        peak
    }

    #[test]
    fn an_opl_vgm_loads_and_sounds() {
        install_once();
        let source = read_source("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses");
        assert!(matches!(source, AudioSource::Opl(_)), "projects to OPL");
        let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);
        assert!(
            render_peak(&mut player, 48_000) > 0.01,
            "the OPL engine sounds"
        );
    }

    #[test]
    fn an_sn76489_vgm_loads_on_the_generic_engine_and_sounds() {
        install_once();
        let bytes = sn76489_vgm();
        let source = read_source("tone.vgm", &bytes).expect("fixture parses");
        assert!(
            matches!(source, AudioSource::Vgm(_)),
            "a bare SN76489 is generic"
        );
        let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);
        assert!(
            render_peak(&mut player, 24_000) > 0.01,
            "the generic engine sounds through a real core"
        );
    }

    #[test]
    fn seek_and_finish_track_the_song() {
        install_once();
        let source = read_source("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses");
        let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);

        // A fresh song is not finished; rendering well past its end finishes it.
        assert!(!player.engine.is_finished());
        render_peak(&mut player, 48_000 * 60);
        assert!(
            player.engine.is_finished(),
            "a fully-rendered song finishes"
        );

        // Seeking back to the start un-finishes it and it plays again.
        player.engine.seek_to_ms(0);
        assert!(!player.engine.is_finished(), "a seek rewinds past the end");
        assert!(
            render_peak(&mut player, 48_000) > 0.01,
            "and it sounds again"
        );
    }

    #[test]
    fn the_peak_meter_reads_once_then_resets() {
        install_once();
        let source = read_source("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses");
        let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);
        let mut left = [0.0f32; 128];
        let mut right = [0.0f32; 128];
        // Render until at least one channel has some energy.
        for _ in 0..400 {
            player.render(&mut left, &mut right);
            if player.peak_left > 0 || player.peak_right > 0 {
                break;
            }
        }
        let first = player.take_peak(0).max(player.take_peak(1));
        assert!(first > 0.0, "a sounded pass reports a peak");
        // A read is destructive: with no render between, the next read is zero.
        assert_eq!(player.take_peak(0), 0.0, "the peak resets after a read");
        assert_eq!(player.take_peak(1), 0.0);
    }

    #[test]
    fn muting_every_opl_channel_quiets_the_stream() {
        install_once();
        // The same fixture, once as it plays and once with every channel muted
        // (drums masked away too -- Muting::from_raw(0, [0xE0; 2])). The muting
        // has to reach the engine through exactly the ABI's raw pair, so a
        // fully-muted pass is a small fraction of the loud one.
        let loud = {
            let source = read_source("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses");
            let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);
            render_peak(&mut player, 48_000)
        };
        let muted = {
            let source = read_source("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses");
            let mut player = WebPlayer::new(source, 48_000, ResampleMode::Sinc);
            player.engine.set_muting(Muting::from_raw(0, [0xE0, 0xE0]));
            render_peak(&mut player, 48_000)
        };
        assert!(loud > 0.01, "the unmuted stream sounds");
        assert!(
            muted < loud * 0.1,
            "muting every channel quiets the stream: loud={loud}, muted={muted}"
        );
    }

    #[test]
    fn the_loop_count_pair_decodes() {
        assert_eq!(loop_count(0, 5), LoopCount::Infinite);
        assert_eq!(loop_count(1, 5), LoopCount::Times(5));
    }
}
