// SPDX-License-Identifier: MIT OR Apache-2.0
//! Capcom's QSound: sixteen PCM voices behind a DSP this core does not run.
//!
//! 1,698 files in the VGMRips corpus -- every CPS2 board: the Street
//! Fighter Alpha and Darkstalkers lines, Marvel, Progear. The real chip is
//! a DSP16A running Capcom's program; what a VGM records is the register
//! interface that program serves, and what this core models is the part of
//! the program that plays music: sixteen PCM voices with pitch, loop and
//! per-voice pan.
//!
//! **Route B, from the documented register interface** (as the community
//! documentation and MAME's `qsound.cpp`, BSD-3-Clause, describe it), so it
//! lives in the permissive crate.
//!
//! Stated approximations: the **echo and filter stages are not modelled**
//! (the "Q" of QSound -- rips still carry all their notes, drier than the
//! cabinet), the **three ADPCM effect voices are not modelled** (music
//! lives on the PCM sixteen), and the pan table is a linear weighting of
//! the documented 33-position range.

use crate::chip::ChipCore;

/// One frame per 166 clocks: 24 kHz-ish at the usual 4 MHz.
const CLOCK_DIVIDER: u32 = 166;

/// One voice.
#[derive(Debug, Default, Clone, Copy)]
struct Voice {
    bank: u16,
    start: u16,
    /// `0x1000` is one ROM byte per output frame.
    pitch: u16,
    /// Bytes to step back at the end. Zero means one-shot.
    loop_length: u16,
    end: u16,
    volume: u16,
    /// Pan position 0..=32, centre 16.
    pan: u16,
    /// Position within the bank, 16.16 over the 16-bit address.
    position: u32,
}

/// The QSound.
#[derive(Debug)]
pub struct QSound {
    rate: u32,
    voices: [Voice; 16],
    rom: Vec<u8>,
}

impl QSound {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 24_038,
            voices: [Voice::default(); 16],
            rom: Vec::new(),
        }
    }
}

impl Default for QSound {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for QSound {
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

    /// The normalised `0xC4`: `addr` is the register, `data` the 16-bit
    /// value. Registers `0x00`-`0x7F` are the voice file, eight apiece;
    /// `0x80`-`0x8F` the pan table positions.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        match addr {
            0x00..=0x7F => {
                let voice = &mut self.voices[usize::from(addr >> 3)];
                match addr & 0x07 {
                    0x00 => voice.bank = data,
                    0x01 => {
                        voice.start = data;
                        voice.position = u32::from(data) << 16;
                    }
                    0x02 => voice.pitch = data,
                    // 0x03 is the phase register, uninteresting to playback.
                    0x04 => voice.loop_length = data,
                    0x05 => voice.end = data,
                    0x06 => voice.volume = data,
                    _ => {}
                }
            }
            // Pan: the DSP program's table positions run 0x0110..=0x0130,
            // left to right.
            0x80..=0x8F => {
                let voice = &mut self.voices[usize::from(addr & 0x0F)];
                voice.pan = data.saturating_sub(0x0110).min(32);
            }
            // Echo depth, filter selects, ADPCM triggers: unmodelled.
            _ => {}
        }
    }

    /// The sample ROM: block type `0x8F`.
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
                // A voice with no pitch or no volume is at rest; drivers
                // stop channels by zeroing the pitch.
                if voice.pitch == 0 || voice.volume == 0 {
                    continue;
                }
                let offset = voice.position >> 16;
                if offset >= u32::from(voice.end) {
                    if voice.loop_length == 0 {
                        voice.pitch = 0;
                        continue;
                    }
                    voice.position = voice
                        .position
                        .wrapping_sub(u32::from(voice.loop_length) << 16);
                }
                let at = ((u32::from(voice.bank & 0x7F) << 16) | (voice.position >> 16)) as usize;
                let Some(&byte) = self.rom.get(at) else {
                    voice.pitch = 0;
                    continue;
                };
                // Pitch 0x1000 is unity, so the 16.16 step is pitch << 4.
                voice.position = voice.position.wrapping_add(u32::from(voice.pitch) << 4);

                // Sample (i8) x volume (0..=0x1FFF typical) scaled so a
                // full-volume voice lands near the ~8k one-channel headroom
                // convention.
                let scaled = (i32::from(byte as i8) * i32::from(voice.volume)) >> 7;
                let pan = i32::from(voice.pan);
                left += (scaled * (32 - pan)) >> 5;
                right += (scaled * pan) >> 5;
            }
            frame[0] = left;
            frame[1] = right;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CPS2 QSound clock.
    const CLOCK: u32 = 4_000_000;

    fn render(chip: &mut QSound, frames: usize) -> Vec<(i32, i32)> {
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

    fn key_on(chip: &mut QSound) {
        chip.write(0, 0x00, 0x0000); // bank 0
        chip.write(0, 0x05, 0x1100); // end
        chip.write(0, 0x04, 0x0000); // one-shot
        chip.write(0, 0x06, 0x1FFF); // volume
        chip.write(0, 0x80, 0x0120); // centre pan
        chip.write(0, 0x01, 0x1000); // start (resets position)
        chip.write(0, 0x02, 0x1000); // pitch: unity -- and the key
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_pitched_one_is_not() {
        let mut chip = QSound::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8F, sample_rom.len() as u32, 0, &sample_rom);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 200)) > 0);
    }

    /// A one-shot silences itself at its end register; a loop length steps
    /// back and keeps sounding.
    #[test]
    fn the_end_stops_and_the_loop_sustains() {
        let mut chip = QSound::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8F, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        render(&mut chip, 0x180);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "one-shot must stop");

        key_on(&mut chip);
        chip.write(0, 0x04, 0x0100); // loop the whole sample
        render(&mut chip, 0x400);
        assert!(energy(&render(&mut chip, 200)) > 0, "loop must sustain");
    }

    /// The pan positions steer the stereo pair: full left leaves the right
    /// channel silent, and the reverse.
    #[test]
    fn the_pan_table_steers() {
        let mut chip = QSound::new();
        chip.reset(CLOCK, false);
        let sample_rom = rom();
        chip.load_rom(0x8F, sample_rom.len() as u32, 0, &sample_rom);
        key_on(&mut chip);
        chip.write(0, 0x80, 0x0110); // full left
        let samples = render(&mut chip, 100);
        assert!(samples.iter().any(|&(l, _)| l != 0));
        assert!(samples.iter().all(|&(_, r)| r == 0));

        chip.write(0, 0x80, 0x0130); // full right
        let samples = render(&mut chip, 100);
        assert!(samples.iter().all(|&(l, _)| l == 0));
        assert!(samples.iter().any(|&(_, r)| r != 0));
    }
}
