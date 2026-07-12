//! The DRO Trimmer GUI: a thin `eframe::run_native` shell injecting the
//! native services into `dro-ui`'s application.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;
use dro_trimmer::services::{
    IniConfigStore, NativeAudioService, NativeFileService, ThreadTaskService,
};
use dro_ui::DroApp;
use dro_ui::platform::FileService;

/// Opens a GUI to edit a DRO song.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// A .dro, .vgm or .vgz file to open at startup.
    file: Option<PathBuf>,
}

fn main() -> eframe::Result {
    env_logger::init();
    let args = Args::parse();

    let mut files = NativeFileService::new();
    if let Some(path) = args.file {
        // Queued through the file service so a failure surfaces as the app's
        // own "Failed to open file" box, like the Python's initial-load path.
        files.open_path(path);
    }

    let config = dro_trimmer::load_config();
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title(format!("DRO Trimmer v{}", env!("CARGO_PKG_VERSION")))
        .with_inner_size([800.0, 600.0])
        .with_maximized(config.ui.maximize_window)
        .with_drag_and_drop(true);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "drotrim",
        options,
        Box::new(move |cc| {
            // The task service pokes the event loop when a background render
            // finishes, so the result is picked up without waiting for input.
            let repaint = {
                let ctx = cc.egui_ctx.clone();
                move || ctx.request_repaint()
            };
            Ok(Box::new(DroApp::new(
                Box::new(files),
                Box::new(NativeAudioService::new()),
                Box::new(ThreadTaskService::with_notifier(repaint)),
                Box::new(IniConfigStore::new()),
                None,
            )))
        }),
    )
}
