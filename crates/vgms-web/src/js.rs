// SPDX-License-Identifier: GPL-2.0-or-later
//! Small shared helpers for reading values thrown across the JS boundary.

use wasm_bindgen::JsValue;

/// The human-readable message a thrown JS value carries: its own string, or its
/// `Error.message`. `None` when it is neither -- the caller picks the fallback,
/// because the right one differs (a failed wasm compile wants a generic label,
/// a file operation wants the value's debug form).
pub(crate) fn message(value: &JsValue) -> Option<String> {
    value.as_string().or_else(|| {
        js_sys::Reflect::get(value, &JsValue::from_str("message"))
            .ok()
            .and_then(|message| message.as_string())
    })
}
