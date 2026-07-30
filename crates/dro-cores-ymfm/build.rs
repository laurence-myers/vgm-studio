//! Compiles the pinned ymfm submodule and our C shim.
//!
//! ymfm is C++, so this uses `cc::Build::cpp(true)`, and that makes the crate
//! native-only: `wasm32-unknown-unknown` has no C++ standard library and ymfm
//! uses `std::vector` and friends, so the registry does not list these cores on
//! web.

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
        // The FM engines are hot loops over operator slots; an unoptimised build
        // makes even a unit test crawl. The arithmetic is deterministic, so opt
        // level changes only speed, not output.
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
