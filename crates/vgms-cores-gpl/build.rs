//! Compiles the pinned upstream C cores, unmodified.
//!
//! Separate from `vgms-cores-nuked` only because these upstreams are GPL and
//! that crate's are LGPL, so the distinction survives into the metadata.
//!
//! Nuked-OPL2-Lite is always compiled; the other cores follow the crate's
//! features (`opll`, `psg`, `lle`), so an OPL2-only build needs only the
//! `nuked-opl2-lite` submodule.

use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=build.rs");

    let mut build = cc::Build::new();

    let opl2_lite = upstream("nuked-opl2-lite", "opl2.c", &["opl2.c", "opl2.h"]);
    build.file(opl2_lite.join("opl2.c")).include(&opl2_lite);

    if feature("OPLL") {
        let opll = upstream("nuked-opll", "opll.c", &["opll.c", "opll.h"]);
        build
            .file(opll.join("opll.c"))
            .include(&opll)
            .define("VGMS_CORE_OPLL", None);
    }

    if feature("PSG") {
        let psg = upstream("nuked-psg", "ympsg.c", &["ympsg.c", "ympsg.h"]);
        build
            .file(psg.join("ympsg.c"))
            .include(&psg)
            .define("VGMS_CORE_PSG", None);
    }

    if feature("LLE") {
        let opm_lle = upstream("ym2151-lle", "fmopm.c", &["fmopm.c", "fmopm.h"]);
        let opl2_lle = upstream("ym3812-lle", "fmopl2.c", &["fmopl2.c", "fmopl2.h"]);
        let opl3_lle = upstream("ymf262-lle", "fmopl3.c", &["fmopl3.c", "fmopl3.h"]);
        let opn_lle = upstream("ym2203-lle", "fmopn.c", &["fmopn.c", "fmopn.h"]);
        let opn2l_lle = upstream("ymf276-lle", "fmopn2.c", &["fmopn2.c", "fmopn2.h"]);
        // The OPN-family dies: one implementation compiled per chip macro. The
        // 2612 and 2608 dies are wrapped; the 2610 configuration does not
        // compile upstream (unguarded 2608-only GPIO writes at the pin), so it
        // waits.
        let opna_lle = upstream(
            "ym2608-lle",
            "fmopna_2612.c",
            &[
                "fmopna_2612.c",
                "fmopna_2612.h",
                "fmopna_2608.c",
                "fmopna_2608.h",
                "fmopna_impl.c",
                "fmopna_impl.h",
                "fmopna_rom.h",
            ],
        );

        build
            .file(opm_lle.join("fmopm.c"))
            .file(opl2_lle.join("fmopl2.c"))
            .file(opl3_lle.join("fmopl3.c"))
            .file(opn_lle.join("fmopn.c"))
            .file(opn2l_lle.join("fmopn2.c"))
            .file(opna_lle.join("fmopna_2612.c"))
            .file(opna_lle.join("fmopna_2608.c"))
            .file("shim/lle_opm.c")
            .file("shim/lle_opl2.c")
            .file("shim/lle_opl3.c")
            .file("shim/lle_opn.c")
            .file("shim/lle_opn2.c")
            .file("shim/lle_opn2l.c")
            .file("shim/lle_opna.c")
            .include(&opm_lle)
            .include(&opl2_lle)
            .include(&opl3_lle)
            .include(&opn_lle)
            .include(&opn2l_lle)
            .include(&opna_lle);
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

/// Whether the crate feature `name` (upper case, as Cargo spells it in the
/// environment) is on.
fn feature(name: &str) -> bool {
    std::env::var_os(format!("CARGO_FEATURE_{name}")).is_some()
}

/// The directory of submodule `name`, checked for `marker` and watched for
/// changes to `files`.
fn upstream(name: &str, marker: &str, files: &[&str]) -> PathBuf {
    let path = PathBuf::from(UPSTREAM).join(name);
    require_submodule(&path, name, marker);
    for file in files {
        println!("cargo::rerun-if-changed={}", path.join(file).display());
    }
    path
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
