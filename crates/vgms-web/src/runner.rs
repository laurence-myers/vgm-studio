// SPDX-License-Identifier: GPL-2.0-or-later
//! The web entry point: boot the egui app on a canvas with the web services.
//!
//! `index.html` calls [`start`] with the page's canvas once the module has
//! initialised. It installs a `console`-backed logger and panic hook, then hands
//! the same `VgmStudioApp` the native shell runs to `eframe::WebRunner`, injecting
//! the web platform services in place of the native ones. Nothing above the
//! service boundary knows it is on the web.

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::{JsFuture, spawn_local};

use vgms_ui::VgmStudioApp;
use vgms_ui::platform::ConfigStore;

use crate::services::{
    LocalStorageStore, WebAudioService, WebFileService, WebPackService, WorkerTaskService,
};

/// A CJK fallback font, fetched at runtime and laid beside the app module by the
/// build. Absent (a bare serve) degrades to the box glyph, as before.
const CJK_FONT_URL: &str = "./cjk-font.otf";

/// Boots the application onto `canvas`. Called from `index.html` after the wasm
/// module initialises; returns immediately, driving eframe on the event loop.
#[wasm_bindgen]
pub fn start(canvas: web_sys::HtmlCanvasElement) {
    install_logger();
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
    }));

    spawn_local(async move {
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
    // Install the core registry into *this* module before the app asks it
    // anything. Without this the app's own registry is the OPL-only fallback, so
    // every "can this play / render" question about a non-OPL chip answers no --
    // the transport is disabled and no waveform is ever requested. The native
    // shell makes the equivalent `install_cores` call in `main`; the Worker and
    // the AudioWorklet install their own copies for the work they do. (The three
    // registries are separate wasm instances, so each must install.)
    vgms_synth_worklet::install_web_cores();

    // A fresh notifier per service, each holding its own cheap `Context` clone,
    // so none has to be `Clone`.
    let notifier = || {
        let ctx = cc.egui_ctx.clone();
        move || ctx.request_repaint()
    };

    let store = LocalStorageStore::new();
    let config = store.load();
    vgms_ui::theme::install(&cc.egui_ctx, config.ui.theme);

    // The web build has no system fonts, so Japanese GD3 tags would render as
    // boxes. Fetch a CJK fallback and install it once it arrives; a missing or
    // malformed font leaves the box fallback in place.
    let ctx = cc.egui_ctx.clone();
    spawn_local(async move {
        if let Ok(bytes) = fetch_cjk_font().await {
            vgms_ui::theme::install_cjk_fallback(&ctx, bytes);
        }
    });

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

/// Fetches the CJK fallback font's bytes from [`CJK_FONT_URL`]. `Err` on any
/// failure -- a missing file, a network error, a non-OK status -- so the caller
/// simply keeps the box fallback.
async fn fetch_cjk_font() -> Result<Vec<u8>, ()> {
    let window = web_sys::window().ok_or(())?;
    let response_value = JsFuture::from(window.fetch_with_str(CJK_FONT_URL))
        .await
        .map_err(|_| ())?;
    let response: web_sys::Response = response_value.dyn_into().map_err(|_| ())?;
    if !response.ok() {
        return Err(());
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|_| ())?)
        .await
        .map_err(|_| ())?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
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
