// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Yamaha YMF278B (OPL4), its wavetable half: 24 PCM voices whose
//! sample headers live in the ROM itself. 494 corpus files, most of them
//! MSX MoonSound.
//!
//! **Route B, from the documented behaviour** (Yamaha's OPL4 application
//! manual as the community preserves it; MAME's `ymf278b.cpp` records the
//! same layout), so it lives in the permissive crate.
//!
//! The chip's defining shape: a voice is pointed at a **wave number**, and
//! the 12-byte header for that wave -- format, start, loop, end -- is read
//! from the sample ROM, not from registers. Formats are 8-bit, packed
//! 12-bit (two samples in three bytes) and 16-bit big-endian.
//!
//! Stated approximations, each carried until a corpus file proves it
//! matters: the **FM half is not modelled** (it is an OPL3 behind ports 0-1;
//! MoonSound rips lean overwhelmingly on the wave side), the **envelope**
//! is simplified to instant attack, sustain at total level, and a fast
//! release on key-off, and the per-voice pan register's curve is linear.

use crate::chip::ChipCore;

/// One frame per 768 clocks: 44.1 kHz at the 33.8688 MHz crystal.
const CLOCK_DIVIDER: u32 = 768;

/// Total level: 0.375 dB a step, compounded at const time with guard bits.
const fn level_gain(step: u8) -> i32 {
    const RATIO: i64 = 62_771; // 10^(-0.375/20) in 16.16
    let mut value: i64 = 65_536 << 16;
    let mut left = step;
    while left > 0 {
        value = (value * RATIO) >> 16;
        left -= 1;
    }
    (value >> 16) as i32
}

/// A wave's format, from its header.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Format {
    #[default]
    Pcm8,
    Pcm12,
    Pcm16,
}

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    /// The nine-bit wave number; the high bit rides the F-number register.
    wave_low: u8,
    wave_high: bool,
    /// Ten bits of F-number and a signed four-bit octave.
    fnum: u16,
    octave: i8,
    total_level: u8,
    /// Pan 0-15 from the key register's neighbour.
    pan: u8,
    keyed: bool,
    /// Fast fade-out after key-off, 0-255.
    release: u8,
    /// From the loaded header.
    format: Format,
    start: u32,
    loop_start: u32,
    end: u32,
    /// Sample index, 16.16.
    position: u64,
}

impl Voice {
    /// The 16.16 sample step: `(1024 + fnum) * 2^octave / 1024`.
    fn step(&self) -> u64 {
        let base = u64::from(1024 + self.fnum) << 16 >> 10;
        if self.octave >= 0 {
            base << self.octave
        } else {
            base >> (-self.octave).min(16)
        }
    }
}

/// The YMF278B, wave side.
#[derive(Debug)]
pub struct Ymf278b {
    rate: u32,
    voices: [Voice; 24],
    rom: Vec<u8>,
}

impl Ymf278b {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            voices: [Voice::default(); 24],
            rom: Vec::new(),
        }
    }

    /// Loads a voice's wave header from the ROM: 12 bytes at `wave * 12`.
    fn load_header(&mut self, index: usize) {
        let voice = self.voices[index];
        let wave = (usize::from(voice.wave_low)) | (usize::from(voice.wave_high) << 8);
        let at = wave * 12;
        let Some(header) = self.rom.get(at..at + 12) else {
            return;
        };
        let voice = &mut self.voices[index];
        voice.format = match header[0] >> 6 {
            1 => Format::Pcm12,
            2 => Format::Pcm16,
            _ => Format::Pcm8,
        };
        voice.start = (u32::from(header[0] & 0x3F) << 16)
            | (u32::from(header[1]) << 8)
            | u32::from(header[2]);
        voice.loop_start = (u32::from(header[3]) << 8) | u32::from(header[4]);
        // The end field is stored negated: the documented encoding.
        let end_field = (u32::from(header[5]) << 8) | u32::from(header[6]);
        voice.end = 0x1_0000 - end_field;
    }

    /// One sample of `format` at sample-index `index` from `base`.
    fn sample(&self, format: Format, base: u32, index: u32) -> Option<i32> {
        match format {
            Format::Pcm8 => {
                let byte = *self.rom.get((base + index) as usize)?;
                Some(i32::from(byte as i8) << 8)
            }
            Format::Pcm12 => {
                // Two samples in three bytes: [a11-4][a3-0 b3-0][b11-4].
                let at = (base + (index / 2) * 3) as usize;
                let trio = self.rom.get(at..at + 3)?;
                let value = if index % 2 == 0 {
                    (i32::from(trio[0] as i8) << 4) | i32::from(trio[1] >> 4)
                } else {
                    (i32::from(trio[2] as i8) << 4) | i32::from(trio[1] & 0x0F)
                };
                Some(value << 4)
            }
            Format::Pcm16 => {
                let at = (base + index * 2) as usize;
                let pair = self.rom.get(at..at + 2)?;
                Some(i32::from(i16::from_be_bytes([pair[0], pair[1]])))
            }
        }
    }
}

impl Default for Ymf278b {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ymf278b {
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

    /// Port 2 carries the wave side; ports 0-1 are the unmodelled OPL3.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        if port != 2 {
            return;
        }
        let reg = (addr & 0xFF) as usize;
        let data = (data & 0xFF) as u8;
        match reg {
            // Wave number low: the write that loads the header.
            0x08..=0x1F => {
                let index = reg - 0x08;
                self.voices[index].wave_low = data;
                self.load_header(index);
            }
            // F-number low bits and the wave number's ninth bit.
            0x20..=0x37 => {
                let voice = &mut self.voices[reg - 0x20];
                voice.wave_high = data & 0x01 != 0;
                voice.fnum = (voice.fnum & 0x380) | u16::from(data >> 1);
            }
            // F-number high and the octave.
            0x38..=0x4F => {
                let voice = &mut self.voices[reg - 0x38];
                voice.fnum = (voice.fnum & 0x07F) | (u16::from(data & 0x07) << 7);
                voice.octave = ((data >> 4) as i8) << 4 >> 4; // sign-extend
            }
            // Total level.
            0x50..=0x67 => self.voices[reg - 0x50].total_level = data >> 1,
            // Key, damp, pan.
            0x68..=0x7F => {
                let voice = &mut self.voices[reg - 0x68];
                let key = data & 0x80 != 0;
                if key && !voice.keyed {
                    voice.position = 0;
                    voice.release = 255;
                }
                voice.keyed = key;
                voice.pan = data & 0x0F;
            }
            // 0x80-0xF7: LFO, ADSR rates, AM depth -- the simplified
            // envelope reads none of them (stated at the module).
            _ => {}
        }
    }

    /// The sample memory: types `0x84` (ROM) and `0x87` (RAM), one image.
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
            for index in 0..self.voices.len() {
                let voice = self.voices[index];
                let fading = !voice.keyed && voice.release > 0;
                if !voice.keyed && !fading {
                    continue;
                }
                let mut sample_index = (voice.position >> 16) as u32;
                if sample_index >= voice.end.max(1) {
                    // Loop from the header's loop point.
                    let span = voice.end.saturating_sub(voice.loop_start).max(1);
                    sample_index = voice.loop_start + (sample_index - voice.end) % span;
                }
                let Some(value) = self.sample(voice.format, voice.start, sample_index) else {
                    self.voices[index].keyed = false;
                    self.voices[index].release = 0;
                    continue;
                };
                let voice = &mut self.voices[index];
                voice.position += voice.step();
                // >>18 rather than >>17: the first corpus render clipped the
                // mixer with 24 voices in play.
                let mut scaled = (value * level_gain(voice.total_level)) >> 18;
                if fading {
                    scaled = scaled * i32::from(voice.release) >> 8;
                    voice.release = voice.release.saturating_sub(2);
                }
                // Pan: 0 is centre; 1-7 attenuates the right side, 9-15
                // the left. Linear steps -- a stated approximation of the
                // register's dB curve.
                let pan = i32::from(voice.pan.clamp(0, 15));
                let left_att = if pan >= 9 { pan - 8 } else { 0 };
                let right_att = if (1..=7).contains(&pan) { pan } else { 0 };
                left += (scaled * (8 - left_att)) >> 3;
                right += (scaled * (8 - right_att)) >> 3;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MoonSound crystal.
    const CLOCK: u32 = 33_868_800;

    fn render(chip: &mut Ymf278b, frames: usize) -> Vec<(i32, i32)> {
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

    /// A ROM with wave 0's header pointing at an 8-bit sample at 0x1000.
    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x2000];
        // Header 0: format 8-bit, start 0x001000, loop 0x40, end count 0x100.
        rom[0] = 0x00;
        rom[1] = 0x10;
        rom[2] = 0x00;
        rom[3] = 0x00;
        rom[4] = 0x40;
        let end_field = 0x1_0000u32 - 0x100;
        rom[5] = (end_field >> 8) as u8;
        rom[6] = (end_field & 0xFF) as u8;
        for (index, byte) in rom[0x1000..0x1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        rom
    }

    fn key_on(chip: &mut Ymf278b) {
        chip.write(2, 0x20, 0x00); // wave bit 8 clear, fnum low 0
        chip.write(2, 0x08, 0x00); // wave 0: loads the header
        chip.write(2, 0x38, 0x00); // octave 0
        chip.write(2, 0x50, 0x00); // full level
        chip.write(2, 0x68, 0x80); // key on, centre pan
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = Ymf278b::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), 44_100);
        let sample_rom = rom();
        chip.load_rom(0x84, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// The header is read from the ROM: format, start and end all come from
    /// the wave table, and a key-off fades rather than clicks.
    #[test]
    fn the_header_drives_playback_and_keyoff_fades() {
        let mut chip = Ymf278b::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x84, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        assert_eq!(chip.voices[0].start, 0x1000, "start from the header");
        assert_eq!(chip.voices[0].end, 0x100, "end count from the header");
        render(&mut chip, 100);

        chip.write(2, 0x68, 0x00); // key off
        let fade = energy(&render(&mut chip, 60));
        assert!(fade > 0, "the release must fade, not cut");
        render(&mut chip, 200);
        assert_eq!(energy(&render(&mut chip, 100)), 0, "and then be silent");
    }

    /// Packed 12-bit: two samples in three bytes, both halves decoded.
    #[test]
    fn the_packed_twelve_bit_format_decodes() {
        let mut chip = Ymf278b::new();
        chip.rom = vec![0u8; 16];
        // Sample a = 0x7F0 positive, sample b = 0x801 negative.
        chip.rom[0] = 0x7F;
        chip.rom[1] = 0x01; // a low nibble 0, b low nibble 1
        chip.rom[2] = 0x80;
        let a = chip.sample(Format::Pcm12, 0, 0).unwrap();
        let b = chip.sample(Format::Pcm12, 0, 1).unwrap();
        assert!(a > 0 && b < 0, "a={a} b={b}");
    }
}
