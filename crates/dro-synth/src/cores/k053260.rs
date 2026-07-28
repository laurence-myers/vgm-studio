// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Konami K053260: four voices of PCM or Konami DPCM from ROM -- the
//! early-90s Konami boards (Asterix, Vendetta, Simpsons). 366 corpus files.
//!
//! **Route B, from the documented behaviour** (the register map as MAME's
//! `k053260.cpp`, BSD-3-Clause, documents it), so it lives in the
//! permissive crate.
//!
//! The first eight register addresses are CPU-to-CPU mailboxes and carry no
//! audio; the four voices live at `0x08`-`0x27`, and the shared key, loop,
//! mode and pan registers above them. Pitch is a divider against the
//! chip's tick (`clock / (0x1000 - pitch)`), length counts bytes down, and
//! the DPCM mode shares the K054539's nibble-delta shape.

use crate::chip::ChipCore;

/// The core renders one frame per 32 clocks.
const CLOCK_DIVIDER: u32 = 32;

/// The Konami DPCM nibble deltas, the K054539's table at sample scale.
const DPCM_DELTA: [i32; 16] = [
    0, 1, 2, 4, 8, 16, 32, 64, -128, -64, -32, -16, -8, -4, -2, -1,
];

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    /// Twelve bits: a divider, `clock / (0x1000 - pitch)`.
    pitch: u16,
    /// Bytes to play.
    length: u32,
    /// 21-bit start address.
    start: u32,
    volume: u8,
    /// Pan 0-7 from the shared registers.
    pan: u8,
    looping: bool,
    dpcm: bool,
    /// Position in 16.16 bytes from the start.
    position: u32,
    playing: bool,
    /// DPCM level and nibble phase.
    level: i32,
    high_nibble: bool,
}

impl Voice {
    fn step(&self) -> u32 {
        (CLOCK_DIVIDER << 16) / (0x1000 - u32::from(self.pitch & 0x0FFF)).max(1)
    }
}

/// The K053260.
#[derive(Debug)]
pub struct K053260 {
    rate: u32,
    voices: [Voice; 4],
    rom: Vec<u8>,
}

impl K053260 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 111_861,
            voices: [Voice::default(); 4],
            rom: Vec::new(),
        }
    }
}

impl Default for K053260 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for K053260 {
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

    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let reg = (addr & 0x3F) as u8;
        let data = (data & 0xFF) as u8;
        match reg {
            // 0x00-0x07: the CPU mailboxes; no audio.
            0x08..=0x27 => {
                let voice = &mut self.voices[usize::from((reg - 8) >> 3)];
                match (reg - 8) & 0x07 {
                    0x00 => voice.pitch = (voice.pitch & 0x0F00) | u16::from(data),
                    0x01 => voice.pitch = (voice.pitch & 0x00FF) | (u16::from(data & 0x0F) << 8),
                    0x02 => voice.length = (voice.length & 0xFF00) | u32::from(data),
                    0x03 => voice.length = (voice.length & 0x00FF) | (u32::from(data) << 8),
                    0x04 => voice.start = (voice.start & 0x1F_FF00) | u32::from(data),
                    0x05 => voice.start = (voice.start & 0x1F_00FF) | (u32::from(data) << 8),
                    0x06 => {
                        voice.start = (voice.start & 0x00_FFFF) | (u32::from(data & 0x1F) << 16)
                    }
                    0x07 => voice.volume = data & 0x7F,
                    _ => unreachable!("masked to three bits"),
                }
            }
            // Key-on: an edge per set bit restarts the voice.
            0x28 => {
                for (index, voice) in self.voices.iter_mut().enumerate() {
                    let on = data & (1 << index) != 0;
                    if on && !voice.playing {
                        voice.position = 0;
                        voice.level = 0;
                        voice.high_nibble = false;
                    }
                    voice.playing = on;
                }
            }
            // Loop enables low, DPCM mode selects high.
            0x2A => {
                for (index, voice) in self.voices.iter_mut().enumerate() {
                    voice.looping = data & (1 << index) != 0;
                    voice.dpcm = data & (0x10 << index) != 0;
                }
            }
            // Pan pairs: three bits a voice.
            0x2C => {
                self.voices[0].pan = data & 0x07;
                self.voices[1].pan = (data >> 3) & 0x07;
            }
            0x2D => {
                self.voices[2].pan = data & 0x07;
                self.voices[3].pan = (data >> 3) & 0x07;
            }
            // 0x2F is the control register (sound/input enables): gating on
            // it is the NES lesson, so it is left unread.
            _ => {}
        }
    }

    /// The sample ROM: block type `0x8E`.
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
            for voice in &mut self.voices {
                if !voice.playing {
                    continue;
                }
                let index = voice.position >> 16;
                // DPCM packs two samples a byte, so its byte span is half.
                let bytes = if voice.dpcm { index / 2 } else { index };
                if bytes >= voice.length.max(1) {
                    if voice.looping {
                        voice.position = 0;
                        voice.level = 0;
                        voice.high_nibble = false;
                        continue;
                    }
                    voice.playing = false;
                    continue;
                }
                let Some(&byte) = self.rom.get((voice.start + bytes) as usize) else {
                    voice.playing = false;
                    continue;
                };
                let value = if voice.dpcm {
                    let nibble = if index % 2 == 0 {
                        byte & 0x0F
                    } else {
                        byte >> 4
                    };
                    let fresh = (index % 2 == 1) != voice.high_nibble || index == 0;
                    if fresh {
                        voice.level =
                            (voice.level + DPCM_DELTA[usize::from(nibble)]).clamp(-128, 127);
                        voice.high_nibble = index % 2 == 1;
                    }
                    voice.level
                } else {
                    i32::from(byte as i8)
                };
                voice.position = voice.position.wrapping_add(voice.step());

                // x5.5 on the first draft's net scale: the scorecard
                // measured our level at 0.178 of the reference's.
                let scaled = (value * i32::from(voice.volume)) >> 1;
                let pan = i32::from(voice.pan.clamp(1, 7));
                left += (scaled * (8 - pan) * 11) >> 3;
                right += (scaled * pan * 11) >> 3;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Konami boards' clock.
    const CLOCK: u32 = 3_579_545;

    fn render(chip: &mut K053260, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut K053260) {
        chip.write(0, 0x08, 0xE0); // pitch 0xFE0: divider 32, one byte a frame
        chip.write(0, 0x09, 0x0F);
        chip.write(0, 0x0A, 0x00); // length 0x100
        chip.write(0, 0x0B, 0x01);
        chip.write(0, 0x0C, 0x00); // start 0x1000
        chip.write(0, 0x0D, 0x10);
        chip.write(0, 0x0E, 0x00);
        chip.write(0, 0x0F, 0x7F); // volume
        chip.write(0, 0x2C, 0x04); // centre-ish pan
        chip.write(0, 0x28, 0x01); // key on voice 0
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = K053260::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8E, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// The length register counts the sample out; the loop bit re-runs it.
    #[test]
    fn the_length_stops_and_the_loop_sustains() {
        let mut chip = K053260::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8E, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x180);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "one-shot must stop");

        chip.write(0, 0x2A, 0x01); // loop voice 0
        chip.write(0, 0x28, 0x00); // key off first: the key is an edge
        chip.write(0, 0x28, 0x01);
        render(&mut chip, 0x400);
        assert!(energy(&render(&mut chip, 200)) > 0, "the loop must sustain");
    }

    /// The mailboxes carry no audio: writes below 0x08 change nothing.
    #[test]
    fn the_mailboxes_are_not_registers() {
        let mut chip = K053260::new();
        chip.reset(CLOCK, false);
        for reg in 0..8u16 {
            chip.write(0, reg, 0xFF);
        }
        assert_eq!(energy(&render(&mut chip, 200)), 0);
    }
}
