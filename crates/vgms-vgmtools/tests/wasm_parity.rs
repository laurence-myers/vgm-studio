//! Byte-parity: each wasip1 tool module must produce exactly what the native
//! executable produces, over the same bytes.
//!
//! The single most valuable test in `OPTIMIZER-WASM-PLAN.md`. The wasm and
//! native builds are the *same C* -- unmodified sources against wasi-libc
//! instead of the platform libc -- so any divergence is a toolchain or host
//! bug, and this gate is what catches it.
//!
//! The native path is the oracle: `optimize_writes`/`trim_sample_roms`/
//! `clean_dac_runs` run the real executables as child processes. The wasm path
//! is driven here through the pure-Rust `wasmi` with `wasmi_wasi` supplying the
//! WASI host functions, so this runs in an ordinary `cargo test` with no
//! browser and no node. Both ends run like processes -- argv, a scratch
//! directory, an exit code -- and both are interpreted by the one shared
//! mapping, [`command_outcome`].
//!
//! It needs the three wasip1 modules pre-built:
//!
//! ```text
//! tools/build-wasi-tools.ps1
//! ```
//!
//! and found at `target/wasi-tools` (or the dir in `VGMS_VGMTOOLS_WASM_DIR`).
//! Absent, the test skips, exactly as `corpus.rs` does without
//! `VGMSTUDIO_CORPUS`. Setting `VGMSTUDIO_CORPUS` widens the sample past the
//! repo fixtures.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use vgms_vgmtools::command::{ToolId, command_outcome};
use vgms_vgmtools::{ToolOutcome, clean_dac_runs, optimize_writes, trim_sample_roms};
use wasmi::{Engine, Linker, Module, Store};
use wasmi_wasi::WasiCtx;

/// One tool: the native entry point and the module that must match it.
struct Tool {
    id: ToolId,
    wasm: &'static str,
    native: fn(&[u8]) -> ToolOutcome,
}

const TOOLS: &[Tool] = &[
    Tool {
        id: ToolId::Compress,
        wasm: "tool_vgm_cmp.wasm",
        native: optimize_writes,
    },
    Tool {
        id: ToolId::SampleRom,
        wasm: "tool_vgm_sro.wasm",
        native: trim_sample_roms,
    },
    Tool {
        id: ToolId::DacRuns,
        wasm: "tool_optdac.wasm",
        native: clean_dac_runs,
    },
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Where the pre-built wasip1 modules live, or `None` to skip.
fn wasm_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VGMS_VGMTOOLS_WASM_DIR") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let dir = manifest_dir().join("../../target/wasi-tools");
    dir.is_dir().then_some(dir)
}

/// A scratch directory for one run -- the same in.vgm/out.vgm world the
/// native binding stages and the browser shim presents.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "vgms-wasi-parity-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Runs one wasip1 tool module over `input` and interprets the run exactly as
/// every other host does, via [`command_outcome`].
fn run_wasm(
    module: &Module,
    engine: &Engine,
    tool: ToolId,
    input: &[u8],
) -> Result<ToolOutcome, String> {
    let scratch = Scratch::new().map_err(|error| format!("scratch dir: {error}"))?;
    std::fs::write(scratch.dir.join("in.vgm"), input)
        .map_err(|error| format!("staging in.vgm: {error}"))?;

    let preopen = wasmi_wasi::sync::Dir::open_ambient_dir(
        &scratch.dir,
        wasmi_wasi::sync::ambient_authority(),
    )
    .map_err(|error| format!("opening the scratch dir: {error}"))?;

    let wasi = wasmi_wasi::sync::WasiCtxBuilder::new()
        .arg(tool.name())
        .map_err(|error| format!("argv: {error}"))?
        .arg("in.vgm")
        .map_err(|error| format!("argv: {error}"))?
        .arg("out.vgm")
        .map_err(|error| format!("argv: {error}"))?
        .preopened_dir(preopen, ".")
        .map_err(|error| format!("preopen: {error}"))?
        .build();

    let mut store = Store::new(engine, wasi);
    let mut linker = Linker::<WasiCtx>::new(engine);
    wasmi_wasi::add_to_linker(&mut linker, |ctx| ctx)
        .map_err(|error| format!("add_to_linker: {error}"))?;

    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|error| format!("instantiate: {error}"))?;

    let start = instance
        .get_typed_func::<(), ()>(&store, "_start")
        .map_err(|error| format!("_start: {error}"))?;

    let exit_code = match start.call(&mut store, ()) {
        Ok(()) => 0,
        Err(error) => match error.i32_exit_status() {
            Some(status) => status,
            None => return Err(format!("trap: {error}")),
        },
    };

    let output = std::fs::read(scratch.dir.join("out.vgm")).ok();
    Ok(command_outcome(tool, exit_code, output, ""))
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
/// EOF/GD3 offsets. The native output there is not even a defined value, so
/// there is no ground truth to match. The optimiser's domain is well-formed
/// VGMs, and so is the parity claim's.
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
/// before handing a file to the optimiser. The parity claim is scoped to this
/// domain: a file the app would never optimise is not one the two builds must
/// agree on.
fn readable(bytes: &[u8]) -> bool {
    vgms_core::vgm::file::read("parity.vgm", bytes).is_ok()
}

#[test]
#[ignore = "needs the wasip1 tool modules pre-built (tools/build-wasi-tools.ps1)"]
fn wasm_tools_match_the_native_exes_byte_for_byte() {
    let Some(dir) = wasm_dir() else {
        eprintln!(
            "skipping: the wasip1 tool modules are not built. Build them with\n  \
             tools/build-wasi-tools.ps1\n\
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
    // upstream C reads uninitialised memory on some malformed inputs, so there
    // is no deterministic answer for wasm to match. Counted, not asserted on.
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

            let wasm = match run_wasm(&module, &engine, tool.id, bytes) {
                Ok(outcome) => outcome,
                Err(error) => {
                    mismatches.push(format!("{}: {name}: wasm error: {error}", tool.id.name()));
                    continue;
                }
            };
            compared += 1;

            let ok = match (&native, &wasm) {
                (ToolOutcome::Smaller(a), ToolOutcome::Smaller(b)) => a == b,
                (ToolOutcome::Unchanged, ToolOutcome::Unchanged) => true,
                _ => false,
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

            let describe = |outcome: &ToolOutcome| match outcome {
                ToolOutcome::Smaller(b) => format!("Smaller({} bytes)", b.len()),
                ToolOutcome::Unchanged => "Unchanged".to_owned(),
                ToolOutcome::Failed(reason) => format!("Failed({reason})"),
            };
            mismatches.push(format!(
                "{}: {name}: native {} but wasm {} (deterministic mismatch)",
                tool.id.name(),
                describe(&native),
                describe(&wasm)
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
