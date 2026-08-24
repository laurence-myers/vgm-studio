// SPDX-License-Identifier: GPL-2.0-or-later
//! [`WebTools`]: the web side of the vgmtools optimisers.
//!
//! `vgms-vgmtools` owns the pipeline -- the order, the wholly-OPL bypass, the
//! ROM-size guard, the chip hold-backs -- and the shared interpretation of a
//! finished run ([`command_outcome`]). The tools themselves are
//! `wasm32-wasip1` command modules fetched beside the app, and a wasm module
//! cannot instantiate another on its own, so the *hosting* lives in
//! `pack_worker.js`: its `globalThis.__vgms_run_tool` runs one module like a
//! process through the vendored browser_wasi_shim (`web/wasi-shim/`) -- argv,
//! an in-memory directory holding `in.vgm`, an exit code, maybe `out.vgm` --
//! and this module carries the result back into the pipeline.
//!
//! Each call instantiates a fresh module instance over there: zero-initialised
//! globals every run, the whole linear memory reclaimed in one go, the wasm
//! analogue of the desktop's fresh child process. A genuinely hung module is
//! the caller's problem: the pack export runs in a Worker whose cancel is
//! `terminate()`, the page arms an inactivity watchdog, and the ROM-size guard
//! refuses the one input known to spin forever before it starts.

use js_sys::{Reflect, Uint8Array, WebAssembly};
use vgms_vgmtools::command::{ToolId, command_outcome};
use vgms_vgmtools::{Options, ToolOutcome, Tools};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};

use crate::pack_zip::SongOptimizer;

#[wasm_bindgen]
extern "C" {
    /// `pack_worker.js`'s tool host (the `__vgms_pick_dir` pattern): runs one
    /// wasip1 module over `input` and returns `{ code, output, log }`.
    #[wasm_bindgen(catch, js_name = "__vgms_run_tool")]
    fn vgms_run_tool(
        module: &WebAssembly::Module,
        name: &str,
        input: &[u8],
    ) -> Result<JsValue, JsValue>;
}

/// The three tool modules, compiled once and instantiated per run by the
/// worker-side host.
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
    /// bytes were shipped -- the caller then falls back to the built-in pass.
    pub fn new(compress: &[u8], sample_rom: &[u8], dac_runs: &[u8]) -> Result<Self, String> {
        Ok(Self {
            compress: compile(compress, ToolId::Compress)?,
            sample_rom: compile(sample_rom, ToolId::SampleRom)?,
            dac_runs: compile(dac_runs, ToolId::DacRuns)?,
        })
    }

    fn run(&self, module: &WebAssembly::Module, tool: ToolId, vgm: &[u8]) -> ToolOutcome {
        let result = match vgms_run_tool(module, tool.name(), vgm) {
            Ok(result) => result,
            // A trap, an instantiation failure, a missing hook: the run is
            // lost, the file is not -- the pipeline records this and moves on.
            Err(error) => {
                return ToolOutcome::Failed(format!("{}: {}", tool.name(), describe(&error)));
            }
        };

        let code = Reflect::get(&result, &JsValue::from_str("code"))
            .ok()
            .and_then(|v| v.as_f64())
            .map_or(-1, |v| v as i32);
        let output = Reflect::get(&result, &JsValue::from_str("output"))
            .ok()
            .and_then(|v| v.dyn_into::<Uint8Array>().ok())
            .map(|bytes| bytes.to_vec());
        // The host hands back the recent lines untrimmed; the same `command::tail`
        // the native path applies reduces them, so both show the same amount.
        let log = Reflect::get(&result, &JsValue::from_str("log"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();

        command_outcome(tool, code, output, &vgms_vgmtools::command::tail(&log))
    }
}

impl Tools for WebTools {
    fn optimize_writes(&self, vgm: &[u8]) -> ToolOutcome {
        self.run(&self.compress, ToolId::Compress, vgm)
    }
    fn trim_sample_roms(&self, vgm: &[u8]) -> ToolOutcome {
        self.run(&self.sample_rom, ToolId::SampleRom, vgm)
    }
    fn clean_dac_runs(&self, vgm: &[u8]) -> ToolOutcome {
        self.run(&self.dac_runs, ToolId::DacRuns, vgm)
    }
}

fn compile(bytes: &[u8], tool: ToolId) -> Result<WebAssembly::Module, String> {
    let source = Uint8Array::from(bytes);
    // An empty byte array is the worker saying the fetch failed; say so rather
    // than letting WebAssembly.Module produce a cryptic validation error.
    if bytes.is_empty() {
        return Err(format!("{}.wasm was not fetched", tool.name()));
    }
    WebAssembly::Module::new(source.as_ref())
        .map_err(|error| format!("compiling {}.wasm: {}", tool.name(), describe(&error)))
}

fn describe(error: &JsValue) -> String {
    crate::js::message(error).unwrap_or_else(|| "wasm error".to_owned())
}

/// The pack Worker's [`SongOptimizer`]: the full vgmtools pipeline over the tool
/// modules ([`WebTools`]) plus `vgms_core`'s finishing pass, logged the way the
/// desktop pack logs it so a web pack's report reads the same.
#[derive(Debug)]
pub struct WebPipelineOptimizer {
    tools: WebTools,
    optimizer: vgms_core::config::OptimizerChoice,
    sample_roms: bool,
    dac_runs: bool,
}

impl WebPipelineOptimizer {
    /// Builds the optimiser from the three tool modules, routed by the Settings
    /// optimiser choice and tool-stage switches.
    pub fn new(
        tools: WebTools,
        optimizer: vgms_core::config::OptimizerChoice,
        sample_roms: bool,
        dac_runs: bool,
    ) -> Self {
        Self {
            tools,
            optimizer,
            sample_roms,
            dac_runs,
        }
    }
}

impl SongOptimizer for WebPipelineOptimizer {
    fn optimize(&self, name: &str, bytes: &[u8], log: &mut Vec<String>) -> Vec<u8> {
        // The pass and its narration are `optimize_song_logged` -- the one copy
        // shared with the desktop pack -- driven by the wasm tool runner.
        vgms_vgmtools::optimize_song_logged(
            name,
            bytes,
            Options {
                optimizer: self.optimizer,
                sample_roms: self.sample_roms,
                dac_runs: self.dac_runs,
            },
            &self.tools,
            log,
        )
    }
}
