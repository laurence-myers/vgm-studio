// SPDX-License-Identifier: GPL-2.0-or-later
//! wasm-bindgen entry point: eframe::WebRunner plus the web platform services.
//!
//! Built two ways. As `wasm32-unknown-unknown` it is the app module -- the egui
//! shell (`runner`), the web platform services, and the Worker entry the task
//! system posts work to. As a native crate it is just the portable [`codec`],
//! whose round-trip tests run in ordinary `cargo test` so a Worker-boundary bug
//! cannot hide where no browser test can see it.

pub mod codec;
