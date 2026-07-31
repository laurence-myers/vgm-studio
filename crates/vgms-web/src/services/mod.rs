// SPDX-License-Identifier: GPL-2.0-or-later
//! The web platform services: the wasm-only implementations of `vgms-ui`'s
//! `FileService`, `TaskService`, `ConfigStore` and `PackService` traits.
//!
//! Each mirrors a native service in `vgms-app`, swapping the OS facility for its
//! browser equivalent -- the filesystem for a file input and downloads, threads
//! for Web Workers, `vgmstudio.ini` for `localStorage`. All are polled, never
//! awaited, so the app's update loop drives them with no web-specific code.

mod audio;
mod config;
mod file;
mod pack;
mod task;

pub use audio::WebAudioService;
pub use config::LocalStorageStore;
pub use file::WebFileService;
pub use pack::WebPackService;
pub use task::WorkerTaskService;
