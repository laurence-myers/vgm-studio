// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Yamaha YMZ280B: eight voices of ADPCM or PCM from ROM -- Cave's
//! shooters, Capcom's ZN boards, Psikyo. 354 corpus files.
//!
//! **Route B, from the documented behaviour** (the register map as MAME's
//! `ymz280b.cpp`, BSD-3-Clause, documents it), so it lives in the
//! permissive crate.
//!
//! The register file is banked by function: the per-voice pitch, control,
//! level and pan live at `0x00`-`0x1F`, and the six address bytes each
//! voice needs are spread across three upper banks -- high bytes at
//! `0x20`-`0x3F`, mids at `0x40`-`0x5F`, lows at `0x60`-`0x7F`. The ADPCM
//! is the familiar OKI ladder into a **16-bit clamping** accumulator --
//! not the OKIM chips' 12-bit clamp, not the OPN family's 12-bit wrap,
//! and the difference is why this core has its own small decoder.

use crate::chip::ChipCore;

/// One frame per 384 clocks: 44.1 kHz at the usual 16.9344 MHz.
const CLOCK_DIVIDER: u32 = 384;

/// The OKI step ladder, shared shape with the other ADPCM chips here.
const STEPS: [i32; 49] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552,
];
const INDEX_DELTA: [i32; 8] = [-1, -1, -1, -1, 2, 5, 7, 9];

/// What a voice's mode bits say it plays.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Mode {
    #[default]
    Adpcm,
    Pcm8,
    Pcm16,
}

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    /// Nine bits: `(pitch + 1) / 256` source samples per output frame.
    pitch: u16,
    mode: Mode,
    looping: bool,
    level: u8,
    /// Pan 0-15, centre 8.
    pan: u8,
    /// 24-bit byte addresses.
    start: u32,
    loop_start: u32,
    loop_end: u32,
    end: u32,
    /// Sample index from the start, 16.8.
    position: u32,
    playing: bool,
    /// ADPCM state: 16-bit clamping accumulator and ladder index.
    signal: i32,
    step: i32,
}

impl Voice {
    fn decode_adpcm(&mut self, nibble: u8) -> i32 {
        let step = STEPS[self.step as usize];
        let magnitude = i32::from(nibble & 0x07);
        let delta = (2 * magnitude + 1) * step / 8;
        let signed = if nibble & 0x08 != 0 { -delta } else { delta };
        self.signal = (self.signal + signed).clamp(-32768, 32767);
        self.step = (self.step + INDEX_DELTA[usize::from(nibble & 0x07)]).clamp(0, 48);
        self.signal
    }
}

/// The YMZ280B.
#[derive(Debug)]
pub struct Ymz280b {
    rate: u32,
    voices: [Voice; 8],
    rom: Vec<u8>,
}

impl Ymz280b {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 44_100,
            voices: [Voice::default(); 8],
            rom: Vec::new(),
        }
    }
}

impl Default for Ymz280b {
    fn default() -> Self {
        Self::new()
    }
}

/// Replaces one byte of a 24-bit address.
fn set_byte(target: &mut u32, shift: u32, data: u8) {
    *target = (*target & !(0xFF << shift)) | (u32::from(data) << shift);
}

impl ChipCore for Ymz280b {
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
        let reg = (addr & 0xFF) as u8;
        let data = (data & 0xFF) as u8;
        if reg >= 0x80 {
            // The DSP/IRQ/enable block: not modelled (gating sound on the
            // master-enable bit is the NES lesson).
            return;
        }
        let voice = &mut self.voices[usize::from((reg >> 2) & 0x07)];
        match reg & 0xE3 {
            // The per-voice file.
            0x00 => voice.pitch = (voice.pitch & 0x100) | u16::from(data),
            0x01 => {
                voice.pitch = (voice.pitch & 0x0FF) | (u16::from(data & 0x01) << 8);
                voice.looping = data & 0x10 != 0;
                voice.mode = match (data >> 5) & 0x03 {
                    2 => Mode::Pcm8,
                    3 => Mode::Pcm16,
                    _ => Mode::Adpcm,
                };
                let key = data & 0x80 != 0;
                if key && !voice.playing {
                    voice.position = 0;
                    voice.signal = 0;
                    voice.step = 0;
                }
                voice.playing = key;
            }
            0x02 => voice.level = data,
            0x03 => voice.pan = data & 0x0F,
            // The three address banks: high, mid, low bytes of the four
            // addresses.
            0x20 => set_byte(&mut voice.start, 16, data),
            0x21 => set_byte(&mut voice.loop_start, 16, data),
            0x22 => set_byte(&mut voice.loop_end, 16, data),
            0x23 => set_byte(&mut voice.end, 16, data),
            0x40 => set_byte(&mut voice.start, 8, data),
            0x41 => set_byte(&mut voice.loop_start, 8, data),
            0x42 => set_byte(&mut voice.loop_end, 8, data),
            0x43 => set_byte(&mut voice.end, 8, data),
            0x60 => set_byte(&mut voice.start, 0, data),
            0x61 => set_byte(&mut voice.loop_start, 0, data),
            0x62 => set_byte(&mut voice.loop_end, 0, data),
            0x63 => set_byte(&mut voice.end, 0, data),
            _ => {}
        }
    }

    /// The sample ROM: block type `0x86`.
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
                // Samples are nibbles in ADPCM mode, bytes in PCM8, byte
                // pairs in PCM16; positions count source samples.
                let index = voice.position >> 8;
                let (address, at_end) = match voice.mode {
                    Mode::Adpcm => {
                        let byte_addr = voice.start + index / 2;
                        (byte_addr, byte_addr >= voice.end.max(voice.start + 1))
                    }
                    Mode::Pcm8 => {
                        let byte_addr = voice.start + index;
                        (byte_addr, byte_addr >= voice.end.max(voice.start + 1))
                    }
                    Mode::Pcm16 => {
                        let byte_addr = voice.start + index * 2;
                        (byte_addr, byte_addr + 1 >= voice.end.max(voice.start + 2))
                    }
                };
                if at_end {
                    if voice.looping {
                        voice.position = 0;
                        voice.start = voice.loop_start;
                        voice.end = voice.loop_end;
                        voice.signal = 0;
                        voice.step = 0;
                        continue;
                    }
                    voice.playing = false;
                    continue;
                }
                let Some(&byte) = self.rom.get(address as usize) else {
                    voice.playing = false;
                    continue;
                };
                let value = match voice.mode {
                    Mode::Adpcm => {
                        let nibble = if index % 2 == 0 {
                            byte >> 4
                        } else {
                            byte & 0x0F
                        };
                        // Decode only when a fresh nibble is reached; hold
                        // otherwise.
                        if voice.position & 0xFF < u32::from(voice.pitch + 1).min(0xFF) {
                            voice.decode_adpcm(nibble)
                        } else {
                            voice.signal
                        }
                    }
                    Mode::Pcm8 => i32::from(byte as i8) << 8,
                    Mode::Pcm16 => {
                        let hi = self.rom.get(address as usize + 1).copied().unwrap_or(0);
                        i32::from(i16::from_le_bytes([byte, hi]))
                    }
                };
                voice.position += u32::from(voice.pitch) + 1;

                // >>7 rather than the first draft's >>10: the scorecard measured
                // our level at 0.124 of the reference's.
                let scaled = (value * i32::from(voice.level)) >> 7;
                let pan = i32::from(voice.pan.clamp(0, 15));
                left += (scaled * (16 - pan)) >> 3;
                right += (scaled * pan.max(1)) >> 3;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual crystal: 44.1 kHz exactly.
    const CLOCK: u32 = 16_934_400;

    fn render(chip: &mut Ymz280b, frames: usize) -> Vec<(i32, i32)> {
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
            *byte = if index % 2 == 0 { 0x77 } else { 0xFF };
        }
        rom
    }

    /// Voice 0 keyed onto the ADPCM run at 0x1000.
    fn key_on(chip: &mut Ymz280b) {
        chip.write(0, 0x20, 0x00); // start 0x001000
        chip.write(0, 0x40, 0x10);
        chip.write(0, 0x60, 0x00);
        chip.write(0, 0x23, 0x00); // end 0x001100
        chip.write(0, 0x43, 0x11);
        chip.write(0, 0x63, 0x00);
        chip.write(0, 0x02, 0xFF); // level
        chip.write(0, 0x03, 0x08); // centre
        chip.write(0, 0x00, 0xFF); // pitch: unity
        chip.write(0, 0x01, 0x80); // key on, ADPCM, no loop
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_one_is_not() {
        let mut chip = Ymz280b::new();
        chip.reset(CLOCK, false);
        assert_eq!(chip.native_rate(), 44_100);
        let sample_rom = rom();
        chip.load_rom(0x86, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A one-shot stops at its end; the loop bit re-runs the loop window.
    #[test]
    fn the_end_stops_and_the_loop_sustains() {
        let mut chip = Ymz280b::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x86, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x300);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "one-shot must stop");

        chip.write(0, 0x21, 0x00); // loop window = the sample itself
        chip.write(0, 0x41, 0x10);
        chip.write(0, 0x61, 0x00);
        chip.write(0, 0x22, 0x00);
        chip.write(0, 0x42, 0x11);
        chip.write(0, 0x62, 0x00);
        chip.write(0, 0x01, 0x00); // key off first
        chip.write(0, 0x01, 0x90); // key on, looping
        render(&mut chip, 0x400);
        assert!(energy(&render(&mut chip, 200)) > 0, "the loop must sustain");
    }

    /// **The accumulator clamps at sixteen bits** -- the property that
    /// separates this decoder from the OKI chips' 12-bit clamp and the OPN
    /// family's 12-bit wrap.
    #[test]
    fn the_adpcm_accumulator_clamps_at_sixteen_bits() {
        let mut voice = Voice::default();
        for _ in 0..2000 {
            voice.decode_adpcm(0x7);
        }
        assert_eq!(voice.signal, 32767, "it must clamp, not wrap");
        for _ in 0..4000 {
            voice.decode_adpcm(0xF);
        }
        assert_eq!(voice.signal, -32768);
    }
}
