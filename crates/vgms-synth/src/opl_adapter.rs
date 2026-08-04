//! Hosting an OPL chip inside [`VgmEngine`](crate::vgm_engine::VgmEngine): the
//! [`OplChip`] → [`ChipCore`] adapter (Stage K / ou-1).
//!
//! The two engines were deliberately separate — `VgmEngine` pulls samples from a
//! [`ChipCore`], `PlayerEngine` drives an [`OplChip`] through its own muting,
//! panning and buffered-write policy — so an OPL VGM through `VgmEngine` rendered
//! silence (there was no OPL `ChipCore` to host). This adapter closes that gap:
//! it presents an `OplChip` as a `ChipCore`, so the generic engine can play the
//! OPL family like any other chip.
//!
//! ## Rate
//!
//! An OPL chip resamples internally to whatever rate it is reset at. The adapter
//! runs it at the chip's **native** rate ([`NATIVE_SAMPLE_RATE`], the YMF262's
//! 14.318 MHz / 288) and reports that as its [`native_rate`](ChipCore::native_rate),
//! so the engine's own [`Voice`](crate::vgm_engine) resampler converts to the
//! output rate — exactly as it does for every other core. At the OPL native rate
//! that resampler's identity bypass engages, so there is never a double resample;
//! and no output rate need reach core construction, which is why
//! [`CoreInfo::build`](crate::CoreInfo) needs no rate parameter.
//!
//! ## Writes
//!
//! A live write takes the [buffered path](OplChip::write_reg_buffered) — the
//! same one `PlayerEngine` uses for live playback — so a back-to-back key
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
//! Per-channel panning is the OPL stereo-ext register policy `PlayerEngine`
//! owns, ported here behind [`set_channel_pans`](ChipCore::set_channel_pans):
//! engaging it forces `0x105`'s stereo-ext bit on (shadowing the song's `newm`
//! bit), writes the panpots (`0x0D0`-`0x0D8` low bank, `0x1D0`-`0x1D8` high), and
//! thereafter suppresses the song's own `0xD0`-`0xD8` writes so they cannot
//! clobber the applied pan. Un-panned, the song's own `0xC0`-`0xC8` stereo bits
//! pass straight through. **One limitation:** the `ChipCore` pan API has no
//! "return to the song's own stereo" call (a channel is a position, not a mode),
//! so once engaged the adapter stays engaged until [`reset`](ChipCore::reset);
//! disengaging mid-playback (the OPL panel's Original-vs-Custom that
//! `PlayerEngine` resyncs through a `0xC0` shadow) is left to ou-2, when OPL
//! documents route here and that vocabulary arrives.

use crate::NATIVE_SAMPLE_RATE;
use crate::chip::ChipCore;
use crate::opl::OplChip;

/// An [`OplChip`] presented as a [`ChipCore`], so [`VgmEngine`](crate::vgm_engine)
/// can drive the OPL family.
pub struct OplCoreAdapter {
    opl: Box<dyn OplChip>,
    /// A reused `i16` staging buffer: the OPL chip renders `i16`, the engine
    /// wants `i32`.
    scratch: Vec<i16>,
    /// Custom per-channel panning is engaged (the stereo-ext panpots). While it
    /// is, the song's own `0xD0`-`0xD8` writes are suppressed and the enable bit
    /// forced on -- the same register policy `PlayerEngine` owns.
    panned: bool,
    /// The song's OPL3 `newm` bit (`0x105` bit 0), shadowed so forcing the
    /// stereo-ext enable (bit 1) never disturbs OPL3 mode.
    newm: u8,
}

impl OplCoreAdapter {
    /// Wraps `opl`, which must already be constructed at [`NATIVE_SAMPLE_RATE`].
    #[must_use]
    pub fn new(opl: Box<dyn OplChip>) -> Self {
        Self {
            opl,
            scratch: Vec::new(),
            panned: false,
            newm: 0,
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
    fn reset(&mut self, _clock: u32, _variant: bool) {
        // The OPL resamples to the rate it is reset at; the engine's Voice
        // resampler does the output conversion, so reset at the native rate. The
        // header clock is not used -- Nuked assumes the standard YMF262 clock,
        // exactly as `PlayerEngine` does.
        self.opl.reset(NATIVE_SAMPLE_RATE);
        // A fresh chip is un-panned; the engine restates the pans afterward (its
        // `apply_mix`), so a loop's rewind keeps custom panning.
        self.panned = false;
        self.newm = 0;
    }

    fn native_rate(&self) -> u32 {
        NATIVE_SAMPLE_RATE
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
