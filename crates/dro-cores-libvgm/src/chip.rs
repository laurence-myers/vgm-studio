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

use dro_core::vgm::stream::{BANK_PORT, MEMORY_PORT, STEREO_PORT};
use dro_core::vgm::{ChipKind, ChipSettings};
use dro_synth::LEVEL_UNITY;
use dro_synth::chip::ChipCore;

use crate::ffi::{
    self, Ay8910Cfg, DevFuncWriteA8D8, DevFuncWriteA8D16, DevFuncWriteA16D8, DevFuncWriteA16D16,
    DevFuncWriteBlock, DevFuncWriteMemSize, DevFuncWriteOptMask, DevGenCfg, DevInfo, DevLinkInfo,
    EERR_OK, Msm6258Cfg, RWF_MEMORY, RWF_REGISTER, RWF_WRITE, SegaPcmCfg, Sn76496Cfg,
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
    /// The MultiPCM: its register file on port 0, its `0xC3` bank select on
    /// [`BANK_PORT`]. Upstream's `Cmd_Ofs8_Data8` (`0xB5`) and `Cmd_YMW_Bank`
    /// (`0xC3`), which are two commands into one chip.
    ///
    /// **The bank is not a register write and does not arrive as one.** The
    /// YMW258 has no bank register on its bus; libvgm's core invents three
    /// (`0x10` for Sega Model 1's 1 MB window, `0x11`/`0x12` for Multi 32's two
    /// 512 KiB banks) and `Cmd_YMW_Bank` translates the command into them. This
    /// rule is that translation, and it has to be here rather than in the
    /// decoder because the register numbers are libvgm's invention -- our
    /// clean-room core models the same banks with no registers at all.
    ///
    /// Both halves come from the command: `addr` is its bank mask and `data`
    /// its offset in 64 KiB units. The offset's *high* byte is dropped, exactly
    /// as upstream drops `fData[0x03]`, because these registers hold a byte and
    /// cannot express a ROM larger than 16 MB. The corpus never sets it.
    MultiPcmBank,
    /// The NES APU with upstream's FDS remap: `Cmd_NES_Reg`. `0x3F` becomes
    /// `0x23` (FDS I/O enable) and `0x20`-`0x3E` become `0x80 | (a & 0x1F)`
    /// (the FDS registers), everything else passes through.
    ///
    /// The remap is the *player's*, not the chip's: VGM stores FDS writes in a
    /// compressed range the NSFPlay core does not use, so a binding that skips
    /// it sends every FDS register at the wrong file.
    NesApu,
    /// The OKIM6295 with upstream's pin-7 strip: `Cmd_OKIM6295_Reg`. A write
    /// to `0x0B` (the clock select) drops bit 7 -- "a bug in some MAME VGM
    /// logs", per upstream's own comment -- and everything else passes.
    Okim6295,
    /// The WonderSwan: registers offset by `0x80` on port 0 (`Cmd_WSwan_Reg`
    /// writes `0x80 + (a & 0x7F)` because the chip's ports really do live
    /// there), wave RAM through the memory writer on [`MEMORY_PORT`]
    /// (`Cmd_Ofs16_Data8`, our `0xC6` convention).
    WonderSwan,
    /// The SAA1099's two-write pair in the *reverse* order to the Yamaha one:
    /// `Cmd_SAA_Reg` puts the register at offset 1 and the data at offset 0,
    /// because that is where the chip's own bus has them.
    ReversedLatch,
    /// One 16-bit-data register write: `writeD16(addr, data)`. Upstream's
    /// `Cmd_Ofs4_Data12` -- the 32X PWM, whose nibble register and 12-bit
    /// value our decoder already delivers as `addr`/`data`. (The ES5506's
    /// `0xD6` would take a two-width sibling of this rule, but libvgm ships
    /// only a stub declaration for that device -- see the spec table.)
    Data16,
    /// The AY8910: an address/data latch pair on offsets 0/1, its `0x31`
    /// stereo mask on [`STEREO_PORT`] -- which goes to a dedicated per-core
    /// function (`Cmd_AY_Stereo` fetches it by the `'ST'` user code), never to
    /// the register file. Upstream's `Cmd_DReg8_Data8` reaches
    /// `SendYMCommand(cDev, 0, reg, data)`, because what both AY cores file
    /// under `DEVRW_A8D8` is their *IO-port* interface (`EPSG_writeIO`, MAME's
    /// `ay8910_write`): an even offset latches the register address, an odd
    /// one writes the data. A single direct `write8(reg, data)` under those
    /// semantics lands every write in the wrong register -- odd register
    /// numbers dump the data wherever the latch happens to point, even ones
    /// re-latch the address to the *data* byte -- which renders as digital
    /// noise. That was this rule's shipped bug, caught by ear on the corpus.
    RegisterWithStereo,
    /// The OPN family (YM2203/2608/2610): the Yamaha latch pair on its ports,
    /// plus the YM2203's SSG stereo mask on [`STEREO_PORT`], which lands on
    /// the *linked* AY8910's mask function.
    OpnFamily,
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
    /// One `writeD16(addr, data)` -- an 8-bit address with 16-bit data.
    RegD16(u8, u16),
    /// The chip's dedicated stereo-mask function, with the six mask bits.
    StereoMask(u8),
    /// Nothing at all. Only [`WriteRule::MultiPcmBank`] produces it, for a bank
    /// select whose mask names no bank -- upstream's `Cmd_YMW_Bank` reaches the
    /// same conclusion by taking neither of its two `if`s.
    Nothing,
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
        WriteRule::NesApu => {
            // `Cmd_NES_Reg`'s FDS remap, register for register.
            let a = addr as u8;
            let remapped = if a == 0x3F {
                0x23
            } else if a & 0xE0 == 0x20 {
                0x80 | (a & 0x1F)
            } else {
                a
            };
            Bus::Reg8(remapped, data as u8)
        }
        WriteRule::Okim6295 => {
            // `Cmd_OKIM6295_Reg`: the clock-select register drops the stray
            // pin-7 bit some MAME logs carry.
            let value = if addr == 0x0B {
                (data as u8) & 0x7F
            } else {
                data as u8
            };
            Bus::Reg8(addr as u8, value)
        }
        WriteRule::WonderSwan => {
            if port == MEMORY_PORT {
                Bus::Mem8(addr, data as u8)
            } else {
                // `Cmd_WSwan_Reg`: the audio ports live at 0x80 on the chip's
                // own bus, and our decoder keeps the file's 0-based numbers.
                Bus::Reg8(0x80 + (addr as u8 & 0x7F), data as u8)
            }
        }
        WriteRule::ReversedLatch => {
            // `Cmd_SAA_Reg`: register to offset 1, then data to offset 0 --
            // the mirror image of the Yamaha pair.
            Bus::Reg8Pair((0x01, addr as u8), (0x00, data as u8))
        }
        WriteRule::Data16 => Bus::RegD16(addr as u8, data),
        WriteRule::RegisterWithStereo => {
            if port == STEREO_PORT {
                Bus::StereoMask(data as u8 & 0x3F)
            } else {
                // The IO-port latch: address to offset 0, data to offset 1.
                Bus::Reg8Pair((0x00, addr as u8), (0x01, data as u8))
            }
        }
        WriteRule::OpnFamily => {
            if port == STEREO_PORT {
                Bus::StereoMask(data as u8 & 0x3F)
            } else {
                Bus::Reg8Pair(((port << 1), addr as u8), ((port << 1) | 1, data as u8))
            }
        }
        WriteRule::MultiPcmBank => {
            if port != BANK_PORT {
                return Bus::Reg8(addr as u8, data as u8);
            }
            // `Cmd_YMW_Bank`, register for register. The offset's low byte is
            // the whole bank as far as these registers go, and each divides it
            // down to what its own shift expects: the core does `data << 20`
            // for `0x10` and `data << 19` for `0x11`/`0x12`, so both land on
            // `(offset & 0xFF) << 16` bytes -- the 64 KiB units the command
            // counts in.
            let bank = (data & 0xFF) as u8;
            match addr & 0x03 {
                // 1 MB banking (Sega Model 1): both halves of one window, and
                // only when the offset is a whole megabyte. An offset with bit
                // 3 set is half a megabyte in, which one window cannot express,
                // so upstream falls through to the pair below.
                0x03 if bank & 0x08 == 0 => Bus::Reg8(0x10, bank / 0x10),
                // 512 KB banking (Sega Multi 32): mask bit 1 is the low bank,
                // bit 0 the high one, and both may be set.
                0x03 => Bus::Reg8Pair((0x11, bank / 0x08), (0x12, bank / 0x08)),
                0x02 => Bus::Reg8(0x11, bank / 0x08),
                0x01 => Bus::Reg8(0x12, bank / 0x08),
                _ => Bus::Nothing,
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
    /// The registry id, `"<chip slug>.libvgm"` -- or, for an alternative
    /// core, `"<chip slug>.libvgm-<core>"`. Written out rather than composed
    /// at runtime because [`CoreInfo::id`](dro_synth::CoreInfo::id) is a
    /// `&'static str` that lands in `drotrim.ini`.
    pub(crate) id: &'static str,
    /// What the Settings picker calls this row. The default rows share one
    /// name; an alternative core names the emulator it selects, or the
    /// dropdown would offer identical entries.
    pub(crate) label: &'static str,
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
    /// This core's measured output calibration, 8.8 fixed point, as
    /// [`CoreInfo::level`](dro_synth::CoreInfo::level).
    ///
    /// **Measured or unity, never guessed.** Our clean-room cores carry
    /// hand-fitted scale factors *inside* them (`cores/k053260.rs`'s `* 11 >> 3`
    /// is a x5.5); libvgm's carry whatever its own upstream chose, and the two
    /// need not agree. The number here is the least-squares gain the parity
    /// harness reports against the pinned reference, and it is only meaningful
    /// because these rows correlate at 1.0000 -- where a single scalar really
    /// does describe the whole difference. A row that has not been measured
    /// stays at unity and is honest about being uncalibrated.
    pub(crate) level: u16,
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
    /// The AY8910 family, whose type and flags bytes select the variant.
    Ay8910(Ay8910Cfg),
    /// The OKIM6258, whose divider and bit widths come from the header flags.
    Msm6258(Msm6258Cfg),
    /// Sega PCM, whose bank shift and mask come from the interface register.
    SegaPcm(SegaPcmCfg),
}

impl DevConfig {
    /// The generic half, which every variant has and every start reads.
    fn generic_mut(&mut self) -> &mut DevGenCfg {
        match self {
            Self::Generic(cfg) => cfg,
            Self::Sn76496(cfg) => &mut cfg.gen_cfg,
            Self::Ay8910(cfg) => &mut cfg.gen_cfg,
            Self::Msm6258(cfg) => &mut cfg.gen_cfg,
            Self::SegaPcm(cfg) => &mut cfg.gen_cfg,
        }
    }

    /// A pointer `SndEmu_Start` can read, whatever the real struct is.
    fn as_ptr(&self) -> *const DevGenCfg {
        match self {
            Self::Generic(cfg) => std::ptr::from_ref(cfg),
            Self::Sn76496(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::Ay8910(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::Msm6258(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::SegaPcm(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
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
/// can be read side by side. **Note `reg16` in particular**: upstream calls it
/// `writeM16` and fetches it with `RWF_REGISTER`, not `RWF_MEMORY`. Copying the
/// name without reading the fetch would have put the C352's writes into the
/// wrong space.
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
/// Transcribed from upstream's `_VGM_ROM_CHIPS` table, whose second column is
/// exactly this. Only three block types name the second space, and every one of
/// them is a chip with two genuinely different memories -- so the default is
/// the first space and the exceptions are listed rather than computed.
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
/// libvgm links devices for *register* traffic (the parent forwards SSG
/// register I/O through hooks `LinkDevice` installs), but each device renders
/// its own audio stream at its own rate -- upstream gives every link its own
/// resampler and volume. So a child here carries a small linear resampler
/// into the parent's rate and a gain, mirroring `GetChipVolume`'s link column.
struct LinkedDev {
    dev: DevInfo,
    /// libvgm's `DEVID_` for this child, so a mute mask can be routed to it --
    /// the OPN family's SSG channels live on the linked `DEVID_AY8910`, not on
    /// the parent's own mute mask.
    dev_id: u8,
    /// The child's level relative to the parent, 8.8 fixed point --
    /// upstream's `GetChipVolume(..., isLinked=1)` over `isLinked=0`: half
    /// for the YM2203's SSG, unity otherwise.
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
    /// Which channels are muted, in [`dro_core::vgm::channels_of`] order.
    /// Kept here because a device restart (every `reset`, and `configure`)
    /// clears the core's own mask, so it is reapplied after each `start`.
    mute_mask: u32,
    /// Where each channel sits in the stereo image, libvgm's `-0x100..=0x100`.
    /// Empty means the chip's own image; reapplied after each `start` like the
    /// mute mask.
    pans: Vec<i16>,
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
            links: Vec::new(),
            clock: 0,
            variant: false,
            settings: ChipSettings::default(),
            mute_mask: 0,
            pans: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
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
            generic.sr_mode = ffi::DEVRI_SRMODE_NATIVE;
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
        // SAFETY: a live device definition from the successful start above.
        self.writers = unsafe { Writers::fetch(dev.dev_def, self.spec) };
        if !self.writers.serves(self.spec.write) {
            log::warn!(
                "libvgm's {} has no writer for {:?}; its registers will be                  silently dropped",
                self.spec.kind.name(),
                self.spec.write,
            );
        }

        // Option bits VGMPlay sets by default, applied before any register
        // arrives so the core never runs in a state the reference never uses.
        let option_bits = default_option_bits(self.spec.kind);
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
            // The child's core and header bytes, exactly as upstream's link
            // callback sets them: EMU2149 for an OPN's SSG (with the header's
            // per-chip SSG flags), adlibemu for the OPL4's FM half.
            //
            // SAFETY: `cfg` points at a config the parent allocated for us to
            // adjust -- that is its documented purpose.
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

            // The stereo mask function lives on the SSG child for the OPN
            // family -- `Cmd_AY_Stereo` fetches it from the linked device.
            // Upstream then invokes it with the *parent's* data pointer,
            // which reads as a bug; the child's own pointer is what the
            // function's core expects, and is what we use.
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

/// The option bits VGMPlay applies to a chip before playing anything --
/// transcribed from its `InitDevOptions` defaults, so the cores run in the
/// same mode the reference runs them.
const fn default_option_bits(kind: ChipKind) -> u32 {
    match kind {
        // NSFPlay's recommended APU/DMC options.
        ChipKind::NesApu => 0x01B7,
        // `OPT_SCSP_BYPASS_DSP`: the DSP is skipped by default upstream.
        ChipKind::Scsp => 0x01,
        _ => 0,
    }
}

/// Splits a canonical channel mute mask into what the parent device mutes and,
/// for the OPN family, what its linked SSG child mutes.
///
/// [`dro_core::vgm::channels_of`] puts the SSG channels last; libvgm's OPN
/// parent mutes only its FM (and, on the 2608/2610, the rhythm/ADPCM that
/// share its device), while the SSG is a separate `DEVID_AY8910` with its own
/// three-bit mask. Every other chip is identity: the whole mask to the parent,
/// nothing to a child. The bit positions here are pinned by the parent cores'
/// own `set_mute_mask` (`fmopn.c`) and the canonical channel table.
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
/// Upstream's `GetChipVolume(..., isLinked=1)`: the YM2203's SSG plays at
/// half the FM's volume; every other link (the 2608/2610 SSG, the OPL4's FM)
/// at parity.
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
                Bus::RegD16(address, value) => {
                    if let Some(write) = self.writers.data16 {
                        write(chip, address, value);
                    }
                }
                Bus::StereoMask(mask) => {
                    if let Some(write) = self.writers.stereo {
                        // The AY's own chip for a bare AY8910; the linked
                        // SSG's for an OPN -- `start_links` fetched the
                        // function from whichever device carries it, and its
                        // data pointer travels with it.
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

    /// A RAM block: through the chip's block writer where it has one, or its
    /// memory writer byte by byte.
    ///
    /// The split is upstream's. The RF5C pair take RAM through `writeM8`
    /// (each byte lands through a bank register, so there is no block form),
    /// while the SCSP's half-megabyte sample RAM arrives through the same
    /// `DEVRW_BLOCK` writer its ROMs would -- its addresses do not even fit
    /// the 16-bit memory writer.
    fn write_ram(&mut self, offset: u32, data: &[u8]) {
        if !self.is_started() {
            return;
        }
        if ram_via_block(self.spec.kind) {
            if let Some(write) = self.writers.rom_write[0]
                && !data.is_empty()
            {
                // SAFETY: a live device; libvgm copies out of `data`.
                unsafe { write(self.dev.data_ptr, offset, data.len() as u32, data.as_ptr()) };
            }
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

        // The linked children -- an OPN's SSG, the OPL4's FM -- render their
        // own streams at their own rates and are mixed in here, each through
        // its resampler and link gain, as upstream mixes each `linkDev`
        // through its own `Resmpl` chain. Taken out of `self` for the loop so
        // the planes can be borrowed alongside them.
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
/// Four of libvgm's default cores carry a `SetPanning` function: the SN76489's
/// Maxim core (our default -- the `-mame` alternative does not pan), the
/// AY8910's EMU2149, the NES APU's NSFPlay, and the YM2413's EMU2413.
/// `LibVgmChip::supports_pan` reads the started device's own field, so a wrong
/// answer here is a hidden-or-shown control, never a dead knob.
pub(crate) const fn default_core_pans(kind: ChipKind) -> bool {
    matches!(
        kind,
        ChipKind::Sn76489 | ChipKind::Ay8910 | ChipKind::NesApu | ChipKind::Ym2413
    )
}

/// Whether `kind` takes its RAM blocks through the block writer rather than
/// the byte-wide memory writer -- see [`LibVgmChip::write_ram`].
///
/// Read off each core's `rwFuncs` table, not guessed: the SCSP, ES5503 and
/// NES APU file their sample RAM *only* under `RWF_MEMORY | DEVRW_BLOCK`
/// (`scsp_write_ram`, `es5503_write_ram`, `nes_write_ram`), so a byte loop
/// finds no writer and silently drops every wavetable -- the ES5503 came
/// back 0-of-12 audible on the corpus before this listed it. The RF5C pair
/// stays on the byte writer: their `A16D8` memory function *is* the banked
/// window our port-1 convention feeds, and it measured exact.
const fn ram_via_block(kind: ChipKind) -> bool {
    matches!(kind, ChipKind::Scsp | ChipKind::Es5503 | ChipKind::NesApu)
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
        $make:ident : $id:literal / $label:literal => $kind:ident,
        $device:expr, $emu_core:expr, $write:expr, $rom_spaces:expr,
        $level:expr, $configure:expr ;
    )*) => {
        /// Every chip this crate can build, in the order the registry lists
        /// them -- so a chip's default row must precede its alternative-core
        /// rows.
        ///
        /// A row here must also have its device named in `build.rs`'s
        /// `ENABLED`, or the start fails and the chip is silent. A `static`
        /// rather than a `const` on purpose: the makers below take `&'static`
        /// references into it, which a `const` -- being a fresh value at each
        /// use -- could not give them.
        pub(crate) static SPECS: &[ChipSpec] = &[$(
            ChipSpec {
                id: $id,
                label: $label,
                kind: ChipKind::$kind,
                device: $device,
                emu_core: $emu_core,
                write: $write,
                rom_spaces: $rom_spaces,
                level: $level,
                configure: $configure,
                make: $make,
            },
        )*];

        $(
            fn $make() -> Box<dyn ChipCore> {
                // By id, not by kind: a chip with alternative cores has
                // several rows of one kind, and each maker must find its own.
                Box::new(LibVgmChip::new(spec_by_id($id)))
            }
        )*
    };
}

chip_specs! {
    // --- `emu_core` and the alternative rows ------------------------------
    //
    // A chip's first row is its default and carries the plain label; the
    // `libvgm-<core>` rows behind it publish the device's other emulators as
    // picker entries, each labelled by what it selects. For a single-core
    // device `0` is unambiguous and there is nothing to publish. The two
    // named default selections (Maxim's SN76489, Ootake's HuC6280) predate
    // the scorecard's retirement and stay: they are the cores the reference
    // ran, and nothing has since arbitrated a better default.

    make_sn76489: "sn76489.libvgm" / "libvgm" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAXM, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_sn76496;
    make_sn76489_mame: "sn76489.libvgm-mame" / "libvgm (MAME core)" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAME, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_sn76496;
    make_huc6280: "huc6280.libvgm" / "libvgm" => HuC6280,
        ffi::DEVID_C6280, ffi::FCC_OOTK, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_huc6280_mame: "huc6280.libvgm-mame" / "libvgm (MAME core)" => HuC6280,
        ffi::DEVID_C6280, ffi::FCC_MAME, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;

    // Plain 8-bit register files: `Cmd_Ofs8_Data8` upstream.
    make_k053260: "k053260.libvgm" / "libvgm" => K053260,
        ffi::DEVID_K053260, 0, WriteRule::Register, [0, 0], 494, configure_none;  // measured 1.930 (n=6)
    make_ga20: "ga20.libvgm" / "libvgm" => Ga20,
        ffi::DEVID_GA20, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_upd7759: "upd7759.libvgm" / "libvgm" => Upd7759,
        ffi::DEVID_UPD7759, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_okim6258: "okim6258.libvgm" / "libvgm" => Okim6258,
        ffi::DEVID_MSM6258, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_msm6258;
    // `Cmd_Port_Ofs8_Data8`: the port selects nothing on the write itself.
    make_es5503: "es5503.libvgm" / "libvgm" => Es5503,
        ffi::DEVID_ES5503, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_es5503;
    make_gameboydmg: "gameboydmg.libvgm" / "libvgm" => GameBoyDmg,
        ffi::DEVID_GB_DMG, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_gameboydmg_sameboy: "gameboydmg.libvgm-sameboy" / "libvgm (SameBoy core)" => GameBoyDmg,
        ffi::DEVID_GB_DMG, ffi::FCC_SBOY, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_pokey: "pokey.libvgm" / "libvgm" => Pokey,
        ffi::DEVID_POKEY, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_mikey: "mikey.libvgm" / "libvgm" => Mikey,
        ffi::DEVID_MIKEY, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;

    // Plain files with one upstream quirk each -- the remap is the rule's.
    make_nesapu: "nesapu.libvgm" / "libvgm" => NesApu,
        ffi::DEVID_NES_APU, 0, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_nesapu_nsfplay: "nesapu.libvgm-nsfplay" / "libvgm (NSFPlay core)" => NesApu,
        ffi::DEVID_NES_APU, ffi::FCC_NSFP, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_okim6295: "okim6295.libvgm" / "libvgm" => Okim6295,
        ffi::DEVID_MSM6295, 0, WriteRule::Okim6295, [0, 0], LEVEL_UNITY, configure_none;
    make_wonderswan: "wonderswan.libvgm" / "libvgm" => WonderSwan,
        ffi::DEVID_WSWAN, 0, WriteRule::WonderSwan, [0, 0], LEVEL_UNITY, configure_none;
    make_saa1099: "saa1099.libvgm" / "libvgm" => Saa1099,
        ffi::DEVID_SAA1099, 0, WriteRule::ReversedLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_saa1099_vb: "saa1099.libvgm-vb" / "libvgm (alt core)" => Saa1099,
        ffi::DEVID_SAA1099, ffi::FCC_VBEL, WriteRule::ReversedLatch, [0, 0], LEVEL_UNITY, configure_none;

    // The AY8910, with its `0x31` stereo mask on the dedicated function.
    make_ay8910: "ay8910.libvgm" / "libvgm" => Ay8910,
        ffi::DEVID_AY8910, 0, WriteRule::RegisterWithStereo, [0, 0], LEVEL_UNITY, configure_ay8910;
    make_ay8910_emu2149: "ay8910.libvgm-emu2149" / "libvgm (EMU2149 core)" => Ay8910,
        ffi::DEVID_AY8910, ffi::FCC_EMU_, WriteRule::RegisterWithStereo, [0, 0], LEVEL_UNITY, configure_ay8910;

    // The Yamaha latch pair.
    make_ymz280b: "ymz280b.libvgm" / "libvgm" => Ymz280b,
        ffi::DEVID_YMZ280B, 0, WriteRule::RegisterLatch, [0, 0], 303, configure_none;  // measured 1.185 (n=12)
    make_k051649: "k051649.libvgm" / "libvgm" => K051649,
        ffi::DEVID_K051649, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2413: "ym2413.libvgm" / "libvgm" => Ym2413,
        ffi::DEVID_YM2413, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2413_emu2413: "ym2413.libvgm-emu2413" / "libvgm (EMU2413 core)" => Ym2413,
        ffi::DEVID_YM2413, ffi::FCC_EMU_, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2612: "ym2612.libvgm" / "libvgm" => Ym2612,
        ffi::DEVID_YM2612, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2612_gens: "ym2612.libvgm-gens" / "libvgm (Gens core)" => Ym2612,
        ffi::DEVID_YM2612, ffi::FCC_GENS, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2151: "ym2151.libvgm" / "libvgm" => Ym2151,
        ffi::DEVID_YM2151, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ymf271: "ymf271.libvgm" / "libvgm" => Ymf271,
        ffi::DEVID_YMF271, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // The OPL4: its wave half is this device, its FM half a linked YMF262.
    // Not an OPL row -- the OPL family's own chips stay on `PlayerEngine`.
    // Known gap: rips that lean on the YRW801 wave ROM without embedding it
    // (some MSX MoonSound rips) play only their FM half -- VGMPlay
    // side-loads `yrw801.rom` from disk, and that ROM is not ours to ship.
    make_ymf278b: "ymf278b.libvgm" / "libvgm" => Ymf278b,
        ffi::DEVID_YMF278B, 0, WriteRule::RegisterLatch, [0x524F, 0x5241], LEVEL_UNITY, configure_none;

    // The OPN family: the latch pair, a linked SSG, and the YM2203's stereo
    // mask riding the SSG's own function.
    make_ym2203: "ym2203.libvgm" / "libvgm" => Ym2203,
        ffi::DEVID_YM2203, 0, WriteRule::OpnFamily, [0, 0], LEVEL_UNITY, configure_none;
    make_ym2608: "ym2608.libvgm" / "libvgm" => Ym2608,
        ffi::DEVID_YM2608, 0, WriteRule::OpnFamily, [0x41, 0x42], LEVEL_UNITY, configure_none;
    make_ym2610: "ym2610.libvgm" / "libvgm" => Ym2610,
        ffi::DEVID_YM2610, 0, WriteRule::OpnFamily, [0x41, 0x42], LEVEL_UNITY, configure_none;

    // Memory-space writes with the address arriving whole (`0xC0`, `0xC5`,
    // `0xC7`, `0xC8`).
    make_segapcm: "segapcm.libvgm" / "libvgm" => SegaPcm,
        ffi::DEVID_SEGAPCM, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_segapcm;
    make_x1010: "x1010.libvgm" / "libvgm" => X1010,
        ffi::DEVID_X1_010, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_none;
    make_vsu: "vsu.libvgm" / "libvgm" => Vsu,
        ffi::DEVID_VBOY_VSU, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_none;
    make_scsp: "scsp.libvgm" / "libvgm" => Scsp,
        ffi::DEVID_SCSP, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_scsp;

    // ...and with it split across our `port`/`addr` (`0xD3`, `0xD4`).
    make_c140: "c140.libvgm" / "libvgm" => C140,
        ffi::DEVID_C140, 0, WriteRule::MemoryPortHigh, [0, 0], 332, configure_c140;  // measured 1.297 (n=12)
    make_k054539: "k054539.libvgm" / "libvgm" => K054539,
        ffi::DEVID_K054539, 0, WriteRule::MemoryPortHigh, [0, 0], LEVEL_UNITY, configure_k054539;

    // The one-off shapes: the three the plan named, the MultiPCM's bank and
    // the PWM's 12-bit values.
    make_c352: "c352.libvgm" / "libvgm" => C352,
        ffi::DEVID_C352, 0, WriteRule::RegisterAddr16Data16, [0, 0], LEVEL_UNITY, configure_c352;
    make_qsound: "qsound.libvgm" / "libvgm" => QSound,
        ffi::DEVID_QSOUND, 0, WriteRule::QSound, [0, 0], LEVEL_UNITY, configure_qsound;
    make_qsound_ctr: "qsound.libvgm-ctr" / "libvgm (superctr core)" => QSound,
        ffi::DEVID_QSOUND, ffi::FCC_CTR_, WriteRule::QSound, [0, 0], LEVEL_UNITY, configure_qsound;
    // A register file plus a second command that is not a register write:
    // `0xB5` and `0xC3`, which upstream splits between `Cmd_Ofs8_Data8` and
    // `Cmd_YMW_Bank`. `Register` served it until 2026-07-29, which sent the
    // bank select at the register file and dropped the bank entirely.
    make_multipcm: "multipcm.libvgm" / "libvgm" => MultiPcm,
        ffi::DEVID_YMW258, 0, WriteRule::MultiPcmBank, [0, 0], LEVEL_UNITY, configure_multipcm;
    make_pwm: "pwm.libvgm" / "libvgm" => Pwm,
        ffi::DEVID_32X_PWM, 0, WriteRule::Data16, [0, 0], LEVEL_UNITY, configure_none;
    // No ES5505/ES5506 row: libvgm's `es5506.c` is a 32-line stub -- a
    // `DEV_DECL` whose core list is `{ NULL }` -- so `SndEmu_Start` has
    // nothing to start. The chip stays unplayable until upstream grows the
    // emulator; the decoder's `0xBE`/`0xD6` conventions are ready for it.

    make_rf5c68: "rf5c68.libvgm" / "libvgm" => Rf5c68,
        ffi::DEVID_RF5C68, 0, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c68;
    make_rf5c68_gens: "rf5c68.libvgm-gens" / "libvgm (Gens core)" => Rf5c68,
        ffi::DEVID_RF5C68, ffi::FCC_GENS, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c68;
    // The same device; `flags` is what makes it the 164.
    make_rf5c164: "rf5c164.libvgm" / "libvgm" => Rf5c164,
        ffi::DEVID_RF5C68, 0, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c164;
    make_rf5c164_gens: "rf5c164.libvgm-gens" / "libvgm (Gens core)" => Rf5c164,
        ffi::DEVID_RF5C68, ffi::FCC_GENS, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c164;
}

/// A chip whose configuration is only the generic fields.
///
/// Most of them: the clock, the rate mode and the variant flag are set before
/// this is called, and there is nothing else the header carries.
fn configure_none(_config: &mut DevConfig, _settings: &ChipSettings) {}

/// The AY8910's type and flags bytes, from the header's `0x78`/`0x79`.
fn configure_ay8910(config: &mut DevConfig, settings: &ChipSettings) {
    let DevConfig::Ay8910(cfg) = config else {
        debug_assert!(false, "the AY8910 spec must be given an Ay8910 config");
        return;
    };
    cfg.chip_type = settings.ay8910_type;
    cfg.chip_flags = settings.ay8910_flags;
}

/// The OKIM6258's divider and bit widths, decoded from the header's flags
/// byte at `0x94` exactly as upstream's `DEVID_MSM6258` case does.
fn configure_msm6258(config: &mut DevConfig, settings: &ChipSettings) {
    let DevConfig::Msm6258(cfg) = config else {
        debug_assert!(false, "the OKIM6258 spec must be given an Msm6258 config");
        return;
    };
    let flags = settings.okim6258_flags;
    cfg.divider = flags & 0x03;
    cfg.adpcm_bits = if flags & 0x04 != 0 { 4 } else { 3 };
    cfg.output_bits = if flags & 0x08 != 0 { 12 } else { 10 };
}

/// Sega PCM's bank shift and mask, from the interface register at `0x3C`.
fn configure_segapcm(config: &mut DevConfig, settings: &ChipSettings) {
    let DevConfig::SegaPcm(cfg) = config else {
        debug_assert!(false, "the SegaPCM spec must be given a SegaPcm config");
        return;
    };
    cfg.bnkshift = (settings.sega_pcm_interface & 0xFF) as u8;
    cfg.bnkmask = ((settings.sega_pcm_interface >> 16) & 0xFF) as u8;
}

/// The K054539's flags byte, plus upstream's low-clock rescue: a "clock"
/// under 1 MHz is really a sample rate from 2012-era logs, times 384.
fn configure_k054539(config: &mut DevConfig, settings: &ChipSettings) {
    let generic = config.generic_mut();
    generic.flags = settings.k054539_flags;
    if generic.clock < 1_000_000 {
        generic.clock *= 384;
    }
}

/// The SCSP's low-clock rescue: under 1 MHz is a sample rate, times 512.
fn configure_scsp(config: &mut DevConfig, _settings: &ChipSettings) {
    let generic = config.generic_mut();
    if generic.clock < 1_000_000 {
        generic.clock *= 512;
    }
}

/// The C140's banking type and clock rescues -- and when the type byte says
/// C219, `start` swaps the *device*, because libvgm gives the variant its own.
fn configure_c140(config: &mut DevConfig, settings: &ChipSettings) {
    let generic = config.generic_mut();
    if settings.c140_type == 2 {
        if generic.clock == 44_100 {
            generic.clock = 25_056_500;
        } else if generic.clock < 1_000_000 {
            generic.clock *= 576;
        }
    } else {
        if generic.clock == 21_390 {
            generic.clock = 12_288_000;
        } else if generic.clock < 1_000_000 {
            generic.clock *= 576;
        }
        generic.flags = settings.c140_type;
    }
}

/// The C352's clock divider: `real = VGM clock * 72 / divider`, as upstream
/// computes it. A zero divider (an unfilled header field) leaves the clock.
fn configure_c352(config: &mut DevConfig, settings: &ChipSettings) {
    let generic = config.generic_mut();
    if settings.c352_clock_divider != 0 {
        let scaled = u64::from(generic.clock) * 72 / u64::from(settings.c352_clock_divider);
        generic.clock = u32::try_from(scaled).unwrap_or(generic.clock);
    }
}

/// The QSound's clock rescue: old logs stored the 4 MHz serial clock where
/// the 60 MHz DSP clock belongs.
fn configure_qsound(config: &mut DevConfig, _settings: &ChipSettings) {
    let generic = config.generic_mut();
    if generic.clock < 5_000_000 {
        generic.clock *= 15;
    }
}

/// The ES5503's output-channel count, from the header's `0xD4`.
fn configure_es5503(config: &mut DevConfig, settings: &ChipSettings) {
    config.generic_mut().flags = settings.es5503_channels;
}

/// The MultiPCM's clock fix: VGM stores the old /180-divider clock, and the
/// core divides by 224, so upstream rescales by 224/180 on the way in.
fn configure_multipcm(config: &mut DevConfig, _settings: &ChipSettings) {
    let generic = config.generic_mut();
    let scaled = u64::from(generic.clock) * 224 / 180;
    generic.clock = u32::try_from(scaled).unwrap_or(generic.clock);
}

/// The RF5C68 half of the shared device: `flags` 0, as upstream sets it.
fn configure_rf5c68(config: &mut DevConfig, _settings: &ChipSettings) {
    config.generic_mut().flags = 0;
}

/// The RF5C164 half: `flags` 1 is what makes the shared device the 164.
fn configure_rf5c164(config: &mut DevConfig, _settings: &ChipSettings) {
    config.generic_mut().flags = 1;
}

/// The *default* spec for `kind` -- its first row, ahead of any
/// alternative-core rows. Test-only since the makers moved to id lookup.
#[cfg(test)]
#[must_use]
pub(crate) fn spec_for(kind: ChipKind) -> &'static ChipSpec {
    SPECS
        .iter()
        .find(|spec| spec.kind == kind)
        .unwrap_or_else(|| unreachable!("chip_specs! generates a maker per row"))
}

/// The spec with this exact id.
///
/// What the generated makers use: a chip with alternative cores has several
/// rows of one kind, and a lookup by kind would hand every one of them the
/// default's `emu_core`.
///
/// # Panics
/// If `id` has no row -- which only a maker generated by [`chip_specs!`] can
/// ask for, and the macro generates one maker per row, so it cannot happen.
#[must_use]
fn spec_by_id(id: &str) -> &'static ChipSpec {
    SPECS
        .iter()
        .find(|spec| spec.id == id)
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

        // `Cmd_Ofs8_Data8` and `Cmd_YMW_Bank`, the MultiPCM's two commands.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, 0, 0x01, 0x1E),
            Bus::Reg8(0x01, 0x1E),
            "port 0 is still the ordinary `0xB5` register file"
        );
        // A whole-megabyte offset with both banks named: Model 1's single 1 MB
        // window, register 0x10, and the value divided down to what the core's
        // `data << 20` expects. `0x10` counts 64 KiB units, so this is
        // `0x10_0000` bytes -- Daytona's, and 125 of the corpus's 296.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0010),
            Bus::Reg8(0x10, 0x01)
        );
        // Half a megabyte in, so one window cannot express it and upstream
        // takes the Multi 32 path for both banks instead -- `0x11` before
        // `0x12`, and `data << 19` this time.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0018),
            Bus::Reg8Pair((0x11, 0x03), (0x12, 0x03))
        );
        // One bank at a time: **mask bit 1 is the low bank and bit 0 the
        // high one**, which is the way round `Cmd_YMW_Bank` has it and the
        // opposite of how the bits read. 96 of the corpus's commands are these.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x02, 0x0020),
            Bus::Reg8(0x11, 0x04)
        );
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x01, 0x0028),
            Bus::Reg8(0x12, 0x05)
        );
        // A mask naming no bank writes nothing, rather than writing zero
        // somewhere: upstream takes neither `if`.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x00, 0x0010),
            Bus::Nothing
        );
        // The offset's high byte is dropped where upstream drops `fData[0x03]`.
        assert_eq!(
            fold(WriteRule::MultiPcmBank, BANK_PORT, 0x03, 0x0110),
            Bus::Reg8(0x10, 0x01),
            "these registers hold a byte; a ROM over 16 MB is not expressible"
        );

        // `Cmd_NES_Reg`'s FDS remap: 0x3F is the I/O enable at 0x23, the
        // 0x20-0x3E block moves to 0x80-0x9E, and the APU's own registers
        // pass through untouched.
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x3F, 0x80),
            Bus::Reg8(0x23, 0x80)
        );
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x22, 0x7F),
            Bus::Reg8(0x82, 0x7F)
        );
        assert_eq!(
            fold(WriteRule::NesApu, 0, 0x15, 0x0F),
            Bus::Reg8(0x15, 0x0F)
        );

        // `Cmd_OKIM6295_Reg`: only the clock-select register strips bit 7.
        assert_eq!(
            fold(WriteRule::Okim6295, 0, 0x0B, 0x85),
            Bus::Reg8(0x0B, 0x05),
            "the stray pin-7 bit some MAME logs carry is dropped"
        );
        assert_eq!(
            fold(WriteRule::Okim6295, 0, 0x00, 0x85),
            Bus::Reg8(0x00, 0x85)
        );

        // `Cmd_WSwan_Reg`: the audio ports live at 0x80 on the chip's own
        // bus; wave RAM keeps its own writer through our port convention.
        assert_eq!(
            fold(WriteRule::WonderSwan, 0, 0x0F, 0x42),
            Bus::Reg8(0x8F, 0x42)
        );
        assert_eq!(
            fold(WriteRule::WonderSwan, MEMORY_PORT, 0x0123, 0x42),
            Bus::Mem8(0x0123, 0x42),
            "wave RAM is a memory poke, not an offset register"
        );

        // `Cmd_SAA_Reg`: the mirror image of the Yamaha pair -- register to
        // offset 1, then data to offset 0.
        assert_eq!(
            fold(WriteRule::ReversedLatch, 0, 0x14, 0xFF),
            Bus::Reg8Pair((0x01, 0x14), (0x00, 0xFF))
        );

        // `Cmd_Ofs4_Data12`: the PWM's nibble register with its 12-bit value,
        // through the 16-bit-data writer.
        assert_eq!(
            fold(WriteRule::Data16, 0, 0x02, 0x0155),
            Bus::RegD16(0x02, 0x0155)
        );

        // `Cmd_AY_Stereo`: the mask goes to the dedicated function, never the
        // register file -- and a register write is the IO-port latch pair
        // (`Cmd_DReg8_Data8` -> `SendYMCommand` on port 0), because the AY
        // cores' `A8D8` writer is `EPSG_writeIO`: even offset latches the
        // address, odd offset carries the data.
        assert_eq!(
            fold(WriteRule::RegisterWithStereo, STEREO_PORT, 0, 0x2D),
            Bus::StereoMask(0x2D)
        );
        assert_eq!(
            fold(WriteRule::RegisterWithStereo, 0, 0x01, 0x0F),
            Bus::Reg8Pair((0x00, 0x01), (0x01, 0x0F))
        );

        // The OPN family: the latch pair on its ports, the YM2203's stereo
        // mask on the SSG's function.
        assert_eq!(
            fold(WriteRule::OpnFamily, 1, 0x28, 0xF1),
            Bus::Reg8Pair((0x02, 0x28), (0x03, 0xF1))
        );
        assert_eq!(
            fold(WriteRule::OpnFamily, STEREO_PORT, 0, 0x15),
            Bus::StereoMask(0x15)
        );
    }

    /// A bare AY8910's register writes go through the IO-port latch, and only
    /// sound proves it: under the direct-write bug this test's volume write
    /// re-latched the address instead of landing in R8, so the chip stayed
    /// silent (and real songs came out as register-scrambled noise). The
    /// clock, type and flags are an Atari ST YM2149's, from the file that
    /// caught it.
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

    /// Registry ids are unique and slot-prefixed, as `dro-synth` requires --
    /// and each chip's default row comes before its alternative-core rows,
    /// because registration order is priority order.
    #[test]
    fn every_spec_has_a_well_formed_unique_id() {
        let mut seen = std::collections::BTreeSet::new();
        let mut first_of: std::collections::BTreeMap<&str, &str> =
            std::collections::BTreeMap::new();
        for spec in SPECS {
            assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
            let default_id = format!("{}.{}", spec.kind.slug(), crate::CORE_SUFFIX);
            assert!(
                spec.id == default_id || spec.id.starts_with(&format!("{default_id}-")),
                "{} must be <slot>.libvgm or <slot>.libvgm-<core>",
                spec.id
            );
            first_of.entry(spec.kind.slug()).or_insert(spec.id);
        }
        // The first row seen for every chip is its plain default id.
        for (slug, first_id) in first_of {
            assert_eq!(
                first_id,
                format!("{slug}.{}", crate::CORE_SUFFIX),
                "{slug}'s default row must precede its alternates"
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

    /// Every chip that takes RAM blocks has the writer its RAM path needs.
    ///
    /// `ram_via_block` is a transcription of each core's `rwFuncs` table, and
    /// the two can drift silently: a chip listed for the block path whose
    /// core has no block writer -- or the reverse -- drops every wavetable
    /// and plays silence. The ES5503 shipped exactly that way once: its only
    /// RAM writer is `DEVRW_BLOCK`, the byte loop found nothing, and the
    /// corpus came back 0-of-12 audible.
    #[test]
    fn every_ram_taking_chip_has_the_writer_its_path_needs() {
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
            if ram_via_block(kind) {
                assert!(
                    chip.writers.rom_write[0].is_some(),
                    "{} takes RAM via the block writer, which its core must file",
                    kind.name()
                );
            } else {
                assert!(
                    chip.writers.mem8.is_some(),
                    "{} takes RAM via the byte writer, which its core must file",
                    kind.name()
                );
            }
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
