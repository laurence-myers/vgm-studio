// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bandai WonderSwan's sound unit: four wavetable channels with a voice
//! mode, a sweep, and a noise mode. 266 corpus files.
//!
//! **Route B, from the documented behaviour** (the community hardware
//! documentation -- WSMan/Sacred Tech's register descriptions -- as MAME's
//! `wswan.cpp` also records them), so it lives in the permissive crate.
//!
//! The wave memory is RAM the driver uploads: four 16-byte tables of 32
//! packed 4-bit samples, reached through `0xC6` memory writes (which the
//! stream decoder hands over with big-endian addresses, like the rest of
//! its range). Registers arrive through `0xBC` as `(register - 0x80)`, the
//! VGM's convention for this chip. Channel 2 can become an 8-bit voice DAC
//! and channel 4 a noise generator; the hardware sweep on channel 3 is
//! modelled, the HyperVoice headphone channel is not.

use crate::chip::ChipCore;

/// One frame per 128 clocks: 24 kHz at the 3.072 MHz master.
const CLOCK_DIVIDER: u32 = 128;

/// One channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// Eleven bits: the period is `2048 - pitch` ticks per wave step.
    pitch: u16,
    /// Volume nibbles: high left, low right.
    volume: u8,
    enabled: bool,
    /// Wave phase, 16.16 over the 32 samples.
    phase: u32,
}

/// The sound unit.
#[derive(Debug)]
pub struct WonderSwan {
    rate: u32,
    channels: [Channel; 4],
    /// The four 16-byte wavetables, as uploaded.
    wave_ram: [u8; 64],
    /// Which base the wave RAM sits at (register 0x8F, in 64-byte units).
    wave_base: u8,
    /// Channel 2 voice mode and its 8-bit sample.
    voice_mode: bool,
    voice_sample: u8,
    /// Channel 4 noise mode and its LFSR.
    noise_mode: bool,
    noise: u16,
    /// Sweep on channel 3.
    sweep_mode: bool,
    sweep_step: i8,
    sweep_time: u8,
    sweep_count: u32,
}

impl WonderSwan {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 24_000,
            channels: [Channel::default(); 4],
            wave_ram: [0; 64],
            wave_base: 0,
            voice_mode: false,
            voice_sample: 0x80,
            noise_mode: false,
            noise: 1,
            sweep_mode: false,
            sweep_step: 0,
            sweep_time: 0,
            sweep_count: 0,
        }
    }
}

impl Default for WonderSwan {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for WonderSwan {
    fn reset(&mut self, clock: u32, _variant: bool) {
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            ..Self::new()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// Port 0 carries the register file (`0xBC`, offset from `0x80`); port 1
    /// the wave-RAM memory writes (`0xC6`).
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let data = (data & 0xFF) as u8;
        if port == 1 {
            // A memory write lands in the wave RAM if it falls inside the
            // window the base register frames.
            let base = u16::from(self.wave_base) * 64;
            if (base..base + 64).contains(&addr) {
                self.wave_ram[usize::from(addr - base)] = data;
            }
            return;
        }
        // The register index, not the raw address: a stream write to
        // 0x208 is register 0x08, and indexing with the raw value was an
        // out-of-bounds panic the corpus harness caught on its first
        // full sweep.
        let reg = addr & 0xFF;
        match reg {
            // Pitch pairs.
            0x00 | 0x02 | 0x04 | 0x06 => {
                let ch = &mut self.channels[usize::from(reg >> 1) & 3];
                ch.pitch = (ch.pitch & 0x0700) | u16::from(data);
            }
            0x01 | 0x03 | 0x05 | 0x07 => {
                let ch = &mut self.channels[usize::from(reg >> 1) & 3];
                ch.pitch = (ch.pitch & 0x00FF) | (u16::from(data & 0x07) << 8);
            }
            // Volumes.
            0x08..=0x0B => self.channels[usize::from(reg - 0x08)].volume = data,
            // The sweep.
            0x0C => self.sweep_step = data as i8,
            0x0D => self.sweep_time = data,
            // Noise control: the low bits pick the tap, bit 4 enables.
            0x0E => {
                if data & 0x10 != 0 {
                    self.noise = 1;
                }
            }
            // Wave RAM base.
            0x0F => self.wave_base = data,
            // The channel control register.
            0x10 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    ch.enabled = data & (1 << index) != 0;
                }
                self.voice_mode = data & 0x20 != 0;
                self.sweep_mode = data & 0x40 != 0;
                self.noise_mode = data & 0x80 != 0;
            }
            // The voice DAC's sample byte.
            0x11 => self.voice_sample = data,
            // 0x12-0x13: output control (headphone/speaker mixing), unmodelled.
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            // The sweep, on its own slow clock.
            if self.sweep_mode && self.sweep_time > 0 {
                self.sweep_count += 1;
                // The documented sweep tick is 8192 master clocks per unit.
                if self.sweep_count >= u32::from(self.sweep_time) * 64 {
                    self.sweep_count = 0;
                    let ch = &mut self.channels[2];
                    ch.pitch = ch.pitch.wrapping_add(self.sweep_step as u16).min(0x07FF);
                }
            }
            // One noise step per frame.
            let feedback = (self.noise ^ (self.noise >> 1)) & 1;
            self.noise = (self.noise >> 1) | (feedback << 14);

            let mut left = 0i32;
            let mut right = 0i32;
            for (index, ch) in self.channels.iter_mut().enumerate() {
                if !ch.enabled {
                    continue;
                }
                let sample: i32 = if index == 1 && self.voice_mode {
                    // Channel 2 as an 8-bit unsigned DAC.
                    i32::from(self.voice_sample) - 0x80
                } else if index == 3 && self.noise_mode {
                    if self.noise & 1 != 0 { 7 } else { -8 }
                } else {
                    // The wavetable: 32 packed 4-bit samples, centred.
                    let step = (CLOCK_DIVIDER << 16) / (2048 - u32::from(ch.pitch));
                    ch.phase = ch.phase.wrapping_add(step);
                    let position = (ch.phase >> 16) as usize % 32;
                    let byte = self.wave_ram[index * 16 + position / 2];
                    let nibble = if position.is_multiple_of(2) {
                        byte & 0x0F
                    } else {
                        byte >> 4
                    };
                    i32::from(nibble) - 8
                };
                // Volume nibbles scale each side; x32 lands one loud channel
                // near the usual ~8k headroom over the 4-bit range.
                left += sample * i32::from(ch.volume >> 4) * 32;
                right += sample * i32::from(ch.volume & 0x0F) * 32;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WonderSwan master clock.
    const CLOCK: u32 = 3_072_000;

    fn render(chip: &mut WonderSwan, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut WonderSwan) {
        chip.write(0, 0x0F, 0x00); // wave RAM at base 0
        for at in 0..16u16 {
            // A square: eight high nibble-pairs, eight low.
            chip.write(1, at, if at < 8 { 0xFF } else { 0x00 });
        }
        chip.write(0, 0x00, 0x00); // pitch: period 2048-0x700
        chip.write(0, 0x01, 0x07);
        chip.write(0, 0x08, 0xFF); // both sides full
        chip.write(0, 0x10, 0x01); // enable channel 1
    }

    #[test]
    fn a_fresh_chip_is_silent_and_an_enabled_one_is_not() {
        let mut chip = WonderSwan::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
    }

    /// The wave RAM base register frames which memory writes land: writes
    /// outside the window must not corrupt the tables.
    #[test]
    fn the_wave_base_frames_the_window() {
        let mut chip = WonderSwan::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x0F, 0x01); // window at 0x40
        chip.write(1, 0x0040, 0xAB);
        chip.write(1, 0x0000, 0xCD); // outside: dropped
        assert_eq!(chip.wave_ram[0], 0xAB);
        assert!(!chip.wave_ram.contains(&0xCD));
    }

    /// Channel 2's voice mode plays the DAC byte instead of its table.
    #[test]
    fn the_voice_mode_is_a_dac() {
        let mut chip = WonderSwan::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x09, 0xFF); // channel 2 volume
        chip.write(0, 0x10, 0x22); // enable channel 2, voice mode
        chip.write(0, 0x11, 0xFF); // a loud positive sample
        let samples = render(&mut chip, 50);
        assert!(samples.iter().all(|&(l, _)| l > 0), "a held DAC level");
    }

    /// Channel 4's noise mode produces both polarities.
    #[test]
    fn the_noise_mode_rattles() {
        let mut chip = WonderSwan::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x0B, 0xFF);
        chip.write(0, 0x10, 0x88); // enable channel 4, noise mode
        let samples = render(&mut chip, 500);
        assert!(samples.iter().any(|&(l, _)| l > 0));
        assert!(samples.iter().any(|&(l, _)| l < 0));
    }
}
