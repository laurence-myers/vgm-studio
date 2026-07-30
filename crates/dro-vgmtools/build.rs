//! Builds each vgmtools optimiser as its own executable, the way upstream's
//! CMake does, and hands the bytes to `lib.rs` to embed.
//!
//! Executables rather than a linked-in library, and that is the whole design:
//! ot-1 in `docs/vgm-multichip-2026-07/OPTIMIZER-PLAN.md` records why. The
//! short version is that these are standalone programs that assume they own
//! the process -- `chip_srom.c` frees none of the ~50 sample-ROM buffers it
//! reallocs, and a ROM size read straight out of a data block can spin a
//! `UINT32` mask forever -- so the process boundary is what turns a leak into
//! nothing and an unkillable hang into a timeout. It also means no symbol
//! renaming, no `llvm-objcopy`, and each tool compiled exactly as shipped.
//!
//! `cc` builds static libraries, not executables, so this drives the detected
//! compiler itself while still borrowing `cc`'s target and environment
//! discovery.
//!
//! The submodule is never edited -- the policy every provider crate here
//! follows. Upgrading is `git -C vendor/upstream/vgmtools pull`, a pin bump,
//! and a re-run of the golden tests.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the submodules live, relative to this crate.
const UPSTREAM: &str = "../../vendor/upstream/vgmtools";

/// One tool: the executable name, and its sources relative to the submodule.
///
/// Transcribed from upstream's `CMakeLists.txt` so the two can be diffed at a
/// pin bump. `optdac` is `EXCLUDE_FROM_ALL` there -- it is a lesser tool, but
/// building it costs one more compile.
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
