//! Compiles the pinned upstream C cores, unmodified.
//!
//! Separate from `vgms-cores-nuked` only because these upstreams are GPL and
//! that crate's are LGPL, so the distinction survives into the metadata.

use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=build.rs");

    let opll = PathBuf::from(UPSTREAM).join("nuked-opll");
    require_submodule(&opll, "nuked-opll", "opll.c");
    for file in ["opll.c", "opll.h"] {
        println!("cargo::rerun-if-changed={}", opll.join(file).display());
    }

    let psg = PathBuf::from(UPSTREAM).join("nuked-psg");
    require_submodule(&psg, "nuked-psg", "ympsg.c");
    for file in ["ympsg.c", "ympsg.h"] {
        println!("cargo::rerun-if-changed={}", psg.join(file).display());
    }

    let opm_lle = PathBuf::from(UPSTREAM).join("ym2151-lle");
    require_submodule(&opm_lle, "ym2151-lle", "fmopm.c");
    for file in ["fmopm.c", "fmopm.h"] {
        println!("cargo::rerun-if-changed={}", opm_lle.join(file).display());
    }

    let opl2_lle = PathBuf::from(UPSTREAM).join("ym3812-lle");
    require_submodule(&opl2_lle, "ym3812-lle", "fmopl2.c");
    for file in ["fmopl2.c", "fmopl2.h"] {
        println!("cargo::rerun-if-changed={}", opl2_lle.join(file).display());
    }

    let opl3_lle = PathBuf::from(UPSTREAM).join("ymf262-lle");
    require_submodule(&opl3_lle, "ymf262-lle", "fmopl3.c");
    for file in ["fmopl3.c", "fmopl3.h"] {
        println!("cargo::rerun-if-changed={}", opl3_lle.join(file).display());
    }

    // The OPN-family dies: one implementation compiled per chip macro. The
    // 2612 and 2608 dies are wrapped; the 2610 configuration does not compile
    // upstream (unguarded 2608-only GPIO writes at the pin), so it waits.
    let opna_lle = PathBuf::from(UPSTREAM).join("ym2608-lle");
    require_submodule(&opna_lle, "ym2608-lle", "fmopna_2612.c");
    for file in [
        "fmopna_2612.c",
        "fmopna_2612.h",
        "fmopna_2608.c",
        "fmopna_2608.h",
        "fmopna_impl.c",
        "fmopna_impl.h",
        "fmopna_rom.h",
    ] {
        println!("cargo::rerun-if-changed={}", opna_lle.join(file).display());
    }

    let mut build = cc::Build::new();
    build
        .file(opll.join("opll.c"))
        .file(psg.join("ympsg.c"))
        .file(opm_lle.join("fmopm.c"))
        .file(opl2_lle.join("fmopl2.c"))
        .file(opl3_lle.join("fmopl3.c"))
        .file(opna_lle.join("fmopna_2612.c"))
        .file(opna_lle.join("fmopna_2608.c"))
        .file("shim/layout.c")
        .file("shim/lle_opm.c")
        .file("shim/lle_opl2.c")
        .file("shim/lle_opl3.c")
        .file("shim/lle_opn2.c")
        .file("shim/lle_opna.c")
        .include(&opll)
        .include(&psg)
        .include(&opm_lle)
        .include(&opl2_lle)
        .include(&opl3_lle)
        .include(&opna_lle)
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
