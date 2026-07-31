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
        Box::new(WebPackService::new(notifier())),
        Box::new(LocalStorageStore::new()),
        None,
    );
    // An `e2e` build wraps the app so `window.__vgms_e2e` can drive it (wt-6); a
    // release build boxes it directly and exposes nothing.
    let boxed: Box<dyn eframe::App> = {
        #[cfg(feature = "e2e")]
        {
            e2e::attach(app, cc.egui_ctx.clone())
        }
        #[cfg(not(feature = "e2e"))]
        {
            Box::new(app)
        }
    };
    Ok(boxed)
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

/// The debug/e2e-only action/state hook (wt-6). Compiled only into an `e2e`
/// build; a release web build has none of this, so `window.__vgms_e2e` never
/// exists for a real user.
///
/// egui draws to one canvas with no DOM to select, so Playwright drives the app
/// through [`VgmStudioApp::e2e_enqueue_action`] (queued, drained next frame with
/// a live `Context`) and reads it through [`VgmStudioApp::e2e_snapshot`] (a pure
/// read). Both are reached through a `thread_local` handle stashed at boot, sound
/// because wasm is single-threaded and every JS->wasm call is synchronous.
#[cfg(feature = "e2e")]
mod e2e {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    use vgms_ui::VgmStudioApp;
    use vgms_ui::action::{Action, AppTab};
    use vgms_ui::app::E2eSnapshot;

    /// The app and the egui context, retained so the JS hook can enqueue actions
    /// and request the frame that drains them.
    struct Handle {
        app: Rc<RefCell<VgmStudioApp>>,
        ctx: eframe::egui::Context,
    }

    thread_local! {
        static E2E: RefCell<Option<Handle>> = const { RefCell::new(None) };
    }

    /// Delegates the frame to the shared app so a `thread_local` clone can reach
    /// it between frames. eframe owns this; the JS hook borrows the `Rc`.
    struct SharedApp(Rc<RefCell<VgmStudioApp>>);

    impl eframe::App for SharedApp {
        fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
            eframe::App::ui(&mut *self.0.borrow_mut(), ui, frame);
        }

        fn on_exit(&mut self) {
            eframe::App::on_exit(&mut *self.0.borrow_mut());
        }
    }

    /// Wraps `app` so `window.__vgms_e2e` can drive it, and installs the hook.
    pub(crate) fn attach(app: VgmStudioApp, ctx: eframe::egui::Context) -> Box<dyn eframe::App> {
        let app = Rc::new(RefCell::new(app));
        E2E.with(|slot| {
            *slot.borrow_mut() = Some(Handle {
                app: Rc::clone(&app),
                ctx,
            });
        });
        install();
        Box::new(SharedApp(app))
    }

    /// Sets `window.__vgms_e2e = { dispatch, state }`. The closures live for the
    /// page's lifetime (`forget`), which an e2e build is fine to leak.
    fn install() {
        let dispatch = Closure::<dyn Fn(String, JsValue) -> Result<(), JsValue>>::new(dispatch);
        let state = Closure::<dyn Fn() -> Result<JsValue, JsValue>>::new(state);
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("dispatch"),
            dispatch.as_ref().unchecked_ref(),
        );
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("state"),
            state.as_ref().unchecked_ref(),
        );
        if let Some(window) = web_sys::window() {
            let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__vgms_e2e"), &obj);
        }
        dispatch.forget();
        state.forget();
    }

    /// Queues an action (named as its `Action` variant) to run next frame.
    fn dispatch(name: String, arg: JsValue) -> Result<(), JsValue> {
        let action = map_action(&name, &arg).map_err(|message| JsValue::from_str(&message))?;
        E2E.with(|slot| -> Result<(), JsValue> {
            let borrow = slot.borrow();
            let handle = borrow
                .as_ref()
                .ok_or_else(|| JsValue::from_str("e2e app not initialised"))?;
            handle
                .app
                .try_borrow_mut()
                .map_err(|_| JsValue::from_str("e2e app is busy rendering"))?
                .e2e_enqueue_action(action);
            // Wake the loop so the queued action drains promptly.
            handle.ctx.request_repaint();
            Ok(())
        })
    }

    /// Returns the app state as a JS object (see [`E2eSnapshot`]).
    fn state() -> Result<JsValue, JsValue> {
        E2E.with(|slot| {
            let borrow = slot.borrow();
            let handle = borrow
                .as_ref()
                .ok_or_else(|| JsValue::from_str("e2e app not initialised"))?;
            let snapshot = handle
                .app
                .try_borrow()
                .map_err(|_| JsValue::from_str("e2e app is busy rendering"))?
                .e2e_snapshot();
            Ok(snapshot_to_js(&snapshot))
        })
    }

    /// Maps a variant name (+ optional argument) to an [`Action`]. Extended as the
    /// specs need more; an unknown name is a loud error rather than a silent no-op.
    fn map_action(name: &str, arg: &JsValue) -> Result<Action, String> {
        let action = match name {
            // File
            "OpenFile" => Action::OpenFile,
            "Save" => Action::Save,
            "SaveAs" => Action::SaveAs,
            "CloseFile" => Action::CloseFile,
            "ConfirmCloseFile" => Action::ConfirmCloseFile,
            // Edit
            "Undo" => Action::Undo,
            "Redo" => Action::Redo,
            "Status" => Action::Status(arg.as_string().unwrap_or_default()),
            // Playback
            "Play" => Action::Play,
            "Stop" => Action::Stop,
            // Pack
            "OpenPackFolder" => Action::OpenPackFolder,
            "ConfirmOpenPackFolder" => Action::ConfirmOpenPackFolder,
            "ClosePack" => Action::ClosePack,
            "ConfirmClosePack" => Action::ConfirmClosePack,
            "PackSaveDocs" => Action::PackSaveDocs,
            "PackScanVolumes" => Action::PackScanVolumes,
            "PackRenameFromTags" => Action::PackRenameFromTags,
            "PackConvertDatesToHyphens" => Action::PackConvertDatesToHyphens,
            "PackExportZip" => Action::PackExportZip,
            "ConfirmExportZip" => Action::ConfirmExportZip,
            "PackSaveArchive" => Action::PackSaveArchive,
            "ConfirmOpenZipPack" => Action::ConfirmOpenZipPack,
            "PackApplySuggestedModifiers" => Action::PackApplySuggestedModifiers {
                album: field_bool(arg, "album").unwrap_or(false),
            },
            "SelectTab" => Action::SelectTab(match arg.as_string().as_deref() {
                Some("pack") => AppTab::Pack,
                _ => AppTab::Editor,
            }),
            "PackTrackOpen" => Action::PackTrackOpen(scalar_usize(arg)?),
            "PackTrackPreview" => Action::PackTrackPreview(scalar_usize(arg)?),
            "ConfirmDeleteScreenshot" => {
                Action::ConfirmDeleteScreenshot(arg.as_string().unwrap_or_default())
            }
            "PackMoveTrack" => Action::PackMoveTrack {
                index: field_usize(arg, "index")?,
                delta: field_isize(arg, "delta")?,
            },
            "PackMoveTrackTo" => Action::PackMoveTrackTo {
                from: field_usize(arg, "from")?,
                to: field_usize(arg, "to")?,
            },
            other => return Err(format!("unknown e2e action {other:?}")),
        };
        Ok(action)
    }

    fn field_f64(arg: &JsValue, key: &str) -> Option<f64> {
        js_sys::Reflect::get(arg, &JsValue::from_str(key))
            .ok()
            .and_then(|value| value.as_f64())
    }

    fn field_bool(arg: &JsValue, key: &str) -> Option<bool> {
        js_sys::Reflect::get(arg, &JsValue::from_str(key))
            .ok()
            .and_then(|value| value.as_bool())
    }

    fn field_usize(arg: &JsValue, key: &str) -> Result<usize, String> {
        field_f64(arg, key)
            .map(|value| value as usize)
            .ok_or_else(|| format!("missing numeric field {key:?}"))
    }

    fn field_isize(arg: &JsValue, key: &str) -> Result<isize, String> {
        field_f64(arg, key)
            .map(|value| value as isize)
            .ok_or_else(|| format!("missing numeric field {key:?}"))
    }

    fn scalar_usize(arg: &JsValue) -> Result<usize, String> {
        arg.as_f64()
            .map(|value| value as usize)
            .ok_or_else(|| "expected a numeric argument".to_owned())
    }

    fn snapshot_to_js(snapshot: &E2eSnapshot) -> JsValue {
        let obj = js_sys::Object::new();
        set(
            &obj,
            "hasDocument",
            &JsValue::from_bool(snapshot.has_document),
        );
        set(
            &obj,
            "documentName",
            &opt_str(snapshot.document_name.as_deref()),
        );
        set(
            &obj,
            "rowCount",
            &JsValue::from_f64(snapshot.row_count as f64),
        );
        set(&obj, "dirty", &JsValue::from_bool(snapshot.dirty));
        set(&obj, "canUndo", &JsValue::from_bool(snapshot.can_undo));
        set(&obj, "canRedo", &JsValue::from_bool(snapshot.can_redo));
        set(&obj, "playing", &JsValue::from_bool(snapshot.playing));
        set(&obj, "status", &JsValue::from_str(&snapshot.status));
        set(&obj, "activeTab", &JsValue::from_str(snapshot.active_tab));
        set(&obj, "alert", &opt_str(snapshot.alert.as_deref()));
        set(
            &obj,
            "dialogOpen",
            &JsValue::from_bool(snapshot.dialog_open),
        );
        match &snapshot.pack {
            Some(pack) => {
                let pobj = js_sys::Object::new();
                set(&pobj, "name", &JsValue::from_str(&pack.name));
                set(&pobj, "dirty", &JsValue::from_bool(pack.dirty));
                set(&pobj, "trackNames", &str_array(&pack.track_names));
                set(&pobj, "imageNames", &str_array(&pack.image_names));
                set(&obj, "pack", &pobj);
            }
            None => set(&obj, "pack", &JsValue::NULL),
        }
        obj.into()
    }

    fn set(obj: &js_sys::Object, key: &str, value: &JsValue) {
        let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), value);
    }

    fn opt_str(value: Option<&str>) -> JsValue {
        value.map_or(JsValue::NULL, JsValue::from_str)
    }

    fn str_array(values: &[String]) -> JsValue {
        let array = js_sys::Array::new();
        for value in values {
            array.push(&JsValue::from_str(value));
        }
        array.into()
    }
}
