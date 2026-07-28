//! Compiles the pinned upstream C cores, unmodified.
//!
//! The same arrangement as `dro-cores-nuked`, kept separate only because the
//! licence differs: these upstreams are GPL and that one's are LGPL, and the
//! whole point of two crates is that the distinction survives into the metadata
//! rather than living in a comment.

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

    // The OPN-family dies: one implementation compiled per chip macro. The
    // 2612 and 2608 dies are wrapped. The 2610 configuration does not
    // compile upstream (unguarded 2608-only GPIO writes at the pin; a
    // different error one commit back -- checked 2026-07-28), so it waits
    // for upstream rather than for us.
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
        .file(opna_lle.join("fmopna_2612.c"))
        .file(opna_lle.join("fmopna_2608.c"))
        .file("shim/layout.c")
        .file("shim/lle_opm.c")
        .file("shim/lle_opn2.c")
        .file("shim/lle_opna.c")
        .include(&opll)
        .include(&psg)
        .include(&opm_lle)
        .include(&opna_lle)
        // Ahead of the upstream's own directory, so the freestanding
        // <string.h> wins over a host one that may not exist.
        .include("shim")
        // Nuked-PSG's DAC is summed in `float`. Contraction would let a
        // compiler fuse those into FMAs on one target and not another, and
        // `ChipCore` promises identical output everywhere -- so forbid it
        // where the compiler understands the flag (MSVC does not contract
        // under its default /fp:precise).
        .flag_if_supported("-ffp-contract=off")
        // The LLE core simulates the die pin by pin -- millions of calls per
        // emulated second -- and an unoptimised build of it makes even the
        // tests crawl. The C is deterministic integer logic (plus the
        // contraction-pinned floats above), so optimisation level does not
        // change output, only how long a debug test run takes.
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
