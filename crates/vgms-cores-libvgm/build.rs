//! Compiles the pinned libvgm submodule -- the framework, plus whichever sound
//! devices [`ENABLED`] names.
//!
//! libvgm has no CMake-free build of its own, so the device-to-source mapping in
//! `emu/CMakeLists.txt` is transcribed here as data ([`DEVICES`], complete
//! whether or not we compile a given chip). Turning a chip on is a one-line edit
//! to [`ENABLED`].
//!
//! The submodule is never edited (the `crates/vgms-synth/PROVENANCE.md` policy):
//! upgrading is a pull, a pin bump and a corpus re-run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

/// The framework: what every device needs, and nothing more.
///
/// `Resampler.c` is absent -- we start devices in `DEVRI_SRMODE_NATIVE` and
/// resample with `vgms_synth::resample`. `dac_control.c` is a VGM-player feature
/// (the `0x90`-`0x95` DAC stream commands) our engine implements itself.
/// Paths here and in [`DEVICES`] are relative to `emu/`, exactly as upstream's
/// `emu/CMakeLists.txt` writes them, so the two diff line for line at a pin bump.
const FRAMEWORK: [&str; 3] = ["SoundEmu.c", "logging.c", "panning.c"];

/// One of libvgm's sound devices: its `SNDDEV_` switch, the sources that are
/// unconditional once it is on, and its selectable emulation cores.
struct Device {
    /// The `SNDDEV_` suffix, e.g. `"SN76496"`. Also this row's key in
    /// [`ENABLED`].
    key: &'static str,
    /// Sources compiled whenever this device is on -- the `*intf.c` dispatcher
    /// and, for devices with only one core, that core too.
    shared: &'static [&'static str],
    /// `(EC_ define, sources)` per selectable core. Empty for a
    /// single-core device.
    cores: &'static [(&'static str, &'static [&'static str])],
}

/// Every device libvgm ships, transcribed from `emu/CMakeLists.txt`.
///
/// Sources repeat across rows on purpose (`fmopn.c` serves four devices) and
/// [`main`] collects them into a set, so a file compiles once however many
/// enabled devices claim it.
///
/// `YM2414` (OPZ) is the one row missing: libvgm's only C++ core, and this
/// build is C-only.
const DEVICES: &[Device] = &[
    Device {
        key: "SN76496",
        shared: &["cores/sn764intf.c"],
        cores: &[
            ("EC_SN76496_MAME", &["cores/sn76496.c"]),
            ("EC_SN76496_MAXIM", &["cores/sn76489.c"]),
        ],
    },
    Device {
        key: "YM2413",
        shared: &["cores/2413intf.c"],
        cores: &[
            ("EC_YM2413_MAME", &["cores/ym2413.c"]),
            ("EC_YM2413_EMU2413", &["cores/emu2413.c"]),
            ("EC_YM2413_NUKED", &["cores/nukedopll.c"]),
        ],
    },
    Device {
        key: "YM2612",
        shared: &["cores/2612intf.c"],
        cores: &[
            ("EC_YM2612_GPGX", &["cores/fmopn.c"]),
            ("EC_YM2612_GENS", &["cores/ym2612.c"]),
            ("EC_YM2612_NUKED", &["cores/ym3438.c"]),
        ],
    },
    Device {
        key: "YM2151",
        shared: &["cores/2151intf.c"],
        cores: &[
            ("EC_YM2151_MAME", &["cores/ym2151.c"]),
            ("EC_YM2151_NUKED", &["cores/nukedopm.c"]),
        ],
    },
    Device {
        key: "SEGAPCM",
        shared: &["cores/segapcm.c"],
        cores: &[],
    },
    Device {
        key: "RF5C68",
        shared: &["cores/rf5cintf.c"],
        cores: &[
            ("EC_RF5C68_MAME", &["cores/rf5c68.c"]),
            ("EC_RF5C68_GENS", &["cores/scd_pcm.c"]),
        ],
    },
    Device {
        key: "YM2203",
        shared: &["cores/opnintf.c", "cores/fmopn.c"],
        cores: &[],
    },
    // The 2608 and 2610 add Delta-T to the same OPN dispatcher.
    Device {
        key: "YM2608",
        shared: &["cores/opnintf.c", "cores/fmopn.c", "cores/ymdeltat.c"],
        cores: &[],
    },
    Device {
        key: "YM2610",
        shared: &["cores/opnintf.c", "cores/fmopn.c", "cores/ymdeltat.c"],
        cores: &[],
    },
    Device {
        key: "YM3812",
        shared: &["cores/oplintf.c"],
        cores: &[
            ("EC_YM3812_MAME", &["cores/fmopl.c"]),
            ("EC_YM3812_ADLIBEMU", &["cores/adlibemu_opl2.c"]),
            ("EC_YM3812_NUKED", &["cores/nukedopl3.c"]),
        ],
    },
    Device {
        key: "YM3526",
        shared: &["cores/oplintf.c", "cores/fmopl.c"],
        cores: &[],
    },
    Device {
        key: "Y8950",
        shared: &["cores/oplintf.c", "cores/fmopl.c", "cores/ymdeltat.c"],
        cores: &[],
    },
    Device {
        key: "YMF262",
        shared: &["cores/262intf.c"],
        cores: &[
            ("EC_YMF262_MAME", &["cores/ymf262.c"]),
            ("EC_YMF262_ADLIBEMU", &["cores/adlibemu_opl3.c"]),
            ("EC_YMF262_NUKED", &["cores/nukedopl3.c"]),
        ],
    },
    // Needs YMF262 enabled alongside it: the OPL4's FM half *is* an OPL3.
    Device {
        key: "YMF278B",
        shared: &["cores/ymf278b.c"],
        cores: &[],
    },
    Device {
        key: "YMZ280B",
        shared: &["cores/ymz280b.c"],
        cores: &[],
    },
    Device {
        key: "YMF271",
        shared: &["cores/ymf271.c"],
        cores: &[],
    },
    Device {
        key: "AY8910",
        shared: &["cores/ayintf.c"],
        cores: &[
            ("EC_AY8910_MAME", &["cores/ay8910.c"]),
            ("EC_AY8910_EMU2149", &["cores/emu2149.c"]),
        ],
    },
    Device {
        key: "32X_PWM",
        shared: &["cores/pwm.c"],
        cores: &[],
    },
    Device {
        key: "GAMEBOY",
        shared: &["cores/gbintf.c"],
        cores: &[
            ("EC_GB_MAME", &["cores/gb_mame.c"]),
            ("EC_GB_SAMEBOY", &["cores/sameboy_apu.c"]),
        ],
    },
    Device {
        key: "NES_APU",
        shared: &["cores/nesintf.c"],
        cores: &[
            ("EC_NES_MAME", &["cores/nes_apu.c"]),
            (
                "EC_NES_NSFPLAY",
                &["cores/np_nes_apu.c", "cores/np_nes_dmc.c"],
            ),
            // Upstream: the FDS core needs an APU core, but pairs with either.
            ("EC_NES_NSFP_FDS", &["cores/np_nes_fds.c"]),
        ],
    },
    Device {
        key: "YMW258",
        shared: &["cores/multipcm.c"],
        cores: &[],
    },
    Device {
        key: "UPD7759",
        shared: &["cores/upd7759.c"],
        cores: &[],
    },
    Device {
        key: "MSM6258",
        shared: &["cores/okim6258.c"],
        cores: &[],
    },
    Device {
        key: "MSM6295",
        shared: &["cores/okim6295.c", "cores/okiadpcm.c"],
        cores: &[],
    },
    Device {
        key: "K051649",
        shared: &["cores/k051649.c"],
        cores: &[],
    },
    Device {
        key: "K054539",
        shared: &["cores/k054539.c"],
        cores: &[],
    },
    Device {
        key: "C6280",
        shared: &["cores/c6280intf.c"],
        cores: &[
            ("EC_C6280_MAME", &["cores/c6280_mame.c"]),
            ("EC_C6280_OOTAKE", &["cores/Ootake_PSG.c"]),
        ],
    },
    Device {
        key: "C140",
        shared: &["cores/c140.c"],
        cores: &[],
    },
    Device {
        key: "C219",
        shared: &["cores/c219.c"],
        cores: &[],
    },
    Device {
        key: "K053260",
        shared: &["cores/k053260.c"],
        cores: &[],
    },
    Device {
        key: "POKEY",
        shared: &["cores/pokey.c"],
        cores: &[],
    },
    Device {
        key: "QSOUND",
        shared: &["cores/qsoundintf.c"],
        cores: &[
            ("EC_QSOUND_MAME", &["cores/qsound_mame.c"]),
            ("EC_QSOUND_CTR", &["cores/qsound_ctr.c"]),
        ],
    },
    Device {
        key: "SCSP",
        shared: &["cores/scsp.c", "cores/scspdsp.c"],
        cores: &[],
    },
    Device {
        key: "WSWAN",
        shared: &["cores/ws_audio.c"],
        cores: &[],
    },
    Device {
        key: "VBOY_VSU",
        shared: &["cores/vsu.c"],
        cores: &[],
    },
    Device {
        key: "SAA1099",
        shared: &["cores/saaintf.c"],
        cores: &[
            ("EC_SAA1099_MAME", &["cores/saa1099_mame.c"]),
            // `EC_SAA1099_NRS` is a define upstream but its source is commented
            // out of the CMake list, so there is nothing to compile.
            ("EC_SAA1099_VB", &["cores/saa1099_vb.c"]),
        ],
    },
    Device {
        key: "ES5503",
        shared: &["cores/es5503.c"],
        cores: &[],
    },
    Device {
        key: "ES5506",
        shared: &["cores/es5506.c"],
        cores: &[],
    },
    Device {
        key: "X1_010",
        shared: &["cores/x1_010.c"],
        cores: &[],
    },
    Device {
        key: "C352",
        shared: &["cores/c352.c"],
        cores: &[],
    },
    Device {
        key: "GA20",
        shared: &["cores/iremga20.c"],
        cores: &[],
    },
    Device {
        key: "MIKEY",
        shared: &["cores/mikey.c"],
        cores: &[],
    },
    Device {
        key: "K007232",
        shared: &["cores/k007232.c"],
        cores: &[],
    },
    Device {
        key: "K005289",
        shared: &["cores/k005289.c"],
        cores: &[],
    },
    Device {
        key: "MSM5205",
        shared: &["cores/msm5205.c"],
        cores: &[],
    },
    Device {
        key: "MSM5232",
        shared: &["cores/msm5232.c"],
        cores: &[],
    },
    Device {
        key: "BSMT2000",
        shared: &["cores/bsmt2000.c"],
        cores: &[],
    },
    Device {
        key: "ICS2115",
        shared: &["cores/ics2115.c"],
        cores: &[],
    },
];

/// The devices this build compiles.
///
/// **This list and `chip.rs`'s `chip_specs!` must agree**: a spec whose device
/// is missing here starts nothing and is silently silent, which
/// `every_spec_can_actually_start` asserts against.
///
/// This is every device our corpus can name, libvgm being the default core for
/// all of them.
///
/// Still absent, deliberately:
///
/// - **The OPL family as chips of their own** (`YM3812`, `YM3526`, `Y8950`) --
///   out of scope by the owner's decision: OPL plays through `PlayerEngine`.
///   `YMF262` is compiled only as the OPL4's linked FM half; no OPL chip is
///   registered from this crate.
/// - **C219** rides the `C140` spec: the header's type byte picks the device
///   at start.
/// - **ES5505/ES5506**: upstream's `es5506.c` is a stub (a `DEV_DECL` whose
///   core list is `{ NULL }`) that `SndEmu_Start` cannot start.
/// - Devices no VGM command reaches in our decoder yet: K007232, K005289,
///   MSM5205, MSM5232, BSMT2000, ICS2115 (and the C++-only YM2414).
const ENABLED: &[&str] = &[
    "SN76496", "SEGAPCM", "RF5C68", "YMZ280B", "YMW258", "UPD7759", "MSM6258", "K051649",
    "K054539", "C6280", "C140", "C219", "K053260", "QSOUND", "VBOY_VSU", "ES5503", "X1_010",
    "C352", "GA20", "YM2413", "YM2612", "YM2151", "YM2203", "YM2608", "YM2610", "YMF278B",
    "YMF262", "YMF271", "AY8910", "32X_PWM", "GAMEBOY", "NES_APU", "MSM6295", "POKEY", "SCSP",
    "WSWAN", "SAA1099", "MIKEY",
];

/// Cores we deliberately do not compile, and why.
///
/// libvgm bundles Nuked-OPN2/OPM/OPLL/OPL3, which `vgms-cores-nuked` and
/// `vgms-cores-gpl` already link from Nuke.YKT's own releases. There is no symbol
/// collision -- libvgm renames every entry point with an `N` prefix
/// (`NOPN2_Reset`, ...) and marks the rest `static` -- but shipping the same
/// emulator twice costs binary size, and a duplicate provenance row would make
/// the About box overcount distinct emulators. Our submodules are the Nuked
/// tier; libvgm supplies what they do not.
const CORES_SERVED_ELSEWHERE: &[&str] = &[
    "EC_YM2612_NUKED",
    "EC_YM2151_NUKED",
    "EC_YM2413_NUKED",
    // OPL never reaches this crate anyway (see the note in `src/lib.rs`), so
    // these two are belt and braces.
    "EC_YM3812_NUKED",
    "EC_YMF262_NUKED",
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=shim");

    let libvgm = PathBuf::from(UPSTREAM).join("libvgm");
    require_submodule(&libvgm, "libvgm", "emu/SoundEmu.c");
    let emu = libvgm.join("emu");

    let mut build = cc::Build::new();
    build
        .include(&libvgm)
        .include(&emu)
        // Selective device compilation. Without this, `SoundEmu.c` defines
        // every `SNDDEV_` itself and fails to link against the ~170 core
        // sources we did not compile.
        .define("SNDDEV_SELECT", None)
        // `stdtype.h` falls back to hand-rolled typedefs without this; clang has
        // `<stdint.h>` on every target we build for.
        .define("HAVE_STDINT_H", None)
        .define("VGM_LITTLE_ENDIAN", None)
        .warnings(false);

    if build.get_compiler().is_like_msvc() {
        // libvgm's cores use `strcpy`/`sprintf` freely; upstream's own CMake
        // silences the same warnings the same way.
        build.define("_CRT_SECURE_NO_WARNINGS", None);
    }

    // `wasm32-unknown-unknown` has no libc and no sysroot: `-ffreestanding`
    // says so, `shim/wasm-libc` declares the slice of libc the cores include,
    // and the symbols come from `src/wasm_libc.rs` plus the printf stubs below.
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32");
    if wasm {
        build.flag("-ffreestanding");
        build.include("shim/wasm-libc");
        build.file("shim/wasm_stubs.c");
    }

    // Every emulator here is a hot per-sample loop. The arithmetic is
    // deterministic integer work, so optimising changes only how long a test
    // waits, not what it produces.
    build.opt_level(2);

    let mut sources: BTreeSet<String> = FRAMEWORK.iter().map(|&s| s.to_owned()).collect();

    for key in ENABLED {
        let device = DEVICES
            .iter()
            .find(|device| device.key == *key)
            .unwrap_or_else(|| panic!("ENABLED names {key:?}, which is not a row in DEVICES"));

        build.define(&format!("SNDDEV_{}", device.key), None);
        sources.extend(device.shared.iter().map(|&s| s.to_owned()));

        let mut any_core = false;
        for (define, files) in device.cores {
            if CORES_SERVED_ELSEWHERE.contains(define) {
                continue;
            }
            build.define(define, None);
            sources.extend(files.iter().map(|&s| s.to_owned()));
            any_core = true;
        }
        assert!(
            any_core || device.cores.is_empty(),
            "every core of {key:?} is in CORES_SERVED_ELSEWHERE, so the device \
             would compile with no emulator behind it and `SndEmu_Start` would \
             return EERR_NOT_FOUND at runtime"
        );
    }

    for source in &sources {
        let path = emu.join(source);
        println!("cargo::rerun-if-changed={}", path.display());
        build.file(path);
    }

    // Ours: reports what libvgm's public structs measure, so `src/layout.rs`
    // can assert the Rust twins in `src/ffi.rs` still agree with the pinned
    // headers. Compiled last so it sees the same includes and defines.
    build.file("shim/layout.c");

    build.compile("libvgm_cores");
}

/// Fails with an instruction rather than a missing-file error.
fn require_submodule(path: &Path, name: &str, marker: &str) {
    if path.join(marker).exists() {
        return;
    }
    panic!(
        "the {name} submodule is empty ({}).\n\
         Run:  git submodule update --init --recursive",
        path.display()
    );
}
