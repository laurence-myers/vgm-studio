//! The OPN family: YM2203 (OPN), YM2608 (OPNA) and YM2610 (OPNB).
//!
//! 13,877 files in the VGMRips corpus between them -- 19.1%, the biggest block
//! still silent before this. They are what PC-88/98 games, the Neo Geo and a
//! great deal of arcade hardware sound like.
//!
//! # Assembled rather than written
//!
//! All three are an FM synthesiser bolted to an SSG, and both halves already
//! exist here:
//!
//! - **The FM is OPN2's.** The YM2612 *is* an OPN, and the family shares one
//!   four-operator engine, one envelope generator and one register map. What
//!   sets the YM2612 apart is its 9-bit ladder DAC and its channel-6 PCM mode;
//!   selecting upstream's CMOS (YM3438) mode turns the ladder off, and that is
//!   what the rest of the family's clean DAC sounds like. So Nuked-OPN2 drives
//!   the FM, in CMOS mode, always.
//! - **The SSG is an AY-3-8910**, which `dro-synth` already has clean-room.
//!
//! **What is not modelled: the ADPCM.** The YM2608 and YM2610 each carry an
//! ADPCM-A rhythm section and an ADPCM-B sample channel, and neither is here.
//! On a Neo Geo rip that means the drums are missing while the FM and SSG play
//! -- a real gap, stated rather than hidden, and the reason this is registered
//! as an approximation. `Playability::Partial` already exists to say so for a
//! whole chip; there is no vocabulary yet for "most of one".
//!
//! Two other simplifications, both recorded because they affect pitch rather
//! than presence: the YM2203's programmable prescaler (`$2D`-`$2F`) is assumed
//! to be its default, and the SSG clock is taken as a quarter of the chip
//! clock for all three.
//!
//! Why not a port of MAME's fmopn, as the plan's fallback said? Because
//! `nukeykt/Nuked-OPNB` is not usable -- version 0.0, a header that declares
//! two of its fields twice so it does not compile, no reset, no output function
//! at all -- and a port of thousands of lines of C++ buys, for the FM half,
//! what an already-shipped and byte-tested core gives for nothing. The ADPCM is
//! where a port would actually earn its keep, and that is where the gap is.

use dro_core::vgm::ChipKind;
use dro_synth::{Ay8910, ChipCore};

use crate::ffi::Opn2Chip;
use crate::write_queue::WriteQueue;

/// The registry ids, one per chip.
pub(crate) const YM2203_ID: &str = "ym2203.nuked";
pub(crate) const YM2608_ID: &str = "ym2608.nuked";
pub(crate) const YM2610_ID: &str = "ym2610.nuked";

/// Internal FM cycles per output sample, as for the YM2612.
const CLOCKS_PER_SAMPLE: u32 = 24;
/// Master clocks per internal cycle.
const MASTER_PER_CLOCK: u32 = 6;

/// The OPN2 pacing, measured for that core: address, value next cycle, then the
/// rest of the rotation. See [`WriteQueue`].
const ADDRESS_SETTLE: u32 = 0;
/// The address goes out on one cycle and its value on the next, so the settle
/// that follows is the rotation less those two -- and less one more, because
/// the queue reaches `Idle` on the cycle *after* the count runs out.
const VALUE_SETTLE: u32 = CLOCKS_PER_SAMPLE - 3;

/// What the chip clock is divided by to reach the SSG.
///
/// A simplification: the real divider follows the FM prescaler, which the
/// YM2203 can reprogram. Assumed at its default here, which is what nearly
/// every rip uses.
const SSG_DIVIDER: u32 = 4;

/// Brings the FM half up to the scale the SSG half already uses.
///
/// The YM2612 core's own gain is documented in `opn2.rs`; this is the same
/// figure for the same reason, and it is what balances FM against SSG. A
/// balance is a listening question -- see the A/B note in `PROVENANCE.md`.
const FM_GAIN: i32 = 5;

/// Which of the family this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpnKind {
    /// Three FM channels, one port, no stereo.
    Ym2203,
    /// Six FM channels over two ports, stereo, ADPCM-A + ADPCM-B (absent here).
    Ym2608,
    /// Four FM channels over two ports, stereo, ADPCM-A + ADPCM-B (absent here).
    Ym2610,
}

impl OpnKind {
    /// Master clocks per output sample.
    ///
    /// The YM2203 runs three FM channels through half the rotation, so it
    /// samples twice as often as its six-channel relatives for the same clock.
    const fn master_per_sample(self) -> u32 {
        match self {
            Self::Ym2203 => MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE / 2,
            Self::Ym2608 | Self::Ym2610 => MASTER_PER_CLOCK * CLOCKS_PER_SAMPLE,
        }
    }

    /// Whether a second register bank exists. The YM2203 has one port.
    const fn has_second_bank(self) -> bool {
        !matches!(self, Self::Ym2203)
    }

    /// Whether the chip pans. The YM2203's FM output is mono, so its rips never
    /// write `$B4` and the FM core has to be told both speakers are on or it
    /// renders silence -- see [`OpnCore::open_the_speakers`].
    const fn is_mono(self) -> bool {
        matches!(self, Self::Ym2203)
    }
}

/// One of the OPN family: OPN2's FM, an AY's SSG, and no ADPCM.
#[derive(Debug)]
pub struct OpnCore {
    kind: OpnKind,
    fm: Opn2Chip,
    ssg: Ay8910,
    writes: WriteQueue,
    rate: u32,
    /// SSG ticks owed per output sample, in 16.16 fixed point -- the two halves
    /// run at unrelated rates and only the FM's is declared.
    ssg_step: u64,
    ssg_owed: u64,
}

impl OpnCore {
    /// A chip of this kind, with no clock yet.
    #[must_use]
    pub fn new(kind: OpnKind) -> Self {
        Self {
            kind,
            fm: Opn2Chip::new(),
            ssg: Ay8910::new(),
            writes: WriteQueue::new(ADDRESS_SETTLE, VALUE_SETTLE),
            rate: 44_100,
            ssg_step: 0,
            ssg_owed: 0,
        }
    }

    /// Turns both speakers on for every FM channel.
    ///
    /// The FM core comes up with its panning bits clear, because a YM2612 rip
    /// always writes `$B4` before it plays. A YM2203 has no such register --
    /// its FM output is mono -- so without this it would render silence no
    /// matter what the song did.
    fn open_the_speakers(&mut self) {
        if !self.kind.is_mono() {
            return;
        }
        for channel in 0..3u8 {
            self.writes.push(0, 0xB4 + channel, 0xC0);
        }
    }
}

impl ChipCore for OpnCore {
    /// `variant` is the YM2610B's extra two FM channels, which need no
    /// different handling here: the FM core has six either way and a 2610 rip
    /// simply never addresses the last two.
    fn reset(&mut self, clock: u32, _variant: bool) {
        self.writes.clear();
        self.rate = (clock / self.kind.master_per_sample()).max(1);
        // CMOS mode, always: the ladder DAC is the YM2612's alone, and the rest
        // of the family has a clean one.
        self.fm.reset(false);
        let ssg_clock = (clock / SSG_DIVIDER).max(1);
        self.ssg.reset(ssg_clock, false);
        // How many SSG ticks fall in one FM sample, carried in fixed point so
        // the two rates need no common factor.
        self.ssg_step =
            (u64::from(Ay8910::tick_rate(ssg_clock).max(1)) << 16) / u64::from(self.rate);
        self.ssg_owed = 0;
        self.open_the_speakers();
    }

    fn native_rate(&self) -> u32 {
        self.rate
    }

    /// The family's register map: the SSG at `$00`-`$0F` on the first port,
    /// FM above it, and a second bank on port 1 where the chip has one.
    ///
    /// The ADPCM registers -- `$10`-`$1F` on either port, depending on the chip
    /// -- are accepted and dropped. They are the documented gap.
    fn write(&mut self, port: u8, addr: u16, data: u16) {
        let register = (addr & 0xFF) as u8;
        let value = (data & 0xFF) as u8;
        let second = port & 1 == 1;

        if second && !self.kind.has_second_bank() {
            return;
        }
        // The SSG lives at the bottom of the *first* port only.
        if !second && register < 0x10 {
            self.ssg.write_register(register, value);
            return;
        }
        // ADPCM: `$10`-`$1F` on port 0 is the rhythm section, and `$00`-`$1F`
        // on port 1 is ADPCM-B. Neither is modelled, and passing them to the FM
        // core would write real FM registers.
        //
        // The range stops at `$20`, not `$30`: `$20`-`$2F` on port 0 is the FM
        // *mode* block -- the LFO, the timers, and `$28`, which is key-on.
        // Dropping those silences the FM completely while every other register
        // still arrives, which reads as a dead core rather than a routing bug.
        if (!second && (0x10..0x20).contains(&register)) || (second && register < 0x20) {
            return;
        }
        self.writes.push(u32::from(port & 1) * 2, register, value);
    }

    fn render(&mut self, out: &mut [i32]) {
        let mut clocking = self.fm.clocking();
        for frame in out.chunks_exact_mut(2) {
            let mut left = 0i32;
            let mut right = 0i32;
            for _ in 0..CLOCKS_PER_SAMPLE {
                self.writes.advance(|port, byte| clocking.write(port, byte));
                let (l, r) = clocking.clock();
                left += l;
                right += r;
            }
            left *= FM_GAIN;
            right *= FM_GAIN;

            // The SSG runs on its own clock, so it is advanced by however many
            // of its ticks fall inside this FM sample.
            self.ssg_owed += self.ssg_step;
            while self.ssg_owed >= 1 << 16 {
                self.ssg_owed -= 1 << 16;
                self.ssg.tick();
            }
            let ssg = self.ssg.output();

            // The SSG is mono and sums into both sides, as it does on the chip.
            frame[0] = left + ssg;
            frame[1] = right + ssg;
        }
    }
}

/// Registers all three of the family.
///
/// One entry each rather than one shared: a maker is a plain function pointer,
/// so the kind has to be baked into the function rather than captured.
pub(crate) fn register(registry: &mut dro_synth::CoreRegistry) {
    fn ym2203() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2203))
    }
    fn ym2608() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2608))
    }
    fn ym2610() -> Box<dyn ChipCore> {
        Box::new(OpnCore::new(OpnKind::Ym2610))
    }

    for (id, chip, make) in [
        (
            YM2203_ID,
            ChipKind::Ym2203,
            ym2203 as fn() -> Box<dyn ChipCore>,
        ),
        (YM2608_ID, ChipKind::Ym2608, ym2608),
        (YM2610_ID, ChipKind::Ym2610, ym2610),
    ] {
        registry.register(dro_synth::CoreInfo {
            id,
            chip,
            // The label states the gap, because the Settings picker is where a
            // user would otherwise wonder why their Neo Geo rip has no drums.
            label: "Nuked-OPN2 FM + SSG (no ADPCM)",
            authors: "Nuke.YKT (FM); this project (SSG, assembly)",
            license: "LGPL-2.1-or-later",
            upstream: "https://github.com/nukeykt/Nuked-OPN2",
            realtime: true,
            make: dro_synth::CoreMaker::Generic(make),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PC-88's YM2203 clock, and a Neo Geo's YM2610.
    const YM2203_CLOCK: u32 = 3_993_600;
    const YM2610_CLOCK: u32 = 8_000_000;

    fn render(chip: &mut OpnCore, frames: usize) -> Vec<[i32; 2]> {
        let mut out = vec![0i32; frames * 2];
        chip.render(&mut out);
        out.chunks_exact(2).map(|f| [f[0], f[1]]).collect()
    }

    fn energy(frames: &[[i32; 2]]) -> i64 {
        frames
            .iter()
            .map(|f| i64::from(f[0].abs()) + i64::from(f[1].abs()))
            .sum()
    }

    /// A loud FM note on channel 1 of `port`, algorithm 7.
    fn fm_key_on(chip: &mut OpnCore, port: u8) {
        for (register, value) in [
            (0x30u16, 0x01u16),
            (0x34, 0x01),
            (0x38, 0x01),
            (0x3C, 0x01),
            (0x40, 0x00),
            (0x44, 0x00),
            (0x48, 0x00),
            (0x4C, 0x00),
            (0x50, 0x1F),
            (0x54, 0x1F),
            (0x58, 0x1F),
            (0x5C, 0x1F),
            (0x60, 0x00),
            (0x64, 0x00),
            (0x68, 0x00),
            (0x6C, 0x00),
            (0x80, 0x00),
            (0x84, 0x00),
            (0x88, 0x00),
            (0x8C, 0x00),
            (0xB0, 0x07),
            (0xB4, 0xC0),
            (0xA4, 0x22),
            (0xA0, 0x69),
        ] {
            chip.write(port, register, value);
        }
        // Key-on is always on port 0, whichever bank the channel is in.
        chip.write(0, 0x28, if port == 0 { 0xF0 } else { 0xF4 });
    }

    /// A loud SSG tone on channel A.
    fn ssg_key_on(chip: &mut OpnCore) {
        chip.write(0, 0x00, 0x40); // period low
        chip.write(0, 0x01, 0x00);
        chip.write(0, 0x07, 0x3E); // tone A on, the rest off
        chip.write(0, 0x08, 0x0F); // full volume
    }

    #[test]
    fn a_fresh_chip_is_silent() {
        for kind in [OpnKind::Ym2203, OpnKind::Ym2608, OpnKind::Ym2610] {
            let mut chip = OpnCore::new(kind);
            chip.reset(YM2610_CLOCK, false);
            assert_eq!(energy(&render(&mut chip, 2000)), 0, "{kind:?}");
        }
    }

    /// **Both halves must sound, and separately.** This core is an assembly of
    /// two, so the failure worth catching is one of them silently missing --
    /// which on a real rip reads as a thin mix rather than as a broken core.
    #[test]
    fn the_fm_and_the_ssg_each_make_a_sound_on_their_own() {
        let mut fm_only = OpnCore::new(OpnKind::Ym2610);
        fm_only.reset(YM2610_CLOCK, false);
        fm_key_on(&mut fm_only, 0);
        let fm = energy(&render(&mut fm_only, 8000));
        assert!(fm > 0, "the FM half is silent");

        let mut ssg_only = OpnCore::new(OpnKind::Ym2610);
        ssg_only.reset(YM2610_CLOCK, false);
        ssg_key_on(&mut ssg_only);
        let ssg = energy(&render(&mut ssg_only, 8000));
        assert!(ssg > 0, "the SSG half is silent");

        // And together they are louder than either alone.
        let mut both = OpnCore::new(OpnKind::Ym2610);
        both.reset(YM2610_CLOCK, false);
        fm_key_on(&mut both, 0);
        ssg_key_on(&mut both);
        let together = energy(&render(&mut both, 8000));
        assert!(
            together > fm.max(ssg),
            "{together} vs FM {fm} and SSG {ssg}"
        );
    }

    /// **The YM2203 is mono and has no panning register**, so the FM core has to
    /// be told its speakers are on. Without that it renders perfect silence --
    /// and every PC-88 rip in the corpus is a YM2203.
    #[test]
    fn the_mono_chip_still_makes_fm_sound() {
        let mut chip = OpnCore::new(OpnKind::Ym2203);
        chip.reset(YM2203_CLOCK, false);
        // Deliberately *without* the `$B4` write a YM2612 rip would make.
        for (register, value) in [
            (0x30u16, 0x01u16),
            (0x40, 0x00),
            (0x50, 0x1F),
            (0x60, 0x00),
            (0x80, 0x00),
            (0x34, 0x01),
            (0x44, 0x00),
            (0x54, 0x1F),
            (0x64, 0x00),
            (0x84, 0x00),
            (0x38, 0x01),
            (0x48, 0x00),
            (0x58, 0x1F),
            (0x68, 0x00),
            (0x88, 0x00),
            (0x3C, 0x01),
            (0x4C, 0x00),
            (0x5C, 0x1F),
            (0x6C, 0x00),
            (0x8C, 0x00),
            (0xB0, 0x07),
            (0xA4, 0x22),
            (0xA0, 0x69),
        ] {
            chip.write(0, register, value);
        }
        chip.write(0, 0x28, 0xF0);
        assert!(
            energy(&render(&mut chip, 8000)) > 0,
            "a YM2203 rip writes no panning register, so the core must open the \\
             speakers itself"
        );
    }

    /// The second register bank is real on the OPNA and OPNB and absent on the
    /// OPN. Writing to it on a YM2203 must do nothing rather than land on the
    /// first bank.
    #[test]
    fn only_the_six_channel_parts_have_a_second_bank() {
        let mut ym2610 = OpnCore::new(OpnKind::Ym2610);
        ym2610.reset(YM2610_CLOCK, false);
        fm_key_on(&mut ym2610, 1);
        assert!(
            energy(&render(&mut ym2610, 8000)) > 0,
            "channel 4 lives in the second bank"
        );

        let mut ym2203 = OpnCore::new(OpnKind::Ym2203);
        ym2203.reset(YM2203_CLOCK, false);
        fm_key_on(&mut ym2203, 1);
        assert_eq!(
            energy(&render(&mut ym2203, 8000)),
            0,
            "a YM2203 has one port; the second must be dropped, not folded back"
        );
    }

    /// **The documented gap.** ADPCM registers are accepted and dropped rather
    /// than passed to the FM core, where they would write real FM registers and
    /// corrupt a voice. Silence from the drums is a missing feature; a mangled
    /// FM patch would be a bug.
    #[test]
    fn adpcm_registers_are_dropped_rather_than_misrouted() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        fm_key_on(&mut chip, 0);
        let clean = render(&mut chip, 4000);

        let mut disturbed = OpnCore::new(OpnKind::Ym2610);
        disturbed.reset(YM2610_CLOCK, false);
        fm_key_on(&mut disturbed, 0);
        // A rhythm section a Neo Geo rip would really write.
        for register in 0x10..0x1Cu16 {
            disturbed.write(0, register, 0xFF);
        }
        // And an ADPCM-B run on the second port.
        for register in 0x00..0x1Cu16 {
            disturbed.write(1, register, 0xFF);
        }
        assert_eq!(
            clean,
            render(&mut disturbed, 4000),
            "ADPCM writes reached the FM core"
        );
    }

    /// The two halves run on unrelated clocks, so the SSG is advanced by a
    /// fixed-point count of its own ticks per FM sample. A zero step would
    /// freeze it -- silently, since the FM would still play.
    #[test]
    fn the_ssg_advances_on_its_own_clock() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        assert!(chip.ssg_step > 0, "the SSG would never tick");
        // Its rate is well above the FM's, so more than one tick per sample.
        assert!(
            chip.ssg_step > 1 << 16,
            "the SSG runs faster than the FM samples: step {}",
            chip.ssg_step
        );

        let mut slow = OpnCore::new(OpnKind::Ym2203);
        slow.reset(YM2203_CLOCK, false);
        assert!(slow.ssg_step > 0);
    }

    /// Chunking must not change the audio -- and this core has two clocks to
    /// keep in step, so it is more at risk than most.
    #[test]
    fn the_output_does_not_depend_on_how_the_caller_chunks_it() {
        fn set_up(chip: &mut OpnCore) {
            chip.reset(YM2610_CLOCK, false);
            fm_key_on(chip, 0);
            ssg_key_on(chip);
        }
        let mut whole = OpnCore::new(OpnKind::Ym2610);
        set_up(&mut whole);
        let mut one_go = vec![0i32; 2048 * 2];
        whole.render(&mut one_go);

        let mut chunked = OpnCore::new(OpnKind::Ym2610);
        set_up(&mut chunked);
        let mut piecemeal = vec![0i32; 2048 * 2];
        for chunk in piecemeal.chunks_mut(64 * 2) {
            chunked.render(chunk);
        }
        assert_eq!(one_go, piecemeal);
    }

    /// A reset must clear the pending writes as well as both chips: a seek must
    /// not deliver registers the song wrote before it.
    #[test]
    fn a_reset_clears_everything_including_the_queue() {
        let mut chip = OpnCore::new(OpnKind::Ym2610);
        chip.reset(YM2610_CLOCK, false);
        fm_key_on(&mut chip, 0);
        assert!(chip.writes.pending() > 0);

        chip.reset(YM2610_CLOCK, false);
        assert_eq!(chip.writes.pending(), 0);
        assert_eq!(energy(&render(&mut chip, 2000)), 0);
    }

    /// The rate the engine resamples from. The YM2203 runs three channels
    /// through half the rotation, so it samples twice as often for its clock.
    #[test]
    fn the_native_rate_follows_the_channel_count() {
        let mut ym2610 = OpnCore::new(OpnKind::Ym2610);
        ym2610.reset(YM2610_CLOCK, false);
        assert_eq!(ym2610.native_rate(), YM2610_CLOCK / 144);

        let mut ym2203 = OpnCore::new(OpnKind::Ym2203);
        ym2203.reset(YM2203_CLOCK, false);
        assert_eq!(ym2203.native_rate(), YM2203_CLOCK / 72);

        ym2203.reset(0, false);
        assert!(
            ym2203.native_rate() >= 1,
            "a zero clock must not divide to zero"
        );
    }
}
