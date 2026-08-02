// SPDX-License-Identifier: GPL-2.0-or-later
//! wasm-bindgen entry point: eframe::WebRunner plus the web platform services.
//!
//! Built two ways. As `wasm32-unknown-unknown` it is the app module -- the egui
//! shell (`runner`, landing with the build), the web platform services
//! ([`services`]), and the Worker entry ([`worker`]) the task system posts work
//! to. As a native crate it is just the portable [`codec`], whose round-trip
//! tests run in ordinary `cargo test` so a Worker-boundary bug cannot hide where
//! no browser test can see it.

pub mod codec;
pub mod pack_zip;

// The Worker task service's stale-result filter. Buildable natively so its logic
// is unit-tested off-target, like `codec`; only the wasm services consume it.
#[cfg(any(target_arch = "wasm32", test))]
mod generations;

#[cfg(target_arch = "wasm32")]
pub mod optimize_tools;
#[cfg(target_arch = "wasm32")]
pub mod runner;
#[cfg(target_arch = "wasm32")]
pub mod services;
#[cfg(target_arch = "wasm32")]
pub mod worker;

#[cfg(target_arch = "wasm32")]
pub use services::{
    LocalStorageStore, WebAudioService, WebFileService, WebPackService, WorkerTaskService,
};
