//! The YM2612 die as a [`ChipCore`], clocked pin by pin.
//!
//! The second chip on the LLE bench, and the one with an open question
//! waiting for it: the shipping Nuked-OPN2 core scores 0.904 against the
//! reference player -- a driver-level difference nobody has isolated -- and
//! a die on the bench turns that question mechanical. `fmopna_2612` is one
//! of three chips the YM2608-LLE decap builds (the OPNA implementation
//! compiled under a per-chip macro); this wrapper drives the 2612 die only.
//!
//! Easier to listen to than the OPM die, as it happens: no serial DAC. The
//! 2612's nine-bit ladder DAC time-multiplexes the six channels on two
//! parallel pins, and the die computes the ladder's asymmetry itself -- an
//! unpanned channel slot emits the famous `+-1` sign residue, which is the
//! ladder distortion every Mega Drive recording carries. Summing the pin
//! over a sample period is exactly the mixdown the real console's analog
//! path performs, and exactly what Nuked-OPN2's wrapper does with its
//! per-cycle outputs -- so the two cores are compared on the same terms.
//!
//! `realtime: false`, like every die: render and oracle only.

use dro_core::vgm::ChipKind;
use dro_synth::ChipCore;
use std::collections::VecDeque;

use crate::ffi::{Opn2LleChip, Opn2Pins};

/// The registry id.
pub(crate) const CORE_ID: &str = "ym2612.lle";

/// Master clocks per output sample: 24 internal slots at clock/6.
const CLOCKS_PER_SAMPLE: u32 = 144;

/// Master clocks the bus signals are held asserted for one byte.
const WRITE_HOLD: u32 = 8;

/// Master clocks of bus silence after an address byte -- Nuked-OPN2's
/// wrapper takes the value immediately after the address, so the die gets
/// the same cadence.
const ADDRESS_RECOVER: u32 = 4;

/// Master clocks of bus silence after a value byte. Address + value + both
/// recoveries is one sample period, which is Nuked-OPN2's pacing (its
/// `VALUE_SETTLE` is the rest of the chip's 24-cycle turn); matching it
/// keeps write bursts landing on the same samples in both cores -- the
/// lesson the OPM bench paid 0.2-0.4 of correlation to learn.
const VALUE_RECOVER: u32 = CLOCKS_PER_SAMPLE - (2 * WRITE_HOLD) - ADDRESS_RECOVER;

/// Master clocks with `IC` held low at reset.
const RESET_HOLD: u32 = 288;

/// One queued byte for the bus.
#[derive(Debug, Clone, Copy)]
struct BusByte {
    /// Bank select: part I or part II registers.
    a1: bool,
    /// Address or value.
    a0: bool,
    data: u8,
}

/// Where the bus state machine is in delivering a byte.
#[derive(Debug, Clone, Copy)]
enum Bus {
    Idle,
    Holding(u32),
    Recovering(u32),
}

/// The YM2612, as its own die computes it.
#[derive(Debug)]
pub struct Ym2612Lle {
    chip: Opn2LleChip,
    rate: u32,
    writes: VecDeque<BusByte>,
    bus: Bus,
}

impl Ym2612Lle {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: Opn2LleChip::new(),
            rate: 53_267,
            writes: VecDeque::new(),
            bus: Bus::Idle,
        }
    }

    /// One master clock: the bus state machine and both clock edges.
    fn master_clock(&mut self) {
        match self.bus {
            Bus::Idle => {
                if let Some(byte) = self.writes.pop_front() {
                    self.chip.set_pins(Opn2Pins {
                        cs: false,
                        wr: false,
                        a0: byte.a0,
                        a1: byte.a1,
                        data: byte.data,
                        ..Opn2Pins::default()
                    });
                    self.bus = Bus::Holding(WRITE_HOLD | (u32::from(byte.a0) << 31));
                }
            }
            Bus::Holding(state) => {
                let left = state & !(1 << 31);
                let was_value = state & (1 << 31) != 0;
                self.bus = if left > 1 {
                    Bus::Holding((left - 1) | (u32::from(was_value) << 31))
                } else {
                    self.chip.set_pins(Opn2Pins::default());
                    Bus::Recovering(if was_value {
                        VALUE_RECOVER
                    } else {
                        ADDRESS_RECOVER
                    })
                };
            }
            Bus::Recovering(left) => {
                self.bus = if left > 1 {
                    Bus::Recovering(left - 1)
                } else {
                    Bus::Idle
                };
            }
        }
        self.chip.clock_edge(false);
        self.chip.clock_edge(true);
    }
}

impl Default for Ym2612Lle {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2612Lle {
    /// `variant` distinguishes the YM3438; this die is the YM2612 decap and
    /// renders both the same -- a stated approximation, as the shipping
    /// core's CMOS/ladder switch is exactly what the variant changes there.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.bus = Bus::Idle;
        self.rate = (clock / CLOCKS_PER_SAMPLE).max(1);

        self.chip.power_cycle();
        // The electrical reset: IC low while the clock runs, then released.
        self.chip.set_pins(Opn2Pins {
            ic: false,
            ..Opn2Pins::default()
        });
        for _ in 0..RESET_HOLD {
            self.chip.clock_edge(false);
            self.chip.clock_edge(true);
        }
        self.chip.set_pins(Opn2Pins::default());
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// `port` selects the register bank, exactly as the shipping core reads
    /// it; each write is an address byte then a value byte on the bus.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let a1 = port & 1 != 0;
        self.writes.push_back(BusByte {
            a1,
            a0: false,
            data: (addr & 0xFF) as u8,
        });
        self.writes.push_back(BusByte {
            a1,
            a0: true,
            data: (data & 0xFF) as u8,
        });
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.master_clock();
                let (l, r) = self.chip.dac_pins();
                left += l;
                right += r;
            }
            // The multiplexed pins change every *two* master clocks --
            // measured against Nuked-OPN2's per-cycle sum via the oracle's
            // level column (dividing by six read exactly 3.00x quiet), not
            // derived from the die -- so dividing by two makes the sum
            // Nuked-equivalent, and the shipping core's calibrated
            // OUTPUT_GAIN then applies unchanged.
            frame[0] = (left / 2) * 21;
            frame[1] = (right / 2) * 21;
        }
    }
}

/// The chip this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2612];

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mega Drive's YM2612 clock.
    const CLOCK: u32 = 7_670_453;

    fn render(chip: &mut Ym2612Lle, frames: usize) -> Vec<(i32, i32)> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| (f[0], f[1])).collect()
    }

    fn energy(samples: &[(i32, i32)]) -> i64 {
        samples
            .iter()
            .map(|&(l, r)| i64::from(l.abs()) + i64::from(r.abs()))
            .sum()
    }

    /// A loud FM note on channel 1, algorithm 7, both speakers -- the same
    /// patch the Nuked-OPN2 wrapper's tests play.
    fn key_on(chip: &mut Ym2612Lle) {
        for (reg, value) in [
            (0x22u16, 0x00u16), // LFO off
            (0x27, 0x00),       // normal timer mode
            (0x28, 0x00),       // all keys off
            (0x30, 0x01),       // ch1 op1 multiple 1
            (0x40, 0x00),       // op1 total level: loudest
            (0x50, 0x1F),       // attack instant
            (0x60, 0x00),
            (0x70, 0x00),
            (0x80, 0x0F), // release
            (0x90, 0x00),
            (0xB0, 0x07), // algorithm 7
            (0xB4, 0xC0), // both speakers
            (0xA4, 0x22), // block and F-number high
            (0xA0, 0x69), // F-number low
            (0x28, 0xF0), // key on ch1, all slots
        ] {
            chip.write(0, reg, value);
        }
    }

    /// The die must link, reset, take bank-0 writes and produce sound on the
    /// multiplexed DAC pins -- silence before, sound after.
    #[test]
    fn the_die_makes_sound_after_a_key_on() {
        let mut chip = Ym2612Lle::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 256));

        key_on(&mut chip);
        render(&mut chip, 128); // let the bus land the burst
        let loud = energy(&render(&mut chip, 1024));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "pin-level write, die, DAC mixdown -- one of them failed: \
             loud={loud} quiet={quiet}"
        );
    }

    /// `clock / 144`, the same rate the shipping core declares.
    #[test]
    fn the_native_rate_matches_the_shipping_core() {
        let mut chip = Ym2612Lle::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 144);
        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }
}
