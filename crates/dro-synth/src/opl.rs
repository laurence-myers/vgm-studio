//! The OPL chip behind a trait, so the emulator core stays swappable.

use core::fmt;

#[cfg(feature = "nuked-opl")]
use nuked_opl3::Opl3Chip;

/// An OPL2/OPL3 chip that turns register writes into stereo PCM.
///
/// Implementations must be deterministic and free of floating-point rounding
/// differences across targets: native and wasm builds are expected to produce
/// bit-identical output, and a test asserts exactly that.
///
/// `Debug + Send` because a chip is now chosen at *runtime* -- the Settings
/// core picker means the engine holds a `Box<dyn OplChip>` -- and the engine it
/// lives in is both printed and moved into an audio callback on another thread.
pub trait OplChip: fmt::Debug + Send {
    /// Discards all chip state and re-initialises at `sample_rate`.
    fn reset(&mut self, sample_rate: u32);

    /// Writes `value` to `reg`, taking effect immediately. Bit 8 of `reg`
    /// selects the high register bank.
    fn write_reg(&mut self, reg: u16, value: u8);

    /// Writes `value` to `reg` through the chip's write buffer, if it has one.
    ///
    /// Nuked-OPL3 resolves key-on/off edges at sample-generation time, so writes
    /// with no samples rendered between them collapse to their net state and fast
    /// retriggers are silently dropped. The buffered path spreads queued writes a
    /// couple of samples apart during generation so every edge is observed --
    /// matching real hardware, where
    /// register writes are inherently spaced in time. Live playback and rendering
    /// use this; a seek's bulk replay uses [`Self::write_reg`], where only the
    /// final register *values* matter. The default is an immediate write.
    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        self.write_reg(reg, value);
    }

    /// Fills `buffer` with interleaved stereo samples.
    ///
    /// `buffer.len()` must be even; it holds `buffer.len() / 2` frames. The
    /// output must not depend on how the caller chunks its calls, so an
    /// `AudioWorklet` pulling 128 frames at a time gets the same audio as an
    /// offline render pulling 4096.
    fn generate_samples(&mut self, buffer: &mut [i16]);
}

/// Nuked-OPL3, the reference-accurate YMF262 emulation, via the pure Rust
/// [`nuked_opl3`] port.
///
/// Behind the default-on `nuked-opl` feature, because the port is
/// LGPL-2.1-or-later and this crate's own license is `MIT OR Apache-2.0`: a
/// consumer who needs a genuinely permissive build turns it off and gets
/// [`SilentOpl`] in its place. See `licenses/README.md`.
///
/// The port pulls in no C toolchain, so `wasm32-unknown-unknown` builds have an
/// empty import section. That it really is bit-identical to Nuke.YKT's C original
/// is asserted, not assumed: see [`CReferenceOpl3`] and `tests/c_parity.rs`.
#[cfg(feature = "nuked-opl")]
pub struct NukedOpl3 {
    chip: Opl3Chip,
}

#[cfg(feature = "nuked-opl")]
impl NukedOpl3 {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            chip: Opl3Chip::new(sample_rate),
        }
    }
}

#[cfg(feature = "nuked-opl")]
impl fmt::Debug for NukedOpl3 {
    /// `Opl3Chip` is ~30 KiB of register and operator state with no `Debug` of
    /// its own; the voice count is the only part worth printing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NukedOpl3")
            .field("active_voices", &self.chip.active_voice_count())
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "nuked-opl")]
impl OplChip for NukedOpl3 {
    fn reset(&mut self, sample_rate: u32) {
        // A fresh chip state.
        self.chip.reset(sample_rate);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        self.chip.write_register(reg, value);
    }

    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        self.chip.write_register_buffered(reg, value);
    }

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        debug_assert!(
            buffer.len().is_multiple_of(2),
            "stereo buffers hold whole frames"
        );
        if buffer.len() < 2 {
            return;
        }
        self.chip
            .generate_stream(buffer)
            .expect("buffer holds at least one frame");
    }
}

/// A boxed chip is a chip.
///
/// What makes the core picker possible: the engine is generic over `C: OplChip`
/// and stays so, but `C` can now be `Box<dyn OplChip>` chosen from the registry
/// at load time. Without this the choice would have to be a compile-time type,
/// which is exactly the arrangement the registry exists to undo.
impl OplChip for Box<dyn OplChip> {
    fn reset(&mut self, sample_rate: u32) {
        (**self).reset(sample_rate);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        (**self).write_reg(reg, value);
    }

    fn write_reg_buffered(&mut self, reg: u16, value: u8) {
        // Forwarded rather than left to the trait's default, which would turn
        // every buffered write into an immediate one and silently drop fast
        // retriggers -- the exact bug the buffered path exists to avoid.
        (**self).write_reg_buffered(reg, value);
    }

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        (**self).generate_samples(buffer);
    }
}

/// The OPL core a build without one has: silence.
///
/// Only reachable with the `nuked-opl` feature off. The point is that turning
/// the feature off must not delete the OPL *path* -- muting, panning, seeking,
/// the channel split and the whole `PlayerEngine` state machine are file-format
/// logic, not emulation, and a permissive consumer wants all of it. So the chip
/// goes quiet and everything else keeps working. The registry registers no OPL
/// core in that build, so the UI reports the silence rather than implying sound.
#[derive(Debug, Default, Clone, Copy)]
pub struct SilentOpl;

impl SilentOpl {
    /// Takes the sample rate every OPL core's constructor takes, and ignores it.
    #[must_use]
    pub const fn new(_sample_rate: u32) -> Self {
        Self
    }
}

impl OplChip for SilentOpl {
    fn reset(&mut self, _sample_rate: u32) {}

    fn write_reg(&mut self, _reg: u16, _value: u8) {}

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        buffer.fill(0);
    }
}

/// The OPL core [`PlayerEngine::new`](crate::engine::PlayerEngine::new) builds.
///
/// A type alias rather than a `cfg` on the engine itself, so the engine has one
/// definition whichever way the feature falls.
#[cfg(feature = "nuked-opl")]
pub type DefaultOplChip = NukedOpl3;

/// The OPL core [`PlayerEngine::new`](crate::engine::PlayerEngine::new) builds
/// when this crate is compiled without an OPL emulator.
#[cfg(not(feature = "nuked-opl"))]
pub type DefaultOplChip = SilentOpl;

/// The original Nuked-OPL3 **C** sources, compiled by `opl3-rs`.
///
/// A parity oracle, nothing more: `tests/c_parity.rs` runs the same register
/// stream through this and [`NukedOpl3`] and asserts the PCM matches sample for
/// sample. `nuked-opl3` is a young, single-maintainer, hand-optimised port, and
/// this is what keeps it honest.
///
/// Never build this for wasm -- `opl3-rs` needs a C sysroot that
/// `wasm32-unknown-unknown` does not have.
#[cfg(feature = "c-parity")]
pub struct CReferenceOpl3 {
    chip: opl3_rs::Opl3Chip,
}

#[cfg(feature = "c-parity")]
impl CReferenceOpl3 {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            chip: opl3_rs::Opl3Chip::new(sample_rate),
        }
    }
}

#[cfg(feature = "c-parity")]
impl fmt::Debug for CReferenceOpl3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CReferenceOpl3").finish_non_exhaustive()
    }
}

#[cfg(feature = "c-parity")]
impl OplChip for CReferenceOpl3 {
    fn reset(&mut self, sample_rate: u32) {
        self.chip.reset(sample_rate);
    }

    fn write_reg(&mut self, reg: u16, value: u8) {
        self.chip.write_register(reg, value);
    }

    fn generate_samples(&mut self, buffer: &mut [i16]) {
        if buffer.len() < 2 {
            return;
        }
        self.chip
            .generate_stream(buffer)
            .expect("buffer holds at least one frame");
    }
}

#[cfg(all(test, feature = "nuked-opl"))]
mod tests {
    use super::*;
    use crate::NATIVE_SAMPLE_RATE;

    #[test]
    fn a_fresh_chip_is_silent() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        let mut buffer = [0i16; 256];
        chip.generate_samples(&mut buffer);
        assert!(buffer.iter().all(|&sample| sample == 0));
    }

    #[test]
    fn keying_on_a_note_produces_sound() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        for (reg, value) in [
            (0x20u16, 0x01u8), // modulator: multiplier 1
            (0x40, 0x10),      // modulator: output level
            (0x60, 0xF0),      // modulator: fast attack, slow decay
            (0x80, 0x77),      // modulator: sustain / release
            (0x23, 0x01),      // carrier
            (0x43, 0x00),
            (0x63, 0xF0),
            (0x83, 0x77),
            (0xA0, 0x98), // frequency low
            (0xB0, 0x31), // key on, octave 4
        ] {
            chip.write_reg(reg, value);
        }
        let mut buffer = vec![0i16; 4096 * 2];
        chip.generate_samples(&mut buffer);
        assert!(buffer.iter().any(|&sample| sample != 0), "expected audio");
    }

    #[test]
    fn reset_silences_the_chip() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        chip.write_reg(0x20, 0x01);
        chip.write_reg(0xA0, 0x98);
        chip.write_reg(0xB0, 0x31);
        let mut buffer = vec![0i16; 1024];
        chip.generate_samples(&mut buffer);

        chip.reset(NATIVE_SAMPLE_RATE);
        buffer.fill(0);
        chip.generate_samples(&mut buffer);
        assert!(buffer.iter().all(|&sample| sample == 0));
    }

    #[test]
    fn undersized_buffers_are_ignored_rather_than_panicking() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        chip.generate_samples(&mut []);
    }

    /// The OPL's operator registers have gaps: `0x26`, `0x27`, `0x2E`, `0x2F` and
    /// their counterparts in the 0x40/0x60/0x80/0xE0 banks address no operator.
    /// DOSBox logs whatever a game writes, so a capture can contain them.
    ///
    /// `nuked-opl3` 0.1.0 overflows on those writes under debug overflow checks
    /// (see the `[profile.dev.package.nuked-opl3]` note in the workspace manifest).
    /// This test is what notices if that workaround is dropped before upstream
    /// lands a fix.
    #[test]
    fn writing_a_gap_register_does_not_panic() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        for register in [
            0x26u16, 0x27, 0x2E, 0x2F, 0x46, 0x47, 0x4E, 0x4F, 0x66, 0x67, 0x6E, 0x6F, 0x86, 0x87,
            0x8E, 0x8F, 0xE6, 0xE7, 0xEE, 0xEF, 0xFF,
        ] {
            chip.write_reg(register, 0xFF);
            chip.write_reg(0x100 | register, 0xFF); // and in the high bank
        }
        let mut buffer = [0i16; 128];
        chip.generate_samples(&mut buffer);
    }

    /// The reason `write_reg_buffered` exists: a back-to-back key-off / key-on
    /// with no samples rendered between them must still retrigger the note.
    ///
    /// Nuked resolves the key edge at generation time, so the immediate path
    /// collapses the off/on to its net state ("still on") and the note is not
    /// restruck -- the very drop this fixes. The buffered path spreads the two
    /// writes a couple of samples apart, so the envelope sees the 0->1 edge and
    /// re-attacks, making the segment right after the retrigger far louder.
    #[test]
    fn buffered_writes_retrigger_a_back_to_back_key_on() {
        // A percussive note (EG-TYP clear): attacks, then decays to silence.
        const SETUP: [(u16, u8); 9] = [
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

        fn energy(samples: &[i16]) -> u64 {
            samples.iter().map(|&s| u64::from(s.unsigned_abs())).sum()
        }

        // Returns the energy of the segment rendered just after a back-to-back
        // key-off (`0x11`) + key-on (`0x31`) retrigger of a decayed note.
        fn retrigger_energy(buffered: bool) -> u64 {
            let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
            for (reg, value) in SETUP {
                chip.write_reg(reg, value);
            }
            chip.write_reg(0xB0, 0x31); // key on
            chip.generate_samples(&mut vec![0i16; 16_000 * 2]); // let it decay to near silence
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
            "buffered writes must retrigger the note: buffered={buffered} immediate={immediate}"
        );
    }

    /// Every register a DRO file can name, in both banks, must be writable.
    #[test]
    fn no_register_write_panics() {
        let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
        for register in 0x000u16..=0x1FF {
            chip.write_reg(register, 0xFF);
            chip.write_reg(register, 0x00);
        }
        let mut buffer = [0i16; 128];
        chip.generate_samples(&mut buffer);
    }
}
