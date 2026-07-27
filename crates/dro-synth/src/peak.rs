//! Peak-level measurement.
//!
//! Drives the same [`PlayerEngine`] the WAV render does, but instead of writing
//! the mixed frames anywhere it scans them for the loudest one and throws them
//! away -- no allocation, no boost, no limiter. This is the sample-exact
//! equivalent of running vgmtools' `vgm_vol` over a rendered WAV, without the
//! render-to-disk step: the number it returns feeds a VGM volume-modifier
//! suggestion and the "match volume" playback boost (both
//! [`dro_core::volume`](../../dro_core/volume/index.html)).

use std::borrow::Borrow;

use dro_core::Song;

use crate::engine::PlayerEngine;

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

/// Measures the peak of a full render of `song` at `sample_rate`.
///
/// Renders one pass through the song -- a freshly built [`PlayerEngine`] does
/// not repeat loops (its `loop_config` starts unset), and every sample a loop
/// would replay already occurs in that first pass, so the peak of one pass is
/// the peak of any number of them.
///
/// The measurement never runs through [`BoostLimiter`](crate::BoostLimiter):
/// there is no boost knob here, so it reports the song's own un-boosted level,
/// the same signal the faithful [`render_wav`](crate::render_wav) writes.
///
/// This is the uncancellable shorthand; [`measure_peak_cancellable`] is the one
/// with progress reporting and cancellation.
#[must_use]
pub fn measure_peak<B: Borrow<Song>>(song: B, sample_rate: u32) -> Peak {
    measure_peak_cancellable(song, sample_rate, &mut |_| {}, &mut || true)
        .expect("a measurement that is never cancelled always completes")
}

/// As [`measure_peak`], reporting the running rendered-frame count to
/// `on_progress` between chunks and polling `keep_going` so a background scan
/// can be abandoned -- when the song it belongs to is replaced, say.
/// `None` iff `keep_going` returned `false`.
///
/// The progress and cancellation shape is identical to
/// [`render_wav_cancellable`](crate::render_wav_cancellable), so the task
/// service drives a volume scan exactly as it drives a WAV export.
#[must_use]
pub fn measure_peak_cancellable<B: Borrow<Song>>(
    song: B,
    sample_rate: u32,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Option<Peak> {
    let mut engine = PlayerEngine::new(song, sample_rate);
    let mut buffer = vec![0i16; 4096 * 2];
    let mut abs_peak: u16 = 0;
    loop {
        // Between chunks, as the WAV render polls: often enough that an
        // abandoned scan stops promptly, never mid-buffer.
        if !keep_going() {
            return None;
        }
        let frames = engine.render(&mut buffer);
        for &sample in &buffer[..frames * 2] {
            // `unsigned_abs` so `i16::MIN` measures as its true magnitude 32768
            // instead of overflowing; `Peak::from_abs` saturates it back down.
            abs_peak = abs_peak.max(sample.unsigned_abs());
        }
        on_progress(engine.position().frames_rendered);
        if frames < buffer.len() / 2 {
            break;
        }
    }
    Some(Peak::from_abs(abs_peak))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::render_wav;
    use dro_core::{DroDataV1, OplType};
    use std::io::Cursor;

    /// The same little keyed-on-then-off DRO the WAV render tests use, so a peak
    /// measured here can be checked against that render's samples.
    fn small_song() -> Song {
        Song::dro_v1(
            "small.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x77, // operator setup
                0xA0, 0x98, 0xB0, 0x31, // key on
                0x00, 0x63, // 100 ms delay
                0xB0, 0x11, // key off
                0x00, 0x31, // 50 ms delay
            ])
            .unwrap(),
            150,
            OplType::Opl2,
        )
    }

    /// The loudest `|sample|` in a rendered 16-bit WAV, as a `u32`.
    fn render_abs_peak(song: &Song) -> u32 {
        let bytes = render_wav(song, 48_000, 16).unwrap();
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        reader
            .into_samples::<i32>()
            .map(|s| s.unwrap().unsigned_abs())
            .max()
            .unwrap()
    }

    #[test]
    fn from_abs_saturates_and_flags_clipping() {
        // Below full scale: stored verbatim, not clipped.
        assert_eq!(
            Peak::from_abs(0),
            Peak {
                max_level: 0,
                clipped: false
            }
        );
        assert_eq!(
            Peak::from_abs(16_384),
            Peak {
                max_level: 16_384,
                clipped: false
            }
        );
        // Exactly full scale: kept, and flagged.
        assert_eq!(
            Peak::from_abs(0x7FFF),
            Peak {
                max_level: 0x7FFF,
                clipped: true
            }
        );
        // `i16::MIN`'s magnitude of 0x8000 is representable as a `u16`; it
        // saturates back to full scale and still flags clipping.
        assert_eq!(
            Peak::from_abs(0x8000),
            Peak {
                max_level: 0x7FFF,
                clipped: true
            }
        );
    }

    #[test]
    fn peak_matches_the_wav_render_it_mirrors() {
        // The faithful WAV render writes the very samples `measure_peak` scans
        // (same engine, no boost, no limiter), so their peaks must agree
        // exactly -- which also pins the measurement as boost-independent.
        let song = small_song();
        let peak = measure_peak(&song, 48_000);
        let render_peak = render_abs_peak(&song);

        assert_eq!(
            i64::from(peak.max_level),
            i64::from(render_peak.min(0x7FFF)),
            "measured peak must equal the render's peak"
        );
        assert_eq!(peak.clipped, render_peak >= 0x7FFF);
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn a_keyed_on_note_is_not_silent() {
        let peak = measure_peak(small_song(), 48_000);
        assert!(peak.max_level > 0, "the keyed-on note made no sound");
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn the_sample_rate_does_not_have_to_be_native() {
        // A different output rate still renders the same note; the peak stays
        // sane rather than depending on the resampler's chosen rate.
        for rate in [44_100, 48_000, 49_716] {
            let peak = measure_peak(small_song(), rate);
            assert!(peak.max_level > 0, "silent at {rate} Hz");
        }
    }

    #[test]
    fn a_cancelled_scan_returns_nothing() {
        let song = small_song();
        // Refused before the first chunk.
        assert!(
            measure_peak_cancellable(&song, 48_000, &mut |_| {}, &mut || false).is_none(),
            "an immediately cancelled scan yields None"
        );

        // ...and part-way through: the scan stops early.
        let mut chunks = 0;
        let cancelled = measure_peak_cancellable(&song, 48_000, &mut |_| {}, &mut || {
            chunks += 1;
            chunks <= 1
        });
        assert!(cancelled.is_none());
    }

    #[test]
    fn an_uncancelled_scan_matches_the_shorthand() {
        let song = small_song();
        let simple = measure_peak(&song, 48_000);
        let cancellable =
            measure_peak_cancellable(&song, 48_000, &mut |_| {}, &mut || true).unwrap();
        assert_eq!(simple, cancellable);
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn progress_is_reported_and_only_grows() {
        let song = small_song();
        let mut frames = Vec::new();
        let peak = measure_peak_cancellable(
            &song,
            48_000,
            &mut |rendered| frames.push(rendered),
            &mut || true,
        )
        .unwrap();
        assert!(peak.max_level > 0);
        assert!(!frames.is_empty(), "progress was reported");
        assert!(
            frames.windows(2).all(|pair| pair[0] <= pair[1]),
            "the reported frame count only grows"
        );
        assert!(*frames.last().unwrap() > 0);
    }
}
