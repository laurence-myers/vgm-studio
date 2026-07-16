//! The application: `wxapp.DTApp` + `containers.DTMainFrame`, as one
//! `eframe::App` driven entirely through the platform-service traits.

use core::fmt;
use core::time::Duration;
use std::collections::VecDeque;

use dro_core::config::{AppConfig, ConfigStore};
use dro_core::{FindTarget, Gd3Tag};
use egui::Key;

use crate::action::{Action, AppTab};
use crate::alert::{self, Alert};
use crate::dialogs::{
    Dialogs, DroInfoDialog, FindRegDialog, Gd3TagDialog, GotoDialog, SettingsDialog,
    TrackEditDialog, VgmMetadataDialog,
};
use crate::editor::{Editor, LoadReport};
use crate::menus::{self, MenuState};
use crate::platform::{
    AudioService, FileService, OptimizedImage, PickedFile, PickedFolder, RipJobOutcome, RipService,
    SaveOutcome, SaveRequest,
};
use crate::rip::RipState;
use crate::tasks::{TaskRequest, TaskResult, TaskService};
use crate::theme::{self, Palette};
use crate::widgets::peak_meter::PeakMeterState;
use crate::widgets::position_panel::PositionPanel;
use crate::widgets::waveform::WaveformState;
use crate::widgets::{channels::ChannelPanel, peak_meter, table, waveform};

const AUTO_TRIM_TITLE: &str = "DRO auto-trimmed";
const AUTO_TRIM_TEXT: &str = "The DRO was found to contain a bogus delay as\n\
                              its first instruction. It has been automatically\n\
                              removed. (Don't forget to save!)";
const MISMATCH_TITLE: &str = "DRO timing mismatch";

const HELP_TITLE: &str = "Help";
const HELP_TEXT: &str = "Full instructions are available online.\n\
    https://github.com/laurence-myers/dro-trimmer\n\
    \n\
    1) Select an instruction.\n\
    2) Delete via button or the Del key.\n\
    3) Profit!\n\
    \n\
    If you're trimming a looping song, look for a\n\
    whole bunch of instructions with no delays, as\n\
    this might be where the instruments are set up.";

fn about_text() -> String {
    format!(
        "DRO Trimmer v{}\n\
         Laurence Dougal Myers\n\
         Web: http://www.jestarjokin.net/apps/drotrimmer\n\
         Web: https://github.com/laurence-myers/dro-trimmer\n\
         E-Mail: jestarjokin@jestarjokin.net\n\
         \n\
         Thanks to:\n\
         The DOSBOX team\n\
         The AdPlug team\n\
         Adam Nielsen for PyOPL\n\
         Nuke.YKT for Nuked-OPL3\n\
         Wraithverge for testing, feedback and contributions\n\
         pi-r-squared for their original attempt at a DRO editor\n\
         \n\
         This build embeds the Nuked-OPL3 emulator and is licensed\n\
         under the LGPL-2.1-or-later. Complete source code:\n\
         https://github.com/laurence-myers/dro-trimmer",
        env!("CARGO_PKG_VERSION"),
    )
}

/// The DRO timing mismatch box (`wxapp.__load_file`), version-specific advice
/// and all. The v2 advice now points at the Settings dialog instead of a hand
/// edit of drotrim.ini, since the port has one.
fn mismatch_alert(auto_trimmed: bool, file_version: u32) -> Alert {
    let prefix = if auto_trimmed {
        "Despite auto-trimming, t"
    } else {
        "T"
    };
    let advice = if file_version == dro_core::song::DRO_FILE_V1 {
        "Please re-save the file to use the calculated value."
    } else {
        "Please enable \"Allow editing in DRO Info\" in the\n\
         Settings dialog, then edit the song length on\n\
         the DRO Info screen."
    };
    Alert::new(
        MISMATCH_TITLE,
        format!(
            "{prefix}here was a mismatch between\n\
             the measured length of the song in milliseconds,\n\
             and the length stored in the DRO file.\n\
             {advice}"
        ),
    )
}

/// Why a save was issued, so its outcome is routed to the right place. Save
/// outcomes arrive in the order the saves were made (the FIFO `FileService`
/// contract), so a queue of these correlates one-to-one with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SavePurpose {
    /// The editor's song (File > Save / Save As).
    Song,
    /// A rip project's description or playlist.
    RipDoc,
    /// A track rewritten in place by the quick-edit dialog.
    TrackRewrite,
    /// A screenshot rewritten in place after an explicit optimise.
    ImageOptimised,
    /// The exported release zip (a Save-As dialog).
    ExportZip,
}

pub struct DroApp {
    editor: Editor,
    files: Box<dyn FileService>,
    audio: Box<dyn AudioService>,
    tasks: Box<dyn TaskService>,
    rip_service: Box<dyn RipService>,
    config_store: Box<dyn ConfigStore>,
    config: AppConfig,

    status: String,
    alerts: VecDeque<Alert>,
    dialogs: Dialogs,

    /// The open rip project, if any.
    rip: Option<RipState>,
    /// The visible tab. Forced to `Editor` whenever no rip is open.
    active_tab: AppTab,
    /// One entry per outstanding `files.save`, in order, to route its outcome.
    pending_saves: VecDeque<SavePurpose>,

    waveform: WaveformState,
    /// The stereo output peak meter beside the waveform.
    peak_meter: PeakMeterState,
    position: PositionPanel,
    channels: ChannelPanel,

    /// A row the table should scroll into view next frame (Python's
    /// `EnsureVisible`).
    scroll_to: Option<usize>,
    /// The last first-selected row, to detect selection changes (the Python
    /// list's `FirstSelectedItemChangedEvent`).
    last_first_selected: Option<usize>,
    /// The editor revision currently loaded into the audio service, if any.
    audio_revision: Option<u64>,
    /// Whether the previous frame was playing, so the frame after playback
    /// ends can display the exact final position.
    was_playing: bool,
    /// A file passed on the command line, loaded on the first frame.
    pending_open: Option<PickedFile>,
}

impl DroApp {
    #[must_use]
    pub fn new(
        files: Box<dyn FileService>,
        audio: Box<dyn AudioService>,
        tasks: Box<dyn TaskService>,
        rip_service: Box<dyn RipService>,
        config_store: Box<dyn ConfigStore>,
        initial_file: Option<PickedFile>,
    ) -> Self {
        let config = config_store.load();
        Self {
            editor: Editor::new(),
            files,
            audio,
            tasks,
            rip_service,
            config_store,
            config,
            status: String::new(),
            alerts: VecDeque::new(),
            dialogs: Dialogs::default(),
            rip: None,
            active_tab: AppTab::Editor,
            pending_saves: VecDeque::new(),
            waveform: WaveformState::default(),
            peak_meter: PeakMeterState::default(),
            position: PositionPanel::new(config.audio.frequency),
            channels: ChannelPanel::new(),
            scroll_to: None,
            last_first_selected: None,
            audio_revision: None,
            was_playing: false,
            pending_open: initial_file,
        }
    }

    /// The active colour scheme.
    fn palette(&self) -> &'static Palette {
        theme::palette(self.config.ui.theme)
    }

    fn update_impl(&mut self, ui: &mut egui::Ui) {
        // The panels carve up `ui`; everything context-wide (input, dialogs,
        // repaint scheduling) still wants a `Context`, which is cheaply Arc-cloned.
        let ctx = ui.ctx().clone();
        if let Some(file) = self.pending_open.take() {
            self.load_file(file);
        }
        self.poll_services();
        self.handle_drops(&ctx);

        let mut actions: Vec<Action> = Vec::new();
        self.gather_key_input(&ctx, &mut actions);

        let p = self.palette();
        // Chrome panels sit on the face colour; the waveform is a data well, so
        // its margins take the main dark background rather than the chrome tint.
        let chrome = egui::Frame::side_top_panel(ui.style()).fill(p.face);
        // No side margins: the reset button owns the left edge and the waveform
        // runs flush to the right edge.
        let well = egui::Frame::side_top_panel(ui.style())
            .fill(p.data_bg)
            .inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 2,
                bottom: 2,
            });

        let menu = egui::Panel::top("menu-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                menus::bar(ui, p, &self.menu_state(), &mut actions);
            });
        // The tab strip switches the editor and rip views; shown only while a
        // rip project is open (otherwise the app is always the editor).
        let tabs = self.rip.is_some().then(|| {
            egui::Panel::top("tab-strip")
                .frame(chrome)
                .show_separator_line(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (tab, label) in [(AppTab::Editor, "Editor"), (AppTab::Rip, "Rip")] {
                            if ui.selectable_label(self.active_tab == tab, label).clicked() {
                                actions.push(Action::SelectTab(tab));
                            }
                        }
                    });
                })
        });
        // The editor-only panels (waveform, transport/boost, position) are hidden
        // on the rip tab, which owns the whole central area.
        let editor_tab = self.active_tab == AppTab::Editor;
        let waveform = editor_tab.then(|| {
            egui::Panel::top("waveform")
                .frame(well)
                .resizable(true)
                .default_size(150.0)
                .min_size(80.0)
                .show_separator_line(false)
                .show(ui, |ui| {
                    let height = ui.available_height();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // A full-height "skip to start" transport button on the left.
                        if theme::bevel::button_sized(ui, p, "\u{23EE}", egui::vec2(34.0, height))
                            .on_hover_text("Rewind to the start")
                            .clicked()
                        {
                            actions.push(Action::RewindToStart);
                        }
                        // Reserve the peak meter's width up front: the waveform
                        // fills whatever space it is given.
                        let wave_width =
                            ui.available_width() - peak_meter::WIDTH - ui.spacing().item_spacing.x;
                        ui.allocate_ui(egui::vec2(wave_width, height), |ui| {
                            let response =
                                waveform::show(ui, &self.waveform, self.editor.song(), p);
                            if let Some((index, ms)) = response.clicked {
                                actions.push(Action::WaveformClicked { index, ms });
                            }
                        });
                        peak_meter::show(ui, &self.peak_meter, p);
                    });
                })
        });
        let status = egui::Panel::bottom("status-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.rip_service.is_busy() {
                            // The status text names the operation (export or a
                            // screenshot optimise); this just shows liveness.
                            ui.label("Working...");
                        }
                        if self.tasks.is_busy() {
                            ui.label("Rendering waveform...");
                        }
                    });
                });
            });
        let position = editor_tab.then(|| {
            egui::Panel::bottom("position-panel")
                .frame(chrome)
                .show_separator_line(false)
                .show(ui, |ui| {
                    self.position.show(ui, p);
                })
        });
        let controls = editor_tab.then(|| {
            // The controls own their vertical spacing (equal padding above and
            // below each row band), so drop the frame's vertical margin/spacing.
            let controls_frame = egui::Frame::side_top_panel(ui.style())
                .fill(p.face)
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 0,
                });
            egui::Panel::bottom("controls")
                .frame(controls_frame)
                .show_separator_line(false)
                .show(ui, |ui| {
                    const PAD: f32 = 6.0;
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(PAD);
                    ui.horizontal(|ui| {
                        ui.set_min_height(ui.spacing().interact_size.y);
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if theme::bevel::button(ui, p, "Del.")
                            .on_hover_text("Delete the selected instruction(s)")
                            .clicked()
                        {
                            actions.push(Action::DeleteSelection);
                        }
                        if theme::bevel::button(ui, p, "Play")
                            .on_hover_text("Play the song from the current position")
                            .clicked()
                        {
                            actions.push(Action::Play);
                        }
                        if theme::bevel::button(ui, p, "Stop")
                            .on_hover_text("Stop playback")
                            .clicked()
                        {
                            actions.push(Action::Stop);
                        }
                        if theme::bevel::button(ui, p, "Tail")
                            .on_hover_text(self.play_tail_label())
                            .clicked()
                        {
                            actions.push(Action::PlayTail);
                        }
                        // Live playback boost, right-aligned in the row. A limiter
                        // behind it prevents clipping; the WAV render and the waveform
                        // stay at the un-boosted level. Built right-to-left: the
                        // up/down arrows, the editable value, the "Boost" label, then a
                        // full-height groove dividing it from the transport buttons.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let row_h = ui.spacing().interact_size.y;
                            // The control runs boost in integer steps 1..=5; a
                            // hand-edited ini may hold a fractional value, floored here.
                            let current = self.config.audio.boost.floor().clamp(1.0, 5.0) as i32;

                            // Up/down arrows, snug together (rightmost in the row).
                            // A nested `ui.horizontal` inherits the enclosing
                            // right-to-left layout, so add down first and up second and
                            // they come out up-on-the-left, down-on-the-right, like a
                            // stepper. (Forcing left-to-right here corrupts the parent.)
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 1.0;
                                let arrow = egui::vec2(20.0, row_h);
                                if theme::bevel::button_sized(ui, p, "\u{25BC}", arrow)
                                    .on_hover_text("Quieter")
                                    .clicked()
                                    && current > 1
                                {
                                    actions.push(Action::SetBoost {
                                        value: (current - 1) as f32,
                                        persist: true,
                                    });
                                }
                                if theme::bevel::button_sized(ui, p, "\u{25B2}", arrow)
                                    .on_hover_text("Louder")
                                    .clicked()
                                    && current < 5
                                {
                                    actions.push(Action::SetBoost {
                                        value: (current + 1) as f32,
                                        persist: true,
                                    });
                                }
                            });

                            // The value: a dark well with the tracker-yellow digit,
                            // click to type. Typed input floors to an integer 1..=5.
                            ui.scope(|ui| {
                                let widgets = &mut ui.visuals_mut().widgets;
                                for w in [
                                    &mut widgets.inactive,
                                    &mut widgets.hovered,
                                    &mut widgets.active,
                                ] {
                                    w.weak_bg_fill = p.data_bg;
                                    w.bg_fill = p.data_bg;
                                    w.fg_stroke.color = p.data_text;
                                }
                                let mut value = self.config.audio.boost;
                                let db = 20.0 * (current as f32).log10();
                                let response = ui
                                    .add(
                                        egui::DragValue::new(&mut value)
                                            .speed(0.0)
                                            .update_while_editing(false)
                                            .custom_formatter(|n, _| {
                                                format!("{}", n.floor().clamp(1.0, 5.0) as i64)
                                            })
                                            .custom_parser(|s| {
                                                s.trim()
                                                    .parse::<f64>()
                                                    .ok()
                                                    .map(|v| v.floor().clamp(1.0, 5.0))
                                            }),
                                    )
                                    .on_hover_text(format!("{current}\u{00d7} ({db:+.1} dB)"));
                                // No continuous drag (speed 0), so a change is always a
                                // committed edit -- persist it once, like an arrow click.
                                if response.changed() {
                                    actions.push(Action::SetBoost {
                                        value,
                                        persist: true,
                                    });
                                }
                            });

                            // The label sits left of the value...
                            ui.label("Boost");
                            // ...and a 2px beveled groove at full row height separates
                            // the boost section from the transport buttons, matching the
                            // grooves between the stacked panels.
                            theme::separator(ui, p);
                        });
                    });
                    ui.add_space(PAD);
                    theme::separator_full(ui, p);
                    ui.add_space(PAD);
                    // A plain OPL2 song has only one bank; hide the high-bank toggles.
                    let show_high_bank = self
                        .editor
                        .song()
                        .is_none_or(|song| song.opl_type != dro_core::OplType::Opl2);
                    if self.channels.show(ui, p, show_high_bank) {
                        actions.push(Action::MutingChanged);
                    }
                    ui.add_space(PAD);
                })
        });
        // The editor's central panel is one big data well; the rip view sits on
        // the FT2 desktop tint, with its own sunken wells inside.
        let central_fill = if editor_tab { p.data_bg } else { p.desktop };
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(central_fill))
            .show(ui, |ui| match self.active_tab {
                AppTab::Editor => {
                    if self.editor.has_song() {
                        // Row hover reads `widgets.hovered.bg_fill`, which is the
                        // bright face colour; scope it to the data-well tone so it
                        // does not flash teal under the yellow text.
                        ui.visuals_mut().widgets.hovered.bg_fill = p.data_hover;
                        table::show(ui, &mut self.editor, self.scroll_to.take(), p);
                    } else {
                        ui.visuals_mut().override_text_color = Some(p.data_label);
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                "Open a DRO, VGM or VGZ file (File > Open..., or drop it here).",
                            );
                        });
                    }
                }
                AppTab::Rip => {
                    if let Some(rip) = self.rip.as_mut() {
                        crate::rip::show(ui, rip, p, &mut actions);
                    }
                }
            });

        // 2px beveled grooves at the seams between the stacked panels. Painted
        // into the shared background layer *after* the panels, so they sit over
        // the panel content but below every Window/menu/popup (which live in
        // higher orders) -- an ad-hoc Middle layer would draw over dialogs. The
        // waveform panel is resizable, so the seams are recomputed each frame.
        let divider = ctx.layer_painter(egui::LayerId::background());
        let x_range = ctx.viewport_rect().x_range();
        // Only the panels actually drawn this frame contribute a seam.
        let mut seams = vec![menu.response.rect.bottom()];
        if let Some(tabs) = &tabs {
            seams.push(tabs.response.rect.bottom());
        }
        if let Some(waveform) = &waveform {
            seams.push(waveform.response.rect.bottom());
        }
        if let Some(controls) = &controls {
            seams.push(controls.response.rect.top());
        }
        if let Some(position) = &position {
            seams.push(position.response.rect.top());
        }
        seams.push(status.response.rect.top());
        for seam in seams {
            theme::bevel::groove_h(&divider, x_range, seam - 1.0, p);
        }

        // Keep the modeless dialogs off the menu bar and tab strip: since
        // egui 0.35 the panels above no longer reserve context space, so an
        // unconstrained window auto-places at the top of the viewport.
        let chrome_bottom = tabs
            .as_ref()
            .map_or(menu.response.rect.bottom(), |t| t.response.rect.bottom());
        let dialog_area = egui::Rect::from_min_max(
            egui::pos2(ctx.content_rect().left(), chrome_bottom),
            ctx.content_rect().max,
        );
        self.dialogs.show_all(&ctx, p, dialog_area, &mut actions);
        alert::show_front(&ctx, p, &mut self.alerts, &mut actions);

        for action in actions {
            self.handle_action(&ctx, action);
        }

        self.sync_selection_indicator();
        self.playback_tick(&ctx);
    }

    // -- frame plumbing ------------------------------------------------------

    fn poll_services(&mut self) {
        if let Some(result) = self.files.poll_picked() {
            match result {
                Ok(file) => self.load_file(file),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new("Failed to open file", message)),
            }
        }
        if let Some(result) = self.files.poll_folder() {
            match result {
                Ok(folder) => self.open_folder(folder),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new("Failed to open folder", message)),
            }
        }
        if let Some(result) = self.files.poll_renamed() {
            match result {
                Ok(()) => {
                    self.rescan_rip_folder();
                    self.status = "Renamed track; rip folder rescanned.".to_owned();
                }
                Err(message) => self.alerts.push_back(Alert::new("Rename failed", message)),
            }
        }
        if let Some(outcome) = self.files.poll_saved() {
            // Outcomes arrive in the order the saves were made, so a FIFO of
            // purposes routes each one to the editor or the rip project.
            let purpose = self.pending_saves.pop_front().unwrap_or(SavePurpose::Song);
            self.handle_save_outcome(purpose, outcome);
        }
        if let Some(outcome) = self.rip_service.poll() {
            match outcome {
                RipJobOutcome::Done {
                    zip_name,
                    bytes,
                    log,
                } => {
                    self.pending_saves.push_back(SavePurpose::ExportZip);
                    self.files.save(SaveRequest::Dialog {
                        suggested_name: zip_name,
                        bytes,
                    });
                    self.status = if log.is_empty() {
                        "Built the rip zip.".to_owned()
                    } else {
                        format!("Built the rip zip. {}", log.join(" "))
                    };
                }
                RipJobOutcome::Failed(message) => self
                    .alerts
                    .push_back(Alert::new("Rip export failed", message)),
            }
        }
        if let Some(result) = self.rip_service.poll_optimized() {
            match result {
                Ok(optimized) => self.image_optimized(optimized),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new("Optimise failed", message)),
            }
        }
        for result in self.tasks.poll() {
            match result {
                TaskResult::Waveform(buckets) => self.waveform.buckets = buckets,
            }
        }
    }

    fn handle_save_outcome(&mut self, purpose: SavePurpose, outcome: SaveOutcome) {
        match outcome {
            SaveOutcome::Saved { name, path } => match purpose {
                SavePurpose::Song => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    // Save As can change the .vgm/.vgz extension after the
                    // bytes were serialised; re-save once so the compression
                    // matches the chosen name.
                    if self.editor.record_saved(name, path)
                        && let (Ok(bytes), Some(path)) =
                            (self.editor.save_bytes(), self.editor.path.clone())
                    {
                        self.pending_saves.push_back(SavePurpose::Song);
                        self.files.save(SaveRequest::InPlace { path, bytes });
                    }
                    self.status = format!("File saved to {shown}.");
                }
                SavePurpose::RipDoc => {
                    // The description and playlist save back to back; report and
                    // clear the dirty flag once the last of them lands.
                    let more = self
                        .pending_saves
                        .iter()
                        .any(|purpose| *purpose == SavePurpose::RipDoc);
                    if !more {
                        if let Some(rip) = self.rip.as_mut() {
                            rip.dirty = false;
                        }
                        let stem = self
                            .rip
                            .as_ref()
                            .map_or_else(String::new, RipState::doc_stem);
                        self.status = format!("Saved {stem}.txt and {stem}.m3u.");
                    }
                }
                SavePurpose::TrackRewrite | SavePurpose::ImageOptimised => {
                    // The file's bytes were rewritten; rescan so the list (or
                    // the inline screenshot and its size) reflects the change. A
                    // rename, if any, rescans on its own outcome too -- both
                    // refresh in place, harmlessly.
                    self.rescan_rip_folder();
                }
                SavePurpose::ExportZip => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = format!("Exported {shown}.");
                }
            },
            SaveOutcome::Cancelled => {}
            SaveOutcome::Failed(message) => self
                .alerts
                .push_back(Alert::new("Failed to save file", message)),
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        // Only single-file drops, as in Python.
        if dropped.len() != 1 {
            return;
        }
        let file = dropped.into_iter().next().expect("len checked");
        let name = file
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.name.clone());
        let lower = name.to_ascii_lowercase();
        let is_song = lower.ends_with(".dro") || lower.ends_with(".vgm") || lower.ends_with(".vgz");
        if let Some(bytes) = file.bytes {
            // The web path: eframe delivers the dropped file's contents. Only
            // songs are handled here (a dropped folder has no bytes).
            if is_song {
                self.load_file(PickedFile {
                    name,
                    path: None,
                    bytes: bytes.to_vec(),
                });
            }
        } else if let Some(path) = file.path {
            // Native: a song opens in the editor; anything else (a folder, which
            // has no extension) is handed to the file service, which routes a
            // directory into rip mode. A junk file surfaces the usual "bad
            // format" alert.
            if is_song || path.extension().is_none() {
                self.files.open_path(path);
            }
        }
    }

    fn gather_key_input(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // A modal (an alert box, or the DRO Info dialog) blocks the app, as
        // wx's modal dialogs did -- without this, Space would start playback
        // behind the message box.
        if !self.alerts.is_empty() || self.dialogs.dro_info.is_some() {
            return;
        }
        // A focused widget owns the keyboard: Ctrl+Z in a tag field must undo
        // the *text*, not the song. (The wx accelerators likewise never fired
        // inside the dialogs' own text fields.)
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // The rip tab hides the editor, so the editor's keys (Del, Space, arrows,
        // Undo, ...) must not fire there. Only Save (the package files) and Help
        // remain.
        if self.active_tab == AppTab::Rip {
            ctx.input_mut(|input| {
                if input.consume_shortcut(&menus::SAVE) {
                    actions.push(Action::RipSaveDocs);
                }
                if input.consume_shortcut(&menus::HELP) {
                    actions.push(Action::Help);
                }
            });
            return;
        }
        ctx.input_mut(|input| {
            // egui's shortcut matching ignores a surplus Shift, so the
            // shifted variants must be consumed before their plain forms.
            if input.consume_shortcut(&menus::SAVE_AS) {
                actions.push(Action::SaveAs);
            }
            if input.consume_shortcut(&menus::SAVE) {
                actions.push(Action::Save);
            }
            if input.consume_shortcut(&menus::REDO_ALT) {
                actions.push(Action::Redo);
            }
            if input.consume_shortcut(&menus::UNDO) {
                actions.push(Action::Undo);
            }
            if input.consume_shortcut(&menus::REDO) {
                actions.push(Action::Redo);
            }
            if input.consume_shortcut(&menus::OPEN) {
                actions.push(Action::OpenFile);
            }
            if input.consume_shortcut(&menus::GOTO) {
                actions.push(Action::OpenGoto);
            }
            if input.consume_shortcut(&menus::FIND_REGISTER) {
                actions.push(Action::OpenFindRegister);
            }
            if input.consume_shortcut(&menus::DRO_INFO) {
                actions.push(Action::OpenDroInfo);
            }
            if input.consume_shortcut(&menus::HELP) {
                actions.push(Action::Help);
            }
        });

        ctx.input(|input| {
            if input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace) {
                actions.push(Action::DeleteSelection);
            }
            if input.key_pressed(Key::Space) {
                actions.push(Action::TogglePlayback);
            }
            if input.key_pressed(Key::ArrowLeft) {
                actions.push(Action::PreviousDelay);
            }
            if input.key_pressed(Key::ArrowRight) {
                actions.push(Action::NextDelay);
            }
            if input.key_pressed(Key::ArrowUp) {
                actions.push(Action::SelectionMove {
                    delta: -1,
                    extend: input.modifiers.shift,
                });
            }
            if input.key_pressed(Key::ArrowDown) {
                actions.push(Action::SelectionMove {
                    delta: 1,
                    extend: input.modifiers.shift,
                });
            }
            const NUMBER_KEYS: [Key; 9] = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            for (channel, key) in NUMBER_KEYS.into_iter().enumerate() {
                if input.key_pressed(key) {
                    actions.push(Action::ToggleChannel(channel));
                }
            }
        });
    }

    fn sync_selection_indicator(&mut self) {
        let first = self.editor.selection.first();
        if first == self.last_first_selected {
            return;
        }
        self.last_first_selected = first;
        // An emptied selection leaves the indicator where it was, as in Python.
        let Some(index) = first else {
            return;
        };
        let Some(song) = self.editor.song() else {
            return;
        };
        if let Some(ms) = song.ms_offset_at(index) {
            self.waveform.start_ms = ms;
            self.position.set_position_ms(ms);
        }
    }

    fn playback_tick(&mut self, ctx: &egui::Context) {
        // Advance the peak meter with the post-limiter peaks the callback
        // published. dt is clamped so a stalled frame cannot snap the bars to
        // zero. Kept separate from the position block below: the meter must
        // keep repainting through its decay after playback ends, without
        // re-running the position updates (which would overwrite the exact
        // end-of-song snap).
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.peak_meter.update(self.audio.take_peaks(), dt);
        if self.peak_meter.is_active() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let playing = self.audio.is_playing();
        if self.active_tab == AppTab::Editor {
            // One more update after playback ends, so the readout and cursor land
            // on the exact final position instead of freezing a buffer short of
            // it. (The Python's timer kept firing after the song finished.)
            if playing || self.was_playing {
                // A song that reached its end lands ~1 ms short of its length,
                // because the frame counter and the ms readout each floor at a
                // rate that need not divide evenly. Snap to the exact end so the
                // ms and sample counters agree. A manual Stop is not `is_finished`,
                // so its position is left exactly where playback paused.
                let ended = !playing && self.was_playing && self.audio.is_finished();
                if let Some(end) = ended
                    .then(|| self.editor.song().map(|song| song.total_delay_ms()))
                    .flatten()
                {
                    self.waveform.cursor_ms = end;
                    self.position.set_position_ms(end);
                } else if let Some(position) = self.audio.position() {
                    self.waveform.cursor_ms = position.elapsed_ms;
                    self.position.set_position(position);
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        } else if self.rip.as_ref().is_some_and(|rip| rip.preview.is_some()) {
            // A rip preview: clear it once it finishes, and keep the frames
            // coming while it plays (the rip view has no position readout).
            if self.audio.is_finished() {
                self.stop_preview();
            } else if playing {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        self.was_playing = playing;
        if self.tasks.is_busy() || self.rip_service.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // -- actions ---------------------------------------------------------

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenFile => self.files.pick_open(),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::OpenSettings => {
                self.dialogs.settings = Some(SettingsDialog::new(&self.config));
            }
            Action::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),

            Action::Undo => {
                if !self.require_song() {
                    return;
                }
                match self.editor.undo() {
                    Some(description) => {
                        self.status = format!("Undone: {description}");
                        self.after_edit();
                    }
                    None => self.status = "Nothing to undo.".to_owned(),
                }
            }
            Action::Redo => {
                if !self.require_song() {
                    return;
                }
                match self.editor.redo() {
                    Some(description) => {
                        self.status = format!("Redone: {description}");
                        self.after_edit();
                    }
                    None => self.status = "Nothing to redo.".to_owned(),
                }
            }
            Action::OpenGoto => {
                if self.require_song() {
                    self.dialogs.goto = Some(GotoDialog::new());
                }
            }
            Action::OpenFindRegister => {
                if self.require_song() {
                    let song = self.editor.song().expect("gated");
                    self.dialogs.find_reg = Some(FindRegDialog::new(song));
                }
            }
            Action::OpenDroInfo => {
                if self.require_song() {
                    let song = self.editor.song().expect("gated");
                    // Never editable for a VGM: its length is derived from
                    // the sample delays (an edit would silently evaporate),
                    // and re-typing its chip corrupts the header's clocks.
                    let edit_allowed = self.config.ui.dro_info_edit_enabled && !song.is_vgm();
                    self.dialogs.dro_info = Some(DroInfoDialog::new(song, edit_allowed));
                }
            }
            Action::OpenEditTag => {
                if !self.require_song() {
                    return;
                }
                let song = self.editor.song().expect("gated");
                if song.is_vgm() {
                    let tag = song.vgm_meta().and_then(|meta| meta.tag.as_ref());
                    self.dialogs.gd3_tag = Some(Gd3TagDialog::new(tag));
                } else {
                    self.status = "Only VGMs support tag editing".to_owned();
                }
            }
            Action::OpenVgmMetadata => {
                if !self.require_song() {
                    return;
                }
                let song = self.editor.song().expect("gated");
                match VgmMetadataDialog::new(song) {
                    Some(dialog) => self.dialogs.vgm_metadata = Some(dialog),
                    // "Songs is not a VGM" in the Python -- typo fixed.
                    None => self.status = "Song is not a VGM".to_owned(),
                }
            }
            Action::ConvertToVgm => {
                if !self.require_song() {
                    return;
                }
                if self.editor.song().expect("gated").is_vgm() {
                    self.status = "File is already in VGM format".to_owned();
                    return;
                }
                match self.editor.convert_to_vgm() {
                    Ok(()) => {
                        self.status = "Successfully converted to VGM".to_owned();
                        self.close_song_dialogs();
                        self.scroll_to = Some(0);
                        self.after_edit();
                    }
                    Err(message) => self.alerts.push_back(Alert::error(message)),
                }
            }
            Action::DeleteSelection => {
                if !self.require_song() {
                    return;
                }
                if self.editor.delete_selection() {
                    self.scroll_to = self.editor.selection.first();
                    self.after_edit();
                }
            }

            Action::OpenRipFolder => {
                if self.rip_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved package details?",
                        "This rip has unsaved changes. Open a different folder anyway?",
                        Action::ConfirmOpenRipFolder,
                    ));
                } else {
                    self.files.pick_folder();
                }
            }
            Action::ConfirmOpenRipFolder => self.files.pick_folder(),
            Action::SelectTab(tab) => self.select_tab(tab),
            Action::CloseRip => {
                if self.rip_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved package details?",
                        "This rip has unsaved changes. Close it anyway?",
                        Action::ConfirmCloseRip,
                    ));
                } else {
                    self.close_rip();
                }
            }
            Action::ConfirmCloseRip => self.close_rip(),
            Action::RipSaveDocs => self.save_rip_docs(),
            Action::RipExportZip => self.export_rip_zip(false),
            Action::ConfirmExportZip => self.export_rip_zip(true),
            Action::RipTrackOpen(index) => self.open_track_in_editor(index),
            Action::RipTrackPreview(index) => self.preview_track(index),
            Action::RipStopPreview => self.stop_preview(),
            Action::OpenTrackQuickEdit(index) => self.open_track_quick_edit(index),
            Action::OptimizeImage(index) => self.optimize_image(index),
            Action::QuickEditSubmitted {
                index,
                file_name,
                tag,
            } => self.quick_edit_submitted(index, file_name, *tag),

            Action::Help => self.alerts.push_back(Alert::new(HELP_TITLE, HELP_TEXT)),
            Action::About => self.alerts.push_back(Alert::new("About", about_text())),

            Action::Play => self.do_play(),
            Action::Stop => self.do_stop(),
            Action::PlayTail => self.do_play_tail(),
            Action::TogglePlayback => {
                if !self.require_song() {
                    return;
                }
                if self.audio.is_playing() {
                    self.do_stop();
                } else {
                    self.do_play();
                }
            }

            Action::NextDelay => self.delay_navigate(false),
            Action::PreviousDelay => self.delay_navigate(true),
            Action::SelectionMove { delta, extend } => {
                if let Some(row) = self
                    .editor
                    .selection
                    .key_move(delta, extend, self.editor.len())
                {
                    self.scroll_to = Some(row);
                }
            }
            Action::WaveformClicked { index, ms } => {
                self.editor.selection.select_only(index);
                // No scroll-into-view here, matching the Python's click path.
                if self.audio.is_playing() {
                    self.audio.seek_pos(index);
                }
                self.position.set_position_ms(ms);
            }
            Action::RewindToStart => {
                // Restart live playback from the top; snap the cursor and the
                // readout to zero whether or not anything is playing.
                self.audio.rewind();
                self.waveform.cursor_ms = 0;
                self.editor.selection.select_only(0);
                if self.audio.is_playing() {
                    self.audio.seek_pos(0);
                }
                self.position.set_position_ms(0);
            }

            Action::ToggleChannel(channel) => {
                self.channels.toggle_channel(channel);
                self.audio.set_muting(self.channels.muting());
            }
            Action::MutingChanged => self.audio.set_muting(self.channels.muting()),
            Action::SetBoost { value, persist } => {
                self.config.audio.boost = value;
                // A loaded stream gets the boost live via the command queue; an
                // unloaded one picks it up from `config.audio` on the next load,
                // so this deliberately does not force an audio reload.
                self.audio.set_boost(value);
                if persist && let Err(error) = self.config_store.save(&self.config) {
                    self.alerts
                        .push_back(Alert::error(format!("Could not save settings: {error}")));
                }
            }

            Action::Alert { title, message } => self.alerts.push_back(Alert::new(title, message)),
            Action::Status(message) => self.status = message,
            Action::GotoSubmitted(text) => self.goto_submitted(&text),
            Action::FindRegister { target, backwards } => self.find_register(&target, backwards),
            Action::UpdateHeader {
                opl_type,
                ms_length,
            } => {
                self.editor.update_header(opl_type, ms_length);
                self.after_edit();
            }
            Action::SaveGd3(tag) => self.editor.set_gd3_tag(*tag),
            Action::SaveVgmMetadata {
                loop_point,
                loop_base,
                loop_modifier,
                volume_modifier,
            } => {
                self.editor
                    .set_vgm_metadata(loop_point, loop_base, loop_modifier, volume_modifier)
            }
            Action::ApplySettings(config) => self.apply_settings(ctx, *config),
        }
    }

    // -- the workflows -----------------------------------------------------

    fn load_file(&mut self, file: PickedFile) {
        let name = file.name.clone();
        match self.editor.load(file) {
            Ok(report) => {
                self.status = format!("Successfully opened {name}.");
                // The Python left these dialogs open, still bound to the old
                // song object -- a stale Save then silently edited the wrong
                // song. Here they would edit the *new* one, which is worse,
                // so anything song-bound closes with the song.
                self.close_song_dialogs();
                self.waveform = WaveformState::default();
                self.submit_waveform(None);
                // A fresh song starts with every channel audible; stale
                // mute/solo state from the previous song must not carry over.
                self.channels = ChannelPanel::new();
                // Unload, not pause: the old stream's position must not leak
                // into the fresh cursor/readout via the end-of-playback
                // update below.
                self.audio.unload();
                self.peak_meter = PeakMeterState::default();
                self.audio_revision = None;
                self.was_playing = false;
                let song = self.editor.song().expect("just loaded");
                let file_version = song.file_version;
                self.position.set_length_ms(song.total_delay_ms());
                self.position.set_position_ms(0);
                self.last_first_selected = None;
                self.scroll_to = Some(0);
                self.push_load_warnings(report, file_version);
            }
            Err(message) => self
                .alerts
                .push_back(Alert::new("Failed to load file", message)),
        }
    }

    fn push_load_warnings(&mut self, report: LoadReport, file_version: u32) {
        if report.auto_trimmed {
            self.alerts
                .push_back(Alert::new(AUTO_TRIM_TITLE, AUTO_TRIM_TEXT));
        }
        if report.delay_mismatch {
            self.alerts
                .push_back(mismatch_alert(report.auto_trimmed, file_version));
        }
    }

    fn save(&mut self, force_dialog: bool) {
        if !self.require_song() {
            return;
        }
        let bytes = match self.editor.save_bytes() {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        let request = match (&self.editor.path, force_dialog) {
            (Some(path), false) => SaveRequest::InPlace {
                path: path.clone(),
                bytes,
            },
            _ => SaveRequest::Dialog {
                suggested_name: self.editor.song().expect("gated").name.clone(),
                bytes,
            },
        };
        self.pending_saves.push_back(SavePurpose::Song);
        self.files.save(request);
    }

    // -- rip mode ----------------------------------------------------------

    fn rip_is_dirty(&self) -> bool {
        self.rip.as_ref().is_some_and(|rip| rip.dirty)
    }

    /// Installs a freshly scanned folder as the rip project, or -- when it is a
    /// redelivery of the folder already open -- rescans in place, keeping the
    /// edited metadata.
    fn open_folder(&mut self, folder: PickedFolder) {
        let same = self
            .rip
            .as_ref()
            .is_some_and(|rip| rip.folder_path.is_some() && rip.folder_path == folder.path);
        if same {
            self.stop_preview();
            if let Some(rip) = self.rip.as_mut() {
                rip.refresh_files(folder);
            }
            return;
        }
        self.stop_preview();
        let today = self.rip_service.today();
        let state = RipState::from_folder(folder, today);
        let warning = state.parse_warning.clone();
        let name = state.folder_name.clone();
        self.rip = Some(state);
        self.active_tab = AppTab::Rip;
        self.close_song_dialogs();
        self.close_rip_dialogs();
        // The editor's audio must not keep playing under the rip view.
        self.audio.unload();
        self.audio_revision = None;
        self.status = format!("Opened rip project: {name}.");
        if let Some(warning) = warning {
            self.alerts.push_back(Alert::new(
                "Description not parsed",
                format!("{warning}\n\nSaving the package files will overwrite it."),
            ));
        }
    }

    fn select_tab(&mut self, tab: AppTab) {
        if self.rip.is_none() {
            self.active_tab = AppTab::Editor;
            return;
        }
        if self.active_tab == tab {
            return;
        }
        self.stop_preview();
        self.active_tab = tab;
        // Returning to the rip tab re-scans the folder so edits made in the
        // editor (or renames) are reflected.
        if tab == AppTab::Rip
            && let Some(path) = self.rip.as_ref().and_then(|rip| rip.folder_path.clone())
        {
            self.files.open_folder_path(path);
        }
    }

    fn close_rip(&mut self) {
        self.stop_preview();
        self.close_rip_dialogs();
        self.rip = None;
        self.active_tab = AppTab::Editor;
        self.status = "Closed the rip project.".to_owned();
    }

    /// Saves `Game Name.txt` and `Game Name.m3u` into the folder.
    fn save_rip_docs(&mut self) {
        if !self.rip.as_ref().is_some_and(RipState::can_save) {
            if self.rip.is_some() {
                self.alerts.push_back(Alert::error(
                    "Enter a game name before saving the package files.",
                ));
            }
            return;
        }
        let rip = self.rip.as_ref().expect("checked");
        let stem = rip.doc_stem();
        let description = rip.description_text().into_bytes();
        let m3u = rip.m3u_text(false).into_bytes();
        let folder = rip.folder_path.clone();
        let docs = [
            (format!("{stem}.txt"), description),
            (format!("{stem}.m3u"), m3u),
        ];
        for (name, bytes) in docs {
            self.pending_saves.push_back(SavePurpose::RipDoc);
            let request = match &folder {
                Some(folder) => SaveRequest::InPlace {
                    path: folder.join(&name),
                    bytes,
                },
                None => SaveRequest::Dialog {
                    suggested_name: name,
                    bytes,
                },
            };
            self.files.save(request);
        }
    }

    /// Builds and saves the release zip. Blocking validation errors abort;
    /// non-blocking warnings prompt first unless `confirmed`.
    fn export_rip_zip(&mut self, confirmed: bool) {
        let Some(rip) = self.rip.as_ref() else {
            return;
        };
        let validations = rip.validations();
        let request = rip.export_request();
        // The `rip` borrow ends here (validations and request are owned).
        if !validations.errors.is_empty() {
            self.alerts
                .push_back(Alert::error(validations.errors.join("\n")));
            return;
        }
        if !validations.warnings.is_empty() && !confirmed {
            self.alerts.push_back(Alert::confirm(
                "Export anyway?",
                format!("{}\n\nExport anyway?", validations.warnings.join("\n")),
                Action::ConfirmExportZip,
            ));
            return;
        }
        // Keep the folder's own docs in step with the zip's.
        if self.rip.as_ref().is_some_and(|rip| rip.dirty) {
            self.save_rip_docs();
        }
        self.rip_service.submit(request);
        self.status = "Building rip zip...".to_owned();
    }

    /// Previews a track through the audio output.
    fn preview_track(&mut self, index: usize) {
        let song = self
            .rip
            .as_ref()
            .and_then(|rip| rip.tracks.get(index))
            .and_then(|track| track.song().cloned());
        let Some(song) = song else {
            return;
        };
        self.audio.pause();
        if let Err(message) = self.audio.load(song, &self.config.audio) {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        if let Some(rip) = self.rip.as_mut() {
            rip.preview = Some(index);
        }
        // The editor's audio snapshot is now this preview; force a reload before
        // the editor's next play so it does not resume the wrong song.
        self.audio_revision = None;
    }

    fn stop_preview(&mut self) {
        if self.rip.as_ref().is_some_and(|rip| rip.preview.is_some()) {
            self.audio.pause();
            self.audio.rewind();
            if let Some(rip) = self.rip.as_mut() {
                rip.preview = None;
            }
            self.audio_revision = None;
        }
    }

    /// Loads a track into the editor and switches to the editor tab. The rip
    /// project is retained; returning to it rescans the folder.
    fn open_track_in_editor(&mut self, index: usize) {
        self.stop_preview();
        let file = self
            .rip
            .as_ref()
            .and_then(|rip| rip.tracks.get(index))
            .map(|track| PickedFile {
                name: track.file_name.clone(),
                path: track.path.clone(),
                bytes: track.bytes.clone(),
            });
        let Some(file) = file else {
            return;
        };
        self.load_file(file);
        self.active_tab = AppTab::Editor;
    }

    fn open_track_quick_edit(&mut self, index: usize) {
        let dialog = self
            .rip
            .as_ref()
            .and_then(|rip| rip.tracks.get(index))
            .and_then(|track| {
                let song = track.song()?;
                let tag = song.vgm_meta().and_then(|meta| meta.tag.as_ref());
                Some(TrackEditDialog::new(index, track.file_name.clone(), tag))
            });
        if let Some(dialog) = dialog {
            self.dialogs.track_edit = Some(dialog);
        }
    }

    /// Applies a quick edit: rewrite the track's bytes with the new tag (and, if
    /// the name changed, rename the file). The list rescans on the outcomes.
    fn quick_edit_submitted(&mut self, index: usize, new_name: String, tag: Gd3Tag) {
        self.stop_preview();
        let Some(track) = self.rip.as_ref().and_then(|rip| rip.tracks.get(index)) else {
            return;
        };
        let old_name = track.file_name.clone();
        let old_path = track.path.clone();
        let bytes = match track.song() {
            Some(song) => crate::rip::retagged_bytes(song, &new_name, tag),
            None => return,
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        if let Some(path) = old_path.clone() {
            self.pending_saves.push_back(SavePurpose::TrackRewrite);
            self.files.save(SaveRequest::InPlace { path, bytes });
        }
        if new_name != old_name {
            if let Some(path) = old_path.clone() {
                self.files.rename(path, new_name.clone());
            }
            // If the renamed file is the one open in the editor, drop its stale
            // path so a later Ctrl+S does not resurrect the old name.
            if self.editor.path == old_path {
                self.editor.path = None;
            }
        }
        self.status = format!("Updated {new_name}.");
    }

    /// Kicks off an explicit lossless recompression of a screenshot.
    fn optimize_image(&mut self, index: usize) {
        let image = self
            .rip
            .as_ref()
            .and_then(|rip| rip.images.get(index))
            .cloned();
        let Some(image) = image else {
            return;
        };
        self.status = format!("Optimising {}...", image.name);
        self.rip_service.optimize(image.name, image.bytes);
    }

    /// Routes a finished optimisation: save a smaller file in place, or report
    /// that the original was already optimal.
    fn image_optimized(&mut self, optimized: OptimizedImage) {
        if optimized.bytes.len() >= optimized.original_len {
            self.status = format!(
                "{} is already optimal ({} bytes).",
                optimized.name, optimized.original_len
            );
            return;
        }
        let path = self.rip.as_ref().and_then(|rip| {
            rip.images
                .iter()
                .find(|image| image.name == optimized.name)
                .and_then(|image| image.path.clone())
        });
        let Some(path) = path else {
            self.status = format!("{}: no file path to save to.", optimized.name);
            return;
        };
        self.status = format!(
            "{}: {} -> {} bytes.",
            optimized.name,
            optimized.original_len,
            optimized.bytes.len()
        );
        self.pending_saves.push_back(SavePurpose::ImageOptimised);
        self.files.save(SaveRequest::InPlace {
            path,
            bytes: optimized.bytes,
        });
    }

    fn rescan_rip_folder(&mut self) {
        if let Some(path) = self.rip.as_ref().and_then(|rip| rip.folder_path.clone()) {
            self.files.open_folder_path(path);
        }
    }

    /// Closes rip-bound dialogs (the quick-edit dialog), analogous to
    /// [`Self::close_song_dialogs`].
    fn close_rip_dialogs(&mut self) {
        self.dialogs.track_edit = None;
    }

    fn do_play(&mut self) {
        if !self.require_song() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.rewind();
        if let Some(first) = self.editor.selection.first() {
            self.audio.seek_pos(first);
        }
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    fn do_stop(&mut self) {
        if !self.require_song() {
            return;
        }
        self.audio.pause();
        self.audio.rewind();
    }

    fn do_play_tail(&mut self) {
        if !self.require_song() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Measured length, not the header's: on a DRO whose header overstates
        // the length, the Python seeked past the end and played nothing.
        let total = self.editor.song().expect("gated").total_delay_ms();
        self.audio.rewind();
        self.audio
            .seek_ms(total.saturating_sub(self.config.ui.tail_length));
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    fn delay_navigate(&mut self, backwards: bool) {
        if !self.require_song() {
            return;
        }
        match self.editor.find_next(FindTarget::AnyDelay, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(index);
            }
            None => self.status = "No more delays found.".to_owned(),
        }
    }

    fn goto_submitted(&mut self, text: &str) {
        if !self.require_song() {
            return;
        }
        let len = self.editor.len() as i64;
        match text.trim().parse::<i64>() {
            Err(_) => self.status = format!("Invalid position for goto: {text}"),
            Ok(position) if position < 0 || position >= len => {
                self.status = format!("Position for goto is out of range: {position}");
            }
            Ok(position) => {
                let position = usize::try_from(position).expect("bounds checked");
                self.editor.selection.select_only(position);
                self.scroll_to = Some(position);
                self.status = format!("Gone to position: {position}");
            }
        }
    }

    fn find_register(&mut self, target: &str, backwards: bool) {
        // An empty choice is a silent no-op, as in Python.
        if target.is_empty() || !self.require_song() {
            return;
        }
        let Ok(parsed) = target.parse::<FindTarget>() else {
            return;
        };
        match self.editor.find_next(parsed, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(index);
                self.status = format!("Occurrence of {target} found at position {index}.");
            }
            None => self.status = format!("Could not find another occurrence of {target}."),
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context, config: AppConfig) {
        if let Err(error) = self.config_store.save(&config) {
            self.alerts
                .push_back(Alert::error(format!("Could not save settings: {error}")));
        }
        // Repaint the whole UI in the new scheme before anything else reads it.
        if config.ui.theme != self.config.ui.theme {
            theme::apply_palette(ctx, config.ui.theme);
        }
        // Only an audio change needs an output reload or a fresh waveform; a
        // theme-only change keeps the existing buckets and just recolours them.
        let audio_changed = config.audio != self.config.audio;
        let waveform_changed = config.audio.frequency != self.config.audio.frequency
            || config.audio.chip_write_delay != self.config.audio.chip_write_delay;
        self.config = config;
        self.position.set_frequency(config.audio.frequency);
        if let Some(song) = self.editor.song() {
            self.position.set_length_ms(song.total_delay_ms());
        }
        if audio_changed {
            // Reload the audio output lazily on the next play.
            self.audio_revision = None;
        }
        if waveform_changed {
            self.submit_waveform(None);
        }
        self.status = "Settings saved.".to_owned();
    }

    // -- helpers -------------------------------------------------------------

    /// Closes every dialog bound to the current song. Goto (validated live)
    /// and Settings (song-independent) survive.
    fn close_song_dialogs(&mut self) {
        self.dialogs.find_reg = None;
        self.dialogs.dro_info = None;
        self.dialogs.gd3_tag = None;
        self.dialogs.vgm_metadata = None;
    }

    /// The `@requires_dro_loaded` decorator: gates an action on a loaded song,
    /// with the Python's exact status message.
    fn require_song(&mut self) -> bool {
        if self.editor.has_song() {
            true
        } else {
            self.status = "Please open a DRO file first.".to_owned();
            false
        }
    }

    /// Everything every edit needs: stale audio paused, the length readout
    /// refreshed, and the waveform re-rendered (debounced, so holding Delete
    /// does not thrash the renderer -- the Python used the same 1 s debounce).
    fn after_edit(&mut self) {
        self.audio.pause();
        self.audio_revision = None;
        if let Some(song) = self.editor.song() {
            self.position.set_length_ms(song.total_delay_ms());
        }
        self.waveform.buckets.clear();
        self.submit_waveform(Some(Duration::from_secs(1)));
        // The selected row's time may have changed; force the indicator sync.
        self.last_first_selected = None;
    }

    fn submit_waveform(&mut self, debounce: Option<Duration>) {
        let Some(snapshot) = self.editor.snapshot() else {
            return;
        };
        self.tasks.submit(
            TaskRequest::RenderWaveform {
                song: snapshot,
                num_buckets: waveform::NUM_BUCKETS,
                sample_rate: self.config.audio.frequency,
                chip_write_delay: self.config.audio.chip_write_delay,
            },
            debounce,
        );
    }

    /// Loads the current song into the audio output if it is not already
    /// there. Cheap when nothing changed.
    fn ensure_audio(&mut self) -> Result<(), String> {
        if self.audio_revision == Some(self.editor.revision()) {
            return Ok(());
        }
        let snapshot = self
            .editor
            .snapshot()
            .ok_or_else(|| "No song is loaded.".to_owned())?;
        self.audio.load(snapshot, &self.config.audio)?;
        self.audio.set_muting(self.channels.muting());
        self.audio_revision = Some(self.editor.revision());
        // The device may have rejected the configured frequency; positions
        // report frames at the stream's real rate, so the panel must too.
        if let Some(rate) = self.audio.output_rate() {
            self.position.set_frequency(rate);
            if let Some(song) = self.editor.song() {
                self.position.set_length_ms(song.total_delay_ms());
            }
        }
        Ok(())
    }

    fn menu_state(&self) -> MenuState {
        MenuState {
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            undo_description: self.editor.undo_description().map(str::to_owned),
            redo_description: self.editor.redo_description().map(str::to_owned),
            has_rip: self.rip.is_some(),
            on_rip_tab: self.active_tab == AppTab::Rip,
        }
    }

    /// `"Play last 3 seconds"`, with the Python's exact formatting: two
    /// decimals only for fractional lengths, singular for exactly one second.
    fn play_tail_label(&self) -> String {
        let ms = self.config.ui.tail_length;
        let value = if ms.is_multiple_of(1000) {
            (ms / 1000).to_string()
        } else {
            format!("{:.2}", f64::from(ms) / 1000.0)
        };
        let plural = if ms == 1000 { "" } else { "s" };
        format!("Play last {value} second{plural}")
    }
}

impl eframe::App for DroApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_impl(ui);
    }

    fn on_exit(&mut self) {
        self.audio.unload();
        self.tasks.shutdown();
    }
}

// The headless GUI tests live in their own file but mount here, as a child
// module of `app`, so they can read `DroApp`'s private fields directly.
#[cfg(test)]
#[path = "app_gui_tests.rs"]
mod gui_tests;

impl fmt::Debug for DroApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DroApp")
            .field("editor", &self.editor)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
