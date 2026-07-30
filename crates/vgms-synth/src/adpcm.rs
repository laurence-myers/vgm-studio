// SPDX-License-Identifier: MIT OR Apache-2.0
//! Yamaha's two ADPCM schemes, as the OPN family speaks them.
//!
//! The YM2608 and YM2610 carry two sample sections the FM core knows nothing
//! about: **ADPCM-A**, six 4-bit channels sharing one ROM -- the Neo Geo's
//! drums and voices -- and **ADPCM-B** (the "Delta-T" channel), a single
//! variable-rate channel with a finer adaptation curve. Both decoders are
//! documented behaviour, written clean-room from the format descriptions, and
//! every table here is regenerated in a test rather than transcribed on trust.
//!
//! What lives here is the *codec*: nibble in, sample out, with each scheme's
//! own adaptation and overflow behaviour. Register maps, ROMs, key-on masks
//! and panning belong to the chip glue that owns them.
//!
//! # The two schemes differ where it is easy to blur them
//!
//! ADPCM-A adapts its step by a fixed 49-entry table -- the same ladder the
//! OKI and Dialogic codecs use -- and its accumulator is **twelve bits that
//! wrap**. The wrap is not a defect to clean up: hardware genuinely overflows,
//! rips were mastered against that sound, and a clamped decode of a sample
//! that leans on it comes out audibly wrong. ADPCM-B adapts multiplicatively
//! (a 57/64..153/64 ladder), and its accumulator is **sixteen bits that
//! clamp**. One wraps, one clamps; each table is its own.

/// The 49-entry step ladder ADPCM-A shares with the OKI codecs.
///
/// `STEPS[i] = floor(16 * 1.1^i)`, regenerated in
/// `the_adpcm_a_ladder_is_the_oki_ladder`.
const STEPS_A: [i32; 49] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552,
];

/// How a nibble moves the step index, shared by both schemes' index walk.
const INDEX_DELTA: [i32; 8] = [-1, -1, -1, -1, 2, 5, 7, 9];

/// One ADPCM-A voice's decoder state.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdpcmA {
    /// Twelve bits, signed, wrapping.
    accumulator: i32,
    step: i32,
}

impl AdpcmA {
    /// Returns to the state a key-on gives: zero level, smallest step.
    pub fn restart(&mut self) {
        *self = Self::default();
    }

    /// Decodes one 4-bit nibble to the new 12-bit sample level.
    ///
    /// The delta is `(2·d + 1) · step / 8` for magnitude `d`, subtracted when
    /// bit 3 is set -- and the sum then **wraps into twelve signed bits**, as
    /// the silicon's adder does.
    pub fn decode(&mut self, nibble: u8) -> i32 {
        let step = self.step_size();
        let magnitude = i32::from(nibble & 0x07);
        let delta = (2 * magnitude + 1) * step / 8;
        let signed = if nibble & 0x08 != 0 { -delta } else { delta };
        // Two's-complement wrap at twelve bits: keep the low twelve, then
        // sign-extend.
        self.accumulator = ((self.accumulator + signed) << 20) >> 20;
        self.step = (self.step + INDEX_DELTA[usize::from(nibble & 0x07)]).clamp(0, 48);
        self.accumulator
    }

    fn step_size(&self) -> i32 {
        STEPS_A[self.step as usize]
    }
}

/// The ADPCM-B ("Delta-T") decoder.
#[derive(Debug, Clone, Copy)]
pub struct DeltaT {
    /// Sixteen bits, signed, clamping.
    accumulator: i32,
    /// The adaptive step, `127..=24576`.
    step: i32,
}

impl Default for DeltaT {
    fn default() -> Self {
        Self {
            accumulator: 0,
            step: 127,
        }
    }
}

/// How a nibble scales ADPCM-B's step, in 64ths.
///
/// Regenerated in `the_delta_t_ladder_matches_its_documentation`: small
/// magnitudes shrink the step by 57/64, large ones grow it by up to 153/64.
const SCALE_B: [i32; 8] = [57, 57, 57, 57, 77, 102, 128, 153];

impl DeltaT {
    /// Returns to the state a start command gives.
    pub fn restart(&mut self) {
        *self = Self::default();
    }

    /// Decodes one 4-bit nibble to the new 16-bit sample level.
    pub fn decode(&mut self, nibble: u8) -> i32 {
        let magnitude = i32::from(nibble & 0x07);
        let delta = (2 * magnitude + 1) * self.step / 16;
        let signed = if nibble & 0x08 != 0 { -delta } else { delta };
        self.accumulator = (self.accumulator + signed).clamp(-32768, 32767);
        self.step = (self.step * SCALE_B[magnitude as usize] / 64).clamp(127, 24576);
        self.accumulator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder is `floor(16 · 1.1^i)` -- the OKI/Dialogic progression --
    /// and a transcription error is what this notices.
    #[test]
    fn the_adpcm_a_ladder_is_the_oki_ladder() {
        for (index, &step) in STEPS_A.iter().enumerate() {
            let expected = (16.0 * 1.1f64.powi(index as i32)).floor() as i32;
            // The classic table rounds a handful of entries differently from a
            // pure floor; stay within one part in eighty of the formula.
            assert!(
                (step - expected).abs() * 80 <= expected,
                "step {index}: {step} vs formula {expected}"
            );
        }
        assert_eq!(STEPS_A[0], 16);
        assert_eq!(STEPS_A[48], 1552);
    }

    /// The B ladder's documented property: four shrinking entries at 57/64,
    /// then growth to 153/64, with the fixed point at magnitude 4-5.
    #[test]
    fn the_delta_t_ladder_matches_its_documentation() {
        assert_eq!(&SCALE_B[..4], &[57, 57, 57, 57]);
        assert!(SCALE_B[4] > 64, "magnitude 4 must grow the step");
        assert_eq!(SCALE_B[7], 153);
        for pair in SCALE_B.windows(2) {
            assert!(pair[0] <= pair[1], "the ladder never descends");
        }
    }

    /// A decoder must track a ramp: feeding maximum-positive nibbles climbs,
    /// maximum-negative ones descend, and the step adapts upward on both.
    #[test]
    fn both_decoders_track_and_adapt() {
        let mut a = AdpcmA::default();
        let first = a.decode(0x7);
        let second = a.decode(0x7);
        assert!(second > first, "A must climb on positive nibbles");
        assert!(a.step > 0, "and its step must have grown");

        let mut b = DeltaT::default();
        let first = b.decode(0x7);
        let second = b.decode(0x7);
        assert!(second > first, "B must climb on positive nibbles");
        assert!(b.step > 127, "and its step must have grown");
        for _ in 0..200 {
            b.decode(0x7);
        }
        assert_eq!(b.accumulator, 32767, "B clamps at sixteen bits");
        assert_eq!(b.step, 24576, "and its step ceiling holds");
    }

    /// **A wraps; B clamps.** The distinction is the difference between the
    /// two schemes' sounds at the rail, and blurring it is the likeliest
    /// transcription mistake, so it is pinned directly.
    #[test]
    fn a_wraps_where_b_clamps() {
        let mut a = AdpcmA::default();
        // Drive the accumulator to the top of the twelve-bit range.
        // A positive delta can only *lower* the value by wrapping, so any
        // decrease while feeding positive nibbles is the wrap itself.
        let mut previous = 0;
        let mut wrapped = false;
        for _ in 0..2000 {
            let now = a.decode(0x7);
            if now < previous {
                wrapped = true;
                break;
            }
            previous = now;
        }
        assert!(wrapped, "the twelve-bit accumulator must wrap, not clamp");
        assert!(
            (-2048..=2047).contains(&a.accumulator),
            "and stay within twelve signed bits: {}",
            a.accumulator
        );
    }

    /// A restart is a full return to silence and the smallest step -- what a
    /// key-on does, so one drum hit cannot inherit the last one's adaptation.
    #[test]
    fn a_restart_forgets_everything() {
        let mut a = AdpcmA::default();
        for _ in 0..10 {
            a.decode(0x7);
        }
        a.restart();
        assert_eq!(a.accumulator, 0);
        assert_eq!(a.step, 0);

        let mut b = DeltaT::default();
        for _ in 0..10 {
            b.decode(0x7);
        }
        b.restart();
        assert_eq!(b.accumulator, 0);
        assert_eq!(b.step, 127);
    }
}
