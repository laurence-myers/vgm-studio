// SPDX-License-Identifier: GPL-2.0-or-later
//! The DRO Trimmer GUI, as a library.
//!
//! The egui application (`app::DroApp`) is consumed by the native `drotrim`
//! binary through `eframe::run_native`, and later (Step 8) by the web shell
//! through `eframe::WebRunner`. Every platform difference -- file dialogs,
//! audio output, background threads, config storage -- is injected through
//! the traits in [`platform`] and [`tasks`], so this crate stays free of
//! native-only dependencies.

#![forbid(unsafe_code)]

pub mod action;
pub mod alert;
pub mod analysis;
pub mod app;
pub mod dialogs;
pub mod editor;
pub mod markers;
pub mod menus;
mod optimise;
pub mod pack;
pub mod platform;
pub mod selection;
pub mod tasks;
#[cfg(test)]
mod test_song;
#[cfg(test)]
mod test_support;
pub mod theme;
#[cfg(test)]
mod theme_showcase;
pub mod widgets;

pub use action::AppTab;
pub use app::DroApp;
pub use pack::PackState;
pub use platform::{
    AudioService, ConfigStore, FileService, OptimizedImage, PackEntry, PackEntryKind,
    PackJobOutcome, PackJobRequest, PackService, PickedFile, PickedFolder, SaveOutcome,
    SaveRequest,
};
pub use tasks::{TaskKind, TaskRequest, TaskResult, TaskService, run_task};
