// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Ricoh RF5C68 and RF5C164: eight PCM channels reading 64 KiB of RAM.
//!
//! 1,124 files in the VGMRips corpus between them -- the RF5C68 on the
//! FM Towns and Sega System 18/32 boards, the RF5C164 in every Mega CD. The
//! two parts answer the same registers and differ only in fabrication, so
//! one core serves both kinds.
//!
//! **Route B, from the documented behaviour** (the register map as MAME's
//! `rf5c68.cpp`, BSD-3-Clause, documents it), so it lives in the permissive
//! crate.
//!
//! # The shape of the chip
//!
//! Unlike its ROM-reading cousins, this chip plays from **RAM the driver
//! streams into it** -- through a 4 KiB window a bank register slides over
//! the 64 KiB. A VGM carries those uploads two ways: `0xC0`-type data
//! blocks, and direct `0xC1`/`0xC2` pokes, which [the stream
//! decoder](dro_core::vgm::stream) hands this core on **port 1** so they
//! cannot collide with the register writes on port 0.
//!
//! Two behaviours define the sound and are pinned by tests. Samples are
//! **sign-magnitude**, not two's complement -- bit 7 set means positive --
//! and the byte `0xFF` is not a sample at all but the **loop marker**: a
//! channel that reads it jumps to its loop address, and a channel that
//! finds the marker there too halts instead of spinning.

use crate::chip::ChipCore;

/// The output sample rate divider: one frame per 384 clocks.
const CLOCK_DIVIDER: u32 = 384;

/// One PCM channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// Envelope: an 8-bit volume.
    envelope: u8,
    /// Pan: low nibble left, high nibble right.
    pan: u8,
    /// The address step per frame, in 16.11 fixed point.
    delta: u16,
    /// The loop address, in whole bytes.
    loop_start: u16,
    /// The start address's high byte: a key-on begins at `start << 8`.
    start: u8,
    /// The playback address, 16.11 fixed point (bits 11.. are the byte).
    address: u32,
    /// Keyed on. The register is an *off* mask; this is kept the readable
    /// way round.
    playing: bool,
}

/// The RF5C68 / RF5C164.
#[derive(Debug)]
pub struct Rf5c68 {
    rate: u32,
    channels: [Channel; 8],
    ram: Vec<u8>,
    /// The 4 KiB window's base within the RAM, from the bank register.
    bank: usize,
    /// Which channel the per-channel registers currently address.
    selected: usize,
    /// The master enable, from the control register's bit 7.
    sounding: bool,
}

impl Rf5c68 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 32_552,
            channels: [Channel::default(); 8],
            ram: vec![0; 0x10000],
            bank: 0,
            selected: 0,
            sounding: false,
        }
    }
}

impl Default for Rf5c68 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Rf5c68 {
    fn reset(&mut self, clock: u32, _variant: bool) {
        let ram = std::mem::take(&mut self.ram);
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            ..Self::new()
        };
        // The RAM arrives before the stream starts (data blocks precede the
        // engine's load-time reset) and must survive it, like every ROM.
        self.ram = ram;
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// Port 0 is the register file; port 1 is a direct memory poke through
    /// the current bank's window.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let data = (data & 0xFF) as u8;
        if port == 1 {
            let at = (self.bank | (addr as usize & 0x0FFF)) & 0xFFFF;
            self.ram[at] = data;
            return;
        }
        let ch = &mut self.channels[self.selected];
        match addr & 0x0F {
            0x00 => ch.envelope = data,
            0x01 => ch.pan = data,
            0x02 => ch.delta = (ch.delta & 0xFF00) | u16::from(data),
            0x03 => ch.delta = (ch.delta & 0x00FF) | (u16::from(data) << 8),
            0x04 => ch.loop_start = (ch.loop_start & 0xFF00) | u16::from(data),
            0x05 => ch.loop_start = (ch.loop_start & 0x00FF) | (u16::from(data) << 8),
            0x06 => ch.start = data,
            // Control: bit 7 is the master enable; bit 6 picks what bits 0-3
            // mean -- a channel selection, or the RAM window's bank.
            0x07 => {
                self.sounding = data & 0x80 != 0;
                if data & 0x40 != 0 {
                    self.selected = usize::from(data & 0x07);
                } else {
                    self.bank = usize::from(data & 0x0F) << 12;
                }
            }
            // The key register is an OFF mask: a zero bit sounds. A channel
            // going from off to on restarts at its start address.
            0x08 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    let on = data & (1 << index) == 0;
                    if on && !ch.playing {
                        ch.address = u32::from(ch.start) << (8 + 11);
                    }
                    ch.playing = on;
                }
            }
            _ => {}
        }
    }

    /// `0xC0`-type RAM blocks, through the current bank's window like the
    /// pokes -- rips upload large samples as bank write, block, bank write,
    /// block.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        let base = self.bank | (offset as usize & 0x0FFF);
        for (index, &byte) in data.iter().enumerate() {
            let Some(slot) = self.ram.get_mut((base + index) & 0xFFFF) else {
                break;
            };
            *slot = byte;
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            if self.sounding {
                for ch in &mut self.channels {
                    if !ch.playing {
                        continue;
                    }
                    let mut byte = self.ram[(ch.address >> 11) as usize & 0xFFFF];
                    if byte == 0xFF {
                        // The loop marker. Jump once; a marker at the loop
                        // point means silence, not a spin.
                        ch.address = u32::from(ch.loop_start) << 11;
                        byte = self.ram[(ch.address >> 11) as usize & 0xFFFF];
                        if byte == 0xFF {
                            ch.playing = false;
                            continue;
                        }
                    }
                    ch.address = ch.address.wrapping_add(u32::from(ch.delta));
                    // Sign-magnitude: bit 7 set is the positive half.
                    let magnitude = i32::from(byte & 0x7F);
                    let sample = if byte & 0x80 != 0 {
                        magnitude
                    } else {
                        -magnitude
                    };
                    // x3 on the first draft: the scorecard measured our
                    // level at 0.344 of the reference's.
                    let scaled = sample * i32::from(ch.envelope) * 3;
                    left += (scaled * i32::from(ch.pan & 0x0F)) >> 5;
                    right += (scaled * i32::from(ch.pan >> 4)) >> 5;
                }
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mega CD's RF5C164 clock.
    const CLOCK: u32 = 12_500_000;

    fn render(chip: &mut Rf5c68, frames: usize) -> Vec<(i32, i32)> {
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

    /// Uploads a loud alternating sample at 0x1000 with a loop marker at its
    /// end, and keys channel 1 onto it.
    fn key_on(chip: &mut Rf5c68) {
        chip.write(0, 0x07, 0x41); // select... first, the bank: window at 0x1000
        chip.write(0, 0x07, 0x01);
        let wave: Vec<u8> = (0..256)
            .map(|i| if i % 2 == 0 { 0xFE } else { 0x7E })
            .collect();
        chip.write_ram(0, &wave);
        chip.ram[0x1100] = 0xFF; // loop marker
        chip.write(0, 0x07, 0xC0); // sounding on, select channel 0
        chip.write(0, 0x00, 0xFF); // envelope full
        chip.write(0, 0x01, 0xFF); // both sides full
        chip.write(0, 0x02, 0x00);
        chip.write(0, 0x03, 0x08); // delta 0x0800: one byte per frame
        chip.write(0, 0x04, 0x00); // loop to 0x1000
        chip.write(0, 0x05, 0x10);
        chip.write(0, 0x06, 0x10); // start at 0x1000
        chip.write(0, 0x08, 0xFE); // key channel 1 on (off-mask)
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = Rf5c68::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 500)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 2000)) > 0);
    }

    /// **`0xFF` is the loop marker, not a sample.** A channel that reads it
    /// jumps to the loop address and keeps sounding; with the marker at the
    /// loop point too, it halts.
    #[test]
    fn the_ff_byte_loops_and_a_looped_ff_halts() {
        let mut chip = Rf5c68::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        // 256 bytes at one byte a frame, then the marker: the loop keeps it
        // sounding far past the sample's end.
        let long = energy(&render(&mut chip, 4000));
        assert!(long > 0, "the loop must keep the channel alive");

        // Point the loop at the marker itself: one jump, then silence.
        chip.write(0, 0x04, 0x00);
        chip.write(0, 0x05, 0x11); // loop to 0x1100 = the marker
        render(&mut chip, 300); // let it reach the end
        assert_eq!(
            energy(&render(&mut chip, 500)),
            0,
            "a marker at the loop point must halt, not spin"
        );
    }

    /// **Samples are sign-magnitude.** `0xFE` and `0x7E` are the same
    /// magnitude on opposite sides of zero -- the alternating upload must
    /// produce both polarities. Read two's complement instead, both land
    /// negative-ish and the wave rides a huge DC offset.
    #[test]
    fn samples_are_sign_magnitude() {
        let mut chip = Rf5c68::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        let samples = render(&mut chip, 200);
        let positive = samples.iter().any(|&(l, _)| l > 0);
        let negative = samples.iter().any(|&(l, _)| l < 0);
        assert!(
            positive && negative,
            "both polarities must appear: {:?}",
            &samples[..8]
        );
    }

    /// The master enable gates everything; the key register is an off-mask.
    #[test]
    fn the_control_bits_gate_the_mix() {
        let mut chip = Rf5c68::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
        chip.write(0, 0x07, 0x40); // sounding off
        assert_eq!(energy(&render(&mut chip, 500)), 0);
        chip.write(0, 0x07, 0xC0); // back on
        chip.write(0, 0x08, 0xFF); // all channels off
        assert_eq!(energy(&render(&mut chip, 500)), 0);
    }

    /// Memory pokes (port 1) go through the bank window, exactly like the
    /// register-driven uploads.
    #[test]
    fn port_one_pokes_land_in_the_banked_window() {
        let mut chip = Rf5c68::new();
        chip.reset(CLOCK, false);
        chip.write(0, 0x07, 0x02); // bank 2: window at 0x2000
        chip.write(1, 0x0034, 0xAB);
        assert_eq!(chip.ram[0x2034], 0xAB);
    }
}
