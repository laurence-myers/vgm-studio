//! Peak-level measurement.
//!
//! Drives the same [`VgmEngine`](crate::vgm_engine::VgmEngine) the WAV render
//! does, but instead of writing
//! the mixed frames anywhere it scans them for the loudest one and throws them
//! away -- no allocation, no boost, no limiter. This is the sample-exact
//! equivalent of running vgmtools' `vgm_vol` over a rendered WAV, without the
//! render-to-disk step: the number it returns feeds a VGM volume-modifier
//! suggestion and the "match volume" playback boost (both
//! [`vgms_core::volume`](../../vgms_core/volume/index.html)).

use std::sync::Arc;

use vgms_core::VgmFile;

use crate::resample::ResampleMode;
use crate::vgm_engine::VgmEngine;

/// The loudest sample a render produces, and whether it reached full scale.
///
/// `max_level` is `max |sample|` over the whole render, saturated into `i16`'s
/// positive range: a sample of `i16::MIN` (`-32768`, whose magnitude `i16`
/// cannot hold) reads as `32767` and sets `clipped`, exactly as a genuine
/// full-scale sample already would. `clipped` mirrors `vgm_vol`'s
/// `MaxLvl >= 0x7FFF` warning -- the signal touched full scale, so a louder
/// source would have clipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peak {
    /// The loudest `|sample|` in the render, in `i16` full-scale units
    /// (`0..=32767`).
    pub max_level: i16,
    /// Whether the peak reached full scale (`>= 0x7FFF`).
    pub clipped: bool,
}

impl Peak {
    /// Full scale -- the largest magnitude `i16` can represent, `0x7FFF`.
    pub const FULL_SCALE: i16 = i16::MAX;

    /// Builds a [`Peak`] from a running `max |sample|` held as a `u16` (so
    /// `-32768`'s magnitude of `32768` is representable). Saturates `max_level`
    /// to [`Self::FULL_SCALE`] and flags `clipped` at or above it.
    fn from_abs(abs: u16) -> Self {
        Self {
            max_level: abs.min(Self::FULL_SCALE as u16) as i16,
            clipped: abs >= Self::FULL_SCALE as u16,
        }
    }
}

/// Measures the peak of a full render of `file` at `sample_rate`, through the
/// multichip engine -- a DRO is projected to its VGM first, so every document
/// measures the same way it plays.
///
/// One pass: a freshly built [`VgmEngine`] has no
/// [`LoopConfig`](crate::LoopConfig) and never wraps, and every sample a loop
/// would replay already occurs in that pass. No boost and no muting or panning,
/// so it reports the file's own un-boosted level -- the same signal the faithful
/// [`render_vgm_wav`](crate::render_vgm_wav) writes at the same `resampling`.
/// A chip this app has no core for contributes silence, so measure only what
/// [`playability`](crate::playability) says would be heard.
#[must_use]
pub fn measure_vgm_peak(file: Arc<VgmFile>, sample_rate: u32, resampling: ResampleMode) -> Peak {
    measure_vgm_peak_cancellable(file, sample_rate, resampling, &mut |_| {}, &mut || true)
        .expect("a measurement that is never cancelled always completes")
}

/// As [`measure_vgm_peak`], reporting progress to `on_progress` and polling
/// `keep_going` so a background scan can be abandoned. `None` iff `keep_going`
/// returned `false`. The task service's volume scans go through here.
#[must_use]
pub fn measure_vgm_peak_cancellable(
    file: Arc<VgmFile>,
    sample_rate: u32,
    resampling: ResampleMode,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Option<Peak> {
    let mut engine = VgmEngine::new(file, sample_rate);
    // Measure the sound the user picked -- the render honours the same choice.
    engine.set_resample_mode(resampling);
    let mut buffer = vec![0i16; 4096 * 2];
    let mut abs_peak: u16 = 0;
    loop {
        if !keep_going() {
            return None;
        }
        let frames = engine.render(&mut buffer);
        for &sample in &buffer[..frames * 2] {
            abs_peak = abs_peak.max(sample.unsigned_abs());
        }
        on_progress(engine.position().frames_rendered);
        if frames < buffer.len() / 2 {
            break;
        }
    }
    Some(Peak::from_abs(abs_peak))
}
