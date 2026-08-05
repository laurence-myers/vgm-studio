// SPDX-License-Identifier: GPL-2.0-or-later
//! The chip table: one [`ChipSpec`] row per core this crate can build, and the
//! per-row maker the registry needs.
//!
//! A row is pure data -- which libvgm device, which [`WriteRule`], the measured
//! level -- plus a [`configure`](ChipSpec::configure) hook that fills the
//! chip-specific half of its [`DevConfig`] from the VGM header. [`chip_specs!`]
//! emits the [`SPECS`] table and, per row, the bare `fn` the registry keys on;
//! each maker builds a [`LibVgmChip`](crate::chip::LibVgmChip) over its own row.

use vgms_core::vgm::{ChipKind, ChipSettings};
use vgms_synth::LEVEL_UNITY;
use vgms_synth::chip::ChipCore;

use crate::chip::LibVgmChip;
use crate::ffi::{self, Ay8910Cfg, DevGenCfg, Msm6258Cfg, SegaPcmCfg, Sn76496Cfg};
use crate::fold::WriteRule;

/// One chip: which libvgm device it is, and how to talk to it.
///
/// A `&'static` table row rather than a trait object: everything here is data,
/// and the alternative is a virtual call per register write.
#[derive(Debug)]
pub(crate) struct ChipSpec {
    /// The registry id, `"<chip slug>.libvgm"` (or `"...-<core>"` for an
    /// alternative core). Written out rather than composed at runtime because
    /// [`CoreInfo::id`](vgms_synth::CoreInfo::id) is a `&'static str` that lands
    /// in `vgmstudio.ini`.
    pub(crate) id: &'static str,
    /// What the Settings picker calls this row. Default rows share one name; an
    /// alternative core names the emulator it selects, or the dropdown would
    /// offer identical entries.
    pub(crate) label: &'static str,
    /// Our engine's name for the chip -- what the registry keys on.
    pub(crate) kind: ChipKind,
    /// libvgm's `DEVID_` constant.
    pub(crate) device: u8,
    /// A four-character code from `EmuCores.h`, or 0 for the device's default
    /// core.
    pub(crate) emu_core: u32,
    /// How writes fold.
    pub(crate) write: WriteRule,
    /// This core's measured output calibration, 8.8 fixed point, as
    /// [`CoreInfo::level`](vgms_synth::CoreInfo::level).
    ///
    /// **Measured or unity, never guessed**, by one of two runs, both in
    /// `vgms-app`'s `reference_parity`:
    ///
    /// * against the pinned reference, for a chip this is the only core for
    ///   (`every_cored_chip_matches_the_reference_within_its_band`, reading the
    ///   `lvl` column -- the RMS ratio, *not* the `gain` column);
    /// * against the chip's default core, for a row that is an alternative to
    ///   one (`every_core_for_a_chip_agrees_on_its_level`) -- which is the same
    ///   anchor by another route, since the default is itself calibrated to the
    ///   reference.
    ///
    /// A row whose ratio scatters across files is left at unity on purpose: one
    /// scalar cannot describe two emulators that differ in more than level, and
    /// the wrong constant is worse than none. The comment on each measured row
    /// carries the sample size and the observed range for exactly that reason.
    pub(crate) level: u16,
    /// The `user` selector for each of the chip's two sample-memory spaces.
    ///
    /// libvgm files a chip's ROM writers by `user`, per chip: `0` for most,
    /// `'A'`/`'B'` for the YM2610's two ADPCM spaces, `"RO"`/`"RA"` for the
    /// YMF278B's ROM and RAM. Taken from the `SndEmu_GetDeviceFunc` calls in
    /// `player/vgmplayer.cpp`; a chip with one space repeats it.
    pub(crate) rom_spaces: [u16; 2],
    /// Fills in the chip-specific half of the configuration from the VGM header.
    ///
    /// Called with the config already carrying clock, sample-rate mode and the
    /// variant flag, so an implementation only sets what is its own.
    pub(crate) configure: fn(&mut DevConfig, &ChipSettings),
    /// Builds this chip, boxed, for the registry.
    ///
    /// The registry takes a bare `fn` pointer, which cannot capture a spec, so
    /// [`chip_specs!`] emits one of these per row, each naming its own
    /// [`ChipKind`] -- the whole reason the macro exists.
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
    pub(crate) const fn maker(&self) -> vgms_synth::CoreMaker {
        vgms_synth::CoreMaker::Generic(self.make)
    }
}

/// The configuration handed to `SndEmu_Start`.
///
/// libvgm's chips with settings define a struct whose first member is a
/// `DEV_GEN_CFG` and pass a pointer to it cast down. Modelling that as an enum
/// keeps field access type-checked; the cast at [`as_ptr`](Self::as_ptr) is
/// upstream's own, and `layout.rs` pins the prefix property it relies on.
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
    pub(crate) fn generic_mut(&mut self) -> &mut DevGenCfg {
        match self {
            Self::Generic(cfg) => cfg,
            Self::Sn76496(cfg) => &mut cfg.gen_cfg,
            Self::Ay8910(cfg) => &mut cfg.gen_cfg,
            Self::Msm6258(cfg) => &mut cfg.gen_cfg,
            Self::SegaPcm(cfg) => &mut cfg.gen_cfg,
        }
    }

    /// A pointer `SndEmu_Start` can read, whatever the real struct is.
    pub(crate) fn as_ptr(&self) -> *const DevGenCfg {
        match self {
            Self::Generic(cfg) => std::ptr::from_ref(cfg),
            Self::Sn76496(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::Ay8910(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::Msm6258(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
            Self::SegaPcm(cfg) => std::ptr::from_ref(cfg).cast::<DevGenCfg>(),
        }
    }
}

/// Declares the chip table and, per row, the bare `fn` the registry needs.
///
/// A registry entry is `(id, ChipKind, fn() -> Box<dyn ChipCore>)` and that last
/// one cannot be a closure over a spec, so each chip needs a function that names
/// its own kind. This way a chip is one line and cannot pair the wrong id with
/// the wrong device.
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
        /// A row must also have its device named in `build.rs`'s `ENABLED`, or
        /// the start fails and the chip is silent. A `static` not a `const`: the
        /// makers below take `&'static` references into it, which a `const`
        /// could not give them.
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
                // By id, not by kind: a chip with alternative cores has several
                // rows of one kind, and each maker must find its own.
                Box::new(LibVgmChip::new(spec_by_id($id)))
            }
        )*
    };
}

chip_specs! {
    // --- `emu_core` and the alternative rows ------------------------------
    //
    // A chip's first row is its default with the plain label; the `libvgm-<core>`
    // rows behind it publish the device's other emulators as picker entries. The
    // two named default selections (Maxim's SN76489, Ootake's HuC6280) are the
    // cores the reference ran.
    //
    // An alternate's `emu_core` must name a core *different* from the default's.
    // `emu_core: 0` takes the device's first-listed core (`SndEmu_StartCore`), so
    // an alternate naming that same first core is a dead picker entry -- both
    // rows start the one emulator. `every_alternate_row_starts_a_distinct_core`
    // pins that they differ.

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
    make_gameboydmg_mame: "gameboydmg.libvgm-mame" / "libvgm (MAME core)" => GameBoyDmg,
        ffi::DEVID_GB_DMG, ffi::FCC_MAME, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_pokey: "pokey.libvgm" / "libvgm" => Pokey,
        ffi::DEVID_POKEY, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_mikey: "mikey.libvgm" / "libvgm" => Mikey,
        ffi::DEVID_MIKEY, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;

    // Plain files with one upstream quirk each -- the remap is the rule's.
    make_nesapu: "nesapu.libvgm" / "libvgm" => NesApu,
        ffi::DEVID_NES_APU, 0, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_nesapu_mame: "nesapu.libvgm-mame" / "libvgm (MAME core)" => NesApu,
        ffi::DEVID_NES_APU, ffi::FCC_MAME, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_okim6295: "okim6295.libvgm" / "libvgm" => Okim6295,
        ffi::DEVID_MSM6295, 0, WriteRule::Okim6295, [0, 0], LEVEL_UNITY, configure_none;
    make_wonderswan: "wonderswan.libvgm" / "libvgm" => WonderSwan,
        ffi::DEVID_WSWAN, 0, WriteRule::WonderSwan, [0, 0], LEVEL_UNITY, configure_none;
    make_saa1099: "saa1099.libvgm" / "libvgm" => Saa1099,
        ffi::DEVID_SAA1099, 0, WriteRule::ReversedLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_saa1099_mame: "saa1099.libvgm-mame" / "libvgm (MAME core)" => Saa1099,
        ffi::DEVID_SAA1099, ffi::FCC_MAME, WriteRule::ReversedLatch, [0, 0], LEVEL_UNITY, configure_none;

    // The AY8910, with its `0x31` stereo mask on the dedicated function.
    make_ay8910: "ay8910.libvgm" / "libvgm" => Ay8910,
        ffi::DEVID_AY8910, 0, WriteRule::RegisterWithStereo, [0, 0], LEVEL_UNITY, configure_ay8910;
    make_ay8910_mame: "ay8910.libvgm-mame" / "libvgm (MAME core)" => Ay8910,
        ffi::DEVID_AY8910, ffi::FCC_MAME, WriteRule::RegisterWithStereo, [0, 0], 399, configure_ay8910;  // measured 1.559 (n=8, 0.5886..0.6938)

    // The Yamaha latch pair.
    make_ymz280b: "ymz280b.libvgm" / "libvgm" => Ymz280b,
        ffi::DEVID_YMZ280B, 0, WriteRule::RegisterLatch, [0, 0], 303, configure_none;  // measured 1.185 (n=12)
    make_k051649: "k051649.libvgm" / "libvgm" => K051649,
        ffi::DEVID_K051649, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // EMU2413 against the chip's Nuked-OPLL default. Anchored to that default
    // and not to the reference on purpose: the YM2413's own level sits at 0.370
    // of VGMPlay's, a shortfall the scorecard has carried as open since
    // 2026-07-28. Calibrating this row to the reference would put it 2.7x above
    // the chip's own default -- the same complaint, louder. **If that open item
    // is ever settled, this number moves with it.**
    make_ym2413: "ym2413.libvgm" / "libvgm" => Ym2413,
        ffi::DEVID_YM2413, 0, WriteRule::RegisterLatch, [0, 0], 385, configure_none;  // measured 1.504 (n=12 native; 0.246/0.370 vs the reference)
    // No number for the MAME core: it scatters 0.672..1.014 over eight files, so
    // one scalar does not describe it and a fitted constant would be a guess.
    make_ym2413_mame: "ym2413.libvgm-mame" / "libvgm (MAME core)" => Ym2413,
        ffi::DEVID_YM2413, ffi::FCC_MAME, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // With the YM2413 above, the three chips whose default is a Nuked core (see
    // `install_cores`) -- so the three where changing the core in Settings
    // swaps emulator *families*, and where the two disagree most about scale.
    // libvgm's YM2612 and YM2151 both render at almost exactly half of Nuked's:
    // left at unity, choosing libvgm for a Mega Drive rip's YM2612 dropped its
    // FM 6 dB under its own PSG.
    make_ym2612: "ym2612.libvgm" / "libvgm" => Ym2612,
        ffi::DEVID_YM2612, 0, WriteRule::RegisterLatch, [0, 0], 525, configure_none;  // measured 2.051 (0.466/0.955 vs the reference, n=12; 0.4877 direct, n=8)
    make_ym2612_gens: "ym2612.libvgm-gens" / "libvgm (Gens core)" => Ym2612,
        ffi::DEVID_YM2612, ffi::FCC_GENS, WriteRule::RegisterLatch, [0, 0], 516, configure_none;  // measured 2.016 (n=8, 0.4744..0.5093)
    make_ym2151: "ym2151.libvgm" / "libvgm" => Ym2151,
        ffi::DEVID_YM2151, 0, WriteRule::RegisterLatch, [0, 0], 514, configure_none;  // measured 2.008 (0.498/1.000 vs the reference, n=12; 0.4973 direct, n=8)
    make_ymf271: "ymf271.libvgm" / "libvgm" => Ymf271,
        ffi::DEVID_YMF271, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // The OPL4: its wave half is this device, its FM half a linked YMF262.
    // Not an OPL row -- the OPL family's own chips stay on `PlayerEngine`.
    // Known gap: rips that lean on the YRW801 wave ROM without embedding it
    // (some MSX MoonSound rips) play only their FM half -- VGMPlay side-loads
    // `yrw801.rom` from disk, and that ROM is not ours to ship.
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
    // Exactly a quarter: the files that correlate at 1.0000 measure lvl
    // 4.0000 against the reference, which is VGMPlay's own `_CHIP_VOLUME`
    // entry for the chip (0x40). At unity this was the "500GP is far too
    // loud" report -- a single-C352 rip clipping the mix on its own.
    make_c352: "c352.libvgm" / "libvgm" => C352,
        ffi::DEVID_C352, 0, WriteRule::RegisterAddr16Data16, [0, 0], 64, configure_c352;  // measured 4.000 (n=12, corr-1.0 rows exact; range 3.21..4.00)
    make_qsound: "qsound.libvgm" / "libvgm" => QSound,
        ffi::DEVID_QSOUND, 0, WriteRule::QSound, [0, 0], LEVEL_UNITY, configure_qsound;
    make_qsound_mame: "qsound.libvgm-mame" / "libvgm (MAME core)" => QSound,
        ffi::DEVID_QSOUND, ffi::FCC_MAME, WriteRule::QSound, [0, 0], LEVEL_UNITY, configure_qsound;
    // A register file plus a second command that is not a register write:
    // `0xB5` and `0xC3`, which upstream splits between `Cmd_Ofs8_Data8` and
    // `Cmd_YMW_Bank`.
    make_multipcm: "multipcm.libvgm" / "libvgm" => MultiPcm,
        ffi::DEVID_YMW258, 0, WriteRule::MultiPcmBank, [0, 0], LEVEL_UNITY, configure_multipcm;
    make_pwm: "pwm.libvgm" / "libvgm" => Pwm,
        ffi::DEVID_32X_PWM, 0, WriteRule::Data16, [0, 0], LEVEL_UNITY, configure_none;
    // No ES5505/ES5506 row: libvgm's `es5506.c` is a stub (a `DEV_DECL` whose
    // core list is `{ NULL }`), so `SndEmu_Start` has nothing to start. The
    // decoder's `0xBE`/`0xD6` conventions are ready for when upstream grows it.

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

/// A chip whose configuration is only the generic fields (clock, rate mode,
/// variant flag), all set before this is called.
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
/// Every field changes what the part *is* rather than how it sounds; a wrong one
/// gives the noise channel a different pseudo-random sequence entirely.
///
/// **Transcribed from libvgm's own `player/vgmplayer.cpp`**, not the VGM
/// specification -- the player is the authority because it is the code the
/// reference measurement runs. Getting a field wrong is silent: the chip still
/// starts and sounds, as a different part.
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

    /// The header-to-config mapping is libvgm's player's, field for field.
    ///
    /// Pinned because getting it wrong is *silent*: every field selects a
    /// different real part, and the chip starts and sounds either way. Expected
    /// values are read off `player/vgmplayer.cpp`'s `DEVID_SN76496` arm.
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

    /// Registry ids are unique and slot-prefixed, as `vgms-synth` requires --
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
}
