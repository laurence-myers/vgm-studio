// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Ensoniq ES5503 DOC: the Apple IIgs's 32 oscillators reading wave RAM.
//! 145 corpus files.
//!
//! **Route B, from the documented behaviour** (the Apple IIgs hardware
//! reference and Ensoniq's datasheet, as MAME's `es5503.cpp` also records
//! them), so it lives in the permissive crate.
//!
//! Oscillators read 8-bit **unsigned** samples from a shared RAM the VGM
//! uploads through `0xE1`-type RAM blocks, and -- the chip's signature --
//! **a zero byte halts the oscillator**: silence is encoded in the wave
//! data itself. The scan rate divides among the enabled oscillators, so
//! enabling more of them lowers everyone's pitch; the oscillator-enable
//! register is honoured for exactly that reason.
//!
//! Stated approximations: swap mode plays as loop (the paired-oscillator
//! handoff is not modelled), and the resolution field scales the
//! accumulator shift without the fine interpolation the silicon lacks
//! anyway.

use crate::chip::ChipCore;

/// One oscillator.
#[derive(Debug, Default, Clone, Copy)]
struct Oscillator {
    /// Sixteen bits of frequency accumulator step.
    frequency: u16,
    volume: u8,
    /// The wave table page: address bits 8-15.
    page: u8,
    /// Control: bit 0 halts; bits 1-2 are the mode.
    halted: bool,
    one_shot: bool,
    /// Table size select: `256 << size` bytes.
    size: u8,
    resolution: u8,
    /// The 24-bit accumulator.
    accumulator: u32,
}

/// The ES5503.
#[derive(Debug)]
pub struct Es5503 {
    rate: u32,
    clock: u32,
    oscillators: [Oscillator; 32],
    /// How many oscillators the enable register turns on.
    enabled: u8,
    ram: Vec<u8>,
}

impl Es5503 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 26_320,
            clock: 7_159_090,
            oscillators: [Oscillator {
                halted: true,
                ..Oscillator::default()
            }; 32],
            enabled: 32,
            ram: vec![0; 0x20000],
        }
    }

    fn update_rate(&mut self) {
        // The documented scan: clock / 8 / (enabled + 2).
        self.rate = (self.clock / 8 / (u32::from(self.enabled) + 2)).max(1);
    }
}

impl Default for Es5503 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Es5503 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let ram = std::mem::take(&mut self.ram);
        *self = Self {
            clock,
            ..Self::new()
        };
        self.ram = ram;
        self.update_rate();
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The register file, `port << 8 | addr` from the `0xD5` command.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let reg = (u16::from(port) << 8) | (addr & 0xFF);
        let data = (data & 0xFF) as u8;
        let osc = usize::from(reg & 0x1F);
        match reg {
            0x00..=0x1F => {
                let o = &mut self.oscillators[osc];
                o.frequency = (o.frequency & 0xFF00) | u16::from(data);
            }
            0x20..=0x3F => {
                let o = &mut self.oscillators[osc];
                o.frequency = (o.frequency & 0x00FF) | (u16::from(data) << 8);
            }
            0x40..=0x5F => self.oscillators[osc].volume = data,
            0x80..=0x9F => self.oscillators[osc].page = data,
            0xA0..=0xBF => {
                let o = &mut self.oscillators[osc];
                let was_halted = o.halted;
                o.halted = data & 0x01 != 0;
                o.one_shot = data & 0x02 != 0;
                if was_halted && !o.halted {
                    o.accumulator = 0;
                }
            }
            0xC0..=0xDF => {
                let o = &mut self.oscillators[osc];
                o.size = (data >> 3) & 0x07;
                o.resolution = data & 0x07;
            }
            // 0xE1: the oscillator-enable register: `(count - 1) * 2`.
            0x1E1 | 0xE1 => {
                self.enabled = ((data >> 1) + 1).min(32);
                self.update_rate();
            }
            _ => {}
        }
    }

    /// Wave RAM arrives as `0xE1`-type RAM-write blocks.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        let at = offset as usize;
        for (index, &byte) in data.iter().enumerate() {
            let Some(slot) = self.ram.get_mut(at + index) else {
                break;
            };
            *slot = byte;
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sum = 0i32;
            for o in &mut self.oscillators {
                if o.halted {
                    continue;
                }
                let size_bits = 8 + u32::from(o.size);
                let shift = (24 - size_bits).saturating_sub(u32::from(o.resolution).min(7));
                let index = (o.accumulator >> shift) & ((1 << size_bits) - 1);
                let address = (usize::from(o.page) << 8) + index as usize;
                let byte = self.ram.get(address).copied().unwrap_or(0);
                if byte == 0x00 {
                    // The zero byte: the DOC's own stop code.
                    o.halted = true;
                    continue;
                }
                let value = i32::from(byte) - 0x80;
                sum += (value * i32::from(o.volume)) >> 4;

                let next = o.accumulator.wrapping_add(u32::from(o.frequency)) & 0x00FF_FFFF;
                if o.one_shot && (next >> shift) >= (1 << size_bits) {
                    o.halted = true;
                }
                o.accumulator = next;
            }
            // Mono: the IIgs mixes the DOC to one path (per-oscillator
            // channel assignment is a stated simplification).
            frame[0] = sum;
            frame[1] = sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The IIgs DOC clock.
    const CLOCK: u32 = 7_159_090;

    fn render(chip: &mut Es5503, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    fn key_on(chip: &mut Es5503) {
        // A 256-byte square avoiding the zero stop code.
        let wave: Vec<u8> = (0..256)
            .map(|i| if i < 128 { 0xFF } else { 0x01 })
            .collect();
        chip.write_ram(0x100, &wave);
        chip.write(0, 0x00, 0x00); // frequency
        chip.write(0, 0x20, 0x08);
        chip.write(0, 0x40, 0xFF); // volume
        chip.write(0, 0x80, 0x01); // page 1
        chip.write(0, 0xC0, 0x00); // 256-byte table
        chip.write(0, 0xA0, 0x00); // free-run, un-halt
    }

    #[test]
    fn a_fresh_chip_is_silent_and_an_unhalted_one_is_not() {
        let mut chip = Es5503::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "all halted at reset");
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
    }

    /// **A zero byte halts the oscillator** -- the DOC's stop code lives in
    /// the wave data.
    #[test]
    fn the_zero_byte_halts() {
        let mut chip = Es5503::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        chip.write_ram(0x180, &[0x00]); // a stop code mid-wave
        // At frequency 0x0800 the index moves 1/32 sample a frame: give it
        // time to walk the 0x80 samples to the marker.
        render(&mut chip, 0x80 * 32 + 100);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
    }

    /// Enabling more oscillators slows the scan: the rate drops.
    #[test]
    fn the_enable_count_divides_the_scan() {
        let mut chip = Es5503::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0xE1, 0x3E); // all 32
        let all = chip.native_rate();
        chip.write(0, 0xE1, 0x00); // one oscillator
        let one = chip.native_rate();
        assert!(
            one > all * 8,
            "one oscillator scans far faster: {one} vs {all}"
        );
    }
}
