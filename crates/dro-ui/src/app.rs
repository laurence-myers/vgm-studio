//! The application: `wxapp.DTApp` + `containers.DTMainFrame`, as one
//! `eframe::App` driven entirely through the platform-service traits.

use core::fmt;
use core::time::Duration;
use std::collections::VecDeque;
use std::path::PathBuf;

use dro_core::config::{AppConfig, ConfigStore};
use dro_core::song::{DRO_FILE_V2, SongFileType};
use dro_core::{FindTarget, Gd3Tag};
use dro_synth::{LoopConfig, LoopCount, Muting, Panning, RenderMix, SplitFormat, SplitOptions};
use egui::Key;

use crate::action::{Action, AppTab};
use crate::alert::{self, Alert};
use crate::dialogs::{
    Dialogs, DroInfoDialog, FindRegDialog, Gd3TagDialog, GotoDialog, RenderWavDialog,
    SettingsDialog, SplitDialog, TrackEditDialog, VgmMetadataDialog,
};
use crate::editor::{Editor, LoadReport};
use crate::markers::RangeMarkers;
use crate::menus::{self, MenuState};
use crate::platform::{
    AudioService, FileService, OptimizedImage, PickedFile, PickedFolder, RipJobOutcome, RipService,
    SaveOutcome, SaveRequest,
};
use crate::rip::{RipMutation, RipState, RipTransaction};
use crate::tasks::{TaskKind, TaskRequest, TaskResult, TaskService};
use crate::theme::{self, Palette};
use crate::widgets::peak_meter::PeakMeterState;
use crate::widgets::position_panel::PositionPanel;
use crate::widgets::waveform::WaveformState;
use crate::widgets::{
    boost_stepper, channels::ChannelPanel, loop_stepper, peak_meter, table, waveform,
};

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
/// What a click on the waveform means, given the button and whether Shift was
/// held. `None` for a gesture that does nothing.
///
/// Shift brackets the loop -- left marks the start, right the end -- so the two
/// markers are one gesture apart rather than one being a modifier deeper than
/// the other. The end is the *time* clicked, hence that instruction's index
/// taken exclusively: everything sounding before the click is inside the loop.
fn waveform_action(index: usize, ms: u32, secondary: bool, shift: bool) -> Option<Action> {
    match (shift, secondary) {
        (true, false) => Some(Action::SetLoopStart(index)),
        (true, true) => Some(Action::SetLoopEnd(index)),
        // A plain right-click marks nothing; seeking is the left button's job.
        (false, true) => None,
        (false, false) => Some(Action::WaveformClicked { index, ms }),
    }
}

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
    /// A WAV rendered by File > Render to WAV.
    WavExport,
    /// One of the per-channel files from File > Split Channels.
    SplitFile,
    /// A rip project's description or playlist.
    RipDoc,
    /// A track rewritten in place by the quick-edit dialog.
    TrackRewrite,
    /// A screenshot rewritten in place after an explicit optimise.
    ImageOptimised,
    /// The exported release zip (a Save-As dialog).
    ExportZip,
    /// A `Write` step of the rip file-op executor (reorder / undo / redo).
    RipOp,
}

/// The stages of File > Split Channels: choose a folder, render into it, write
/// the files out.
#[derive(Debug, Clone)]
enum SplitFlow {
    /// The options are chosen; the folder picker is up.
    AwaitingFolder { options: SplitOptions },
    /// The split is rendering, bound for `dir`.
    Rendering { dir: PathBuf },
    /// Writing the outputs, counting them off as their saves land.
    Writing {
        dir: PathBuf,
        written: usize,
        failed: bool,
    },
}

/// Whether the running file-op sequence is a fresh edit, a redo, or an undo --
/// deciding which stack its transaction lands on when the sequence completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RipRunKind {
    /// A brand-new edit (reorder): push to undo, clear the redo stack.
    NewEdit,
    /// Re-applying a previously undone edit: push back to undo.
    Redo,
    /// Reverting an edit: push to redo.
    Undo,
}

/// A rip file-op sequence in flight: the mutations still to run, the transaction
/// they belong to, and where it lands on completion. Runs one mutation at a time,
/// advancing as each rename/write outcome arrives.
struct RipRun {
    queue: VecDeque<RipMutation>,
    transaction: RipTransaction,
    kind: RipRunKind,
    /// Set while a `Rename` mutation is awaiting its `poll_renamed`, so that
    /// outcome advances the run rather than the quick-edit rename path.
    rename_in_flight: bool,
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
    /// How far along File > Split Channels is, if it is running at all. Doubles
    /// as the in-flight guard and as the gate that drops a result belonging to a
    /// split the user has since abandoned.
    split_flow: Option<SplitFlow>,

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
    /// Whether playback repeats the marked region. Off by default: Play means
    /// "play the song" until asked otherwise.
    loop_enabled: bool,
    /// How many times the region repeats while looping.
    loop_count: LoopCount,
    /// Whether the previous frame was playing, so the frame after playback
    /// ends can display the exact final position.
    was_playing: bool,
    /// A file passed on the command line, loaded on the first frame.
    pending_open: Option<PickedFile>,
    /// A file waiting behind the discard-changes prompt; loaded if confirmed.
    pending_load: Option<PickedFile>,
    /// Set once the user confirms quitting past unsaved changes, so the
    /// close interception lets the next close request through.
    quitting: bool,
    /// A quick-edit byte rewrite deferred until its rename lands, so a failed
    /// rename can't leave the old file holding bytes in the new format.
    pending_rewrite: Option<(PathBuf, Vec<u8>)>,
    /// Set if any package-doc save in the current batch failed or was cancelled,
    /// so the rip's dirty flag is kept rather than cleared once the batch ends.
    rip_docs_failed: bool,
    /// The rip file-op sequence currently executing (reorder / undo / redo), if
    /// any. Only one runs at a time; edits are ignored while it is `Some`.
    rip_run: Option<RipRun>,
    /// Applied rip edits available to undo, oldest first.
    rip_undo: Vec<RipTransaction>,
    /// Undone rip edits available to redo.
    rip_redo: Vec<RipTransaction>,
    /// A quick-edit / optimise transaction whose forward ran through the bespoke
    /// save path; committed to the undo stack once that save succeeds (and
    /// dropped if it fails), so undo only ever reverses edits that landed.
    pending_rip_undo: Option<RipTransaction>,
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
            split_flow: None,
            waveform: WaveformState::default(),
            peak_meter: PeakMeterState::default(),
            position: PositionPanel::new(config.audio.frequency),
            channels: ChannelPanel::new(),
            scroll_to: None,
            last_first_selected: None,
            audio_revision: None,
            loop_enabled: false,
            loop_count: LoopCount::Infinite,
            was_playing: false,
            pending_open: initial_file,
            pending_load: None,
            quitting: false,
            pending_rewrite: None,
            rip_docs_failed: false,
            rip_run: None,
            rip_undo: Vec::new(),
            rip_redo: Vec::new(),
            pending_rip_undo: None,
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
        self.intercept_close(&ctx);
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
                                actions.extend(waveform_action(
                                    index,
                                    ms,
                                    response.secondary,
                                    response.modifiers.shift,
                                ));
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
                        // Name the job rather than just "busy": a WAV render can
                        // take a while, and the waveform's own render runs after
                        // every edit.
                        if self.tasks.is_busy_kind(TaskKind::RenderWav) {
                            ui.label("Rendering WAV...");
                        }
                        if self.tasks.is_busy_kind(TaskKind::Split) {
                            ui.label("Splitting channels...");
                        }
                        if self.tasks.is_busy_kind(TaskKind::RenderWaveform) {
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
                        if theme::bevel::button(ui, p, "Seam")
                            .on_hover_text(self.play_seam_label())
                            .clicked()
                        {
                            actions.push(Action::PlaySeam);
                        }
                        let mut looping = self.loop_enabled;
                        if theme::bevel::toggle(ui, p, &mut looping, "Loop")
                            .on_hover_text(
                                "Repeat the marked region. Shift+click the waveform to mark \
                                 the start and Shift+right-click the end; [ and ] use the \
                                 selected row.",
                            )
                            .clicked()
                        {
                            actions.push(Action::ToggleLoopPlayback);
                        }
                        loop_stepper::loop_count_stepper(ui, p, self.loop_count, &mut actions);
                        boost_stepper::boost_stepper(ui, p, self.config.audio.boost, &mut actions);
                    });
                    ui.add_space(PAD);
                    theme::separator_full(ui, p);
                    ui.add_space(PAD);
                    // The panel hides its own high bank for a plain OPL2 song.
                    let channels = self.channels.show(ui, p);
                    if channels.muting_changed {
                        actions.push(Action::MutingChanged);
                    }
                    if channels.panning_changed {
                        actions.push(Action::PanningChanged);
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
        // Cheap, and derived from three separate pieces of state (markers, the
        // loop toggle, the song's stored loop), so it is refreshed per frame
        // rather than at each of the places any of them can change.
        self.sync_loop_overlay();
        self.playback_tick(&ctx);
    }

    // -- frame plumbing ------------------------------------------------------

    fn poll_services(&mut self) {
        if let Some(result) = self.files.poll_picked() {
            match result {
                Ok(file) => self.load_or_confirm(file),
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
            let is_rip_op = self
                .rip_run
                .as_ref()
                .is_some_and(|run| run.rename_in_flight);
            match result {
                Ok(()) if is_rip_op => {
                    if let Some(run) = self.rip_run.as_mut() {
                        run.rename_in_flight = false;
                    }
                    self.advance_rip_run();
                }
                Ok(()) => {
                    // A quick-edit rename paired with a byte rewrite: now that the
                    // file has its new name, write the target-format bytes to it
                    // (its own TrackRewrite outcome then rescans the folder).
                    if let Some((path, bytes)) = self.pending_rewrite.take() {
                        self.pending_saves.push_back(SavePurpose::TrackRewrite);
                        self.files.save(SaveRequest::InPlace { path, bytes });
                    } else {
                        self.rescan_rip_folder();
                        self.status = "Renamed track; rip folder rescanned.".to_owned();
                    }
                }
                Err(message) if is_rip_op => self.abort_rip_run(message),
                Err(message) => {
                    self.pending_rewrite = None;
                    self.alerts.push_back(Alert::new("Rename failed", message));
                }
            }
        }
        if let Some(chosen) = self.files.poll_output_folder() {
            self.split_into(chosen);
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
                RipJobOutcome::Failed(message) => {
                    // Replace the stale "Building rip zip..." status (ux-11).
                    self.status = "Rip export failed.".to_owned();
                    self.alerts
                        .push_back(Alert::new("Rip export failed", message));
                }
            }
        }
        if let Some(result) = self.rip_service.poll_optimized() {
            match result {
                Ok(optimized) => self.image_optimized(optimized),
                Err(message) => {
                    self.status = "Screenshot optimise failed.".to_owned();
                    self.alerts
                        .push_back(Alert::new("Optimise failed", message));
                }
            }
        }
        for result in self.tasks.poll() {
            match result {
                TaskResult::Waveform(buckets) => self.waveform.buckets = buckets,
                TaskResult::Wav(rendered) => self.handle_wav_result(rendered),
                TaskResult::Split(outputs) => self.write_split(outputs),
            }
        }
    }

    /// Offers a finished render to the save dialog, or reports why there is
    /// nothing to offer.
    ///
    /// The picker blocks the UI thread, but only once the long part is done --
    /// the same shape as the rip zip export.
    fn handle_wav_result(&mut self, rendered: Result<(String, Vec<u8>), String>) {
        match rendered {
            Ok((name, bytes)) => {
                self.pending_saves.push_back(SavePurpose::WavExport);
                self.files.save(SaveRequest::Dialog {
                    suggested_name: name,
                    bytes,
                });
            }
            Err(message) => {
                self.status = "The WAV render failed.".to_owned();
                self.alerts.push_back(Alert::error(message));
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
                    // The song on disk now matches the editor: mark it clean so
                    // the discard-changes prompts stop firing.
                    self.editor.mark_saved();
                    self.status = format!("File saved to {shown}.");
                }
                SavePurpose::RipDoc => {
                    // The description and playlist save back to back; report and
                    // clear the dirty flag once the last of them lands -- but only
                    // if none of the batch failed, so edits aren't lost (uishell-7).
                    let more = self
                        .pending_saves
                        .iter()
                        .any(|purpose| *purpose == SavePurpose::RipDoc);
                    if !more {
                        let stem = self
                            .rip
                            .as_ref()
                            .map_or_else(String::new, RipState::doc_stem);
                        if self.rip_docs_failed {
                            self.status =
                                "Some package files could not be saved; changes kept.".to_owned();
                        } else {
                            if let Some(rip) = self.rip.as_mut() {
                                rip.dirty = false;
                            }
                            self.status = format!("Saved {stem}.txt and {stem}.m3u.");
                        }
                    }
                }
                SavePurpose::TrackRewrite | SavePurpose::ImageOptimised => {
                    // The file's bytes were rewritten; rescan so the list (or
                    // the inline screenshot and its size) reflects the change. A
                    // rename, if any, rescans on its own outcome too -- both
                    // refresh in place, harmlessly. The edit landed, so its undo
                    // transaction (stashed at submit) becomes reversible.
                    if let Some(transaction) = self.pending_rip_undo.take() {
                        self.rip_undo.push(transaction);
                        self.rip_redo.clear();
                    }
                    self.rescan_rip_folder();
                }
                SavePurpose::RipOp => self.advance_rip_run(),
                SavePurpose::ExportZip => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = format!("Exported {shown}.");
                }
                SavePurpose::WavExport => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = format!("Rendered {shown}.");
                }
                SavePurpose::SplitFile => self.split_file_saved(true),
            },
            SaveOutcome::Cancelled => match purpose {
                SavePurpose::RipDoc => self.rip_docs_failed = true,
                SavePurpose::RipOp => self.abort_rip_run("The save was cancelled.".to_owned()),
                SavePurpose::TrackRewrite | SavePurpose::ImageOptimised => {
                    self.pending_rip_undo = None;
                }
                // Split files save in place, so there is no picker to cancel --
                // but the tally still has to move on, or the batch never ends.
                SavePurpose::SplitFile => self.split_file_saved(false),
                _ => {}
            },
            SaveOutcome::Failed(message) => match purpose {
                SavePurpose::RipOp => self.abort_rip_run(message),
                SavePurpose::SplitFile => {
                    // One alert at the end for the whole batch, not eighteen.
                    log::warn!("split file could not be written: {message}");
                    self.split_file_saved(false);
                }
                other => {
                    if other == SavePurpose::RipDoc {
                        self.rip_docs_failed = true;
                    }
                    if matches!(
                        other,
                        SavePurpose::TrackRewrite | SavePurpose::ImageOptimised
                    ) {
                        self.pending_rip_undo = None;
                    }
                    self.alerts
                        .push_back(Alert::new("Failed to save file", message));
                }
            },
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // Only single-file drops, as in Python; say so rather than silently
        // ignoring a multi-drop (ux-17).
        if dropped.len() > 1 {
            self.status = "Drop a single file at a time.".to_owned();
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
            } else {
                self.status = format!("Can't open {name}: unsupported file type.");
            }
        } else if let Some(path) = file.path {
            // Native: a song opens in the editor; anything else (a folder, which
            // has no extension) is handed to the file service, which routes a
            // directory into rip mode. A junk file surfaces the usual "bad
            // format" alert.
            if is_song || path.extension().is_none() {
                self.files.open_path(path);
            } else {
                self.status = format!("Can't open {name}: unsupported file type.");
            }
        }
    }

    /// Cancels a window-close request while there are unsaved changes, raising a
    /// discard-changes confirm instead. A confirmed quit (`quitting`) is let
    /// straight through.
    fn intercept_close(&mut self, ctx: &egui::Context) {
        if self.quitting || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.editor.is_dirty() || self.rip_is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let already_asking = self
                .alerts
                .iter()
                .any(|alert| alert.confirm.as_deref() == Some(&Action::ConfirmExit));
            if !already_asking {
                self.alerts.push_back(Alert::confirm(
                    "Discard unsaved changes?",
                    "You have unsaved changes. Quit anyway?",
                    Action::ConfirmExit,
                ));
            }
        }
    }

    /// Loads `file` into the editor, or -- if the current song has unsaved edits
    /// -- stashes it behind a discard-changes confirm first.
    fn load_or_confirm(&mut self, file: PickedFile) {
        if self.editor.is_dirty() {
            self.pending_load = Some(file);
            self.alerts.push_back(Alert::confirm(
                "Discard unsaved changes?",
                "The current song has unsaved changes. Open a different file anyway?",
                Action::ConfirmDiscardAndLoad,
            ));
        } else {
            self.load_file(file);
        }
    }

    fn gather_key_input(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // An alert or any open dialog owns the keyboard: the editor's shortcuts
        // (Space, Delete, ...) must not fire behind it, and Ctrl+Z in a tag field
        // must undo the *text*, not the song. Unlike egui_wants_keyboard_input(),
        // this does NOT fire when a chrome button merely holds focus -- the editor
        // view has no text inputs, so a stray Tab onto a button used to disable
        // every shortcut and let Space "click" the focused button (e.g. delete).
        if !self.alerts.is_empty() || self.dialogs.any_open() {
            return;
        }
        // The rip tab hides the editor, so the editor's playback/navigation keys
        // must not fire there. Save (the package files), Undo/Redo (the file
        // edits) and Help remain.
        if self.active_tab == AppTab::Rip {
            ctx.input_mut(|input| {
                if input.consume_shortcut(&menus::SAVE) {
                    actions.push(Action::RipSaveDocs);
                }
                // Shifted variants first (egui ignores a surplus Shift).
                if input.consume_shortcut(&menus::REDO_ALT) {
                    actions.push(Action::Redo);
                }
                if input.consume_shortcut(&menus::UNDO) {
                    actions.push(Action::Undo);
                }
                if input.consume_shortcut(&menus::REDO) {
                    actions.push(Action::Redo);
                }
                if input.consume_shortcut(&menus::HELP) {
                    actions.push(Action::Help);
                }
            });
            return;
        }
        ctx.input_mut(|input| {
            // The editor view has no focusable text, so swallow Tab/Shift+Tab: a
            // stray Tab would otherwise move focus onto a chrome button, where
            // Space activates it (e.g. "Del.") instead of toggling playback.
            input.consume_key(egui::Modifiers::NONE, Key::Tab);
            input.consume_key(egui::Modifiers::SHIFT, Key::Tab);
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
            let mods = input.modifiers;
            // Plain editor keys must not fire with Command/Alt held (those form
            // menu shortcuts, handled above). Shift stays meaningful: extend the
            // selection on the arrows, and select the high channel bank on the
            // number row.
            if mods.command || mods.alt {
                return;
            }
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
                    extend: mods.shift,
                });
            }
            if input.key_pressed(Key::ArrowDown) {
                actions.push(Action::SelectionMove {
                    delta: 1,
                    extend: mods.shift,
                });
            }
            // [ and ] bracket the loop around the focused row -- the fastest way
            // to mark a region, since the table is where an exact instruction is
            // found. The end is exclusive, so ] marks *past* the focused row,
            // taking it into the loop rather than stopping just short of it.
            if let Some(row) = self.editor.selection.first() {
                if input.key_pressed(Key::OpenBracket) {
                    actions.push(Action::SetLoopStart(row));
                }
                if input.key_pressed(Key::CloseBracket) {
                    actions.push(Action::SetLoopEnd(row + 1));
                }
            }
            // 1..9 toggle channels 0..8; Shift+1..9 the high bank, channels 9..17.
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
            let bank = if mods.shift { 9 } else { 0 };
            for (offset, key) in NUMBER_KEYS.into_iter().enumerate() {
                if input.key_pressed(key) {
                    actions.push(Action::ToggleChannel(bank + offset));
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
                    // A wrap rewinds the engine's frame count to the loop start,
                    // so the cursor and readout follow the loop without any
                    // special handling here.
                    self.waveform.cursor_ms = position.elapsed_ms;
                    self.position.set_position(position);
                    self.position.set_loop_progress(
                        (self.loop_enabled && playing)
                            .then_some((position.loop_iteration, self.loop_count)),
                    );
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
            Action::OpenRenderWav => {
                if self.require_song() {
                    self.dialogs.render_wav = Some(RenderWavDialog::new(self.config.audio.boost));
                }
            }
            Action::RenderWavSubmitted {
                use_toggles,
                use_panning,
                boost,
            } => self.render_to_wav(use_toggles, use_panning, boost),
            Action::OpenSplit => {
                if !self.require_song() {
                    return;
                }
                if self.split_is_running() {
                    self.status = "Already splitting channels.".to_owned();
                    return;
                }
                self.dialogs.split = Some(SplitDialog::new());
            }
            Action::SplitSubmitted {
                format,
                isolate_percussion,
            } => self.start_split(format, isolate_percussion),
            Action::OpenSettings => {
                self.dialogs.settings = Some(SettingsDialog::new(&self.config));
            }
            Action::Exit => {
                if self.editor.is_dirty() || self.rip_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved changes?",
                        "You have unsaved changes. Quit anyway?",
                        Action::ConfirmExit,
                    ));
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::ConfirmExit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::ConfirmDiscardAndLoad => {
                if let Some(file) = self.pending_load.take() {
                    self.load_file(file);
                }
            }

            Action::Undo => {
                // On the rip tab, Undo reverses the last file edit; on the editor
                // tab it reverses the last song edit.
                if self.active_tab == AppTab::Rip {
                    self.undo_rip_edit();
                } else if self.require_song() {
                    match self.editor.undo() {
                        Some(description) => {
                            self.status = format!("Undone: {description}");
                            self.after_edit();
                        }
                        None => self.status = "Nothing to undo.".to_owned(),
                    }
                }
            }
            Action::Redo => {
                if self.active_tab == AppTab::Rip {
                    self.redo_rip_edit();
                } else if self.require_song() {
                    match self.editor.redo() {
                        Some(description) => {
                            self.status = format!("Redone: {description}");
                            self.after_edit();
                        }
                        None => self.status = "Nothing to redo.".to_owned(),
                    }
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
                    // The menu hides this for a VGM, so the shortcut must agree
                    // -- otherwise Ctrl+I opens a dialog the menu says does not
                    // apply. A VGM's header is the VGM Metadata dialog's job.
                    if song.is_vgm() {
                        self.status =
                            "DRO Info applies to DRO files; use Edit VGM Metadata.".to_owned();
                        return;
                    }
                    let edit_allowed = self.config.ui.dro_info_edit_enabled;
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
            Action::ConvertToDro1 => {
                if !self.require_song() {
                    return;
                }
                match self.editor.convert_to_dro1() {
                    Ok(()) => {
                        self.status = "Successfully converted to DRO v1".to_owned();
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
            Action::RipMoveTrack { index, delta } => self.move_rip_track(index, delta),
            Action::OptimizeImage(index) => self.optimize_image(index),
            Action::QuickEditSubmitted {
                original_name,
                file_name,
                tag,
            } => self.quick_edit_submitted(original_name, file_name, *tag),

            Action::Help => self.alerts.push_back(Alert::new(HELP_TITLE, HELP_TEXT)),
            Action::About => self.alerts.push_back(Alert::new("About", about_text())),

            Action::Play => self.do_play(),
            Action::Stop => self.do_stop(),
            Action::PlayTail => self.do_play_tail(),
            Action::PlaySeam => self.do_play_seam(),
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

            Action::SetLoopStart(index) => self.set_loop_marker(Some(index), None),
            Action::SetLoopEnd(index) => self.set_loop_marker(None, Some(index)),
            Action::ClearLoopMarkers => {
                self.editor.markers = RangeMarkers::full(self.editor.len());
                self.push_loop_config();
                self.status = "Loop markers reset to the whole song.".to_owned();
            }
            Action::ToggleLoopPlayback => {
                self.loop_enabled = !self.loop_enabled;
                self.push_loop_config();
            }
            Action::SetLoopCount(count) => {
                self.loop_count = count;
                self.push_loop_config();
            }
            Action::ApplyLoopToMetadata => self.apply_loop_to_metadata(),

            Action::ToggleChannel(channel) => {
                self.channels.toggle_channel(channel);
                self.audio.set_muting(self.channels.muting());
            }
            Action::MutingChanged => self.audio.set_muting(self.channels.muting()),
            Action::PanningChanged => self.audio.set_panning(self.channels.panning()),
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
                // The chip type may have changed the high-bank visibility and the
                // Original pan policy; after_edit invalidates the audio revision,
                // so the next ensure_audio pushes the fresh panning.
                self.channels.set_opl_type(opl_type, self.editor.song());
                self.after_edit();
            }
            Action::SaveGd3(tag) => self.editor.set_gd3_tag(*tag),
            Action::SaveVgmMetadata {
                loop_point,
                loop_end,
                loop_base,
                loop_modifier,
                volume_modifier,
            } => {
                let dropped = self.editor.set_vgm_metadata(
                    loop_point,
                    loop_end,
                    loop_base,
                    loop_modifier,
                    volume_modifier,
                );
                // The stored loop is now the marked one, so re-arm playback.
                self.push_loop_config();
                if dropped {
                    self.alerts.push_back(Alert::new(
                        "Loop point cleared",
                        "The loop start was past the end of the song (shortened since the \
                         dialog opened) and has been cleared.",
                    ));
                } else {
                    self.status = "Updated VGM metadata.".to_owned();
                }
            }
            Action::ApplySettings(config) => self.apply_settings(ctx, *config),
        }
    }

    // -- the workflows -----------------------------------------------------

    fn load_file(&mut self, file: PickedFile) {
        // Loading a song belongs to the editor: stop any rip preview and show
        // the editor tab so the load isn't invisible (menu Open, drag-and-drop,
        // and the CLI initial load can all fire while the rip tab is active).
        // Idempotent with open_track_in_editor, which also sets the tab.
        self.stop_preview();
        self.active_tab = AppTab::Editor;
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
                // The exports belong to the song being replaced; drop them
                // rather than write out a song no longer on screen. (Their own
                // kinds, so this does not disturb the waveform render below.)
                self.tasks.cancel(TaskKind::RenderWav);
                self.tasks.cancel(TaskKind::Split);
                self.split_flow = None;
                self.submit_waveform(None);
                // Unload, not pause: the old stream's position must not leak
                // into the fresh cursor/readout via the end-of-playback
                // update below.
                self.audio.unload();
                self.peak_meter = PeakMeterState::default();
                self.audio_revision = None;
                self.was_playing = false;
                let song = self.editor.song().expect("just loaded");
                // A fresh song starts with every channel audible and panning reset
                // to Original (pans seeded from the song type); stale mute/pan
                // state from the previous song must not carry over.
                self.channels = ChannelPanel::for_song(song);
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

    /// Whether any rip file mutation is in flight (a reorder/undo/redo sequence,
    /// or a quick-edit rewrite/rename), so a new one is deferred rather than
    /// interleaved with it.
    fn rip_busy(&self) -> bool {
        self.rip_run.is_some()
            || self.pending_rip_undo.is_some()
            || self.pending_rewrite.is_some()
            || self.rip_service.is_busy()
    }

    /// Starts running `transaction` -- its `forward` mutations, or (for `Undo`)
    /// its `inverse` -- one at a time through the file service.
    fn start_rip_run(&mut self, transaction: RipTransaction, kind: RipRunKind) {
        self.stop_preview();
        let mutations = if kind == RipRunKind::Undo {
            transaction.inverse.clone()
        } else {
            transaction.forward.clone()
        };
        self.rip_run = Some(RipRun {
            queue: mutations.into(),
            transaction,
            kind,
            rename_in_flight: false,
        });
        self.advance_rip_run();
    }

    /// Runs the next mutation of the in-flight sequence, or -- once the queue
    /// drains -- lands its transaction on the right stack and rescans the folder.
    fn advance_rip_run(&mut self) {
        let next = match self.rip_run.as_mut() {
            Some(run) => run.queue.pop_front(),
            None => return,
        };
        match next {
            Some(RipMutation::Rename { from, to }) => {
                if let Some(run) = self.rip_run.as_mut() {
                    run.rename_in_flight = true;
                }
                self.files.rename(from, to);
            }
            Some(RipMutation::Write { path, bytes }) => {
                self.pending_saves.push_back(SavePurpose::RipOp);
                self.files.save(SaveRequest::InPlace { path, bytes });
            }
            None => {
                let Some(run) = self.rip_run.take() else {
                    return;
                };
                let RipRun {
                    transaction, kind, ..
                } = run;
                let label = transaction.label.clone();
                match kind {
                    RipRunKind::NewEdit => {
                        self.rip_undo.push(transaction);
                        self.rip_redo.clear();
                    }
                    RipRunKind::Redo => self.rip_undo.push(transaction),
                    RipRunKind::Undo => self.rip_redo.push(transaction),
                }
                self.rescan_rip_folder();
                self.status = match kind {
                    RipRunKind::Undo => format!("Undone: {label}."),
                    RipRunKind::Redo => format!("Redone: {label}."),
                    RipRunKind::NewEdit => format!("{label}."),
                };
            }
        }
    }

    /// Aborts the in-flight sequence after a failed rename/write, resyncing the
    /// folder to whatever actually landed. The transaction is discarded (not
    /// stacked), since it did not fully apply.
    fn abort_rip_run(&mut self, message: String) {
        self.rip_run = None;
        self.alerts
            .push_back(Alert::new("Track operation failed", message));
        self.rescan_rip_folder();
    }

    /// Drops the rip undo/redo history and any in-flight sequence -- for opening
    /// a new project or closing the current one. (A same-folder rescan keeps it.)
    fn clear_rip_edits(&mut self) {
        self.rip_run = None;
        self.rip_undo.clear();
        self.rip_redo.clear();
        self.pending_rip_undo = None;
    }

    /// Moves the track at `index` by `delta` (`-1` up, `+1` down), renumbering the
    /// affected files. Ignored while another sequence runs or the move is a no-op.
    fn move_rip_track(&mut self, index: usize, delta: isize) {
        if self.rip_busy() {
            return;
        }
        let Some(to) = index.checked_add_signed(delta) else {
            return;
        };
        let transaction = self
            .rip
            .as_ref()
            .and_then(|rip| rip.reorder_transaction(index, to));
        if let Some(transaction) = transaction {
            self.start_rip_run(transaction, RipRunKind::NewEdit);
        }
    }

    /// Undo the most recent rip edit, running its inverse. Ignored while busy.
    fn undo_rip_edit(&mut self) {
        if self.rip_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        if let Some(transaction) = self.rip_undo.pop() {
            self.start_rip_run(transaction, RipRunKind::Undo);
        } else {
            self.status = "Nothing to undo.".to_owned();
        }
    }

    /// Redo the most recently undone rip edit, re-running its forward. Ignored
    /// while busy.
    fn redo_rip_edit(&mut self) {
        if self.rip_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        if let Some(transaction) = self.rip_redo.pop() {
            self.start_rip_run(transaction, RipRunKind::Redo);
        } else {
            self.status = "Nothing to redo.".to_owned();
        }
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
            // Keep a running preview alive across an in-place rescan (e.g. after
            // a screenshot optimise redelivers the folder): refresh_files
            // re-matches it by name. Only stop the audio if that track vanished.
            let preview_lost = if let Some(rip) = self.rip.as_mut() {
                let had_preview = rip.preview.is_some();
                rip.refresh_files(folder);
                had_preview && rip.preview.is_none()
            } else {
                false
            };
            if preview_lost {
                self.audio.pause();
                self.audio.rewind();
                self.audio_revision = None;
            }
            // A rescan can reorder or drop tracks; the quick-edit dialog is bound
            // to one track, so close it rather than let it act on a stale list (H1).
            self.close_rip_dialogs();
            return;
        }
        self.stop_preview();
        // A brand-new project starts with an empty edit history.
        self.clear_rip_edits();
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
        // Leaving the editor: its audio must not keep playing under the rip view
        // (mirrors open_folder's rule). Resetting the revision makes the next
        // editor Play reload cleanly.
        if self.active_tab == AppTab::Editor {
            self.audio.unload();
            self.audio_revision = None;
        }
        self.active_tab = tab;
        if tab == AppTab::Rip {
            // Song-bound modeless dialogs (Find Register, DRO Info, GD3, VGM
            // metadata) and Goto don't belong on the rip tab -- mirror the menu
            // gating that disables them there.
            self.close_song_dialogs();
            self.dialogs.goto = None;
            // Returning to the rip tab re-scans the folder so edits made in the
            // editor (or renames) are reflected.
            if let Some(path) = self.rip.as_ref().and_then(|rip| rip.folder_path.clone()) {
                self.files.open_folder_path(path);
            }
        }
    }

    fn close_rip(&mut self) {
        self.stop_preview();
        self.close_rip_dialogs();
        self.clear_rip_edits();
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
        // Fresh batch: forget any failure from a previous save-docs run.
        self.rip_docs_failed = false;
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
        // Preview with the track's own default panning and no channel mutes: the
        // editor's channel panel is for a different song, and its stored
        // panning/muting would otherwise leak into the preview (e.g. a dual-OPL2
        // editor song's fixed hard-L/R image applied to a mono track plays it
        // hard left).
        let preview_panning = ChannelPanel::for_song(&song).panning();
        // `load` below tears down the editor's stream the instant it runs --
        // success or not -- so the editor's audio snapshot is gone regardless.
        // Invalidate the revision *before* the load so the editor's next Play
        // reloads its own song instead of wedging on "No song is loaded" or
        // resuming this preview. Clear any prior preview marker up front too, so
        // a failure below can't strand a stop button on the old track.
        self.audio_revision = None;
        if let Some(rip) = self.rip.as_mut() {
            rip.preview = None;
        }
        self.audio.pause();
        if let Err(message) = self.audio.load(song, &self.config.audio) {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.set_muting(Muting::all());
        self.audio.set_panning(preview_panning);
        if let Err(message) = self.audio.play() {
            // Load succeeded but playback won't start: drop the half-started
            // preview so the service isn't left holding it (and the editor's
            // next Play reloads cleanly via the reset revision above).
            self.audio.unload();
            self.alerts.push_back(Alert::error(message));
            return;
        }
        if let Some(rip) = self.rip.as_mut() {
            rip.preview = Some(index);
        }
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
        // load_file stops any preview and switches to the editor tab; the
        // discard-changes prompt (if the editor is dirty) defers both until the
        // load is confirmed.
        self.load_or_confirm(file);
    }

    fn open_track_quick_edit(&mut self, index: usize) {
        let dialog = self.rip.as_ref().and_then(|rip| {
            let track = rip.tracks.get(index)?;
            let song = track.song()?;
            let tag = song.vgm_meta().and_then(|meta| meta.tag.as_ref());
            // Every other track's name, so a rename can't collide with one.
            let siblings = rip
                .tracks
                .iter()
                .filter(|other| other.file_name != track.file_name)
                .map(|other| other.file_name.clone())
                .collect();
            Some(TrackEditDialog::new(
                index + 1,
                track.file_name.clone(),
                tag,
                siblings,
            ))
        });
        if let Some(dialog) = dialog {
            self.dialogs.track_edit = Some(dialog);
        }
    }

    /// Applies a quick edit: rewrite the track's bytes with the new tag (and, if
    /// the name changed, rename the file). The list rescans on the outcomes, and
    /// the edit's inverse is stashed so it becomes undoable once it lands.
    fn quick_edit_submitted(&mut self, original_name: String, new_name: String, tag: Gd3Tag) {
        if self.rip_busy() {
            return;
        }
        self.stop_preview();
        // Re-resolve the target by the name the dialog opened on: a rescan may
        // have reordered the list since, so the original index is unreliable.
        let Some(track) = self.rip.as_ref().and_then(|rip| {
            rip.tracks
                .iter()
                .find(|track| track.file_name == original_name)
        }) else {
            self.alerts.push_back(Alert::error(format!(
                "\"{original_name}\" is no longer in the folder; the edit was not applied."
            )));
            return;
        };
        let old_name = track.file_name.clone();
        let old_path = track.path.clone();
        // The bytes before this edit, for the undo transaction's inverse write.
        let old_bytes = track.bytes.clone();
        let new_bytes = match track.song() {
            Some(song) => crate::rip::retagged_bytes(song, &new_name, tag),
            None => return,
        };
        let new_bytes = match new_bytes {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        let Some(old_path) = old_path else {
            return;
        };
        let new_path = old_path.with_file_name(&new_name);

        // Stash the reversible transaction: its forward matches what the bespoke
        // save path does below (so redo can replay it), its inverse restores the
        // old name and bytes. Committed to the undo stack when the save lands.
        let (forward, inverse) = if new_name == old_name {
            (
                vec![RipMutation::Write {
                    path: old_path.clone(),
                    bytes: new_bytes.clone(),
                }],
                vec![RipMutation::Write {
                    path: old_path.clone(),
                    bytes: old_bytes,
                }],
            )
        } else {
            (
                vec![
                    RipMutation::Rename {
                        from: old_path.clone(),
                        to: new_name.clone(),
                    },
                    RipMutation::Write {
                        path: new_path.clone(),
                        bytes: new_bytes.clone(),
                    },
                ],
                vec![
                    RipMutation::Rename {
                        from: new_path.clone(),
                        to: old_name.clone(),
                    },
                    RipMutation::Write {
                        path: old_path.clone(),
                        bytes: old_bytes,
                    },
                ],
            )
        };
        self.pending_rip_undo = Some(RipTransaction {
            label: format!("Edit {new_name}"),
            forward,
            inverse,
        });

        if new_name == old_name {
            // No rename: rewrite the bytes in place, in the unchanged format.
            self.pending_saves.push_back(SavePurpose::TrackRewrite);
            self.files.save(SaveRequest::InPlace {
                path: old_path,
                bytes: new_bytes,
            });
        } else {
            // Rename first, then rewrite the target-format bytes to the new path
            // once the rename lands (see poll_renamed) -- so a failed rename
            // can't strand the old file holding bytes its extension no longer
            // matches.
            self.pending_rewrite = Some((new_path, new_bytes));
            self.files.rename(old_path.clone(), new_name.clone());
            // If the renamed file is the one open in the editor, drop its stale
            // path so a later Ctrl+S does not resurrect the old name.
            if self.editor.path.as_deref() == Some(old_path.as_path()) {
                self.editor.path = None;
            }
        }
        self.status = format!("Updated {new_name}.");
    }

    /// Kicks off an explicit lossless recompression of a screenshot.
    fn optimize_image(&mut self, index: usize) {
        if self.rip_busy() {
            return;
        }
        let image = self
            .rip
            .as_ref()
            .and_then(|rip| rip.images.get(index))
            .cloned();
        let Some(image) = image else {
            return;
        };
        self.status = format!("Optimising {}...", image.name);
        self.rip_service.optimize(image.name, image.bytes.to_vec());
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
        // The path and the pre-optimise bytes (for the undo transaction's inverse).
        let found = self.rip.as_ref().and_then(|rip| {
            rip.images
                .iter()
                .find(|image| image.name == optimized.name)
                .and_then(|image| image.path.clone().map(|path| (path, image.bytes.to_vec())))
        });
        let Some((path, old_bytes)) = found else {
            self.status = format!("{}: no file path to save to.", optimized.name);
            return;
        };
        self.status = format!(
            "{}: {} -> {} bytes.",
            optimized.name,
            optimized.original_len,
            optimized.bytes.len()
        );
        self.pending_rip_undo = Some(RipTransaction {
            label: format!("Optimise {}", optimized.name),
            forward: vec![RipMutation::Write {
                path: path.clone(),
                bytes: optimized.bytes.clone(),
            }],
            inverse: vec![RipMutation::Write {
                path: path.clone(),
                bytes: old_bytes,
            }],
        });
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

    /// Plays the loop join: the last `tail_length` ms of the region, looping, so
    /// the seam is heard on its own instead of after a full pass.
    ///
    /// Forces looping on -- auditioning a join with looping off would play the
    /// tail straight through and never reach the seam at all.
    fn do_play_seam(&mut self) {
        if !self.require_song() {
            return;
        }
        self.loop_enabled = true;
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        let Some(end_ms) = self
            .editor
            .song()
            .and_then(|song| song.ms_offset_at(self.editor.markers.end()))
        else {
            return;
        };
        self.audio.rewind();
        self.audio
            .seek_ms(end_ms.saturating_sub(self.config.ui.tail_length));
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    /// Moves one loop marker (whichever is `Some`) and re-arms playback.
    fn set_loop_marker(&mut self, start: Option<usize>, end: Option<usize>) {
        let len = self.editor.len();
        if len == 0 {
            return;
        }
        if let Some(index) = start {
            self.editor.markers.set_start(index, len);
        }
        if let Some(index) = end {
            self.editor.markers.set_end(index, len);
        }
        let markers = self.editor.markers;
        self.push_loop_config();
        self.status = format!(
            "Loop {} - {} ({} instructions).",
            markers.start(),
            markers.end(),
            markers.end() - markers.start()
        );
    }

    /// Writes the marked region into the song's VGM loop fields.
    fn apply_loop_to_metadata(&mut self) {
        if !self.require_song() {
            return;
        }
        let markers = self.editor.markers;
        let len = self.editor.len();
        if !self.editor.apply_loop_to_metadata() {
            self.alerts.push_back(Alert::new(
                "Not a VGM".to_owned(),
                "Only a VGM file stores loop points. Convert the song to VGM first \
                 (File > Convert > Convert to VGM)."
                    .to_owned(),
            ));
            return;
        }
        // A VGM's loop length is defined as running to the end of the file, and
        // other players restart at the end-of-data command whatever the header
        // says. An end short of the tail is honoured here and survives a save,
        // but say so plainly rather than let it be discovered later.
        self.status = if markers.end() < len {
            format!(
                "Loop saved: {} - {}. Other players loop the whole tail until it is trimmed.",
                markers.start(),
                markers.end()
            )
        } else {
            format!("Loop saved: {} - end of song.", markers.start())
        };
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
        let len = self.editor.len();
        // The Pos. column is hex, so Goto reads hex too (an optional 0x is fine),
        // and the messages echo the position in hex (parity-2).
        let trimmed = text.trim();
        let digits = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        match usize::from_str_radix(digits, 16) {
            Err(_) => self.status = format!("Invalid position for goto: {text}"),
            Ok(position) if position >= len => {
                self.status = format!("Position for goto is out of range: {position:04X}");
            }
            Ok(position) => {
                self.editor.selection.select_only(position);
                self.scroll_to = Some(position);
                self.status = format!("Gone to position: {position:04X}");
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
                self.status = format!("Occurrence of {target} found at position {index:04X}.");
            }
            None => self.status = format!("Could not find another occurrence of {target}."),
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context, mut config: AppConfig) {
        // The Settings dialog snapshots the config at open and doesn't expose the
        // boost, so a boost changed via the transport slider meanwhile would be
        // reverted on Save. Keep the live value (M4/ux-15).
        config.audio.boost = self.config.audio.boost;
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
        let waveform_changed = config.audio.frequency != self.config.audio.frequency;
        self.config = config;
        // Don't retune the position panel to the configured rate while a stream
        // is live: it reports frames at the stream's real (still-old) rate, so
        // the readout would mix a new-rate length with old-rate frames. On the
        // next reload, ensure_audio adopts the new rate from output_rate (ux-16).
        if self.audio.output_rate().is_none() {
            self.position.set_frequency(config.audio.frequency);
        }
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
        self.dialogs.render_wav = None;
        self.dialogs.split = None;
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

    /// Renders the song to a WAV in the background; the result reaches a save
    /// dialog through `poll_services`.
    ///
    /// Each option is opt-in, so with none of them this is exactly what
    /// `drotrim render` writes.
    fn render_to_wav(&mut self, use_toggles: bool, use_panning: bool, boost: f32) {
        let Some(song) = self.editor.snapshot() else {
            self.require_song();
            return;
        };
        // One render at a time: a second would finish into the same save queue,
        // and the first's dialog is already in the user's way.
        if self.tasks.is_busy_kind(TaskKind::RenderWav) {
            self.status = "Already rendering a WAV.".to_owned();
            return;
        }
        let mix = RenderMix {
            muting: if use_toggles {
                self.channels.muting()
            } else {
                Muting::all()
            },
            panning: if use_panning {
                self.channels.panning()
            } else {
                Panning::Original
            },
            boost,
        };
        self.tasks.submit(
            TaskRequest::RenderWav {
                song,
                mix,
                sample_rate: self.config.audio.frequency,
                bit_depth: self.config.audio.bit_depth,
            },
            None,
        );
        self.status = "Rendering to WAV...".to_owned();
    }

    /// Whether a split is somewhere between its dialog and its last written file.
    fn split_is_running(&self) -> bool {
        self.split_flow.is_some() || self.tasks.is_busy_kind(TaskKind::Split)
    }

    /// Asks where the split's files should go. The split itself starts once the
    /// answer arrives in `poll_services`.
    fn start_split(&mut self, format: SplitFormat, isolate_percussion: bool) {
        if !self.require_song() || self.split_is_running() {
            return;
        }
        self.split_flow = Some(SplitFlow::AwaitingFolder {
            options: SplitOptions {
                format,
                isolate_percussion,
                audio: self.config.audio,
            },
        });
        self.files.pick_output_folder();
    }

    /// Starts the split now that `dir` is known, or gives up if the picker was
    /// dismissed.
    fn split_into(&mut self, dir: Option<PathBuf>) {
        let Some(SplitFlow::AwaitingFolder { options }) = self.split_flow.clone() else {
            // A folder arrived with no split waiting for it; nothing to do.
            return;
        };
        let (Some(dir), Some(song)) = (dir, self.editor.snapshot()) else {
            self.split_flow = None;
            self.status = "Split cancelled.".to_owned();
            return;
        };
        self.tasks
            .submit(TaskRequest::Split { song, options }, None);
        self.split_flow = Some(SplitFlow::Rendering { dir });
        self.status = "Splitting channels...".to_owned();
    }

    /// Writes a finished split's files into the folder chosen for it.
    fn write_split(&mut self, outputs: Result<Vec<(String, Vec<u8>)>, String>) {
        // Only the split still being waited on: a result from one the user
        // abandoned (by loading another song) has nowhere to go.
        let Some(SplitFlow::Rendering { dir }) = self.split_flow.clone() else {
            return;
        };
        let files = match outputs {
            Ok(files) => files,
            Err(message) => {
                self.split_flow = None;
                self.status = "The split failed.".to_owned();
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        if files.is_empty() {
            self.split_flow = None;
            self.status = "No channels to split.".to_owned();
            return;
        }
        for (name, bytes) in files {
            self.pending_saves.push_back(SavePurpose::SplitFile);
            // In place, not a dialog: the user already chose the folder, and
            // there may be eighteen of these. Existing files are overwritten,
            // as `drotrim split` does.
            self.files.save(SaveRequest::InPlace {
                path: dir.join(name),
                bytes,
            });
        }
        self.split_flow = Some(SplitFlow::Writing {
            dir,
            written: 0,
            failed: false,
        });
    }

    /// Counts off one split file's save, reporting once the last one lands.
    fn split_file_saved(&mut self, ok: bool) {
        let Some(SplitFlow::Writing {
            dir,
            written,
            failed,
        }) = &mut self.split_flow
        else {
            return;
        };
        if ok {
            *written += 1;
        } else {
            *failed = true;
        }
        // The whole batch is queued at once, so the last outcome is the one with
        // no `SplitFile` left behind it -- the same rule rip mode's docs use.
        if self
            .pending_saves
            .iter()
            .any(|purpose| *purpose == SavePurpose::SplitFile)
        {
            return;
        }
        self.status = if *failed {
            "Some split files could not be written.".to_owned()
        } else {
            format!("Wrote {written} file(s) to {}.", dir.display())
        };
        self.split_flow = None;
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
        self.audio.set_panning(self.channels.panning());
        self.audio_revision = Some(self.editor.revision());
        // The device may have rejected the configured frequency; positions
        // report frames at the stream's real rate, so the panel must too.
        if let Some(rate) = self.audio.output_rate() {
            self.position.set_frequency(rate);
            if let Some(song) = self.editor.song() {
                self.position.set_length_ms(song.total_delay_ms());
            }
        }
        // Only now is the stream's real rate known, and the loop's start frame is
        // denominated in it -- so this must follow the load, not precede it.
        self.push_loop_config();
        Ok(())
    }

    /// Refreshes the waveform's loop brackets from the markers.
    ///
    /// Nothing is drawn for an untouched region with looping off -- brackets at
    /// both extremes would be noise on a song nobody has marked up. Marking, or
    /// switching looping on, brings them in.
    fn sync_loop_overlay(&mut self) {
        let markers = self.editor.markers;
        let len = self.editor.len();
        let worth_showing = self.editor.has_song() && (!markers.is_full(len) || self.loop_enabled);
        self.waveform.loop_overlay = worth_showing
            .then(|| {
                let song = self.editor.song()?;
                Some(waveform::LoopOverlay {
                    start_ms: song.ms_offset_at(markers.start())?,
                    // The end is exclusive, so its time is where the *next*
                    // instruction starts -- which for `len` is the end of the song.
                    end_ms: song
                        .ms_offset_at(markers.end())
                        .unwrap_or_else(|| song.total_delay_ms()),
                    active: self.loop_enabled,
                    unapplied: self.editor.loop_markers_are_unapplied(),
                })
            })
            .flatten();
    }

    /// Hands the audio service the region to repeat, or `None` when looping is
    /// off. Cheap and idempotent; call it after anything that moves the markers,
    /// changes the count, or reloads the stream.
    fn push_loop_config(&mut self) {
        let config = self
            .loop_enabled
            .then(|| self.editor.song())
            .flatten()
            .map(|song| {
                // The stream's real rate while one is live, else the configured one --
                // the same rule the position readout follows. `ensure_audio` re-pushes
                // once a device has negotiated its rate, so a mismatch cannot outlive
                // the next load.
                let rate = self
                    .audio
                    .output_rate()
                    .unwrap_or(self.config.audio.frequency);
                let markers = self.editor.markers;
                LoopConfig::for_song(song, markers.start(), markers.end(), self.loop_count, rate)
            });
        self.audio.set_loop(config);
    }

    fn menu_state(&self) -> MenuState {
        let on_rip_tab = self.active_tab == AppTab::Rip;
        // Undo/Redo act on whichever tab shows: the rip file-edit stacks on the
        // rip tab, the editor's song-undo stack otherwise. On the rip tab they are
        // held off while a sequence is still running.
        let (can_undo, can_redo, undo_description, redo_description) = if on_rip_tab {
            let idle = !self.rip_busy();
            (
                idle && !self.rip_undo.is_empty(),
                idle && !self.rip_redo.is_empty(),
                self.rip_undo.last().map(|txn| txn.label.clone()),
                self.rip_redo.last().map(|txn| txn.label.clone()),
            )
        } else {
            (
                self.editor.can_undo(),
                self.editor.can_redo(),
                self.editor.undo_description().map(str::to_owned),
                self.editor.redo_description().map(str::to_owned),
            )
        };
        MenuState {
            can_undo,
            can_redo,
            undo_description,
            redo_description,
            has_rip: self.rip.is_some(),
            on_rip_tab,
            focused_row: self.editor.selection.first(),
            song_type: self.editor.song().map(|song| song.file_type),
            is_dro_v2: self.editor.song().is_some_and(|song| {
                song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V2
            }),
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

    fn play_seam_label(&self) -> String {
        format!(
            "Play the loop join: the last {} of the region, repeating",
            self.play_tail_label()
                .trim_start_matches("Play last ")
                .to_owned()
        )
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
