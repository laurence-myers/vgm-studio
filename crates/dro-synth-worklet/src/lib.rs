//! A minimal `extern "C"` ABI over `dro-core` and `dro-synth`, for the AudioWorklet.
//!
//! Deliberately free of wasm-bindgen: `AudioWorkletGlobalScope` provides no
//! `TextDecoder`/`TextEncoder`, which bindgen's generated glue requires.
//!
//! Placeholder: filled in during Step 9 of the Rust rewrite. This crate will need
//! to opt out of the workspace's `unsafe_code = "deny"` lint for
//! `#[unsafe(no_mangle)]` exports.
