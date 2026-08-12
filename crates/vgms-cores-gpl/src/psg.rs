//! Nuked-PSG as a [`ChipCore`]: the SN76489 as Sega's VDPs integrate it.
//!
//! The picker's alternative to `vgms-synth`'s clean-room SN76489, not its
//! replacement: Nuke.YKT's die-traced model of the PSG inside the Sega VDPs,
//! with that part's own noise tap and measured volume ladder. It gives up
//! generality -- the header's feedback/shift-width fields that let the
//! clean-room core play a BBC Micro's or Tandy's variant mean nothing to a die
//! trace of one chip -- so [`configure`](ChipCore::configure) is the no-op
//! default, and a non-Sega rip plays with Sega noise.
//!
//! The Game Gear's stereo port *is* modelled, on top of the trace rather than
//! inside it: the mask never touches the PSG bus (address 1 is its own port,
//! as the reference's `SN76496_W_GGST` has it), and each output side sums the
//! channels its mask nibble enables, through the die's own mixer-side mute.
//!
//! # Determinism at the DAC
//!
//! Upstream sums its DAC in `float`, which is fine by [`ChipCore`]'s
//! identical-everywhere rule only because the build forbids floating-point
//! contraction (`build.rs`), leaving plain IEEE adds and multiplies that agree
//! bit-for-bit across x86-64 and wasm32. The scale to integer happens once per
//! sample, here, not in C.

use std::collections::VecDeque;
use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::PsgChip;

/// The registry id.
pub(crate) const CORE_ID: &str = "sn76489.nuked-psg";

/// Internal clocks per output sample, as upstream's own `YMPSG_Generate`
/// paces it -- the familiar `clock / 16`.
const CLOCKS_PER_SAMPLE: u32 = 16;

/// Internal clocks between queued command bytes.
///
/// The chip consumes a written byte on its next clock, but two bytes inside
/// the same clock would lose the first -- the real bus stalls the CPU for
/// roughly a rotation instead. One sample's worth is that stall, near enough,
/// and VGM command streams are far sparser than it.
const SETTLE: u32 = CLOCKS_PER_SAMPLE;

/// Scales the unipolar float DAC sum (one channel at full volume is `1.0`)
/// to the mixer's range.
///
/// 4096 puts one full-volume channel near the clean-room core's calibrated
/// `PEAK = 4000`, so swapping cores in the picker changes the noise texture,
/// not the volume.
const OUTPUT_SCALE: f32 = 4096.0;

/// The SN76489 (Sega VDP flavour), Nuke.YKT's die-traced emulation of it.
#[derive(Debug)]
pub struct Sn76489Nuked {
    chip: PsgChip,
    rate: u32,
    /// Command bytes waiting for the bus, and the clocks left until the next
    /// may be presented.
    writes: VecDeque<u8>,
    settle: u32,
    /// The Game Gear stereo register: bits 0-3 enable channels 0-3 on the
    /// right, bits 4-7 on the left. `0xFF` -- everything both sides -- is the
    /// power-on state and what a non-Game-Gear file never changes.
    ///
    /// It is the Game Gear's own port, not a PSG register: the mask byte must
    /// never reach the PSG bus, where its bit pattern would decode as a latch
    /// (the frequent `0xFF` mask latches the noise channel silent).
    stereo_mask: u8,
}

impl Sn76489Nuked {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: PsgChip::new(),
            rate: 44_100,
            writes: VecDeque::new(),
            settle: 0,
            stereo_mask: 0xFF,
        }
    }
}

impl Default for Sn76489Nuked {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Sn76489Nuked {
    /// `variant` (the T6W28 flag) is not modelled, as in the clean-room core.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.settle = 0;
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);
        self.stereo_mask = 0xFF;
        self.chip.reset();
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// Address 0 is the PSG bus (latch/data telling themselves apart by bit 7,
    /// inside the chip). Address 1 is the Game Gear stereo port (`0x4F`), which
    /// only moves the mask -- exactly the reference's dedicated `SN76496_W_GGST`
    /// entry point, never the data bus.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        if addr == 1 {
            self.stereo_mask = (data & 0xFF) as u8;
            return;
        }
        self.writes.push_back((data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            for _ in 0..CLOCKS_PER_SAMPLE {
                if self.settle > 0 {
                    self.settle -= 1;
                } else if let Some(byte) = self.writes.pop_front() {
                    self.chip.write(byte);
                    self.settle = SETTLE;
                }
                self.chip.clock();
            }
            // Each side hears the channels its mask nibble enables. The DAC
            // sum is idempotent within a clock, so the two reads are two views
            // of one instant; with the power-on mask both sides are the full
            // mono sum, exactly the single-pin behaviour this had before.
            // Truncation rather than rounding, matching upstream's own cast.
            self.chip.set_mute(!(self.stereo_mask >> 4) & 0x0F);
            frame[0] = (self.chip.output() * OUTPUT_SCALE) as i32;
            self.chip.set_mute(!self.stereo_mask & 0x0F);
            frame[1] = (self.chip.output() * OUTPUT_SCALE) as i32;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Sn76489];

#[cfg(test)]
mod tests {
    use super::*;

    /// The NTSC colour burst, as the SMS and Mega Drive divide it down.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Sn76489Nuked, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    #[test]
    fn a_fresh_chip_is_quiet_and_a_keyed_on_one_is_not() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        // Power-on: silence all four channels first, as every BIOS does.
        for byte in [0x9F, 0xBF, 0xDF, 0xFF] {
            chip.write(0, 0, byte);
        }
        let quiet = energy(&render(&mut chip, 4000));

        chip.write(0, 0, 0x80 | 0x06); // channel 0: period low nibble
        chip.write(0, 0, 0x08); //         period high bits: period 0x86
        chip.write(0, 0, 0x90); //         volume 0 (loudest)
        let loud = energy(&render(&mut chip, 8000));
        assert!(
            loud > quiet * 8,
            "the C core linked, reset, latched its writes and generated -- or \
             it did not: loud={loud} quiet={quiet}"
        );
    }

    /// The rate the engine resamples from: `clock / 16`, same as the
    /// clean-room core -- the two must be interchangeable in the picker.
    #[test]
    fn the_native_rate_is_the_clock_over_sixteen() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 16);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// A burst of writes at one instant must all arrive: the settle pacing
    /// delays them, it must not drop them.
    #[test]
    fn a_burst_of_writes_all_reach_the_chip() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        // Three channels keyed on in one burst -- six bytes back to back.
        for byte in [0x86, 0x08, 0x90, 0xA9, 0x0A, 0xB0] {
            chip.write(0, 0, byte);
        }
        render(&mut chip, 200);
        assert!(chip.writes.is_empty(), "every byte must have been consumed");
        let sustained = render(&mut chip, 4000);
        assert!(energy(&sustained) > 0, "and the notes must be sounding");
    }

    /// One full-volume channel peaks near the clean-room core's calibrated
    /// 4000 -- swapping cores in the picker changes the texture, not the
    /// volume.
    #[test]
    fn the_level_matches_the_clean_room_core() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        for byte in [0x9F, 0xBF, 0xDF, 0xFF] {
            chip.write(0, 0, byte);
        }
        render(&mut chip, 100);
        chip.write(0, 0, 0x80 | 0x06);
        chip.write(0, 0, 0x08);
        chip.write(0, 0, 0x90);
        let peak = render(&mut chip, 8000)
            .iter()
            .map(|&s| s.abs())
            .max()
            .unwrap_or(0);
        assert!(
            (3600..=4600).contains(&peak),
            "one channel at full volume peaked at {peak}, want ~4096"
        );
    }

    /// The `0x4F` mask must never reach the PSG bus: the frequent all-on mask
    /// `0xFF` would decode there as "latch channel 3 volume to silence".
    #[test]
    fn the_stereo_port_never_touches_the_psg_bus() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        chip.write(0, 1, 0xFF); // the Game Gear stereo port
        assert!(
            chip.writes.is_empty(),
            "the mask is not a bus byte and must not be queued as one"
        );
        assert_eq!(chip.stereo_mask, 0xFF);
    }

    /// The mask nibbles gate each side: left nibble clear silences the left,
    /// right nibble clear the right, and the power-on mask is plain mono.
    #[test]
    fn the_stereo_mask_pans_channels() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        for byte in [0x9F, 0xBF, 0xDF, 0xFF] {
            chip.write(0, 0, byte);
        }
        chip.write(0, 0, 0x80 | 0x06);
        chip.write(0, 0, 0x08);
        chip.write(0, 0, 0x90); // channel 0, full volume
        let mut out = vec![0i32; 8000 * 2];
        chip.render(&mut out);
        let (left, right): (Vec<i32>, Vec<i32>) = out
            .chunks_exact(2)
            .map(|frame| (frame[0], frame[1]))
            .unzip();
        assert_eq!(left, right, "the default mask is mono");
        assert!(energy(&left) > 0);

        chip.write(0, 1, 0x0F); // right nibble only: the left side goes dark
        let mut out = vec![0i32; 4000 * 2];
        chip.render(&mut out);
        let (left, right): (Vec<i32>, Vec<i32>) = out
            .chunks_exact(2)
            .map(|frame| (frame[0], frame[1]))
            .unzip();
        // The DAC sum is unipolar, so a fully muted side sits at a constant
        // floor: flat, while the playing side keeps its tone.
        let swing = |side: &[i32]| {
            side.iter().max().copied().unwrap_or(0) - side.iter().min().copied().unwrap_or(0)
        };
        assert_eq!(swing(&left), 0, "every channel is masked off the left");
        assert!(swing(&right) > 0, "the right still carries the tone");

        chip.write(0, 1, 0xF0); // and the mirror image
        let mut out = vec![0i32; 4000 * 2];
        chip.render(&mut out);
        let (left, right): (Vec<i32>, Vec<i32>) = out
            .chunks_exact(2)
            .map(|frame| (frame[0], frame[1]))
            .unzip();
        assert!(swing(&left) > 0);
        assert_eq!(swing(&right), 0);
    }

    /// A reset must silence a playing chip and forget its queue.
    #[test]
    fn a_reset_silences_and_forgets() {
        let mut chip = Sn76489Nuked::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0, 0x80 | 0x06);
        chip.write(0, 0, 0x08);
        chip.write(0, 0, 0x90);
        render(&mut chip, 2000);

        chip.write(0, 0, 0x93); // left in the queue on purpose
        chip.write(0, 1, 0x0F); // and a stereo image
        chip.reset(CLOCK, false);
        assert!(chip.writes.is_empty(), "a reset must clear the queue");
        assert_eq!(chip.stereo_mask, 0xFF, "and restore the mono power-on mask");
    }
}
