// SPDX-License-Identifier: GPL-2.0-or-later
//! Sound cores from [libvgm](https://github.com/ValleyBell/libvgm), Valley
//! Bell's modular rewrite of VGMPlay's emulation half.
//!
//! What makes libvgm different in kind from the other providers is that **one
//! API covers every chip**: `SndEmu_Start(DEV_ID, const DEV_GEN_CFG*,
//! DEV_INFO*)` hands back a `DEV_DEF` of function pointers, so the same wrapper
//! drives a QSound and a SAA1099. No per-chip C++ class as ymfm needs, no device
//! framework as MAME needs; adding a chip is a build-table row and a write-table
//! row.
//!
//! # The licence position
//!
//! **libvgm ships no licence grant** -- no `LICENSE`/`COPYING`, no per-file tag
//! on the framework headers this crate compiles against. Some vendored cores
//! carry the tags they arrived with (MAME-derived `BSD-3-Clause`, Nuked
//! `LGPL-2.1+`), but the framework that binds them does not. Code published
//! without a grant is all rights reserved by default: the git submodule
//! redistributes nothing, but **a released binary containing this object code
//! is redistribution of a derivative work**, and that is the unresolved
//! question the project owner must call. The crate's `license` key is set to the
//! app's copyleft tier as the most conservative home for unresolved terms.
//!
//! # What is not here
//!
//! **OPL, by the owner's decision.** libvgm's YM3812/YM3526/Y8950/YMF262 cores
//! are not compiled: OPL plays through our own OPL path (`VgmEngine`'s
//! `OplCoreAdapter`), which carries the buffered-write spacing, muting and
//! panning the DRO editor depends on.
//!
//! **libvgm's own Nuked cores**, which our submodules already serve -- see
//! `build.rs`'s `CORES_SERVED_ELSEWHERE`.
//!
//! **libvgm's resampler and DAC-stream controller.** We start chips at their
//! native rate and resample with `vgms_synth::resample`, and our engine
//! implements the VGM `0x90`-`0x95` DAC stream commands itself.

mod chip;
mod ffi;
mod fold;
/// Test-only: nothing in the shipped library reads a struct offset, so the
/// layout guard exists purely to fail a test.
#[cfg(test)]
mod layout;
mod specs;
#[cfg(target_arch = "wasm32")]
mod wasm_libc;

pub use chip::LibVgmChip;

/// The id every libvgm core is registered under, per chip slot.
///
/// One suffix for the whole provider rather than one per chip: the id is
/// `"<slot>.<core>"` and the slot already names the chip.
pub const CORE_SUFFIX: &str = "libvgm";

/// Adds every chip this build can serve to the registry.
///
/// **Registered ahead of the other providers, so libvgm is the default for
/// every chip it serves.** The app's `install_cores` calls this first; Nuked and
/// LLE register behind it as picker options. Three exceptions: the app promotes
/// Nuked back over the YM2612, YM2151 and YM2413 rows, leaving libvgm on their
/// pickers.
pub fn register(registry: &mut vgms_synth::CoreRegistry) {
    for spec in specs::SPECS {
        registry.register(vgms_synth::CoreInfo {
            id: spec.registry_id(),
            chip: spec.kind,
            label: spec.label,
            authors: "the libvgm project and upstream core authors",
            license: "see PROVENANCE.md -- upstream publishes no grant",
            upstream: "https://github.com/ValleyBell/libvgm",
            realtime: true,
            channel_pan: chip::default_core_pans(spec.kind),
            // libvgm implements `set_channel_mutes` (via `split_mute`), so its
            // cores honour channel muting -- the reason the UI keeps the toggles
            // live for a chip resolved to libvgm.
            channel_mute: true,
            level: spec.level,
            make: spec.maker(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use crate::ffi::{self, DevGenCfg, DevInfo, EERR_OK, RWF_REGISTER, RWF_WRITE, Sn76496Cfg};

    /// The SN76489 as the corpus declares it: 15-bit shift register, `0x0003`
    /// feedback, output negated, clock divided by 8.
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

    /// libvgm compiles into this workspace, links, starts a device, takes
    /// register writes and makes a sound.
    ///
    /// Asserts sound, not just a clean start: a core that starts and stays
    /// silent is the classic mis-paced-write symptom. The sequence mirrors
    /// upstream's own `emutest.c`.
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
        // right offsets rather than merely reading something.
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

    /// A device we did not compile is refused the same way -- which makes
    /// `build.rs`'s `ENABLED` the single source of truth for what this build
    /// offers, rather than something the registry must be kept in step with.
    #[test]
    fn a_device_left_out_of_the_build_is_refused() {
        let cfg = sn76489_config(3_579_545);
        let mut dev = DevInfo::empty();
        // `DEVID_MSM5232` (0x2D) is a real device this build does not compile;
        // it becomes available the moment `ENABLED` names it.
        // SAFETY: as above; an uncompiled device ID is simply unknown.
        let started =
            unsafe { ffi::SndEmu_Start(0x2D, (&raw const cfg).cast::<DevGenCfg>(), &raw mut dev) };
        assert_ne!(
            started, EERR_OK,
            "MSM5232 is not in build.rs's ENABLED list, so it must not start"
        );
    }

    /// The device names itself from its configuration -- a second witness that
    /// `DEV_GEN_CFG` is laid out right, since the long name is chosen by reading
    /// through the extended config.
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

    /// `device_func` reports absence rather than handing back a null pointer to
    /// transmute -- the difference between a `None` and a crash.
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
