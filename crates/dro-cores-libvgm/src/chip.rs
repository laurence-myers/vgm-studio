// SPDX-License-Identifier: GPL-2.0-or-later
//! One wrapper, every chip: [`LibVgmChip`] over libvgm's uniform device API.
//!
//! This is what makes libvgm different in kind from the other providers. A
//! `DEV_DEF` is a vtable of `Start`/`Stop`/`Reset`/`Update` plus a table of
//! width-typed register writers, so the twenty lines below drive a QSound and
//! a SAA1099 alike. What varies per chip is *data* -- a [`ChipSpec`] row saying
//! which device ID, which writer width, and how our engine's
//! `(port, addr, data)` folds into that writer's arguments.
//!
//! # The two conventions that have to be reconciled
//!
//! **libvgm takes the clock at construction.** `DEV_GEN_CFG::clock` is read by
//! `Start`, and the sample rate falls out of it. Our [`ChipCore::reset`] hands
//! a clock to a chip that already exists. So `reset` here *restarts*: stop the
//! old device, start a new one. Same shape as `dro-cores-ymfm`, for the same
//! reason.
//!
//! **libvgm renders planar, our engine wants interleaved.** `Update` writes
//! `outputs[0]` and `outputs[1]` as two separate `INT32` runs and *overwrites*
//! rather than accumulating (upstream's own silent-path `memset` proves it), so
//! [`render`](ChipCore::render) keeps two scratch planes and weaves them.

use std::ffi::c_void;

use dro_core::vgm::{ChipKind, ChipSettings};
use dro_synth::chip::ChipCore;

use crate::ffi::{
    self, DevFuncWriteA8D8, DevFuncWriteA16D8, DevFuncWriteA16D16, DevFuncWriteBlock,
    DevFuncWriteMemSize, DevGenCfg, DevInfo, EERR_OK, RWF_MEMORY, RWF_REGISTER, RWF_WRITE,
    Sn76496Cfg,
};

/// The rate asked for in `DEV_GEN_CFG::smplRate`.
///
/// Nominally unused: we start every chip in `DEVRI_SRMODE_NATIVE` so it renders
/// at its own rate and `dro_synth::resample` does the conversion. But upstream
/// warns that *some cores ignore `srMode` and always use `smplRate`*, and
/// Maxim's SN76489 is one of them -- so for those chips this is not a fallback,
/// it is the rate they will run at.
///
/// 44100 because that is what the pinned parity reference renders at, so a
/// rate-fixed core measures against it with no resampler on either side. The
/// engine is unaffected either way: it resamples from whatever
/// [`native_rate`](ChipCore::native_rate) reports, and that is read back from
/// libvgm rather than assumed.
const REQUESTED_RATE: u32 = 44_100;

/// How a chip's `(port, addr, data)` reaches libvgm's register writer.
///
/// **Every variant is transcribed from a handler in libvgm's
/// `player/vgmplayer_cmdhandler.cpp`**, named in its doc comment, and the
/// mapping from our `(port, addr, data)` is the inverse of what
/// `dro_core::vgm::stream` did on the way in.
///
/// That inversion is the whole difficulty of this step. Our decoder
/// *normalises*: it folds the QSound's `0xC4` so the register and the 16-bit
/// value stop trading places, reads the C352's `0xE1` big-endian, splits the
/// `0xD3`/`0xD4` address across `port` and `addr`, and routes the RF chips'
/// memory pokes to port 1 so they cannot collide with their registers. libvgm's
/// cores expect the raw conventions those fixes were written to hide, so each
/// rule here puts one of them back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRule {
    /// `write8(addr, data)`. Upstream's `Cmd_SN76489`, `Cmd_GGStereo`,
    /// `Cmd_Ofs8_Data8` and `Cmd_Port_Ofs8_Data8`.
    ///
    /// The SN76489's two commands are why `addr` is passed through rather than
    /// forced to zero: our decoder gives `0x50` an address of 0 and `0x4F`
    /// (Game Gear stereo) an address of 1, which is exactly libvgm's
    /// `SN76496_W_REG` and `SN76496_W_GGST`.
    Register,
    /// The Yamaha address/data latch pair, on `port`: `write8((port<<1)|0, reg)`
    /// then `write8((port<<1)|1, data)`. Upstream's `SendYMCommand`, reached
    /// from `Cmd_Reg8_Data8`, `Cmd_CPort_Reg8_Data8`, `Cmd_DReg8_Data8` and
    /// `Cmd_Port_Reg8_Data8`.
    ///
    /// **Two writes, not one**, and getting that wrong is the classic silent
    /// core: the chip latches a register number and never receives a value.
    RegisterLatch,
    /// `writeM8(addr, data)` -- the *memory* space, not the register file.
    /// Upstream's `Cmd_RF5C_Mem` and `Cmd_Ofs16_Data8` for the chips whose
    /// address arrives whole (`0xC5`-`0xC8`: SCSP, WonderSwan RAM, VSU,
    /// X1-010).
    ///
    /// This is where our port-1 convention is undone. `dro_core` routes
    /// `0xC1`/`0xC2` to port 1 precisely so a RAM poke cannot be mistaken for a
    /// register write; here the two are different *functions*, so the port has
    /// done its job and is dropped.
    Memory,
    /// `writeM8((port << 8) | addr, data)`. Upstream's `Cmd_Ofs16_Data8` for
    /// `0xD3`/`0xD4` (K054539, C140), where the 16-bit offset arrives split:
    /// our decoder put the high seven bits in `port` and the low eight in
    /// `addr`, so they are recombined here.
    MemoryPortHigh,
    /// `writeM16(addr, data)` -- fetched with `RWF_REGISTER`, despite upstream's
    /// field name. `Cmd_Ofs16_Data16`, which is the C352's `0xE1` alone.
    ///
    /// Our decoder already reads both operands big-endian (the corpus
    /// arbitrated it: register addresses only land on the voice-times-eight
    /// grid under that reading), which is what libvgm's `ReadBE16` does, so
    /// both values pass straight through.
    RegisterAddr16Data16,
    /// The QSound's three writes: data MSB to `0x00`, data LSB to `0x01`, then
    /// the register to `0x02`. Upstream's `Cmd_QSound_Reg`.
    ///
    /// The sharpest inversion in the table. Our decoder normalised `0xC4` so
    /// that `addr` is the register and `data` the 16-bit value -- the opposite
    /// of every other command in its range -- and this splits them back apart
    /// in the order the chip's own bus expects. **The register goes last**: it
    /// is the write that commits the pair.
    QSound,
    /// Port 0 is a register write, port 1 a memory poke: `write8(addr, data)`
    /// or `writeM8(addr, data)`. Upstream's `Cmd_RF5C_Reg` (`0xB0`/`0xB1`) and
    /// `Cmd_RF5C_Mem` (`0xC1`/`0xC2`), which are two commands into one chip.
    ///
    /// **The one rule that reads `port` as meaning rather than as address.**
    /// The RF5C68's register file and its 4 KiB RAM window overlap as far as a
    /// core is concerned, so `dro_core` puts registers on port 0 and memory on
    /// port 1 to keep them apart -- a convention of ours, not the format's.
    /// Here the two are different libvgm functions, so the port chooses which.
    RegisterOrMemoryByPort,
}

/// Exactly what a [`WriteRule`] decided to put on libvgm's bus.
///
/// The point of naming this rather than calling straight through: **a fold is
/// testable and an FFI call is not.** `LIBVGM-PLAN` lv-3 asks for "a unit test
/// per entry asserting the bytes that reach libvgm", and with the decision
/// separated from the call, that is an ordinary `assert_eq!` on a value instead
/// of an attempt to intercept a C function pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bus {
    /// One `write8(addr, data)`.
    Reg8(u8, u8),
    /// Two, in order -- the Yamaha address/data latch.
    Reg8Pair((u8, u8), (u8, u8)),
    /// Three, in order -- the QSound's MSB, LSB, register.
    Reg8Triple((u8, u8), (u8, u8), (u8, u8)),
    /// One `writeM8(addr, data)`.
    Mem8(u16, u8),
    /// One `writeM16(addr, data)`, fetched as a register writer.
    Reg16(u16, u16),
}

/// Turns our engine's `(port, addr, data)` into the bytes libvgm expects.
///
/// Pure, total and `const`: no chip, no pointers, nothing to mock. Every arm is
/// the inverse of a normalisation `dro_core::vgm::stream` applied on the way
/// in, and each is pinned by a test naming the upstream handler it mirrors.
const fn fold(rule: WriteRule, port: u8, addr: u16, data: u16) -> Bus {
    match rule {
        WriteRule::Register => Bus::Reg8(addr as u8, data as u8),
        WriteRule::RegisterLatch => {
            Bus::Reg8Pair(((port << 1), addr as u8), ((port << 1) | 1, data as u8))
        }
        WriteRule::Memory => Bus::Mem8(addr, data as u8),
        WriteRule::MemoryPortHigh => Bus::Mem8((port as u16) << 8 | (addr & 0xFF), data as u8),
        WriteRule::RegisterAddr16Data16 => Bus::Reg16(addr, data),
        WriteRule::QSound => Bus::Reg8Triple(
            (0x00, (data >> 8) as u8),
            (0x01, (data & 0xFF) as u8),
            (0x02, addr as u8),
        ),
        WriteRule::RegisterOrMemoryByPort => {
            if port == 0 {
                Bus::Reg8(addr as u8, data as u8)
            } else {
                Bus::Mem8(addr, data as u8)
            }
        }
    }
}

/// One chip: which libvgm device it is, and how to talk to it.
///
/// A `&'static` table row rather than a trait object, because everything here
/// is data and the alternative is a virtual call per register write.
#[derive(Debug)]
pub(crate) struct ChipSpec {
    /// The registry id, `"<chip slug>.libvgm"`. Written out rather than
    /// composed at runtime because [`CoreInfo::id`](dro_synth::CoreInfo::id) is
    /// a `&'static str` that lands in `drotrim.ini`.
    pub(crate) id: &'static str,
    /// Our engine's name for the chip -- what the registry keys on.
    pub(crate) kind: ChipKind,
    /// libvgm's `DEVID_` constant.
    pub(crate) device: u8,
    /// A four-character code from `EmuCores.h`, or 0 for the device's default
    /// core. lv-6 publishes the alternatives as picker entries; until then
    /// every row takes the default.
    pub(crate) emu_core: u32,
    /// How writes fold.
    pub(crate) write: WriteRule,
    /// The `user` selector for each of the chip's two sample-memory spaces.
    ///
    /// libvgm files a chip's ROM writers by `user`, and the value is per chip:
    /// `0` for most, `'A'`/`'B'` for the YM2610's two ADPCM spaces, `"RO"`/`"RA"`
    /// for the YMF278B's ROM and RAM. Taken from the `SndEmu_GetDeviceFunc`
    /// calls in `player/vgmplayer.cpp`'s device-setup switch; a chip with one
    /// space repeats it, and a chip with none never reaches these.
    pub(crate) rom_spaces: [u16; 2],
    /// Fills in the chip-specific half of the configuration from the VGM
    /// header, if it has one.
    ///
    /// Called with the config already carrying clock, sample-rate mode and the
    /// variant flag, so an implementation only sets what is its own.
    pub(crate) configure: fn(&mut DevConfig, &ChipSettings),
    /// Builds this chip, boxed, for the registry.
    ///
    /// The registry takes a bare `fn` pointer, which cannot capture a spec --
    /// so [`chip_specs!`] emits one of these per row, each naming its own
    /// [`ChipKind`]. That is the whole reason the macro exists.
    pub(crate) make: fn() -> Box<dyn ChipCore>,
}

impl ChipSpec {
    /// This chip's registry id.
    #[must_use]
    pub(crate) const fn registry_id(&self) -> &'static str {
        self.id
    }

    /// How the registry builds it.
    #[must_use]
    pub(crate) const fn maker(&self) -> dro_synth::CoreMaker {
        dro_synth::CoreMaker::Generic(self.make)
    }
}

/// The configuration handed to `SndEmu_Start`.
///
/// libvgm's chips with settings define a struct whose first member is a
/// `DEV_GEN_CFG` and pass a pointer to it cast down. Modelling that as an enum
/// rather than a byte buffer keeps the field access type-checked; the cast at
/// [`as_ptr`](Self::as_ptr) is the same one upstream's own `emutest.c` makes,
/// and `layout.rs` pins the prefix property it relies on.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DevConfig {
    /// A chip whose configuration is only the generic fields.
    Generic(DevGenCfg),
    /// The SN76496 family, whose noise taps and shift-register width decide
    /// which of a dozen parts it actually is.
    Sn76496(Sn76496Cfg),
}

impl DevConfig {
    /// The generic half, which every variant has and every start reads.
    fn generic_mut(&mut self) -> &mut DevGenCfg {
        match self {
            Self::Generic(cfg) => cfg,
            Self::Sn76496(cfg) => &mut cfg.gen_cfg,
        }
    }

    /// A pointer `SndEmu_Start` can read, whatever the real struct is.
    fn as_ptr(&self) -> *const DevGenCfg {
        match self {
            Self::Generic(cfg) => std::ptr::from_ref(cfg),
            Self::Sn76496(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
        }
    }
}

/// Every entry point a chip might use, fetched once at start.
///
/// libvgm files its writers by `(funcType, rwType, user)` and the *combination*
/// is the signature contract -- a pointer filed under `DEVRW_A8D8` takes
/// `(void*, UINT8, UINT8)` and nothing else. Fetching the whole set here rather
/// than only the one a rule needs costs a handful of table scans per chip
/// construction and means [`WriteRule`] can stay a pure description of a fold.
///
/// The names mirror `player/vgmplayer.cpp`'s `CHIP_DEVICE` fields so the two
/// can be read side by side. Its `writeD16` (`RWF_REGISTER | DEVRW_A8D16`) has
/// no field here yet: the only chips that use it are the ES5506 and the 32X
/// PWM, neither of which lv-3 enables, and a fetched-but-uncalled pointer is a
/// guess about a chip nobody has measured. It arrives with the ES5506 at lv-5. **Note `reg16` in particular**: upstream calls it
/// `writeM16` and fetches it with `RWF_REGISTER`, not `RWF_MEMORY`. Copying the
/// name without reading the fetch would have put the C352's writes into the
/// wrong space.
#[derive(Debug, Clone, Copy, Default)]
struct Writers {
    /// `write8`: `RWF_REGISTER | DEVRW_A8D8`.
    reg8: Option<DevFuncWriteA8D8>,
    /// `writeM16`: `RWF_REGISTER | DEVRW_A16D16` -- register, despite the name.
    reg16: Option<DevFuncWriteA16D16>,
    /// `writeM8`: `RWF_MEMORY | DEVRW_A16D8`.
    mem8: Option<DevFuncWriteA16D8>,
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
                reg16: reg(ffi::DEVRW_A16D16, 0)
                    .map(|p| std::mem::transmute::<*mut c_void, DevFuncWriteA16D16>(p)),
                // **Memory first, then register** -- the space a chip files
                // its 16-bit-address writer under is per chip, not per width.
                // Upstream's player fetches this same field with `RWF_MEMORY`
                // for the RF5C pair and with `RWF_REGISTER` for others, and
                // SegaPCM is the case that proves it: it exposes only
                // `RWF_REGISTER | DEVRW_A16D8`, so asking the memory space
                // alone finds nothing and every one of its writes is dropped.
                // A chip exposes the width in exactly one space, so trying
                // both cannot pick the wrong one.
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
    /// A missing one is not fatal -- writes are dropped and the chip is silent
    /// -- but it is the single most likely symptom of a wrong table row, so it
    /// is worth saying out loud at start rather than leaving as "no sound".
    const fn serves(&self, rule: WriteRule) -> bool {
        match rule {
            WriteRule::Register | WriteRule::RegisterLatch | WriteRule::QSound => {
                self.reg8.is_some()
            }
            WriteRule::Memory | WriteRule::MemoryPortHigh => self.mem8.is_some(),
            WriteRule::RegisterAddr16Data16 => self.reg16.is_some(),
            // Needs both, and a chip missing either is half-mute.
            WriteRule::RegisterOrMemoryByPort => self.reg8.is_some() && self.mem8.is_some(),
        }
    }
}

/// Which of a chip's two sample-memory spaces a VGM data-block type names.
///
/// Transcribed from upstream's `_VGM_ROM_CHIPS` table, whose second column is
/// exactly this. Only three block types name the second space, and every one of
/// them is a chip with two genuinely different memories -- so the default is
/// the first space and the exceptions are listed rather than computed.
const fn rom_space(block_type: u8) -> u8 {
    match block_type {
        // The YM2610's ADPCM-B (Delta-T), beside `0x82`'s ADPCM-A.
        0x83 => 1,
        // The YMF278B's RAM, beside `0x84`'s ROM.
        0x87 => 1,
        _ => 0,
    }
}

/// One libvgm chip, owned.
pub struct LibVgmChip {
    spec: &'static ChipSpec,
    /// Zeroed while stopped; `data_ptr` non-null means started.
    dev: DevInfo,
    writers: Writers,
    /// What the last [`reset`](ChipCore::reset) asked for, kept so
    /// [`configure`](ChipCore::configure) can restart at the same clock.
    clock: u32,
    variant: bool,
    settings: ChipSettings,
    /// The two planes `Update` writes, grown as needed and never shrunk.
    left: Vec<i32>,
    right: Vec<i32>,
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

// SAFETY: the device is exclusively owned -- the handle is never cloned or
// handed out -- and the cores this crate compiles hold no mutable file-scope
// state, so all of a chip's mutation is behind `data_ptr`. That was checked
// against the pinned tree rather than assumed, and it is a **per-core**
// property: a core added to `build.rs`'s ENABLED list must be checked for
// mutable globals before it is trusted here. Not `Sync`: two threads must not
// write one chip at once.
unsafe impl Send for LibVgmChip {}

impl LibVgmChip {
    /// A chip built to `spec`, not yet started.
    ///
    /// Starting waits for [`reset`](ChipCore::reset), which is what supplies
    /// the clock -- and the clock is a construction parameter to libvgm, so
    /// there is nothing to build before it arrives.
    #[must_use]
    pub(crate) fn new(spec: &'static ChipSpec) -> Self {
        Self {
            spec,
            dev: DevInfo::empty(),
            writers: Writers::default(),
            clock: 0,
            variant: false,
            settings: ChipSettings::default(),
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    fn is_started(&self) -> bool {
        !self.dev.data_ptr.is_null()
    }

    /// Stops the device, if one is running. Idempotent.
    fn stop(&mut self) {
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
    }

    /// Stops whatever is running and starts a device at the current clock and
    /// settings.
    ///
    /// A failure leaves the chip stopped rather than half-built, so
    /// [`render`](ChipCore::render) renders silence and nothing reads a dangling
    /// pointer. That is the honest outcome for "this build has no such device":
    /// the registry is what should have prevented it, and a silent chip is
    /// visible in a way a crash is not useful.
    fn start(&mut self) {
        self.stop();
        if self.clock == 0 {
            return;
        }

        let mut config = match self.spec.kind {
            // The one chip whose extended config lv-2 carries; lv-3's table
            // is where this becomes a per-spec constructor rather than a
            // match. Kept here for now so the enum has exactly one writer.
            ChipKind::Sn76489 => DevConfig::Sn76496(Sn76496Cfg::default()),
            _ => DevConfig::Generic(DevGenCfg::default()),
        };
        {
            let generic = config.generic_mut();
            generic.emu_core = self.spec.emu_core;
            generic.sr_mode = ffi::DEVRI_SRMODE_NATIVE;
            generic.flags = u8::from(self.variant);
            generic.clock = self.clock;
            generic.smpl_rate = REQUESTED_RATE;
        }
        (self.spec.configure)(&mut config, &self.settings);

        let mut dev = DevInfo::empty();
        // SAFETY: `config` outlives the call, its pointer is the documented
        // cast to the generic prefix, and `dev` is a valid out-param.
        let started = unsafe { ffi::SndEmu_Start(self.spec.device, config.as_ptr(), &raw mut dev) };
        if started != EERR_OK || dev.data_ptr.is_null() || dev.dev_def.is_null() {
            log::warn!(
                "libvgm refused to start {} (device {:#04x}): error {started:#04x}",
                self.spec.kind.name(),
                self.spec.device,
            );
            return;
        }

        self.dev = dev;
        // SAFETY: a live device definition from the successful start above.
        self.writers = unsafe { Writers::fetch(dev.dev_def, self.spec) };
        if !self.writers.serves(self.spec.write) {
            log::warn!(
                "libvgm's {} has no writer for {:?}; its registers will be                  silently dropped",
                self.spec.kind.name(),
                self.spec.write,
            );
        }

        // SAFETY: as above -- a live device, reset exactly as upstream's own
        // example does immediately after starting.
        unsafe {
            if let Some(reset) = (*dev.dev_def).reset {
                reset(dev.data_ptr);
            }
        }
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
        // it (see `VgmEngine::voice`), and carrying the previous file's noise
        // taps into the gap between the two would be a silent bug.
        self.settings = ChipSettings::default();
        self.start();
    }

    /// Restarts again, now that the header's chip settings are known.
    ///
    /// libvgm wants them at construction and our engine delivers them after
    /// reset, so the second start is how the two orders are reconciled. It
    /// costs one allocation per chip per file load, and it happens before any
    /// register write, so nothing is lost.
    fn configure(&mut self, settings: &ChipSettings) {
        self.settings = *settings;
        self.start();
    }

    fn native_rate(&self) -> u32 {
        self.dev.sample_rate.max(1)
    }

    fn write(&mut self, port: u8, addr: u16, data: u16) {
        if !self.is_started() {
            return;
        }
        let chip = self.dev.data_ptr;
        // SAFETY: a live device, and every writer below was fetched from it
        // and is called with the signature libvgm filed it under. What to send
        // was decided by `fold`, which is pure and tested on its own.
        unsafe {
            match fold(self.spec.write, port, addr, data) {
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
            }
        }
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

    /// A RAM block, through the chip's memory writer, byte by byte.
    ///
    /// libvgm has no block form for RAM -- upstream's `Cmd_PcmRamWrite` loops
    /// `writeM8` too -- because the chips that take one (the RF5C pair) apply
    /// a bank offset per byte.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        if !self.is_started() {
            return;
        }
        let Some(write) = self.writers.mem8 else {
            return;
        };
        for (index, &byte) in data.iter().enumerate() {
            let address = offset.wrapping_add(index as u32);
            // SAFETY: a live device and its own memory writer; libvgm masks
            // the address into the chip's window itself.
            unsafe { write(self.dev.data_ptr, address as u16, byte) };
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

        for (frame, (&left, &right)) in out
            .chunks_exact_mut(2)
            .zip(self.left.iter().zip(self.right.iter()))
        {
            frame[0] = left;
            frame[1] = right;
        }
    }
}

/// Declares the chip table and, per row, the bare `fn` the registry needs.
///
/// A registry entry is `(id, ChipKind, fn() -> Box<dyn ChipCore>)` and that
/// last one cannot be a closure over a spec, so each chip needs a function
/// that names its own kind. Writing them by hand would be two lines of
/// boilerplate per chip and one opportunity per chip to pair the wrong id with
/// the wrong device; this way a chip is one line and the three cannot drift.
macro_rules! chip_specs {
    ($(
        $make:ident : $id:literal => $kind:ident,
        $device:expr, $emu_core:expr, $write:expr, $rom_spaces:expr, $configure:expr ;
    )*) => {
        /// Every chip this crate can build, in the order the registry lists
        /// them.
        ///
        /// lv-4 is what grows this; a row here must also have its device named
        /// in `build.rs`'s `ENABLED`, or the start fails and the chip is
        /// silent. A `static` rather than a `const` on purpose: the makers
        /// below take `&'static` references into it, which a `const` -- being
        /// a fresh value at each use -- could not give them.
        pub(crate) static SPECS: &[ChipSpec] = &[$(
            ChipSpec {
                id: $id,
                kind: ChipKind::$kind,
                device: $device,
                emu_core: $emu_core,
                write: $write,
                rom_spaces: $rom_spaces,
                configure: $configure,
                make: $make,
            },
        )*];

        $(
            fn $make() -> Box<dyn ChipCore> {
                Box::new(LibVgmChip::new(spec_for(ChipKind::$kind)))
            }
        )*
    };
}

chip_specs! {
    // --- `emu_core` is named only where the device has more than one core ---
    //
    // For a single-core device, `0` is unambiguous. Where there is a choice,
    // the value must match what the pinned reference runs or the parity row
    // measures two emulators rather than our binding -- the lv-2 lesson,
    // written into the two rows that need it. `[SN76496] Core = MAXM` and
    // `[HuC6280] Core = OOTK` are the only relevant selections the pinned ini
    // makes; QSound and RF5C68 also have a choice and the ini leaves it to
    // VGMPlay's default, so **their core is unarbitrated and lv-4 must settle
    // it by measurement** before either takes a row.

    make_sn76489: "sn76489.libvgm" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAXM, WriteRule::Register, [0, 0], configure_sn76496;
    make_huc6280: "huc6280.libvgm" => HuC6280,
        ffi::DEVID_C6280, ffi::FCC_OOTK, WriteRule::Register, [0, 0], configure_none;

    // Plain 8-bit register files: `Cmd_Ofs8_Data8` upstream.
    make_k053260: "k053260.libvgm" => K053260,
        ffi::DEVID_K053260, 0, WriteRule::Register, [0, 0], configure_none;
    make_ga20: "ga20.libvgm" => Ga20,
        ffi::DEVID_GA20, 0, WriteRule::Register, [0, 0], configure_none;
    make_upd7759: "upd7759.libvgm" => Upd7759,
        ffi::DEVID_UPD7759, 0, WriteRule::Register, [0, 0], configure_none;
    make_okim6258: "okim6258.libvgm" => Okim6258,
        ffi::DEVID_MSM6258, 0, WriteRule::Register, [0, 0], configure_none;
    make_multipcm: "multipcm.libvgm" => MultiPcm,
        ffi::DEVID_YMW258, 0, WriteRule::Register, [0, 0], configure_none;
    // `Cmd_Port_Ofs8_Data8`: the port selects nothing on the write itself.
    make_es5503: "es5503.libvgm" => Es5503,
        ffi::DEVID_ES5503, 0, WriteRule::Register, [0, 0], configure_none;

    // The Yamaha latch pair.
    make_ymz280b: "ymz280b.libvgm" => Ymz280b,
        ffi::DEVID_YMZ280B, 0, WriteRule::RegisterLatch, [0, 0], configure_none;
    make_k051649: "k051649.libvgm" => K051649,
        ffi::DEVID_K051649, 0, WriteRule::RegisterLatch, [0, 0], configure_none;

    // Memory-space writes with the address arriving whole (`0xC0`, `0xC7`,
    // `0xC8`).
    make_segapcm: "segapcm.libvgm" => SegaPcm,
        ffi::DEVID_SEGAPCM, 0, WriteRule::Memory, [0, 0], configure_none;
    make_x1010: "x1010.libvgm" => X1010,
        ffi::DEVID_X1_010, 0, WriteRule::Memory, [0, 0], configure_none;
    make_vsu: "vsu.libvgm" => Vsu,
        ffi::DEVID_VBOY_VSU, 0, WriteRule::Memory, [0, 0], configure_none;

    // ...and with it split across our `port`/`addr` (`0xD3`, `0xD4`).
    make_c140: "c140.libvgm" => C140,
        ffi::DEVID_C140, 0, WriteRule::MemoryPortHigh, [0, 0], configure_none;
    make_k054539: "k054539.libvgm" => K054539,
        ffi::DEVID_K054539, 0, WriteRule::MemoryPortHigh, [0, 0], configure_none;

    // The three one-off shapes the plan named.
    make_c352: "c352.libvgm" => C352,
        ffi::DEVID_C352, 0, WriteRule::RegisterAddr16Data16, [0, 0], configure_none;
    make_qsound: "qsound.libvgm" => QSound,
        ffi::DEVID_QSOUND, 0, WriteRule::QSound, [0, 0], configure_none;
    make_rf5c68: "rf5c68.libvgm" => Rf5c68,
        ffi::DEVID_RF5C68, 0, WriteRule::RegisterOrMemoryByPort, [0, 0], configure_none;
    // The same device; `flags` (our `variant`) is what makes it the 164, and
    // the engine sets that from the header.
    make_rf5c164: "rf5c164.libvgm" => Rf5c164,
        ffi::DEVID_RF5C68, 0, WriteRule::RegisterOrMemoryByPort, [0, 0], configure_none;
}

/// A chip whose configuration is only the generic fields.
///
/// Most of them: the clock, the rate mode and the variant flag are set before
/// this is called, and there is nothing else the header carries.
fn configure_none(_config: &mut DevConfig, _settings: &ChipSettings) {}

/// The spec for `kind`.
///
/// # Panics
/// If `kind` has no row -- which only a maker generated by [`chip_specs!`] can
/// ask for, and the macro generates one maker per row, so it cannot happen.
#[must_use]
pub(crate) fn spec_for(kind: ChipKind) -> &'static ChipSpec {
    SPECS
        .iter()
        .find(|spec| spec.kind == kind)
        .unwrap_or_else(|| unreachable!("chip_specs! generates a maker per row"))
}

/// The SN76489's identity, from the VGM header.
///
/// Every field here changes what the part *is* rather than how it sounds, and
/// the frozen scorecard records what a wrong one costs: the noise channel
/// emits a different pseudo-random sequence entirely.
///
/// **Transcribed from libvgm's own `player/vgmplayer.cpp`**, not derived from
/// the VGM specification, and that is the rule this function exists to
/// establish for lv-3. A first attempt here read the spec and got six of the
/// seven fields wrong -- inverted sense on `stereo`, hard-coded `negate` and
/// `clkDiv`, `segaPSG` and `ncrPSG` missed entirely, and both defaults set to
/// the TI part when libvgm's are the SEGA PSG's. Every one of those is a
/// silent wrongness: the chip still starts, still sounds, and is simply a
/// different part. The player is the authority because it is the code the
/// reference measurement runs.
///
/// The flag bits, for reading alongside `vgmplayer.cpp`:
/// `0x01` frequency 0 is 0x400 (so *clear* means the SEGA behaviour),
/// `0x02` negate output, `0x04` stereo off, `0x08` clock divider off,
/// `0x10` NCR noise algorithm.
fn configure_sn76496(config: &mut DevConfig, settings: &ChipSettings) {
    let DevConfig::Sn76496(cfg) = config else {
        debug_assert!(false, "the SN76489 spec must be given an Sn76496 config");
        return;
    };
    let flags = settings.sn76489_flags;
    cfg.shift_reg_width = if settings.sn76489_shift_width == 0 {
        0x10
    } else {
        settings.sn76489_shift_width
    };
    cfg.noise_taps = if settings.sn76489_feedback == 0 {
        0x09
    } else {
        settings.sn76489_feedback
    };
    cfg.sega_psg = u8::from(flags & 0x01 == 0);
    cfg.negate = u8::from(flags & 0x02 != 0);
    cfg.stereo = u8::from(flags & 0x04 == 0);
    cfg.clk_div = if flags & 0x08 != 0 { 1 } else { 8 };
    cfg.ncr_psg = u8::from(flags & 0x10 != 0);
    cfg.t6w28_tone = std::ptr::null_mut();
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::spec_for as spec;

    fn energy(out: &[i32]) -> i64 {
        out.iter().map(|&s| i64::from(s.abs())).sum()
    }

    /// Construction, native rate, writes and render, end to end through the
    /// `ChipCore` trait rather than through the raw FFI -- the lv-2 gate.
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

    /// `native_rate` reports what the core will *actually* render at, which is
    /// not always derived from the clock.
    ///
    /// Upstream warns that some cores ignore `srMode` and always use
    /// `smplRate`, and Maxim's SN76489 is one: asked for native mode it still
    /// answers [`REQUESTED_RATE`]. That is not a defect and not something to
    /// work around -- the engine resamples from whatever `native_rate` says --
    /// but it *is* worth pinning, because the obvious assumption (a libvgm
    /// chip's rate follows its clock, as ymfm's does) is false, and code
    /// written on it would look right and drift pitch.
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
    /// test above would still pass -- which is exactly the bug the frozen
    /// scorecard caught in our own core.
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

    /// The header-to-config mapping is libvgm's player's, field for field.
    ///
    /// Pinned because getting it wrong is *silent*: every field here selects a
    /// different real part, and the chip starts and sounds either way. The
    /// expected values are read straight off `player/vgmplayer.cpp`'s
    /// `DEVID_SN76496` arm at the pinned commit.
    #[test]
    fn the_header_maps_to_libvgms_own_config_fields() {
        let built = |settings: ChipSettings| -> Sn76496Cfg {
            let mut config = DevConfig::Sn76496(Sn76496Cfg::default());
            configure_sn76496(&mut config, &settings);
            let DevConfig::Sn76496(cfg) = config else {
                unreachable!()
            };
            cfg
        };

        // An empty header: libvgm falls back to the SEGA PSG, *not* the TI
        // part -- 16-bit register, taps 0x09 -- and every flag reads as its
        // zero sense.
        let empty = built(ChipSettings::default());
        assert_eq!(empty.shift_reg_width, 0x10);
        assert_eq!(empty.noise_taps, 0x09);
        assert_eq!(empty.sega_psg, 1, "flag 0x01 clear means SEGA frequencies");
        assert_eq!(empty.negate, 0);
        assert_eq!(empty.stereo, 1, "flag 0x04 clear means stereo *on*");
        assert_eq!(empty.clk_div, 8, "flag 0x08 clear means the divider is on");
        assert_eq!(empty.ncr_psg, 0);

        // The TI SN76489 as the corpus's own files declare it.
        let ti = built(ChipSettings {
            sn76489_feedback: 0x0003,
            sn76489_shift_width: 15,
            sn76489_flags: 0x02,
            ..ChipSettings::default()
        });
        assert_eq!((ti.shift_reg_width, ti.noise_taps), (15, 0x0003));
        assert_eq!(ti.negate, 1, "flag 0x02 set negates the output");

        // Every flag set: the opposite sense of each.
        let all = built(ChipSettings {
            sn76489_flags: 0x1F,
            ..ChipSettings::default()
        });
        assert_eq!(all.sega_psg, 0);
        assert_eq!(all.negate, 1);
        assert_eq!(all.stereo, 0);
        assert_eq!(all.clk_div, 1);
        assert_eq!(all.ncr_psg, 1);
    }

    /// **lv-3's per-entry gate: the exact bytes each rule puts on the bus.**
    ///
    /// Every case is checked against the handler in
    /// `player/vgmplayer_cmdhandler.cpp` that it mirrors, because these are
    /// inversions of our own decoder's normalisations and a wrong one is
    /// silent -- the chip takes the write and plays something else.
    #[test]
    fn each_write_rule_puts_the_documented_bytes_on_the_bus() {
        // `Cmd_SN76489` / `Cmd_GGStereo` / `Cmd_Ofs8_Data8`: straight through,
        // and the address is *not* forced to zero -- the SN76489's `0x4F`
        // (Game Gear stereo) arrives as address 1 and must stay there.
        assert_eq!(
            fold(WriteRule::Register, 0, 0x00, 0x8E),
            Bus::Reg8(0x00, 0x8E)
        );
        assert_eq!(
            fold(WriteRule::Register, 0, 0x01, 0xDE),
            Bus::Reg8(0x01, 0xDE),
            "SN76496_W_GGST is address 1; folding it to 0 would send a stereo \
             mask to the tone latch"
        );

        // `SendYMCommand`: the address/data latch pair, port-shifted. Two
        // writes -- one alone latches a register number and plays nothing.
        assert_eq!(
            fold(WriteRule::RegisterLatch, 0, 0x28, 0xF1),
            Bus::Reg8Pair((0x00, 0x28), (0x01, 0xF1))
        );
        assert_eq!(
            fold(WriteRule::RegisterLatch, 1, 0x28, 0xF1),
            Bus::Reg8Pair((0x02, 0x28), (0x03, 0xF1)),
            "port 1 is offsets 2 and 3, not 1 and 2"
        );

        // `Cmd_RF5C_Mem` / `Cmd_Ofs16_Data8` with a whole address.
        assert_eq!(
            fold(WriteRule::Memory, 0, 0x0F12, 0x7F),
            Bus::Mem8(0x0F12, 0x7F)
        );

        // `Cmd_Ofs16_Data8` for `0xD3`/`0xD4`, where our decoder split the
        // 16-bit offset across `port` and `addr`. Recombining it wrongly is
        // the difference between C140 voice 0 and voice 8.
        assert_eq!(
            fold(WriteRule::MemoryPortHigh, 0x12, 0x34, 0x56),
            Bus::Mem8(0x1234, 0x56)
        );

        // `Cmd_Ofs16_Data16`: the C352, both operands already big-endian.
        assert_eq!(
            fold(WriteRule::RegisterAddr16Data16, 0, 0x0108, 0xBEEF),
            Bus::Reg16(0x0108, 0xBEEF)
        );

        // `Cmd_QSound_Reg`: MSB, LSB, then the register -- and the register
        // last, because it is the write that commits the pair.
        assert_eq!(
            fold(WriteRule::QSound, 0, 0x09, 0x1234),
            Bus::Reg8Triple((0x00, 0x12), (0x01, 0x34), (0x02, 0x09))
        );

        // The RF5C pair: our port-1 convention chooses the *function*, so the
        // same address means two different places.
        assert_eq!(
            fold(WriteRule::RegisterOrMemoryByPort, 0, 0x07, 0xC0),
            Bus::Reg8(0x07, 0xC0),
            "port 0 is the register file"
        );
        assert_eq!(
            fold(WriteRule::RegisterOrMemoryByPort, 1, 0x07, 0xC0),
            Bus::Mem8(0x0007, 0xC0),
            "port 1 is the RAM window -- the same number, a different space"
        );
    }

    /// Every spec's device is actually compiled in, so no row is silently
    /// silent.
    ///
    /// `build.rs`'s `ENABLED` and this table are two lists that have to agree
    /// and cannot see each other. A spec whose device was left out starts
    /// nothing, and the only symptom is a chip that plays silence -- which is
    /// indistinguishable from a chip that plays badly until someone checks.
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

    /// Registry ids are unique and slot-prefixed, as `dro-synth` requires.
    #[test]
    fn every_spec_has_a_well_formed_unique_id() {
        let mut seen = std::collections::BTreeSet::new();
        for spec in SPECS {
            assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
            assert_eq!(
                spec.id,
                format!("{}.{}", spec.kind.slug(), crate::CORE_SUFFIX),
                "an id must be <slot>.<core> or the config cannot address it"
            );
        }
    }

    /// A ROM block reaches the space its type names, and the size is declared
    /// before the data -- both are what upstream's `WriteChipROM` does.
    ///
    /// There is nothing observable to assert against a real core here, so this
    /// checks the routing decision (which is ours) and that delivery is
    /// survivable (which is libvgm's).
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

    /// Every chip in the table starts, takes its own rule's writes, and
    /// renders -- the lv-3 equivalent of lv-1's sound gate, across the batch.
    ///
    /// Not an audibility assertion: most of these need a sample ROM and a
    /// driver's worth of setup before they make a sound, and inventing one per
    /// chip would be inventing the very conventions the corpus is there to
    /// arbitrate. What it does catch is a rule whose writer is missing, a
    /// device that refuses its own registers, and a render that reads a
    /// dangling pointer.
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

    /// Dropping a started chip stops its device. Nothing observable proves a
    /// free happened, so this is a leak-check under a loop rather than an
    /// assertion: it exists so a missing `Stop` shows up under a sanitiser or
    /// as unbounded growth rather than never.
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
