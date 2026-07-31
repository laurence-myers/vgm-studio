// SPDX-License-Identifier: GPL-2.0-or-later
//! A minimal `extern "C"` ABI over `vgms-core` and `vgms-synth`, for the
//! AudioWorklet.
//!
//! Deliberately free of wasm-bindgen: `AudioWorkletGlobalScope` provides no
//! `TextDecoder`/`TextEncoder`, which bindgen's generated glue requires. The host
//! (`worklet-processor.js`) instead moves bytes through [`abi::vgmsw_alloc`] /
//! [`abi::vgmsw_free`] buffers and calls the exports directly.
//!
//! The module is one loaded song's worth of playback: it mirrors
//! `vgms-audio-native`'s cpal callback, minus the device. All the behaviour lives
//! in [`player`] as safe Rust the native test suite drives; [`abi`] is only the
//! pointer plumbing. `vgms-web` links this crate as an rlib to share
//! [`install_web_cores`], so the app module and the worklet module cannot
//! disagree about which cores exist.

pub mod abi;
mod player;

pub use player::install_web_cores;
