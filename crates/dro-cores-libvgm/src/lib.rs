// SPDX-License-Identifier: GPL-2.0-or-later
//! Sound cores from [libvgm](https://github.com/ValleyBell/libvgm), Valley
//! Bell's modular rewrite of VGMPlay's emulation half.
//!
//! This is the primary accuracy tier of the reuse re-scope
//! (`docs/vgm-multichip-2026-07/LIBVGM-PLAN.md`). What makes libvgm different
//! in kind from the other providers is that **one API covers every chip**:
//! `SndEmu_Start(DEV_ID, const DEV_GEN_CFG*, DEV_INFO*)` hands back a `DEV_DEF`
//! of function pointers, so the same wrapper drives a QSound and a SAA1099.
//! There is no per-chip C++ class to instantiate as ymfm needs, and no device
//! framework to satisfy as MAME needs. Adding a chip is a build-table row and a
//! write-table row.
//!
//! # The licence position, stated plainly
//!
//! **libvgm ships no licence grant.** As of the pinned commit there is no
//! `LICENSE` and no `COPYING` at its root, GitHub's licence API reports none,
//! and the framework headers this crate compiles against -- `EmuStructs.h`,
//! `SoundEmu.h`, `EmuHelper.h`, `snddef.h` -- carry no per-file tag. Some
//! vendored cores do carry the tags they arrived with (the MAME-derived ones
//! are `BSD-3-Clause`, the Nuked ones `LGPL-2.1+`), but the framework that
//! binds them does not.
//!
//! Code published without a grant is all rights reserved by default. A git
//! submodule is unaffected by that -- we redistribute nothing and the user
//! fetches from upstream -- but **a released binary containing this object code
//! is redistribution of a derivative work**, and that is the unresolved
//! question. It is not a copyleft-compatibility problem that the GPL tier
//! solves; there is no grant to be compatible with.
//!
//! This is a factual finding, not legal advice, and the project owner makes the
//! risk call. `LIBVGM-PLAN.md` lv-0 tracks it, and `CORES-REUSE-PLAN.md` §5
//! lists the options -- the cheapest being to ask upstream for a `LICENSE`.
//! The crate's `license` key is set to the app's copyleft tier because that is
//! the most conservative home for a dependency whose terms are unresolved.
//!
//! # What is not here
//!
//! **OPL, by the owner's decision.** libvgm has YM3812, YM3526, Y8950 and
//! YMF262 cores and this crate does not compile them. OPL2/OPL3 keeps exactly
//! three options -- Nuked-OPL3 (the default, a vendored Rust port), Nuked-CQM
//! and RetroWave -- because `PlayerEngine` carries the buffered-write spacing,
//! muting and panning the DRO *editor* depends on, which makes it the editing
//! engine rather than a swappable playback core.
//!
//! **libvgm's own Nuked cores**, which our submodules already serve. See
//! `build.rs`'s `CORES_SERVED_ELSEWHERE` -- and note that the symbol collision
//! LIBVGM-PLAN §4 feared does not exist, because upstream renamed every entry
//! point with an `N` prefix.
//!
//! **libvgm's resampler and DAC-stream controller.** We start chips at their
//! native rate and resample with `dro_synth::resample`, and our engine already
//! implements the VGM `0x90`-`0x95` DAC stream commands itself.

mod ffi;
/// Test-only: nothing in the shipped library reads a struct offset, so the
/// guard and its `extern` declarations exist purely to fail a test.
#[cfg(test)]
mod layout;

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use crate::ffi::{self, DevGenCfg, DevInfo, EERR_OK, RWF_REGISTER, RWF_WRITE, Sn76496Cfg};

    /// The SN76489 as the corpus declares it: 15-bit shift register, `0x0003`
    /// feedback, output negated, clock divided by 8. These exact numbers are
    /// what the frozen scorecard caught our own core getting wrong.
    fn sn76489_config(clock: u32) -> Sn76496Cfg {
        Sn76496Cfg {
            gen_cfg: DevGenCfg {
                emu_core: 0, // the device's default core
                sr_mode: ffi::DEVRI_SRMODE_NATIVE,
                flags: 0,
                clock,
                // Ignored in native mode, but some cores read it regardless,
                // so it is never left as garbage.
                smpl_rate: 44_100,
            },
            noise_taps: 0x0003,
            shift_reg_width: 15,
            negate: 1,
            clk_div: 8,
            ncr_psg: 0,
            sega_psg: 0,
            stereo: 0,
            t6w28_tone: std::ptr::null_mut(),
        }
    }

    /// **The lv-1 PoC gate.** libvgm compiles into this workspace, links,
    /// starts a device, takes register writes and makes a sound.
    ///
    /// Deliberately the same standard as ru-1's ymfm gate rather than the
    /// weaker "`SndEmu_Start` returns 0" the plan wrote down: a core that
    /// starts and stays silent is the classic mis-paced-write symptom, and
    /// catching it here is the whole point of having a gate before the
    /// generic binding is built on top.
    ///
    /// The sequence mirrors upstream's own `emutest.c`.
    #[test]
    fn libvgm_links_and_the_psg_sounds() {
        let cfg = sn76489_config(3_579_545);
        let mut dev = DevInfo::empty();

        // SAFETY: `cfg` is an `SN76496_CFG` whose first member is a
        // `DEV_GEN_CFG`, which is the cast upstream's own example makes; `dev`
        // is a valid out-param that libvgm fills in.
        let started = unsafe {
            ffi::SndEmu_Start(
                ffi::DEVID_SN76496,
                (&raw const cfg).cast::<DevGenCfg>(),
                &raw mut dev,
            )
        };
        assert_eq!(started, EERR_OK, "SndEmu_Start returned an error code");
        assert!(!dev.data_ptr.is_null());
        assert!(!dev.dev_def.is_null());
        assert!(
            dev.sample_rate > 8_000,
            "a native-rate PSG should render somewhere near its clock/16, got {}",
            dev.sample_rate
        );

        // SAFETY: `dev_def` was just filled in by a successful start.
        let dev_def = unsafe { &*dev.dev_def };

        // The core identifies itself; proves we are reading the vtable at the
        // right offsets rather than merely reading *something*.
        // SAFETY: libvgm's core names are static nul-terminated literals.
        let name = unsafe { CStr::from_ptr(dev_def.name) }.to_string_lossy();
        assert!(!name.is_empty(), "the core should name itself");

        // SAFETY: `data_ptr` came from the same successful start.
        unsafe { dev_def.reset.expect("every core has a Reset")(dev.data_ptr) };

        let writer =
            unsafe { ffi::device_func(dev.dev_def, RWF_REGISTER | RWF_WRITE, ffi::DEVRW_A8D8, 0) }
                .expect("the SN76496 takes 8-bit address, 8-bit data register writes");
        // SAFETY: `DEVRW_A8D8` is exactly this signature, by the constant's
        // definition in `EmuStructs.h`.
        let write: ffi::DevFuncWriteA8D8 = unsafe { std::mem::transmute(writer) };

        let render = |frames: usize| -> i64 {
            let mut left = vec![0i32; frames];
            let mut right = vec![0i32; frames];
            let mut planes = [left.as_mut_ptr(), right.as_mut_ptr()];
            // SAFETY: `Update` writes `frames` samples into each of the two
            // planes, which are exactly `frames` long and outlive the call.
            unsafe {
                dev_def.update.expect("every core has an Update")(
                    dev.data_ptr,
                    frames as u32,
                    planes.as_mut_ptr(),
                );
            }
            left.iter()
                .chain(right.iter())
                .map(|&s| i64::from(s.abs()))
                .sum()
        };

        // Every channel attenuated to silence is the reset state; prove it is
        // quiet before proving a note is loud, so the assertion below cannot
        // pass on noise that was there all along.
        let at_rest = render(4096);

        // Channel 0: latch tone (0x80 | freq low nibble), then the high six
        // bits, then attenuation 0 (0x90) -- which on this chip means loudest.
        // SAFETY: `write` is the core's own `A8D8` register writer and
        // `data_ptr` is the handle it expects; the PSG masks the address
        // itself, so no value here can be out of range.
        unsafe {
            write(dev.data_ptr, 0, 0x80 | 0x0E);
            write(dev.data_ptr, 0, 0x02);
            write(dev.data_ptr, 0, 0x90);
        }

        let keyed = render(4096);
        assert!(
            keyed > at_rest * 4 + 1000,
            "the PSG must sound once a channel is un-attenuated \
             (rest {at_rest}, playing {keyed}) -- a core that starts but stays \
             silent is the classic mis-paced-write symptom"
        );

        // SAFETY: `dev` was started successfully and is stopped exactly once.
        // The link data is the caller's to free, so both calls are needed or
        // the allocation leaks.
        unsafe {
            ffi::SndEmu_FreeDevLinkData(&raw mut dev);
            ffi::SndEmu_Stop(&raw mut dev);
        }
    }

    /// An unknown device ID is an error code, never a half-built chip.
    #[test]
    fn an_unknown_device_is_refused() {
        let cfg = sn76489_config(3_579_545);
        let mut dev = DevInfo::empty();
        // SAFETY: deliberately passing a device ID libvgm does not define;
        // the answer is `EERR_UNK_DEVICE` and an untouched `dev`.
        let started =
            unsafe { ffi::SndEmu_Start(0xFE, (&raw const cfg).cast::<DevGenCfg>(), &raw mut dev) };
        assert_ne!(started, EERR_OK);
        assert!(dev.data_ptr.is_null());
    }

    /// A device we did not compile is refused the same way -- which is what
    /// makes `build.rs`'s `ENABLED` list the single source of truth for what
    /// this build offers, rather than something the registry has to be kept in
    /// step with by hand.
    #[test]
    fn a_device_left_out_of_the_build_is_refused() {
        let cfg = sn76489_config(3_579_545);
        let mut dev = DevInfo::empty();
        // `DEVID_QSOUND` (0x1F) is a real device that this build does not
        // compile. It becomes available the moment `ENABLED` names it.
        // SAFETY: as above; an uncompiled device ID is simply unknown.
        let started =
            unsafe { ffi::SndEmu_Start(0x1F, (&raw const cfg).cast::<DevGenCfg>(), &raw mut dev) };
        assert_ne!(
            started, EERR_OK,
            "QSOUND is not in build.rs's ENABLED list, so it must not start"
        );
    }

    /// The device names itself from its configuration -- the mechanism the
    /// Settings picker will read at lv-6, and a second witness that
    /// `DEV_GEN_CFG` is laid out right, since the long name is chosen by
    /// reading through the extended config.
    #[test]
    fn the_device_names_the_variant_its_config_describes() {
        let cfg = sn76489_config(3_579_545);
        // SAFETY: `opts` bit 0 asks for the long name, which reads `cfg`.
        let name = unsafe {
            let ptr = ffi::SndEmu_GetDevName(
                ffi::DEVID_SN76496,
                0x01,
                (&raw const cfg).cast::<DevGenCfg>(),
            );
            assert!(!ptr.is_null());
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        };
        assert_eq!(
            name, "SN76489",
            "a 15-bit shift register with clkDiv 8 is an SN76489; a different \
             answer means DEV_GEN_CFG's fields are being read at the wrong \
             offsets"
        );
    }

    /// `device_func` reports absence rather than handing back a null pointer
    /// to transmute -- the difference between a `None` and a crash at lv-3,
    /// where every chip asks for the writer width it wants.
    #[test]
    fn asking_for_a_width_the_core_lacks_is_a_none() {
        let cfg = sn76489_config(3_579_545);
        let mut dev = DevInfo::empty();
        // SAFETY: a valid config and out-param, as in the gate above.
        let started = unsafe {
            ffi::SndEmu_Start(
                ffi::DEVID_SN76496,
                (&raw const cfg).cast::<DevGenCfg>(),
                &raw mut dev,
            )
        };
        assert_eq!(started, EERR_OK);

        // SAFETY: a live `DEV_DEF` from a successful start.
        let missing = unsafe {
            ffi::device_func(dev.dev_def, RWF_REGISTER | RWF_WRITE, ffi::DEVRW_A16D16, 0)
        };
        assert!(
            missing.is_none(),
            "the PSG has no 16-bit-address, 16-bit-data register writer"
        );

        // SAFETY: started successfully, stopped exactly once.
        unsafe {
            ffi::SndEmu_FreeDevLinkData(&raw mut dev);
            ffi::SndEmu_Stop(&raw mut dev);
        }
    }
}
