// SPDX-License-Identifier: GPL-2.0-or-later
//! The Web Worker entry: run one background task and post its results back.
//!
//! `task_worker.js` loads this same app module inside a dedicated Worker, hands
//! each encoded [`TaskRequest`] to [`vgms_web_run_task`], and posts every encoded
//! [`TaskResult`] the task emits back to the page. Cancellation is
//! `Worker.terminate()` -- the whole instance dies -- so the task never needs to
//! ask whether it was cancelled.

use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};

/// Decodes `request`, runs the task, and calls `emit` with each result's bytes.
///
/// Installs the web core registry first so a VGM waveform or render builds real
/// chip cores (idempotent -- the first call in this Worker wins). `emit` is the
/// Worker's own `postMessage` bridge; a task may call it many times (the waveform
/// render streams progressive snapshots) and the Worker posts each straight to
/// the page.
#[wasm_bindgen]
pub fn vgms_web_run_task(request: &[u8], emit: &js_sys::Function) {
    vgms_synth_worklet::install_web_cores();

    let request = match crate::codec::decode_request(request) {
        Ok(request) => request,
        Err(error) => {
            web_sys::console::error_1(&JsValue::from_str(&format!(
                "vgms-web: could not decode a task request: {error}"
            )));
            return;
        }
    };

    let emit = emit.clone();
    // Cancellation is Worker.terminate(), so the task itself is never cancelled
    // cooperatively -- `is_cancelled` is always false here.
    let never_cancelled = || false;
    let mut on_result = |result| {
        let bytes = crate::codec::encode_result(&result);
        let array = js_sys::Uint8Array::from(bytes.as_slice());
        if let Err(error) = emit.call1(&JsValue::NULL, &array) {
            web_sys::console::error_1(&error);
        }
    };
    vgms_ui::run_task(&request, &never_cancelled, &mut on_result);
}

/// Builds a pack release zip and returns the encoded [`PackJobOutcome`] bytes.
///
/// `pack_worker.js` fetches the three optimiser `.wasm` modules
/// (`tool_vgm_cmp`/`tool_vgm_sro`/`tool_optdac`, built beside the app) and passes
/// their bytes here, then posts the returned outcome straight back to the page.
/// Cancellation is `Worker.terminate()`, so the build always runs to completion
/// here. Needs no cores -- the optimise pass rewrites the command stream, it does
/// not synthesise.
#[wasm_bindgen]
#[must_use]
pub fn vgms_web_run_pack_job(
    request: &[u8],
    tool_vgm_cmp: &[u8],
    tool_vgm_sro: &[u8],
    tool_optdac: &[u8],
) -> Vec<u8> {
    use vgms_ui::platform::PackJobOutcome;

    use crate::pack_zip::{BuiltInOptimizer, SongOptimizer};

    let request = match crate::codec::decode_pack_job(request) {
        Ok(request) => request,
        Err(error) => {
            return crate::codec::encode_pack_outcome(&PackJobOutcome::Failed(format!(
                "could not decode the pack job: {error}"
            )));
        }
    };

    // Compile the tool modules once, if the export asked to optimise. If they do
    // not load (the wrong bytes were shipped), fall back to `vgms_core`'s
    // built-in pass rather than fail the whole export.
    let built_in = BuiltInOptimizer;
    let pipeline = request.optimize_vgms.then(|| {
        crate::optimize_tools::WebTools::new(tool_vgm_cmp, tool_vgm_sro, tool_optdac)
            .map(crate::optimize_tools::WebPipelineOptimizer::new)
            .map_err(|error| {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "vgms-web: the optimiser modules did not load ({error}); \
                     using the built-in pass"
                )));
            })
            .ok()
    });
    let optimize: Option<&dyn SongOptimizer> = match &pipeline {
        None => None,                             // optimise off
        Some(Some(web)) => Some(web),             // the full vgmtools pipeline
        Some(None) => Some(&built_in),            // modules failed: built-in pass
    };

    let never_cancelled = || false;
    // A heartbeat per entry, so the page's inactivity watchdog can tell a slow
    // job from a hung one (a tool spinning forever is terminate()d by the page,
    // the wasm analogue of the native 120 s timeout kill).
    let heartbeat = || {
        let Ok(scope) = js_sys::global().dyn_into::<web_sys::DedicatedWorkerGlobalScope>() else {
            return;
        };
        let message = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &message,
            &JsValue::from_str("heartbeat"),
            &JsValue::from_bool(true),
        );
        let _ = scope.post_message(&message);
    };
    let outcome = match crate::pack_zip::build_pack_zip(
        &request.entries,
        request.gzip_vgms,
        optimize,
        &never_cancelled,
        &heartbeat,
    ) {
        Ok(Some(output)) => PackJobOutcome::Done {
            zip_name: request.zip_name,
            bytes: output.bytes,
            log: output.log,
        },
        // Unreachable (cancel is terminate), but honest rather than a panic.
        Ok(None) => PackJobOutcome::Failed("the pack export was cancelled".to_owned()),
        Err(error) => PackJobOutcome::Failed(error),
    };
    crate::codec::encode_pack_outcome(&outcome)
}
