//! Nuked-OPLL as a [`ChipCore`]: the YM2413, the cheap FM chip that put a
//! synthesiser in a Master System, an MSX-MUSIC cartridge and a fruit machine.
//!
//! # What makes it odd
//!
//! Nine channels of two-operator FM, but only one is programmable. The other
//! eight play from a ROM of fifteen fixed instruments, which is why YM2413
//! music has a recognisable palette: everyone had the same violin. In rhythm
//! mode the last three channels become five percussion voices instead.
//!
//! The chip also has two DACs -- melody and rhythm -- multiplexed across its
//! 18-cycle rotation, so one output sample is that whole rotation of both
//! summed. Same shape as the OPN2's pins.
//!
//! Konami's VRC VII (`ds1001`) is a variant with its own instrument ROM, which
//! a VGM signals with bit 31 of the clock; this passes the flag through.

use vgms_core::vgm::ChipKind;
use vgms_synth::{ChipCore, WriteQueue};

use crate::ffi::OpllChip;

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2413.nuked";

/// Internal cycles per output sample: the chip rotates through 18 slots.
const CLOCKS_PER_SAMPLE: u32 = 18;
/// Master clocks per internal cycle, so the familiar `clock / 72`.
const MASTER_PER_CLOCK: u32 = 4;
const MASTER_PER_SAMPLE: u32 = MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE;

/// The write pacing: a whole rotation on each side of the address/value pair.
const SETTLE: u32 = CLOCKS_PER_SAMPLE;

/// Scales the summed DAC outputs towards `i16` range.
///
/// Nine channels through a nine-bit DAC leave the raw sum well below full
/// scale; this balances against the other cores, pinned by
/// `a_loud_chord_uses_the_range_without_clipping_it`.
const OUTPUT_GAIN: i32 = 12;

/// The YM2413 (OPLL), Nuke.YKT's emulation of it.
///
/// The raw rotation sum goes out unfiltered, standing DC offset and all --
/// exactly as the reference plays this core. A one-pole DC blocker used to sit
/// here (the chip's two DACs do not idle at zero, so a note-on steps the
/// offset audibly), but it moved sub-bass phase on every render and cost real
/// parity: removing it took the YM2413's reference correlation from 0.9767 to
/// 0.9896 (n=12, 2026-08-12). If the note-on step ever needs taming again, it
/// has to happen after the mix, not per chip.
#[derive(Debug)]
pub struct Ym2413 {
    chip: OpllChip,
    rate: u32,
    writes: WriteQueue,
    /// A mirror of the chip's internal 18-cycle counter, which upstream keeps
    /// private: zeroed by reset, advanced by every `NOPLL_Clock`, untouched by
    /// writes. What [`render`](ChipCore::render)'s mute gate keys on.
    cycles: u32,
    /// The channel-mute mask, [`vgms_core::vgm::channels_of`] order: bits 0-8
    /// the FM channels, 9-13 the rhythm voices (BD, SD, TT, CY, HH).
    mute_mask: u32,
}

impl Ym2413 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: OpllChip::new(),
            rate: 44_100,
            writes: WriteQueue::new(SETTLE, SETTLE),
            cycles: 0,
            mute_mask: 0,
        }
    }

    /// Whether anything is still waiting to be latched.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.writes.pending()
    }
}

impl Default for Ym2413 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2413 {
    /// `variant` is the header's bit 31: Konami's VRC VII, which has its own
    /// instrument ROM and permanent rhythm mode.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.writes.clear();
        self.rate = (clock / MASTER_PER_SAMPLE).max(1);
        self.chip.reset(variant);
        // The chip's internal counter restarts with it; the mirror follows.
        self.cycles = 0;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One register port: the address, then its value.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        self.writes
            .push(0, (addr & 0xFF) as u8, (data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sum = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                let chip = &mut self.chip;
                self.writes.advance(|port, byte| chip.write(port, byte));
                // Whose voices are on the two DACs this cycle, read *before*
                // the clock advances the rotation.
                let (melody_bit, rhythm_bit) = mute_bits_for_cycle(self.cycles);
                let (melody, rhythm) = self.chip.clock();
                self.cycles = (self.cycles + 1) % CLOCKS_PER_SAMPLE;
                // Two DACs, multiplexed across the rotation: the sample is that
                // whole rotation of both, minus the cycles of muted voices.
                if melody_bit.is_none_or(|bit| self.mute_mask & (1 << bit) == 0) {
                    sum += melody;
                }
                if rhythm_bit.is_none_or(|bit| self.mute_mask & (1 << bit) == 0) {
                    sum += rhythm;
                }
            }
            // The raw rotation sum, unfiltered, as the reference outputs this
            // core -- the type-level doc records why there is no DC blocker.
            let sample = sum * OUTPUT_GAIN;
            // Mono: the chip has one output pin.
            frame[0] = sample;
            frame[1] = sample;
        }
    }

    /// Which voices to leave out of the sum: bit `i` is entry `i` of the
    /// canonical order -- FM 1-9, then BD, SD, TT, CY, HH. Gated in
    /// [`render`], since the chip itself has no mute (two time-multiplexed
    /// DACs, so muting is choosing which cycles to add).
    fn set_channel_mutes(&mut self, muted: u32) {
        self.mute_mask = muted;
    }
}

/// The mask bits for the voices on the melody and rhythm DACs during internal
/// cycle `cycle` -- upstream's time-multiplex order, transcribed from the mute
/// gating in libvgm's `nukedopll_update`, whose copy of this same core is
/// where muting it from the outside comes from. `None` when that DAC carries
/// nothing gateable this cycle.
const fn mute_bits_for_cycle(cycle: u32) -> (Option<u32>, Option<u32>) {
    match cycle {
        1 => (Some(6), Some(9)),
        2 => (Some(7), Some(10)),
        3 => (Some(8), Some(12)),
        4 => (None, Some(13)),
        5 => (None, Some(11)),
        6 => (None, Some(9)),
        7 => (Some(0), None),
        8 => (Some(1), None),
        9 => (Some(2), None),
        10 => (None, Some(10)),
        11 => (None, Some(12)),
        12 | 16 => (None, None),
        13 => (Some(3), None),
        14 => (Some(4), None),
        15 => (Some(5), None),
        17 => (None, Some(13)),
        // Cycle 0, and anything unexpected: upstream's `default` arm.
        _ => (None, Some(11)),
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2413];

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual YM2413 clock, 3.579545 MHz.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Ym2413, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    /// Energy about the mean. The output carries the chip's standing DC
    /// offset by design (no DC blocker -- see the type-level doc), so a quiet
    /// chip sits at a constant nonzero level; what a note adds is *swing*.
    fn energy(samples: &[i32]) -> i64 {
        let mean = samples.iter().map(|&s| i64::from(s)).sum::<i64>() / samples.len().max(1) as i64;
        samples.iter().map(|&s| (i64::from(s) - mean).abs()).sum()
    }

    /// A note on channel 1 using instrument 1 from the ROM, at full volume.
    ///
    /// Only channel 1's *patch* is programmable; every other channel picks one
    /// of the fifteen fixed instruments, which is the chip's whole character.
    fn key_on(chip: &mut Ym2413) {
        chip.write(0, 0x10, 0x40); // channel 1: F-number low
        chip.write(0, 0x30, 0x10); // instrument 1, volume 0 (loudest)
        chip.write(0, 0x20, 0x1D); // block 3, key on, F-number high
    }

    #[test]
    fn a_fresh_chip_is_quiet_and_a_keyed_on_one_is_not() {
        let mut chip = Ym2413::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 4000));

        key_on(&mut chip);
        let loud = energy(&render(&mut chip, 8000));
        assert!(
            loud > quiet * 8,
            "the C core linked, reset, latched its writes and generated -- or it \
             did not: loud={loud} quiet={quiet}"
        );
    }

    /// Muting the playing channel silences it; muting a different one does
    /// not. The gate lives in the binding's render loop (the chip itself has
    /// no mute), keyed on a mirrored cycle counter -- so this also pins that
    /// the mirror stays in phase with the chip's own rotation.
    #[test]
    fn muting_gates_the_playing_channels_cycles() {
        let energy_with = |mask: u32| -> i64 {
            let mut chip = Ym2413::new();
            chip.reset(CLOCK, false);
            chip.set_channel_mutes(mask);
            key_on(&mut chip);
            energy(&render(&mut chip, 8000))
        };
        let loud = energy_with(0);
        let muted = energy_with(0b1); // FM 1, the one key_on plays
        let other = energy_with(0b10_0000); // FM 6, which is idle
        assert!(
            muted * 8 < loud,
            "muting FM 1 must silence the note (loud {loud}, muted {muted})"
        );
        assert!(
            other * 2 > loud,
            "muting an idle channel must not touch the note (loud {loud}, other {other})"
        );
    }

    /// The rate the engine resamples from: `clock / 72`, which for the usual
    /// crystal is the familiar 49716 Hz the OPL family also runs at.
    #[test]
    fn the_native_rate_is_the_clock_over_seventy_two() {
        let mut chip = Ym2413::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 72);
        // 49715 rather than the OPL family's familiar 49716: the same crystal,
        // but integer division of 3579545 by 72 loses the fraction.
        assert_eq!(chip.native_rate(), 49_715);

        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// The write queue's reason for existing, checked here as for every other
    /// core in the family: a burst of registers at one instant must all arrive.
    #[test]
    fn a_burst_of_writes_all_reach_the_chip() {
        let mut chip = Ym2413::new();
        chip.reset(CLOCK, false);
        // The whole custom-instrument block plus a note: eleven registers at one
        // timestamp, which is what a driver writes when it changes patch.
        for (register, value) in [
            (0x00u16, 0x21u16),
            (0x01, 0x11),
            (0x02, 0x1B),
            (0x03, 0x00),
            (0x04, 0xF8),
            (0x05, 0xF8),
            (0x06, 0x00),
            (0x07, 0x00),
            (0x10, 0x40),
            (0x30, 0x00), // the custom instrument, volume 0
        ] {
            chip.write(0, register, value);
        }
        chip.write(0, 0x20, 0x1D);
        assert!(
            energy(&render(&mut chip, 8000)) > 0,
            "a burst of writes produced silence: the queue is dropping them"
        );
    }

    /// A reset must discard the queue as well as the chip -- otherwise a seek
    /// delivers registers the song wrote before it.
    #[test]
    fn a_reset_clears_the_chip_and_its_pending_writes() {
        let mut chip = Ym2413::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        assert!(chip.pending() > 0, "writes are pending");

        chip.reset(CLOCK, false);
        assert_eq!(chip.pending(), 0, "a seek must not carry writes across");
    }

    /// Konami's VRC VII has its own instrument ROM, so the same registers must
    /// not produce the same samples. The flag has to reach upstream rather than
    /// being decoration.
    #[test]
    fn the_vrc_seven_variant_reaches_the_chip() {
        fn render_variant(vrc7: bool) -> Vec<i32> {
            let mut chip = Ym2413::new();
            chip.reset(CLOCK, vrc7);
            key_on(&mut chip);
            render(&mut chip, 8000)
        }
        let ym2413 = render_variant(false);
        let vrc7 = render_variant(true);
        assert!(energy(&ym2413) > 0 && energy(&vrc7) > 0);
        assert_ne!(ym2413, vrc7, "the variant flag is not reaching OPLL_Reset");
    }

    /// Chunking must not change the audio.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let mut whole = Ym2413::new();
        whole.reset(CLOCK, false);
        key_on(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = Ym2413::new();
        chunked.reset(CLOCK, false);
        key_on(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// The output scale, pinned so a change to the gain is deliberate.
    #[test]
    fn a_loud_chord_uses_the_range_without_clipping_it() {
        let mut chip = Ym2413::new();
        chip.reset(CLOCK, false);
        for channel in 0..3u16 {
            chip.write(0, 0x10 + channel, 0x40);
            chip.write(0, 0x30 + channel, 0x10);
            chip.write(0, 0x20 + channel, 0x1D);
        }
        let loudest = render(&mut chip, 8000)
            .iter()
            .map(|&s| s.abs())
            .max()
            .unwrap_or(0);
        assert!(
            loudest > i32::from(i16::MAX) / 16,
            "a three-note chord peaked at {loudest}, far below the mixer's range"
        );
        assert!(
            loudest < i32::from(i16::MAX),
            "a three-note chord peaked at {loudest}, which the mixer would clamp"
        );
    }
}
