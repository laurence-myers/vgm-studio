//! Byte-parity: each wasm tool module must produce exactly what the native
//! executable produces, over the same bytes.
//!
//! The single most valuable test in `OPTIMIZER-WASM-PLAN.md`. The wasm and
//! native builds are the *same C* over a different libc, so any divergence is a
//! shim bug -- and it is localised to `shim/memfile.c`, `shim/wasm_printf.c` and
//! `src/wasm_libc.rs`, the ~few-hundred lines that differ.
//!
//! The native path is the oracle: `optimize_writes`/`trim_sample_roms`/
//! `clean_dac_runs` run the real executables as child processes. The wasm path
//! is driven here through the pure-Rust `wasmi`, so this runs in an ordinary
//! `cargo test` with no browser and no node.
//!
//! It needs the three `.wasm` examples pre-built:
//!
//! ```text
//! cargo build -p vgms-vgmtools --target wasm32-unknown-unknown --release \
//!     --features tool-modules \
//!     --example tool_vgm_cmp --example tool_vgm_sro --example tool_optdac
//! ```
//!
//! and found at `target/wasm32-unknown-unknown/release/examples` (or the dir in
//! `VGMS_VGMTOOLS_WASM_DIR`). Absent, the test skips, exactly as `corpus.rs`
//! does without `VGMSTUDIO_CORPUS`. Setting `VGMSTUDIO_CORPUS` widens the sample
//! past the repo fixtures.

use std::path::{Path, PathBuf};

use vgms_vgmtools::{ToolOutcome, clean_dac_runs, optimize_writes, trim_sample_roms};
use wasmi::{Engine, Linker, Module, Store};

/// One tool: the native entry point and the module that must match it.
struct Tool {
    name: &'static str,
    wasm: &'static str,
    native: fn(&[u8]) -> ToolOutcome,
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "vgm_cmp",
        wasm: "tool_vgm_cmp.wasm",
        native: optimize_writes,
    },
    Tool {
        name: "vgm_sro",
        wasm: "tool_vgm_sro.wasm",
        native: trim_sample_roms,
    },
    Tool {
        name: "optdac",
        wasm: "tool_optdac.wasm",
        native: clean_dac_runs,
    },
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Where the three pre-built `.wasm` modules live, or `None` to skip.
fn wasm_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VGMS_VGMTOOLS_WASM_DIR") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let dir = manifest_dir().join("../../target/wasm32-unknown-unknown/release/examples");
    dir.is_dir().then_some(dir)
}

/// Runs one tool module over `input`, returning the bytes it wrote (empty when
/// it wrote nothing -- i.e. left the file unchanged or declined it).
fn run_wasm(module: &Module, engine: &Engine, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut store = Store::new(engine, ());
    let linker = Linker::<()>::new(engine);
    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|error| format!("instantiate: {error}"))?
        .start(&mut store)
        .map_err(|error| format!("start: {error}"))?;

    let memory = instance
        .get_memory(&store, "memory")
        .ok_or("the module exports no memory")?;
    let reserve = instance
        .get_typed_func::<i32, i32>(&store, "reserve_input")
        .map_err(|error| format!("reserve_input: {error}"))?;
    let run = instance
        .get_typed_func::<(), i32>(&store, "run")
        .map_err(|error| format!("run: {error}"))?;
    let output_len = instance
        .get_typed_func::<(), i32>(&store, "output_len")
        .map_err(|error| format!("output_len: {error}"))?;
    let output_ptr = instance
        .get_typed_func::<(), i32>(&store, "output_ptr")
        .map_err(|error| format!("output_ptr: {error}"))?;

    let len = i32::try_from(input.len()).map_err(|_| "input too large".to_owned())?;
    let ptr = reserve
        .call(&mut store, len)
        .map_err(|error| format!("reserve_input call: {error}"))?;
    memory
        .write(&mut store, ptr as usize, input)
        .map_err(|error| format!("writing input: {error}"))?;

    run.call(&mut store, ())
        .map_err(|error| format!("run call: {error}"))?;

    let out_len = output_len
        .call(&mut store, ())
        .map_err(|error| format!("output_len call: {error}"))? as usize;
    let out_ptr = output_ptr
        .call(&mut store, ())
        .map_err(|error| format!("output_ptr call: {error}"))? as usize;

    let mut out = vec![0u8; out_len];
    if out_len != 0 {
        memory
            .read(&store, out_ptr, &mut out)
            .map_err(|error| format!("reading output: {error}"))?;
    }
    Ok(out)
}

/// Every `.vgm`/`.vgz` under `root`, uncompressed, capped at `limit`.
fn collect_dir(root: &Path, limit: usize, out: &mut Vec<(String, Vec<u8>)>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if out.len() >= limit {
                return;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_vgm = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                e.eq_ignore_ascii_case("vgm") || e.eq_ignore_ascii_case("vgz")
            });
            if !is_vgm {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let bytes = if raw.len() >= 2 && raw[0] == 0x1F && raw[1] == 0x8B {
                let mut decoded = Vec::new();
                if std::io::copy(&mut flate2::read::GzDecoder::new(raw.as_slice()), &mut decoded)
                    .is_err()
                {
                    continue;
                }
                decoded
            } else {
                raw
            };
            out.push((path.display().to_string(), bytes));
        }
    }
}

/// The repo fixtures (always) plus `VGMSTUDIO_CORPUS` (when set).
///
/// Deliberately *not* `vendor/upstream/vgmtools/ptch-test-vgms`: those are
/// malformed-by-design fixtures for `vgm_ptch`, the *repairer*, and several
/// trigger uninitialised-memory reads in the upstream optimisers on their broken
/// EOF/GD3 offsets (a wrong offset leaves a 2-byte gap the tool copies from an
/// unread buffer tail). The native output there is not even a defined value --
/// two runtimes give two answers, verified: `node` reproduces the native bytes
/// exactly, `wasmi` reads the fresh-instance zeros. Those are not the
/// optimiser's inputs, and there is no ground truth to match, so they are out of
/// scope. The optimiser's domain is well-formed VGMs.
fn collect_fixtures() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let manifest = manifest_dir();
    collect_dir(&manifest.join("../../tests"), usize::MAX, &mut out);
    if let Some(corpus) = std::env::var_os("VGMSTUDIO_CORPUS") {
        let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        collect_dir(&PathBuf::from(corpus), limit, &mut out);
    }
    out
}

/// Whether `vgms_core` can read `bytes` -- the same gate production applies
/// before handing a file to the optimiser (`pack_zip::optimize_song`,
/// `corpus.rs`). The parity claim is scoped to this domain: a file the app would
/// never optimise is not one the two builds must agree on.
fn readable(bytes: &[u8]) -> bool {
    vgms_core::vgm::file::read("parity.vgm", bytes).is_ok()
}

#[test]
#[ignore = "needs the tool .wasm modules pre-built; run in the wasm CI job"]
fn wasm_tools_match_the_native_exes_byte_for_byte() {
    let Some(dir) = wasm_dir() else {
        eprintln!(
            "skipping: the tool .wasm modules are not built. Build them with\n  \
             cargo build -p vgms-vgmtools --target wasm32-unknown-unknown --release \
             --example tool_vgm_cmp --example tool_vgm_sro --example tool_optdac\n\
             or point VGMS_VGMTOOLS_WASM_DIR at them."
        );
        return;
    };

    let engine = Engine::default();
    let fixtures = collect_fixtures();
    assert!(!fixtures.is_empty(), "no fixtures found to compare");

    let mut compared = 0usize;
    let mut unreadable = 0usize;
    let mut native_failed = 0usize;
    // Files where the *native* tool disagrees with itself run-to-run: the
    // upstream C reads uninitialised memory on some malformed inputs (a wrong
    // EOF/GD3 offset leaves a tail of a copied buffer untouched), so there is no
    // deterministic answer for wasm to match. Counted, not asserted on.
    let mut native_nondeterministic = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for tool in TOOLS {
        let wasm_bytes = std::fs::read(dir.join(tool.wasm))
            .unwrap_or_else(|e| panic!("reading {}: {e}", tool.wasm));
        let module = Module::new(&engine, &wasm_bytes[..])
            .unwrap_or_else(|e| panic!("compiling {}: {e}", tool.wasm));

        for (name, bytes) in &fixtures {
            if !readable(bytes) {
                unreadable += 1;
                continue;
            }

            let native = (tool.native)(bytes);
            // A native failure (a corrupt fixture the tool bails on, a timeout)
            // is not a parity data point: there is no agreed answer to match.
            if let ToolOutcome::Failed(_) = native {
                native_failed += 1;
                continue;
            }

            let wasm_out = match run_wasm(&module, &engine, bytes) {
                Ok(out) => out,
                Err(error) => {
                    mismatches.push(format!("{}: {name}: wasm error: {error}", tool.name));
                    continue;
                }
            };
            compared += 1;

            let ok = match &native {
                ToolOutcome::Smaller(native_out) => wasm_out == *native_out,
                ToolOutcome::Unchanged => wasm_out.is_empty(),
                ToolOutcome::Failed(_) => unreachable!("handled above"),
            };
            if ok {
                continue;
            }

            // A would-be mismatch. Before flagging it, prove the native output
            // is even deterministic: re-run the native tool and see if it agrees
            // with itself. If it does not, the input triggers the upstream
            // uninitialised-read UB and there is nothing for wasm to match.
            if (tool.native)(bytes) != native {
                native_nondeterministic += 1;
                continue;
            }

            let native_desc = match &native {
                ToolOutcome::Smaller(b) => format!("Smaller({} bytes)", b.len()),
                ToolOutcome::Unchanged => "Unchanged".to_owned(),
                ToolOutcome::Failed(_) => unreachable!(),
            };
            mismatches.push(format!(
                "{}: {name}: native {native_desc} but wasm wrote {} bytes (deterministic mismatch)",
                tool.name,
                wasm_out.len()
            ));
        }
    }

    println!(
        "compared {compared} (tool, file) pairs; {unreadable} skipped (unreadable), \
         {native_failed} (native failed), {native_nondeterministic} (native nondeterministic UB)"
    );
    assert!(
        mismatches.is_empty(),
        "{} parity mismatch(es):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
