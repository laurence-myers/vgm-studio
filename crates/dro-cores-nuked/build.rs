//! Compiles the pinned upstream C cores, unmodified.
//!
//! The sourcing policy in `crates/dro-synth/PROVENANCE.md`: an actively
//! maintained C upstream is a git submodule pinned to a commit, compiled as it
//! stands. Pulling a fix upstream is then `git -C vendor/upstream/<x> pull`, a
//! pin bump and a corpus re-run -- not a re-port, and not a merge against local
//! edits, because there are none. Everything this build needs that the upstream
//! does not provide lives in `shim/`.

use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=build.rs");

    let cqm = PathBuf::from(UPSTREAM).join("nuked-cqm");
    require_submodule(&cqm, "nuked-cqm", "cqm.c");
    watch(&cqm, &["cqm.c", "cqm.h"]);

    let opn2 = PathBuf::from(UPSTREAM).join("nuked-opn2");
    require_submodule(&opn2, "nuked-opn2", "ym3438.c");
    watch(&opn2, &["ym3438.c", "ym3438.h"]);

    let opm = PathBuf::from(UPSTREAM).join("nuked-opm");
    require_submodule(&opm, "nuked-opm", "opm.c");
    watch(&opm, &["opm.c", "opm.h"]);

    let mut build = cc::Build::new();
    build
        .file(cqm.join("cqm.c"))
        .file(opn2.join("ym3438.c"))
        .file(opm.join("opm.c"))
        // Ours: reports what each upstream struct measures, so the Rust side
        // can allocate one without declaring a twin of it that could drift.
        .file("shim/layout.c")
        .include(&cqm)
        .include(&opn2)
        .include(&opm)
        // Ahead of the upstream's own directory, so the freestanding
        // <string.h> wins over a host one that may not exist.
        .include("shim")
        .warnings(false);

    // `wasm32-unknown-unknown` has no libc and no sysroot. `-ffreestanding`
    // says so, and `--no-standard-includes` would go further than we want --
    // clang's own stdint.h is still needed. The shim covers the rest.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        build.flag("-ffreestanding");
    }

    build.compile("nuked_cores");
}

/// Rebuild when an upstream source changes -- which here means when a submodule
/// pin moves, since nothing else ever edits them.
fn watch(dir: &Path, files: &[&str]) {
    for file in files {
        println!("cargo::rerun-if-changed={}", dir.join(file).display());
    }
}

/// Fails with an instruction rather than a missing-file error.
///
/// A fresh clone has empty submodule directories, and `cc` would report a
/// missing source file -- true, and useless. This says what to run.
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
