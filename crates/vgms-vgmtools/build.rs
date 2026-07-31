//! Builds each vgmtools optimiser as its own executable and hands the bytes to
//! `lib.rs` to embed.
//!
//! Executables rather than a linked-in library is the whole design: these are
//! standalone programs that assume they own the process -- `chip_srom.c` frees
//! none of the ~50 sample-ROM buffers it reallocs, and a ROM size read straight
//! out of a data block can spin a `UINT32` mask forever -- so the process
//! boundary turns a leak into nothing and an unkillable hang into a timeout.
//! Each tool is compiled exactly as shipped, with no symbol renaming.
//!
//! `cc` builds static libraries, not executables, so this drives the detected
//! compiler itself while borrowing `cc`'s target and environment discovery.
//!
//! The submodule is never edited: upgrading is a submodule pull, a pin bump,
//! and a re-run of the golden tests.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the submodules live, relative to this crate.
const UPSTREAM: &str = "../../vendor/upstream/vgmtools";

/// One tool: the executable name, and its sources relative to the submodule.
///
/// Transcribed from upstream's `CMakeLists.txt` so the two can be diffed at a
/// pin bump. `optdac` is `EXCLUDE_FROM_ALL` there but costs only one more
/// compile.
struct Tool {
    name: &'static str,
    sources: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "vgm_cmp",
        sources: &["vgm_cmp.c", "chip_cmp.c"],
    },
    Tool {
        name: "vgm_sro",
        sources: &["vgm_sro.c", "chip_srom.c"],
    },
    Tool {
        name: "optdac",
        sources: &["optdac.c"],
    },
    Tool {
        name: "vgm_ptch",
        sources: &["vgm_ptch.c", "chip_strp.c"],
    },
];

fn main() {
    let upstream = PathBuf::from(UPSTREAM);
    let shim = PathBuf::from("shim");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));

    if !upstream.join("vgm_cmp.c").exists() {
        panic!(
            "the vgmtools submodule is empty -- run `git submodule update --init --recursive`\n\
             (looked in {})",
            upstream.display()
        );
    }

    // The web target compiles the three optimisers into one `.wasm` each (the
    // `[[example]]` cdylibs) over the freestanding libc in `src/wasm_libc.rs`
    // and `shim/`, rather than into standalone executables. See
    // `docs/vgm-multichip-2026-07/OPTIMIZER-WASM-PLAN.md`.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        build_wasm(&upstream, &shim);
        return;
    }

    println!("cargo:rerun-if-changed=shim/zshim.c");
    println!("cargo:rerun-if-changed=shim/zlib.h");
    for tool in TOOLS {
        for source in tool.sources {
            println!("cargo:rerun-if-changed={UPSTREAM}/{source}");
        }
    }

    for tool in TOOLS {
        build_tool(tool, &upstream, &shim, &out_dir);
    }
}

fn build_tool(tool: &Tool, upstream: &Path, shim: &Path, out_dir: &Path) {
    // `cc` is here for what it knows about the target and the environment --
    // which compiler, where MSVC's headers are -- not to compile anything.
    let probe = cc::Build::new().get_compiler();
    let exe = out_dir.join(exe_name(tool.name));

    let mut command = Command::new(probe.path());
    command.envs(probe.env().iter().cloned());

    let sources = tool.sources.iter().map(|source| upstream.join(source));

    if probe.is_like_msvc() {
        command
            .arg("/nologo")
            .arg("/O2")
            // The tools are C90 and use strcpy/sprintf throughout; upstream's
            // own CMakeLists sets this same define for MSVC.
            .arg("/D_CRT_SECURE_NO_WARNINGS")
            .arg("/W0")
            .arg(format!("/I{}", upstream.display()))
            // Ahead of any real zlib: `shim/zlib.h` is what the tools get.
            .arg(format!("/I{}", shim.display()))
            .args(sources)
            .arg(shim.join("zshim.c"))
            .arg(format!("/Fe:{}", exe.display()))
            // Objects land beside the exe rather than in the crate root; the
            // trailing separator is what tells `cl` this is a directory.
            .arg(format!(
                "/Fo:{}{}",
                out_dir.display(),
                std::path::MAIN_SEPARATOR
            ))
            .arg("/link")
            // `common.h`'s ReadFilename calls OemToChar.
            .arg("user32.lib");
    } else {
        command
            .arg("-O2")
            .arg("-w")
            .arg("-D_CRT_SECURE_NO_WARNINGS")
            .arg(format!("-I{}", upstream.display()))
            .arg(format!("-I{}", shim.display()))
            .args(sources)
            .arg(shim.join("zshim.c"))
            .arg("-o")
            .arg(&exe);
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            command.arg("-luser32");
        } else {
            // `vgm_ptch` is the one tool that needs libm (upstream's
            // CMakeLists notes this for non-MSVC).
            command.arg("-lm");
        }
    }

    let status = command
        .status()
        .unwrap_or_else(|error| panic!("could not run the compiler for {}: {error}", tool.name));
    assert!(status.success(), "building {} failed: {status}", tool.name);
    assert!(
        exe.exists(),
        "{} reported success but produced no executable at {}",
        tool.name,
        exe.display()
    );
}

fn exe_name(name: &str) -> String {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// Builds the three optimisers for `wasm32-unknown-unknown`, one **self-contained**
/// static archive each (tool sources + its chip table + the three shim objects),
/// left on disk for the matching `[[example]]` to link.
///
/// The desktop symbol-isolation problem the plan's Key 2 hoped separate modules
/// would dissolve does bite: `chip_cmp.c` and `chip_srom.c` define dozens of
/// globals with the same names and different meanings (`VGMHead`, `InitAllChips`,
/// `SetChipSet`, ...). So the tools must never share a link. Two rules, the wasm
/// analogue of three processes:
///
/// 1. **Each tool is its own self-contained archive** -- its own copy of the
///    shims included, so its link never reaches for a sibling's archive.
/// 2. **build.rs emits only the search path, not the `-l`.** Each `[[example]]`
///    names its own archive with `#[link(name = ..., kind = "static")]` (via the
///    `wasm_tool!` macro), so `tool_vgm_sro`'s link sees `vgmtools_wasm_sro` and
///    nothing else. Emitting `-l` here would instead put all three archives on
///    every example's line, and the linker would grab `vgm_cmp.o` for
///    `tool_vgm_sro` and duplicate-symbol.
///
/// Each main-bearing unit is `-Dmain=<tool>_main` so the tools' entry points --
/// and the globals `main` would otherwise anchor -- stay distinct.
fn build_wasm(upstream: &Path, shim: &Path) {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-changed=src/wasm_libc.rs");
    for source in ["vgm_cmp.c", "chip_cmp.c", "vgm_sro.c", "chip_srom.c", "optdac.c"] {
        println!("cargo:rerun-if-changed={UPSTREAM}/{source}");
    }

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    println!("cargo:rustc-link-search=native={}", out_dir.display());

    // (archive name, main-bearing source, its chip table -- optdac has none).
    let tools: &[(&str, &str, &[&str])] = &[
        ("vgmtools_wasm_cmp", "vgm_cmp.c", &["chip_cmp.c"]),
        ("vgmtools_wasm_sro", "vgm_sro.c", &["chip_srom.c"]),
        ("vgmtools_wasm_dac", "optdac.c", &[]),
    ];
    let shims = ["zshim.c", "memfile.c", "wasm_printf.c"];

    for (archive, main_source, chip_sources) in tools {
        let rename = format!("{}_main", main_source.trim_end_matches(".c"));

        let mut build = cc::Build::new();
        configure_wasm(&mut build, upstream, shim);
        // The `[[example]]`s do the linking via `#[link]`, so cc must not emit a
        // crate-wide `-l` that would land on every example.
        build.cargo_metadata(false);
        // Rename this unit's `main` so the tools' entry points -- and the globals
        // `main` would otherwise anchor -- stay distinct across the crate.
        build.define("main", rename.as_str());
        build.file(upstream.join(main_source));
        for chip in *chip_sources {
            build.file(upstream.join(chip));
        }
        for shim_source in shims {
            build.file(shim.join(shim_source));
        }
        build.compile(archive);
    }
}

/// Shared cc setup for the wasm tool objects: freestanding, our headers first.
fn configure_wasm(build: &mut cc::Build, upstream: &Path, shim: &Path) {
    build
        // `shim/wasm-libc` must precede the others so our `<stdio.h>` (with a
        // real `FILE`) wins over anything clang might otherwise find; `shim`
        // serves `<zlib.h>`; `upstream` serves the tools' own headers.
        .include(shim.join("wasm-libc"))
        .include(shim)
        .include(upstream)
        // No libc, no sysroot: say so, and let `shim/wasm-libc` + `wasm_libc.rs`
        // supply the slice the tools use.
        .flag("-ffreestanding")
        .warnings(false)
        .opt_level(2);
}
