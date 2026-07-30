// SPDX-License-Identifier: GPL-2.0-or-later
//! DRO Trimmer's one executable: the GUI, plus the `play`, `render` and `split`
//! subcommands.
//!
//! With no subcommand this is a thin `eframe::run_native` shell injecting the
//! native services into `vgms-ui`'s application; with one, it runs that
//! subcommand and exits. See `vgms_app::cli`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

use clap::Parser;
use vgms_app::cli::Cli;
use vgms_app::services::{
    IniConfigStore, NativeFileService, NativePackService, SwitchingAudioService, ThreadTaskService,
};
use vgms_ui::VgmStudioApp;
use vgms_ui::platform::FileService;

/// Decodes the embedded `dt.ico` into the window/taskbar icon.
/// Returns `None` -- no icon, not a failure -- if it can't be decoded, so a bad
/// icon never blocks startup.
fn load_icon() -> Option<eframe::egui::IconData> {
    let image = image::load_from_memory_with_format(
        include_bytes!("../../../../src/dt.ico"),
        image::ImageFormat::Ico,
    )
    .ok()?
    .into_rgba8();
    let (width, height) = image.dimensions();
    Some(eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn main() -> ExitCode {
    // A release build is GUI-subsystem, so borrow the parent's console before
    // anything prints -- including clap's own `--help` and usage errors. Only
    // worth doing when there are arguments: a bare `drotrim` opens the GUI and
    // should stay silent.
    #[cfg(all(windows, not(debug_assertions)))]
    if std::env::args_os().len() > 1 {
        vgms_app::cli::attach_parent_console();
    }

    env_logger::init();
    // Before anything can ask what plays what: the GUI's Settings dialog, the
    // About credits and every subcommand that renders all read the registry.
    vgms_app::install_cores();
    let cli = Cli::parse();

    match cli.command {
        Some(command) => match vgms_app::cli::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                // `{:?}` on an anyhow error prints the whole cause chain.
                eprintln!("Error: {err:?}");
                ExitCode::FAILURE
            }
        },
        None => match run_gui(cli.file) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Opens the editor, optionally loading `file` at startup.
fn run_gui(file: Option<std::path::PathBuf>) -> eframe::Result {
    let mut files = NativeFileService::new();
    if let Some(path) = file {
        // Queued through the file service so a failure surfaces as the app's
        // own "Failed to open file" box.
        files.open_path(path);
    }

    let config = vgms_app::load_config();
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(format!("DRO Trimmer v{}", env!("CARGO_PKG_VERSION")))
        .with_inner_size([800.0, 600.0])
        .with_maximized(config.ui.maximize_window)
        .with_drag_and_drop(true);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "drotrim",
        options,
        Box::new(move |cc| {
            // The DOS-tracker look: fonts, palette and square bevelled chrome.
            vgms_ui::theme::install(&cc.egui_ctx, config.ui.theme);
            // The background services poke the event loop when a render or a
            // pack export finishes, so the result is picked up without waiting
            // for input.
            let repaint_tasks = {
                let ctx = cc.egui_ctx.clone();
                move || ctx.request_repaint()
            };
            let repaint_pack = {
                let ctx = cc.egui_ctx.clone();
                move || ctx.request_repaint()
            };
            Ok(Box::new(VgmStudioApp::new(
                Box::new(files),
                Box::new(SwitchingAudioService::new()),
                Box::new(ThreadTaskService::with_notifier(repaint_tasks)),
                Box::new(NativePackService::with_notifier(repaint_pack)),
                Box::new(IniConfigStore::new()),
                None,
            )))
        }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes_to_rgba() {
        let icon = super::load_icon().expect("dt.ico decodes");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }
}
