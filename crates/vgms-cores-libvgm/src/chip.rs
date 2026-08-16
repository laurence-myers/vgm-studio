// SPDX-License-Identifier: GPL-2.0-or-later
//! One wrapper, every chip: [`LibVgmChip`] over libvgm's uniform device API.
//!
//! A `DEV_DEF` is a vtable of `Start`/`Stop`/`Reset`/`Update` plus a table of
//! width-typed register writers, so one wrapper drives a QSound and a SAA1099
//! alike. What varies per chip is *data* -- a [`ChipSpec`] row saying which
//! device ID, which writer width, and how our engine's `(port, addr, data)`
//! folds into that writer's arguments.
//!
//! # The two conventions that have to be reconciled
//!
//! **libvgm takes the clock at construction.** `DEV_GEN_CFG::clock` is read by
//! `Start` and the sample rate falls out of it, but our [`ChipCore::reset`]
//! hands a clock to a chip that already exists -- so `reset` here *restarts*:
//! stop the old device, start a new one.
//!
//! **libvgm renders planar, our engine wants interleaved.** `Update` writes
//! `outputs[0]` and `outputs[1]` as two separate `INT32` runs and *overwrites*
//! rather than accumulating, so [`render`](ChipCore::render) keeps two scratch
//! planes and weaves them.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use vgms_core::vgm::{ChipKind, ChipSettings};
use vgms_synth::chip::ChipCore;

use crate::ffi::{
    self, Ay8910Cfg, DevFuncWriteA8D8, DevFuncWriteA8D16, DevFuncWriteA16D8, DevFuncWriteA16D16,
    DevFuncWriteBlock, DevFuncWriteMemSize, DevFuncWriteOptMask, DevGenCfg, DevInfo, DevLinkInfo,
    EERR_OK, Msm6258Cfg, RWF_MEMORY, RWF_REGISTER, RWF_WRITE, SegaPcmCfg, Sn76496Cfg,
};
use crate::fold::{Bus, WriteRule, fold};
use crate::specs::{ChipSpec, DevConfig};

/// The rate asked for in `DEV_GEN_CFG::smplRate`.
///
/// Nominally unused: we start every chip in `DEVRI_SRMODE_NATIVE`. But upstream
/// warns that *some cores ignore `srMode` and always use `smplRate`* (Maxim's
/// SN76489 is one), so for those chips this is the rate they run at. 44100
/// matches the pinned parity reference, so a rate-fixed core measures against it
/// with no resampler either side. The engine resamples from whatever
/// [`native_rate`](ChipCore::native_rate) reports, read back from libvgm.
const REQUESTED_RATE: u32 = 44_100;

/// Every entry point a chip might use, fetched once at start.
///
/// libvgm files its writers by `(funcType, rwType, user)`, and the combination
/// is the signature contract -- a pointer filed under `DEVRW_A8D8` takes
/// `(void*, UINT8, UINT8)` and nothing else. Fetching the whole set keeps
/// [`WriteRule`] a pure description of a fold.
///
/// The names mirror `player/vgmplayer.cpp`'s `CHIP_DEVICE` fields. **Note
/// `reg16`**: upstream calls it `writeM16` but fetches it with `RWF_REGISTER`,
/// not `RWF_MEMORY` -- copying the name without reading the fetch would put the
/// C352's writes into the wrong space.
#[derive(Debug, Clone, Copy, Default)]
struct Writers {
    /// `write8`: `RWF_REGISTER | DEVRW_A8D8`.
    reg8: Option<DevFuncWriteA8D8>,
    /// `writeD16`: `RWF_REGISTER | DEVRW_A8D16` -- the 32X PWM and ES5506.
    data16: Option<DevFuncWriteA8D16>,
    /// `writeM16`: `RWF_REGISTER | DEVRW_A16D16` -- register, despite the name.
    reg16: Option<DevFuncWriteA16D16>,
    /// `writeM8`: `RWF_MEMORY | DEVRW_A16D8`.
    mem8: Option<DevFuncWriteA16D8>,
    /// The stereo-mask function AY8910 cores file under the `'ST'` user code.
    /// For an OPN chip it is fetched from the *linked* SSG device instead --
    /// see `start_links`.
    stereo: Option<DevFuncWriteOptMask>,
    /// `romSize`/`romSizeB`: `RWF_MEMORY | DEVRW_MEMSIZE`, per memory space.
    rom_size: [Option<DevFuncWriteMemSize>; 2],
    /// `romWrite`/`romWriteB`: `RWF_MEMORY | DEVRW_BLOCK`, per memory space.
    rom_write: [Option<DevFuncWriteBlock>; 2],
}

impl Writers {
    /// Fetches everything `spec` might need from a started device.
    ///
    /// # Safety
    /// `dev_def` must belong to a live device.
    unsafe fn fetch(dev_def: *const ffi::DevDef, spec: &ChipSpec) -> Self {
        // SAFETY: the caller guarantees `dev_def`; each transmute pairs a
        // `DEVRW_` constant with the one signature libvgm files under it.
        unsafe {
            let reg =
                |rw: u8, user: u16| ffi::device_func(dev_def, RWF_REGISTER | RWF_WRITE, rw, user);
            let mem =
                |rw: u8, user: u16| ffi::device_func(dev_def, RWF_MEMORY | RWF_WRITE, rw, user);
            Self {
                reg8: reg(ffi::DEVRW_A8D8, 0)
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteA8D8>(p)),
                data16: reg(ffi::DEVRW_A8D16, 0)
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteA8D16>(p)),
                reg16: reg(ffi::DEVRW_A16D16, 0)
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteA16D16>(p)),
                stereo: reg(ffi::DEVRW_ALL, ffi::USER_STEREO_MASK)
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteOptMask>(p)),
                // **Memory first, then register** -- the space a chip files its
                // 16-bit-address writer under is per chip, not per width. SegaPCM
                // exposes only `RWF_REGISTER | DEVRW_A16D8`, so the memory space
                // alone finds nothing and drops every write. A chip exposes the
                // width in exactly one space, so trying both cannot pick wrong.
                mem8: mem(ffi::DEVRW_A16D8, 0)
                    .or_else(|| reg(ffi::DEVRW_A16D8, 0))
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteA16D8>(p)),
                rom_size: spec.rom_spaces.map(|user| {
                    mem(ffi::DEVRW_MEMSIZE, user)
                        .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteMemSize>(p))
                }),
                rom_write: spec.rom_spaces.map(|user| {
                    mem(ffi::DEVRW_BLOCK, user)
                        .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteBlock>(p))
                }),
            }
        }
    }

    /// Whether the entry point `rule` needs was actually found.
    ///
    /// A missing one is not fatal (writes are dropped, the chip is silent) but is
    /// the likeliest symptom of a wrong table row, so it is logged at start.
    const fn serves(&self, rule: WriteRule) -> bool {
        match rule {
            WriteRule::Register
            | WriteRule::RegisterLatch
            | WriteRule::QSound
            | WriteRule::MultiPcmBank
            | WriteRule::NesApu
            | WriteRule::Okim6295
            | WriteRule::ReversedLatch
            // The stereo mask is optional per core; the register file is not.
            | WriteRule::RegisterWithStereo
            | WriteRule::OpnFamily => self.reg8.is_some(),
            WriteRule::Memory | WriteRule::MemoryPortHigh => self.mem8.is_some(),
            WriteRule::RegisterAddr16Data16 => self.reg16.is_some(),
            WriteRule::Data16 => self.data16.is_some(),
            // Needs both, and a chip missing either is half-mute.
            WriteRule::RegisterOrMemoryByPort | WriteRule::WonderSwan => {
                self.reg8.is_some() && self.mem8.is_some()
            }
        }
    }
}

/// Which of a chip's two sample-memory spaces a VGM data-block type names.
///
/// Transcribed from upstream's `_VGM_ROM_CHIPS` table. Only three block types
/// name the second space, so the default is the first and the exceptions are
/// listed.
const fn rom_space(block_type: u8) -> u8 {
    match block_type {
        // The YM2608's Delta-T, the second space beside its (internal)
        // rhythm ROM.
        0x81 => 1,
        // The YM2610's ADPCM-B (Delta-T), beside `0x82`'s ADPCM-A.
        0x83 => 1,
        // The YMF278B's RAM, beside `0x84`'s ROM.
        0x87 => 1,
        _ => 0,
    }
}

/// One of a chip's linked child devices -- an OPN's SSG, the OPL4's FM half.
///
/// libvgm links devices for *register* traffic (the parent forwards SSG register
/// I/O through hooks `LinkDevice` installs), but each device renders its own
/// audio stream at its own rate. So a child here carries a small linear
/// resampler into the parent's rate and a gain, mirroring `GetChipVolume`'s
/// link column.
struct LinkedDev {
    dev: DevInfo,
    /// libvgm's `DEVID_` for this child, so a mute mask can be routed to it: the
    /// OPN family's SSG channels live on the linked `DEVID_AY8910`, not on the
    /// parent's own mute mask.
    dev_id: u8,
    /// The child's level relative to the parent, 8.8 fixed point --
    /// upstream's `GetChipVolume(..., isLinked=1)`: half for the YM2203's SSG,
    /// unity otherwise.
    gain: u16,
    /// Child frames per parent frame, 32.32 fixed point.
    step: u64,
    /// Position between [`last`](Self::last) and the next child frame, 32.32.
    pos: u64,
    /// The child frame behind the cursor, for interpolation across calls.
    last: [i32; 2],
    /// Scratch planes for the child's own `Update`.
    left: Vec<i32>,
    right: Vec<i32>,
}

impl LinkedDev {
    /// Renders `frames` parent-rate frames of the child, linearly resampled,
    /// and mixes them into `out_left`/`out_right` at [`gain`](Self::gain).
    ///
    /// # Safety
    /// The child device must be live.
    unsafe fn mix_into(&mut self, out_left: &mut [i32], out_right: &mut [i32], frames: usize) {
        const ONE: u64 = 1 << 32;
        let Some(update) = (unsafe { *self.dev.dev_def }).update else {
            return;
        };

        // How many child frames the cursor will cross while `frames` parent
        // frames advance it by `step` each: everything left of the final
        // position's integer part.
        let end = self.pos + self.step * frames as u64;
        let needed = (end >> 32) as usize;
        if self.left.len() < needed.max(1) {
            self.left.resize(needed.max(1), 0);
            self.right.resize(needed.max(1), 0);
        }
        if needed > 0 {
            let mut planes = [self.left.as_mut_ptr(), self.right.as_mut_ptr()];
            // SAFETY: a live child; the planes hold `needed` frames.
            unsafe { update(self.dev.data_ptr, needed as u32, planes.as_mut_ptr()) };
        }

        let gain = i64::from(self.gain);
        let mut consumed = 0usize;
        for frame in 0..frames {
            self.pos += self.step;
            while self.pos >= ONE {
                self.last = [
                    self.left.get(consumed).copied().unwrap_or(0),
                    self.right.get(consumed).copied().unwrap_or(0),
                ];
                consumed += 1;
                self.pos -= ONE;
            }
            let next = [
                self.left.get(consumed).copied().unwrap_or(self.last[0]),
                self.right.get(consumed).copied().unwrap_or(self.last[1]),
            ];
            let frac = (self.pos >> 16) as i64; // 0..65536
            let blend = |last: i32, next: i32| -> i64 {
                let interpolated =
                    i64::from(last) + (((i64::from(next) - i64::from(last)) * frac) >> 16);
                (interpolated * gain) >> 8
            };
            out_left[frame] = out_left[frame].saturating_add(blend(self.last[0], next[0]) as i32);
            out_right[frame] = out_right[frame].saturating_add(blend(self.last[1], next[1]) as i32);
        }
        debug_assert!(consumed <= needed, "resampler overran its buffer");
    }
}

/// One libvgm chip, owned.
pub struct LibVgmChip {
    spec: &'static ChipSpec,
    /// Zeroed while stopped; `data_ptr` non-null means started.
    dev: DevInfo,
    writers: Writers,
    /// Children started from the parent's `linkDevs` declarations, rendering
    /// their own streams beside it.
    links: Vec<LinkedDev>,
    /// What the last [`reset`](ChipCore::reset) asked for, kept so
    /// [`configure`](ChipCore::configure) can restart at the same clock.
    clock: u32,
    variant: bool,
    settings: ChipSettings,
    /// Which channels are muted, in [`vgms_core::vgm::channels_of`] order. Kept
    /// here because a device restart clears the core's own mask, so it is
    /// reapplied after each `start`.
    mute_mask: u32,
    /// Where each channel sits in the stereo image, libvgm's `-0x100..=0x100`.
    /// Empty means the chip's own image; reapplied after each `start`.
    pans: Vec<i16>,
    /// The RF5C pair's selected RAM bank, pre-shifted (`wbank << 12`), OR'd
    /// into every RAM-write address exactly as upstream's `DoRAMOfsPatches`
    /// does. Tracked here -- mirroring `Cmd_RF5C_Reg`'s bank patch -- because
    /// the *player* owns this OR upstream; the core cannot do it (its byte
    /// window masks to 4 KiB) and the block writer takes absolute addresses.
    /// Zero for every other chip.
    ram_bank: u32,
    /// The QSound old-log key-on caches, upstream's `QSOUND_WORK`: per
    /// channel, the last start-address and pitch writes. Old (4 MHz clock)
    /// QSound VGMs were logged against an HLE where an address or phase write
    /// implied key-on, and `vgm_cmp` then stripped the "redundant" address
    /// rewrites -- so the reference re-injects the cached start address on a
    /// pitch rising from zero and on any phase write. Only read for an
    /// old-clock QSound; zero for every other chip.
    qsound_start: [u16; 16],
    qsound_pitch: [u16; 16],
    /// Whether the started QSound core wants those old-log key-on injections:
    /// true only for an old-clock QSound VGM whose started core is not MAME.
    /// Mirrors upstream's `chipDev.flags & 0x01` -- set for an old clock, then
    /// cleared again for the MAME core, whose HLE never needed the hacks.
    /// Computed at `start`; false for every other chip.
    qsound_hacks: bool,
    /// The two planes `Update` writes, grown as needed and never shrunk.
    left: Vec<i32>,
    right: Vec<i32>,
    /// The rate the core is rendering at *now* -- not necessarily what `start`
    /// read back. A few cores re-derive their output rate from a register
    /// write (the ES5503's oscillator-enable register divides its clock by
    /// `oscillators + 2`; the OKIM6258's clock registers move it too) and
    /// announce it through `SetSampleRateChangeCallback`. This is the slot
    /// that callback writes into; boxed so the address handed to C stays put
    /// while the chip itself moves.
    rate: Box<AtomicU32>,
}

/// The `DEVCB_SRATE_CHG` trampoline: `param` is the chip's [`LibVgmChip::rate`]
/// slot. Called from inside `write`/`reset` on the thread driving the chip.
unsafe extern "C" fn rate_changed(param: *mut c_void, new_rate: u32) {
    // SAFETY: `param` is the address of the owning chip's boxed slot, which
    // outlives the device (the device is stopped before the box drops).
    let slot = unsafe { &*param.cast::<AtomicU32>() };
    slot.store(new_rate, Ordering::Relaxed);
}

impl std::fmt::Debug for LibVgmChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibVgmChip")
            .field("chip", &self.spec.kind)
            .field("device", &self.spec.device)
            .field("clock", &self.clock)
            .field("started", &self.is_started())
            .field("rate", &self.dev.sample_rate)
            .finish()
    }
}

// SAFETY: the device is exclusively owned (the handle is never cloned or handed
// out) and the cores this crate compiles hold no mutable file-scope state, so
// all mutation is behind `data_ptr`. This is a **per-core** property: a core
// added to `build.rs`'s ENABLED list must be checked for mutable globals before
// it is trusted here. Not `Sync`: two threads must not write one chip at once.
unsafe impl Send for LibVgmChip {}

impl LibVgmChip {
    /// A chip built to `spec`, not yet started.
    ///
    /// Starting waits for [`reset`](ChipCore::reset), which supplies the clock --
    /// a construction parameter to libvgm, so there is nothing to build first.
    #[must_use]
    pub(crate) fn new(spec: &'static ChipSpec) -> Self {
        Self {
            spec,
            dev: DevInfo::empty(),
            writers: Writers::default(),
            links: Vec::new(),
            clock: 0,
            variant: false,
            settings: ChipSettings::default(),
            mute_mask: 0,
            pans: Vec::new(),
            ram_bank: 0,
            qsound_start: [0; 16],
            qsound_pitch: [0; 16],
            qsound_hacks: false,
            left: Vec::new(),
            right: Vec::new(),
            rate: Box::new(AtomicU32::new(0)),
        }
    }

    fn is_started(&self) -> bool {
        !self.dev.data_ptr.is_null()
    }

    /// Stops the device and its linked children, if running. Idempotent.
    ///
    /// Children first, then the parent -- the reverse of how they were
    /// started, and the order upstream's `FreeDeviceTree` uses.
    fn stop(&mut self) {
        for mut link in self.links.drain(..) {
            // SAFETY: each child was filled by its own successful start.
            unsafe {
                ffi::SndEmu_FreeDevLinkData(&raw mut link.dev);
                ffi::SndEmu_Stop(&raw mut link.dev);
            }
        }
        if !self.is_started() {
            return;
        }
        // SAFETY: `dev` was filled by a successful `SndEmu_Start` and is
        // stopped exactly once -- `data_ptr` is cleared below, and every path
        // that starts a device goes through `start`, which stops first.
        unsafe {
            ffi::SndEmu_FreeDevLinkData(&raw mut self.dev);
            ffi::SndEmu_Stop(&raw mut self.dev);
        }
        self.dev = DevInfo::empty();
        self.writers = Writers::default();
        self.rate.store(0, Ordering::Relaxed);
        self.qsound_hacks = false;
    }

    /// Stops whatever is running and starts a device at the current clock and
    /// settings.
    ///
    /// A failure leaves the chip stopped rather than half-built, so
    /// [`render`](ChipCore::render) renders silence and nothing reads a dangling
    /// pointer -- the honest outcome for "this build has no such device".
    fn start(&mut self) {
        self.stop();
        if self.clock == 0 {
            return;
        }

        // Which config shape the device reads: the chips with extended
        // structs, and the generic prefix for everything else.
        let mut config = match self.spec.kind {
            ChipKind::Sn76489 => DevConfig::Sn76496(Sn76496Cfg::default()),
            ChipKind::Ay8910 => DevConfig::Ay8910(Ay8910Cfg::default()),
            ChipKind::Okim6258 => DevConfig::Msm6258(Msm6258Cfg::default()),
            ChipKind::SegaPcm => DevConfig::SegaPcm(SegaPcmCfg::default()),
            _ => DevConfig::Generic(DevGenCfg::default()),
        };
        {
            let generic = config.generic_mut();
            generic.emu_core = self.spec.emu_core;
            generic.sr_mode = sr_mode(self.spec.kind);
            generic.flags = u8::from(self.variant);
            generic.clock = self.clock;
            generic.smpl_rate = REQUESTED_RATE;
        }
        (self.spec.configure)(&mut config, &self.settings);

        // The C219 is its own libvgm device rather than a C140 flag, so the
        // header's type byte picks the device -- upstream's `DEVID_C140` case
        // does exactly this.
        let device = if self.spec.kind == ChipKind::C140 && self.settings.c140_type == 2 {
            ffi::DEVID_C219
        } else {
            self.spec.device
        };

        let mut dev = DevInfo::empty();
        // SAFETY: `config` outlives the call, its pointer is the documented
        // cast to the generic prefix, and `dev` is a valid out-param.
        let started = unsafe { ffi::SndEmu_Start(device, config.as_ptr(), &raw mut dev) };
        if started != EERR_OK || dev.data_ptr.is_null() || dev.dev_def.is_null() {
            log::warn!(
                "libvgm refused to start {} (device {device:#04x}): error {started:#04x}",
                self.spec.kind.name(),
            );
            return;
        }

        self.dev = dev;
        self.rate.store(dev.sample_rate, Ordering::Relaxed);
        // Upstream's QSound flag (vgmplayer.cpp:1567-1585): the old-log key-on
        // hacks apply to an old-clock QSound VGM, but are cleared when the
        // *started* core is MAME, whose HLE emulates the implied key-on itself.
        // Testing the started core id (not the requested emu_core) is exactly
        // what the reference does.
        // SAFETY: a live device definition from the successful start above
        // (dev.dev_def was null-checked).
        let core_id = unsafe { (*dev.dev_def).core_id };
        self.qsound_hacks = self.spec.kind == ChipKind::QSound
            && self.clock < crate::specs::QSOUND_OLD_CLOCK_MAX_HZ
            && core_id != ffi::FCC_MAME;
        // SAFETY: a live device from the start above; the slot's box outlives
        // it. Registered before the reset below, because a reset is one of the
        // places a rate-deriving core (the ES5503) announces its rate.
        unsafe {
            if let Some(set_rate_cb) = (*dev.dev_def).set_srate_chg_cb {
                let slot: *const AtomicU32 = &raw const *self.rate;
                set_rate_cb(dev.data_ptr, Some(rate_changed), slot.cast_mut().cast());
            }
        }
        // SAFETY: a live device definition from the successful start above.
        self.writers = unsafe { Writers::fetch(dev.dev_def, self.spec) };
        if !self.writers.serves(self.spec.write) {
            log::warn!(
                "libvgm's {} has no writer for {:?}; its registers will be \
                 silently dropped",
                self.spec.kind.name(),
                self.spec.write,
            );
        }

        // Option bits VGMPlay sets by default, applied before any register
        // arrives so the core never runs in a state the reference never uses.
        let option_bits = start_option_bits(self.spec.kind, self.variant);
        if option_bits != 0 {
            // SAFETY: a live device from the start above.
            unsafe {
                if let Some(set_options) = (*dev.dev_def).set_option_bits {
                    set_options(dev.data_ptr, option_bits);
                }
            }
        }

        self.start_links();

        // SAFETY: as above -- a live device, reset exactly as upstream's own
        // example does immediately after starting.
        unsafe {
            if let Some(reset) = (*dev.dev_def).reset {
                reset(dev.data_ptr);
            }
        }

        // The start above cleared any mute mask and pan image the core held,
        // so restate them -- a reset must not un-mute a channel the user
        // muted before it.
        self.apply_muting();
        self.apply_panning();
    }

    /// Re-writes a QSound channel's cached start address as the old HLE's
    /// implied key-on -- upstream's `qsWork->write(cDev, (chn << 3) | 0x01,
    /// startAddrCache[chn])`, folded to the QSound bus triple (data MSB, data
    /// LSB, then the committing register write). Routed through the shared
    /// [`put_bus`](Self::put_bus) dispatch but *not* through
    /// [`write`](ChipCore::write): the injection must not re-enter the caches.
    fn qsound_key_on(&self, addr: u16, start: u16) {
        let register = (addr & !0x07) | 0x01;
        self.put_bus(fold(WriteRule::QSound, 0, register, start));
    }

    /// Puts one folded [`Bus`] onto libvgm's FFI writers -- the single dispatch
    /// every write goes through, whether from a register write or a QSound
    /// key-on injection.
    fn put_bus(&self, bus: Bus) {
        let chip = self.dev.data_ptr;
        // SAFETY: a live device, and every writer below was fetched from it and
        // is called with the signature libvgm filed it under. What to send was
        // decided by `fold`, which is pure and tested on its own.
        unsafe {
            match bus {
                Bus::Reg8(address, value) => {
                    if let Some(write) = self.writers.reg8 {
                        write(chip, address, value);
                    }
                }
                Bus::Reg8Pair(first, second) => {
                    if let Some(write) = self.writers.reg8 {
                        write(chip, first.0, first.1);
                        write(chip, second.0, second.1);
                    }
                }
                Bus::Reg8Triple(first, second, third) => {
                    if let Some(write) = self.writers.reg8 {
                        write(chip, first.0, first.1);
                        write(chip, second.0, second.1);
                        write(chip, third.0, third.1);
                    }
                }
                Bus::Mem8(address, value) => {
                    if let Some(write) = self.writers.mem8 {
                        write(chip, address, value);
                    }
                }
                Bus::Reg16(address, value) => {
                    if let Some(write) = self.writers.reg16 {
                        write(chip, address, value);
                    }
                }
                Bus::RegD16(address, value) => {
                    if let Some(write) = self.writers.data16 {
                        write(chip, address, value);
                    }
                }
                Bus::StereoMask(mask) => {
                    if let Some(write) = self.writers.stereo {
                        // The AY's own chip for a bare AY8910; the linked SSG's
                        // for an OPN -- `start_links` fetched the function from
                        // whichever device carries it, and its data pointer
                        // travels with it.
                        let target = self
                            .links
                            .iter()
                            .find(|link| !link.dev.data_ptr.is_null())
                            .map_or(chip, |link| link.dev.data_ptr);
                        write(target, u32::from(mask));
                    }
                }
                Bus::Nothing => {}
            }
        }
    }

    /// Pushes the stored mute mask into the device (and, for the OPN family,
    /// its linked SSG). A no-op while stopped or when the core cannot mute.
    fn apply_muting(&self) {
        if !self.is_started() {
            return;
        }
        let (parent_mask, ssg_mask) = split_mute(self.spec.kind, self.mute_mask);
        // SAFETY: a live device; `set_mute_mask` is `DEV_DEF` field 9, taking
        // `(info, u32)` -- the `DevFuncOptMask` signature it is declared with.
        unsafe {
            if let Some(set_mute) = (*self.dev.dev_def).set_mute_mask {
                set_mute(self.dev.data_ptr, parent_mask);
            }
        }
        let Some(ssg_mask) = ssg_mask else {
            return;
        };
        // The SSG lives on the linked `DEVID_AY8910` child, with its own
        // 3-bit mask -- the parent's mute mask does not reach it.
        for link in &self.links {
            if link.dev_id != ffi::DEVID_AY8910 || link.dev.data_ptr.is_null() {
                continue;
            }
            // SAFETY: a live child device, its own `set_mute_mask` field.
            unsafe {
                if let Some(set_mute) = (*link.dev.dev_def).set_mute_mask {
                    set_mute(link.dev.data_ptr, ssg_mask);
                }
            }
        }
    }

    /// Pushes the stored pan image into the device, if it has one set and the
    /// core can pan.
    fn apply_panning(&self) {
        if !self.is_started() || self.pans.is_empty() {
            return;
        }
        // SAFETY: a live device; `set_panning` is `DEV_DEF` field 10, taking
        // `(info, *const i16)` -- the `DevFuncPanAll` signature. The array
        // outlives the call, and the core reads only as many channels as it
        // has (it is handed the whole chip's worth).
        unsafe {
            if let Some(set_pan) = (*self.dev.dev_def).set_panning {
                set_pan(self.dev.data_ptr, self.pans.as_ptr());
            }
        }
    }

    /// Starts the linked children the parent's `Start` declared, and links
    /// them -- the transcription of upstream's `SetupLinkedDevices` plus the
    /// per-child tweaks its `DeviceLinkCallback` applies.
    fn start_links(&mut self) {
        if self.dev.link_dev_count == 0 || self.dev.link_devs.is_null() {
            return;
        }
        // SAFETY: `SndEmu_Start` filled `link_devs` with `link_dev_count`
        // entries, owned by us until `SndEmu_FreeDevLinkData`.
        let declared = unsafe {
            std::slice::from_raw_parts(
                self.dev.link_devs.cast::<DevLinkInfo>(),
                self.dev.link_dev_count as usize,
            )
        };
        // SAFETY: a live parent device.
        let link_device = unsafe { (*self.dev.dev_def).link_device };
        let Some(link_device) = link_device else {
            return;
        };

        for declaration in declared {
            if declaration.cfg.is_null() {
                continue;
            }
            // The child's core and header bytes, as upstream's link callback
            // sets them: EMU2149 for an OPN's SSG (with the header's per-chip SSG
            // flags), adlibemu for the OPL4's FM half.
            //
            // SAFETY: `cfg` points at a config the parent allocated for us to
            // adjust -- its documented purpose.
            unsafe {
                match declaration.dev_id {
                    ffi::DEVID_AY8910 => {
                        (*declaration.cfg).emu_core = ffi::FCC_EMU_;
                        let ay = declaration.cfg.cast::<Ay8910Cfg>();
                        match self.spec.kind {
                            ChipKind::Ym2203 => {
                                (*ay).chip_flags = self.settings.ym2203_ay_flags;
                            }
                            ChipKind::Ym2608 => {
                                (*ay).chip_flags = self.settings.ym2608_ay_flags;
                            }
                            _ => {}
                        }
                    }
                    ffi::DEVID_YMF262 => {
                        (*declaration.cfg).emu_core = ffi::FCC_ADLE;
                    }
                    _ => {}
                }
            }

            let mut child = DevInfo::empty();
            // SAFETY: the parent-allocated cfg outlives the call; `child` is a
            // valid out-param.
            let started =
                unsafe { ffi::SndEmu_Start(declaration.dev_id, declaration.cfg, &raw mut child) };
            if started != EERR_OK || child.data_ptr.is_null() || child.dev_def.is_null() {
                log::warn!(
                    "libvgm's {} refused its linked device {:#04x}: error {started:#04x}",
                    self.spec.kind.name(),
                    declaration.dev_id,
                );
                continue;
            }
            // SAFETY: both devices are live; this is upstream's
            // `LinkDevice(parent, linkID, &child)` call.
            unsafe { link_device(self.dev.data_ptr, declaration.link_id, &raw const child) };

            // The reference pushes the AY option bits to an OPN's linked SSG
            // as well as to a standalone AY8910 -- the PCM3CH detection reads
            // the same on both.
            if declaration.dev_id == ffi::DEVID_AY8910 {
                let bits = default_option_bits(ChipKind::Ay8910);
                // SAFETY: a live child device from the start above.
                unsafe {
                    if let Some(set_options) = (*child.dev_def).set_option_bits {
                        set_options(child.data_ptr, bits);
                    }
                }
            }

            // The stereo mask function lives on the SSG child for the OPN
            // family -- `Cmd_AY_Stereo` fetches it from the linked device.
            // Upstream invokes it with the *parent's* data pointer, which reads
            // as a bug; the child's own pointer is what the core expects, so we
            // use that.
            if declaration.dev_id == ffi::DEVID_AY8910 && self.writers.stereo.is_none() {
                // SAFETY: a live child device definition.
                self.writers.stereo = unsafe {
                    ffi::device_func(
                        child.dev_def,
                        RWF_REGISTER | RWF_WRITE,
                        ffi::DEVRW_ALL,
                        ffi::USER_STEREO_MASK,
                    )
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteOptMask>(p))
                };
            }

            let child_rate = child.sample_rate.max(1);
            let parent_rate = self.dev.sample_rate.max(1);
            self.links.push(LinkedDev {
                dev: child,
                dev_id: declaration.dev_id,
                gain: link_gain(self.spec.kind),
                step: (u64::from(child_rate) << 32) / u64::from(parent_rate),
                pos: 0,
                last: [0, 0],
                left: Vec::new(),
                right: Vec::new(),
            });
        }
    }
}

/// The sample-rate mode a chip's device starts in, mirroring the pinned
/// reference's `ChipSmplMode = 3` (playcfg's `ConvertChipSmplModeOption`):
/// native rate for the ten FM chips -- their aliasing is part of the sound --
/// and `max(native, 44100)` for everything else, so a low-rate core (the
/// WonderSwan's 24 kHz) synthesises on the output grid as the reference does
/// instead of being band-limited by the resampler at its native rate.
const fn sr_mode(kind: ChipKind) -> u8 {
    match kind {
        ChipKind::Ym3526
        | ChipKind::Y8950
        | ChipKind::Ym3812
        | ChipKind::Ym2413
        | ChipKind::Ymf262
        | ChipKind::Ym2151
        | ChipKind::Ym2203
        | ChipKind::Ym2608
        | ChipKind::Ym2610
        | ChipKind::Ym2612 => ffi::DEVRI_SRMODE_NATIVE,
        _ => ffi::DEVRI_SRMODE_HIGHEST,
    }
}

/// The option bits a device starts with: the per-chip defaults plus what the
/// header's variant flag adds.
///
/// The YM2612's GPGX and Gens starts ignore `cfg->flags` entirely -- the
/// reference pushes the YM3438 mode through `SetOptionBits`
/// (`OPT_YM2612_TYPE_OPN2C_ASIC`, 0x10) when clock bit 31 is set, and so does
/// this. Each core reads the bits it knows (Gens has no type bit and ignores
/// it, exactly as under the reference). The reference's other YM2612 arm --
/// the Project2612 legacy-mode fix (`OPT_YM2612_LEGACY_MODE` for v<=1.50
/// single-YM2612 files, cleared at the first render) -- is deliberately not
/// ported: it needs a file-level fact and a render hook this layer lacks, it
/// exists for one archive's old trims, and the default Nuked row never
/// consults it.
const fn start_option_bits(kind: ChipKind, variant: bool) -> u32 {
    let variant_bits = match kind {
        ChipKind::Ym2612 if variant => 0x10,
        _ => 0,
    };
    default_option_bits(kind) | variant_bits
}

/// The option bits VGMPlay applies to a chip before playing anything, from its
/// `InitDevOptions` defaults, so the cores run in the reference's mode.
const fn default_option_bits(kind: ChipKind) -> u32 {
    match kind {
        // The pinned reference's NSFPlay options: SharedOpts 0x03, APUOpts
        // 0x01, DMCOpts 0x3B assemble to 0x3B7. One bit above libvgm's own
        // 0x1B7 default: bit 9, `OPT_TRI_NULL`, drains a halted triangle
        // channel to the null level instead of freezing it mid-step -- without
        // it every triangle stop leaves a DC pedestal and a click, and shifts
        // the nonlinear mixer's operating point while held.
        ChipKind::NesApu => 0x03B7,
        // `OPT_AY8910_PCM3CH_DETECT`, on by default in both the reference's
        // playcfg and libvgm's own player: 3-channel PCM songs (Atari ST
        // style) drop per-channel panning so the correlated channels sum
        // cleanly. Identical output at centre pans; audible once our chip
        // mixer pans channels apart.
        ChipKind::Ay8910 => 0x01,
        // `OPT_SCSP_BYPASS_DSP`: the DSP is skipped by default upstream.
        ChipKind::Scsp => 0x01,
        // `OPT_GB_DMG_LEGACY_MODE`: VGMPlay's `playcfg` defaults `LegacyMode`
        // on, which reloads a channel's length counter on every trigger and
        // forces the wave channel on at `NR30` -- the "VGM log fix" that
        // `vgm_cmp`-optimised rips rely on, having had their redundant length
        // re-writes stripped. Both GB cores start with it off, so without this
        // those rips cut notes short or lose the wave channel.
        ChipKind::GameBoyDmg => 0x80,
        // `OPT_MSM6258_FORCE_12BIT`: the pinned reference sets `Enable10Bit =
        // False`, which widens the DAC to full 12-bit precision regardless of
        // the header's 10-bit default. Left unset, every sample loses its low
        // two bits against the reference.
        ChipKind::Okim6258 => 0x01,
        _ => 0,
    }
}

/// Byte-swaps C219 sample ROM into the order its shared C140 core expects.
///
/// The core copies ROM verbatim (`c219_write_rom` is a `memcpy`) and reads it in
/// 16-bit units, so the player must swap each pair. Mirrors upstream's
/// `Cmd_DataBlock` C219 case (`chipType == 0x1C && flags & 0x01`), including its
/// `dataLen &= ~0x01`: an odd trailing byte is dropped, which `chunks_exact`
/// does for free.
///
/// The fresh `Vec` is a deliberate cold-path choice: this runs on a data-block
/// load (file load, and a loop wrap that replays ROM blocks), never per sample,
/// so a reused scratch field would trade a standing per-chip ROM-sized buffer
/// for a saving too small to measure.
fn c219_byteswap(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() & !1);
    for pair in data.chunks_exact(2) {
        out.push(pair[1]);
        out.push(pair[0]);
    }
    out
}

/// Splits a canonical channel mute mask into what the parent device mutes and,
/// for the OPN family, what its linked SSG child mutes.
///
/// [`vgms_core::vgm::channels_of`] puts the SSG channels last; libvgm's OPN
/// parent mutes only its FM (and, on the 2608/2610, the rhythm/ADPCM that share
/// its device), while the SSG is a separate `DEVID_AY8910` with its own three-bit
/// mask. Every other chip is identity. The bit positions are pinned by the
/// parent cores' `set_mute_mask` (`fmopn.c`) and the canonical channel table.
const fn split_mute(kind: ChipKind, mask: u32) -> (u32, Option<u32>) {
    match kind {
        // FM 1-3 (bits 0-2), then SSG A-C (bits 3-5).
        ChipKind::Ym2203 => (mask & 0b111, Some((mask >> 3) & 0b111)),
        // FM 1-6, ADPCM-A/rhythm 1-6, ADPCM-B (bits 0-12), then SSG A-C
        // (bits 13-15).
        ChipKind::Ym2608 | ChipKind::Ym2610 => (mask & 0x1FFF, Some((mask >> 13) & 0b111)),
        _ => (mask, None),
    }
}

/// A linked child's level relative to its parent, 8.8 fixed point.
///
/// Upstream's `GetChipVolume(..., isLinked=1)`: the YM2203's SSG plays at half
/// the FM's volume; every other link at parity.
const fn link_gain(kind: ChipKind) -> u16 {
    match kind {
        ChipKind::Ym2203 => 0x80,
        _ => 0x100,
    }
}

impl Drop for LibVgmChip {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ChipCore for LibVgmChip {
    /// Restarts at `clock`, because libvgm reads the clock at construction and
    /// derives the sample rate from it.
    fn reset(&mut self, clock: u32, variant: bool) {
        self.clock = clock;
        self.variant = variant;
        // A reset discards the header settings too: `configure` always follows
        // it, and carrying the previous file's noise taps into the gap would be
        // a silent bug.
        self.settings = ChipSettings::default();
        // ...and the RF5C bank: the device restart clears the core's own.
        self.ram_bank = 0;
        // ...and the QSound key-on caches, which describe the previous song.
        self.qsound_start = [0; 16];
        self.qsound_pitch = [0; 16];
        self.start();
    }

    /// Restarts again, now that the header's chip settings are known.
    ///
    /// libvgm wants them at construction and our engine delivers them after
    /// reset, so the second start reconciles the two orders. It happens before
    /// any register write, so nothing is lost.
    fn configure(&mut self, settings: &ChipSettings) {
        self.settings = *settings;
        self.start();
    }

    fn native_rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed).max(1)
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        if !self.is_started() {
            return;
        }
        // Upstream's `Cmd_RF5C_Reg` bank patch: the player watches the RF5C
        // control register (7) in bank mode (bit 6 clear) and remembers the
        // bank, because RAM-write commands need it OR'd into their addresses
        // (`DoRAMOfsPatches`) -- see [`write_ram_absolute`](Self::write_ram_absolute).
        if matches!(self.spec.kind, ChipKind::Rf5c68 | ChipKind::Rf5c164)
            && port == 0
            && addr == 0x07
            && data & 0x40 == 0
        {
            self.ram_bank = u32::from(data & 0x0F) << 12;
        }
        // Upstream's `Cmd_QSound_Reg` hacks, gated by `chipDev.flags & 0x01`
        // (our `qsound_hacks`): an old (4 MHz clock) QSound log on a core other
        // than MAME. Channel registers sit below 0x80, eight per channel:
        // 1 = start address, 2 = pitch, 3 = phase. The old HLE keyed a note on
        // at a pitch rising from zero or a phase write, and `vgm_cmp` stripped
        // the address rewrites those logs relied on, so the cached start
        // address is injected back exactly where the reference injects it.
        if self.qsound_hacks && addr < 0x80 {
            let chn = usize::from(addr >> 3);
            match addr & 0x07 {
                0x01 => self.qsound_start[chn] = data,
                0x02 => {
                    if self.qsound_pitch[chn] == 0 && data != 0 {
                        self.qsound_key_on(addr, self.qsound_start[chn]);
                    }
                    self.qsound_pitch[chn] = data;
                }
                0x03 => {
                    self.qsound_key_on(addr, self.qsound_start[chn]);
                }
                _ => {}
            }
        }
        self.put_bus(fold(self.spec.write, port, addr, data));
    }

    /// Hands a sample ROM block to the memory space its block type names.
    ///
    /// Mirrors upstream's `WriteChipROM`: declare the full image size first,
    /// then write the piece. Both are needed and in that order -- a core sizes
    /// its buffer on the first call, and a block written before the size is a
    /// block written into nothing.
    fn load_rom(&mut self, block_type: u8, total_size: u32, start: u32, data: &[u8]) {
        if !self.is_started() {
            return;
        }
        // The C219 (ASIC 219) shares the C140 core, which copies sample ROM
        // verbatim and relies on the player to have byte-swapped each 16-bit
        // sample first. We started the C219 device from the C140 header's type
        // byte, so the same `c140_type == 2` condition selects it here --
        // upstream's `Cmd_DataBlock` swaps on the equivalent per-device flag.
        let swapped;
        let data = if self.spec.kind == ChipKind::C140 && self.settings.c140_type == 2 {
            swapped = c219_byteswap(data);
            swapped.as_slice()
        } else {
            data
        };
        let space = usize::from(rom_space(block_type));
        // SAFETY: a live device; `data` is valid for its length and libvgm
        // copies out of it before returning.
        unsafe {
            if let Some(size) = self.writers.rom_size[space] {
                size(self.dev.data_ptr, total_size);
            }
            if let Some(write) = self.writers.rom_write[space]
                && !data.is_empty()
            {
                write(self.dev.data_ptr, start, data.len() as u32, data.as_ptr());
            }
        }
    }

    /// A RAM block, at a window-relative offset. Same path as
    /// [`write_ram_absolute`](Self::write_ram_absolute): upstream patches both
    /// the `0xC0` blocks and the `0x68` copies identically.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        self.write_ram_absolute(offset, data);
    }

    /// A RAM image: through the chip's `DEVRW_BLOCK` writer, with the RF5C
    /// pair's tracked bank OR'd into the address (upstream's
    /// `DoRAMOfsPatches`; the bank is zero for every other chip).
    ///
    /// The block writer, **not** the byte window, and the difference is the
    /// Lemmings (FM Towns) bug: `rf5c68_mem_w` masks its offset into the CPU's
    /// 4 KiB banked window (`offset &= 0x0FFF`), so a whole-RAM image looped
    /// through it folds onto one window and the channels -- which fetch
    /// *absolute* addresses -- play empty RAM. Upstream sends every RAM write
    /// command through `romWrite`, which each RAM chip files absolute; the
    /// byte loop stays only as a fallback for a core with no block writer.
    fn write_ram_absolute(&mut self, address: u32, data: &[u8]) {
        if !self.is_started() || data.is_empty() {
            return;
        }
        let address = address | self.ram_bank;
        if let Some(write) = self.writers.rom_write[0] {
            // SAFETY: a live device; libvgm copies out of `data`.
            unsafe { write(self.dev.data_ptr, address, data.len() as u32, data.as_ptr()) };
            return;
        }
        let Some(write) = self.writers.mem8 else {
            return;
        };
        for (index, &byte) in data.iter().enumerate() {
            let at = address.wrapping_add(index as u32);
            // SAFETY: a live device and its own memory writer.
            unsafe { write(self.dev.data_ptr, at as u16, byte) };
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        if !self.is_started() {
            out.fill(0);
            return;
        }

        // Grown, never shrunk: a worklet pulling 128 frames after an offline
        // render pulled 4096 should not reallocate.
        if self.left.len() < frames {
            self.left.resize(frames, 0);
            self.right.resize(frames, 0);
        }
        let mut planes = [self.left.as_mut_ptr(), self.right.as_mut_ptr()];

        // SAFETY: a live device; `Update` writes exactly `frames` samples into
        // each of the two planes, both of which are at least that long and
        // outlive the call.
        unsafe {
            let Some(update) = (*self.dev.dev_def).update else {
                out.fill(0);
                return;
            };
            update(self.dev.data_ptr, frames as u32, planes.as_mut_ptr());
        }

        // The linked children -- an OPN's SSG, the OPL4's FM -- render their own
        // streams at their own rates and are mixed in here, each through its
        // resampler and link gain. Taken out of `self` for the loop so the planes
        // can be borrowed alongside them.
        let mut links = std::mem::take(&mut self.links);
        for link in &mut links {
            if link.dev.data_ptr.is_null() {
                continue;
            }
            // SAFETY: a live child device.
            unsafe { link.mix_into(&mut self.left, &mut self.right, frames) };
        }
        self.links = links;

        for (frame, (&left, &right)) in out
            .chunks_exact_mut(2)
            .zip(self.left.iter().zip(self.right.iter()))
        {
            frame[0] = left;
            frame[1] = right;
        }
    }

    fn set_channel_mutes(&mut self, muted: u32) {
        self.mute_mask = muted;
        self.apply_muting();
    }

    fn set_channel_pans(&mut self, pans: &[i16]) {
        self.pans = pans.to_vec();
        self.apply_panning();
    }

    /// Whether the started core actually carries a pan function -- the
    /// ground truth the registry's `channel_pan` flag mirrors. `false` while
    /// stopped, since there is no device to ask.
    fn supports_pan(&self) -> bool {
        if !self.is_started() {
            return false;
        }
        // SAFETY: a live device definition; reading a nullable field.
        unsafe { (*self.dev.dev_def).set_panning.is_some() }
    }
}

/// Whether libvgm's *default* core for `kind` can place its channels in the
/// stereo image -- what the registry's `channel_pan` flag reports, so the UI
/// hides pan controls for the rest.
///
/// Four default cores carry a `SetPanning` function: the SN76489's Maxim core,
/// the AY8910's EMU2149, the NES APU's NSFPlay, and the YM2413's EMU2413.
/// `LibVgmChip::supports_pan` reads the started device's own field, so a wrong
/// answer here is only a hidden-or-shown control, never a dead knob.
pub(crate) const fn default_core_pans(kind: ChipKind) -> bool {
    matches!(
        kind,
        ChipKind::Sn76489 | ChipKind::Ay8910 | ChipKind::NesApu | ChipKind::Ym2413
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::specs::{SPECS, spec_for as spec};

    fn energy(out: &[i32]) -> i64 {
        out.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// The option bits VGMPlay applies by default reach the chips that need
    /// them. Pinned as values because a wrong bit is silent: the core simply
    /// runs in a mode the reference never uses.
    #[test]
    fn default_option_bits_match_the_reference_defaults() {
        // `OPT_GB_DMG_LEGACY_MODE` (gbintf.h) and `OPT_MSM6258_FORCE_12BIT`
        // (okim6258.h). Both cores clear these at `device_start` and neither
        // reset re-derives them, so the bit set here (before reset) survives.
        assert_eq!(default_option_bits(ChipKind::GameBoyDmg), 0x80);
        assert_eq!(default_option_bits(ChipKind::Okim6258), 0x01);
        // The pinned NES value is 0x3B7 (SharedOpts | APUOpts << 2 |
        // DMCOpts << 4), NOT libvgm's own 0x1B7 -- the one differing bit is
        // OPT_TRI_NULL. The AY carries the PCM3CH detection bit.
        assert_eq!(default_option_bits(ChipKind::NesApu), 0x03B7);
        assert_eq!(default_option_bits(ChipKind::Ay8910), 0x01);
        // Unchanged neighbours, so a future edit cannot drop them unnoticed.
        assert_eq!(default_option_bits(ChipKind::Scsp), 0x01);
        assert_eq!(default_option_bits(ChipKind::Ym2612), 0x00);
        // The header's bit-31 variant reaches the YM2612 rows as the YM3438
        // mode bit (the GPGX/Gens starts ignore cfg->flags, so SetOptionBits
        // is the only road, as the reference drives it).
        assert_eq!(start_option_bits(ChipKind::Ym2612, true), 0x10);
        assert_eq!(start_option_bits(ChipKind::Ym2612, false), 0x00);
        assert_eq!(
            start_option_bits(ChipKind::GameBoyDmg, true),
            0x80,
            "a variant means nothing to the other chips"
        );
    }

    /// The Y8950's two halves, end to end: the FM half keys a note on, and the
    /// ADPCM-B half plays a `0x88`-loaded sample ROM through the delta-T unit.
    /// The delta-T is the whole reason this chip is served from libvgm -- the
    /// adapter-tier cores have no sample unit and played this half as silence.
    #[test]
    fn the_y8950_plays_both_its_halves() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Y8950));
        chip.reset(3_579_545, false);
        chip.configure(&ChipSettings::default());
        assert!(chip.is_started(), "the Y8950 device must start");

        let mut quiet = vec![0i32; 8192];
        chip.render(&mut quiet);
        let at_rest = energy(&quiet);

        // FM: modulator + carrier levels, a period, and key-on (channel 0).
        for (reg, value) in [
            (0x20u16, 0x01u16),
            (0x23, 0x01),
            (0x40, 0x10),
            (0x43, 0x00),
            (0x60, 0xF0),
            (0x63, 0xF0),
            (0x80, 0x77),
            (0x83, 0x77),
            (0xA0, 0x98),
            (0xB0, 0x31),
        ] {
            chip.write(0, reg, value);
        }
        let mut fm = vec![0i32; 8192];
        chip.render(&mut fm);
        assert!(
            energy(&fm) > at_rest * 4 + 1000,
            "the FM half must sound after a key-on (rest {at_rest}, playing {})",
            energy(&fm)
        );

        // ADPCM-B: an 0x88 ROM lands through y8950_alloc_pcmrom/write_pcmrom,
        // and the delta-T plays it. Arbitrary bytes decode to audible noise.
        chip.write(0, 0xB0, 0x11); // key the FM note off again
        let rom: Vec<u8> = (0..0x10000u32).map(|at| (at * 37) as u8).collect();
        chip.load_rom(0x88, rom.len() as u32, 0, &rom);
        for (reg, value) in [
            (0x08u16, 0x01u16), // control 2: external memory is ROM type
            (0x09, 0x00),       // start address 0
            (0x0A, 0x00),
            (0x0B, 0xFF), // stop address: far end
            (0x0C, 0xFF),
            (0x10, 0xFF), // delta-N
            (0x11, 0x7F),
            (0x12, 0xFF), // level
            (0x07, 0xA0), // control 1: START | MEMDATA (play external memory)
        ] {
            chip.write(0, reg, value);
        }
        let mut adpcm = vec![0i32; 8192];
        chip.render(&mut adpcm);
        assert!(
            energy(&adpcm) > at_rest * 4 + 1000,
            "the delta-T must sound playing the loaded ROM (rest {at_rest}, \
             playing {})",
            energy(&adpcm)
        );
    }

    /// Non-FM chips run in HIGHEST mode: a core whose derived rate falls below
    /// 44100 synthesises at 44100 instead, as the reference's ChipSmplMode=3
    /// does. The WonderSwan is the audible case -- 24 kHz native at the stock
    /// 3.072 MHz clock, which cost the top octave through the resampler.
    #[test]
    fn a_low_rate_non_fm_chip_renders_on_the_output_grid() {
        let mut chip = LibVgmChip::new(spec(ChipKind::WonderSwan));
        chip.reset(3_072_000, false);
        assert_eq!(
            chip.native_rate(),
            44_100,
            "ws_audio honours SRATE_CUSTOM_HIGHEST"
        );
        // An FM chip stays native: its aliasing is part of the sound.
        let mut fm = LibVgmChip::new(spec(ChipKind::Ym2612));
        fm.reset(7_670_454, false);
        assert_eq!(fm.native_rate(), 7_670_454 / 144);
        // And a non-FM chip already above 44100 keeps its own rate: HIGHEST
        // only raises, never lowers (the YMZ280B derives clock/192 = 88200).
        let mut pcm = LibVgmChip::new(spec(ChipKind::Ymz280b));
        pcm.reset(16_934_400, false);
        assert_eq!(pcm.native_rate(), 16_934_400 / 192);
    }

    /// The C219 swap reverses each 16-bit pair and drops an odd trailing byte,
    /// exactly as upstream's `dataLen &= ~0x01` does.
    #[test]
    fn c219_byteswap_reverses_pairs_and_drops_the_odd_tail() {
        assert_eq!(
            c219_byteswap(&[0x11, 0x22, 0x33, 0x44]),
            [0x22, 0x11, 0x44, 0x33]
        );
        assert_eq!(c219_byteswap(&[0xAA, 0xBB, 0xCC]), [0xBB, 0xAA]);
        assert_eq!(c219_byteswap(&[0x01]), Vec::<u8>::new());
        assert_eq!(c219_byteswap(&[]), Vec::<u8>::new());
    }

    /// The QSound old-log key-on hack follows the *started* core: on for an
    /// old-clock DSP QSound, but off for the MAME row (whose HLE keys on
    /// itself, so the reference clears `chipDev.flags` for FCC_MAME) and off
    /// for a modern-clock QSound.
    #[test]
    fn the_qsound_key_on_hack_is_off_for_the_mame_core() {
        let mut dsp = LibVgmChip::new(spec(ChipKind::QSound));
        dsp.reset(4_000_000, false);
        dsp.configure(&ChipSettings::default());
        assert!(dsp.is_started(), "the DSP QSound must start");
        assert!(
            dsp.qsound_hacks,
            "an old-clock DSP QSound wants the injections"
        );

        let mame_spec = SPECS
            .iter()
            .find(|row| row.id == "qsound.libvgm-mame")
            .expect("the MAME QSound row");
        let mut mame = LibVgmChip::new(mame_spec);
        mame.reset(4_000_000, false);
        mame.configure(&ChipSettings::default());
        assert!(mame.is_started(), "the MAME QSound must start");
        assert!(
            !mame.qsound_hacks,
            "the MAME core keys on itself, so the hack is cleared for it"
        );

        let mut modern = LibVgmChip::new(spec(ChipKind::QSound));
        modern.reset(60_000_000, false);
        modern.configure(&ChipSettings::default());
        assert!(
            !modern.qsound_hacks,
            "a modern-clock QSound carries the DSP clock and needs no hacks"
        );

        // Drive the injection dispatch (put_bus) on the DSP core: cache a start
        // address, then a pitch rising from zero fires the key-on. It must
        // render without reading a dangling pointer.
        dsp.write(0, 0x01, 0x0040);
        dsp.write(0, 0x02, 0x0001);
        let mut out = vec![0i32; 512];
        dsp.render(&mut out);
    }

    /// Construction, native rate, writes and render, end to end through the
    /// `ChipCore` trait rather than the raw FFI.
    #[test]
    fn the_generic_binding_drives_a_chip_end_to_end() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        chip.configure(&ChipSettings::default());

        assert!(
            chip.native_rate() > 8_000,
            "rate {} looks wrong",
            chip.native_rate()
        );

        let mut quiet = vec![0i32; 4096];
        chip.render(&mut quiet);
        let at_rest = energy(&quiet);

        // The `0x50` command's byte, exactly as our decoder hands it over:
        // latch channel 0's period, its high bits, then un-attenuate it.
        chip.write(0, 0, 0x8E);
        chip.write(0, 0, 0x02);
        chip.write(0, 0, 0x90);

        let mut loud = vec![0i32; 4096];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > at_rest * 4 + 1000,
            "the chip must sound after a write (rest {at_rest}, playing {})",
            energy(&loud)
        );
    }

    /// A whole-RAM image lands at its absolute addresses and the channels
    /// sound -- the Lemmings (FM Towns) shape: one 34 KiB type-`0xC0` block at
    /// offset 0, channels starting at 0x7400.
    ///
    /// The regression this pins: RAM images used to loop through the byte-wide
    /// memory writer, whose window masks offsets to 4 KiB (`offset &= 0x0FFF`),
    /// so everything past the first window folded and the channels -- which
    /// fetch absolute addresses -- played empty RAM. Silence, with the play
    /// cursor advancing normally.
    #[test]
    fn an_rf5c68_ram_image_lands_absolute_and_sounds() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Rf5c68));
        chip.reset(8_000_000, false);
        chip.configure(&ChipSettings::default());
        assert!(chip.is_started());

        // A loud square well past the 4 KiB window; 0xFF avoided (the
        // end-of-sample marker), 0x80 too (the loop-to marker).
        let mut ram = vec![0u8; 0x9000];
        for (index, byte) in ram.iter_mut().enumerate() {
            *byte = if (index / 32) % 2 == 0 { 0x7E } else { 0xFE };
        }
        chip.write_ram(0, &ram);

        // The file's own driver sequence: sounding on, ch0 selected, envelope,
        // pan, start 0x7400, loop start, frequency, key-on.
        for (addr, data) in [
            (7u16, 0x88u16),
            (7, 0xC0),
            (0, 0x50),
            (1, 0x3D),
            (6, 0x74),
            (4, 0x02),
            (5, 0x80),
            (2, 0xDC),
            (3, 0x04),
            (8, 0xFE),
        ] {
            chip.write(0, addr, data);
        }
        let mut out = vec![0i32; 8192];
        chip.render(&mut out);
        assert!(
            energy(&out) > 0,
            "channels starting past the CPU window must find their samples"
        );
    }

    /// A RAM image sent after the driver selects a bank lands at that bank --
    /// upstream's `Cmd_RF5C_Reg` bank patch plus `DoRAMOfsPatches`, mirrored
    /// by the binding since the block writer takes absolute addresses.
    #[test]
    fn an_rf5c68_ram_image_follows_the_selected_bank() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Rf5c68));
        chip.reset(8_000_000, false);
        chip.configure(&ChipSettings::default());

        // Sounding on, bank 2 selected -- then the image at window offset 0,
        // which must land at 0x2000.
        chip.write(0, 7, 0x82);
        chip.write_ram(0, &[0x7E; 0x800]);

        // A channel playing from 0x2000: silence unless the image landed there.
        for (addr, data) in [
            (7u16, 0xC0u16),
            (0, 0x50),
            (1, 0x3D),
            (6, 0x20),
            (4, 0x00),
            (5, 0x20),
            (2, 0x00),
            (3, 0x04),
            (8, 0xFE),
        ] {
            chip.write(0, addr, data);
        }
        let mut out = vec![0i32; 4096];
        chip.render(&mut out);
        assert!(
            energy(&out) > 0,
            "the image must land at the bank the driver had selected"
        );
    }

    /// A chip that was never reset has no clock, so it never started -- and
    /// renders silence rather than reading a null `data_ptr`.
    #[test]
    fn an_unstarted_chip_is_silent_rather_than_unsound() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        assert_eq!(chip.native_rate(), 1, "no device, so no rate");

        let mut out = vec![7i32; 64];
        chip.render(&mut out);
        assert!(out.iter().all(|&s| s == 0));

        // And a write with nowhere to go is dropped, not dereferenced.
        chip.write(0, 0, 0x90);
    }

    /// Reset is a restart, and it really does discard state: a chip made loud,
    /// then reset, is quiet again.
    #[test]
    fn reset_discards_the_previous_devices_state() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        chip.write(0, 0, 0x8E);
        chip.write(0, 0, 0x02);
        chip.write(0, 0, 0x90);

        let mut loud = vec![0i32; 2048];
        chip.render(&mut loud);
        assert!(energy(&loud) > 1000, "sanity: it should be playing");

        chip.reset(3_579_545, false);
        let mut after = vec![0i32; 2048];
        chip.render(&mut after);
        assert!(
            energy(&after) * 4 < energy(&loud),
            "a reset chip should be far quieter than a playing one \
             (was {}, now {})",
            energy(&loud),
            energy(&after)
        );
    }

    /// The SN76489's tone 1: latch its period, and un-attenuate it. Factored
    /// so a mute test can replay it before and after a reset.
    fn play_sn_tone1(chip: &mut LibVgmChip) {
        chip.write(0, 0, 0x8E);
        chip.write(0, 0, 0x02);
        chip.write(0, 0, 0x90);
    }

    /// Muting a channel silences it, and the mask survives a reset -- a reset
    /// restarts the device, which clears the core's own mask, so the wrapper
    /// must restate it or a seek would un-mute what the user muted.
    #[test]
    fn muting_silences_a_channel_and_survives_a_reset() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        play_sn_tone1(&mut chip);
        let mut loud = vec![0i32; 4096];
        chip.render(&mut loud);
        assert!(energy(&loud) > 1000, "sanity: tone 1 should play");

        // Tone 1 is channel 0 of the canonical SN76489 order.
        chip.set_channel_mutes(0b0001);
        let mut muted = vec![0i32; 4096];
        chip.render(&mut muted);
        assert!(
            energy(&muted) * 8 < energy(&loud),
            "muting tone 1 should silence it (loud {}, muted {})",
            energy(&loud),
            energy(&muted)
        );

        chip.reset(3_579_545, false);
        play_sn_tone1(&mut chip);
        let mut after = vec![0i32; 4096];
        chip.render(&mut after);
        assert!(
            energy(&after) * 8 < energy(&loud),
            "the mute mask must survive a reset (loud {}, after {})",
            energy(&loud),
            energy(&after)
        );
    }

    /// The OPN family's SSG is a linked `DEVID_AY8910`, not part of the
    /// parent's mute mask -- so an SSG-channel mute must reach the child.
    #[test]
    fn muting_an_opn_ssg_channel_reaches_the_linked_device() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Ym2203));
        chip.reset(3_579_545, false);
        // SSG channel A: a tone, enabled in the mixer, full amplitude.
        chip.write(0, 0x00, 0xFE); // period fine
        chip.write(0, 0x01, 0x00); // period coarse
        chip.write(0, 0x07, 0x3E); // mixer: tone A on (bit 0 clear), rest off
        chip.write(0, 0x08, 0x0F); // channel A amplitude: full
        let mut loud = vec![0i32; 8192];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > 1000,
            "sanity: SSG A should play ({})",
            energy(&loud)
        );

        // SSG A is channel 3 (FM 1-3, then SSG A-C) in the canonical order.
        chip.set_channel_mutes(1 << 3);
        let mut muted = vec![0i32; 8192];
        chip.render(&mut muted);
        assert!(
            energy(&muted) * 8 < energy(&loud),
            "muting SSG A must reach the linked EMU2149 (loud {}, muted {})",
            energy(&loud),
            energy(&muted)
        );
    }

    /// Pan capability is the started core's own truth, and it matches what the
    /// registry advertises: the SN76489 (Maxim default), AY8910, NES APU and
    /// YM2413 default cores all carry a pan function.
    #[test]
    fn pan_capability_matches_the_default_core() {
        for kind in [
            ChipKind::Sn76489,
            ChipKind::Ay8910,
            ChipKind::NesApu,
            ChipKind::Ym2413,
        ] {
            let mut chip = LibVgmChip::new(spec(kind));
            chip.reset(3_579_545, false);
            assert!(
                chip.supports_pan(),
                "{}'s default libvgm core should pan",
                kind.name()
            );
            assert!(
                default_core_pans(kind),
                "{} should advertise pan in the registry",
                kind.name()
            );
        }
        // A chip with no pan function: the YM2612's Nuked-OPN2 has none, and
        // the registry agrees.
        let mut ym = LibVgmChip::new(spec(ChipKind::Ym2612));
        ym.reset(7_670_454, false);
        assert!(
            !ym.supports_pan(),
            "the YM2612's libvgm core has no pan function"
        );
        assert!(!default_core_pans(ChipKind::Ym2612));
    }

    /// The OPN mute split: the parent takes its FM (and ADPCM), the SSG bits
    /// go to the child; every other chip is identity with no child.
    #[test]
    fn the_opn_mute_splits_fm_from_ssg() {
        // YM2203: FM bits 0-2, SSG bits 3-5.
        assert_eq!(
            split_mute(ChipKind::Ym2203, 0b000_111),
            (0b111, Some(0b000))
        );
        assert_eq!(
            split_mute(ChipKind::Ym2203, 0b111_000),
            (0b000, Some(0b111))
        );
        // YM2608 / YM2610: parent bits 0-12, SSG bits 13-15.
        assert_eq!(split_mute(ChipKind::Ym2610, 0x1FFF), (0x1FFF, Some(0)));
        assert_eq!(split_mute(ChipKind::Ym2608, 0x7 << 13), (0, Some(0b111)));
        // Everything else is identity, no child.
        assert_eq!(split_mute(ChipKind::Sn76489, 0b1010), (0b1010, None));
        assert_eq!(split_mute(ChipKind::Ym2612, 0x7F), (0x7F, None));
    }

    /// `native_rate` reports what the core will *actually* render at, which is
    /// not always derived from the clock.
    ///
    /// Some cores ignore `srMode` and always use `smplRate`, and Maxim's SN76489
    /// is one: asked for native mode it still answers [`REQUESTED_RATE`]. Worth
    /// pinning because the obvious assumption (rate follows clock, as ymfm's
    /// does) is false, and code written on it would drift pitch.
    #[test]
    fn the_rate_is_whatever_the_core_will_really_render_at() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
        chip.reset(3_579_545, false);
        let slow = chip.native_rate();
        chip.reset(3_579_545 * 2, false);
        let fast = chip.native_rate();

        assert!(slow > 0 && fast > 0, "a started chip has a rate");
        assert_eq!(
            (slow, fast),
            (REQUESTED_RATE, REQUESTED_RATE),
            "Maxim's SN76489 ignores srMode and renders at the rate it was \
             asked for; if this ever starts following the clock, the core \
             changed and the parity row must be re-measured"
        );
    }

    /// And it keeps reporting it when the core *changes* its mind: the
    /// ES5503's output rate is `clock / 8 / (oscillators + 2)`, re-derived on
    /// every oscillator-enable write and announced through libvgm's
    /// sample-rate-change callback.
    ///
    /// The regression this pins: without the callback registered, the rate
    /// read at start (one oscillator, `clock / 24` -- ~298 kHz on a IIgs)
    /// stood for the whole file while a real rip runs all 32 oscillators at
    /// ~26 kHz, so everything played ~11x too fast. The parity harness read
    /// corr 0.0022 -- unrelated waveforms -- and the level row was
    /// unmeasurable until this held.
    #[test]
    fn the_es5503_rate_follows_the_oscillator_enable_register() {
        const IIGS_CLOCK: u32 = 7_159_090;
        let mut chip = LibVgmChip::new(spec(ChipKind::Es5503));
        chip.reset(IIGS_CLOCK, false);
        chip.configure(&ChipSettings::default());
        assert_eq!(
            chip.native_rate(),
            IIGS_CLOCK / 8 / 3,
            "at reset one oscillator is enabled, so the rate is clock/8/(1+2)"
        );

        // Register 0xE1 holds (oscillators - 1) * 2: 62 enables all 32.
        chip.write(0, 0xE1, 62);
        assert_eq!(
            chip.native_rate(),
            IIGS_CLOCK / 8 / 34,
            "all 32 oscillators enabled, so the rate is clock/8/(32+2)"
        );

        // A reset returns the core to one oscillator, and the rate with it.
        chip.reset(IIGS_CLOCK, false);
        assert_eq!(chip.native_rate(), IIGS_CLOCK / 8 / 3);
    }

    /// Rendering repeatedly must not depend on how the caller chunks it: the
    /// engine relies on a 128-frame worklet pull sounding identical to a
    /// 4096-frame offline render.
    #[test]
    fn chunking_the_render_does_not_change_it() {
        let play = |chunk: usize| -> Vec<i32> {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            chip.write(0, 0, 0x8E);
            chip.write(0, 0, 0x02);
            chip.write(0, 0, 0x90);
            let mut out = vec![0i32; 4096];
            for block in out.chunks_mut(chunk * 2) {
                chip.render(block);
            }
            out
        };
        assert_eq!(play(2048), play(128), "render must be chunk-independent");
    }

    /// The header's noise taps and shift width reach libvgm, and changing them
    /// changes the sound. Without this, `configure` could be a no-op and every
    /// test above would still pass.
    #[test]
    fn the_headers_noise_settings_reach_the_chip() {
        let noise_with = |feedback: u16, width: u8| -> Vec<i32> {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            chip.configure(&ChipSettings {
                sn76489_feedback: feedback,
                sn76489_shift_width: width,
                ..ChipSettings::default()
            });
            // Channel 3 is the noise channel: select white noise at the
            // fastest rate, then un-attenuate it.
            chip.write(0, 0, 0xE4);
            chip.write(0, 0, 0xF0);
            let mut out = vec![0i32; 8192];
            chip.render(&mut out);
            out
        };

        let ti = noise_with(0x0003, 15);
        let sega = noise_with(0x0009, 16);
        assert!(energy(&ti) > 1000, "the noise channel should sound");
        assert_ne!(
            ti, sega,
            "a 15-bit 0x0003 register and a 16-bit 0x0009 one must produce \
             different sequences -- equal output means `configure` never \
             reached the chip"
        );
    }

    /// A bare AY8910's register writes go through the IO-port latch, and only
    /// sound proves it: a direct write would re-latch the address instead of
    /// landing in R8, leaving the chip silent. The clock, type and flags are an
    /// Atari ST YM2149's.
    #[test]
    fn a_bare_ay8910_actually_sounds() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Ay8910));
        chip.reset(2_005_311, false);
        let settings = ChipSettings {
            ay8910_type: 0x10,  // YM2149
            ay8910_flags: 0x02, // single output
            ..ChipSettings::default()
        };
        chip.configure(&settings);
        assert!(chip.is_started(), "the AY8910 starts");

        let mut quiet = vec![0i32; 4096];
        chip.render(&mut quiet);
        let at_rest = energy(&quiet);

        // Channel A: a mid period, tone A alone in the mixer, full volume.
        chip.write(0, 0x00, 0x50); // fine period
        chip.write(0, 0x07, 0x3E); // mixer: tone A on, the rest off
        chip.write(0, 0x08, 0x0F); // channel A volume

        let mut loud = vec![0i32; 4096];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > at_rest * 4 + 1000,
            "the AY8910 must sound once a channel is keyed \
             (rest {at_rest}, playing {})",
            energy(&loud)
        );
    }

    /// The OPN family's SSG is a *linked* AY8910 device, and the link is only
    /// proven by sound: the register writes travel through the parent, the
    /// audio comes back through the child's own stream and the mixer.
    #[test]
    fn an_opn_chips_linked_ssg_actually_sounds() {
        let mut chip = LibVgmChip::new(spec(ChipKind::Ym2203));
        chip.reset(3_993_600, false);
        chip.configure(&ChipSettings::default());
        assert!(chip.is_started(), "the YM2203 starts");
        assert!(!chip.links.is_empty(), "and brings its SSG up with it");

        let mut quiet = vec![0i32; 4096];
        chip.render(&mut quiet);
        let at_rest = energy(&quiet);

        // SSG channel A: a mid period, tone A enabled, full volume -- the
        // registers live at the bottom of the OPN's first port.
        chip.write(0, 0x00, 0x50); // fine period
        chip.write(0, 0x07, 0x3E); // mixer: tone A on, the rest off
        chip.write(0, 0x08, 0x0F); // channel A volume

        let mut loud = vec![0i32; 4096];
        chip.render(&mut loud);
        assert!(
            energy(&loud) > at_rest * 4 + 1000,
            "the SSG must sound through the link (rest {at_rest}, playing {})",
            energy(&loud)
        );
    }

    /// Every spec's device is actually compiled in, so no row is silently
    /// silent.
    ///
    /// `build.rs`'s `ENABLED` and this table have to agree and cannot see each
    /// other; a spec whose device was left out starts nothing and plays silence.
    #[test]
    fn every_spec_can_actually_start() {
        for spec in SPECS {
            let mut chip = LibVgmChip::new(spec);
            chip.reset(4_000_000, false);
            assert!(
                chip.is_started(),
                "{} (device {:#04x}) did not start -- is it named in \
                 build.rs's ENABLED list?",
                spec.kind.name(),
                spec.device,
            );
            assert!(
                chip.writers.serves(spec.write),
                "{} has no writer for {:?}; its writes would be dropped",
                spec.kind.name(),
                spec.write,
            );
            assert!(chip.native_rate() > 0, "{} has no rate", spec.kind.name());
        }
    }

    /// A chip's several rows must each start a *different* libvgm core, or a
    /// picker entry is dead: choosing it changes the id in `vgmstudio.ini` but not
    /// a sample of the sound.
    ///
    /// The bug this pins: an alternate whose `emu_core` names the device's
    /// first-listed core resolves, via `SndEmu_StartCore`, to the very core
    /// `emu_core: 0` already takes. This reads the *started* `DEV_DEF::coreID`
    /// back rather than trusting the spec, so it measures what libvgm chose.
    #[test]
    fn every_alternate_row_starts_a_distinct_core() {
        // (kind, id, the core libvgm actually started), for every row.
        let mut started: Vec<(ChipKind, &'static str, u32)> = Vec::new();
        for spec in SPECS {
            let mut chip = LibVgmChip::new(spec);
            chip.reset(4_000_000, false);
            assert!(chip.is_started(), "{} did not start", spec.id);
            // SAFETY: a live device definition from the successful start above.
            let core_id = unsafe { (*chip.dev.dev_def).core_id };
            started.push((spec.kind, spec.id, core_id));
        }

        for (index, &(kind, id, core)) in started.iter().enumerate() {
            for &(other_kind, other_id, other_core) in &started[index + 1..] {
                if kind != other_kind {
                    continue;
                }
                assert_ne!(
                    core, other_core,
                    "{kind:?}: rows {id:?} and {other_id:?} both start libvgm \
                     core {core:#010x} -- one is a dead picker entry (an \
                     alternate that names the device's first-listed core \
                     resolves to the same emulator as `emu_core: 0`)"
                );
            }
        }
    }

    /// A ROM block reaches the space its type names, and the size is declared
    /// before the data -- both as upstream's `WriteChipROM` does.
    ///
    /// Nothing observable to assert against a real core, so this checks the
    /// routing decision (ours) and that delivery is survivable (libvgm's).
    #[test]
    fn rom_blocks_route_to_the_space_their_type_names() {
        assert_eq!(rom_space(0x82), 0, "YM2610 ADPCM-A is the first space");
        assert_eq!(rom_space(0x83), 1, "YM2610 ADPCM-B is the second");
        assert_eq!(rom_space(0x84), 0, "YMF278B ROM is the first");
        assert_eq!(rom_space(0x87), 1, "YMF278B RAM is the second");
        assert_eq!(rom_space(0x8E), 0, "K053260, like most, has one space");

        // And a real delivery does not fall over.
        let mut chip = LibVgmChip::new(spec(ChipKind::K053260));
        chip.reset(3_579_545, false);
        let rom = vec![0x40u8; 0x400];
        chip.load_rom(0x8E, rom.len() as u32, 0, &rom);
        let mut out = vec![0i32; 512];
        chip.render(&mut out);
    }

    /// Every chip in the table starts, takes its own rule's writes, and renders.
    ///
    /// Not an audibility assertion: most of these need a sample ROM and driver
    /// setup before they sound. What it catches is a rule whose writer is
    /// missing, a device that refuses its own registers, and a render that reads
    /// a dangling pointer.
    #[test]
    fn every_chip_takes_writes_and_renders() {
        for spec in SPECS {
            let mut chip = LibVgmChip::new(spec);
            chip.reset(4_000_000, false);
            chip.configure(&ChipSettings::default());
            for (port, addr, data) in [(0u8, 0x00u16, 0x00u16), (0, 0x01, 0xFF), (1, 0x10, 0x80)] {
                chip.write(port, addr, data);
            }
            let mut out = vec![0i32; 256];
            chip.render(&mut out);
            assert_eq!(out.len(), 256, "{} rendered", spec.kind.name());
        }
    }

    /// Every chip that takes RAM blocks files a block writer.
    ///
    /// [`LibVgmChip::write_ram_absolute`] writes images through `DEVRW_BLOCK`
    /// -- the absolute path -- and only falls back to the byte window when a
    /// core files none. The fallback must stay a fallback: the RF5C pair's
    /// byte window masks to 4 KiB, so an image through it folds and the file
    /// plays silence (the Lemmings FM Towns bug).
    #[test]
    fn every_ram_taking_chip_has_a_block_writer() {
        for kind in [
            ChipKind::Rf5c68,
            ChipKind::Rf5c164,
            ChipKind::NesApu,
            ChipKind::Scsp,
            ChipKind::Es5503,
        ] {
            let mut chip = LibVgmChip::new(spec(kind));
            chip.reset(8_000_000, false);
            assert!(chip.is_started(), "{} starts", kind.name());
            assert!(
                chip.writers.rom_write[0].is_some(),
                "{} takes RAM images, so its core must file a block writer",
                kind.name()
            );
        }
    }

    /// Dropping a started chip stops its device. Nothing observable proves the
    /// free, so this is a leak-check under a loop: a missing `Stop` shows up
    /// under a sanitiser or as unbounded growth.
    #[test]
    fn chips_can_be_built_and_dropped_repeatedly() {
        for _ in 0..64 {
            let mut chip = LibVgmChip::new(spec(ChipKind::Sn76489));
            chip.reset(3_579_545, false);
            let mut out = vec![0i32; 256];
            chip.render(&mut out);
        }
    }
}
