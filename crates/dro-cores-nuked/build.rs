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
    require_submodule(&cqm, "nuked-cqm");
    println!("cargo::rerun-if-changed={}", cqm.join("cqm.c").display());
    println!("cargo::rerun-if-changed={}", cqm.join("cqm.h").display());

    let mut build = cc::Build::new();
    build
        .file(cqm.join("cqm.c"))
        // Ours: reports what the C side thinks `cqm_t` measures, so the Rust
        // mirror of it can be checked against the compiler rather than against
        // a size copied out of the header.
        .file("shim/layout.c")
        .include(&cqm)
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

    build.compile("nuked_cqm");
}

/// Fails with an instruction rather than a missing-file error.
///
/// A fresh clone has empty submodule directories, and `cc` would report a
/// missing `cqm.c` -- true, and useless. This says what to run.
fn require_submodule(path: &Path, name: &str) {
    if path.join("cqm.c").exists() {
        return;
    }
    panic!(
        "the {name} submodule is empty ({}).\n\
         Run:  git submodule update --init --recursive",
        path.display()
    );
}
