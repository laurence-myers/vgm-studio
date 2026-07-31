// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebTools`]: the wasm side of the vgmtools optimisers.
//!
//! `vgms-vgmtools` owns the pipeline -- the order, the wholly-OPL bypass, the
//! ROM-size guard and the chip hold-backs -- but a wasm module cannot
//! instantiate another wasm module on its own. So the web supplies the runner:
//! each of `vgm_cmp`, `vgm_sro` and `optdac` is its own `.wasm` (built by
//! `vgms-vgmtools`'s `tool-modules` feature, fetched beside the app), and this
//! drives them through the tiny ABI the modules export -- `reserve_input`,
//! `run`, `output_ptr`/`output_len`, `log_ptr`/`log_len`.
//!
//! Each call **instantiates a fresh module**: zero-initialised globals every
//! run, and O(1) reclamation when the instance is dropped -- the wasm analogue
//! of the desktop's fresh child process. A tool that traps surfaces as
//! [`ToolOutcome::Failed`], which the pipeline records and steps past, never
//! fatal. A genuinely hung module is the caller's problem: the pack export runs
//! in a Worker whose cancel is `terminate()` (ow-6), and the ROM-size guard
//! already refuses the one input known to spin forever (ow-5).
//!
//! The modules are compiled once (`WebAssembly.Module`) and instantiated per
//! call; compilation is synchronous, which is why this only ever runs in the
//! pack Worker, never on the main thread.

use js_sys::{Function, Object, Reflect, Uint8Array, WebAssembly};
use vgms_vgmtools::{Options, StageOutcome, ToolOutcome, Tools};
use wasm_bindgen::{JsCast, JsValue};

use crate::pack_zip::SongOptimizer;

/// The three tool modules, compiled once and instantiated per run.
pub struct WebTools {
    compress: WebAssembly::Module,
    sample_rom: WebAssembly::Module,
    dac_runs: WebAssembly::Module,
}

impl std::fmt::Debug for WebTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebTools").finish_non_exhaustive()
    }
}

impl WebTools {
    /// Compiles the three tool modules from their `.wasm` bytes (fetched beside
    /// the app). Fails only if a module will not compile, which means the wrong
    /// bytes were shipped.
    pub fn new(compress: &[u8], sample_rom: &[u8], dac_runs: &[u8]) -> Result<Self, String> {
        Ok(Self {
            compress: compile(compress, "vgm_cmp")?,
            sample_rom: compile(sample_rom, "vgm_sro")?,
            dac_runs: compile(dac_runs, "optdac")?,
        })
    }
}

impl Tools for WebTools {
    fn optimize_writes(&self, vgm: &[u8]) -> ToolOutcome {
        run_module(&self.compress, "vgm_cmp", vgm)
    }
    fn trim_sample_roms(&self, vgm: &[u8]) -> ToolOutcome {
        run_module(&self.sample_rom, "vgm_sro", vgm)
    }
    fn clean_dac_runs(&self, vgm: &[u8]) -> ToolOutcome {
        run_module(&self.dac_runs, "optdac", vgm)
    }
}

/// The pack Worker's [`SongOptimizer`]: the full vgmtools pipeline over the tool
/// modules ([`WebTools`]) plus `vgms_core`'s finishing pass, logged the way the
/// desktop pack logs it so a web pack's report reads the same.
#[derive(Debug)]
pub struct WebPipelineOptimizer {
    tools: WebTools,
}

impl WebPipelineOptimizer {
    /// Builds the optimiser from the three tool modules.
    pub fn new(tools: WebTools) -> Self {
        Self { tools }
    }
}

impl SongOptimizer for WebPipelineOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        let Ok(file) = vgms_core::vgm::file::read(name, bytes) else {
            // A DRO, or something unreadable. Either way it passes through.
            log.push(format!("{name}: kept as-is (not a readable VGM)"));
            return bytes.to_vec();
        };
        // The tools take plain bytes, and a pack entry may already be a `.vgz`.
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            log.push(format!("{name}: kept as-is (could not be prepared)"));
            return bytes.to_vec();
        };

        let result = vgms_vgmtools::optimize_vgm_with(&plain, Options::default(), &self.tools);

        if result.changed() {
            log.push(format!(
                "{name}: {} -> {} bytes (optimized, {} saved)",
                bytes.len(),
                result.bytes.len(),
                result.saved()
            ));
        }
        // Only the stages worth a line: "nothing to gain" is the common case.
        for stage in &result.stages {
            match &stage.outcome {
                StageOutcome::Shrank { from, to } => {
                    log.push(format!("{name}:   {} {from} -> {to} bytes", stage.name));
                }
                StageOutcome::Failed(reason) => {
                    log.push(format!("{name}:   {} failed: {reason}", stage.name));
                }
                StageOutcome::Skipped(reason) => {
                    log.push(format!("{name}:   {} skipped: {reason}", stage.name));
                }
                StageOutcome::Unchanged => {}
            }
        }

        let untouched: Vec<&str> = file
            .header
            .chips()
            .iter()
            .filter(|chip| vgms_vgmtools::passthrough_chips().contains(&chip.kind))
            .map(|chip| chip.kind.name())
            .collect();
        if !untouched.is_empty() {
            log.push(format!(
                "{name}: {} not optimised yet -- their writes were all kept",
                untouched.join(", ")
            ));
        }

        result.bytes
    }
}

fn compile(bytes: &[u8], name: &str) -> Result<WebAssembly::Module, String> {
    let source = Uint8Array::from(bytes);
    WebAssembly::Module::new(source.as_ref())
        .map_err(|error| format!("compiling {name}.wasm: {}", describe(&error)))
}

/// Instantiates `module` fresh, runs the tool over `input`, and maps what it
/// wrote to a [`ToolOutcome`] -- the same three answers the native binding gives:
/// smaller bytes, unchanged (the tool declined or gained nothing), or failed.
fn run_module(module: &WebAssembly::Module, name: &str, input: &[u8]) -> ToolOutcome {
    match run_module_inner(module, input) {
        Ok(RunResult { output, log }) => {
            if output.is_empty() {
                // No output file: the tool wrote nothing, i.e. it declined the
                // file or found nothing to gain -- exactly `Unchanged` natively.
                ToolOutcome::Unchanged
            } else if is_vgm(&output) {
                ToolOutcome::Smaller(output)
            } else {
                // Exited having written something that is not a VGM: the native
                // `collect` treats this the same way -- a failure, not output.
                ToolOutcome::Failed(with_tail(format!("{name} wrote a non-VGM"), &log))
            }
        }
        Err(reason) => ToolOutcome::Failed(format!("{name}: {reason}")),
    }
}

/// What a run produced: the bytes the tool wrote (empty when it wrote nothing)
/// and the tail of what it printed, for a failure message.
struct RunResult {
    output: Vec<u8>,
    log: String,
}

fn run_module_inner(module: &WebAssembly::Module, input: &[u8]) -> Result<RunResult, String> {
    let instance = WebAssembly::Instance::new(module, &Object::new()).map_err(js_reason)?;
    let exports = instance.exports();

    let memory: WebAssembly::Memory = get(&exports, "memory")?
        .dyn_into()
        .map_err(|_| "the module's `memory` export is not a Memory".to_owned())?;
    let reserve_input = func(&exports, "reserve_input")?;
    let run = func(&exports, "run")?;
    let output_len = func(&exports, "output_len")?;
    let output_ptr = func(&exports, "output_ptr")?;
    let log_len = func(&exports, "log_len")?;
    let log_ptr = func(&exports, "log_ptr")?;

    let len = u32::try_from(input.len()).map_err(|_| "input too large".to_owned())?;
    let ptr = call_u32(&reserve_input, len)?;
    // Write the input into the module's memory. `subarray` views the live
    // buffer, so this must happen before `run` grows (and detaches) it.
    Uint8Array::new(&memory.buffer())
        .subarray(ptr, ptr + len)
        .copy_from(input);

    call0(&run)?;

    let read_region = |ptr_fn: &Function, len_fn: &Function| -> Result<Vec<u8>, String> {
        let region_len = call0_u32(len_fn)?;
        if region_len == 0 {
            return Ok(Vec::new());
        }
        let region_ptr = call0_u32(ptr_fn)?;
        // Re-view the buffer: `run` may have grown memory, detaching the old one.
        let mut bytes = vec![0u8; region_len as usize];
        Uint8Array::new(&memory.buffer())
            .subarray(region_ptr, region_ptr + region_len)
            .copy_to(&mut bytes);
        Ok(bytes)
    };

    let output = read_region(&output_ptr, &output_len)?;
    let log = String::from_utf8_lossy(&read_region(&log_ptr, &log_len)?).into_owned();
    Ok(RunResult { output, log })
}

/// The last non-empty line of `log`, appended to `message` when there is one.
fn with_tail(message: String, log: &str) -> String {
    match log.split(['\r', '\n']).rfind(|l| !l.trim().is_empty()) {
        Some(tail) => format!("{message} ({tail})"),
        None => message,
    }
}

fn is_vgm(bytes: &[u8]) -> bool {
    bytes.len() >= 0x40 && &bytes[..4] == b"Vgm "
}

fn get(exports: &Object, name: &str) -> Result<JsValue, String> {
    Reflect::get(exports, &JsValue::from_str(name))
        .map_err(|_| format!("the module exports no `{name}`"))
}

fn func(exports: &Object, name: &str) -> Result<Function, String> {
    get(exports, name)?
        .dyn_into()
        .map_err(|_| format!("the module's `{name}` export is not a function"))
}

fn call0(f: &Function) -> Result<JsValue, String> {
    f.call0(&JsValue::NULL).map_err(js_reason)
}

fn call0_u32(f: &Function) -> Result<u32, String> {
    as_u32(call0(f)?)
}

fn call_u32(f: &Function, arg: u32) -> Result<u32, String> {
    as_u32(f.call1(&JsValue::NULL, &JsValue::from(arg)).map_err(js_reason)?)
}

fn as_u32(value: JsValue) -> Result<u32, String> {
    value
        .as_f64()
        .map(|n| n as u32)
        .ok_or_else(|| "a module export did not return a number".to_owned())
}

fn js_reason(error: JsValue) -> String {
    describe(&error)
}

fn describe(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            Reflect::get(error, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "wasm error".to_owned())
}
