// SPDX-License-Identifier: GPL-2.0-or-later
//! The VGM Studio GUI, as a library.
//!
//! The egui application (`app::VgmStudioApp`) is consumed by the native `vgmstudio`
//! binary through `eframe::run_native`, and later by the web shell
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
pub mod optimize;
pub mod pack;
pub mod platform;
pub mod selection;
pub(crate) mod strings;
pub mod tasks;
#[cfg(test)]
mod test_gpu;
#[cfg(test)]
mod test_song;
#[cfg(test)]
mod test_support;
pub mod theme;
#[cfg(test)]
mod theme_showcase;
// Crate-internal: nothing outside `vgms-ui` builds these widgets, and keeping the
// tree off the public API is what lets `dead_code` see an unused role inside it.
pub(crate) mod widgets;

pub use action::AppTab;
pub use app::VgmStudioApp;
#[cfg(any(test, feature = "e2e"))]
pub use app::{E2ePackSnapshot, E2eSnapshot};
pub use pack::PackState;
pub use platform::{
    ArchiveBackend, AudioService, ConfigStore, FileService, OptimizedImage, PackEntry,
    PackEntryKind, PackJobOutcome, PackJobRequest, PackOrigin, PackService, PickedFile,
    PickedFolder, SaveOutcome, SaveRequest,
};
pub use tasks::{TaskKind, TaskRequest, TaskResult, TaskService, run_task};
