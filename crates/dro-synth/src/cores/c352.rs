// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Namco C352: thirty-two PCM voices, four output buses, one noise
//! generator.
//!
//! 1,271 files in the VGMRips corpus -- the largest single catalogue left
//! when this core was written: Namco's System 11/12/22/23 era (Ridge Racer,
//! Tekken, Time Crisis, Ace Combat). Thirty-two voices of 8-bit linear or
//! mu-law PCM from a 24-bit ROM space, keyed through their flag registers,
//! at a rate the header's own divider field sets.
//!
//! **Route B, from the documented behaviour** (the register and flag layout
//! as MAME's `c352.cpp`, BSD-3-Clause, documents them), so it lives in the
//! permissive crate.
//!
//! Stated approximations, each carried until a corpus file proves it
//! matters: the **mu-law curve** is a piecewise-linear reconstruction of
//! the documented shape rather than a silicon measurement; the **rear
//! outputs mix into the front pair** (this engine is stereo); and the
//! frequency-modulation, link and filter flags are unmodelled. Phase
//! inversion, loops, reverse playback and the noise voice are real.

use crate::chip::ChipCore;

/// Flag bits, as documented.
const FLG_KEYON: u16 = 0x4000;
const FLG_KEYOFF: u16 = 0x2000;
const FLG_PHASEFL: u16 = 0x0100;
const FLG_NOISE: u16 = 0x0010;
const FLG_MULAW: u16 = 0x0008;
const FLG_LOOP: u16 = 0x0002;
const FLG_REVERSE: u16 = 0x0001;

/// The mu-law expansion: a piecewise-linear curve, gentler near zero,
/// doubling its slope over four documented breakpoints. Regenerated in a
/// test from the same piecewise description.
fn mulaw(byte: u8) -> i32 {
    let magnitude = i32::from(byte & 0x7F);
    let mut level = 0i32;
    let mut code = 0i32;
    for (until, slope) in [(16, 1), (24, 2), (48, 4), (100, 8), (128, 16)] {
        let span = magnitude.min(until) - code;
        if span <= 0 {
            break;
        }
        level += span * slope;
        code += span;
    }
    let value = level << 5;
    if byte & 0x80 != 0 { -value } else { value }
}

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    /// Front volumes: high byte left, low byte right.
    volume_front: u16,
    /// Rear volumes, mixed into the same stereo pair here.
    volume_rear: u16,
    frequency: u16,
    flags: u16,
    bank: u16,
    start: u16,
    end: u16,
    loop_start: u16,
    /// Sample position from the start, 16.16.
    position: u32,
    playing: bool,
}

/// The C352.
#[derive(Debug)]
pub struct C352 {
    rate: u32,
    clock: u32,
    divider: u32,
    voices: [Voice; 32],
    rom: Vec<u8>,
    /// The noise generator's shift register.
    noise: u32,
}

impl C352 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 42_667,
            clock: 0,
            divider: 288,
            voices: [Voice::default(); 32],
            rom: Vec::new(),
            noise: 0x5A5A_5A5A,
        }
    }

    fn update_rate(&mut self) {
        self.rate = (self.clock / self.divider.max(1)).max(1);
    }
}

impl Default for C352 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for C352 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        let divider = self.divider;
        *self = Self {
            clock,
            divider,
            ..Self::new()
        };
        self.rom = rom;
        self.update_rate();
    }

    /// The header's own divider field, times the documented four.
    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        let field = u32::from(settings.c352_clock_divider);
        self.divider = if field == 0 { 288 } else { field * 4 };
        self.update_rate();
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The VGM's `0xE1 aa aa dd dd`: a 16-bit register address, 16-bit
    /// data. Voices are eight words apart.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        if addr >= 0x100 {
            // The global file: unmodelled (the master volume and DSP
            // controls the corpus leaves at their defaults).
            return;
        }
        let voice = &mut self.voices[usize::from(addr >> 3)];
        match addr & 0x07 {
            0x00 => voice.volume_front = data,
            0x01 => voice.volume_rear = data,
            0x02 => voice.frequency = data,
            0x03 => {
                voice.flags = data;
                if data & FLG_KEYON != 0 {
                    voice.position = 0;
                    voice.playing = true;
                }
                if data & FLG_KEYOFF != 0 {
                    voice.playing = false;
                }
            }
            0x04 => voice.bank = data,
            0x05 => voice.start = data,
            0x06 => voice.end = data,
            0x07 => voice.loop_start = data,
            _ => unreachable!(),
        }
    }

    /// The sample ROM: block type `0x92`.
    fn load_rom(&mut self, _block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        let total = total_size as usize;
        if self.rom.len() < total {
            self.rom.resize(total, 0);
        }
        let at = start as usize;
        let end = (at + data.len()).min(self.rom.len());
        if at < end {
            self.rom[at..end].copy_from_slice(&data[..end - at]);
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            // One noise step per frame; every noise voice shares it.
            let feedback = (self.noise ^ (self.noise >> 1)) & 1;
            self.noise = (self.noise >> 1) | (feedback << 30);
            let noise_sample = if self.noise & 1 != 0 { 0x1000 } else { -0x1000 };

            let mut left = 0i32;
            let mut right = 0i32;
            for voice in &mut self.voices {
                if !voice.playing {
                    continue;
                }
                let value = if voice.flags & FLG_NOISE != 0 {
                    noise_sample
                } else {
                    let steps = voice.position >> 16;
                    let span = u32::from(voice.end.wrapping_sub(voice.start));
                    if steps > span {
                        if voice.flags & FLG_LOOP != 0 {
                            voice.position = 0;
                            voice.start = voice.loop_start;
                            continue;
                        }
                        voice.playing = false;
                        continue;
                    }
                    let offset = if voice.flags & FLG_REVERSE != 0 {
                        u32::from(voice.start).wrapping_sub(steps)
                    } else {
                        u32::from(voice.start).wrapping_add(steps)
                    };
                    let at = ((u32::from(voice.bank) << 16) | (offset & 0xFFFF)) as usize;
                    let Some(&byte) = self.rom.get(at) else {
                        voice.playing = false;
                        continue;
                    };
                    if voice.flags & FLG_MULAW != 0 {
                        mulaw(byte)
                    } else {
                        // Linear lands on the same ~15-bit scale the
                        // mu-law curve tops out at (992 << 5), so the two
                        // formats sit level in a mix.
                        i32::from(byte as i8) << 8
                    }
                };
                voice.position = voice.position.wrapping_add(u32::from(voice.frequency));

                // Rear mixes into the same stereo pair at half weight --
                // this engine is stereo, and the rear bus usually carries
                // the same material into the cabinet's back speakers.
                let volume_left =
                    i32::from(voice.volume_front >> 8) + (i32::from(voice.volume_rear >> 8) >> 1);
                let volume_right = i32::from(voice.volume_front & 0xFF)
                    + (i32::from(voice.volume_rear & 0xFF) >> 1);
                // Full volume on a full-scale sample lands near the ~8k
                // one-channel headroom the other cores use.
                let mut sample_left = (value * volume_left) >> 10;
                let sample_right = (value * volume_right) >> 10;
                if voice.flags & FLG_PHASEFL != 0 {
                    sample_left = -sample_left;
                }
                left += sample_left;
                right += sample_right;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A System 11 C352 clock with the usual divider field of 72.
    const CLOCK: u32 = 24_192_000;

    fn configured() -> C352 {
        let mut chip = C352::new();
        chip.reset(CLOCK, false);
        chip.configure(&dro_core::vgm::ChipSettings {
            c352_clock_divider: 72,
            ..Default::default()
        });
        chip
    }

    fn render(chip: &mut C352, frames: usize) -> Vec<(i32, i32)> {
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

    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        for (index, byte) in rom[0x1000..0x1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        rom
    }

    fn key_on(chip: &mut C352) {
        chip.write(0, 0x00, 0xFFFF); // front volumes full
        chip.write(0, 0x02, 0xFFFF); // frequency just under 1:1
        chip.write(0, 0x04, 0x0000); // bank 0
        chip.write(0, 0x05, 0x1000); // start
        chip.write(0, 0x06, 0x1100); // end
        chip.write(0, 0x03, FLG_KEYON);
    }

    /// The divider field is the rate: 24.192 MHz over 72*4.
    #[test]
    fn the_header_divider_sets_the_rate() {
        let chip = configured();
        assert_eq!(chip.native_rate(), CLOCK / (72 * 4));
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = configured();
        let sample_rom = rom();
        chip.load_rom(0x92, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A one-shot stops at its end; the loop flag sustains through it; the
    /// key-off flag stops that.
    #[test]
    fn end_loop_and_keyoff_behave() {
        let mut chip = configured();
        let sample_rom = rom();
        chip.load_rom(0x92, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x180);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "one-shot must stop");

        chip.write(0, 0x07, 0x1000); // loop to the start
        chip.write(0, 0x03, FLG_KEYON | FLG_LOOP);
        render(&mut chip, 0x400);
        assert!(energy(&render(&mut chip, 200)) > 0, "loop must sustain");

        chip.write(0, 0x03, FLG_KEYOFF);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "key-off must stop");
    }

    /// The mu-law curve: symmetric, monotonic, gentler near zero than at
    /// the top -- regenerated from the same piecewise description.
    #[test]
    fn the_mulaw_curve_is_symmetric_and_progressive() {
        assert_eq!(mulaw(0x00), 0);
        for code in 1..0x80u8 {
            assert!(mulaw(code) > mulaw(code - 1), "monotonic at {code}");
            assert_eq!(mulaw(code | 0x80), -mulaw(code), "symmetric at {code}");
        }
        let low_step = mulaw(1) - mulaw(0);
        let high_step = mulaw(0x7F) - mulaw(0x7E);
        assert!(high_step / low_step >= 8, "the top must be steeper");
    }

    /// A noise voice sounds without any ROM at all, and both polarities
    /// appear.
    #[test]
    fn the_noise_flag_plays_the_generator() {
        let mut chip = configured();
        chip.write(0, 0x00, 0xFFFF);
        chip.write(0, 0x02, 0xFFFF);
        chip.write(0, 0x03, FLG_KEYON | FLG_NOISE);
        let samples = render(&mut chip, 500);
        assert!(samples.iter().any(|&(l, _)| l > 0));
        assert!(samples.iter().any(|&(l, _)| l < 0));
    }
}
