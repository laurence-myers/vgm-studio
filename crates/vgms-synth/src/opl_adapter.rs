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
//! Register writes take the [buffered path](OplChip::write_reg_buffered) — the
//! same one `PlayerEngine` uses for live playback — so a back-to-back key
//! off/on with no samples between still retriggers the note, matching real
//! hardware. (A seek's bulk replay wants the immediate path instead, since only
//! the final register *values* matter; giving the adapter that distinction is a
//! follow-up — `ChipCore` has no live-vs-replay signal today, and the ou-1
//! acceptance gate renders from the start, never seeking.)
//!
//! ## Muting and panning
//!
//! Per-channel muting is the [`ChannelGate`](crate::channel_gate)'s job: the OPL
//! rows are already built, and [`CoreInfo::build`](crate::CoreInfo) wraps a
//! gate-covered, non-native-mute core in a `GatedCore` that filters the writes.
//! So the adapter itself keeps the trait's no-op mute. Per-channel panning (the
//! OPL stereo-ext register policy `PlayerEngine` owns) is not ported here yet —
//! a follow-up — so the adapter reports [`supports_pan`](ChipCore::supports_pan)
//! false and passes the song's own `0xC0`-`0xC8` stereo bits straight through.

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
}

impl OplCoreAdapter {
    /// Wraps `opl`, which must already be constructed at [`NATIVE_SAMPLE_RATE`].
    #[must_use]
    pub fn new(opl: Box<dyn OplChip>) -> Self {
        Self {
            opl,
            scratch: Vec::new(),
        }
    }
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
    }

    fn native_rate(&self) -> u32 {
        NATIVE_SAMPLE_RATE
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        // OPL3's two register banks are the write's port: port 0 is the low
        // bank, port 1 the high bank (bit 8 of the chip's register address).
        let reg = (u16::from(port) << 8) | addr;
        self.opl.write_reg_buffered(reg, data as u8);
    }

    fn render(&mut self, out: &mut [i32]) {
        self.scratch.resize(out.len(), 0);
        self.opl.generate_samples(&mut self.scratch);
        for (slot, &sample) in out.iter_mut().zip(&self.scratch) {
            *slot = i32::from(sample);
        }
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
