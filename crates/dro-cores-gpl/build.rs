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

    let mut build = cc::Build::new();
    build
        .file(opll.join("opll.c"))
        .file("shim/layout.c")
        .include(&opll)
        // Ahead of the upstream's own directory, so the freestanding
        // <string.h> wins over a host one that may not exist.
        .include("shim")
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
