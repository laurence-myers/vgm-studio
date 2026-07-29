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
//! sample fetches for rips of banked boards -- Sega's two 512 KiB latches,
//! which Model 1 drives as one 1 MB window.
//!
//! Stated approximations, as the OPL4 core's: the envelope is instant
//! attack, sustain at total level, fast fade on key-off; the LFO is
//! unmodelled; pan is linear.

use dro_core::vgm::stream::BANK_PORT;

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
    /// The `0xC3` bank offsets in bytes, one per 512 KiB half: `bank_low`
    /// serves addresses with bit 19 clear, `bank_high` those with it set.
    ///
    /// Two of them because the command sets them independently -- Sega Multi 32
    /// wires the halves to separate bank latches, and 96 of the corpus's 296
    /// bank commands name one half without the other. A 1 MB bank select
    /// (Model 1) is the case where they are one window, and it just sets both.
    bank_low: u32,
    bank_high: u32,
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
            // The power-on default is the 1 MB window at `0x18_0000` -- the
            // pair a `0xC3` of that offset would set. See `set_bank`.
            bank_low: 0x18_0000,
            bank_high: 0x20_0000,
            rom: Vec::new(),
        }
    }

    /// The `0xC3` bank select: `mask` names the halves, `offset` is in 64 KiB
    /// units.
    ///
    /// The three cases are upstream's `Cmd_YMW_Bank`, which is where the mask's
    /// bit order comes from: **bit 1 is the low half, bit 0 the high one**.
    /// Both halves at a megabyte-aligned offset is Model 1's single 1 MB
    /// window, so the high half sits half a megabyte above the low one; every
    /// other mask moves the halves it names to the same place.
    fn set_bank(&mut self, mask: u16, offset: u16) {
        let base = u32::from(offset) << 16;
        if mask & 0x03 == 0x03 && base & 0x08_0000 == 0 {
            self.bank_low = base;
            self.bank_high = base | 0x08_0000;
            return;
        }
        if mask & 0x02 != 0 {
            self.bank_low = base;
        }
        if mask & 0x01 != 0 {
            self.bank_high = base;
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
    /// eighth encoding, the chip's 28-of-32 map. `0xC3` arrives on
    /// [`BANK_PORT`] with the mask in `addr` and the offset in `data`.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        if port == BANK_PORT {
            self.set_bank(addr, data);
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
                // Banking, as MAME's `multipcm.cpp` documents it: only
                // addresses with bit 20 set are banked, and bit 19 chooses
                // which half's latch replaces the top of the address. The
                // corpus agrees at both ends -- Daytona writes a 1 MB bank of
                // 0x100000 and every sample decodes into its delivered ROM
                // regions, OutRunners writes no bank at all and its high
                // samples land only under the power-on default, and both rips'
                // low samples decode at face value unbanked.
                let mut address = voice.start + sample_index;
                if address & 0x10_0000 != 0 {
                    let bank = if address & 0x08_0000 == 0 {
                        self.bank_low
                    } else {
                        self.bank_high
                    };
                    address = (address & 0x07_FFFF) | bank;
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
                    scaled = (scaled * i32::from(voice.release)) >> 8;
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
        key_on_sample(chip, 0x00);
    }

    fn key_on_sample(chip: &mut MultiPcm, sample: u16) {
        chip.write(0, 1, 0x00); // select slot 0
        chip.write(0, 2, 0x00); // register: pan/sample-high
        chip.write(0, 0, 0x00);
        chip.write(0, 2, 0x01); // register: sample number (loads header)
        chip.write(0, 0, sample);
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

    /// The `0xC3` bank select, case for case against upstream's
    /// `Cmd_YMW_Bank` -- which is the only place the mask's bit order is
    /// written down.
    #[test]
    fn the_bank_select_moves_the_half_its_mask_names() {
        let mut chip = MultiPcm::new();

        // Both banks at a megabyte-aligned offset: one 1 MB window, so the
        // high half follows half a megabyte above the low one.
        chip.set_bank(0x03, 0x0010);
        assert_eq!((chip.bank_low, chip.bank_high), (0x10_0000, 0x18_0000));

        // Half a megabyte in, which one window cannot express: both halves go
        // to the same place instead, exactly as upstream's second branch does.
        chip.set_bank(0x03, 0x0018);
        assert_eq!((chip.bank_low, chip.bank_high), (0x18_0000, 0x18_0000));

        // Bit 1 is the low half and bit 0 the high one -- the way round
        // `Cmd_YMW_Bank` has it, and the opposite of how the bits read.
        chip.set_bank(0x02, 0x0020);
        assert_eq!((chip.bank_low, chip.bank_high), (0x20_0000, 0x18_0000));
        chip.set_bank(0x01, 0x0028);
        assert_eq!((chip.bank_low, chip.bank_high), (0x20_0000, 0x28_0000));

        // A mask naming no half moves nothing.
        chip.set_bank(0x00, 0x0000);
        assert_eq!((chip.bank_low, chip.bank_high), (0x20_0000, 0x28_0000));
    }

    /// Splitting one bank into two must not move a single fetch on the rips
    /// this core was calibrated against.
    ///
    /// Those never send a `0xC3` at all, so they run on the power-on pair, and
    /// the pair is chosen to reproduce the single `0x18_0000` bank the core
    /// held until 2026-07-29 -- which masked twenty bits and *added*, where
    /// this masks nineteen and ORs the half the address selects. The two agree
    /// for every address, and this is that proof rather than a promise.
    #[test]
    fn the_power_on_banks_fetch_where_the_single_bank_did() {
        let chip = MultiPcm::new();
        assert_eq!((chip.bank_low, chip.bank_high), (0x18_0000, 0x20_0000));
        for address in (0..0x40_0000u32).step_by(0x111) {
            let was = if address & 0x10_0000 == 0 {
                address
            } else {
                (address & 0x0F_FFFF) + 0x18_0000
            };
            let now = if address & 0x10_0000 == 0 {
                address
            } else if address & 0x08_0000 == 0 {
                (address & 0x07_FFFF) | chip.bank_low
            } else {
                (address & 0x07_FFFF) | chip.bank_high
            };
            assert_eq!(was, now, "{address:#08X}");
        }
    }

    /// Banking is fetch-time, and only for addresses above the megabyte line:
    /// a bank select must move the samples it names and leave the rest alone.
    #[test]
    fn only_addresses_above_the_megabyte_line_are_banked() {
        // Sample 0 lives at 0x1000, below the line; sample 1 at 0x10_1000,
        // above it. Only the waveform at 0x20_1000 is filled in, so sample 1
        // is audible exactly when the low bank points there.
        let mut sample_rom = rom();
        sample_rom.resize(0x20_2000, 0);
        for (index, byte) in sample_rom[0x20_1000..0x20_1100].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        sample_rom[12] = 0x10;
        sample_rom[13] = 0x10;
        sample_rom[14] = 0x00;
        let end_field = 0x1_0000u32 - 0x100;
        sample_rom[17] = (end_field >> 8) as u8;
        sample_rom[18] = (end_field & 0xFF) as u8;

        let start = |bank: Option<u16>| {
            let mut chip = MultiPcm::new();
            chip.reset(CLOCK, false);
            chip.load_rom(0x89, sample_rom.len() as u32, 0, &sample_rom);
            if let Some(offset) = bank {
                chip.write(BANK_PORT, 0x02, offset);
            }
            chip
        };

        // Below the line: audible whatever the bank says.
        let mut chip = start(None);
        key_on_sample(&mut chip, 0x00);
        assert_eq!(chip.voices[0].start, 0x1000);
        assert!(energy(&render(&mut chip, 200)) > 0);

        // Above it, and the power-on window sends it to 0x18_1000, which is
        // zeroes.
        let mut chip = start(None);
        key_on_sample(&mut chip, 0x01);
        assert_eq!(chip.voices[0].start, 0x10_1000, "the header is not banked");
        assert_eq!(energy(&render(&mut chip, 200)), 0);

        // Bank the low half to 0x20_0000 and the same voice finds its waveform.
        let mut chip = start(Some(0x0020));
        key_on_sample(&mut chip, 0x01);
        assert!(energy(&render(&mut chip, 200)) > 0, "0x20_1000 holds it");

        // ...and the *high* half is a different latch, so naming it instead
        // leaves this fetch where it was.
        let mut chip = start(None);
        chip.write(BANK_PORT, 0x01, 0x0020);
        key_on_sample(&mut chip, 0x01);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "bit 19 is clear here");
    }
}
