// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sega PCM (315-5218): sixteen channels of 8-bit PCM on Sega's Super
//! Scaler boards -- OutRun, After Burner, X-Board. 215 corpus files.
//!
//! **Route B, from the documented behaviour** (the register layout as MAME's
//! `segapcm.cpp`, BSD-3-Clause, documents it), so it lives in the permissive
//! crate.
//!
//! The chip's register file is a 256-byte RAM the driver writes directly,
//! and this core keeps it exactly that way: writes land in the RAM, and the
//! sixteen channels are *read out of it* at render time -- there is no
//! register decode to get wrong, only the readout. Addresses are 16.8 fixed
//! point (the fraction lives in a hidden per-channel latch), the end is a
//! page compare, and the bank register is folded through the header's
//! interface word (shift and mask), which [`configure`](ChipCore::configure)
//! supplies.

use crate::chip::ChipCore;

/// One frame per 128 clocks: 31.25 kHz at the usual 4 MHz.
const CLOCK_DIVIDER: u32 = 128;

/// The Sega PCM.
#[derive(Debug)]
pub struct SegaPcm {
    rate: u32,
    /// The register RAM, as the driver sees it.
    ram: [u8; 0x100],
    /// The hidden low byte of each channel's 16.8 address.
    fraction: [u8; 16],
    rom: Vec<u8>,
    bank_shift: u32,
    bank_mask: u32,
}

impl SegaPcm {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 31_250,
            ram: [0xFF; 0x100],
            fraction: [0; 16],
            rom: Vec::new(),
            bank_shift: 12,
            bank_mask: 0x70,
        }
    }
}

impl Default for SegaPcm {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for SegaPcm {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        let (bank_shift, bank_mask) = (self.bank_shift, self.bank_mask);
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            bank_shift,
            bank_mask,
            ..Self::new()
        };
        self.rom = rom;
    }

    /// The header's interface word: the bank shift in the low byte, the bank
    /// mask in bits 16-23 (zero meaning the classic `0x70`).
    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        let interface = settings.sega_pcm_interface;
        if interface != 0 {
            self.bank_shift = interface & 0xFF;
            let mask = (interface >> 16) & 0xFF;
            self.bank_mask = if mask == 0 { 0x70 } else { mask };
        }
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// A raw register-RAM write; the channels read it back at render time.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        self.ram[usize::from(addr & 0xFF)] = (data & 0xFF) as u8;
    }

    /// The sample ROM: block type `0x80`.
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
            for ch in 0..16usize {
                let regs = |offset: usize| u32::from(self.ram[8 * ch + offset]);
                // Flags: bit 0 stops the channel, bit 1 disables the loop.
                let flags = regs(0x86);
                if flags & 1 != 0 {
                    continue;
                }
                let mut addr =
                    (regs(0x85) << 16) | (regs(0x84) << 8) | u32::from(self.fraction[ch]);
                let loop_addr = (regs(0x05) << 16) | (regs(0x04) << 8);
                let end_page = regs(0x06) + 1;

                if addr >> 16 == end_page {
                    if flags & 2 != 0 {
                        // One-shot: the chip sets its own stop bit.
                        self.ram[8 * ch + 0x86] |= 1;
                        continue;
                    }
                    addr = loop_addr;
                }

                let bank = ((flags & self.bank_mask) >> 4) << self.bank_shift;
                let at = (bank as usize) + (addr >> 8) as usize;
                let Some(&byte) = self.rom.get(at) else {
                    self.ram[8 * ch + 0x86] |= 1;
                    continue;
                };
                let value = i32::from(byte as i8);
                left += (value * regs(0x02) as i32) >> 4;
                right += (value * regs(0x03) as i32) >> 4;

                addr = (addr + regs(0x07)) & 0x00FF_FFFF;
                self.fraction[ch] = (addr & 0xFF) as u8;
                self.ram[8 * ch + 0x84] = ((addr >> 8) & 0xFF) as u8;
                self.ram[8 * ch + 0x85] = ((addr >> 16) & 0xFF) as u8;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OutRun board's PCM clock.
    const CLOCK: u32 = 4_000_000;

    fn render(chip: &mut SegaPcm, frames: usize) -> Vec<(i32, i32)> {
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
        let mut rom = vec![0u8; 0x3000];
        for (index, byte) in rom[0x1000..0x2000].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        rom
    }

    /// Channel 0 keyed onto the sample at 0x1000, one byte a frame.
    fn key_on(chip: &mut SegaPcm) {
        chip.write(0, 0x02, 0x1F); // volume left
        chip.write(0, 0x03, 0x1F); // volume right
        chip.write(0, 0x84, 0x00); // address 0x1000 (16.8: page 0x10)
        chip.write(0, 0x85, 0x10);
        chip.write(0, 0x06, 0x1F); // end page 0x20
        chip.write(0, 0x07, 0xFF); // delta: just under one byte a frame
        chip.write(0, 0x86, 0x02); // run, loop disabled
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = SegaPcm::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x80, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "all stop bits at reset");
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A one-shot sets its own stop bit at the end page; with the loop
    /// enabled the same channel returns to its loop address instead.
    #[test]
    fn the_end_page_stops_or_loops() {
        let mut chip = SegaPcm::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x80, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x1100);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "one-shot must stop");
        assert_eq!(chip.ram[0x86] & 1, 1, "and the stop bit is readable");

        chip.write(0, 0x04, 0x00); // loop to 0x1000
        chip.write(0, 0x05, 0x10);
        chip.write(0, 0x84, 0x00);
        chip.write(0, 0x85, 0x10);
        chip.write(0, 0x86, 0x00); // run, loop enabled
        render(&mut chip, 0x1100);
        assert!(energy(&render(&mut chip, 200)) > 0, "the loop must sustain");
    }

    /// The interface word folds the bank bits: OutRun's shift-12 layout
    /// finds a byte at bank 1 where the default would not.
    #[test]
    fn the_interface_word_places_the_bank() {
        let mut chip = SegaPcm::new();
        chip.reset(CLOCK, false);
        chip.configure(&dro_core::vgm::ChipSettings {
            sega_pcm_interface: 0x00F8_000C,
            ..Default::default()
        });
        assert_eq!(chip.bank_shift, 0x0C);
        assert_eq!(chip.bank_mask, 0xF8);
    }
}
