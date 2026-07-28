// SPDX-License-Identifier: MIT OR Apache-2.0
//! The 32X PWM: not a synthesizer at all, but a pulse-width DAC pair the
//! SH-2s stream samples into. 184 corpus files.
//!
//! **Route B, from the documented behaviour** (Sega's 32X hardware manual as
//! the community preserves it), so it lives in the permissive crate.
//!
//! The VGM's `0xB2 ad dd` carries a register nibble and a 12-bit value.
//! Register 1 sets the cycle (the pulse period, which is also the sample
//! rate); 2, 3 and 4 write left, right or both duty values. The output is
//! simply the duty against the cycle, centred -- the "synth" is the game
//! streaming PCM through it.
//!
//! One stated approximation: the engine fixes a core's rate at reset, so
//! this core renders at the standard `clock / 1042` (22 kHz at the 32X's
//! 23.01 MHz) and a rip that programs an unusual cycle plays at a slightly
//! shifted pitch rather than a shifted rate.

use crate::chip::ChipCore;

/// The standard 32X cycle: 1042 clocks a sample, 22.05 kHz.
const STANDARD_CYCLE: u32 = 1042;

/// The PWM.
#[derive(Debug)]
pub struct Pwm {
    rate: u32,
    /// The programmed cycle, for scaling duties.
    cycle: u32,
    left: u32,
    right: u32,
}

impl Pwm {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 22_050,
            cycle: STANDARD_CYCLE,
            left: STANDARD_CYCLE / 2,
            right: STANDARD_CYCLE / 2,
        }
    }
}

impl Default for Pwm {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Pwm {
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: (clock / STANDARD_CYCLE).max(1),
            ..Self::new()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// `addr` is the register nibble; `data` the 12-bit value.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let value = u32::from(data & 0x0FFF);
        match addr & 0x0F {
            0x01 => {
                self.cycle = value.max(1);
                // Recentre the idle level so a cycle change is not a pop.
                self.left = self.cycle / 2;
                self.right = self.cycle / 2;
            }
            0x02 => self.left = value,
            0x03 => self.right = value,
            0x04 => {
                self.left = value;
                self.right = value;
            }
            // 0x00 is the control register (timer/interrupt/dreq): none of
            // it changes what a duty value sounds like.
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let cycle = self.cycle.max(1) as i32;
            // Duty against cycle, centred, scaled to the usual ~8k
            // one-channel headroom.
            let centre = cycle / 2;
            frame[0] = ((self.left.min(self.cycle) as i32 - centre) * 16_000) / cycle;
            frame[1] = ((self.right.min(self.cycle) as i32 - centre) * 16_000) / cycle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 32X master clock.
    const CLOCK: u32 = 23_011_360;

    #[test]
    fn the_rate_is_the_standard_cycle() {
        let mut chip = Pwm::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), CLOCK / 1042);
    }

    /// A centred duty is silence; the extremes swing both ways; the `both`
    /// register writes the pair at once.
    #[test]
    fn duties_map_to_centred_samples() {
        let mut chip = Pwm::new();
        chip.reset(CLOCK, false);
        chip.write(0, 1, 1042);
        chip.write(0, 2, 521);
        chip.write(0, 3, 521);
        let mut out = [0i32; 2];
        chip.render(&mut out);
        assert_eq!(out, [0, 0], "the centre is silence");

        chip.write(0, 2, 1042); // full left duty
        chip.write(0, 3, 0);
        chip.render(&mut out);
        assert!(out[0] > 7000, "full duty swings positive: {}", out[0]);
        assert!(out[1] < -7000, "zero duty swings negative: {}", out[1]);

        chip.write(0, 4, 521);
        chip.render(&mut out);
        assert_eq!(out, [0, 0], "the both-register recentres the pair");
    }
}
