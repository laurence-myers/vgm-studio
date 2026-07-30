//! Turning a measured peak into a suggested loudness.
//!
//! Two pure, dependency-free helpers translate the peak level a render reached
//! (see `vgms_synth::measure_peak`) into the two loudness levers this app
//! exposes:
//!
//! - [`suggest_volume_modifier`] -- the VGM header `Volume Modifier` byte
//!   (offset `0x7C`), so a pack's tracks can be levelled to a consistent
//!   loudness. This is the sample-exact equivalent of vgmtools' `vgm_vol`.
//! - [`boost_for_peak`] -- the live-playback boost factor that brings a quiet
//!   song up to full scale, for the "match volume" button beside the boost
//!   stepper.
//!
//! [`volume_modifier_factor`] runs the header byte the other way -- back to the
//! `0.25x..=64x` gain a player would apply -- so the GUI can show what a
//! suggested (or hand-entered) modifier actually does.
//!
//! The peak helpers take an `i16` peak rather than `vgms_synth`'s `Peak`, keeping
//! this crate free of any audio dependency; the GUI reads `peak.max_level` and
//! passes it in.

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

/// The most attenuation the header byte can express, as a step count for the
/// `256 + steps` encoding: `-63` maps to `0xC1`. Players then read `0xC1` as
/// `-64` -- a clean `0.25x` -- but the encoding arithmetic keeps `-63` so the
/// byte comes out `0xC1` (see [`volume_modifier_factor`]).
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
///
/// The extreme `0xC1` is special: players read it as `-64` rather than `-63` so
/// the smallest factor is a clean `0.25x`, making the reachable player-side
/// range `0.25x..=64x` (see [`volume_modifier_factor`]). That substitution is a
/// decode-time detail -- the encoding still writes `0xC1` from `-63` steps.
#[must_use]
pub fn encode_volume_modifier(steps: i32) -> u8 {
    if steps >= 0 {
        steps.min(MAX_GAIN_STEPS) as u8
    } else {
        // -1 -> 0xFF, -63 -> 0xC1; anything lower clamps to -63.
        (256 + steps.max(MAX_ATTEN_STEPS)) as u8
    }
}

/// The playback gain a VGM `Volume Modifier` byte asks players for:
/// `2^(steps / 0x20)`, the inverse of [`encode_volume_modifier`].
///
/// Decodes the split range -- `0x00..=0xC0` as `0..=192` gain steps,
/// `0xC1..=0xFF` as `-63..=-1` attenuation steps -- with the spec's one quirk:
/// byte `0xC1` (nominally `-63`) is read as `-64`, so the smallest factor is a
/// clean `0.25` rather than `0.2557`. The reachable range is therefore
/// `0.25..=64.0`, and the default `0x00` is unity. Feeds the "this modifier
/// means N×" readout beside the metadata field.
#[must_use]
pub fn volume_modifier_factor(modifier: u8) -> f32 {
    let steps = if i32::from(modifier) <= MAX_GAIN_STEPS {
        i32::from(modifier)
    } else {
        // 0xC1..=0xFF -> -63..=-1, but 0xC1's -63 is read as -64 (a clean 0.25x).
        let nominal = i32::from(modifier) - 256;
        if nominal == MAX_ATTEN_STEPS {
            -64
        } else {
            nominal
        }
    };
    2.0f32.powf(steps as f32 / 32.0)
}

/// The modifier byte whose playback factor is closest (in log space, so the
/// perceptual distance is even) to `factor`, saturating past the ladder's ends.
///
/// Snaps a free-form boost -- a hand-edited `vgmstudio.ini` value, or the exact
/// `boost_for_peak` a "Match Volume" produces -- onto a real modifier value, so
/// the stepper always sits on a ladder position and the number it shows is one a
/// player could reproduce.
///
/// Because the ladder is geometric (one byte step = `2^(1/32)`), "nearest in log
/// space" is just the step count rounded: the inverse of
/// [`volume_modifier_factor`], with [`encode_volume_modifier`]'s clamps covering
/// the ends (its `-63` byte `0xC1` reads back as `-64`, the clean `0.25x` floor).
/// `max(MIN_POSITIVE)` keeps a zero or negative input finite.
#[must_use]
pub fn nearest_volume_modifier(factor: f32) -> u8 {
    let steps = factor.max(f32::MIN_POSITIVE).log2() * 32.0;
    encode_volume_modifier(steps.round() as i32)
}

/// The next ladder volume *above* `factor`, for the volume stepper's up arrow.
///
/// Steps by about `1.0` at unity and above (so the lever climbs `1x -> 2x -> 3x`)
/// and by about `0.1` below unity (`0.8x -> 0.9x -> 1.0x`), then snaps to the
/// nearest modifier value. Saturating at the `64x` ceiling returns `factor`
/// unchanged, which the widget reads as "cannot go higher".
#[must_use]
pub fn volume_step_up(factor: f32) -> f32 {
    let step = if factor >= 1.0 { 1.0 } else { 0.1 };
    volume_modifier_factor(nearest_volume_modifier(factor + step))
}

/// The next ladder volume *below* `factor`; the mirror of [`volume_step_up`].
///
/// The unity boundary is asymmetric so the sequence is continuous: `factor > 1.0`
/// takes the `1.0` step, but `1.0` itself takes the `0.1` step, so `1.00x` steps
/// down to `~0.90x` rather than jumping to the `0.25x` floor. Saturates at that
/// floor.
#[must_use]
pub fn volume_step_down(factor: f32) -> f32 {
    let step = if factor > 1.0 { 1.0 } else { 0.1 };
    volume_modifier_factor(nearest_volume_modifier(factor - step))
}

/// The playback boost that brings `peak` up to full scale, clamped to the
/// gain half of the `0.25..=64.0` range
/// [`AudioConfig::boost`](crate::config::AudioConfig::boost) accepts.
///
/// `boost = clamp(0x8000 / peak, 1.0, 64.0)`: a song already at full scale gets
/// unity, a half-scale (`-6` dB) song gets `2.0`, and anything quieter than
/// `1/64` scale clamps to the `64.0` ceiling. The floor stays `1.0` -- a
/// measured peak is never over full scale, so matching it only ever boosts,
/// never attenuates. A `peak` of `0` is treated as `1` so the division stays
/// finite (it then clamps to the ceiling). The "Match Volume" caller snaps the
/// result to the modifier ladder with [`nearest_volume_modifier`].
#[must_use]
pub fn boost_for_peak(peak: i16) -> f32 {
    let peak = f32::from(peak).max(1.0);
    (32_768.0 / peak).clamp(1.0, 64.0)
}

/// The modifier-ladder volume that brings `peak` up to full scale -- what the
/// "Match Volume" button sets the playback lever to.
///
/// [`boost_for_peak`] gives the exact factor; this snaps it onto the modifier
/// ladder with [`nearest_volume_modifier`] so the lever sits on a real modifier
/// value. Silence (`peak == 0`) has no meaningful match; callers should special
/// case it (this returns the max-gain clamp, as `boost_for_peak` does).
#[must_use]
pub fn matched_volume(peak: i16) -> f32 {
    volume_modifier_factor(nearest_volume_modifier(boost_for_peak(peak)))
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
    fn decoding_a_modifier_matches_the_spec_range() {
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // Gain range: default unity, then a doubling per 0x20, up to +192 = 64x.
        assert!(
            approx(volume_modifier_factor(0x00), 1.0),
            "default is unity"
        );
        assert!(approx(volume_modifier_factor(0x20), 2.0));
        assert!(approx(volume_modifier_factor(0x40), 4.0));
        assert!(
            approx(volume_modifier_factor(0xC0), 64.0),
            "max gain is 64x"
        );
        // Attenuation range: a halving per -0x20 (0xE0 = -32 steps = 0.5x)...
        assert!(approx(volume_modifier_factor(0xE0), 0.5));
        assert!(approx(
            volume_modifier_factor(0xFF),
            2.0f32.powf(-1.0 / 32.0)
        ));
        // ...and the spec's quirk: 0xC1 is read as -64, not -63, for a clean
        // 0.25x floor rather than 0.2557x.
        assert!(approx(volume_modifier_factor(0xC1), 0.25), "0xC1 is 0.25x");
        assert!(
            !approx(volume_modifier_factor(0xC1), 2.0f32.powf(-63.0 / 32.0)),
            "0xC1 must not decode as the nominal -63 steps"
        );
    }

    #[test]
    fn a_suggested_modifier_decodes_to_roughly_the_scale_up_it_asked_for() {
        // The round trip is lossy (the byte quantises to 1/32-of-a-doubling
        // steps), but a half-scale peak suggests 0x20, which is a 2x lift -- so
        // applying it to that peak lands near full scale, as intended.
        let modifier = suggest_volume_modifier(0x4000, None);
        let lifted = f32::from(0x4000i32 as i16) * volume_modifier_factor(modifier);
        assert!(
            (lifted - 32_768.0).abs() < 1_024.0,
            "half-scale peak lifted to ~full scale, got {lifted}"
        );
    }

    #[test]
    fn silence_is_finite_and_clamps_to_maximum_gain() {
        assert_eq!(suggest_volume_modifier(0, None), 0xC0);
        assert_eq!(boost_for_peak(0), 64.0);
        assert_eq!(peak_dbfs(0), f32::NEG_INFINITY);
    }

    #[test]
    fn boost_brings_the_peak_to_full_scale_within_range() {
        assert!(
            (boost_for_peak(0x7FFF) - 1.0).abs() < 0.001,
            "full scale -> unity"
        );
        assert!((boost_for_peak(0x4000) - 2.0).abs() < 0.001, "half -> 2x");
        assert!((boost_for_peak(0x0800) - 16.0).abs() < 0.001, "1/16 -> 16x");
        assert!(
            (boost_for_peak(0x0200) - 64.0).abs() < 0.001,
            "1/64 -> the ceiling"
        );
        // Quieter than the 64x ceiling can reach still clamps, never exceeds it.
        assert_eq!(boost_for_peak(0x0100), 64.0);
        // And it never drops below unity: matching only ever boosts.
        assert!(boost_for_peak(i16::MAX) >= 1.0);
    }

    #[test]
    fn boost_stays_inside_the_configurable_range() {
        // Every peak yields a boost the config's validator (0.25..=64.0) accepts;
        // matching only boosts, so the effective floor is 1.0.
        for raw in (0..=i16::MAX as i32).step_by(97) {
            let boost = boost_for_peak(raw as i16);
            assert!(
                (1.0..=64.0).contains(&boost),
                "peak {raw} gave out-of-range boost {boost}"
            );
        }
    }

    #[test]
    fn nearest_snaps_a_free_factor_onto_the_ladder() {
        // Exact ladder values map to themselves -- including the 0xC1 = 0.25x
        // floor, whose byte is nominally -63 steps but decodes as -64.
        assert_eq!(nearest_volume_modifier(1.0), 0x00);
        assert_eq!(nearest_volume_modifier(2.0), 0x20);
        assert_eq!(nearest_volume_modifier(0.25), 0xC1);
        assert_eq!(nearest_volume_modifier(64.0), 0xC0);
        // Every byte round-trips through its own factor -- including 0xC1, whose
        // decoded -64 steps clamp back to the -63 encoding, i.e. the same byte --
        // so the snap is exact across the whole ladder.
        for byte in 0..=u8::MAX {
            assert_eq!(
                nearest_volume_modifier(volume_modifier_factor(byte)),
                byte,
                "byte {byte:#04X} does not round-trip"
            );
        }
        // Off-ladder values snap to the perceptually nearest, and out-of-range
        // values saturate at the ends.
        assert_eq!(
            nearest_volume_modifier(1.99),
            0x20,
            "just under 2x rounds to it"
        );
        assert_eq!(nearest_volume_modifier(1000.0), 0xC0, "above 64x clamps");
        assert_eq!(nearest_volume_modifier(0.01), 0xC1, "below 0.25x clamps");
        // A snapped Match-Volume boost decodes back to about the boost asked for.
        let boost = boost_for_peak(0x3000); // ~2.67x
        let snapped = volume_modifier_factor(nearest_volume_modifier(boost));
        assert!((snapped - boost).abs() < 0.05, "{snapped} vs {boost}");
    }

    #[test]
    fn volume_steps_are_coarse_above_unity_and_fine_below() {
        // Snapping to the geometric ladder means "about", not exact.
        let approx = |a: f32, b: f32| (a - b).abs() < 0.06;

        // At unity and above, the arrows move in whole numbers.
        assert!(
            approx(volume_step_up(1.0), 2.0),
            "1 -> {}",
            volume_step_up(1.0)
        );
        assert!(approx(volume_step_up(2.0), 3.0));
        assert!(approx(volume_step_down(3.0), 2.0));
        assert!(approx(volume_step_down(2.0), 1.0));

        // The unity boundary is continuous: down from 1.0 is a fine 0.1 step to
        // ~0.9, not a jump to the 0.25 floor; up from ~0.9 lands back on 1.0.
        assert!(
            approx(volume_step_down(1.0), 0.9),
            "1 down -> {}",
            volume_step_down(1.0)
        );
        assert!(approx(volume_step_up(0.9), 1.0));

        // Below unity, the arrows move in tenths.
        assert!(approx(volume_step_down(0.9), 0.8));
        assert!(approx(volume_step_up(0.5), 0.6));

        // The ladder ends saturate -- stepping past them returns the same value,
        // which the stepper reads as "cannot move further".
        assert_eq!(volume_step_up(64.0), volume_modifier_factor(0xC0));
        assert_eq!(volume_step_down(0.25), volume_modifier_factor(0xC1));
    }

    #[test]
    fn matched_volume_snaps_the_full_scale_boost_onto_the_ladder() {
        // A half-scale peak wants a clean 2x lift; full scale wants none.
        assert!((matched_volume(0x4000) - 2.0).abs() < 1e-3, "half -> 2x");
        assert!(
            (matched_volume(0x7FFF) - 1.0).abs() < 1e-3,
            "full scale -> unity"
        );
        // Whatever the peak, the result is a real ladder value in the gain range.
        for peak in [0x0100i32, 0x0800, 0x1234, 0x4000, 0x7FFF] {
            let volume = matched_volume(peak as i16);
            assert_eq!(
                volume,
                volume_modifier_factor(nearest_volume_modifier(volume)),
                "peak {peak:#06X} sits on the ladder"
            );
            assert!(
                (1.0..=64.0).contains(&volume),
                "peak {peak:#06X} in gain range"
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
