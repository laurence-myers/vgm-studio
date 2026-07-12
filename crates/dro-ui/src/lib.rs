//! The DRO Trimmer GUI, as a library.
//!
//! The egui application (`app::DroApp`, added on top of this core) is consumed
//! by the native `drotrim` binary through `eframe::run_native`, and later
//! (Step 8) by the web shell through `eframe::WebRunner`. Every platform
//! difference -- file dialogs, audio output, background threads, config
//! storage -- is injected through the traits in [`platform`] and [`tasks`], so
//! this crate stays free of native-only dependencies.
//!
//! This module tree is the platform-agnostic core: the editor model,
//! selection, register-analysis cache, and the platform-service traits. The
//! egui presentation layer is layered on top of it.

#![forbid(unsafe_code)]

pub mod analysis;
pub mod editor;
pub mod platform;
pub mod selection;
pub mod tasks;
#[cfg(test)]
mod test_song;

pub use platform::{AudioService, ConfigStore, FileService, PickedFile, SaveOutcome, SaveRequest};
pub use tasks::{TaskKind, TaskRequest, TaskResult, TaskService, run_task};
