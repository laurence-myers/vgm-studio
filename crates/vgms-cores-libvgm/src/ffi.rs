// SPDX-License-Identifier: GPL-2.0-or-later
//! The raw libvgm C surface, transcribed from `emu/EmuStructs.h` and
//! `emu/SoundEmu.h`.
//!
//! Nothing here is safe or opinionated: this module is the declarations, and
//! `chip` is where they turn into a `ChipCore`. Keeping the two apart means a
//! header drift at a pin bump shows up as a compile error in one file.
//!
//! **Layout is load-bearing.** Every struct below is `#[repr(C)]` and must match
//! the pinned header field for field; a mismatch is not a compile error but a
//! wrong pointer read at runtime. The `layout` module asserts the sizes *and the
//! offsets* against what the C compiler reports, so a drift fails a test instead
//! of corrupting a chip.

#![allow(non_camel_case_types)]
// A transcribed C header is declared *complete*, not as-needed: `DEV_DEF`'s
// every field must be declared or the fields we do call sit at the wrong
// offsets, so `set_option_bits` and `link_device` exist to be correct, not to
// be called. Likewise the `DEVRW_` widths are the vocabulary the per-chip write
// table draws from; a width no chip has asked for yet is unclaimed, not dead.
#![allow(dead_code)]

use std::ffi::{c_char, c_void};

/// libvgm's per-sample type: `DEV_SMPL` in `snddef.h`.
pub(crate) type DevSmpl = i32;

/// `SndEmu_Start` succeeded.
pub(crate) const EERR_OK: u8 = 0x00;

// --- Sample-rate modes (`DEVRI_SRMODE_*`) ---------------------------------

/// Render at the chip's own natural rate and let the caller resample.
///
/// The only mode this crate uses: `vgms_synth::resample` does the conversion, so
/// libvgm's `Resampler.c` is not even compiled (see `build.rs`).
pub(crate) const DEVRI_SRMODE_NATIVE: u8 = 0x00;

// --- Read/write function selectors (`RWF_*` / `DEVRW_*`) ------------------

pub(crate) const RWF_WRITE: u8 = 0x00;
pub(crate) const RWF_REGISTER: u8 = 0x00;
pub(crate) const RWF_MEMORY: u8 = 0x10;

/// Matches any width -- used with a `user` code to fetch a chip's special
/// entry points, such as the AY8910's stereo-mask function.
pub(crate) const DEVRW_ALL: u8 = 0x00;

pub(crate) const DEVRW_A8D8: u8 = 0x11;
pub(crate) const DEVRW_A8D16: u8 = 0x12;
pub(crate) const DEVRW_A16D8: u8 = 0x21;
pub(crate) const DEVRW_A16D16: u8 = 0x22;
pub(crate) const DEVRW_BLOCK: u8 = 0x80;
pub(crate) const DEVRW_MEMSIZE: u8 = 0x81;

/// The `user` code the AY8910 cores file their stereo-mask writer under:
/// `'ST'`, as upstream's `Cmd_AY_Stereo` fetches it.
pub(crate) const USER_STEREO_MASK: u16 = 0x5354;

// --- Device IDs (`SoundDevs.h`) -------------------------------------------

/// SN76496 and its variants -- SN76489(A), SEGA PSG, T6W28.
pub(crate) const DEVID_SN76496: u8 = 0x00;
pub(crate) const DEVID_YM2413: u8 = 0x01;
pub(crate) const DEVID_YM2612: u8 = 0x02;
pub(crate) const DEVID_YM2151: u8 = 0x03;
pub(crate) const DEVID_SEGAPCM: u8 = 0x04;
/// RF5C68; the RF5C164 is the same device with `flags` set.
pub(crate) const DEVID_RF5C68: u8 = 0x05;
pub(crate) const DEVID_YM2203: u8 = 0x06;
pub(crate) const DEVID_YM2608: u8 = 0x07;
pub(crate) const DEVID_YM2610: u8 = 0x08;
/// The OPL3. Never registered as a chip from this crate -- OPL plays through
/// `DroEngine` -- compiled and declared only as the OPL4's linked FM half.
pub(crate) const DEVID_YMF262: u8 = 0x0C;
/// The OPL4. Not an OPL row here -- the OPL family plays through
/// `DroEngine` -- but the OPL4's own wave half is this device, which links
/// a YMF262 child for its FM half.
pub(crate) const DEVID_YMF278B: u8 = 0x0D;
pub(crate) const DEVID_YMF271: u8 = 0x0E;
pub(crate) const DEVID_YMZ280B: u8 = 0x0F;
pub(crate) const DEVID_32X_PWM: u8 = 0x11;
pub(crate) const DEVID_AY8910: u8 = 0x12;
pub(crate) const DEVID_GB_DMG: u8 = 0x13;
pub(crate) const DEVID_NES_APU: u8 = 0x14;
/// MultiPCM (315-5560).
pub(crate) const DEVID_YMW258: u8 = 0x15;
pub(crate) const DEVID_UPD7759: u8 = 0x16;
pub(crate) const DEVID_MSM6258: u8 = 0x17;
pub(crate) const DEVID_MSM6295: u8 = 0x18;
pub(crate) const DEVID_K051649: u8 = 0x19;
pub(crate) const DEVID_K054539: u8 = 0x1A;
pub(crate) const DEVID_C6280: u8 = 0x1B;
pub(crate) const DEVID_C140: u8 = 0x1C;
pub(crate) const DEVID_K053260: u8 = 0x1D;
pub(crate) const DEVID_POKEY: u8 = 0x1E;
pub(crate) const DEVID_QSOUND: u8 = 0x1F;
pub(crate) const DEVID_SCSP: u8 = 0x20;
pub(crate) const DEVID_WSWAN: u8 = 0x21;
pub(crate) const DEVID_VBOY_VSU: u8 = 0x22;
pub(crate) const DEVID_SAA1099: u8 = 0x23;
pub(crate) const DEVID_ES5503: u8 = 0x24;
/// ES5506; the ES5505 is a variant of the same device.
pub(crate) const DEVID_ES5506: u8 = 0x25;
pub(crate) const DEVID_X1_010: u8 = 0x26;
pub(crate) const DEVID_C352: u8 = 0x27;
pub(crate) const DEVID_GA20: u8 = 0x28;
pub(crate) const DEVID_MIKEY: u8 = 0x29;
/// The C219, a C140 variant with its own device ("TODO: renumber" upstream).
pub(crate) const DEVID_C219: u8 = 0x80;

// --- Emulation-core codes (`EmuCores.h`) ----------------------------------
//
// The four-character code in `DEV_GEN_CFG::emuCore` picks *which* emulator
// serves a device, and `0` takes whichever the device lists first. Naming one
// is what makes a parity measurement mean anything: the pinned reference config
// names a core per chip, and a row that does not match it measures two
// different emulators.

/// MAME's cores. libvgm's default for several devices, including the SN76496.
pub(crate) const FCC_MAME: u32 = 0x4D414D45;
/// Maxim's SN76489, from in_vgm -- what the pinned reference selects.
pub(crate) const FCC_MAXM: u32 = 0x4D41584D;
/// Ootake's HuC6280 -- what the pinned reference selects for that chip.
pub(crate) const FCC_OOTK: u32 = 0x4F4F544B;
/// EMU2149/EMU2413 -- what upstream's link callback selects for an OPN's SSG.
/// `"EMU\0"`, not `"EMU_"`: the fourth character really is a NUL upstream.
pub(crate) const FCC_EMU_: u32 = 0x454D5500;
/// adlibemu -- what upstream's link callback selects for the OPL4's FM half.
pub(crate) const FCC_ADLE: u32 = 0x41444C45;
/// Gens -- the YM2612 and RF5C68 alternatives.
pub(crate) const FCC_GENS: u32 = 0x47454E53;

// NSFPlay (`FCC_NSFP`), SameBoy (`FCC_SBOY`), superctr (`FCC_CTR_`) and Valley
// Bell (`FCC_VBEL`) are each a device's *first-listed* core, so `emu_core: 0`
// already reaches them and no row names them explicitly. A constant here exists
// iff a row selects that core; add one back from `EmuCores.h` if that changes.

// --- Function-pointer types (`EmuStructs.h`) ------------------------------

pub(crate) type DevFuncStart =
    Option<unsafe extern "C" fn(cfg: *const DevGenCfg, ret_dev_inf: *mut DevInfo) -> u8>;
pub(crate) type DevFuncCtrl = Option<unsafe extern "C" fn(info: *mut c_void)>;
pub(crate) type DevFuncUpdate =
    Option<unsafe extern "C" fn(info: *mut c_void, samples: u32, outputs: *mut *mut DevSmpl)>;
pub(crate) type DevFuncOptMask = Option<unsafe extern "C" fn(info: *mut c_void, option_bits: u32)>;
pub(crate) type DevFuncPanAll =
    Option<unsafe extern "C" fn(info: *mut c_void, channel_pan_val: *const i16)>;
pub(crate) type DevFuncSrcCb = Option<
    unsafe extern "C" fn(
        info: *mut c_void,
        callback: Option<unsafe extern "C" fn(*mut c_void, u32)>,
        param: *mut c_void,
    ),
>;
pub(crate) type DevFuncLinkDev = Option<
    unsafe extern "C" fn(info: *mut c_void, link_id: u8, dev_inf_link: *const DevInfo) -> u8,
>;
pub(crate) type DevFuncSetLogCb = Option<
    unsafe extern "C" fn(
        info: *mut c_void,
        log_func: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, u8, *const c_char)>,
        user_param: *mut c_void,
    ),
>;

/// A register writer: `DEVFUNC_WRITE_A8D8` and friends.
///
/// These are what [`SndEmu_GetDeviceFunc`] hands back, one per address/data
/// width. The per-chip table records which width each chip wants and how our
/// `(port, addr, data)` folds into it.
pub(crate) type DevFuncWriteA8D8 = unsafe extern "C" fn(info: *mut c_void, addr: u8, data: u8);
pub(crate) type DevFuncWriteA8D16 = unsafe extern "C" fn(info: *mut c_void, addr: u8, data: u16);
pub(crate) type DevFuncWriteA16D8 = unsafe extern "C" fn(info: *mut c_void, addr: u16, data: u8);
pub(crate) type DevFuncWriteA16D16 = unsafe extern "C" fn(info: *mut c_void, addr: u16, data: u16);
pub(crate) type DevFuncWriteMemSize = unsafe extern "C" fn(info: *mut c_void, memsize: u32);
pub(crate) type DevFuncWriteBlock =
    unsafe extern "C" fn(info: *mut c_void, offset: u32, length: u32, data: *const u8);

// --- Structs (`EmuStructs.h`) ---------------------------------------------

/// `DEV_GEN_CFG`. The base of every chip's configuration.
///
/// Chips with extra settings define a struct whose **first member is one of
/// these** (`SN76496_CFG` below is the pattern), and `SndEmu_Start` is handed a
/// pointer to the extended struct cast to this type -- so the field order here
/// is not negotiable.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct DevGenCfg {
    /// Four-character core code from `EmuCores.h`; 0 selects the device's
    /// default core.
    pub emu_core: u32,
    pub sr_mode: u8,
    pub flags: u8,
    pub clock: u32,
    /// Only read in `SRMODE_CUSTOM`/`SRMODE_HIGHEST` -- but some cores ignore
    /// `srMode` and always use this, so it is always set to something sane.
    pub smpl_rate: u32,
}

/// `SN76496_CFG` from `cores/sn764intf.h`.
///
/// These seven fields are the difference between an SN76489 and a SEGA PSG; the
/// wrong ones give the noise channel a completely different pseudo-random
/// sequence that no level matching can bring back.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Sn76496Cfg {
    pub gen_cfg: DevGenCfg,
    pub noise_taps: u16,
    pub shift_reg_width: u8,
    pub negate: u8,
    pub clk_div: u8,
    pub ncr_psg: u8,
    pub sega_psg: u8,
    pub stereo: u8,
    /// The tone half of a T6W28, which is two SN76489As linked. Null for
    /// everything else.
    pub t6w28_tone: *mut c_void,
}

/// A special writer fetched by `user` code with [`DEVRW_ALL`]: upstream's
/// `DEVFUNC_OPTMASK` shape, used for the AY8910's stereo mask.
pub(crate) type DevFuncWriteOptMask = unsafe extern "C" fn(info: *mut c_void, bits: u32);

/// `AY8910_CFG` from `cores/ayintf.h`: the chip type and flags bytes the VGM
/// header carries at `0x78`/`0x79` (and, for an OPN's SSG, at `0x7A`/`0x7B`).
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Ay8910Cfg {
    pub gen_cfg: DevGenCfg,
    pub chip_type: u8,
    pub chip_flags: u8,
}

/// `MSM6258_CFG` from `cores/okim6258.h`, filled from the header's flags byte
/// at `0x94` exactly as upstream's `DEVID_MSM6258` case does.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Msm6258Cfg {
    pub gen_cfg: DevGenCfg,
    /// 0 = /1024, 1 = /768, 2 = /512.
    pub divider: u8,
    /// Bits per ADPCM sample: 3 or 4; 0 takes the default (4).
    pub adpcm_bits: u8,
    /// DAC output precision: 10 or 12; 0 takes the default (10).
    pub output_bits: u8,
}

/// `SEGAPCM_CFG` from `cores/segapcm.h`: the interface register the header
/// carries at `0x3C`.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct SegaPcmCfg {
    pub gen_cfg: DevGenCfg,
    pub bnkshift: u8,
    pub bnkmask: u8,
}

/// `DEVLINK_INFO` from `EmuStructs.h`: one linkable child device, declared by
/// a parent's `Start` and freed by [`SndEmu_FreeDevLinkData`].
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct DevLinkInfo {
    /// `DEVID_` constant of the expected child.
    pub dev_id: u8,
    /// The id `LinkDevice` is called with.
    pub link_id: u8,
    /// The child's configuration, allocated by the parent -- mutated in place
    /// before the child starts, exactly as upstream's link callback does.
    pub cfg: *mut DevGenCfg,
}

/// `DEVDEF_RWFUNC`: one entry in a core's read/write function table.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct DevDefRwFunc {
    pub func_type: u8,
    pub rw_type: u8,
    pub user: u16,
    pub func_ptr: *mut c_void,
}

/// `DEV_DEF`: a core's vtable.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct DevDef {
    pub name: *const c_char,
    pub author: *const c_char,
    pub core_id: u32,

    pub start: DevFuncStart,
    pub stop: DevFuncCtrl,
    pub reset: DevFuncCtrl,
    pub update: DevFuncUpdate,

    pub set_option_bits: DevFuncOptMask,
    pub set_mute_mask: DevFuncOptMask,
    /// Deprecated upstream in favour of an `rwFuncs` entry; declared so the
    /// layout is right, never called.
    pub set_panning: DevFuncPanAll,
    pub set_srate_chg_cb: DevFuncSrcCb,
    pub set_log_cb: DevFuncSetLogCb,
    pub link_device: DevFuncLinkDev,

    /// Terminated by an entry whose `func_ptr` is null.
    pub rw_funcs: *const DevDefRwFunc,
}

/// `DEV_INFO`: what `SndEmu_Start` fills in.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(crate) struct DevInfo {
    /// `DEV_DATA*` -- the handle every `DEV_DEF` function takes as `info`.
    pub data_ptr: *mut c_void,
    /// The rate `Update` renders at, once started in `SRMODE_NATIVE`.
    pub sample_rate: u32,
    pub dev_def: *const DevDef,
    /// `DEV_DECL*`; null when a `DEV_DEF::Start` was called directly.
    pub dev_decl: *const c_void,

    pub link_dev_count: u32,
    /// `DEVLINK_INFO*`, freed by [`SndEmu_FreeDevLinkData`].
    pub link_devs: *mut c_void,
}

impl DevInfo {
    /// A zeroed `DEV_INFO` to hand to `SndEmu_Start`.
    pub(crate) const fn empty() -> Self {
        Self {
            data_ptr: std::ptr::null_mut(),
            sample_rate: 0,
            dev_def: std::ptr::null(),
            dev_decl: std::ptr::null(),
            link_dev_count: 0,
            link_devs: std::ptr::null_mut(),
        }
    }
}

// --- The C entry points (`SoundEmu.h`) ------------------------------------

unsafe extern "C" {
    /// Starts `device_id` with `cfg`, filling `ret_dev_inf`. Returns an
    /// `EERR_` code; [`EERR_OK`] is success.
    pub(crate) fn SndEmu_Start(
        device_id: u8,
        cfg: *const DevGenCfg,
        ret_dev_inf: *mut DevInfo,
    ) -> u8;

    /// Stops a device started by [`SndEmu_Start`]. Always returns 0.
    pub(crate) fn SndEmu_Stop(dev_inf: *mut DevInfo) -> u8;

    /// Frees the linkable-device information `SndEmu_Start` allocated.
    ///
    /// The caller owns it, so this leaks if it is not called -- which is why
    /// the wrapper's `Drop` does both this and [`SndEmu_Stop`].
    pub(crate) fn SndEmu_FreeDevLinkData(dev_inf: *mut DevInfo);

    /// Fetches one of a core's read/write functions by
    /// (`func_type`, `rw_type`, `user`). Returns an `EERR_` code.
    pub(crate) fn SndEmu_GetDeviceFunc(
        dev_def: *const DevDef,
        func_type: u8,
        rw_type: u8,
        user: u16,
        ret_func_ptr: *mut *mut c_void,
    ) -> u8;

    /// The device's name. `opts` bit 0 asks for the long form, which reads
    /// `cfg` to name the exact variant.
    pub(crate) fn SndEmu_GetDevName(
        device_id: u8,
        opts: u8,
        dev_cfg: *const DevGenCfg,
    ) -> *const c_char;
}

/// Fetches a register/memory writer from `dev_def`, or `None` when the core
/// has none of that width.
///
/// # Safety
/// `dev_def` must be a live `DEV_DEF` from a started device. The caller is
/// responsible for transmuting the returned pointer to the *right* signature
/// for `rw_type` -- `DEVRW_A8D8` really is `A8D8` and nothing else.
pub(crate) unsafe fn device_func(
    dev_def: *const DevDef,
    func_type: u8,
    rw_type: u8,
    user: u16,
) -> Option<*mut c_void> {
    let mut ptr: *mut c_void = std::ptr::null_mut();
    // SAFETY: the caller guarantees `dev_def`; `ptr` is a valid out-param.
    // libvgm returns EERR_MORE_FOUND (1) when several candidates matched,
    // having still written the first -- that is a success, so the test is on
    // the pointer rather than on the code.
    unsafe { SndEmu_GetDeviceFunc(dev_def, func_type, rw_type, user, &raw mut ptr) };
    (!ptr.is_null()).then_some(ptr)
}
