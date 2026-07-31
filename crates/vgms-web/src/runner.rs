// SPDX-License-Identifier: GPL-2.0-or-later
//! The web entry point: boot the egui app on a canvas with the web services.
//!
//! `index.html` calls [`start`] with the page's canvas once the module has
//! initialised. It installs a `console`-backed logger and panic hook, then hands
//! the same `VgmStudioApp` the native shell runs to `eframe::WebRunner`, injecting
//! the web platform services in place of the native ones. Nothing above the
//! service boundary knows it is on the web.

use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use vgms_ui::VgmStudioApp;
use vgms_ui::platform::ConfigStore;

use crate::services::{
    LocalStorageStore, WebAudioService, WebFileService, WebPackService, WorkerTaskService,
};

/// Boots the application onto `canvas`. Called from `index.html` after the wasm
/// module initialises; returns immediately, driving eframe on the event loop.
#[wasm_bindgen]
pub fn start(canvas: web_sys::HtmlCanvasElement) {
    install_logger();
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
    }));

    wasm_bindgen_futures::spawn_local(async move {
        let runner = eframe::WebRunner::new();
        if let Err(error) = runner
            .start(canvas, eframe::WebOptions::default(), Box::new(build_app))
            .await
        {
            web_sys::console::error_1(&error);
        }
    });
}

/// Builds the app with the web services, injecting a repaint notifier drawn from
/// the egui context so the polled services can wake the loop.
fn build_app(
    cc: &eframe::CreationContext<'_>,
) -> Result<Box<dyn eframe::App>, Box<dyn std::error::Error + Send + Sync>> {
    // A fresh notifier per service, each holding its own cheap `Context` clone,
    // so none has to be `Clone`.
    let notifier = || {
        let ctx = cc.egui_ctx.clone();
        move || ctx.request_repaint()
    };

    let store = LocalStorageStore::new();
    let config = store.load();
    vgms_ui::theme::install(&cc.egui_ctx, config.ui.theme);

    let app = VgmStudioApp::new(
        Box::new(WebFileService::new(notifier())),
        Box::new(WebAudioService::new(notifier())),
        Box::new(WorkerTaskService::new(notifier())),
        Box::new(WebPackService::new()),
        Box::new(LocalStorageStore::new()),
        None,
    );
    Ok(Box::new(app))
}

/// Routes `log` records to the browser console. Hand-rolled to keep the web build
/// free of an extra logging dependency.
fn install_logger() {
    use log::{Level, LevelFilter, Metadata, Record};

    struct ConsoleLogger;

    impl log::Log for ConsoleLogger {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            let message = JsValue::from_str(&format!("{}: {}", record.target(), record.args()));
            match record.level() {
                Level::Error => web_sys::console::error_1(&message),
                Level::Warn => web_sys::console::warn_1(&message),
                Level::Info => web_sys::console::info_1(&message),
                Level::Debug | Level::Trace => web_sys::console::debug_1(&message),
            }
        }

        fn flush(&self) {}
    }

    // Ignore the error if a logger is already set (a second `start` in one page).
    let _ = log::set_logger(&ConsoleLogger).map(|()| log::set_max_level(LevelFilter::Info));
}
