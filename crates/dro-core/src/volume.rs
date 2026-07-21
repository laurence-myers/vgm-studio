//! Turning a measured peak into a suggested loudness.
//!
//! Two pure, dependency-free helpers translate the peak level a render reached
//! (see `dro_synth::measure_peak`) into the two loudness levers this app
//! exposes:
//!
//! - [`suggest_volume_modifier`] -- the VGM header `Volume Modifier` byte
//!   (offset `0x7C`), so a pack's tracks can be levelled to a consistent
//!   loudness. This is the sample-exact equivalent of vgmtools' `vgm_vol`.
//! - [`boost_for_peak`] -- the live-playback boost factor that brings a quiet
//!   song up to full scale, for the "match volume" button beside the boost
//!   stepper.
//!
//! Both take an `i16` peak rather than `dro_synth`'s `Peak`, keeping this crate
//! free of any audio dependency; the GUI reads `peak.max_level` and passes it
//! in.

/// The full-scale reference the loudness maths normalise toward: `0x8000`.
///
/// `vgm_vol` uses this same `0x8000` as its "0 dBFS" point even though a signed
/// 16-bit sample only reaches `0x7FFF`; the one-count difference is far below
/// the modifier's `1/32`-of-a-doubling resolution, and matching it keeps this
/// suggestion bit-for-bit with the tool packs are QA'd against.
const FULL_SCALE: f64 = 32_768.0;

/// The largest byte the header's *gain* range uses: `+0xC0` steps (`+6` dB × 32
/// = 192 steps above unity). Bytes `0x00..=0xC0` are gain; `0xC1..=0xFF` are
/// attenuation of `-63..=-1` steps.
const MAX_GAIN_STEPS: i32 = 0xC0;

/// The most attenuation the header can express: `-63` steps, stored as `0xC1`.
const MAX_ATTEN_STEPS: i32 = -63;

/// Suggests a VGM header `Volume Modifier` byte that brings `peak` (or the
/// louder `album_peak`, when levelling a whole pack) to full scale.
///
/// Mirrors `vgm_vol`'s `PrintVolMod`: a linear `factor = 0x8000 / peak` becomes
/// `floor(log2(factor) * 0x20)` signed steps -- **32 steps per doubling** -- and
/// those steps are encoded into the header's split byte (see
/// [`encode_volume_modifier`]). A measured peak is always `<= 0x7FFF`, so the
/// factor is always `>= 1` and the result always lands in the gain range
/// `0x00..=0xC0`; the attenuation range exists only for completeness of the
/// encoding.
///
/// - **Track mode** (`album_peak` is `None`): each track is normalised to its
///   own peak.
/// - **Album mode** (`album_peak` is `Some`): every track is scaled by the one
///   factor derived from the pack's loudest peak, so their relative levels are
///   preserved -- `vgm_vol`'s `MaxLvlAlbum` behaviour. The track's own `peak` is
///   then unused; pass the album maximum as `album_peak`.
///
/// A `peak` of `0` (pure silence) is treated as `1` so the maths stay finite;
/// it clamps to maximum gain, which is harmless for a track that makes no sound.
#[must_use]
pub fn suggest_volume_modifier(peak: i16, album_peak: Option<i16>) -> u8 {
    let effective = i32::from(album_peak.unwrap_or(peak)).max(1);
    let factor = FULL_SCALE / f64::from(effective);
    let steps = (factor.log2() * 32.0).floor() as i32;
    encode_volume_modifier(steps)
}

/// Encodes a signed step count (32 per doubling) into the VGM `Volume Modifier`
/// byte's split range, clamping to what the byte can hold.
///
/// Per the VGM spec, the byte is not plain two's-complement: `0x00..=0xC0` are
/// `0..=192` gain steps, while `0xC1..=0xFF` are `-63..=-1` attenuation steps.
/// So `0` → `0x00`, `+32` (one doubling) → `0x20`, `+192` → `0xC0`, `-1` →
/// `0xFF`, `-63` → `0xC1`. Values beyond either end clamp to it.
#[must_use]
pub fn encode_volume_modifier(steps: i32) -> u8 {
    if steps >= 0 {
        steps.min(MAX_GAIN_STEPS) as u8
    } else {
        // -1 -> 0xFF, -63 -> 0xC1; anything lower clamps to -63.
        (256 + steps.max(MAX_ATTEN_STEPS)) as u8
    }
}

/// The playback boost that brings `peak` up to full scale, clamped to the
/// `[1.0, 16.0]` range [`AudioConfig::boost`](crate::config::AudioConfig::boost)
/// accepts.
///
/// `boost = clamp(0x8000 / peak, 1.0, 16.0)`: a song already at full scale gets
/// unity, a half-scale (`-6` dB) song gets `2.0`, and anything quieter than
/// `1/16` scale clamps to the `16.0` ceiling. A `peak` of `0` is treated as `1`
/// so the division stays finite (it then clamps to the ceiling).
#[must_use]
pub fn boost_for_peak(peak: i16) -> f32 {
    let peak = f32::from(peak).max(1.0);
    (32_768.0 / peak).clamp(1.0, 16.0)
}

/// The peak level in dBFS, for a readout beside the modifier and boost controls.
///
/// `20 * log10(peak / 0x8000)`: `0` dBFS at full scale, `-6.02` at half,
/// [`f32::NEG_INFINITY`] for pure silence.
#[must_use]
pub fn peak_dbfs(peak: i16) -> f32 {
    let peak = f32::from(peak);
    if peak <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * (peak / 32_768.0).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_modifier_pins_known_peaks() {
        // (peak, expected byte). 32 steps per doubling: each halving of the peak
        // adds 0x20 to the modifier.
        let cases = [
            (0x7FFF, 0x00), // full scale -> no change
            (0x4000, 0x20), // half   -> +1 doubling
            (0x2000, 0x40), // quarter -> +2 doublings
            (0x1000, 0x60), // eighth  -> +3 doublings
            (0x0800, 0x80), // 1/16    -> +4 doublings
        ];
        for (peak, expected) in cases {
            assert_eq!(
                suggest_volume_modifier(peak, None),
                expected,
                "peak {peak:#06X}"
            );
        }
    }

    #[test]
    fn album_mode_levels_every_track_by_the_pack_maximum() {
        // The loudest track in a pack peaks at half scale.
        let album_peak = Some(0x4000);
        // A track that is itself at full scale, and a much quieter one, both get
        // the *same* modifier -- the one the album maximum implies (0x20) --
        // preserving their relative levels rather than flattening them.
        assert_eq!(suggest_volume_modifier(0x7FFF, album_peak), 0x20);
        assert_eq!(suggest_volume_modifier(0x0800, album_peak), 0x20);
        // Track mode would instead normalise each to its own peak.
        assert_eq!(suggest_volume_modifier(0x0800, None), 0x80);
    }

    #[test]
    fn encoding_covers_the_split_range() {
        // Gain range 0x00..=0xC0.
        assert_eq!(encode_volume_modifier(0), 0x00);
        assert_eq!(encode_volume_modifier(32), 0x20);
        assert_eq!(encode_volume_modifier(0xC0), 0xC0);
        // Gain clamps at 0xC0.
        assert_eq!(encode_volume_modifier(1_000), 0xC0);
        // Attenuation range 0xC1..=0xFF for -63..=-1 steps.
        assert_eq!(encode_volume_modifier(-1), 0xFF);
        assert_eq!(encode_volume_modifier(-32), 0xE0);
        assert_eq!(encode_volume_modifier(-63), 0xC1);
        // Attenuation clamps at -63 (0xC1).
        assert_eq!(encode_volume_modifier(-1_000), 0xC1);
    }

    #[test]
    fn silence_is_finite_and_clamps_to_maximum_gain() {
        assert_eq!(suggest_volume_modifier(0, None), 0xC0);
        assert_eq!(boost_for_peak(0), 16.0);
        assert_eq!(peak_dbfs(0), f32::NEG_INFINITY);
    }

    #[test]
    fn boost_brings_the_peak_to_full_scale_within_range() {
        assert!(
            (boost_for_peak(0x7FFF) - 1.0).abs() < 0.001,
            "full scale -> unity"
        );
        assert!((boost_for_peak(0x4000) - 2.0).abs() < 0.001, "half -> 2x");
        assert!(
            (boost_for_peak(0x0800) - 16.0).abs() < 0.001,
            "1/16 -> ceiling"
        );
        // Quieter than the 16x ceiling can reach still clamps, never exceeds it.
        assert_eq!(boost_for_peak(0x0400), 16.0);
        // And it never drops below unity.
        assert!(boost_for_peak(i16::MAX) >= 1.0);
    }

    #[test]
    fn boost_stays_inside_the_configurable_range() {
        // Every peak yields a boost the config's validator (1.0..=16.0) accepts.
        for raw in (0..=i16::MAX as i32).step_by(97) {
            let boost = boost_for_peak(raw as i16);
            assert!(
                (1.0..=16.0).contains(&boost),
                "peak {raw} gave out-of-range boost {boost}"
            );
        }
    }

    #[test]
    fn dbfs_tracks_the_familiar_landmarks() {
        assert!(peak_dbfs(0x7FFF).abs() < 0.01, "full scale is ~0 dBFS");
        assert!((peak_dbfs(0x4000) - -6.02).abs() < 0.05, "half is ~-6 dBFS");
        assert!(
            (peak_dbfs(0x2000) - -12.04).abs() < 0.05,
            "quarter is ~-12 dBFS"
        );
    }
}
