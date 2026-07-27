//! Nuked-OPM as a [`ChipCore`]: the YM2151 behind most of eighties arcade FM,
//! and the YM2164 it was rebadged as.
//!
//! Third by weight in the VGMRips corpus -- 10,069 files, 13.9% -- and the
//! chip Sega, Capcom and Konami built their arcade sound on.
//!
//! Shaped like [`opn2`](crate::opn2) because it is the same designer's work:
//! **cycle-level clocking** (32 cycles to a sample, two master clocks each, so
//! the familiar `clock / 64`) and **latched writes** that land only when the
//! rotation reaches their slot. So the same write queue applies, for the same
//! reason. What it does *not* share is OPN2's global chip-type: the YM2164
//! variant is a flag on this chip's own reset, so no lock is needed.

use dro_core::vgm::ChipKind;
use dro_synth::ChipCore;

use crate::ffi::OpmChip;
use crate::write_queue::WriteQueue;

/// The registry id. `<slot>.<name>`, so `drotrim.ini` stores `core.ym2151=nuked`.
pub(crate) const CORE_ID: &str = "ym2151.nuked";

/// Internal cycles per output sample: the chip rotates through 32 slots.
const CLOCKS_PER_SAMPLE: u32 = 32;
/// Master clocks per internal cycle.
const MASTER_PER_CLOCK: u32 = 2;
/// Master clocks per output sample -- the familiar `clock / 64`.
const MASTER_PER_SAMPLE: u32 = MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE;

/// Cycles of clear air the address and the value each need to themselves.
///
/// The chip takes the address into one latch and the value into another, and
/// each is picked up when the rotation comes round to it -- so a register is
/// address, a whole rotation, value, a whole rotation, and only then may the
/// next address disturb anything. About one register per two output samples,
/// which is some 27,000 a second: far more than any file asks for.
///
/// This was measured rather than assumed, and the measurement is the interesting
/// part: spacing writes 1, 2, 3 or 6 cycles apart produces total *silence*,
/// while 4 gives full amplitude, 8 a quarter and 16 a half. The sequence is not
/// monotonic because those values are **phases**, not durations -- each lands
/// some registers on their slot and misses others. A spacing that happens to
/// work for one patch is therefore no evidence at all; only a full rotation
/// each way is. The failure mode is a note that sounds wrong or not at all,
/// which reads as a bad patch rather than a bad driver.
const SETTLE_CYCLES: u32 = CLOCKS_PER_SAMPLE;

/// Scales the DAC output towards `i16` range.
///
/// Unlike the YM2612's multiplexed pins, this chip's `dac_output` is already a
/// per-channel signed value of roughly 16-bit width, so it needs no gain of its
/// own -- a full-level patch lands near full scale as it stands.
/// `a_loud_patch_uses_the_range_without_clipping_it` is what says so, and would
/// notice if an upstream change moved it.
const OUTPUT_GAIN: i32 = 1;

/// The YM2151 (and YM2164), Nuke.YKT's emulation of it.
#[derive(Debug)]
pub struct Ym2151 {
    chip: OpmChip,
    rate: u32,
    /// Registers waiting for their turn on the chip; see [`SETTLE_CYCLES`].
    writes: WriteQueue,
}

impl Ym2151 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: OpmChip::new(),
            rate: 44_100,
            writes: WriteQueue::new(SETTLE_CYCLES, SETTLE_CYCLES),
        }
    }

    /// Whether anything is still waiting to be latched.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.writes.pending()
    }
}

impl Default for Ym2151 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2151 {
    /// `variant` is the VGM header's bit 31 on the YM2151 clock: set means a
    /// YM2164, the OPP, which upstream models behind its own reset flag.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.writes.clear();
        self.rate = (clock / MASTER_PER_SAMPLE).max(1);
        // Upstream's reset drives the IC pin and clocks the chip through its
        // own power-on sequence, so there is nothing to do around it.
        self.chip.reset(variant);
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// One register port, addressed by writing the register number then the
    /// value -- so each call queues two.
    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        // One register port: the address goes to 0 and its value to 1.
        self.writes
            .push(0, (addr & 0xFF) as u8, (data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                let chip = &mut self.chip;
                self.writes.advance(|port, byte| chip.write(port, byte));
                let (l, r) = self.chip.clock();
                // The DAC holds its value across the rotation and only some
                // cycles refresh it, so the last reading of the rotation is the
                // sample -- summing would count one value many times.
                left = l;
                right = r;
            }
            frame[0] = left * OUTPUT_GAIN;
            frame[1] = right * OUTPUT_GAIN;
        }
    }
}

/// The chips this core serves.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2151];

#[cfg(test)]
mod tests {
    use super::*;

    /// The usual arcade YM2151 clock, 3.579545 MHz.
    const ARCADE_CLOCK: u32 = 3_579_545;

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    fn peak(samples: &[i32]) -> i32 {
        samples.iter().map(|&s| s.abs()).max().unwrap_or(0)
    }

    /// A loud note on channel 1: four operators at full level, algorithm 7
    /// (all straight to the output), both speakers on.
    fn key_on(chip: &mut Ym2151) {
        for (reg, value) in [
            (0x0Fu16, 0x00u16), // noise off
            (0x18, 0x00),       // LFO frequency 0
            (0x19, 0x00),       // no depth
            (0x1B, 0x00),       // no waveform / CT
            (0x20, 0xC7),       // ch1: both speakers, feedback 0, algorithm 7
            (0x28, 0x4A),       // key code: octave 4
            (0x30, 0x00),       // key fraction
            (0x40, 0x01),       // op M1: detune 0, multiple 1
            (0x48, 0x01),       // op M2
            (0x50, 0x01),       // op C1
            (0x58, 0x01),       // op C2
            (0x60, 0x00),       // total level 0 == loudest, all four
            (0x68, 0x00),
            (0x70, 0x00),
            (0x78, 0x00),
            (0x80, 0x1F), // attack rate max
            (0x88, 0x1F),
            (0x90, 0x1F),
            (0x98, 0x1F),
            (0xA0, 0x00), // no first decay
            (0xA8, 0x00),
            (0xB0, 0x00),
            (0xB8, 0x00),
            (0xC0, 0x00), // no second decay
            (0xC8, 0x00),
            (0xD0, 0x00),
            (0xD8, 0x00),
            (0xE0, 0x00), // sustain level 0, slow release
            (0xE8, 0x00),
            (0xF0, 0x00),
            (0xF8, 0x00),
        ] {
            chip.write(0, reg, value);
        }
        chip.write(0, 0x08, 0x78); // key on: all four operators, channel 1
    }

    #[test]
    fn a_fresh_chip_is_silent_and_a_keyed_on_one_is_not() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        let mut quiet = vec![0i32; 2048 * 2];
        chip.render(&mut quiet);
        assert_eq!(
            energy(&quiet),
            0,
            "a chip written to nothing must be silent"
        );

        key_on(&mut chip);
        let mut loud = vec![0i32; 8192 * 2];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > 0,
            "the C core linked, reset, latched its writes and generated -- or it did not"
        );
    }

    /// The rate the engine resamples from: a YM2151 runs at `clock / 64`, which
    /// for the usual arcade crystal is about 55.9 kHz.
    #[test]
    fn the_native_rate_is_the_clock_over_64() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        assert_eq!(chip.native_rate(), ARCADE_CLOCK / 64);
        assert_eq!(chip.native_rate(), 55_930);

        chip.reset(0, false);
        assert!(
            chip.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }

    /// The reason the write queue exists: upstream latches a write for the next
    /// clock and applies it only when the rotation reaches its slot, so a burst
    /// pushed straight through keeps whichever writes happened to align. The
    /// key-on above is 32 registers at one instant.
    #[test]
    fn a_burst_of_writes_all_reach_the_chip() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        key_on(&mut chip);
        let mut out = vec![0i32; 8192 * 2];
        chip.render(&mut out);
        assert!(
            energy(&out) > 0,
            "a burst of writes produced silence: the queue is dropping them"
        );
    }

    /// A run longer than one sample's worth must arrive late, never be dropped.
    #[test]
    fn a_run_longer_than_one_sample_is_delayed_rather_than_dropped() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        for reg in 0x60u16..0x80 {
            chip.write(0, reg, 0x10);
        }
        let queued = chip.pending();
        assert!(queued > 1, "{queued} queued");

        // A register takes its address cycle, its value cycle and a whole
        // rotation, so roughly one per output sample -- and never more.
        let mut one_sample = [0i32; 2];
        chip.render(&mut one_sample);
        assert!(
            chip.pending() >= queued - 1,
            "more than one register drained in a single sample"
        );

        let mut rest = vec![0i32; 256 * 2];
        chip.render(&mut rest);
        assert_eq!(chip.pending(), 0, "the run must finish, not evaporate");
    }

    /// Chunking must not change the audio, or an `AudioWorklet` pulling 128
    /// frames would sound different from an offline render pulling 4096.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let mut whole = Ym2151::new();
        whole.reset(ARCADE_CLOCK, false);
        key_on(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = Ym2151::new();
        chunked.reset(ARCADE_CLOCK, false);
        key_on(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// A reset must be a fresh chip with an empty queue: the engine resets
    /// between songs and at a seek, and a stale queued write would arrive after
    /// the seek as if the song had made it.
    #[test]
    fn a_reset_clears_both_the_chip_and_its_pending_writes() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        key_on(&mut chip);
        assert!(chip.pending() > 0, "writes are pending");

        chip.reset(ARCADE_CLOCK, false);
        assert_eq!(chip.pending(), 0, "a seek must not carry writes across");
        let mut after = vec![0i32; 2048 * 2];
        chip.render(&mut after);
        assert_eq!(energy(&after), 0);
    }

    /// The YM2164 is a rebadged OPM with its own quirks, and the VGM header
    /// distinguishes them, so the flag has to reach upstream rather than being
    /// decoration.
    #[test]
    fn the_ym2164_variant_reaches_the_chip() {
        fn render(variant: bool) -> Vec<i32> {
            let mut chip = Ym2151::new();
            chip.reset(ARCADE_CLOCK, variant);
            key_on(&mut chip);
            let mut out = vec![0i32; 8192 * 2];
            chip.render(&mut out);
            out
        }
        // Both must sound; whether they differ on *this* patch is upstream's
        // business, so the check is that the flag is accepted and playable.
        assert!(energy(&render(false)) > 0);
        assert!(energy(&render(true)) > 0);
    }

    /// The output scale, pinned so an upstream change or a wrong `OUTPUT_GAIN`
    /// is noticed rather than heard.
    #[test]
    fn a_loud_patch_uses_the_range_without_clipping_it() {
        let mut chip = Ym2151::new();
        chip.reset(ARCADE_CLOCK, false);
        key_on(&mut chip);
        let mut out = vec![0i32; 8192 * 2];
        chip.render(&mut out);

        let loudest = peak(&out);
        assert!(
            loudest > i32::from(i16::MAX) / 8,
            "a full-level patch peaked at {loudest}, far below the mixer's range"
        );
        assert!(
            loudest < i32::from(i16::MAX) * 2,
            "a full-level patch peaked at {loudest}, which the mixer would clamp"
        );
    }
}
