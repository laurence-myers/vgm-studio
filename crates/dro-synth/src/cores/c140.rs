// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Namco C140 (and, approximately, the C219): 24 PCM voices from ROM.
//!
//! 639 files in the VGMRips corpus -- Namco System 2 and System 21 arcade
//! boards (Assault, Starblade, the Final Lap line), with the C219 variant
//! carrying the NA-1/NA-2 games. Twenty-four voices of 8-bit linear or
//! mu-law-compressed PCM, each with per-side volumes, a 16-bit frequency,
//! and loop points.
//!
//! **Route B, from the documented behaviour** (the register map and the
//! compression segment table as MAME's `c140.cpp`, BSD-3-Clause, documents
//! them), so it lives in the permissive crate.
//!
//! # Banking is board-shaped
//!
//! The chip emits more address than its socket wires up, and each Namco
//! board rearranged the upper lines its own way. The VGM header carries
//! which board (`c140_type`): System 2 folds bit 21 down by two, System 21
//! by one. The C219's own scheme (group bank registers at `0x1F0`-`0x1F7`)
//! is **approximated as linear addressing** here -- a stated gap, carried
//! until a corpus file proves it matters audibly.

use crate::chip::ChipCore;

/// The mu-law segment bases: eight segments, each twice the last, exactly
/// the table MAME's implementation builds. Regenerated in a test.
const SEGMENT_BASE: [i32; 8] = [0, 16, 48, 112, 240, 496, 1008, 2032];

/// Decodes one compressed sample byte to a 12-bit-ish linear value.
const fn mulaw(byte: u8) -> i32 {
    let magnitude = (byte & 0x7F) as i32;
    let segment = (magnitude >> 4) as usize;
    let value = SEGMENT_BASE[segment] + ((magnitude & 0x0F) << segment);
    if byte & 0x80 != 0 { -value } else { value }
}

/// Which board's address fold to apply.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Banking {
    #[default]
    System2,
    System21,
    /// The C219's own scheme, approximated as linear.
    C219,
}

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    right_volume: u8,
    left_volume: u8,
    /// 16-bit: `0x10000` would be one sample per output frame; the chip's
    /// registers only reach `0xFFFF`, so everything plays slightly under
    /// the base rate.
    frequency: u16,
    bank: u8,
    mode: u8,
    start: u16,
    end: u16,
    loop_start: u16,
    /// Sample position from the start address, 16.16.
    position: u32,
    playing: bool,
}

impl Voice {
    fn compressed(&self) -> bool {
        self.mode & 0x08 != 0
    }

    fn looping(&self) -> bool {
        self.mode & 0x10 != 0
    }
}

/// The C140 / C219.
#[derive(Debug)]
pub struct C140 {
    rate: u32,
    banking: Banking,
    voices: [Voice; 24],
    rom: Vec<u8>,
}

impl C140 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 21_390,
            banking: Banking::System2,
            voices: [Voice::default(); 24],
            rom: Vec::new(),
        }
    }

    /// A voice's base byte address through the board's fold.
    fn address(&self, bank: u8, offset: u16) -> usize {
        let raw = (u32::from(bank) << 16) | u32::from(offset);
        (match self.banking {
            Banking::System2 => ((raw & 0x20_0000) >> 2) | (raw & 0x07_FFFF),
            Banking::System21 => ((raw & 0x30_0000) >> 1) | (raw & 0x07_FFFF),
            Banking::C219 => raw,
        }) as usize
    }
}

impl Default for C140 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for C140 {
    /// `variant` is the header's bit 31: the C219. The finer distinction --
    /// System 2 against System 21 -- arrives in
    /// [`configure`](ChipCore::configure).
    fn reset(&mut self, clock: u32, variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        *self = Self {
            // The header's C140 "clock" is the sample rate itself (the spec
            // says "usually 21390"); guard against files that store a real
            // crystal by dividing one down.
            rate: if clock < 1_000_000 {
                clock.max(1)
            } else {
                (clock / 288).max(1)
            },
            banking: if variant {
                Banking::C219
            } else {
                Banking::System2
            },
            ..Self::new()
        };
        self.rom = rom;
    }

    /// The header's `c140_type` byte: 0 System 2, 1 System 21, 2 C219.
    fn configure(&mut self, settings: &dro_core::vgm::ChipSettings) {
        self.banking = match settings.c140_type {
            1 => Banking::System21,
            2 => Banking::C219,
            _ => {
                // Keep a variant-flagged C219 even if the type byte is 0 --
                // older files set only the clock bit.
                if self.banking == Banking::C219 {
                    Banking::C219
                } else {
                    Banking::System2
                }
            }
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The VGM's `0xD4 pp rr dd`: the register index is `pp << 8 | rr`.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let reg = (u16::from(port) << 8) | (addr & 0xFF);
        let data = (data & 0xFF) as u8;
        // 24 voices, 16 registers each; the block above them is the C219's
        // bank file, unmodelled.
        if reg >= 0x180 {
            return;
        }
        let voice = &mut self.voices[usize::from(reg >> 4)];
        match reg & 0x0F {
            0x00 => voice.right_volume = data,
            0x01 => voice.left_volume = data,
            0x02 => voice.frequency = (voice.frequency & 0x00FF) | (u16::from(data) << 8),
            0x03 => voice.frequency = (voice.frequency & 0xFF00) | u16::from(data),
            0x04 => voice.bank = data,
            // The mode register is also the key: bit 7 starts the voice
            // from its start address, clear stops it.
            0x05 => {
                voice.mode = data;
                if data & 0x80 != 0 {
                    voice.position = 0;
                    voice.playing = true;
                } else {
                    voice.playing = false;
                }
            }
            0x06 => voice.start = (voice.start & 0x00FF) | (u16::from(data) << 8),
            0x07 => voice.start = (voice.start & 0xFF00) | u16::from(data),
            0x08 => voice.end = (voice.end & 0x00FF) | (u16::from(data) << 8),
            0x09 => voice.end = (voice.end & 0xFF00) | u16::from(data),
            0x0A => voice.loop_start = (voice.loop_start & 0x00FF) | (u16::from(data) << 8),
            0x0B => voice.loop_start = (voice.loop_start & 0xFF00) | u16::from(data),
            _ => {}
        }
    }

    /// The sample ROM: block type `0x8D`.
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
                if !voice.playing {
                    continue;
                }
                let length = u32::from(voice.end.saturating_sub(voice.start));
                let sample_index = voice.position >> 16;
                if sample_index >= length.max(1) {
                    // The end: loop back, or stop.
                    let voice = &mut self.voices[index];
                    if voice.looping() {
                        voice.position = 0;
                        voice.start = voice.loop_start;
                        continue;
                    }
                    voice.playing = false;
                    continue;
                }
                let base = self.address(voice.bank, voice.start);
                let Some(&byte) = self.rom.get(base + sample_index as usize) else {
                    self.voices[index].playing = false;
                    continue;
                };
                let value = if voice.compressed() {
                    mulaw(byte)
                } else {
                    i32::from(byte as i8) << 4
                };
                self.voices[index].position =
                    voice.position.wrapping_add(u32::from(voice.frequency));
                left += (value * i32::from(voice.left_volume)) >> 8;
                right += (value * i32::from(voice.right_volume)) >> 8;
            }
            // One voice at full scale lands near +-2k; the x4 puts it at the
            // ~8k one-channel headroom the other cores use.
            frame[0] = left * 4;
            frame[1] = right * 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate the spec says C140 headers usually carry.
    const CLOCK: u32 = 21_390;

    fn render(chip: &mut C140, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut C140) {
        chip.write(0, 0x00, 0xFF); // both sides full
        chip.write(0, 0x01, 0xFF);
        chip.write(0, 0x02, 0xFF); // frequency just under 1:1
        chip.write(0, 0x03, 0xFF);
        chip.write(0, 0x04, 0x00); // bank 0
        chip.write(0, 0x06, 0x10); // start 0x1000
        chip.write(0, 0x07, 0x00);
        chip.write(0, 0x08, 0x11); // end 0x1100
        chip.write(0, 0x09, 0x00);
        chip.write(0, 0x05, 0x80); // key on, linear, no loop
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = C140::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8D, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A one-shot stops at its end register; a looped voice keeps sounding.
    #[test]
    fn the_end_register_stops_and_the_loop_bit_sustains() {
        let mut chip = C140::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8D, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x180); // run past the end
        assert_eq!(energy(&render(&mut chip, 200)), 0, "a one-shot must stop");

        chip.write(0, 0x0A, 0x10); // loop back to the start
        chip.write(0, 0x0B, 0x00);
        chip.write(0, 0x05, 0x90); // key on, looping
        render(&mut chip, 0x400);
        assert!(
            energy(&render(&mut chip, 200)) > 0,
            "a looped voice must keep sounding"
        );
    }

    /// The mu-law segment table against its construction, and the decode's
    /// polarity: bit 7 is the negative half.
    #[test]
    fn the_mulaw_table_doubles_per_segment() {
        let mut base = 0;
        for (index, &entry) in SEGMENT_BASE.iter().enumerate() {
            assert_eq!(entry, base, "segment {index}");
            base += 16 << index;
        }
        assert_eq!(mulaw(0x00), 0);
        assert_eq!(mulaw(0x7F), 2032 + (15 << 7));
        assert_eq!(mulaw(0xFF), -(2032 + (15 << 7)));
        // Compressed mode actually reaches the decode.
        let mut chip = C140::new();
        chip.reset(CLOCK, false);
        let mut sample_rom = vec![0u8; 0x2000];
        sample_rom[0x1000..0x1100].fill(0x7F); // loudest positive mu-law code
        chip.load_rom(0x8D, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        chip.write(0, 0x05, 0x88); // key on, compressed
        let peak = render(&mut chip, 50)
            .iter()
            .map(|&(l, _)| l)
            .max()
            .unwrap_or(0);
        assert!(peak > 8000, "the loudest mu-law code must be loud: {peak}");
    }

    /// The System 21 fold moves bit 21 down by one; System 2 by two. A ROM
    /// byte placed where only the folded address finds it proves which fold
    /// ran.
    #[test]
    fn the_board_type_selects_the_address_fold() {
        let mut chip = C140::new();
        chip.reset(CLOCK, false);
        chip.configure(&dro_core::vgm::ChipSettings {
            c140_type: 1, // System 21
            ..Default::default()
        });
        // bank 0x20 -> raw 0x200000; System 21 folds it to 0x100000.
        assert_eq!(chip.address(0x20, 0x0000), 0x10_0000);
        chip.configure(&dro_core::vgm::ChipSettings {
            c140_type: 0,
            ..Default::default()
        });
        assert_eq!(chip.address(0x20, 0x0000), 0x08_0000);
    }
}
