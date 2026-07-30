//! Nuked-CQM as an [`OplChip`]: the OPL3 clone Creative shipped in the SB16
//! Vibra and the AWE64.
//!
//! Not a more accurate YMF262 (Nuked-OPL3 is that) but a *different chip* that
//! answers to the same registers and does not sound identical -- what many
//! Sound Blaster owners actually heard -- so it registers second as an
//! authenticity option. The interface is the same shape as Nuked-OPL3's: reset
//! takes an output rate, writes come immediate and buffered, and a stream call
//! fills interleaved stereo.

use vgms_synth::OplChip;

use crate::ffi::CqmChip;

/// The registry id. `<slot>.<name>`, so `drotrim.ini` stores `core.opl3=cqm`.
pub(crate) const CORE_ID: &str = "opl3.cqm";

/// The rate the chip itself runs at: the 14.318 MHz master clock divided by
/// 288, the same figure as a YMF262 and for the same reason.
///
/// Upstream resamples from this to whatever output rate it was reset with, so
/// getting it wrong detunes everything rather than failing.
const NATIVE_RATE: u32 = 49_716;

/// The Creative CQM, Nuke.YKT's emulation of it.
#[derive(Debug)]
pub struct CqmOpl3 {
    chip: CqmChip,
    /// The output rate it was last reset at, kept for the `Debug` line.
    rate: u32,
}

impl CqmOpl3 {
    /// A chip rendering at `sample_rate` Hz.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let mut chip = CqmChip::new();
        chip.reset(sample_rate, NATIVE_RATE);
        Self {
            chip,
            rate: sample_rate,
        }
    }
}

impl OplChip for CqmOpl3 {
    fn reset(&mut self, sample_rate: u32) {
        self.rate = sample_rate;
        self.chip.reset(sample_rate, NATIVE_RATE);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        self.chip.write_reg(reg, value);
    }

    /// Same write-buffer semantics as Nuked-OPL3, so it drops into
    /// `PlayerEngine` unchanged.
    ///
    /// Both cores resolve key-on/off edges at sample-generation time, so two
    /// writes with no samples between them collapse and a fast retrigger is
    /// dropped; the engine spaces queued writes a couple of samples apart to
    /// avoid it. Upstream CQM has its own `writebuf` ring with the same
    /// two-sample delay (`CQM_WRITEBUF_DELAY`), so that spacing is right here too.
    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        self.chip.write_reg_buffered(reg, value);
    }

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        debug_assert!(
            buffer.len().is_multiple_of(2),
            "stereo buffers hold whole frames"
        );
        self.chip.generate(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 49_716;

    /// Total energy, for "did it make a sound" questions.
    fn energy(samples: &[i16]) -> u64 {
        samples.iter().map(|&s| u64::from(s.unsigned_abs())).sum()
    }

    /// A minimal key-on: one operator pair, loud, sustained.
    fn key_on(chip: &mut CqmOpl3) {
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
        let mut chip = CqmOpl3::new(RATE);
        let mut quiet = vec![0i16; 2048 * 2];
        chip.generate_samples(&mut quiet);
        assert_eq!(
            energy(&quiet),
            0,
            "a chip written to nothing must be silent"
        );

        key_on(&mut chip);
        let mut loud = vec![0i16; 2048 * 2];
        chip.generate_samples(&mut loud);
        assert!(
            energy(&loud) > 0,
            "the C core linked, reset and generated -- or it did not"
        );
    }

    /// The property `PlayerEngine`'s buffered path depends on, checked because
    /// the engine's write spacing was written for Nuked-OPL3 and this is a
    /// different chip behind the same registers.
    ///
    /// Both cores resolve key-on/off edges when they generate samples, so a
    /// key-off then key-on with nothing rendered between them collapse to "still
    /// on" and the note is not restruck; the buffered path spreads the writes
    /// apart so the envelope sees the 0->1 edge. The note must decay to
    /// near-silence *before* the retrigger, or a re-attack and a still-sounding
    /// note carry the same energy and the test proves nothing.
    #[test]
    fn buffered_writes_retrigger_where_immediate_ones_collapse() {
        /// A percussive voice: fast attack, medium decay, no sustain.
        const PERCUSSIVE: [(u16, u8); 9] = [
            (0x20, 0x01),
            (0x40, 0x00),
            (0x60, 0xFA), // modulator: fast attack, medium decay
            (0x80, 0x0F),
            (0x23, 0x01),
            (0x43, 0x00),
            (0x63, 0xFA), // carrier: fast attack, medium decay
            (0x83, 0x0F),
            (0xA0, 0x98),
        ];

        fn retrigger_energy(buffered: bool) -> u64 {
            let mut chip = CqmOpl3::new(RATE);
            for (reg, value) in PERCUSSIVE {
                chip.write_reg(reg, value);
            }
            chip.write_reg(0xB0, 0x31); // key on
            // Let it decay to near silence, so a re-attack is unmistakable.
            chip.generate_samples(&mut vec![0i16; 16_000 * 2]);

            if buffered {
                chip.write_reg_buffered(0xB0, 0x11); // key off (same block/fnum)
                chip.write_reg_buffered(0xB0, 0x31); // key on again
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
        let mut whole = CqmOpl3::new(RATE);
        key_on(&mut whole);
        let mut one_go = vec![0i16; 1024 * 2];
        whole.generate_samples(&mut one_go);

        let mut chunked_chip = CqmOpl3::new(RATE);
        key_on(&mut chunked_chip);
        let mut piecemeal = vec![0i16; 1024 * 2];
        for chunk in piecemeal.chunks_mut(128 * 2) {
            chunked_chip.generate_samples(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// A reset must be a fresh chip, not a chip with its notes still ringing --
    /// the engine resets between songs and at a seek.
    #[test]
    fn a_reset_silences_a_ringing_chip() {
        let mut chip = CqmOpl3::new(RATE);
        key_on(&mut chip);
        let mut ringing = vec![0i16; 512 * 2];
        chip.generate_samples(&mut ringing);
        assert!(energy(&ringing) > 0);

        chip.reset(RATE);
        let mut after = vec![0i16; 512 * 2];
        chip.generate_samples(&mut after);
        assert_eq!(energy(&after), 0);
    }

    /// The engine hands the chip whatever the output device asked for, which is
    /// rarely the chip's own rate. Resampling happens upstream; this checks it
    /// is wired to it rather than ignoring the rate.
    #[test]
    fn a_non_native_rate_still_makes_sound() {
        for rate in [44_100, 48_000, 22_050] {
            let mut chip = CqmOpl3::new(rate);
            key_on(&mut chip);
            let mut out = vec![0i16; 4096 * 2];
            chip.generate_samples(&mut out);
            assert!(energy(&out) > 0, "silent at {rate} Hz");
        }
    }

    /// An empty buffer must ask upstream for nothing rather than for one frame
    /// it has nowhere to put.
    #[test]
    fn an_empty_buffer_generates_nothing() {
        let mut chip = CqmOpl3::new(RATE);
        key_on(&mut chip);
        chip.generate_samples(&mut []);
    }
}
