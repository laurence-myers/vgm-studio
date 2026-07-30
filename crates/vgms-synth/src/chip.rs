//! The sound chip a [`VgmEngine`](crate::vgm_engine::VgmEngine) drives, and the
//! registry that decides which chips it can drive at all.
//!
//! [`OplChip`](crate::opl::OplChip) is the OPL-only equivalent, and stays: the
//! OPL player has register policy of its own (muting, panning) that belongs
//! nowhere near a generic engine. This trait is deliberately smaller. A core
//! receives writes, ROM and RAM, and renders frames at whatever rate it likes;
//! everything else -- routing, banks, timing, mixing -- is the engine's.
//!
//! This crate registers no generic core of its own: provider crates --
//! `vgms-cores-libvgm` first among them -- fill the registry, and
//! [`RecordingChip`] is what proves the engine right without any of them.

use vgms_core::vgm::ChipKind;

/// One sound chip, rendering at its own natural rate.
///
/// Implementors must be deterministic: the same writes and the same requested
/// frame counts must produce the same samples however the caller chunks its
/// [`render`](Self::render) calls. The engine relies on that to keep an audio
/// worklet pulling 128 frames sounding identical to an offline render pulling
/// 4096.
pub trait ChipCore: Send {
    /// Discards all state and re-initialises for a chip clocked at `clock` Hz.
    ///
    /// `variant` carries the flags the VGM header packs alongside the clock
    /// (bit 31 for the chips that use it -- an AY8910 variant, a YM2610B, a
    /// dual-mode T6W28). A core that has no variants ignores it.
    fn reset(&mut self, clock: u32, variant: bool);

    /// Hands the core the per-chip configuration bytes from the file's header.
    ///
    /// Called once, immediately after [`reset`](Self::reset), which is why the
    /// default is to ignore it: most cores have nothing here to read.
    ///
    /// **This is not decoration.** The VGM header carries settings that change
    /// what the silicon *is*, not merely how loud it is -- the SN76489's noise
    /// feedback mask and shift-register width being the sharpest example, since
    /// a core using the wrong ones emits a completely different pseudo-random
    /// sequence and no amount of tuning brings it back.
    ///
    /// `ChipSettings` also carries the AY8910's type and flags, the SSG flags
    /// of the OPN family, and the OKIM6258's -- none of which reach a core yet.
    fn configure(&mut self, _settings: &vgms_core::vgm::ChipSettings) {}

    /// The rate this core renders at, in Hz. Usually derived from the clock it
    /// was reset with, so call it after [`reset`](Self::reset).
    fn native_rate(&self) -> u32;

    /// Writes `data` to register `addr` on `port`.
    ///
    /// Ports are the chip's own: a YM2612 has two, an OPL3 has two, an SN76489
    /// has one. `addr` is 16 bits because a few chips address more than 256
    /// registers per port.
    fn write(&mut self, port: u8, addr: u16, data: u16);

    /// Hands the core a ROM image, or the part of one it has been given so far.
    ///
    /// `total_size` is the full image's size, which arrives with the first
    /// piece; `start` is where `data` belongs in it. A core with no ROM ignores
    /// this. The default does.
    fn load_rom(&mut self, _block_type: u8, _total_size: u32, _start: u32, _data: &[u8]) {}

    /// Writes `data` into the chip's RAM at `offset`. The default ignores it.
    fn write_ram(&mut self, _offset: u32, _data: &[u8]) {}

    /// Writes `data` at an *absolute* RAM `address`, for the `0x68` PCM RAM
    /// write. The default treats it as [`write_ram`](Self::write_ram); a chip
    /// whose `write_ram` goes through a banked window (the RF5C68) overrides
    /// this to bypass it -- `0x68`'s 24-bit address field spans the whole
    /// RAM, and the corpus's rips fill all of it.
    fn write_ram_absolute(&mut self, address: u32, data: &[u8]) {
        self.write_ram(address, data);
    }

    /// Renders `out.len() / 2` interleaved stereo frames at
    /// [`native_rate`](Self::native_rate).
    ///
    /// Samples are `i32` so a core can render at full internal precision and
    /// leave headroom for the mixer; the engine scales and clips once, at the
    /// end, rather than once per chip.
    fn render(&mut self, out: &mut [i32]);

    /// Mutes the channels whose bits are set, in the canonical order of
    /// [`vgms_core::vgm::channels_of`] -- bit `i` is entry `i`. Zero unmutes
    /// everything.
    ///
    /// The default ignores it: a core that cannot mute plays everything.
    /// **A mask does not survive [`reset`](Self::reset)** -- the engine
    /// reapplies it, and a provider whose device restarts on reset must too.
    fn set_channel_mutes(&mut self, _muted: u32) {}

    /// Places each channel in the stereo image: entry `i` is channel `i`'s
    /// position, `-0x100 ..= 0x100` for hard left through hard right, `0`
    /// centre (see [`chip_mix`](crate::chip_mix)).
    ///
    /// Only meaningful when [`supports_pan`](Self::supports_pan) says so;
    /// the default ignores it. Like the mute mask, it does not survive
    /// [`reset`](Self::reset).
    fn set_channel_pans(&mut self, _pans: &[i16]) {}

    /// Whether [`set_channel_pans`](Self::set_channel_pans) reaches this
    /// core's mix. The UI hides pan controls when it does not, rather than
    /// drawing knobs that turn and do nothing.
    fn supports_pan(&self) -> bool {
        false
    }
}

/// Whether this app can play a file, and what it would be missing if not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playability {
    /// Every chip the file clocks has a core.
    Full,
    /// Some chips have cores and some do not. Playing it renders what it can
    /// and leaves the rest silent -- worth offering, but only with the missing
    /// chips named.
    Partial(Vec<ChipKind>),
    /// No chip in the file has a core, so playing it would render silence.
    None,
}

impl Playability {
    /// Whether playing this would produce any sound at all.
    #[must_use]
    pub const fn can_play(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The chips that would be silent, if any.
    #[must_use]
    pub fn missing(&self) -> &[ChipKind] {
        match self {
            Self::Partial(chips) => chips,
            _ => &[],
        }
    }
}

/// Builds a core for `kind`, or `None` when this build has none for it.
///
/// Delegates to the [registry](crate::registry), which is where cores are
/// actually declared, honouring the process-wide per-chip choice the app
/// installed with [`set_core_choices`](crate::registry::set_core_choices) --
/// so the WAV render, the waveform and the peak scan all build the core the
/// user picked in Settings, not merely the registry's default.
///
/// OPL returns `None` on purpose, and not by oversight: an OPL file plays
/// through `PlayerEngine`, which carries the muting and panning policy this
/// trait has no place for. The registry still *lists* OPL cores, so
/// [`playability`] and the Settings picker see them.
#[must_use]
pub fn core_for(kind: ChipKind) -> Option<Box<dyn ChipCore>> {
    let registry = crate::registry::registry();
    registry
        .resolve_choice(kind, crate::registry::core_choice(kind).as_deref())?
        .build()
}

/// As [`core_for`], but never a core that cannot keep up with playback.
///
/// What the *transport* builds from: a chosen offline-tier core (the LLE die
/// sims render slower than realtime by design) falls back to the chip's best
/// realtime core rather than underrunning the audio callback. Offline renders
/// keep [`core_for`], which honours the choice as made.
#[must_use]
pub fn core_for_realtime(kind: ChipKind) -> Option<Box<dyn ChipCore>> {
    let registry = crate::registry::registry();
    registry
        .resolve_choice_realtime(kind, crate::registry::core_choice(kind).as_deref())?
        .build()
}

/// What playing a file with these chips through [`VgmEngine`] would sound like.
///
/// Asks whether a core can be *built*, not merely whether one is listed, and
/// the difference is load-bearing: the registry lists OPL cores so the Settings
/// picker and the About credits can see them, but `VgmEngine` cannot drive one.
/// Every caller here has already routed its OPL documents to `PlayerEngine`, so
/// counting a listed-but-routed OPL core as playable would send an OPL file
/// that failed to decode into the generic engine and render silence.
///
/// [`VgmEngine`]: crate::vgm_engine::VgmEngine
#[must_use]
pub fn playability(chips: &[ChipKind]) -> Playability {
    let registry = crate::registry::registry();
    let missing: Vec<ChipKind> = chips
        .iter()
        .copied()
        .filter(|&kind| !registry.can_build(kind))
        .collect();
    match (missing.len(), chips.len()) {
        (0, _) => Playability::Full,
        (missing_count, total) if missing_count == total => Playability::None,
        _ => Playability::Partial(missing),
    }
}

/// A core that renders silence and remembers everything it was told.
///
/// The engine's test double. Routing, banks, ROM delivery and DAC streams are
/// all assertions about *what reached which chip*, which needs no emulation to
/// check -- and checking them against a real core would be checking the core.
#[derive(Debug, Default, Clone)]
pub struct RecordingChip {
    pub clock: u32,
    pub variant: bool,
    pub resets: usize,
    /// Every `(port, addr, data)` in the order it arrived.
    pub writes: Vec<(u8, u16, u16)>,
    /// Every `(block_type, total_size, start, len)` handed over.
    pub roms: Vec<(u8, u32, u32, usize)>,
    /// Every `(offset, len)` written to RAM.
    pub ram: Vec<(u32, usize)>,
    /// Every mute mask handed over, in order -- so a test can see not only
    /// the mask in force but that a reset was followed by a reapplication.
    pub mutes: Vec<u32>,
    /// Every pan array handed over, in order.
    pub pans: Vec<Vec<i16>>,
    /// What [`ChipCore::supports_pan`] reports, for tests exercising the
    /// pan-capable path.
    pub pan_capable: bool,
    /// Frames rendered so far.
    pub frames: usize,
    /// The rate to report. Zero means "derive from the clock", which is what a
    /// real core usually does.
    pub rate: u32,
}

impl RecordingChip {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A recorder that reports `rate` however it is clocked, for tests that
    /// care about resampling rather than about clocks.
    #[must_use]
    pub fn at_rate(rate: u32) -> Self {
        Self {
            rate,
            ..Self::default()
        }
    }
}

impl ChipCore for RecordingChip {
    fn reset(&mut self, clock: u32, variant: bool) {
        self.clock = clock;
        self.variant = variant;
        self.resets += 1;
    }

    fn native_rate(&self) -> u32 {
        if self.rate > 0 {
            self.rate
        } else {
            // A plausible stand-in: most cores divide their clock down by some
            // fixed factor, and the engine only cares that it is non-zero.
            (self.clock / 72).max(1)
        }
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        self.writes.push((port, addr, data));
    }

    fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        self.roms.push((block_type, total_size, start, data.len()));
    }

    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        self.ram.push((offset, data.len()));
    }

    fn render(&mut self, out: &mut [i32]) {
        self.frames += out.len() / 2;
        out.fill(0);
    }

    fn set_channel_mutes(&mut self, muted: u32) {
        self.mutes.push(muted);
    }

    fn set_channel_pans(&mut self, pans: &[i16]) {
        self.pans.push(pans.to_vec());
    }

    fn supports_pan(&self) -> bool {
        self.pan_capable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playability_is_per_chip_and_says_what_is_missing() {
        // The stub registry: this crate ships no generic cores of its own, so
        // the buildable half of the question is a test double and the logic
        // under test is unchanged.
        crate::testing::install_registry_with_stub();
        assert!(core_for(ChipKind::Sn76489).is_some());
        assert!(core_for(ChipKind::Ym2612).is_none());

        assert_eq!(playability(&[ChipKind::Sn76489]), Playability::Full);
        assert_eq!(playability(&[]), Playability::Full);
        assert_eq!(playability(&[ChipKind::Ym2612]), Playability::None);
        // A Mega Drive rip: one chip playable, one not, so it is worth offering
        // -- with the silent one named.
        let mixed = playability(&[ChipKind::Sn76489, ChipKind::Ym2612]);
        assert_eq!(mixed, Playability::Partial(vec![ChipKind::Ym2612]));
        assert!(mixed.can_play());
        assert_eq!(mixed.missing(), [ChipKind::Ym2612]);
    }

    #[test]
    fn a_recorder_reports_what_it_was_told() {
        let mut chip = RecordingChip::new();
        chip.reset(3_579_545, true);
        chip.write(1, 0x28, 0xF0);
        chip.load_rom(0x80, 1024, 0, &[0u8; 16]);
        chip.write_ram(4, &[0u8; 8]);
        let mut out = [1i32; 8];
        chip.render(&mut out);

        assert_eq!(
            (chip.clock, chip.variant, chip.resets),
            (3_579_545, true, 1)
        );
        assert_eq!(chip.writes, [(1, 0x28, 0xF0)]);
        assert_eq!(chip.roms, [(0x80, 1024, 0, 16)]);
        assert_eq!(chip.ram, [(4, 8)]);
        assert_eq!(chip.frames, 4);
        assert_eq!(out, [0; 8], "a recorder renders silence");
    }
}
