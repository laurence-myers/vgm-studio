// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Konami K051649 (SCC) and K052539 (SCC+): five channels of 32-byte
//! wavetables.
//!
//! 512 files in the VGMRips corpus -- Konami's MSX cartridges (Gradius 2,
//! Snatcher) and a run of Konami arcade boards. The chip is a memory mapper
//! that happens to contain a synthesizer, and the synthesizer is the part a
//! VGM records: five channels, each reading a 32-sample signed 8-bit wave at
//! a 12-bit period, through a 4-bit volume, gated by one enable mask.
//!
//! **Route B, from the documented behaviour**, so it lives in the permissive
//! crate.
//!
//! # SCC versus SCC+
//!
//! The original SCC has **four** wave memories: channel 5 plays channel 4's
//! table, and Konami's drivers were written around that economy. The SCC+
//! gives channel 5 its own memory, reached through its own register window,
//! which the VGM stream carries as writes on port 4. The header's variant
//! bit (bit 31 of the clock) says which chip the file means; on an SCC the
//! port-4 window does not exist and its writes are dropped rather than
//! misfiled.
//!
//! Not modelled: the deformation/test register (port 5) -- its counter and
//! read-rotation tricks are undocumented corner behaviour no corpus file
//! sampled here exercises -- and the memory-mapper half of the chip, which
//! makes no sound.

use crate::chip::ChipCore;

/// The chip divides its clock by 16 to step channel phase accumulators.
const CLOCK_DIVIDER: u32 = 16;

/// Peak amplitude contribution of one channel at full volume.
///
/// A full-scale wave sample (+-128) at volume 15 is +-1920; scaled by 4 that
/// is +-7680, the same one-channel headroom convention the AY and SN cores
/// use (their PEAK is 8000).
const VOLUME_SCALE: i32 = 4;

/// One wavetable channel.
#[derive(Debug, Default, Clone, Copy)]
struct Channel {
    /// Twelve bits; zero behaves as one.
    period: u16,
    /// Wave position in 16.16, the low bits carrying the fraction.
    phase: u32,
    /// Wave steps per internal tick, in 16.16 -- recomputed on period writes
    /// so the render loop divides nothing.
    step: u32,
    /// Four bits.
    volume: u8,
    enabled: bool,
}

impl Channel {
    fn set_period(&mut self, period: u16) {
        self.period = period & 0x0FFF;
        // The wave pointer advances at clock/(period+1); one internal tick is
        // sixteen clocks, so sixteen wave steps' worth of clocks pass per
        // tick.
        self.step = (CLOCK_DIVIDER << 16) / (u32::from(self.period) + 1);
    }
}

/// The K051649 / K052539.
#[derive(Debug)]
pub struct K051649 {
    rate: u32,
    /// The SCC+ (K052539): channel 5 has its own wave memory.
    plus: bool,
    channels: [Channel; 5],
    /// Five tables; on the plain SCC the fifth is never read (channel 5
    /// plays table 4) and never written (port 4 is dropped).
    waves: [[i8; 32]; 5],
}

impl K051649 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 111_861,
            plus: false,
            channels: [Channel::default(); 5],
            waves: [[0; 32]; 5],
        }
    }
}

impl Default for K051649 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for K051649 {
    /// `variant` is the header's bit 31: the K052539 (SCC+).
    fn reset(&mut self, clock: u32, variant: bool) {
        *self = Self {
            rate: (clock / CLOCK_DIVIDER).max(1),
            plus: variant,
            ..Self::new()
        };
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The VGM's `0xD2 pp aa dd`: `port` picks the register window.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let addr = (addr & 0xFF) as usize;
        let data = (data & 0xFF) as u8;
        match port {
            // Waveform memory: 32 bytes a channel. The window covers five
            // tables' worth of addresses; what lies past the fourth exists
            // only on the SCC+ (some rips reach channel 5 here rather than
            // through port 4).
            0x00 => {
                let (table, at) = (addr / 32, addr % 32);
                if table < 4 || (table == 4 && self.plus) {
                    self.waves[table][at] = data as i8;
                }
            }
            // Frequency: two registers per channel, low byte then the top
            // four bits.
            0x01 => {
                let channel = addr / 2;
                if let Some(ch) = self.channels.get_mut(channel) {
                    let period = if addr.is_multiple_of(2) {
                        (ch.period & 0x0F00) | u16::from(data)
                    } else {
                        (ch.period & 0x00FF) | (u16::from(data & 0x0F) << 8)
                    };
                    ch.set_period(period);
                }
            }
            // Volume, one register per channel.
            0x02 => {
                if let Some(ch) = self.channels.get_mut(addr) {
                    ch.volume = data & 0x0F;
                }
            }
            // The enable mask: one register, a bit per channel. The offset
            // is ignored -- there is only the one.
            0x03 => {
                for (index, ch) in self.channels.iter_mut().enumerate() {
                    ch.enabled = data & (1 << index) != 0;
                }
            }
            // The SCC+'s own window onto channel 5's wave memory.
            0x04 if self.plus => self.waves[4][addr % 32] = data as i8,
            // Port 5 is the deformation/test register: not modelled, and
            // port 4 on the plain SCC does not exist.
            _ => {}
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut sum = 0i32;
            for (index, ch) in self.channels.iter_mut().enumerate() {
                ch.phase = ch.phase.wrapping_add(ch.step);
                if !ch.enabled || ch.volume == 0 {
                    continue;
                }
                // Channel 5 reads table 4 on the plain SCC -- the sharing
                // that defines the original chip's sound.
                let table = if index == 4 && !self.plus { 3 } else { index };
                let sample = self.waves[table][(ch.phase >> 16) as usize % 32];
                sum += i32::from(sample) * i32::from(ch.volume) * VOLUME_SCALE;
            }
            // Mono: one output pin, mixed on the cartridge.
            frame[0] = sum;
            frame[1] = sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MSX's SCC clock.
    const CLOCK: u32 = 1_789_772;

    fn render(chip: &mut K051649, frames: usize) -> Vec<i32> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| f[0]).collect()
    }

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// A loud square wave into channel 1.
    fn key_on(chip: &mut K051649) {
        for at in 0..32u16 {
            chip.write(0, at, if at < 16 { 0x7F } else { 0x80 });
        }
        chip.write(1, 0, 0x6D); // period 0x16D: roughly A440
        chip.write(1, 1, 0x01);
        chip.write(2, 0, 0x0F); // full volume
        chip.write(3, 0, 0x01); // enable channel 1
    }

    #[test]
    fn a_fresh_chip_is_silent_and_an_enabled_one_is_not() {
        let mut chip = K051649::new();
        chip.reset(CLOCK, false);
        assert_eq!(energy(&render(&mut chip, 500)), 0);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 2000)) > 0);
    }

    /// Pitch counted in wave cycles rather than asserted from the formula:
    /// the tone is `clock / (32 x (period + 1))`.
    #[test]
    fn a_tone_sounds_at_the_documented_frequency() {
        let mut chip = K051649::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        let second = chip.native_rate() as usize;
        let samples = render(&mut chip, second);
        let mut crossings = 0u32;
        for pair in samples.windows(2) {
            if (pair[0] >= 0) != (pair[1] >= 0) {
                crossings += 1;
            }
        }
        let cycles = crossings / 2;
        let expected = CLOCK / (32 * (0x16D + 1));
        let drift = cycles.abs_diff(expected);
        assert!(
            drift * 50 <= expected,
            "counted {cycles} Hz, expected about {expected}"
        );
    }

    /// **Channel 5 plays channel 4's table on the SCC.** Writing table 4 and
    /// enabling only channel 5 must sound on the plain chip; on the SCC+ the
    /// same writes leave channel 5 with its own -- empty -- table.
    #[test]
    fn channel_five_shares_table_four_on_the_plain_scc() {
        for (plus, should_sound) in [(false, true), (true, false)] {
            let mut chip = K051649::new();
            chip.reset(CLOCK, plus);
            for at in 0..32u16 {
                chip.write(0, 0x60 + at, if at < 16 { 0x7F } else { 0x80 });
            }
            chip.write(1, 8, 0x40); // channel 5 period
            chip.write(1, 9, 0x01);
            chip.write(2, 4, 0x0F);
            chip.write(3, 0, 0x10); // enable channel 5 only
            let loud = energy(&render(&mut chip, 2000)) > 0;
            assert_eq!(loud, should_sound, "plus={plus}");
        }
    }

    /// The SCC+'s port-4 window reaches channel 5's own memory -- and does
    /// not exist on the plain chip.
    #[test]
    fn the_port_four_window_is_scc_plus_only() {
        for (plus, should_sound) in [(true, true), (false, false)] {
            let mut chip = K051649::new();
            chip.reset(CLOCK, plus);
            for at in 0..32u16 {
                chip.write(4, at, if at < 16 { 0x7F } else { 0x80 });
            }
            chip.write(1, 8, 0x40);
            chip.write(1, 9, 0x01);
            chip.write(2, 4, 0x0F);
            chip.write(3, 0, 0x10);
            let loud = energy(&render(&mut chip, 2000)) > 0;
            assert_eq!(loud, should_sound, "plus={plus}");
        }
    }

    /// The enable mask gates exactly the channels it names.
    #[test]
    fn the_enable_mask_gates_by_bit() {
        let mut chip = K051649::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        assert!(energy(&render(&mut chip, 1000)) > 0);
        chip.write(3, 0, 0x00);
        assert_eq!(energy(&render(&mut chip, 1000)), 0);
    }

    /// A zero period must not divide by zero.
    #[test]
    fn a_zero_period_behaves_as_one() {
        let mut chip = K051649::new();
        chip.reset(CLOCK, false);
        key_on(&mut chip);
        chip.write(1, 0, 0x00);
        chip.write(1, 1, 0x00);
        render(&mut chip, 100);
    }
}
