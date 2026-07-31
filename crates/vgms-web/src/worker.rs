// SPDX-License-Identifier: GPL-2.0-or-later
//! The Web Worker entry: run one background task and post its results back.
//!
//! `task_worker.js` loads this same app module inside a dedicated Worker, hands
//! each encoded [`TaskRequest`] to [`vgms_web_run_task`], and posts every encoded
//! [`TaskResult`] the task emits back to the page. Cancellation is
//! `Worker.terminate()` -- the whole instance dies -- so the task never needs to
//! ask whether it was cancelled.

use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

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
