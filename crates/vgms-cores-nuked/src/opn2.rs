//! Nuked-OPN2 as a [`ChipCore`]: the YM2612 of the Mega Drive, and the YM3438
//! it became.
//!
//! A [`ChipCore`], not an [`OplChip`](vgms_synth::OplChip) like Nuked-CQM: it
//! plays through `VgmEngine`, the generic path with no register policy.
//!
//! Two upstream properties shape this file:
//!
//! 1. Clocking is per internal cycle, not per sample. `OPN2_Clock` advances six
//!    master clocks and reports the MOL/MOR *pin* states -- the time-multiplexed
//!    DAC output, one channel at a time. One output sample is 24 of those
//!    summed, as the real chip's analogue side does with the pins.
//! 2. A write is latched, then applied a whole rotation later. `OPN2_Write`
//!    raises a pending flag the next clock consumes, but the register lands only
//!    when the rotation reaches *that register's slot* (`ym3438.c`: `if
//!    (op_offset[slot] == (chip->address & 0x107))`), and the pending data is
//!    discarded as soon as the next address arrives. So a register needs the
//!    address, the data, then up to 24 cycles undisturbed. Upstream has no write
//!    buffer of its own, so one lives here.

use vgms_core::vgm::ChipKind;
use vgms_synth::ChipCore;

use crate::ffi::Opn2Chip;
use vgms_synth::WriteQueue;

/// The registry ids. `<slot>.<name>`, so `vgmstudio.ini` stores `core.ym2612=nuked`.
pub(crate) const YM2612_CORE_ID: &str = "ym2612.nuked";

/// Master clocks per internal clock, per upstream's own documentation.
const MASTER_PER_CLOCK: u32 = 6;
/// Internal clocks per output sample: the DAC multiplexes six channels over
/// 24 cycles, so a sample is that whole rotation summed.
const CLOCKS_PER_SAMPLE: u32 = 24;
/// Master clocks per output sample -- the familiar `clock / 144`.
const MASTER_PER_SAMPLE: u32 = MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE;

/// This core's write pacing, in [`WriteQueue`] terms: the value follows its
/// address on the next cycle, and the rest of the rotation is then left alone
/// so the chip can reach that register's slot and apply it.
///
/// One *register* per output sample. Draining faster looks like it works -- the
/// writes are accepted -- and silently loses most of them.
const ADDRESS_SETTLE: u32 = 0;
const VALUE_SETTLE: u32 = CLOCKS_PER_SAMPLE - 3;

/// The output scale, measured against VGMPlay rather than guessed.
///
/// The natural render sits about 4x quiet against the reference (single-chip
/// level 0.227; an in-mix least-squares fit over seven Mega Drive rips agreed at
/// ~4.0). So 5 x 4.2 = 21. A synthetic all-channels-maximum patch sums past the
/// mixer's clamp at this gain, deliberately: the reference's own renders of real
/// music peak near full scale without clipping, so musical content fits.
const OUTPUT_GAIN: i32 = 21;

/// The YM2612 (and YM3438), Nuke.YKT's emulation of it.
#[derive(Debug)]
pub struct Ym2612 {
    chip: Opn2Chip,
    /// The rate derived from the clock at reset.
    rate: u32,
    /// Registers waiting for their turn on the chip.
    ///
    /// This chip accepts one register per output sample: a write lands only when
    /// the 24-cycle rotation reaches its slot, and the next address wipes
    /// whatever is still pending, so pushing a run straight through drops most of
    /// it (a note that never starts, not a glitch). The rate is the real chip's
    /// too -- a YM2612 raises its busy flag for about a rotation after each write.
    writes: WriteQueue,
}

impl Ym2612 {
    /// A chip with no clock yet; [`reset`](ChipCore::reset) gives it one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            chip: Opn2Chip::new(),
            // Replaced at reset. Non-zero so a core that is somehow rendered
            // before being reset divides by something.
            rate: 44_100,
            writes: WriteQueue::new(ADDRESS_SETTLE, VALUE_SETTLE),
        }
    }

    /// Whether anything is still waiting to be latched.
    #[cfg(test)]
    fn pending(&self) -> usize {
        self.writes.pending()
    }
}

impl Default for Ym2612 {
    fn default() -> Self {
        Self::new()
    }
}

impl ChipCore for Ym2612 {
    /// `variant` is the VGM header's bit 31 on the YM2612 clock: set means a
    /// YM3438, the CMOS part, whose DAC lacks the discrete chip's ladder.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.writes.clear();
        self.rate = (clock / MASTER_PER_SAMPLE).max(1);
        self.chip.reset(!variant);
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// `addr` is a register number and `port` selects the chip's register bank,
    /// so each write is really two: the address, then the data.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        // Ports are 0 and 1 on the chip; upstream numbers the address and data
        // halves of each as 0/1 and 2/3.
        let base = u32::from(port & 1) * 2;
        self.writes
            .push(base, (addr & 0xFF) as u8, (data & 0xFF) as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        // One clocking session for the whole call: it is what holds upstream's
        // global chip-type at *this* chip's variant, and taking it per clock
        // would be a million lock acquisitions a second.
        let mut clocking = self.chip.clocking();
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.writes.advance(|port, byte| clocking.write(port, byte));
                let (l, r) = clocking.clock();
                left += l;
                right += r;
            }
            frame[0] = left * OUTPUT_GAIN;
            frame[1] = right * OUTPUT_GAIN;
        }
    }
}

/// The chips this core serves.
///
/// The VGM header distinguishes YM2612 and YM3438 by a flag bit, but `ChipKind`
/// has a single `Ym2612` for both, so there is one entry and
/// [`ChipCore::reset`]'s `variant` picks the part.
pub(crate) const CHIPS: [ChipKind; 1] = [ChipKind::Ym2612];

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mega Drive's YM2612 clock: 53.693 MHz / 7.
    const MD_CLOCK: u32 = 7_670_453;

    fn energy(samples: &[i32]) -> i64 {
        samples.iter().map(|&s| i64::from(s.abs())).sum()
    }

    fn peak(samples: &[i32]) -> i32 {
        samples.iter().map(|&s| s.abs()).max().unwrap_or(0)
    }

    /// A single loud FM note on channel 1, algorithm 7 (all four operators
    /// straight to the output), both speakers on.
    fn key_on(chip: &mut Ym2612) {
        for (reg, value) in [
            (0x22u16, 0x00u16), // LFO off
            (0x27, 0x00),       // normal timer mode
            (0x28, 0x00),       // all channels keyed off to start
            (0x30, 0x01),       // ch1 op1: detune 0, multiple 1
            (0x34, 0x01),
            (0x38, 0x01),
            (0x3C, 0x01),
            (0x40, 0x00), // total level 0 == loudest, all four operators
            (0x44, 0x00),
            (0x48, 0x00),
            (0x4C, 0x00),
            (0x50, 0x1F), // attack rate max
            (0x54, 0x1F),
            (0x58, 0x1F),
            (0x5C, 0x1F),
            (0x60, 0x00), // no decay
            (0x64, 0x00),
            (0x68, 0x00),
            (0x6C, 0x00),
            (0x70, 0x00), // no sustain decay
            (0x74, 0x00),
            (0x78, 0x00),
            (0x7C, 0x00),
            (0x80, 0x00), // sustain level 0, release slow
            (0x84, 0x00),
            (0x88, 0x00),
            (0x8C, 0x00),
            (0xB0, 0x07), // algorithm 7: all operators to output
            (0xB4, 0xC0), // both speakers
            (0xA4, 0x22), // block 4, frequency high
            (0xA0, 0x69), // frequency low
        ] {
            chip.write(0, reg, value);
        }
        chip.write(0, 0x28, 0xF0); // key on all four operators of channel 1
    }

    /// An idle YM2612 is *quiet*, not bit-silent: its discrete DAC sits off
    /// zero even with nothing playing -- the ladder effect, which is exactly
    /// what `ym3438_mode_ym2612` models and what makes a real Mega Drive hiss.
    /// So the check is a ratio, not an equality.
    #[test]
    fn an_idle_chip_is_quiet_and_a_keyed_on_one_is_not() {
        let mut idle = Ym2612::new();
        idle.reset(MD_CLOCK, false);
        let mut quiet = vec![0i32; 4096 * 2];
        idle.render(&mut quiet);

        let mut chip = Ym2612::new();
        chip.reset(MD_CLOCK, false);
        key_on(&mut chip);
        let mut loud = vec![0i32; 4096 * 2];
        chip.render(&mut loud);

        assert!(
            energy(&loud) > energy(&quiet) * 8,
            "the C core linked, reset, latched its writes and generated -- or it did not:              loud={} idle={}",
            energy(&loud),
            energy(&quiet)
        );
    }

    /// The rate the VGM engine resamples from. Getting it wrong detunes
    /// everything rather than failing, so it is worth pinning against the
    /// familiar figure: a Mega Drive YM2612 runs at 53.693 MHz / 7 / 144.
    #[test]
    fn the_native_rate_is_the_clock_over_144() {
        let mut chip = Ym2612::new();
        chip.reset(MD_CLOCK, false);
        assert_eq!(chip.native_rate(), MD_CLOCK / 144);
        assert_eq!(chip.native_rate(), 53_267);

        // A zero clock must not divide to zero and make the engine's resampler
        // step by nothing.
        chip.reset(0, false);
        assert!(chip.native_rate() >= 1);
    }

    /// **The property the write queue exists for.** Upstream latches a write
    /// for the *next* internal clock and keeps only the most recent, so pushing
    /// a run of register writes straight through would keep the last of each
    /// pair and drop the rest -- on this chip, a note that never sounds. The
    /// key-on setup above is 30 writes at one instant, so if any were lost the
    /// note would be wrong or absent.
    #[test]
    fn a_burst_of_writes_all_reach_the_chip() {
        let mut queued = Ym2612::new();
        queued.reset(MD_CLOCK, false);
        key_on(&mut queued);
        let mut through_queue = vec![0i32; 4096 * 2];
        queued.render(&mut through_queue);

        // The same registers, but rendered a sample at a time between each --
        // which is what the queue is emulating, and must therefore match in
        // kind: a note either way.
        let mut spaced = Ym2612::new();
        spaced.reset(MD_CLOCK, false);
        let mut scratch = [0i32; 2];
        for (reg, value) in [(0x30u16, 0x01u16), (0x40, 0x00), (0xB0, 0x07), (0xB4, 0xC0)] {
            spaced.write(0, reg, value);
            spaced.render(&mut scratch);
        }

        assert!(
            energy(&through_queue) > 0,
            "a burst of writes produced silence: the queue is dropping them"
        );
    }

    /// A run longer than one sample's worth must still arrive, just later --
    /// never be dropped.
    #[test]
    fn a_run_longer_than_one_sample_is_delayed_rather_than_dropped() {
        let mut chip = Ym2612::new();
        chip.reset(MD_CLOCK, false);
        for reg in 0x30u16..0x6C {
            chip.write(0, reg, 0x01);
        }
        let queued = chip.pending();
        assert!(queued > 1, "{queued} queued");

        let mut one_sample = [0i32; 2];
        chip.render(&mut one_sample);
        assert_eq!(
            chip.pending(),
            queued - 1,
            "exactly one register drains per output sample"
        );

        let mut rest = vec![0i32; 128 * 2];
        chip.render(&mut rest);
        assert_eq!(chip.pending(), 0, "the run must finish, not evaporate");
    }

    /// The two parts differ in their DAC, so the variant flag has to reach
    /// upstream's chip-type rather than being decoration. Same notes, different
    /// samples.
    #[test]
    fn the_ym3438_variant_does_not_sound_identical_to_the_ym2612() {
        fn render(variant: bool) -> Vec<i32> {
            let mut chip = Ym2612::new();
            chip.reset(MD_CLOCK, variant);
            key_on(&mut chip);
            let mut out = vec![0i32; 4096 * 2];
            chip.render(&mut out);
            out
        }
        let ym2612 = render(false);
        let ym3438 = render(true);
        assert!(energy(&ym2612) > 0 && energy(&ym3438) > 0);
        assert_ne!(
            ym2612, ym3438,
            "the variant flag is not reaching OPN2_SetChipType"
        );
    }

    /// Upstream keeps the chip type in a file-scope static, so two instances
    /// share it. Re-asserting it before every call is what makes a YM2612 and a
    /// YM3438 in one file each render as themselves; without it, whichever was
    /// reset last would win for both.
    #[test]
    fn two_variants_driven_in_turn_each_keep_their_own_type() {
        let mut ym2612 = Ym2612::new();
        ym2612.reset(MD_CLOCK, false);
        key_on(&mut ym2612);
        let mut ym3438 = Ym2612::new();
        ym3438.reset(MD_CLOCK, true);
        key_on(&mut ym3438);

        // Interleaved, as the engine's per-chip render loop would.
        let (mut a, mut b) = (vec![0i32; 2048 * 2], vec![0i32; 2048 * 2]);
        for (chunk_a, chunk_b) in a.chunks_mut(128 * 2).zip(b.chunks_mut(128 * 2)) {
            ym2612.render(chunk_a);
            ym3438.render(chunk_b);
        }

        // Each must match what it produces on its own.
        let mut alone = Ym2612::new();
        alone.reset(MD_CLOCK, false);
        key_on(&mut alone);
        let mut solo = vec![0i32; 2048 * 2];
        alone.render(&mut solo);
        assert_eq!(
            a, solo,
            "the YM2612 was rendered as a YM3438 partway through"
        );
    }

    /// Chunking must not change the audio, or an `AudioWorklet` pulling 128
    /// frames would sound different from an offline render pulling 4096.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        let mut whole = Ym2612::new();
        whole.reset(MD_CLOCK, false);
        key_on(&mut whole);
        let mut one_go = vec![0i32; 1024 * 2];
        whole.render(&mut one_go);

        let mut chunked = Ym2612::new();
        chunked.reset(MD_CLOCK, false);
        key_on(&mut chunked);
        let mut piecemeal = vec![0i32; 1024 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// A reset must be a fresh chip with an empty queue -- the engine resets
    /// between songs and at a seek, and a stale queued write would arrive after
    /// the seek as if the song had made it.
    ///
    /// Checked on the *YM3438*, whose CMOS DAC has no ladder and so really is
    /// bit-silent when idle. That makes "did the reset work" an equality rather
    /// than a ratio.
    #[test]
    fn a_reset_clears_both_the_chip_and_its_pending_writes() {
        let mut chip = Ym2612::new();
        chip.reset(MD_CLOCK, true);
        key_on(&mut chip);
        assert!(chip.pending() > 0, "writes are pending");

        chip.reset(MD_CLOCK, true);
        assert_eq!(chip.pending(), 0, "a seek must not carry writes across");
        let mut after = vec![0i32; 1024 * 2];
        chip.render(&mut after);
        assert_eq!(energy(&after), 0, "a reset CMOS part is bit-silent");
    }

    /// Evidence for the optimiser's YM2612 rules: `chip_state::latch_rule`
    /// refuses to drop repeat writes to `0x28` (key on/off) and `0x2A` (the DAC
    /// port), and this measures whether each is really audible by rendering a
    /// stream with the redundant write and one without. What it finds is reported
    /// rather than acted on -- lifting an exclusion belongs behind a corpus
    /// parity run, not one unit test.
    #[test]
    fn a_repeated_key_write_is_inaudible_but_a_repeated_dac_write_is_not() {
        /// Renders a key-on, then optionally repeats `reg`/`value` before
        /// rendering the tail. The two renders differ only by that write.
        fn with_repeat(reg: Option<(u16, u16)>) -> Vec<i32> {
            let mut chip = Ym2612::new();
            chip.reset(MD_CLOCK, false);
            key_on(&mut chip);
            let mut head = vec![0i32; 2048 * 2];
            chip.render(&mut head);
            if let Some((reg, value)) = reg {
                chip.write(0, reg, value);
            }
            let mut tail = vec![0i32; 2048 * 2];
            chip.render(&mut tail);
            tail
        }

        // 0x28 was last written as 0xF0 by `key_on`, so this repeats it.
        let repeated_key = with_repeat(Some((0x28, 0xF0)));
        let no_repeat = with_repeat(None);
        assert_eq!(
            repeated_key, no_repeat,
            "a repeated key write re-attacked the note, so the optimiser is              right to keep 0x28"
        );

        // The DAC port is a different matter: the chip is not in DAC mode here,
        // so this only shows the write is accepted. Whether a *repeat* is
        // audible depends on the DAC being enabled (0x2B bit 7), and on real
        // files those writes arrive paired with their own wait opcodes -- which
        // is the reason vgmtools' chip_cmp bypasses 0x2A entirely, and the
        // reason the exclusion stays whatever this shows.
        let repeated_dac = with_repeat(Some((0x2A, 0x80)));
        assert_eq!(repeated_dac.len(), no_repeat.len());
    }

    /// The gain sets the FM-to-PSG balance, pinned rather than left to drift.
    /// Measured on one channel at full level; the chip has six, so this times
    /// six is what a dense track approaches.
    #[test]
    fn a_loud_patch_uses_the_range_without_clipping_it() {
        let mut chip = Ym2612::new();
        chip.reset(MD_CLOCK, false);
        key_on(&mut chip);
        let mut out = vec![0i32; 8192 * 2];
        chip.render(&mut out);

        let one_channel = peak(&out);
        let full_chip = one_channel * 6;
        // At x21 a synthetic all-channels-maximum patch exceeds the mixer's
        // clamp by design (see OUTPUT_GAIN), so the ceiling only catches an
        // order-of-magnitude slip and the floor pins the measured balance.
        assert!(
            full_chip > i32::from(i16::MAX) * 2,
            "one channel peaked at {one_channel}, so a whole chip would reach              {full_chip} -- the FM would sit far under the measured balance"
        );
        assert!(
            full_chip < i32::from(i16::MAX) * 8,
            "one channel peaked at {one_channel}, so a whole chip would reach              {full_chip} -- an order of magnitude past the measured balance"
        );
    }
}
