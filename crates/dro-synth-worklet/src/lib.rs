// SPDX-License-Identifier: GPL-2.0-or-later
//! A minimal `extern "C"` ABI over `dro-core` and `dro-synth`, for the AudioWorklet.
//!
//! Deliberately free of wasm-bindgen: `AudioWorkletGlobalScope` provides no
//! `TextDecoder`/`TextEncoder`, which bindgen's generated glue requires.
//!
//! Placeholder, not yet implemented. This crate will need to opt out of the
//! workspace's `unsafe_code = "deny"` lint for `#[unsafe(no_mangle)]` exports.
