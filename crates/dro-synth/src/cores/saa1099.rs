// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Philips SAA1099: six squares, two noise generators, two envelopes --
//! the SAM Coupé's chip and a scattering of arcade boards. 116 corpus files.
//!
//! **Route B, from the Philips datasheet**, so it lives in the permissive
//! crate.
//!
//! Each tone is `(clock/512) x 2^octave / (511 - N)`; channels 0-2 share
//! noise generator 0 and channels 3-5 generator 1; the two envelope units
//! ride channels 2 and 5. The envelope's finer shapes (4-bit resolution
//! toggle, external clock) are simplified to the four families the corpus
//! uses: off, decay, sawtooth, triangle.

use crate::chip::ChipCore;

/// One frame per 256 clocks: 31.25 kHz at the usual 8 MHz.
const CLOCK_DIVIDER: u32 = 256;

/// One tone channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// Amplitude nibbles: left low, right high.
    amplitude: u8,
    /// The frequency byte `N`: period `511 - N`.
    frequency: u8,
    /// Three bits.
    octave: u8,
    tone_on: bool,
    noise_on: bool,
    /// Square phase, 16.16.
    phase: u32,
}

/// One envelope unit.
#[derive(Debug, Default, Clone, Copy)]
struct EnvelopeUnit {
    enabled: bool,
    /// Shape bits from the register.
    shape: u8,
    /// Position on a 16-step ramp.
    step: u8,
    count: u32,
}

impl EnvelopeUnit {
    /// The current 0-15 level.
    fn level(&self) -> u8 {
        if !self.enabled {
            return 15;
        }
        match (self.shape >> 1) & 0x07 {
            // 0-1: held at zero / maximum.
            0 => 0,
            1 => 15,
            // 2-3: single/repeating decay.
            2 | 3 => 15 - self.step.min(15),
            // 4-5: single/repeating triangle.
            4 | 5 => {
                if self.step < 16 {
                    self.step
                } else {
                    31 - self.step
                }
            }
            // 6-7: single/repeating sawtooth (attack).
            _ => self.step.min(15),
        }
    }

    fn advance(&mut self) {
        let repeating = matches!((self.shape >> 1) & 0x07, 3 | 5 | 7);
        let span: u8 = if matches!((self.shape >> 1) & 0x07, 4 | 5) {
            32
        } else {
            16
        };
        if self.step + 1 < span {
            self.step += 1;
        } else if repeating {
            self.step = 0;
        }
    }
}

/// The SAA1099.
#[derive(Debug)]
pub struct Saa1099 {
    rate: u32,
    channels: [Channel; 6],
    /// Noise generators: rate select and LFSR each.
    noise_rate: [u8; 2],
    noise: [u32; 2],
    noise_phase: [u32; 2],
    envelopes: [EnvelopeUnit; 2],
    /// The address latch: VGM sends (register, data) pairs directly, but
    /// the chip is addr/data and some rips write the latch explicitly.
    all_enabled: bool,
}

impl Saa1099 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 31_250,
            channels: [Channel::default(); 6],
            noise_rate: [0; 2],
            noise: [1; 2],
            noise_phase: [0; 2],
            envelopes: [EnvelopeUnit::default(); 2],
            all_enabled: false,
        }
    }
}

impl Default for Saa1099 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Saa1099 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            ..Self::new()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let data = (data & 0xFF) as u8;
        match addr & 0x1F {
            0x00..=0x05 => self.channels[usize::from(addr)].amplitude = data,
            0x08..=0x0D => self.channels[usize::from(addr - 8)].frequency = data,
            0x10..=0x12 => {
                let base = usize::from(addr - 0x10) * 2;
                self.channels[base].octave = data & 0x07;
                self.channels[base + 1].octave = (data >> 4) & 0x07;
            }
            0x14 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    ch.tone_on = data & (1 << index) != 0;
                }
            }
            0x15 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    ch.noise_on = data & (1 << index) != 0;
                }
            }
            0x16 => {
                self.noise_rate[0] = data & 0x03;
                self.noise_rate[1] = (data >> 4) & 0x03;
            }
            0x18 | 0x19 => {
                let env = &mut self.envelopes[usize::from(addr - 0x18)];
                env.enabled = data & 0x80 != 0;
                env.shape = data & 0x0F;
                env.step = 0;
            }
            // Control: bit 0 is the master enable; a reset-and-sync bit
            // (bit 1) restarts the generators.
            0x1C => {
                self.all_enabled = data & 0x01 != 0;
                if data & 0x02 != 0 {
                    for ch in &mut self.channels {
                        ch.phase = 0;
                    }
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            if !self.all_enabled {
                frame[0] = 0;
                frame[1] = 0;
                continue;
            }
            // The noise generators: rate 0-2 are fixed dividers, 3 follows
            // its group's channel 0 frequency.
            for unit in 0..2usize {
                let step = match self.noise_rate[unit] {
                    0 => 1 << 14,
                    1 => 1 << 13,
                    2 => 1 << 12,
                    _ => {
                        let ch = &self.channels[unit * 3];
                        ((u32::from(CLOCK_DIVIDER) << 16) / (511 - u32::from(ch.frequency)).max(1))
                            << ch.octave
                            >> 9
                    }
                };
                self.noise_phase[unit] = self.noise_phase[unit].wrapping_add(step);
                while self.noise_phase[unit] >= 1 << 16 {
                    self.noise_phase[unit] -= 1 << 16;
                    let lfsr = self.noise[unit];
                    let feedback = ((lfsr >> 10) ^ (lfsr >> 2)) & 1;
                    self.noise[unit] = ((lfsr << 1) | feedback) & 0x1FFFF;
                }
            }
            // Envelopes advance on channel 1/4 frequency in the real chip;
            // a fixed ~488 Hz walk is the simplification here.
            for env in &mut self.envelopes {
                env.count += 1;
                if env.count >= self.rate / 488 {
                    env.count = 0;
                    env.advance();
                }
            }

            let mut left = 0i32;
            let mut right = 0i32;
            for (index, ch) in self.channels.iter_mut().enumerate() {
                // The square advances regardless of the mixer.
                let step = ((u32::from(CLOCK_DIVIDER) << 16)
                    / (511 - u32::from(ch.frequency)).max(1))
                    << ch.octave
                    >> 9;
                ch.phase = ch.phase.wrapping_add(step);
                let square = ch.phase & (1 << 16) != 0;
                let noise = self.noise[index / 3] & 1 != 0;

                let high = match (ch.tone_on, ch.noise_on) {
                    (false, false) => continue,
                    (true, false) => square,
                    (false, true) => noise,
                    (true, true) => square ^ noise,
                };
                if !high {
                    continue;
                }
                let env_level = match index {
                    2 => self.envelopes[0].level(),
                    5 => self.envelopes[1].level(),
                    _ => 15,
                };
                // Amplitude nibbles x envelope, x32 to the usual headroom.
                left += i32::from(ch.amplitude & 0x0F) * i32::from(env_level) * 32 / 15;
                right += i32::from(ch.amplitude >> 4) * i32::from(env_level) * 32 / 15;
            }
            frame[0] = left * 16;
            frame[1] = right * 16;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual 8 MHz crystal.
    const CLOCK: u32 = 8_000_000;

    fn render(chip: &mut Saa1099, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut Saa1099) {
        chip.write(0, 0x1C, 0x01); // master enable
        chip.write(0, 0x00, 0xFF); // channel 0 amplitude
        chip.write(0, 0x08, 0x80); // frequency
        chip.write(0, 0x10, 0x04); // octave 4
        chip.write(0, 0x14, 0x01); // tone on
    }

    #[test]
    fn the_master_enable_gates_everything() {
        let mut chip = Saa1099::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
        chip.write(0, 0x1C, 0x00);
        assert_eq!(energy(&render(&mut chip, 500)), 0);
    }

    /// An octave doubles the pitch: one octave up counts twice the cycles.
    #[test]
    fn an_octave_doubles_the_rate() {
        let count = |octave: u8| {
            let mut chip = Saa1099::new();
            chip.reset(CLOCK, false);
            key_on(&mut chip);
            chip.write(0, 0x10, u16::from(octave));
            let samples = render(&mut chip, 4000);
            samples
                .windows(2)
                .filter(|pair| (pair[0].0 > 0) != (pair[1].0 > 0))
                .count()
        };
        let low = count(3);
        let high = count(4);
        assert!(
            (high as f64 / low as f64 - 2.0).abs() < 0.2,
            "octave 4 vs 3: {high} vs {low} crossings"
        );
    }

    /// The noise mixer sounds without any tone.
    #[test]
    fn noise_alone_sounds() {
        let mut chip = Saa1099::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x1C, 0x01);
        chip.write(0, 0x00, 0xFF);
        chip.write(0, 0x16, 0x00); // fastest noise
        chip.write(0, 0x15, 0x01); // noise on channel 0
        assert!(energy(&render(&mut chip, 500)) > 0);
    }

    /// The envelope on channel 2 decays it to silence in decay mode.
    #[test]
    fn the_envelope_rides_channel_two() {
        let mut chip = Saa1099::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x1C, 0x01);
        chip.write(0, 0x02, 0xFF); // channel 2 amplitude
        chip.write(0, 0x0A, 0x80);
        chip.write(0, 0x11, 0x04);
        chip.write(0, 0x14, 0x04); // tone on channel 2
        chip.write(0, 0x18, 0x84); // envelope: enabled, single decay
        let early = energy(&render(&mut chip, 200));
        render(&mut chip, 4000);
        let late = energy(&render(&mut chip, 200));
        assert!(early > 0);
        assert_eq!(late, 0, "a decayed envelope must be silent");
    }
}
