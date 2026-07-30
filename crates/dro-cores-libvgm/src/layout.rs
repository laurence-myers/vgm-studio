// SPDX-License-Identifier: GPL-2.0-or-later
//! Proof that [`ffi`](crate::ffi)'s `#[repr(C)]` twins still match the pinned
//! headers.
//!
//! A struct that has drifted upstream neither fails to compile nor to link: it
//! reads the wrong field, yielding a chip at a nonsense rate or a function
//! pointer that is really an integer. The sizes and offsets below come from the
//! compiler that built libvgm, so a pin bump that moves a field fails a test
//! rather than corrupting playback.

use std::ffi::c_void;

use crate::ffi::{
    Ay8910Cfg, DevDef, DevDefRwFunc, DevGenCfg, DevInfo, DevLinkInfo, Msm6258Cfg, SegaPcmCfg,
    Sn76496Cfg,
};

unsafe extern "C" {
    fn drotrim_libvgm_gencfg_sizeof() -> usize;
    fn drotrim_libvgm_gencfg_alignof() -> usize;
    fn drotrim_libvgm_devinfo_sizeof() -> usize;
    fn drotrim_libvgm_devinfo_alignof() -> usize;
    fn drotrim_libvgm_devdef_sizeof() -> usize;
    fn drotrim_libvgm_devdef_alignof() -> usize;
    fn drotrim_libvgm_rwfunc_sizeof() -> usize;
    fn drotrim_libvgm_rwfunc_alignof() -> usize;
    fn drotrim_libvgm_sn76496cfg_sizeof() -> usize;
    fn drotrim_libvgm_sn76496cfg_alignof() -> usize;
    fn drotrim_libvgm_ay8910cfg_sizeof() -> usize;
    fn drotrim_libvgm_ay8910cfg_alignof() -> usize;
    fn drotrim_libvgm_ay8910cfg_off_chiptype() -> usize;
    fn drotrim_libvgm_msm6258cfg_sizeof() -> usize;
    fn drotrim_libvgm_msm6258cfg_alignof() -> usize;
    fn drotrim_libvgm_msm6258cfg_off_divider() -> usize;
    fn drotrim_libvgm_segapcmcfg_sizeof() -> usize;
    fn drotrim_libvgm_segapcmcfg_alignof() -> usize;
    fn drotrim_libvgm_segapcmcfg_off_bnkshift() -> usize;
    fn drotrim_libvgm_devlink_sizeof() -> usize;
    fn drotrim_libvgm_devlink_alignof() -> usize;
    fn drotrim_libvgm_devlink_off_linkid() -> usize;
    fn drotrim_libvgm_devlink_off_cfg() -> usize;

    fn drotrim_libvgm_devinfo_off_dataptr() -> usize;
    fn drotrim_libvgm_devinfo_off_samplerate() -> usize;
    fn drotrim_libvgm_devinfo_off_devdef() -> usize;
    fn drotrim_libvgm_devinfo_off_linkdevcount() -> usize;
    fn drotrim_libvgm_devinfo_off_linkdevs() -> usize;

    fn drotrim_libvgm_devdef_off_start() -> usize;
    fn drotrim_libvgm_devdef_off_stop() -> usize;
    fn drotrim_libvgm_devdef_off_reset() -> usize;
    fn drotrim_libvgm_devdef_off_update() -> usize;
    fn drotrim_libvgm_devdef_off_rwfuncs() -> usize;

    fn drotrim_libvgm_gencfg_off_srmode() -> usize;
    fn drotrim_libvgm_gencfg_off_flags() -> usize;
    fn drotrim_libvgm_gencfg_off_clock() -> usize;
    fn drotrim_libvgm_gencfg_off_smplrate() -> usize;
}

/// Rust's `offset_of!` for a field, as a `usize`.
macro_rules! offset {
    ($ty:ty, $field:ident) => {
        std::mem::offset_of!($ty, $field)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every struct we mirror is the size and alignment libvgm's compiler says
    /// it is.
    #[test]
    fn the_mirrored_structs_are_the_right_size() {
        // SAFETY: all of these are argument-free C functions returning a
        // `size_t`, compiled from `shim/layout.c` in this crate's own build.
        unsafe {
            assert_eq!(
                (size_of::<DevGenCfg>(), align_of::<DevGenCfg>()),
                (
                    drotrim_libvgm_gencfg_sizeof(),
                    drotrim_libvgm_gencfg_alignof()
                ),
                "DEV_GEN_CFG"
            );
            assert_eq!(
                (size_of::<DevInfo>(), align_of::<DevInfo>()),
                (
                    drotrim_libvgm_devinfo_sizeof(),
                    drotrim_libvgm_devinfo_alignof()
                ),
                "DEV_INFO"
            );
            assert_eq!(
                (size_of::<DevDef>(), align_of::<DevDef>()),
                (
                    drotrim_libvgm_devdef_sizeof(),
                    drotrim_libvgm_devdef_alignof()
                ),
                "DEV_DEF"
            );
            assert_eq!(
                (size_of::<DevDefRwFunc>(), align_of::<DevDefRwFunc>()),
                (
                    drotrim_libvgm_rwfunc_sizeof(),
                    drotrim_libvgm_rwfunc_alignof()
                ),
                "DEVDEF_RWFUNC"
            );
            assert_eq!(
                (size_of::<Sn76496Cfg>(), align_of::<Sn76496Cfg>()),
                (
                    drotrim_libvgm_sn76496cfg_sizeof(),
                    drotrim_libvgm_sn76496cfg_alignof()
                ),
                "SN76496_CFG"
            );
            assert_eq!(
                (size_of::<Ay8910Cfg>(), align_of::<Ay8910Cfg>()),
                (
                    drotrim_libvgm_ay8910cfg_sizeof(),
                    drotrim_libvgm_ay8910cfg_alignof()
                ),
                "AY8910_CFG"
            );
            assert_eq!(
                (size_of::<Msm6258Cfg>(), align_of::<Msm6258Cfg>()),
                (
                    drotrim_libvgm_msm6258cfg_sizeof(),
                    drotrim_libvgm_msm6258cfg_alignof()
                ),
                "MSM6258_CFG"
            );
            assert_eq!(
                (size_of::<SegaPcmCfg>(), align_of::<SegaPcmCfg>()),
                (
                    drotrim_libvgm_segapcmcfg_sizeof(),
                    drotrim_libvgm_segapcmcfg_alignof()
                ),
                "SEGAPCM_CFG"
            );
            assert_eq!(
                (size_of::<DevLinkInfo>(), align_of::<DevLinkInfo>()),
                (
                    drotrim_libvgm_devlink_sizeof(),
                    drotrim_libvgm_devlink_alignof()
                ),
                "DEVLINK_INFO"
            );
            assert_eq!(
                offset!(Ay8910Cfg, chip_type),
                drotrim_libvgm_ay8910cfg_off_chiptype(),
                "AY8910_CFG::chipType"
            );
            assert_eq!(
                offset!(Msm6258Cfg, divider),
                drotrim_libvgm_msm6258cfg_off_divider(),
                "MSM6258_CFG::divider"
            );
            assert_eq!(
                offset!(SegaPcmCfg, bnkshift),
                drotrim_libvgm_segapcmcfg_off_bnkshift(),
                "SEGAPCM_CFG::bnkshift"
            );
            assert_eq!(
                offset!(DevLinkInfo, link_id),
                drotrim_libvgm_devlink_off_linkid(),
                "DEVLINK_INFO::linkID"
            );
            assert_eq!(
                offset!(DevLinkInfo, cfg),
                drotrim_libvgm_devlink_off_cfg(),
                "DEVLINK_INFO::cfg"
            );
        }
    }

    /// ...and every field we read sits where C puts it.
    ///
    /// Size agreement alone would pass with two same-width fields swapped,
    /// which is exactly the drift that would hand `Update` a pointer to
    /// `Reset`.
    #[test]
    fn the_fields_we_read_sit_where_c_puts_them() {
        // SAFETY: as above -- argument-free `size_t` returns from our shim.
        unsafe {
            assert_eq!(
                offset!(DevInfo, data_ptr),
                drotrim_libvgm_devinfo_off_dataptr(),
                "DEV_INFO::dataPtr"
            );
            assert_eq!(
                offset!(DevInfo, sample_rate),
                drotrim_libvgm_devinfo_off_samplerate(),
                "DEV_INFO::sampleRate"
            );
            assert_eq!(
                offset!(DevInfo, dev_def),
                drotrim_libvgm_devinfo_off_devdef(),
                "DEV_INFO::devDef"
            );
            assert_eq!(
                offset!(DevInfo, link_dev_count),
                drotrim_libvgm_devinfo_off_linkdevcount(),
                "DEV_INFO::linkDevCount"
            );
            assert_eq!(
                offset!(DevInfo, link_devs),
                drotrim_libvgm_devinfo_off_linkdevs(),
                "DEV_INFO::linkDevs"
            );

            assert_eq!(
                offset!(DevDef, start),
                drotrim_libvgm_devdef_off_start(),
                "DEV_DEF::Start"
            );
            assert_eq!(
                offset!(DevDef, stop),
                drotrim_libvgm_devdef_off_stop(),
                "DEV_DEF::Stop"
            );
            assert_eq!(
                offset!(DevDef, reset),
                drotrim_libvgm_devdef_off_reset(),
                "DEV_DEF::Reset"
            );
            assert_eq!(
                offset!(DevDef, update),
                drotrim_libvgm_devdef_off_update(),
                "DEV_DEF::Update"
            );
            assert_eq!(
                offset!(DevDef, rw_funcs),
                drotrim_libvgm_devdef_off_rwfuncs(),
                "DEV_DEF::rwFuncs"
            );

            assert_eq!(
                offset!(DevGenCfg, sr_mode),
                drotrim_libvgm_gencfg_off_srmode(),
                "DEV_GEN_CFG::srMode"
            );
            assert_eq!(
                offset!(DevGenCfg, flags),
                drotrim_libvgm_gencfg_off_flags(),
                "DEV_GEN_CFG::flags"
            );
            assert_eq!(
                offset!(DevGenCfg, clock),
                drotrim_libvgm_gencfg_off_clock(),
                "DEV_GEN_CFG::clock"
            );
            assert_eq!(
                offset!(DevGenCfg, smpl_rate),
                drotrim_libvgm_gencfg_off_smplrate(),
                "DEV_GEN_CFG::smplRate"
            );
        }
    }

    /// An extended config must start with the generic one, byte for byte:
    /// `SndEmu_Start` takes `(const DEV_GEN_CFG*)&snCfg` and reads through it.
    /// If this ever stopped holding, every chip with settings would start with
    /// a garbage clock.
    #[test]
    fn an_extended_config_begins_with_the_generic_one() {
        assert_eq!(offset!(Sn76496Cfg, gen_cfg), 0);
        // And the extension really does extend rather than overlap.
        assert!(size_of::<Sn76496Cfg>() > size_of::<DevGenCfg>());
        assert_eq!(size_of::<*mut c_void>(), align_of::<Sn76496Cfg>());
    }
}
