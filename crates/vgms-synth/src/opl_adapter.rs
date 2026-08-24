//! Hosting an OPL chip inside [`VgmEngine`](crate::vgm_engine::VgmEngine): the
//! [`OplChip`] → [`ChipCore`] adapter (Stage K / ou-1).
//!
//! `VgmEngine` pulls samples from a [`ChipCore`], but an OPL chip is an
//! [`OplChip`] with its own muting, panning and buffered-write policy, so an OPL
//! VGM through `VgmEngine` once rendered silence — there was no OPL `ChipCore` to
//! host. This adapter closes that gap: it presents an `OplChip` as a `ChipCore`,
//! so the one engine plays the OPL family like any other chip. With the separate
//! OPL engine retired, this is the OPL family's only playback path.
//!
//! ## Rate, and a non-standard clock
//!
//! An OPL chip resamples internally to whatever rate it is reset at, and every
//! [`OplChip`] assumes the standard crystal. So the adapter always runs the chip
//! at [`NATIVE_SAMPLE_RATE`] (the standard clock's 49716 Hz, where the chip's
//! internal resampler is an identity pass) and honours a file's *non-standard*
//! clock by reporting the projected rate — `clock / 72` for the OPL2 generation,
//! `clock / 288` for the YMF262 — as its [`native_rate`](ChipCore::native_rate).
//! The engine's own [`Voice`](crate::vgm_engine) resampler then repitches the
//! whole render by `clock / standard`: exactly what a different crystal does to
//! real silicon, envelopes and vibrato included (the corpus's 4 MHz and 3 MHz
//! YM3526 arcade boards are the population this serves). At the standard clock
//! the projected rate *is* the native rate, that resampler's identity bypass
//! engages, and there is never a double resample; and no output rate need reach
//! core construction, which is why [`CoreInfo::build`](crate::CoreInfo) needs no
//! rate parameter.
//!
//! The [hardware host](crate::registry::opl_hardware_core) uses the un-projected
//! [`OplCoreAdapter::new`] instead: a real chip runs at its real crystal, and
//! repitching a board is not something a register stream can do.
//!
//! ## Writes
//!
//! A live write takes the [buffered path](OplChip::write_reg_buffered) — the
//! spacing Nuked's write buffer needs — so a back-to-back key
//! off/on with no samples between still retriggers the note, matching real
//! hardware. A seek's bulk replay takes the immediate path
//! ([`replay_write`](ChipCore::replay_write)) instead: only the final register
//! *values* matter there, so applying the burst at once avoids it trickling out
//! through Nuked's spaced write buffer over the samples after the seek.
//!
//! ## Muting and panning
//!
//! Per-channel muting is the [`ChannelGate`](crate::channel_gate)'s job: the OPL
//! rows are already built, and [`CoreInfo::build`](crate::CoreInfo) wraps a
//! gate-covered, non-native-mute core in a `GatedCore` that filters the writes.
//! So the adapter itself keeps the trait's no-op mute.
//!
//! Per-channel panning is the OPL stereo-ext register policy, implemented here
//! behind [`set_channel_pans`](ChipCore::set_channel_pans):
//! engaging it forces `0x105`'s stereo-ext bit on (shadowing the song's `newm`
//! bit), writes the panpots (`0x0D0`-`0x0D8` low bank, `0x1D0`-`0x1D8` high), and
//! thereafter suppresses the song's own `0xD0`-`0xD8` writes so they cannot
//! clobber the applied pan. Un-panned, the song's own `0xC0`-`0xC8` stereo bits
//! pass straight through. Disengaging mid-playback (the OPL panel's
//! Original-vs-Custom, or Reset) is [`clear_channel_pans`](ChipCore::clear_channel_pans):
//! it drops the `0x105` enable bit, which makes the stereo-ext panpots inert at
//! once and reverts the chip to the song's own `0xC0` stereo image with no
//! reset. The engine restates panning every mix pass, so an un-panned chip is
//! actively disengaged rather than left latched at its last custom image.

use vgms_core::vgm::ChipKind;

use crate::NATIVE_SAMPLE_RATE;
use crate::chip::ChipCore;
use crate::opl::OplChip;

/// An [`OplChip`] presented as a [`ChipCore`], so [`VgmEngine`](crate::vgm_engine)
/// can drive the OPL family.
pub struct OplCoreAdapter {
    opl: Box<dyn OplChip>,
    /// The clock divider projecting a header clock onto this chip's sample rate
    /// (72 for the OPL2 generation, 288 for the YMF262), or `None` to pin the
    /// native rate whatever the clock -- the hardware host's choice, since a
    /// real chip's crystal is not the file's to change.
    divider: Option<u32>,
    /// What [`native_rate`](ChipCore::native_rate) reports:
    /// [`NATIVE_SAMPLE_RATE`] until a [`reset`](ChipCore::reset) carries a
    /// projectable clock.
    native: u32,
    /// A reused `i16` staging buffer: the OPL chip renders `i16`, the engine
    /// wants `i32`.
    scratch: Vec<i16>,
    /// Custom per-channel panning is engaged (the stereo-ext panpots). While it
    /// is, the song's own `0xD0`-`0xD8` writes are suppressed and the enable bit
    /// forced on -- this adapter's OPL stereo-ext register policy.
    panned: bool,
    /// The song's OPL3 `newm` bit (`0x105` bit 0), shadowed so forcing the
    /// stereo-ext enable (bit 1) never disturbs OPL3 mode.
    newm: u8,
}

impl OplCoreAdapter {
    /// Wraps `opl`, which must already be constructed at [`NATIVE_SAMPLE_RATE`],
    /// with the native rate pinned: the header clock is ignored. The hardware
    /// host's constructor; the emulated path wants [`projected`](Self::projected).
    #[must_use]
    pub fn new(opl: Box<dyn OplChip>) -> Self {
        Self {
            opl,
            divider: None,
            native: NATIVE_SAMPLE_RATE,
            scratch: Vec::new(),
            panned: false,
            newm: 0,
        }
    }

    /// Wraps `opl` as `chip`, honouring a reset's header clock through the
    /// reported rate (the module's *Rate* section): a 4 MHz YM3526 rip plays
    /// sharp of a 3.58 MHz one, as its board did.
    #[must_use]
    pub fn projected(opl: Box<dyn OplChip>, chip: ChipKind) -> Self {
        // One sample every 72 clocks on the OPL2 generation; the YMF262's
        // standard crystal is 4x theirs, one sample every 288 -- both land on
        // 49716 Hz.
        let divider = if chip == ChipKind::Ymf262 { 288 } else { 72 };
        Self {
            divider: Some(divider),
            ..Self::new(opl)
        }
    }

    /// One register write, buffered (live) or immediate (a seek replay), with the
    /// stereo-ext register policy applied.
    fn routed_write(&mut self, port: u8, addr: u16, data: u16, immediate: bool) {
        let reg = reg_of(port, addr);
        // The engine owns the stereo-ext enable (`0x105` bit 1): force it to match
        // whether custom panning is engaged, and shadow the song's `newm` bit
        // (bit 0) so toggling the enable never flips OPL3 mode. On the replay path
        // too, so a seek never leaves the chip's mode diverged.
        if reg == STEREO_EXT_REG {
            self.newm = (data as u8) & 0x01;
            let value = self.newm | if self.panned { STEREO_EXT_ENABLE } else { 0 };
            self.emit(reg, value, immediate);
            return;
        }
        // While panned, the chip repurposes `0xD0`-`0xD8` as the panpots, so a
        // song's own writes there (no-ops on a real OPL3 when disengaged) would
        // clobber the applied pan -- drop them.
        if self.panned && is_panpot(reg) {
            return;
        }
        self.emit(reg, data as u8, immediate);
    }

    fn emit(&mut self, reg: u16, value: u8, immediate: bool) {
        if immediate {
            self.opl.write_reg(reg, value);
        } else {
            self.opl.write_reg_buffered(reg, value);
        }
    }
}

/// OPL3 register `0x105` (high-bank `0x05`): bit 0 is `newm` (OPL3 mode), bit 1
/// the stereo-ext panpot enable this adapter owns.
const STEREO_EXT_REG: u16 = 0x105;
const STEREO_EXT_ENABLE: u8 = 0x02;
/// The five rhythm voices at the tail of every OPL roster; they pan with melodic
/// channels 6-8, so only the melodic channels take a panpot.
const RHYTHM_VOICES: usize = 5;

/// The OPL register a `(port, addr)` write addresses: OPL3's two register banks
/// are the write's port -- port 0 the low bank, port 1 the high bank (bit 8 of
/// the chip's 9-bit register address).
fn reg_of(port: u8, addr: u16) -> u16 {
    (u16::from(port) << 8) | addr
}

/// Whether `reg` is a stereo-ext panpot (`0x0D0`-`0x0D8` low, `0x1D0`-`0x1D8`
/// high).
fn is_panpot(reg: u16) -> bool {
    (0x0D0..=0x0D8).contains(&reg) || (0x1D0..=0x1D8).contains(&reg)
}

/// The stereo-ext panpot for a [`chip_mix`](crate::chip_mix) pan: `-0x100` hard
/// left through `0` centre (`0x80`) to `0x100` hard right (`0xFF`).
fn to_panpot(pan: i16) -> u8 {
    (0x80 + i32::from(pan) / 2).clamp(0, 0xFF) as u8
}

impl core::fmt::Debug for OplCoreAdapter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OplCoreAdapter")
            .field("opl", &self.opl)
            .finish_non_exhaustive()
    }
}

impl ChipCore for OplCoreAdapter {
    fn reset(&mut self, clock: u32, _variant: bool) {
        // The chip itself always runs at the standard-clock native rate (every
        // OplChip assumes the standard crystal); a non-standard header clock is
        // honoured by *reporting* the projected rate instead, so the engine's
        // Voice resampler repitches the whole render by clock/standard -- the
        // module's Rate section. Rounded, so the standard clocks land exactly
        // on NATIVE_SAMPLE_RATE and the identity bypass still engages.
        self.native = match self.divider {
            Some(divider) if clock != 0 => (clock + divider / 2) / divider,
            _ => NATIVE_SAMPLE_RATE,
        };
        self.opl.reset(NATIVE_SAMPLE_RATE);
        // A fresh chip is un-panned; the engine restates the pans afterward (its
        // `apply_mix`), so a loop's rewind keeps custom panning.
        self.panned = false;
        self.newm = 0;
    }

    fn native_rate(&self) -> u32 {
        self.native
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        self.routed_write(port, addr, data, false);
    }

    fn replay_write(&mut self, port: u8, addr: u16, data: u16) {
        // A seek's replay wants the immediate path: only the final register
        // values matter, and a burst through the spaced buffer would trickle out
        // over the samples after the seek (a stuck/late note).
        self.routed_write(port, addr, data, true);
    }

    fn render(&mut self, out: &mut [i32]) {
        self.scratch.resize(out.len(), 0);
        self.opl.generate_samples(&mut self.scratch);
        for (slot, &sample) in out.iter_mut().zip(&self.scratch) {
            *slot = i32::from(sample);
        }
    }

    fn set_channel_pans(&mut self, pans: &[i16]) {
        self.panned = true;
        // Enable stereo-ext first -- `0xD0` writes are inert until it lands --
        // keeping the song's `newm` bit.
        self.opl
            .write_reg(STEREO_EXT_REG, self.newm | STEREO_EXT_ENABLE);
        // Only the melodic channels take a panpot; the roster's last five are the
        // rhythm voices (they pan with channels 6-8). Low bank `0x0D0`-`0x0D8`,
        // high bank `0x1D0`-`0x1D8`.
        let melodic = pans.len().saturating_sub(RHYTHM_VOICES).min(18);
        for (channel, &pan) in pans.iter().take(melodic).enumerate() {
            let reg = if channel < 9 {
                0x0D0 + channel as u16
            } else {
                0x1D0 + (channel - 9) as u16
            };
            self.opl.write_reg(reg, to_panpot(pan));
        }
    }

    fn clear_channel_pans(&mut self) {
        // Idempotent: a chip that never engaged custom panning has nothing to
        // undo, and the engine calls this every mix pass for an un-panned chip.
        if !self.panned {
            return;
        }
        // Drop the enable bit (keeping the song's `newm`): the stereo-ext panpots
        // go inert the instant `0x105` bit 1 clears, so the stale `0xD0`-`0xD8`
        // values need no rewrite and the chip reverts to the song's own `0xC0`
        // stereo image. Clearing `panned` also lets `routed_write` stop forcing
        // the enable bit and stop dropping the song's own `0xD0` writes.
        self.panned = false;
        self.opl.write_reg(STEREO_EXT_REG, self.newm);
    }

    fn supports_pan(&self) -> bool {
        true
    }
}

#[cfg(all(test, feature = "nuked-opl"))]
mod tests {
    use super::*;
    use crate::opl::NukedOpl3;

    fn adapter() -> OplCoreAdapter {
        OplCoreAdapter::new(Box::new(NukedOpl3::new(NATIVE_SAMPLE_RATE)))
    }

    #[test]
    fn it_renders_at_the_opl_native_rate() {
        assert_eq!(adapter().native_rate(), NATIVE_SAMPLE_RATE);
    }

    /// A non-standard header clock repitches through the *reported* rate: the
    /// chip itself stays at the standard-clock native rate, and the engine's
    /// resampler does the transposing (the module's Rate section).
    #[test]
    fn a_projected_adapter_reports_the_header_clocks_rate() {
        let mut core = OplCoreAdapter::projected(
            Box::new(NukedOpl3::new(NATIVE_SAMPLE_RATE)),
            ChipKind::Ym3526,
        );
        // The corpus's YM3526 arcade boards: 4 MHz and 3 MHz crystals.
        core.reset(4_000_000, false);
        assert_eq!(core.native_rate(), 55_556);
        core.reset(3_000_000, false);
        assert_eq!(core.native_rate(), 41_667);
        // The standard crystal lands exactly on the native rate (rounded, not
        // truncated), so the engine's identity bypass still engages.
        core.reset(3_579_545, false);
        assert_eq!(core.native_rate(), NATIVE_SAMPLE_RATE);
        // No header clock at all falls back to the native rate.
        core.reset(0, false);
        assert_eq!(core.native_rate(), NATIVE_SAMPLE_RATE);
    }

    /// The YMF262's standard crystal is 4x the OPL2 generation's; its own
    /// divider lands it on the same 49716 Hz.
    #[test]
    fn the_ymf262_projects_through_its_own_divider() {
        let mut core = OplCoreAdapter::projected(
            Box::new(NukedOpl3::new(NATIVE_SAMPLE_RATE)),
            ChipKind::Ymf262,
        );
        core.reset(14_318_180, false);
        assert_eq!(core.native_rate(), NATIVE_SAMPLE_RATE);
    }

    /// The hardware host's constructor pins the rate: a real chip's crystal is
    /// not the file's to change.
    #[test]
    fn an_unprojected_adapter_ignores_the_header_clock() {
        let mut core = adapter();
        core.reset(4_000_000, false);
        assert_eq!(core.native_rate(), NATIVE_SAMPLE_RATE);
    }

    #[test]
    fn a_fresh_adapter_is_silent() {
        let mut core = adapter();
        let mut out = [0i32; 256];
        core.render(&mut out);
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn keying_a_note_makes_sound_and_maps_the_high_bank() {
        let mut core = adapter();
        // A minimal OPL note on channel 0 (low bank).
        for (addr, data) in [
            (0x20u16, 0x01u16),
            (0x40, 0x10),
            (0x60, 0xF0),
            (0x80, 0x77),
            (0x23, 0x01),
            (0x43, 0x00),
            (0x63, 0xF0),
            (0x83, 0x77),
            (0xA0, 0x98),
            (0xB0, 0x31), // key on
        ] {
            core.write(0, addr, data);
        }
        // A high-bank write (port 1) must reach register 0x105 (OPL3 mode), not
        // low-bank 0x05 -- exercised for the port->bank mapping.
        core.write(1, 0x05, 0x01);

        let mut out = vec![0i32; 4096 * 2];
        core.render(&mut out);
        assert!(out.iter().any(|&s| s != 0), "the note should sound");
    }

    /// `write` takes Nuked's buffered path (so a back-to-back key off/on
    /// retriggers), `replay_write` the immediate one (so a seek's burst collapses
    /// to its net state). Measured through the retrigger energy, as the OPL chip
    /// test does.
    #[test]
    fn replay_write_is_immediate_and_write_is_buffered() {
        const SETUP: [(u16, u16); 8] = [
            (0x20, 0x01),
            (0x40, 0x00),
            (0x60, 0xFA),
            (0x80, 0x0F),
            (0x23, 0x01),
            (0x43, 0x00),
            (0x63, 0xFA),
            (0x83, 0x0F),
        ];
        fn retrigger_energy(via_replay: bool) -> u64 {
            let mut core = adapter();
            for (addr, data) in SETUP {
                core.write(0, addr, data);
            }
            core.write(0, 0xB0, 0x31); // key on
            core.render(&mut vec![0i32; 16_000 * 2]); // decay to near silence
            if via_replay {
                core.replay_write(0, 0xB0, 0x11); // key off
                core.replay_write(0, 0xB0, 0x31); // key on again
            } else {
                core.write(0, 0xB0, 0x11);
                core.write(0, 0xB0, 0x31);
            }
            let mut segment = vec![0i32; 2000 * 2];
            core.render(&mut segment);
            segment.iter().map(|&s| u64::from(s.unsigned_abs())).sum()
        }
        let buffered = retrigger_energy(false);
        let immediate = retrigger_energy(true);
        assert!(
            buffered > immediate * 4,
            "write must buffer (retrigger), replay_write must not: \
             buffered={buffered} immediate={immediate}"
        );
    }

    /// A spy chip logging every register write (immediate or buffered), so the
    /// pan register policy can be inspected without an emulator.
    #[derive(Debug, Clone, Default)]
    struct SpyOpl {
        log: std::sync::Arc<std::sync::Mutex<Vec<(u16, u8)>>>,
    }

    impl OplChip for SpyOpl {
        fn reset(&mut self, _sample_rate: u32) {}
        fn write_reg(&mut self, reg: u16, value: u8) {
            self.log.lock().unwrap().push((reg, value));
        }
        fn write_reg_buffered(&mut self, reg: u16, value: u8) {
            self.log.lock().unwrap().push((reg, value));
        }
        fn generate_samples(&mut self, buffer: &mut [i16]) {
            buffer.fill(0);
        }
    }

    #[test]
    fn panning_engages_stereo_ext_and_suppresses_song_panpots() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = OplCoreAdapter::new(Box::new(SpyOpl {
            log: std::sync::Arc::clone(&log),
        }));
        assert!(core.supports_pan());

        // The song enables OPL3 mode (0x105 = newm), which the adapter passes with
        // the stereo-ext bit forced off (un-panned).
        core.write(1, 0x05, 0x01);
        assert!(log.lock().unwrap().contains(&(0x105, 0x01)));

        // Engage custom panning: channel 0 hard left, channel 1 hard right.
        let mut pans = vec![0i16; 14]; // an OPL2 roster: 9 melodic + 5 rhythm
        pans[0] = -0x100;
        pans[1] = 0x100;
        core.set_channel_pans(&pans);

        let writes = log.lock().unwrap().clone();
        assert!(
            writes.contains(&(0x105, 0x03)),
            "stereo-ext enabled, newm kept: {writes:02X?}"
        );
        assert!(
            writes.contains(&(0x0D0, 0x00)),
            "channel 0 panned hard left"
        );
        assert!(
            writes.contains(&(0x0D1, 0xFF)),
            "channel 1 panned hard right"
        );
        assert!(writes.contains(&(0x0D2, 0x80)), "channel 2 centred");

        // While panned, the song's own 0xD0 write is dropped so it cannot clobber
        // the applied pan.
        log.lock().unwrap().clear();
        core.write(0, 0xD0, 0x42);
        assert!(
            log.lock().unwrap().is_empty(),
            "a song panpot write is suppressed while panned"
        );
    }

    /// The disengage counterpart: `clear_channel_pans` drops the enable bit and
    /// unlatches, so the song's own stereo (and its `0xD0` writes) come back
    /// without a reset. This is the fix for the "Reset panning / Custom off does
    /// nothing on Nuked-OPL3" bug.
    #[test]
    fn clearing_pans_disengages_stereo_ext_and_restores_song_panpots() {
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut core = OplCoreAdapter::new(Box::new(SpyOpl {
            log: std::sync::Arc::clone(&log),
        }));
        core.write(1, 0x05, 0x01); // OPL3 mode: newm set
        let mut pans = vec![0i16; 14];
        pans[0] = -0x100;
        core.set_channel_pans(&pans);
        assert!(core.panned, "custom panning is engaged");

        // Disengage.
        log.lock().unwrap().clear();
        core.clear_channel_pans();
        assert!(!core.panned, "the latch is cleared");
        assert!(
            log.lock().unwrap().contains(&(0x105, 0x01)),
            "stereo-ext disabled, newm kept: {:02X?}",
            log.lock().unwrap()
        );

        // The song's own 0xD0 write now passes straight through again.
        log.lock().unwrap().clear();
        core.write(0, 0xD0, 0x42);
        assert!(
            log.lock().unwrap().contains(&(0x0D0, 0x42)),
            "a song panpot passes through once disengaged"
        );

        // Idempotent: clearing an already-un-panned chip writes nothing.
        log.lock().unwrap().clear();
        core.clear_channel_pans();
        assert!(
            log.lock().unwrap().is_empty(),
            "clearing an un-panned chip is a no-op"
        );
    }

    #[test]
    fn reset_silences_the_chip() {
        let mut core = adapter();
        core.write(0, 0x20, 0x01);
        core.write(0, 0xA0, 0x98);
        core.write(0, 0xB0, 0x31);
        let mut out = vec![0i32; 1024];
        core.render(&mut out);

        core.reset(0, false);
        let mut after = vec![0i32; 1024];
        core.render(&mut after);
        assert!(after.iter().all(|&s| s == 0), "reset silences it");
    }
}
