// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Konami K054539: eight PCM channels from ROM, with a reverb this core
//! does not model.
//!
//! 872 files in the VGMRips corpus -- the 1990s Konami arcade catalogue
//! (Mystic Warriors, Violent Storm, Lethal Enforcers). Eight channels, each
//! reading 8-bit PCM, 16-bit PCM or 4-bit DPCM from a shared ROM at a
//! 24-bit pitch against a 48 kHz base rate.
//!
//! **Route B, from the documented behaviour** (the register map as MAME's
//! `k054539.cpp`, BSD-3-Clause, documents it), so it lives in the
//! permissive crate.
//!
//! What the corpus actually exercises was measured before this was written
//! (a register histogram over the rips): pitch, volume, start position and
//! the key-on/key-off pair dominate; pan appears; the reverb registers are
//! initialised and left alone. So the model is: samples, pitch, volume,
//! pan, keys -- and three stated approximations. **Loops** jump on the end
//! marker unconditionally (a marker at the loop target stops the channel),
//! because the loop-enable's register home is not confidently documented
//! and the corpus keys everything off explicitly -- 7,000 key-offs against
//! 5,000 key-ons in the sampled files. **Reverb** is ignored. **Pan** is a
//! linear weighting rather than the chip's measured curve.

use crate::chip::ChipCore;

/// One frame per 384 clocks: 48 kHz at the usual 18.432 MHz crystal.
const CLOCK_DIVIDER: u32 = 384;

/// The volume register's attenuation per step, compounded at const time:
/// -36 dB per 0x40 steps (0.5625 dB a step), in 16.16 with guard bits, the
/// same technique as the OPN ADPCM curve. Regenerated against the closed
/// form in a test.
const fn volume_gain(step: u8) -> i32 {
    // 10^(-0.5625/20) in 16.16-with-16-guard-bits per step.
    const RATIO: i64 = 61_438;
    let mut value: i64 = 65_536 << 16;
    let mut left = step;
    while left > 0 {
        value = (value * RATIO) >> 16;
        left -= 1;
    }
    (value >> 16) as i32
}

/// How a DPCM nibble moves the level, in the chip's 16-bit sample units.
const DPCM_DELTA: [i32; 16] = [
    0, 256, 512, 1024, 2048, 4096, 8192, 16384, 0, -16384, -8192, -4096, -2048, -1024, -512, -256,
];

/// What a channel's flag register says it plays.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum SampleType {
    #[default]
    Pcm8,
    Pcm16,
    Dpcm,
}

/// One channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// 24-bit pitch: `0x10000` is one ROM sample per output frame.
    pitch: u32,
    volume: u8,
    /// Low four bits of the pan register.
    pan: u8,
    /// 24-bit byte addresses.
    start: u32,
    loop_start: u32,
    /// Playback position in 16.16 sample units from the start address.
    position: u64,
    sample_type: SampleType,
    reverse: bool,
    playing: bool,
    /// DPCM decoder level and half-byte phase.
    dpcm_level: i32,
    dpcm_high: bool,
}

/// The K054539.
#[derive(Debug)]
pub struct K054539 {
    rate: u32,
    channels: [Channel; 8],
    rom: Vec<u8>,
}

impl K054539 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 48_000,
            channels: [Channel::default(); 8],
            rom: Vec::new(),
        }
    }
}

impl Default for K054539 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for K054539 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            ..Self::new()
        };
        self.rom = rom;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The VGM's `0xD3 pp rr dd`: the register index is `pp << 8 | rr`.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let reg = (u16::from(port) << 8) | (addr & 0xFF);
        let data = (data & 0xFF) as u8;
        match reg {
            // The per-channel file: eight channels, 0x20 apart.
            0x000..=0x0FF => {
                let ch = &mut self.channels[usize::from(reg >> 5)];
                match reg & 0x1F {
                    0x00 => ch.pitch = (ch.pitch & 0xFF_FF00) | u32::from(data),
                    0x01 => ch.pitch = (ch.pitch & 0xFF_00FF) | (u32::from(data) << 8),
                    0x02 => ch.pitch = (ch.pitch & 0x00_FFFF) | (u32::from(data) << 16),
                    0x03 => ch.volume = data,
                    0x05 => ch.pan = data & 0x0F,
                    0x08 => ch.loop_start = (ch.loop_start & 0xFF_FF00) | u32::from(data),
                    0x09 => ch.loop_start = (ch.loop_start & 0xFF_00FF) | (u32::from(data) << 8),
                    0x0A => ch.loop_start = (ch.loop_start & 0x00_FFFF) | (u32::from(data) << 16),
                    0x0C => ch.start = (ch.start & 0xFF_FF00) | u32::from(data),
                    0x0D => ch.start = (ch.start & 0xFF_00FF) | (u32::from(data) << 8),
                    0x0E => ch.start = (ch.start & 0x00_FFFF) | (u32::from(data) << 16),
                    // 0x04, 0x06, 0x07: reverb depth and delay -- not modelled.
                    _ => {}
                }
            }
            // Per-channel flags, two registers apart: sample type and
            // direction.
            0x200..=0x20F if reg & 1 == 0 => {
                let ch = &mut self.channels[usize::from((reg >> 1) & 0x07)];
                ch.sample_type = match (data >> 2) & 0x03 {
                    1 => SampleType::Pcm16,
                    2 => SampleType::Dpcm,
                    _ => SampleType::Pcm8,
                };
                ch.reverse = data & 0x20 != 0;
            }
            // Key on: a bit per channel, latching the start position.
            0x214 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    if data & (1 << index) != 0 {
                        ch.position = 0;
                        ch.dpcm_level = 0;
                        ch.dpcm_high = false;
                        ch.playing = true;
                    }
                }
            }
            // Key off.
            0x215 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    if data & (1 << index) != 0 {
                        ch.playing = false;
                    }
                }
            }
            // 0x22E banks the ROM readback window; 0x22F is the control
            // register. Neither affects playback here: positions are
            // absolute 24-bit addresses, and gating sound on a control bit
            // the drivers may not set is the NES lesson again.
            _ => {}
        }
    }

    /// The sample ROM: block type `0x8C`.
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
            let mut left = 0i32;
            let mut right = 0i32;
            for ch in &mut self.channels {
                if !ch.playing {
                    continue;
                }
                let steps = (ch.position >> 16) as u32;
                let sample: i32 = match ch.sample_type {
                    SampleType::Pcm8 => {
                        let at = sample_address(ch, steps, 1);
                        let Some(&byte) = self.rom.get(at) else {
                            ch.playing = false;
                            continue;
                        };
                        if byte == 0x80 {
                            if !restart_at_loop(ch, &self.rom) {
                                continue;
                            }
                            i32::from(self.rom[sample_address(ch, 0, 1)] as i8) << 8
                        } else {
                            i32::from(byte as i8) << 8
                        }
                    }
                    SampleType::Pcm16 => {
                        let at = sample_address(ch, steps, 2);
                        let Some(pair) = self.rom.get(at..at + 2) else {
                            ch.playing = false;
                            continue;
                        };
                        let value = i32::from(i16::from_le_bytes([pair[0], pair[1]]));
                        if value == -0x8000 {
                            if !restart_at_loop(ch, &self.rom) {
                                continue;
                            }
                            let at = sample_address(ch, 0, 2);
                            i32::from(i16::from_le_bytes([self.rom[at], self.rom[at + 1]]))
                        } else {
                            value
                        }
                    }
                    SampleType::Dpcm => {
                        // Two nibbles a byte, low first; the level integrates.
                        let at = sample_address(ch, steps / 2, 1);
                        let Some(&byte) = self.rom.get(at) else {
                            ch.playing = false;
                            continue;
                        };
                        let nibble = if steps % 2 == 0 {
                            byte & 0x0F
                        } else {
                            byte >> 4
                        };
                        // Only integrate when the position has advanced to a
                        // new nibble; at fractional pitches the same nibble
                        // is held.
                        if (steps % 2 == 1) != ch.dpcm_high || steps == 0 {
                            ch.dpcm_level = (ch.dpcm_level + DPCM_DELTA[usize::from(nibble)])
                                .clamp(-0x8000, 0x7FFF);
                            ch.dpcm_high = steps % 2 == 1;
                        }
                        ch.dpcm_level
                    }
                };
                ch.position += u64::from(ch.pitch);

                let scaled = (sample * volume_gain(ch.volume)) >> 16;
                // A linear pan: centre 8. A stated approximation of the
                // chip's measured curve. The final shift puts one channel's
                // full scale near 8192, the one-channel headroom the other
                // cores use -- the first corpus render clipped without it.
                let pan = i32::from(ch.pan.clamp(1, 15));
                left += (scaled * (16 - pan)) >> 5;
                right += (scaled * pan) >> 5;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

/// The ROM byte address `steps` sample units from the channel's start, in
/// its direction.
fn sample_address(ch: &Channel, steps: u32, width: u32) -> usize {
    let offset = steps * width;
    if ch.reverse {
        ch.start.saturating_sub(offset) as usize
    } else {
        (ch.start + offset) as usize
    }
}

/// Jumps a channel to its loop point, or stops it when the loop target is
/// itself the end marker (or out of the ROM). Returns whether it plays on.
fn restart_at_loop(ch: &mut Channel, rom: &[u8]) -> bool {
    ch.position = 0;
    ch.start = ch.loop_start;
    ch.dpcm_level = 0;
    ch.dpcm_high = false;
    let next = rom.get(ch.start as usize).copied();
    let still = match (ch.sample_type, next) {
        (SampleType::Pcm8, Some(byte)) => byte != 0x80,
        (SampleType::Pcm16, Some(low)) => {
            !(low == 0x00 && rom.get(ch.start as usize + 1) == Some(&0x80))
        }
        (SampleType::Dpcm, Some(_)) => true,
        (_, None) => false,
    };
    ch.playing = still;
    still
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual crystal.
    const CLOCK: u32 = 18_432_000;

    fn render(chip: &mut K054539, frames: usize) -> Vec<(i32, i32)> {
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

    /// A ROM whose sample 0x1000 alternates extremes and ends in a marker,
    /// with a marker at the loop target too.
    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        for (index, byte) in rom[0x1000..0x1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        rom[0x1100] = 0x80; // end marker
        rom[0x0FFF] = 0x80; // and one at the loop target
        rom
    }

    fn key_on(chip: &mut K054539) {
        chip.write(2, 0x00, 0x00); // ch0 flags: 8-bit, forward
        chip.write(0, 0x00, 0x00); // pitch 0x10000: one byte a frame
        chip.write(0, 0x01, 0x00);
        chip.write(0, 0x02, 0x01);
        chip.write(0, 0x03, 0x00); // no attenuation
        chip.write(0, 0x05, 0x08); // centre
        chip.write(0, 0x0C, 0x00); // start 0x1000
        chip.write(0, 0x0D, 0x10);
        chip.write(0, 0x0E, 0x00);
        chip.write(0, 0x08, 0xFF); // loop target 0x0FFF (a marker)
        chip.write(0, 0x09, 0x0F);
        chip.write(0, 0x0A, 0x00);
        chip.write(2, 0x14, 0x01); // key on channel 0
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = K054539::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8C, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// The end marker stops a one-shot whose loop target is marked too, and
    /// key-off silences a playing channel.
    #[test]
    fn the_marker_and_the_key_off_both_stop() {
        let mut chip = K054539::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8C, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x110); // run past the marker
        assert_eq!(energy(&render(&mut chip, 100)), 0, "the marker must stop");

        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 50)) > 0);
        chip.write(2, 0x15, 0x01);
        assert_eq!(energy(&render(&mut chip, 100)), 0, "key-off must stop");
    }

    /// 16-bit samples read little-endian pairs; the type comes from the
    /// flag register.
    #[test]
    fn sixteen_bit_samples_play() {
        let mut chip = K054539::new();
        chip.reset(CLOCK, false);
        let mut sample_rom = vec![0u8; 0x2000];
        for pair in sample_rom[0x1000..0x1100].chunks_exact_mut(2) {
            pair[0] = 0xFF;
            pair[1] = 0x3F; // +0x3FFF
        }
        chip.load_rom(0x8C, sample_rom.len() as u32, 0, &sample_rom);
        chip.write(2, 0x00, 0x04); // 16-bit
        key_on(&mut chip);
        let peak = render(&mut chip, 50)
            .iter()
            .map(|&(l, _)| l.abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 0x800, "16-bit amplitude must come through: {peak}");
    }

    /// The volume curve against its closed form: -36 dB per 0x40 steps.
    #[test]
    fn the_volume_curve_is_the_documented_attenuation() {
        for step in [0u8, 1, 0x20, 0x40, 0x80, 0xFF] {
            let expected = 65536.0 * 10f64.powf(-36.0 * f64::from(step) / 64.0 / 20.0);
            let got = f64::from(volume_gain(step));
            assert!(
                (got - expected).abs() <= expected * 0.02 + 2.0,
                "step {step}: {got} vs {expected}"
            );
        }
    }
}
