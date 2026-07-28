//! Compiles the pinned ymfm submodule and our C shim.
//!
//! The same arrangement as the other provider crates, with one difference
//! that shapes everything: ymfm is **C++**, so this is the workspace's first
//! `cc::Build::cpp(true)`. That is also why the crate is native-only for now
//! -- `wasm32-unknown-unknown` has no C++ standard library, and ymfm uses
//! `std::vector` and friends. The registry simply will not list these cores
//! on web (CORES-PLAN §4: no stubs), which is the job the clean-room cores
//! in `dro-synth` keep.

use std::path::{Path, PathBuf};

/// Where the submodules live, relative to the workspace root.
const UPSTREAM: &str = "../../vendor/upstream";

/// The ymfm translation units we need. `ymfm_opn.cpp` is the OPN family;
/// `ymfm_adpcm.cpp` and `ymfm_ssg.cpp` are the sections it composes.
const SOURCES: [&str; 3] = ["ymfm_opn.cpp", "ymfm_adpcm.cpp", "ymfm_ssg.cpp"];

fn main() {
    println!("cargo::rerun-if-changed=shim");
    println!("cargo::rerun-if-changed=build.rs");

    let ymfm = PathBuf::from(UPSTREAM).join("ymfm");
    require_submodule(&ymfm, "ymfm", "src/ymfm.h");

    let src = ymfm.join("src");
    for file in SOURCES {
        println!("cargo::rerun-if-changed={}", src.join(file).display());
    }

    let mut build = cc::Build::new();
    build.cpp(true);
    for file in SOURCES {
        build.file(src.join(file));
    }
    build
        .file("shim/ymfm_c.cpp")
        .include(&src)
        // ymfm states C++14 as its requirement.
        .std("c++14")
        // The FM engines are hot loops over operator slots; an unoptimised
        // build makes even a unit test crawl, exactly as the LLE cores do.
        // The arithmetic is deterministic integer work, so the level changes
        // only the wait, not the output.
        .opt_level(2)
        .warnings(false);

    build.compile("ymfm_cores");
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
