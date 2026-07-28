// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Seta X1-010: sixteen channels that are each either a PCM sampler or
//! a wavetable voice with a RAM envelope. 229 corpus files -- the Seta 1
//! and 2 arcade boards (Blandia, Zing Zing Zip, the Thundercade line).
//!
//! **Route B, from the documented behaviour** (the register layout as
//! MAME's `x1_010.cpp` documents it), scoped by a register histogram over
//! the corpus (both modes are heavily used; the envelope RAM sees over a
//! million writes across the rips).
//!
//! The chip's address space *is* its sound RAM: the sixteen 8-byte channel
//! files at `0x0000`, the thirty-one 128-byte envelope tables from
//! `0x0080`, and the 128-byte waveforms from `0x1000` -- all written
//! through the same command. A channel's registers mean different things
//! per mode: in PCM, volume nibbles / frequency / start page / end-page
//! complement; in wavetable, waveform number / 13-bit pitch / envelope
//! time / envelope number.
//!
//! Stated approximations: the envelope walks its table at a linear rate
//! derived from the time register (the silicon's exact divider is not
//! confidently documented), and the two rarely-written per-channel spare
//! registers are ignored.

use crate::chip::ChipCore;

/// One frame per 512 clocks: 31.25 kHz at the usual 16 MHz.
const CLOCK_DIVIDER: u32 = 512;

/// The X1-010.
#[derive(Debug)]
pub struct X1010 {
    rate: u32,
    /// The whole 8 KiB sound-RAM image: channel files, envelopes, waves.
    ram: Vec<u8>,
    /// Per-channel playback state the RAM does not carry.
    position: [u64; 16],
    env_position: [u32; 16],
    keyed: [bool; 16],
    rom: Vec<u8>,
}

impl X1010 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 31_250,
            ram: vec![0; 0x2000],
            position: [0; 16],
            env_position: [0; 16],
            keyed: [false; 16],
            rom: Vec::new(),
        }
    }
}

impl Default for X1010 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for X1010 {
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

    /// Every write is a sound-RAM write; a control-register key-on edge
    /// also restarts the channel's position.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        let at = usize::from(addr & 0x1FFF);
        let data = (data & 0xFF) as u8;
        if at < 0x80 && at % 8 == 0 {
            let ch = at / 8;
            let was = self.keyed[ch];
            let now = data & 0x01 != 0;
            if now && !was {
                self.position[ch] = 0;
                self.env_position[ch] = 0;
            }
            self.keyed[ch] = now;
        }
        self.ram[at] = data;
    }

    /// The PCM sample ROM: block type `0x91`.
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
                if !self.keyed[ch] {
                    continue;
                }
                let reg = |offset: usize| self.ram[ch * 8 + offset];
                let control = reg(0);
                if control & 0x02 != 0 {
                    // Wavetable mode: reg1 names the waveform, reg2/3 the
                    // pitch, reg4 the envelope time, reg5 the envelope.
                    let pitch = (u32::from(reg(3) & 0x1F) << 8) | u32::from(reg(2));
                    // pitch/1024 wave samples per frame, in 16.16.
                    self.position[ch] = self.position[ch].wrapping_add(u64::from(pitch) << 6);
                    let index = ((self.position[ch] >> 16) & 0x7F) as usize;
                    let wave_at = 0x1000 + usize::from(reg(1) & 0x1F) * 0x80 + index;
                    let sample = i32::from(self.ram[wave_at] as i8);

                    // The envelope: a 128-entry table of volume nibbles,
                    // walked at a rate the time register scales.
                    let env_at = usize::from(reg(5) & 0x1F) * 0x80;
                    let step = u32::from(reg(4)).max(1);
                    self.env_position[ch] = self.env_position[ch].wrapping_add(step);
                    let env_index = ((self.env_position[ch] >> 9) & 0x7F) as usize;
                    let env = self.ram[env_at + env_index];
                    left += sample * i32::from(env >> 4) * 4;
                    right += sample * i32::from(env & 0x0F) * 4;
                } else {
                    // PCM mode: reg1 volume nibbles, reg2 the byte rate in
                    // 1/256ths, reg4 the start page, reg5 the end-page
                    // complement.
                    let start = u64::from(reg(4)) << 12;
                    let end = (0x100 - u64::from(reg(5))) << 12;
                    self.position[ch] = self.position[ch].wrapping_add(u64::from(reg(2)) << 8);
                    let byte_at = start + (self.position[ch] >> 16);
                    if byte_at >= end {
                        self.keyed[ch] = false;
                        self.ram[ch * 8] &= !0x01;
                        continue;
                    }
                    let Some(&byte) = self.rom.get(byte_at as usize) else {
                        self.keyed[ch] = false;
                        continue;
                    };
                    let sample = i32::from(byte as i8);
                    let volume = reg(1);
                    left += sample * i32::from(volume >> 4) * 4;
                    right += sample * i32::from(volume & 0x0F) * 4;
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

    /// The Seta boards' 16 MHz.
    const CLOCK: u32 = 16_000_000;

    fn render(chip: &mut X1010, frames: usize) -> Vec<(i32, i32)> {
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

    fn pcm_key_on(chip: &mut X1010) {
        let mut rom = vec![0u8; 0x3000];
        for (index, byte) in rom[0x1000..0x2000].iter_mut().enumerate() {
            *byte = if index % 2 == 0 { 0x7F } else { 0x81 };
        }
        chip.load_rom(0x91, rom.len() as u32, 0, &rom);
        chip.write(0, 1, 0xFF); // volume nibbles
        chip.write(0, 2, 0x40); // a quarter byte a frame
        chip.write(0, 4, 0x01); // start page 1 (0x1000)
        chip.write(0, 5, 0xFE); // end page: 0x100-0xFE = 2 (0x2000)
        chip.write(0, 0, 0x01); // key on, PCM
    }

    #[test]
    fn pcm_mode_plays_and_self_stops_at_the_end() {
        let mut chip = X1010::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 200)), 0);
        pcm_key_on(&mut chip);
        assert!(energy(&render(&mut chip, 500)) > 0);
        // 0x1000 bytes at 1/4 byte a frame.
        render(&mut chip, 0x4000);
        assert_eq!(energy(&render(&mut chip, 200)), 0, "the end page stops");
        assert_eq!(chip.ram[0] & 1, 0, "and clears its own key bit");
    }

    /// Wavetable mode reads a RAM wave through a RAM envelope -- all of it
    /// uploaded through the same address space as the registers.
    #[test]
    fn wave_mode_reads_ram_through_the_envelope() {
        let mut chip = X1010::new();
        chip.reset(CLOCK, false);
        // Waveform 1: a square. Envelope 2: full volume throughout.
        for at in 0..0x80u16 {
            chip.write(0, 0x1080 + at, if at < 0x40 { 0x7F } else { 0x81 });
            chip.write(0, 0x100 + at, 0xFF);
        }
        chip.write(0, 1, 0x01); // waveform 1
        chip.write(0, 2, 0x00); // pitch
        chip.write(0, 3, 0x08);
        chip.write(0, 4, 0x10); // envelope time
        chip.write(0, 5, 0x02); // envelope 2
        chip.write(0, 0, 0x03); // key on, wavetable
        assert!(energy(&render(&mut chip, 500)) > 0);

        // A silent envelope table silences the same wave.
        for at in 0..0x80u16 {
            chip.write(0, 0x100 + at, 0x00);
        }
        assert_eq!(energy(&render(&mut chip, 200)), 0);
    }
}
