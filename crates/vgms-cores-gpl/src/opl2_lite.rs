//! Nuked-OPL2-Lite as an [`OplChip`]: a high-accuracy *behavioural* OPL2,
//! realtime-fast where the YM3812 die sim is not.
//!
//! The middle rung of the OPL2 authenticity ladder: Nuked-OPL3 plays OPL2
//! captures as an OPL3 in compat mode would, YM3812-LLE plays the decapped
//! die below realtime, and this plays a *modelled* genuine OPL2 -- composite
//! sine mode, OPL2 DAC behaviour -- at emulator speed. Same author, same API
//! shape as Nuked-OPL3: reset at an output rate, immediate and buffered
//! writes, internal resampling.
//!
//! The chip is mono, as the YM3812 was; the wrapper mirrors its one DAC to
//! both sides. It has one register bank, so bank-1 writes (which only an
//! OPL3-family document produces) are dropped rather than aliased onto bank 0
//! -- a real OPL2 has no `A1` pin to see them with.

use vgms_synth::OplChip;

use crate::ffi::Opl2LiteChip;

/// The registry id: the OPL family shares the `opl3` slot, so this names the
/// core within it.
pub(crate) const CORE_ID: &str = "opl3.opl2-lite";

/// Nuked-OPL2-Lite, wrapped.
#[derive(Debug)]
pub struct Opl2Lite {
    chip: Opl2LiteChip,
    /// The output rate it was last reset at, kept for the `Debug` line.
    rate: u32,
}

impl Opl2Lite {
    /// A chip rendering at `sample_rate` Hz.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let mut chip = Opl2LiteChip::new();
        chip.reset(sample_rate);
        Self {
            chip,
            rate: sample_rate,
        }
    }
}

impl OplChip for Opl2Lite {
    fn reset(&mut self, sample_rate: u32) {
        self.rate = sample_rate;
        self.chip.reset(sample_rate);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        // One register bank: an OPL2 has no A1 pin, so a bank-1 write from an
        // OPL3-family document addresses nothing rather than aliasing.
        if reg > 0xFF {
            return;
        }
        self.chip.write_reg(reg as u8, value);
    }

    /// Same write-buffer semantics as Nuked-OPL3 (two-sample spacing), so the
    /// engine's buffered write path suits it unchanged.
    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        if reg > 0xFF {
            return;
        }
        self.chip.write_reg_buffered(reg as u8, value);
    }

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        debug_assert!(
            buffer.len().is_multiple_of(2),
            "stereo buffers hold whole frames"
        );
        for frame in buffer.chunks_exact_mut(2) {
            // Mono chip, one DAC: the same sample to both sides.
            let sample = self.chip.generate_resampled();
            frame[0] = sample;
            frame[1] = sample;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 49_716;

    fn energy(samples: &[i16]) -> u64 {
        samples.iter().map(|&s| u64::from(s.unsigned_abs())).sum()
    }

    /// A minimal key-on: one operator pair, loud, sustained.
    fn key_on(chip: &mut Opl2Lite) {
        chip.write_reg(0x20, 0x01); // modulator: multiple 1
        chip.write_reg(0x23, 0x01); // carrier: multiple 1
        chip.write_reg(0x40, 0x10); // modulator level
        chip.write_reg(0x43, 0x00); // carrier at full volume
        chip.write_reg(0x60, 0xF0); // fast attack
        chip.write_reg(0x63, 0xF0);
        chip.write_reg(0x80, 0x77); // sustain high, slow release
        chip.write_reg(0x83, 0x77);
        chip.write_reg(0xA0, 0x98); // frequency
        chip.write_reg(0xB0, 0x31); // block 2, key on
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_on_one_is_not() {
        let mut chip = Opl2Lite::new(RATE);
        let mut quiet = vec![0i16; 2048 * 2];
        chip.generate_samples(&mut quiet);
        assert_eq!(energy(&quiet), 0, "a chip written to nothing is silent");

        key_on(&mut chip);
        let mut loud = vec![0i16; 2048 * 2];
        chip.generate_samples(&mut loud);
        assert!(
            energy(&loud) > 0,
            "the C core linked, reset and generated -- or it did not"
        );
    }

    /// The chip is mono; the wrapper must mirror it, not stagger it.
    #[test]
    fn both_sides_carry_the_same_signal() {
        let mut chip = Opl2Lite::new(RATE);
        key_on(&mut chip);
        let mut out = vec![0i16; 1024 * 2];
        chip.generate_samples(&mut out);
        assert!(
            out.chunks_exact(2).all(|frame| frame[0] == frame[1]),
            "one DAC, two wires"
        );
    }

    /// A bank-1 write must address nothing: a real OPL2 has no A1 pin, so a
    /// stray OPL3-style write must not clobber the bank-0 register under it.
    #[test]
    fn a_bank_one_write_is_dropped_not_aliased() {
        let mut chip = Opl2Lite::new(RATE);
        key_on(&mut chip);
        // 0x1B0 aliased onto 0xB0 would key the note off.
        chip.write_reg(0x1B0, 0x11);
        let mut out = vec![0i16; 2048 * 2];
        chip.generate_samples(&mut out);
        assert!(energy(&out) > 0, "the note must still be sounding");
    }

    /// Same property as the other buffered-write cores: a back-to-back
    /// key-off/key-on with no samples between must still retrigger.
    #[test]
    fn buffered_writes_retrigger_where_immediate_ones_collapse() {
        /// A percussive voice: fast attack, medium decay, no sustain.
        const PERCUSSIVE: [(u16, u8); 9] = [
            (0x20, 0x01),
            (0x40, 0x00),
            (0x60, 0xFA),
            (0x80, 0x0F),
            (0x23, 0x01),
            (0x43, 0x00),
            (0x63, 0xFA),
            (0x83, 0x0F),
            (0xA0, 0x98),
        ];

        fn retrigger_energy(buffered: bool) -> u64 {
            let mut chip = Opl2Lite::new(RATE);
            for (reg, value) in PERCUSSIVE {
                chip.write_reg(reg, value);
            }
            chip.write_reg(0xB0, 0x31); // key on
            // Let it decay to near silence, so a re-attack is unmistakable.
            chip.generate_samples(&mut vec![0i16; 16_000 * 2]);

            if buffered {
                chip.write_reg_buffered(0xB0, 0x11);
                chip.write_reg_buffered(0xB0, 0x31);
            } else {
                chip.write_reg(0xB0, 0x11);
                chip.write_reg(0xB0, 0x31);
            }
            let mut segment = vec![0i16; 2000 * 2];
            chip.generate_samples(&mut segment);
            energy(&segment)
        }

        let buffered = retrigger_energy(true);
        let immediate = retrigger_energy(false);
        assert!(
            buffered > immediate * 4,
            "buffered writes must retrigger the note: \
             buffered={buffered} immediate={immediate}"
        );
    }

    /// Chunking must not change the audio, or an `AudioWorklet` pulling 128
    /// frames would sound different from an offline render pulling 4096.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let mut whole = Opl2Lite::new(RATE);
        key_on(&mut whole);
        let mut one_go = vec![0i16; 1024 * 2];
        whole.generate_samples(&mut one_go);

        let mut chunked = Opl2Lite::new(RATE);
        key_on(&mut chunked);
        let mut piecemeal = vec![0i16; 1024 * 2];
        for chunk in piecemeal.chunks_mut(128 * 2) {
            chunked.generate_samples(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// The engine hands the chip whatever rate the device negotiated; the
    /// internal resampler must be wired to it rather than ignoring it.
    #[test]
    fn a_non_native_rate_still_makes_sound() {
        for rate in [44_100, 48_000, 22_050] {
            let mut chip = Opl2Lite::new(rate);
            key_on(&mut chip);
            let mut out = vec![0i16; 4096 * 2];
            chip.generate_samples(&mut out);
            assert!(energy(&out) > 0, "silent at {rate} Hz");
        }
    }
}
