// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Yamaha YMW258-F ("MultiPCM", Sega 315-5560): 28 voices whose sample
//! headers live in the ROM, on Sega's Multi 32 and Model 1 boards (Virtua
//! Racing, Star Wars Arcade, OutRunners). 224 corpus files.
//!
//! **Route B, from the documented behaviour** (the register interface as
//! MAME's `multipcm.cpp` documents it), so it lives in the permissive
//! crate. The chip is the OPL4 wave side's close cousin: a voice names a
//! sample number and a 12-byte header in ROM carries start, loop and
//! negated end.
//!
//! The write interface is indirect: one port selects the voice, one the
//! register, one carries the value. The VGM's `0xC3` bank command offsets
//! sample fetches for rips of banked boards.
//!
//! Stated approximations, as the OPL4 core's: the envelope is instant
//! attack, sustain at total level, fast fade on key-off; the LFO is
//! unmodelled; pan is linear.

use crate::chip::ChipCore;

/// One frame per 224 clocks: 44.1 kHz at the boards' 9.878 MHz.
const CLOCK_DIVIDER: u32 = 224;

/// Total level: 0.375 dB a step, the OPL4 curve.
const fn level_gain(step: u8) -> i32 {
    const RATIO: i64 = 62_771;
    let mut value: i64 = 65_536 << 16;
    let mut left = step;
    while left > 0 {
        value = (value * RATIO) >> 16;
        left -= 1;
    }
    (value >> 16) as i32
}

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    /// Sample number: low eight bits plus the pan register's ninth bit.
    sample_low: u8,
    sample_high: bool,
    pan: u8,
    /// Ten bits of F-number, signed four-bit octave.
    fnum: u16,
    octave: i8,
    total_level: u8,
    keyed: bool,
    release: u8,
    /// From the header.
    start: u32,
    loop_start: u32,
    end: u32,
    /// Sample index, 16.16.
    position: u64,
}

impl Voice {
    fn step(&self) -> u64 {
        let base = u64::from(1024 + self.fnum) << 16 >> 10;
        if self.octave >= 0 {
            base << self.octave
        } else {
            base >> (-self.octave).min(16)
        }
    }
}

/// The MultiPCM.
#[derive(Debug)]
pub struct MultiPcm {
    rate: u32,
    voices: [Voice; 28],
    /// The indirect interface's latches.
    selected_voice: usize,
    selected_register: u8,
    /// The `0xC3` bank offset, in bytes.
    bank: u32,
    rom: Vec<u8>,
}

impl MultiPcm {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            voices: [Voice::default(); 28],
            selected_voice: 0,
            selected_register: 0,
            bank: 0x18_0000,
            rom: Vec::new(),
        }
    }

    /// Loads the selected voice's sample header: 12 bytes at `sample * 12`.
    fn load_header(&mut self, index: usize) {
        let voice = self.voices[index];
        let sample = usize::from(voice.sample_low) | (usize::from(voice.sample_high) << 8);
        let at = sample * 12;
        let Some(header) = self.rom.get(at..at + 12) else {
            return;
        };
        let voice = &mut self.voices[index];
        voice.start = (u32::from(header[0] & 0x3F) << 16)
            | (u32::from(header[1]) << 8)
            | u32::from(header[2]);
        voice.loop_start = (u32::from(header[3]) << 8) | u32::from(header[4]);
        let end_field = (u32::from(header[5]) << 8) | u32::from(header[6]);
        voice.end = 0x1_0000 - end_field;
    }

    /// One register write to the selected voice.
    fn write_register(&mut self, data: u8) {
        let index = self.selected_voice;
        match self.selected_register {
            // Pan and the sample number's ninth bit.
            0 => {
                let voice = &mut self.voices[index];
                voice.pan = data >> 4;
                voice.sample_high = data & 0x01 != 0;
            }
            // Sample number: the write that loads the header.
            1 => {
                self.voices[index].sample_low = data;
                self.load_header(index);
            }
            // Pitch pair.
            2 => {
                let voice = &mut self.voices[index];
                voice.fnum = (voice.fnum & 0x3C0) | u16::from(data >> 2);
            }
            3 => {
                let voice = &mut self.voices[index];
                voice.fnum = (voice.fnum & 0x03F) | (u16::from(data & 0x0F) << 6);
                voice.octave = ((data >> 4) as i8) << 4 >> 4;
            }
            // Key.
            4 => {
                let voice = &mut self.voices[index];
                let key = data & 0x80 != 0;
                if key && !voice.keyed {
                    voice.position = 0;
                    voice.release = 255;
                }
                voice.keyed = key;
            }
            // Total level (the direct/interpolate bit is ignored).
            5 => self.voices[index].total_level = data >> 1,
            // 6-7: LFO -- unmodelled.
            _ => {}
        }
    }
}

impl Default for MultiPcm {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for MultiPcm {
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

    /// `0xB5 aa dd`: offset 0 carries the data, 1 selects the slot, 2 the
    /// register -- the corpus's own write pattern arbitrated it (the
    /// register/data pairs come in equal counts on 0 and 2, and offset 1
    /// spams the effect slots between them). Slot numbers skip every
    /// eighth encoding, the chip's 28-of-32 map. `0xC3` arrives as port 1
    /// with the bank offset.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        if port == 1 {
            // The 0xC3 bank command: the offset word in 64 KiB units.
            self.bank = u32::from(addr) << 16;
            return;
        }
        let data8 = (data & 0xFF) as u8;
        match addr & 0x03 {
            0 => self.write_register(data8),
            1 => {
                let raw = usize::from(data8 & 0x1F);
                if raw & 0x07 != 0x07 {
                    self.selected_voice = ((raw >> 3) * 7 + (raw & 0x07)).min(27);
                }
            }
            2 => self.selected_register = data8 & 0x07,
            _ => {}
        }
    }

    /// The sample ROM: block type `0x89`.
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
                    let span = voice.end.saturating_sub(voice.loop_start).max(1);
                    sample_index = voice.loop_start + (sample_index - voice.end) % span;
                }
                // Banking, calibrated against the corpus: only addresses
                // with bit 20 set pass through the bank register. Daytona
                // writes bank 0x100000 (identity for its layout) and every
                // sample decodes into its delivered ROM regions; OutRunners
                // writes no bank at all and its high samples land only
                // under the 0x180000 power-on default. Low addresses are
                // never banked -- both rips' low samples decode at face
                // value.
                let mut address = voice.start + sample_index;
                if address & 0x10_0000 != 0 {
                    address = (address & 0x0F_FFFF) + self.bank;
                }
                let at = address as usize;
                let Some(&byte) = self.rom.get(at) else {
                    self.voices[index].keyed = false;
                    self.voices[index].release = 0;
                    continue;
                };
                let voice = &mut self.voices[index];
                voice.position += voice.step();
                let value = i32::from(byte as i8) << 8;
                let mut scaled = (value * level_gain(voice.total_level)) >> 18;
                if fading {
                    scaled = scaled * i32::from(voice.release) >> 8;
                    voice.release = voice.release.saturating_sub(2);
                }
                // Pan: 0 centre, 1-7 right-attenuating, 8-15 left (linear,
                // stated).
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

    /// The Multi 32's MultiPCM clock.
    const CLOCK: u32 = 9_878_400;

    fn render(chip: &mut MultiPcm, frames: usize) -> Vec<(i32, i32)> {
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
        rom[0] = 0x00; // sample 0 header: start 0x001000
        rom[1] = 0x10;
        rom[2] = 0x00;
        rom[3] = 0x00; // loop 0
        rom[4] = 0x00;
        let end_field = 0x1_0000u32 - 0x100;
        rom[5] = (end_field >> 8) as u8;
        rom[6] = (end_field & 0xFF) as u8;
        for (index, byte) in rom[0x1000..0x1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        rom
    }

    fn key_on(chip: &mut MultiPcm) {
        chip.write(0, 1, 0x00); // select slot 0
        chip.write(0, 2, 0x00); // register: pan/sample-high
        chip.write(0, 0, 0x00);
        chip.write(0, 2, 0x01); // register: sample number (loads header)
        chip.write(0, 0, 0x00);
        chip.write(0, 2, 0x03); // octave 0, fnum hi 0
        chip.write(0, 0, 0x00);
        chip.write(0, 2, 0x05); // full level
        chip.write(0, 0, 0x00);
        chip.write(0, 2, 0x04); // key on
        chip.write(0, 0, 0x80);
    }

    #[test]
    fn the_indirect_interface_reaches_a_voice() {
        let mut chip = MultiPcm::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x89, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert_eq!(chip.voices[0].start, 0x1000, "the header loaded");
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A key-off fades; the header's negated end field is honoured.
    #[test]
    fn keyoff_fades_and_the_end_field_decodes() {
        let mut chip = MultiPcm::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x89, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        assert_eq!(chip.voices[0].end, 0x100);
        chip.write(0, 2, 0x04);
        chip.write(0, 0, 0x00); // key off
        assert!(energy(&render(&mut chip, 60)) > 0, "the release fades");
        render(&mut chip, 200);
        assert_eq!(energy(&render(&mut chip, 100)), 0);
    }
}
