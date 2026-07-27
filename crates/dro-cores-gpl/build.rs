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

    let mut build = cc::Build::new();
    build
        .file(opll.join("opll.c"))
        .file(psg.join("ympsg.c"))
        .file("shim/layout.c")
        .include(&opll)
        .include(&psg)
        // Ahead of the upstream's own directory, so the freestanding
        // <string.h> wins over a host one that may not exist.
        .include("shim")
        // Nuked-PSG's DAC is summed in `float`. Contraction would let a
        // compiler fuse those into FMAs on one target and not another, and
        // `ChipCore` promises identical output everywhere -- so forbid it
        // where the compiler understands the flag (MSVC does not contract
        // under its default /fp:precise).
        .flag_if_supported("-ffp-contract=off")
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
