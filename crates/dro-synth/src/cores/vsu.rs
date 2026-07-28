// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Virtual Boy's VSU: five wavetable channels and a noise channel.
//! 353 corpus files.
//!
//! **Route B, from the documented behaviour** (Nintendo's Virtual Boy
//! developer documentation as the community preserves it; ares and MAME
//! record the same layout), so it lives in the permissive crate.
//!
//! The VGM stores VSU addresses divided by four (the console maps each
//! 6-bit wave sample and each register into its own 32-bit word): the five
//! 32-entry wavetables at `0x000`-`0x09F`, and the channel files at
//! `0x100 + channel * 0x10`. Channel 6 is the noise channel; channel 5's
//! sweep/modulation unit is **not modelled** (a stated approximation --
//! its games still sound, without the slide).

use crate::chip::ChipCore;

/// One frame per 120 clocks: ~41.7 kHz at the 5.0 MHz master.
const CLOCK_DIVIDER: u32 = 120;

/// One channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    enabled: bool,
    /// Volume nibbles: left high, right low.
    volume: u8,
    /// Eleven bits: the period is `2048 - freq`.
    frequency: u16,
    /// The envelope: current level, direction, interval, enable, repeat.
    env_level: u8,
    env_grow: bool,
    env_interval: u8,
    env_on: bool,
    env_repeat: bool,
    env_count: u32,
    /// Which of the five wavetables this channel reads.
    wave: u8,
    /// Wave phase, 16.16 over the 32 samples.
    phase: u32,
}

/// The VSU.
#[derive(Debug)]
pub struct Vsu {
    rate: u32,
    channels: [Channel; 6],
    /// Five 32-entry tables of 6-bit samples.
    waves: [[u8; 32]; 5],
    /// The noise LFSR.
    noise: u32,
}

impl Vsu {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 41_667,
            channels: [Channel::default(); 6],
            waves: [[0; 32]; 5],
            noise: 1,
        }
    }
}

impl Default for Vsu {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Vsu {
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
        match addr {
            // The five wavetables: 32 words apiece.
            0x0000..=0x009F => {
                self.waves[usize::from(addr >> 5)][usize::from(addr & 0x1F)] = data & 0x3F;
            }
            // The channel files.
            0x0100..=0x015F => {
                let ch = &mut self.channels[usize::from((addr >> 4) & 0x07).min(5)];
                match addr & 0x0F {
                    // INT: bit 7 enables. (The auto-shutoff interval is not
                    // modelled; rips key off explicitly.)
                    0x0 => ch.enabled = data & 0x80 != 0,
                    0x1 => ch.volume = data,
                    0x2 => ch.frequency = (ch.frequency & 0x0700) | u16::from(data),
                    0x3 => ch.frequency = (ch.frequency & 0x00FF) | (u16::from(data & 0x07) << 8),
                    // EV0: the envelope's initial level, direction, interval.
                    0x4 => {
                        ch.env_level = data >> 4;
                        ch.env_grow = data & 0x08 != 0;
                        ch.env_interval = data & 0x07;
                        ch.env_count = 0;
                    }
                    // EV1: enable and repeat. (Channel 5's modulation bits
                    // share this register and are not modelled.)
                    0x5 => {
                        ch.env_on = data & 0x01 != 0;
                        ch.env_repeat = data & 0x02 != 0;
                    }
                    // RAM: which wavetable.
                    0x6 => ch.wave = data & 0x07,
                    // 0x7 is the sweep/modulation register: not modelled.
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        // The envelope clock: one step per interval unit of ~15.4 ms.
        let env_unit = (self.rate / 65).max(1);
        for frame in out.chunks_exact_mut(2) {
            // One noise step per frame.
            let feedback = ((self.noise >> 7) ^ (self.noise >> 14)) & 1;
            self.noise = ((self.noise << 1) | feedback) & 0x7FFF;

            let mut left = 0i32;
            let mut right = 0i32;
            for (index, ch) in self.channels.iter_mut().enumerate() {
                if !ch.enabled {
                    continue;
                }
                // The envelope walks its level at its interval.
                if ch.env_on {
                    ch.env_count += 1;
                    if ch.env_count >= env_unit * (u32::from(ch.env_interval) + 1) {
                        ch.env_count = 0;
                        if ch.env_grow {
                            if ch.env_level < 15 {
                                ch.env_level += 1;
                            } else if ch.env_repeat {
                                ch.env_level = 0;
                            }
                        } else if ch.env_level > 0 {
                            ch.env_level -= 1;
                        } else if ch.env_repeat {
                            ch.env_level = 15;
                        }
                    }
                }

                let sample: i32 = if index == 5 {
                    if self.noise & 1 != 0 { 31 } else { -32 }
                } else {
                    let step =
                        (u32::from(CLOCK_DIVIDER) << 16) / (2048 - u32::from(ch.frequency)) / 32;
                    ch.phase = ch.phase.wrapping_add(step << 5);
                    let position = (ch.phase >> 16) as usize % 32;
                    // 6-bit unsigned samples, centred.
                    i32::from(self.waves[usize::from(ch.wave.min(4))][position]) - 32
                };
                let level = i32::from(ch.env_level);
                // Volume nibble x envelope level x8 lands one loud channel
                // near the usual ~8k headroom.
                left += sample * i32::from(ch.volume >> 4) * level / 2;
                right += sample * i32::from(ch.volume & 0x0F) * level / 2;
            }
            // x2 rather than the first draft's x8: the first corpus render
            // clipped the mixer.
            frame[0] = left * 2;
            frame[1] = right * 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Virtual Boy's 5 MHz sound clock.
    const CLOCK: u32 = 5_000_000;

    fn render(chip: &mut Vsu, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut Vsu) {
        for at in 0..32u16 {
            chip.write(0, at, if at < 16 { 0x3F } else { 0x00 }); // a square
        }
        chip.write(0, 0x0102, 0x00); // frequency
        chip.write(0, 0x0103, 0x07);
        chip.write(0, 0x0101, 0xFF); // both sides full
        chip.write(0, 0x0104, 0xF0); // envelope level 15, static
        chip.write(0, 0x0106, 0x00); // wavetable 0
        chip.write(0, 0x0100, 0x80); // enable
    }

    #[test]
    fn a_fresh_chip_is_silent_and_an_enabled_one_is_not() {
        let mut chip = Vsu::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
    }

    /// A decaying envelope fades the channel to silence and stays there
    /// without the repeat bit.
    #[test]
    fn the_envelope_decays_to_silence() {
        let mut chip = Vsu::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        chip.write(0, 0x0104, 0xF0); // level 15, decay, fastest interval
        chip.write(0, 0x0105, 0x01); // envelope on
        let early = energy(&render(&mut chip, 500));
        let second = chip.rate as usize;
        render(&mut chip, second); // a second of decay
        let late = energy(&render(&mut chip, 500));
        assert!(early > 0);
        assert_eq!(late, 0, "a decayed envelope must be silent");
    }

    /// The noise channel rattles both polarities.
    #[test]
    fn the_noise_channel_rattles() {
        let mut chip = Vsu::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x0151, 0xFF);
        chip.write(0, 0x0154, 0xF0);
        chip.write(0, 0x0150, 0x80);
        let samples = render(&mut chip, 500);
        assert!(samples.iter().any(|&(l, _)| l > 0));
        assert!(samples.iter().any(|&(l, _)| l < 0));
    }
}
