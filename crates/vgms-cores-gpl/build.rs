//! Compiles the pinned upstream C cores, unmodified.
//!
//! Separate from `vgms-cores-nuked` only because these upstreams are GPL and
//! that crate's are LGPL, so the distinction survives into the metadata.
//!
//! Each core is a crate feature, and only the enabled cores are compiled, so
//! a build needs only their submodules. With no core enabled this compiles
//! nothing and `lib.rs` stops the build with a `compile_error!`.

use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

/// One core: its feature (as Cargo spells it in the environment), its
/// submodule, the upstream files to compile, the files to watch, the shim
/// files, and the define that `shim/layout.c` reads.
struct Core {
    feature: &'static str,
    submodule: &'static str,
    sources: &'static [&'static str],
    watch: &'static [&'static str],
    shims: &'static [&'static str],
    define: Option<&'static str>,
}

/// The OPN-family dies of YM2608-LLE: one implementation compiled per chip
/// macro. The 2612 and 2608 dies are wrapped; the 2610 configuration does not
/// compile upstream (unguarded 2608-only GPIO writes at the pin), so it waits.
const OPNA_FILES: &[&str] = &[
    "fmopna_2612.c",
    "fmopna_2612.h",
    "fmopna_2608.c",
    "fmopna_2608.h",
    "fmopna_impl.c",
    "fmopna_impl.h",
    "fmopna_rom.h",
];

const CORES: &[Core] = &[
    Core {
        feature: "OPL2_LITE",
        submodule: "nuked-opl2-lite",
        sources: &["opl2.c"],
        watch: &["opl2.c", "opl2.h"],
        shims: &[],
        define: Some("VGMS_CORE_OPL2_LITE"),
    },
    Core {
        feature: "OPLL",
        submodule: "nuked-opll",
        sources: &["opll.c"],
        watch: &["opll.c", "opll.h"],
        shims: &[],
        define: Some("VGMS_CORE_OPLL"),
    },
    Core {
        feature: "PSG",
        submodule: "nuked-psg",
        sources: &["ympsg.c"],
        watch: &["ympsg.c", "ympsg.h"],
        shims: &[],
        define: Some("VGMS_CORE_PSG"),
    },
    Core {
        feature: "YM2151_LLE",
        submodule: "ym2151-lle",
        sources: &["fmopm.c"],
        watch: &["fmopm.c", "fmopm.h"],
        shims: &["shim/lle_opm.c"],
        define: None,
    },
    Core {
        feature: "YM2203_LLE",
        submodule: "ym2203-lle",
        sources: &["fmopn.c"],
        watch: &["fmopn.c", "fmopn.h"],
        shims: &["shim/lle_opn.c"],
        define: None,
    },
    Core {
        feature: "YM2608_LLE",
        submodule: "ym2608-lle",
        sources: &["fmopna_2608.c"],
        watch: OPNA_FILES,
        shims: &["shim/lle_opna.c"],
        define: None,
    },
    Core {
        feature: "YM2612_LLE",
        submodule: "ym2608-lle",
        sources: &["fmopna_2612.c"],
        watch: OPNA_FILES,
        shims: &["shim/lle_opn2.c"],
        define: None,
    },
    Core {
        feature: "YMF276_LLE",
        submodule: "ymf276-lle",
        sources: &["fmopn2.c"],
        watch: &["fmopn2.c", "fmopn2.h"],
        shims: &["shim/lle_opn2l.c"],
        define: None,
    },
    Core {
        feature: "YM3812_LLE",
        submodule: "ym3812-lle",
        sources: &["fmopl2.c"],
        watch: &["fmopl2.c", "fmopl2.h"],
        shims: &["shim/lle_opl2.c"],
        define: None,
    },
    Core {
        feature: "YMF262_LLE",
        submodule: "ymf262-lle",
        sources: &["fmopl3.c"],
        watch: &["fmopl3.c", "fmopl3.h"],
        shims: &["shim/lle_opl3.c"],
        define: None,
    },
];

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=build.rs");

    let enabled: Vec<&Core> = CORES
        .iter()
        .filter(|core| std::env::var_os(format!("CARGO_FEATURE_{}", core.feature)).is_some())
        .collect();
    if enabled.is_empty() {
        // `lib.rs` says what to do; there is nothing to compile.
        return;
    }

    let mut build = cc::Build::new();
    let mut included = Vec::new();
    for core in enabled {
        let dir = PathBuf::from(UPSTREAM).join(core.submodule);
        require_submodule(&dir, core.submodule, core.sources[0]);
        for file in core.watch {
            println!("cargo::rerun-if-changed={}", dir.join(file).display());
        }
        for source in core.sources {
            build.file(dir.join(source));
        }
        for shim in core.shims {
            build.file(shim);
        }
        if !included.contains(&dir) {
            build.include(&dir);
            included.push(dir);
        }
        if let Some(define) = core.define {
            build.define(define, None);
        }
    }

    build
        .file("shim/layout.c")
        // Ahead of the upstream's own directory, so the freestanding
        // <string.h> wins over a host one that may not exist.
        .include("shim")
        // Nuked-PSG sums its DAC in `float`; contraction would fuse those into
        // FMAs on one target but not another, breaking `ChipCore`'s promise of
        // identical output everywhere. MSVC does not contract under /fp:precise.
        .flag_if_supported("-ffp-contract=off")
        // The LLE cores simulate the die pin by pin (millions of calls per
        // emulated second); an unoptimised build makes even the tests crawl.
        // The C is deterministic, so opt level changes speed, not output.
        .opt_level(2)
        .warnings(false);

    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        build.flag("-ffreestanding");
    }

    build.compile("gpl_cores");
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
