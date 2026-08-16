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

    // x4 by the same staging the default was measured against: the sweep read
    // the chip's default (Nuked-PSG, its own row in vgms-cores-gpl) at lvl
    // 0.247 vs the reference, and the agreement run puts these two at medians
    // 1.10 / 1.12 of that default -- gross level shared, so the x4 rides
    // along. Their residual 10-12% scatters 1.5x across files (the noise
    // band) and is left uncorrected deliberately; the round 1024 is VGMPlay's
    // staging (0x80 doubled twice = 2.0) over the survey's raw-half note
    // ("their SN76489 measured 2x off the table derivation").
    make_sn76489: "sn76489.libvgm" / "libvgm (Maxim)" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAXM, WriteRule::Register, [0, 0], 1024, configure_sn76496;  // x4 with the default; median 1.10 of it (n=12, 0.91..1.44)
    make_sn76489_mame: "sn76489.libvgm-mame" / "libvgm (MAME)" => Sn76489,
        ffi::DEVID_SN76496, ffi::FCC_MAME, WriteRule::Register, [0, 0], 1024, configure_sn76496;  // x4 with the default; median 1.12 of it (n=12, 0.94..1.46)
    make_huc6280: "huc6280.libvgm" / "libvgm (Ootake)" => HuC6280,
        ffi::DEVID_C6280, ffi::FCC_OOTK, WriteRule::Register, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 1.0000, n=12)
    make_huc6280_mame: "huc6280.libvgm-mame" / "libvgm (MAME)" => HuC6280,
        ffi::DEVID_C6280, ffi::FCC_MAME, WriteRule::Register, [0, 0], 512, configure_none;  // x2 with the default; median 1.047 of it (n=12)

    // Plain 8-bit register files: `Cmd_Ofs8_Data8` upstream.
    make_k053260: "k053260.libvgm" / "libvgm (MAME)" => K053260,
        ffi::DEVID_K053260, 0, WriteRule::Register, [0, 0], 494, configure_none;  // measured 1.930 (n=6)
    make_ga20: "ga20.libvgm" / "libvgm (MAME)" => Ga20,
        ffi::DEVID_GA20, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    // One single-chip corpus file, but it reads corr 1.0000 and its lvl 0.448
    // is the chip's staging arithmetic (0x11E doubled = 2.234) to three
    // decimals -- measurement and derivation co-sign the constant.
    make_upd7759: "upd7759.libvgm" / "libvgm (MAME)" => Upd7759,
        ffi::DEVID_UPD7759, 0, WriteRule::Register, [0, 0], 572, configure_none;  // measured 2.232 (lvl 0.448, corr 1.0000, n=1)
    make_okim6258: "okim6258.libvgm" / "libvgm (MAME)" => Okim6258,
        ffi::DEVID_MSM6258, 0, WriteRule::Register, [0, 0], 219, configure_msm6258;  // measured 0.855 (lvl 1.170, corr 0.9766, n=9); at the flag-configured native rate lvl reads 0.978 (corr 1.0000, n=9)
    // `Cmd_Port_Ofs8_Data8`: the port selects nothing on the write itself.
    // The 64 is VGMPlay's staging (`_CHIP_VOLUME` 0x40 = 0.25) and the
    // measurement lands on it to within 1% -- only measurable at all once the
    // dynamic-rate fix held (the oscillator-enable register moves the chip's
    // output rate; with the resampler stuck at the reset rate the sweep read
    // corr 0.0022, unrelated waveforms, and its lvl meant nothing).
    make_es5503: "es5503.libvgm" / "libvgm (MAME)" => Es5503,
        ffi::DEVID_ES5503, 0, WriteRule::Register, [0, 0], 64, configure_es5503;  // measured 0.252 (lvl 3.976, corr 0.9912, n=12, native rate); post-fix lvl 1.005, corr 0.9944
    make_gameboydmg: "gameboydmg.libvgm" / "libvgm (SameBoy)" => GameBoyDmg,
        ffi::DEVID_GB_DMG, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    make_gameboydmg_mame: "gameboydmg.libvgm-mame" / "libvgm (MAME)" => GameBoyDmg,
        ffi::DEVID_GB_DMG, ffi::FCC_MAME, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;
    // Measured at output rate: the MAME POKEY renders at its ~1.79 MHz clock
    // and the harness's pitch search is quadratic in rate, so a native-rate
    // comparison costs hours per file. The resampled corr is meaningless but
    // the RMS ratio is honest, and it lands on the staging arithmetic
    // (0x100 doubled) to three decimals.
    make_pokey: "pokey.libvgm" / "libvgm (MAME)" => Pokey,
        ffi::DEVID_POKEY, 0, WriteRule::Register, [0, 0], 512, configure_none;  // measured 1.996 (lvl 0.501, output-rate, n=12)
    make_mikey: "mikey.libvgm" / "libvgm (laoo)" => Mikey,
        ffi::DEVID_MIKEY, 0, WriteRule::Register, [0, 0], LEVEL_UNITY, configure_none;

    // Plain files with one upstream quirk each -- the remap is the rule's.
    make_nesapu: "nesapu.libvgm" / "libvgm (NSFPlay)" => NesApu,
        ffi::DEVID_NES_APU, 0, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_nesapu_mame: "nesapu.libvgm-mame" / "libvgm (MAME)" => NesApu,
        ffi::DEVID_NES_APU, ffi::FCC_MAME, WriteRule::NesApu, [0, 0], LEVEL_UNITY, configure_none;
    make_okim6295: "okim6295.libvgm" / "libvgm (MAME)" => Okim6295,
        ffi::DEVID_MSM6295, 0, WriteRule::Okim6295, [0, 0], LEVEL_UNITY, configure_none;
    make_wonderswan: "wonderswan.libvgm" / "libvgm" => WonderSwan,
        ffi::DEVID_WSWAN, 0, WriteRule::WonderSwan, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 0.9888, n=12)
    // lvl reads exactly 0.500 and the collapsed-but-consistent gain (1.878)
    // concurs; the 0.85 correlation is the noise generators' phase, not scale.
    make_saa1099: "saa1099.libvgm" / "libvgm (Valley Bell)" => Saa1099,
        ffi::DEVID_SAA1099, 0, WriteRule::ReversedLatch, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 0.8471, n=12)
    make_saa1099_mame: "saa1099.libvgm-mame" / "libvgm (MAME)" => Saa1099,
        ffi::DEVID_SAA1099, ffi::FCC_MAME, WriteRule::ReversedLatch, [0, 0], 512, configure_none;  // x2 with the default; median 1.004 of it (n=12)

    // The AY8910, with its `0x31` stereo mask on the dedicated function.
    make_ay8910: "ay8910.libvgm" / "libvgm (EMU2149)" => Ay8910,
        ffi::DEVID_AY8910, 0, WriteRule::RegisterWithStereo, [0, 0], LEVEL_UNITY, configure_ay8910;
    make_ay8910_mame: "ay8910.libvgm-mame" / "libvgm (MAME)" => Ay8910,
        ffi::DEVID_AY8910, ffi::FCC_MAME, WriteRule::RegisterWithStereo, [0, 0], 399, configure_ay8910;  // measured 1.559 (n=8, 0.5886..0.6938)

    // The Yamaha latch pair.
    make_ymz280b: "ymz280b.libvgm" / "libvgm (MAME)" => Ymz280b,
        ffi::DEVID_YMZ280B, 0, WriteRule::RegisterLatch, [0, 0], 303, configure_none;  // measured 1.185 (n=12)
    make_k051649: "k051649.libvgm" / "libvgm (MAME)" => K051649,
        ffi::DEVID_K051649, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // EMU2413 against the chip's Nuked-OPLL default. Anchored to that default
    // and not to the reference on purpose: the YM2413's own level sits at 0.370
    // of VGMPlay's, a shortfall the scorecard has carried as open since
    // 2026-07-28. Calibrating this row to the reference would put it 2.7x above
    // the chip's own default -- the same complaint, louder. **If that open item
    // is ever settled, this number moves with it.**
    make_ym2413: "ym2413.libvgm" / "libvgm (EMU2413)" => Ym2413,
        ffi::DEVID_YM2413, 0, WriteRule::RegisterLatch, [0, 0], 385, configure_none;  // measured 1.504 (n=12 native; 0.246/0.370 vs the reference)
    // No number for the MAME core: it scatters 0.672..1.014 over eight files, so
    // one scalar does not describe it and a fitted constant would be a guess.
    make_ym2413_mame: "ym2413.libvgm-mame" / "libvgm (MAME)" => Ym2413,
        ffi::DEVID_YM2413, ffi::FCC_MAME, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // With the YM2413 above, the three chips whose default is a Nuked core (see
    // `install_cores`) -- so the three where changing the core in Settings
    // swaps emulator *families*, and where the two disagree most about scale.
    // libvgm's YM2612 and YM2151 both render at almost exactly half of Nuked's:
    // left at unity, choosing libvgm for a Mega Drive rip's YM2612 dropped its
    // FM 6 dB under its own PSG.
    make_ym2612: "ym2612.libvgm" / "libvgm (Genesis Plus GX)" => Ym2612,
        ffi::DEVID_YM2612, 0, WriteRule::RegisterLatch, [0, 0], 525, configure_none;  // measured 2.051 (0.466/0.955 vs the reference, n=12; 0.4877 direct, n=8)
    make_ym2612_gens: "ym2612.libvgm-gens" / "libvgm (Gens)" => Ym2612,
        ffi::DEVID_YM2612, ffi::FCC_GENS, WriteRule::RegisterLatch, [0, 0], 516, configure_none;  // measured 2.016 (n=8, 0.4744..0.5093)
    make_ym2151: "ym2151.libvgm" / "libvgm (MAME)" => Ym2151,
        ffi::DEVID_YM2151, 0, WriteRule::RegisterLatch, [0, 0], 514, configure_none;  // measured 2.008 (0.498/1.000 vs the reference, n=12; 0.4973 direct, n=8)
    // The Y8950, served from this crate (owner's decision, 2026-08-12): MAME
    // fmopl is the only core with the Y8950's ADPCM-B
    // (delta-T) half -- the Nuked/LLE adapters have no sample unit, so the
    // speech half of every Y8950 rip was silent. The reference always plays
    // this same core, so the pairing is also the parity pairing. Level unity
    // until the harness measures it; `0x88` blocks land through the default
    // memory space (`y8950_alloc_pcmrom`/`y8950_write_pcmrom`, user 0).
    //
    // The id lives in the OPL family's config slot (`opl3.*`), as every OPL
    // row must: config stores one choice per slot. Registered for the Y8950
    // alone, so if a user picks it for the *family*, each chip resolves it
    // per chip -- the Y8950 gets this core, the others fall back to their
    // default. That per-chip fallback is `CoreRegistry::resolve`'s.
    make_y8950: "opl3.libvgm-y8950" / "libvgm (MAME + ADPCM)" => Y8950,
        ffi::DEVID_Y8950, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    // The YM3526 shares the OPL family's `opl3.*` slot, registered for the
    // YM3526 alone (per-chip resolve, exactly as the Y8950 above). Not the
    // ADPCM story -- the YM3526 is pure FM the OPL adapter plays -- but the
    // parity story: the reference offers the YM3526 no Nuked option and always
    // plays MAME fmopl, so the OPL adapter's Nuked-OPL3 is a cross-core
    // comparison (corr 0.75 with cents ~0 -- a waveform difference, not pitch).
    // This row is that same fmopl, the reference's own core. Level measured
    // against the harness (unity until then).
    make_ym3526: "opl3.libvgm-ym3526" / "libvgm (MAME)" => Ym3526,
        ffi::DEVID_YM3526, 0, WriteRule::RegisterLatch, [0, 0], LEVEL_UNITY, configure_none;
    make_ymf271: "ymf271.libvgm" / "libvgm (MAME)" => Ymf271,
        ffi::DEVID_YMF271, 0, WriteRule::RegisterLatch, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 1.0000, n=12)
    // The OPL4: its wave half is this device, its FM half a linked YMF262.
    // Not an OPL row -- the OPL family's own chips stay on our OPL adapter path.
    // Known gap: rips that lean on the YRW801 wave ROM without embedding it
    // (some MSX MoonSound rips) play only their FM half -- VGMPlay side-loads
    // `yrw801.rom` from disk, and that ROM is not ours to ship.
    make_ymf278b: "ymf278b.libvgm" / "libvgm (openMSX)" => Ymf278b,
        ffi::DEVID_YMF278B, 0, WriteRule::RegisterLatch, [0x524F, 0x5241], LEVEL_UNITY, configure_none;

    // The OPN family: the latch pair, a linked SSG, and the YM2203's stereo
    // mask riding the SSG's own function.
    // Same story as the YM2612/YM2151 rows above: libvgm's build of the MAME
    // core renders at almost exactly half the reference's level. Found via
    // "Cameltry" (YM2203+OKIM6295), where the FM sat 6 dB under the OKI.
    make_ym2203: "ym2203.libvgm" / "libvgm (MAME)" => Ym2203,
        ffi::DEVID_YM2203, 0, WriteRule::OpnFamily, [0, 0], 504, configure_none;  // measured 1.969 (lvl 0.508, corr 0.9999, n=12)
    make_ym2608: "ym2608.libvgm" / "libvgm (MAME)" => Ym2608,
        ffi::DEVID_YM2608, 0, WriteRule::OpnFamily, [0x41, 0x42], LEVEL_UNITY, configure_none;
    make_ym2610: "ym2610.libvgm" / "libvgm (MAME)" => Ym2610,
        ffi::DEVID_YM2610, 0, WriteRule::OpnFamily, [0x41, 0x42], LEVEL_UNITY, configure_none;

    // Memory-space writes with the address arriving whole (`0xC0`, `0xC5`,
    // `0xC7`, `0xC8`).
    make_segapcm: "segapcm.libvgm" / "libvgm (MAME)" => SegaPcm,
        ffi::DEVID_SEGAPCM, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_segapcm;
    make_x1010: "x1010.libvgm" / "libvgm (MAME)" => X1010,
        ffi::DEVID_X1_010, 0, WriteRule::Memory, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 1.0000, n=12)
    make_vsu: "vsu.libvgm" / "libvgm (Mednafen)" => Vsu,
        ffi::DEVID_VBOY_VSU, 0, WriteRule::Memory, [0, 0], 512, configure_none;  // measured 2.000 (lvl 0.500, corr 1.0000, n=12)
    make_scsp: "scsp.libvgm" / "libvgm (MAME)" => Scsp,
        ffi::DEVID_SCSP, 0, WriteRule::Memory, [0, 0], LEVEL_UNITY, configure_scsp;

    // ...and with it split across our `port`/`addr` (`0xD3`, `0xD4`).
    make_c140: "c140.libvgm" / "libvgm (MAME)" => C140,
        ffi::DEVID_C140, 0, WriteRule::MemoryPortHigh, [0, 0], 332, configure_c140;  // measured 1.297 (n=12)
    make_k054539: "k054539.libvgm" / "libvgm (MAME)" => K054539,
        ffi::DEVID_K054539, 0, WriteRule::MemoryPortHigh, [0, 0], 512, configure_k054539;  // measured 2.000 (lvl 0.500, corr 0.9987, n=12)

    // The one-off shapes: the three the plan named, the MultiPCM's bank and
    // the PWM's 12-bit values.
    // Exactly a quarter: the files that correlate at 1.0000 measure lvl
    // 4.0000 against the reference, which is VGMPlay's own `_CHIP_VOLUME`
    // entry for the chip (0x40). At unity this was the "500GP is far too
    // loud" report -- a single-C352 rip clipping the mix on its own.
    make_c352: "c352.libvgm" / "libvgm (superctr)" => C352,
        ffi::DEVID_C352, 0, WriteRule::RegisterAddr16Data16, [0, 0], 64, configure_c352;  // measured 4.000 (n=12, corr-1.0 rows exact; range 3.21..4.00)
    make_qsound: "qsound.libvgm" / "libvgm (superctr)" => QSound,
        ffi::DEVID_QSOUND, 0, WriteRule::QSound, [0, 0], 512, configure_qsound;  // measured 2.000 (lvl 0.500, corr 1.0000, n=12)
    make_qsound_mame: "qsound.libvgm-mame" / "libvgm (MAME)" => QSound,
        ffi::DEVID_QSOUND, ffi::FCC_MAME, WriteRule::QSound, [0, 0], 512, configure_qsound;  // x2 with the default; median 0.995 of it (n=12)
    // A register file plus a second command that is not a register write:
    // `0xB5` and `0xC3`, which upstream splits between `Cmd_Ofs8_Data8` and
    // `Cmd_YMW_Bank`.
    // The one row the sweep found too LOUD: VGMPlay stages the MultiPCM down
    // (volume 0x40, one doubling = 0.5), so unity here sat 2x above it.
    make_multipcm: "multipcm.libvgm" / "libvgm (MAME)" => MultiPcm,
        ffi::DEVID_YMW258, 0, WriteRule::MultiPcmBank, [0, 0], 128, configure_multipcm;  // measured 0.501 (lvl 1.998, corr 0.9999, n=12)
    make_pwm: "pwm.libvgm" / "libvgm (Gens)" => Pwm,
        ffi::DEVID_32X_PWM, 0, WriteRule::Data16, [0, 0], LEVEL_UNITY, configure_none;
    // No ES5505/ES5506 row: libvgm's `es5506.c` is a stub (a `DEV_DECL` whose
    // core list is `{ NULL }`), so `SndEmu_Start` has nothing to start. The
    // decoder's `0xBE`/`0xD6` conventions are ready for when upstream grows it.

    // libvgm renders the RF5C68 device at ~0.36 of VGMPlay 0.52's level:
    // measured lvl 0.364 (n=12, corr 1.0000 -- a pure scale difference) over
    // single-chip RF5C68 corpus files, so `256/0.364 = 703` brings it up to the
    // reference (re-verified lvl 0.999 in the 2026-08-12 sweep). The 68 rows
    // only: the 0.364 is the *reference's staging* of the 68 (0xB0 doubled
    // twice), not a property of the shared device, so the 164 rows below keep
    // their own number.
    make_rf5c68: "rf5c68.libvgm" / "libvgm (MAME)" => Rf5c68,
        ffi::DEVID_RF5C68, 0, WriteRule::RegisterOrMemoryByPort, [0, 0], 703, configure_rf5c68;
    make_rf5c68_gens: "rf5c68.libvgm-gens" / "libvgm (Gens)" => Rf5c68,
        ffi::DEVID_RF5C68, ffi::FCC_GENS, WriteRule::RegisterOrMemoryByPort, [0, 0], 703, configure_rf5c68;
    // The same device, but the Gens core leads: every RF5C164 player in the
    // lineage runs `scd_pcm.c` for it -- libvgm's own player forces FCC_GENS
    // when `flags == 1` (vgmplayer.cpp), and the pinned reference (legacy
    // VGMPlay 0.52) has no other 164 core at all. The two cores also read the
    // sign-magnitude sample bytes with OPPOSITE polarity (MAME: bit 7 set
    // adds; Gens: bit 7 set subtracts), so the MAME row against the Gens
    // reference measured corr 0.2605 at fit gain -0.963 -- the inversion
    // signature the 2026-08-12 sweep flagged OPEN. (`flags` itself is inert
    // inside both cores; core choice is the real 68-vs-164 switch.)
    //
    // Level does NOT inherit the 68's 703: VGMPlay stages the two differently
    // (RF5C68 0xB0 doubled twice = 2.75, RF5C164 0x80 doubled once = 1.0), so
    // the shared raw scale that leaves the 68 at 0.364 leaves the 164 at
    // unity. The 2026-08-08 fix copied 703 onto all four rows and put the 164
    // at lvl 2.649 (measured, n=12) -- the sweep caught it 2.65x hot, and
    // 703/2.649 = 265 lands on this row's derived 256.
    make_rf5c164: "rf5c164.libvgm" / "libvgm (Gens)" => Rf5c164,
        ffi::DEVID_RF5C68, ffi::FCC_GENS, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c164;  // measured: 703 read lvl 2.649 (n=12), so unity
    make_rf5c164_mame: "rf5c164.libvgm-mame" / "libvgm (MAME)" => Rf5c164,
        ffi::DEVID_RF5C68, ffi::FCC_MAME, WriteRule::RegisterOrMemoryByPort, [0, 0], LEVEL_UNITY, configure_rf5c164;
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

/// Below this clock a QSound header carries the old 4 MHz serial clock rather
/// than the 60 MHz DSP clock -- the single trigger shared by the `* 15` rescale
/// in [`configure_qsound`] and `chip.rs`'s `Cmd_QSound_Reg` key-on hacks
/// (upstream's `hdrClock < devCfg->clock`).
pub(crate) const QSOUND_OLD_CLOCK_MAX_HZ: u32 = 5_000_000;

/// The QSound's clock rescue: old logs stored the 4 MHz serial clock where
/// the 60 MHz DSP clock belongs.
fn configure_qsound(config: &mut DevConfig, _settings: &ChipSettings) {
    let generic = config.generic_mut();
    if generic.clock < QSOUND_OLD_CLOCK_MAX_HZ {
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
    ///
    /// The prefix is the chip's *config slot*, which for most chips is its own
    /// slug -- but an OPL-family chip (the Y8950) shares the family slot, so
    /// its row takes the alternate-core shape there (`opl3.libvgm-y8950`) and
    /// is exempt from the plain-default-first rule: the family's plain default
    /// is the built-ins' Nuked-OPL3 row, not one of ours.
    #[test]
    fn every_spec_has_a_well_formed_unique_id() {
        let mut seen = std::collections::BTreeSet::new();
        let mut first_of: std::collections::BTreeMap<&str, &str> =
            std::collections::BTreeMap::new();
        for spec in SPECS {
            assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
            let slot = vgms_synth::registry::slot_slug(spec.kind);
            let default_id = format!("{slot}.{}", crate::CORE_SUFFIX);
            assert!(
                spec.id == default_id
                    || spec.id.starts_with(&format!("{default_id}-"))
                    || spec
                        .id
                        .starts_with(&format!("{slot}.{}-", crate::CORE_SUFFIX)),
                "{} must be <slot>.libvgm or <slot>.libvgm-<core>",
                spec.id
            );
            if slot == spec.kind.slug() {
                first_of.entry(spec.kind.slug()).or_insert(spec.id);
            }
        }
        // The first row seen for every own-slot chip is its plain default id.
        for (slug, first_id) in first_of {
            assert_eq!(
                first_id,
                format!("{slug}.{}", crate::CORE_SUFFIX),
                "{slug}'s default row must precede its alternates"
            );
        }
    }
}
