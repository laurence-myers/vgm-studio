// SPDX-License-Identifier: GPL-2.0-or-later
//! The raw libvgm C surface, transcribed from `emu/EmuStructs.h` and
//! `emu/SoundEmu.h`.
//!
//! Nothing here is safe and nothing here is opinionated: this module is the
//! declarations, and `chip` (lv-2) is where they turn into a `ChipCore`.
//! Keeping the two apart means a header drift at a pin bump shows up as a
//! compile error in one file rather than as behaviour changing in several.
//!
//! **Layout is load-bearing.** Every struct below is `#[repr(C)]` and must
//! match the pinned header field for field; a mismatch is not a compile error
//! but a wrong pointer read at runtime. The `layout` module asserts the sizes
//! *and the offsets* against what the C compiler reports, so a drift fails a
//! test instead of corrupting a chip.

#![allow(non_camel_case_types)]
// A transcribed C header is declared *complete*, not as-needed, and that is
// deliberate rather than lazy. `DEV_DEF`'s every field has to be declared or
// the struct's layout is wrong and the fields we do call sit at the wrong
// offsets -- so `set_option_bits` and `link_device` exist to be correct, not to
// be called. The same goes for the `DEVRW_` widths: the set is the vocabulary
// lv-3's per-chip write table draws from, and a width no chip has asked for yet
// is not dead, it is unclaimed.
#![allow(dead_code)]

use std::ffi::{c_char, c_void};

/// libvgm's per-sample type: `DEV_SMPL` in `snddef.h`.
pub(crate) type DevSmpl = i32;

/// `SndEmu_Start` succeeded.
pub(crate) const EERR_OK: u8 = 0x00;

// --- Sample-rate modes (`DEVRI_SRMODE_*`) ---------------------------------

/// Render at the chip's own natural rate and let the caller resample.
///
/// The only mode this crate uses: `dro_synth::resample` is what every other
/// core in the workspace goes through, so libvgm's `Resampler.c` is not even
/// compiled (see `build.rs`).
pub(crate) const DEVRI_SRMODE_NATIVE: u8 = 0x00;

// --- Read/write function selectors (`RWF_*` / `DEVRW_*`) ------------------

pub(crate) const RWF_WRITE: u8 = 0x00;
pub(crate) const RWF_REGISTER: u8 = 0x00;
pub(crate) const RWF_MEMORY: u8 = 0x10;

pub(crate) const DEVRW_A8D8: u8 = 0x11;
pub(crate) const DEVRW_A8D16: u8 = 0x12;
pub(crate) const DEVRW_A16D8: u8 = 0x21;
pub(crate) const DEVRW_A16D16: u8 = 0x22;
pub(crate) const DEVRW_BLOCK: u8 = 0x80;
pub(crate) const DEVRW_MEMSIZE: u8 = 0x81;

// --- Device IDs (`SoundDevs.h`) -------------------------------------------

/// SN76496 and its variants -- SN76489(A), SEGA PSG, T6W28.
pub(crate) const DEVID_SN76496: u8 = 0x00;

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
/// width. The per-chip table at lv-3 records which width each chip wants and
/// how our `(port, addr, data)` folds into it.
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
/// pointer to the extended struct cast to this type. That is why the field
/// order here is not negotiable.
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
/// The PoC's chip, and the reason it is the PoC's chip: these seven fields are
/// the difference between an SN76489 and a SEGA PSG, and the frozen scorecard
/// records what happens when a core has the wrong ones -- the noise channel
/// emits a completely different pseudo-random sequence and no amount of level
/// matching brings it back.
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
