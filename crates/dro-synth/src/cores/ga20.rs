// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Irem GA20: four channels of 8-bit PCM on the M-92 and M-107 boards
//! (In the Hunt, R-Type Leo, Air Assault). 136 corpus files.
//!
//! **Route B, from the documented behaviour** (the register map as MAME's
//! `iremga20.cpp`, BSD-3-Clause, documents it), so it lives in the
//! permissive crate.
//!
//! The chip's two defining habits: samples are **unsigned** 8-bit with the
//! byte `0x00` as the end marker (there are no loops -- the driver replays),
//! and the rate register is a divider, `clock/4 / (256 - rate)`.

use crate::chip::ChipCore;

/// The core renders one frame per 64 clocks; each channel's own rate is a
/// fraction of the chip's `clock / 4` against its divider.
const CLOCK_DIVIDER: u32 = 64;

/// One channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// Byte addresses, written in 16-byte units.
    start: u32,
    end: u32,
    /// The rate register: a divider against `clock / 4`.
    rate: u8,
    volume: u8,
    /// Position in 16.16 bytes.
    position: u32,
    playing: bool,
}

impl Channel {
    /// The 16.16 step per output frame: 16 chip ticks a frame, each moving
    /// `1 / (256 - rate)` bytes.
    fn step(&self) -> u32 {
        (16 << 16) / (256 - u32::from(self.rate))
    }
}

/// The GA20.
#[derive(Debug)]
pub struct Ga20 {
    rate: u32,
    channels: [Channel; 4],
    rom: Vec<u8>,
}

impl Ga20 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 55_930,
            channels: [Channel::default(); 4],
            rom: Vec::new(),
        }
    }
}

impl Default for Ga20 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ga20 {
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

    /// Eight registers a channel: start and end in 16-byte units, the rate
    /// divider, volume, and the control register whose bit 1 keys.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let ch = &mut self.channels[usize::from((addr >> 3) & 0x03)];
        let data = (data & 0xFF) as u8;
        match addr & 0x07 {
            0x00 => ch.start = (ch.start & 0xFF000) | (u32::from(data) << 4),
            0x01 => ch.start = (ch.start & 0x00FF0) | (u32::from(data) << 12),
            0x02 => ch.end = (ch.end & 0xFF000) | (u32::from(data) << 4),
            0x03 => ch.end = (ch.end & 0x00FF0) | (u32::from(data) << 12),
            0x04 => ch.rate = data,
            0x05 => ch.volume = data,
            0x06 => {
                if data & 0x02 != 0 {
                    ch.position = ch.start << 16;
                    ch.playing = true;
                } else {
                    ch.playing = false;
                }
            }
            _ => {}
        }
    }

    /// The sample ROM: block type `0x93`.
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
            let mut sum = 0i32;
            for ch in &mut self.channels {
                if !ch.playing {
                    continue;
                }
                let at = (ch.position >> 16) as usize;
                if at >= (ch.end as usize).min(self.rom.len()) {
                    ch.playing = false;
                    continue;
                }
                let byte = self.rom[at];
                if byte == 0x00 {
                    // The end marker: no loops on this chip, the driver
                    // replays.
                    ch.playing = false;
                    continue;
                }
                // Unsigned samples, centred here.
                let value = i32::from(byte) - 0x80;
                sum += (value * i32::from(ch.volume)) >> 4;
                ch.position = ch.position.wrapping_add(ch.step());
            }
            // Mono into both sides: the boards mix to one speaker path.
            frame[0] = sum;
            frame[1] = sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The M-92's GA20 clock.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut Ga20, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    fn rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x3000];
        for (index, byte) in rom[0x1000..0x2000].iter_mut().enumerate() {
            // Alternating extremes, avoiding the 0x00 end marker.
            *byte = if index % 2 == 0 { 0xFF } else { 0x01 };
        }
        rom
    }

    fn key_on(chip: &mut Ga20) {
        chip.write(0, 0x00, 0x00); // start 0x1000 = unit 0x0100
        chip.write(0, 0x01, 0x01);
        chip.write(0, 0x02, 0x00); // end 0x2000 = unit 0x0200
        chip.write(0, 0x03, 0x02);
        chip.write(0, 0x04, 0xF0); // divider 16: one byte a frame
        chip.write(0, 0x05, 0xFF); // full volume
        chip.write(0, 0x06, 0x02); // key on
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = Ga20::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x93, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// **`0x00` is the end marker.** A marker mid-sample stops the channel
    /// early; the end register stops it regardless.
    #[test]
    fn the_zero_byte_ends_a_sample() {
        let mut chip = Ga20::new();
        chip.reset(CLOCK, false);
        let mut sample_rom = rom();
        sample_rom[0x1080] = 0x00; // an early marker
        chip.load_rom(0x93, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x100); // past the marker, well short of the end
        assert_eq!(
            energy(&render(&mut chip, 100)),
            0,
            "the marker must stop the channel"
        );
    }

    /// The rate register is a divider: 0xF8 plays twice as fast as 0xF0.
    #[test]
    fn the_rate_register_divides() {
        let a = Channel {
            rate: 0xF0,
            ..Channel::default()
        };
        let b = Channel {
            rate: 0xF8,
            ..Channel::default()
        };
        assert_eq!(b.step(), a.step() * 2);
    }
}
