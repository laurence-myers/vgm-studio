// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Yamaha Y8950 (MSX-AUDIO): an OPL with a Delta-T sample channel bolted
//! on.
//!
//! 268 files in the VGMRips corpus on its own account -- and the missing
//! half of a few hundred more, because MSX rips pair it with the SCC. The
//! FM side *is* the YM3526's register file (the OPL2's, less the waveform
//! select), and the sample side is the same ADPCM-B scheme the YM2608 and
//! YM2610 carry -- which is why this core is mostly two existing pieces
//! joined: the Nuked-OPL3 port that already serves the OPL path, run in its
//! OPL2-compatible register range, and [`DeltaT`](crate::adpcm::DeltaT).
//!
//! Behind the `nuked-opl` feature with the rest of the OPL machinery: the
//! port is LGPL, and a permissive-only build loses this core along with the
//! OPL path it rides on.
//!
//! **Written but not yet registered.** The Y8950 is one of the OPL chips,
//! and the registry's standing invariant is that OPL is listed but never
//! buildable for `VgmEngine` -- OPL documents route through `PlayerEngine`,
//! and every `playability` caller leans on that. Registering this core
//! without auditing that routing would send Y8950 files down two paths at
//! once. The audit is its own step; until then this core's tests keep it
//! honest.
//!
//! Not modelled: the keyboard/I-O interface (registers `0x18`-`0x19` carry
//! no audio), ADPCM recording, and the CSM mode no rip uses.

use crate::adpcm::DeltaT;
use crate::chip::ChipCore;
use crate::opl::{NukedOpl3, OplChip};

/// The OPL family's frame: one sample per 72 clocks.
const CLOCK_DIVIDER: u32 = 72;

/// The Delta-T section, the same shape as the OPN family's.
#[derive(Debug, Default)]
struct DeltaTSection {
    decoder: DeltaT,
    playing: bool,
    repeat: bool,
    /// Start and stop in the chip's 4-byte units, as the registers hold
    /// them; byte addresses are derived at use.
    start_units: u16,
    stop_units: u16,
    position: u32,
    high_nibble: bool,
    /// 16.16 nibble pacing.
    fraction: u32,
    delta_n: u16,
    /// The level register: a linear 8-bit volume.
    level: u8,
    /// The last decoded sample, held between nibbles.
    held: i32,
}

/// The Y8950.
#[derive(Debug)]
pub struct Y8950 {
    opl: NukedOpl3,
    rate: u32,
    delta: DeltaTSection,
    rom: Vec<u8>,
    /// The address register the two-write OPL interface has latched.
    latched: u8,
}

impl Y8950 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            opl: NukedOpl3::new(49_716),
            rate: 49_716,
            delta: DeltaTSection::default(),
            rom: Vec::new(),
            latched: 0,
        }
    }

    /// One ADPCM register write, the YM2608 Delta-T register shape at
    /// `0x07`-`0x12`.
    fn write_adpcm(&mut self, reg: u8, data: u8) {
        let delta = &mut self.delta;
        match reg {
            0x07 => {
                // Control 1: bit 7 start, bit 4 repeat, bit 0 reset.
                delta.repeat = data & 0x10 != 0;
                if data & 0x80 != 0 {
                    delta.playing = true;
                    delta.position = u32::from(delta.start_units) << 2;
                    delta.high_nibble = true;
                    delta.fraction = 0;
                    delta.decoder.restart();
                } else if data & 0x01 != 0 {
                    delta.playing = false;
                }
            }
            // Control 2 selects the memory arrangement; the VGM delivers a
            // flat image, so only the address unit depends on it, and the
            // corpus agrees with the 4-byte unit used here.
            0x09 => delta.start_units = (delta.start_units & 0xFF00) | u16::from(data),
            0x0A => delta.start_units = (delta.start_units & 0x00FF) | (u16::from(data) << 8),
            0x0B => delta.stop_units = (delta.stop_units & 0xFF00) | u16::from(data),
            0x0C => delta.stop_units = (delta.stop_units & 0x00FF) | (u16::from(data) << 8),
            0x10 => delta.delta_n = (delta.delta_n & 0xFF00) | u16::from(data),
            0x11 => delta.delta_n = (delta.delta_n & 0x00FF) | (u16::from(data) << 8),
            0x12 => delta.level = data,
            _ => {}
        }
    }
}

impl Default for Y8950 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Y8950 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let rom = std::mem::take(&mut self.rom);
        let rate = (clock / CLOCK_DIVIDER).max(1);
        *self = Self {
            opl: NukedOpl3::new(rate),
            rate,
            ..Self::new()
        };
        self.rom = rom;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The engine hands the register and value in one call; the ADPCM range
    /// peels off to the sample section and everything else is OPL.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let reg = (addr & 0xFF) as u8;
        let data = (data & 0xFF) as u8;
        self.latched = reg;
        if (0x07..=0x12).contains(&reg) {
            self.write_adpcm(reg, data);
        } else {
            self.opl.write_reg_buffered(u16::from(reg), data);
        }
    }

    /// The sample memory: block type `0x88`.
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
            let mut opl_frame = [0i16; 2];
            self.opl.generate_samples(&mut opl_frame);

            let delta = &mut self.delta;
            if delta.playing {
                // Nibble pacing: delta-N against a 64k base, one nibble per
                // overflow.
                delta.fraction = delta.fraction.wrapping_add(u32::from(delta.delta_n));
                while delta.fraction >= 0x10000 {
                    delta.fraction -= 0x10000;
                    let Some(&byte) = self.rom.get(delta.position as usize) else {
                        delta.playing = false;
                        break;
                    };
                    let nibble = if delta.high_nibble {
                        byte >> 4
                    } else {
                        byte & 0x0F
                    };
                    delta.held = delta.decoder.decode(nibble);
                    if delta.high_nibble {
                        delta.high_nibble = false;
                    } else {
                        delta.high_nibble = true;
                        delta.position += 1;
                        if delta.position > (u32::from(delta.stop_units) << 2) + 3 {
                            if delta.repeat {
                                delta.position = u32::from(delta.start_units) << 2;
                                delta.decoder.restart();
                            } else {
                                delta.playing = false;
                            }
                        }
                    }
                }
            }
            let sample = if self.delta.playing {
                (self.delta.held * i32::from(self.delta.level)) >> 9
            } else {
                0
            };

            frame[0] = i32::from(opl_frame[0]) + sample;
            frame[1] = i32::from(opl_frame[1]) + sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MSX-AUDIO clock.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Y8950, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// A loud FM note through the OPL half.
    fn fm_key_on(chip: &mut Y8950) {
        for (reg, value) in [
            (0x20u16, 0x01u16), // modulator multiple 1
            (0x23, 0x01),       // carrier multiple 1
            (0x40, 0x3F),       // modulator quiet: nearly a sine from the carrier
            (0x43, 0x00),       // carrier loudest
            (0x60, 0xF0),       // fast attack
            (0x63, 0xF0),
            (0x80, 0x0F),
            (0x83, 0x0F),
            (0xA0, 0x69), // F-number low
            (0xB0, 0x2A), // key on, block 2, F-number high
        ] {
            chip.write(0, reg, value);
        }
    }

    #[test]
    fn the_fm_half_answers_opl_registers() {
        let mut chip = Y8950::new();
        chip.reset(CLOCK, false);
        let quiet = energy(&render(&mut chip, 500));
        fm_key_on(&mut chip);
        let loud = energy(&render(&mut chip, 4000));
        assert!(
            loud > quiet * 4 && loud > 10_000,
            "the OPL half must sound: loud={loud} quiet={quiet}"
        );
    }

    /// The ADPCM half plays a sample from the block-delivered memory, at
    /// the level register's volume, and stops at its stop address.
    #[test]
    fn the_adpcm_half_plays_and_stops() {
        let mut chip = Y8950::new();
        chip.reset(CLOCK, false);
        let mut rom = vec![0u8; 0x400];
        for (index, byte) in rom[0x100..0x200].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        chip.load_rom(0x88, rom.len() as u32, 0, &rom);

        chip.write(0, 0x09, 0x40); // start 0x100 in 4-byte units: 0x0040
        chip.write(0, 0x0A, 0x00);
        chip.write(0, 0x0B, 0x80); // stop 0x200: 0x0080 units
        chip.write(0, 0x0C, 0x00);
        chip.write(0, 0x10, 0x00); // delta-N 0x8000: a nibble every other frame
        chip.write(0, 0x11, 0x80);
        chip.write(0, 0x12, 0xFF); // full level
        chip.write(0, 0x07, 0x80); // start

        let playing = energy(&render(&mut chip, 600));
        assert!(playing > 0, "the sample must sound: {playing}");

        // 512 nibbles at half a nibble a frame: finished within 1200.
        render(&mut chip, 800);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "a one-shot must end");
    }
}
