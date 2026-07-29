//! The application, as one
//! `eframe::App` driven entirely through the platform-service traits.

use core::fmt;
use core::time::Duration;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use dro_core::config::{AppConfig, ConfigStore, SurfaceChoice, ThemeChoice};
use dro_core::song::{DRO_FILE_V2, SongFileType};
use dro_core::{FindTarget, Gd3Tag};
use dro_synth::{LoopConfig, LoopCount, Muting, Panning, RenderMix, SplitFormat, SplitOptions};
use egui::Key;

use crate::action::{Action, AppTab};
use crate::alert::{self, Alert};
use crate::dialogs::{
    BulkTagDialog, Dialogs, DroInfoDialog, FindLoopDialog, FindRegDialog, Gd3TagDialog, GotoDialog,
    HelpDialog, RenderWavDialog, ScreenshotRenameDialog, SettingsDialog, SplitDialog,
    SplitSongsDialog, TrackEditDialog, UnwalkableVgmDialog, VgmMetadataDialog,
};
use crate::editor::{Editor, LoadFailure, LoadReport};
use crate::markers::RangeMarkers;
use crate::menus::{self, MenuState};
use crate::pack::{BulkTagOverlay, PackMutation, PackState, PackTransaction};
use crate::platform::{
    AudioService, FileService, OptimizedImage, PackJobOutcome, PackService, PickedFile,
    PickedFolder, SaveOutcome, SaveRequest,
};
use crate::tasks::{TaskKind, TaskRequest, TaskResult, TaskService};
use crate::theme::{self, Palette};
use crate::widgets::peak_meter::PeakMeterState;
use crate::widgets::position_panel::PositionPanel;
use crate::widgets::waveform::WaveformState;
use crate::widgets::{
    boost_stepper, channels::ChannelPanel, chip_panels::ChipPanels, loop_stepper, peak_meter,
    table, waveform,
};

const AUTO_TRIM_TITLE: &str = "DRO auto-trimmed";
const AUTO_TRIM_TEXT: &str = "The DRO was found to contain a bogus delay as\n\
                              its first instruction. It has been automatically\n\
                              removed. (Don't forget to save!)";
const MISMATCH_TITLE: &str = "DRO timing mismatch";

/// The About box: who wrote it, and -- because this program links copyleft
/// emulator cores -- what it is licensed under and where each core came from.
///
/// The core stanza is generated from [`dro_synth::credits`] rather than typed
/// here, so a core cannot be linked in without being credited. cr-2 feeds that
/// list from the core registry, at which point a newly registered provider
/// appears in this box automatically.
fn about_text() -> String {
    format!(
        "DRO Trimmer v{}\n\
         Laurence Dougal Myers\n\
         Web: http://www.jestarjokin.net/apps/drotrimmer\n\
         Web: https://github.com/laurence-myers/dro-trimmer\n\
         E-Mail: jestarjokin@jestarjokin.net\n\
         \n\
         This program is licensed under the GNU General Public License,\n\
         version 2 or (at your option) any later version -- it links\n\
         emulator cores under the GPL and LGPL. Complete corresponding\n\
         source code: https://github.com/laurence-myers/dro-trimmer\n\
         \n\
         The file model and playback engine (dro-core, dro-synth) are\n\
         separately available under MIT OR Apache-2.0; see licenses/\n\
         in the source distribution.\n\
         \n\
         Emulator cores in this build:\n\
         {}\n\
         RetroWave OPL3 output links the serialport crate, used under\n\
         the MPL-2.0. Its source: https://github.com/serialport/serialport-rs",
        env!("CARGO_PKG_VERSION"),
        dro_synth::credits_text(),
    )
}

/// The DRO timing mismatch box, version-specific advice
/// and all. The v2 advice points at the Settings dialog instead of a hand
/// edit of drotrim.ini, since the app has one.
/// What a click on the waveform means, given the button and whether Shift was
/// held. `None` for a gesture that does nothing.
///
/// Shift brackets the loop -- left marks the start, right the end -- so the two
/// markers are one gesture apart rather than one being a modifier deeper than
/// the other. The end is the *time* clicked, hence that instruction's index
/// taken exclusively: everything sounding before the click is inside the loop.
/// What the crop and cut items say when there is no region to act on. Their menu
/// items are disabled in that case, so this is the belt to that braces.
const NOTHING_MARKED: &str = "Mark a region first -- the loop markers cover the whole song.";

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
    /// A pack project's description or playlist.
    PackDoc,
    /// A track rewritten in place by the quick-edit dialog.
    TrackRewrite,
    /// A screenshot copied into the pack folder. Unlike the rewrites, this
    /// A screenshot copied into the pack folder. Unlike a replace, there is no
    /// previous file whose bytes could serve as an inverse, so it rescans
    /// without touching the undo stack.
    ScreenshotAdded,
    /// A screenshot rewritten in place -- a recompress, or a replace. Both hold
    /// the old bytes, so both land a reversible transaction.
    ImageWritten,
    /// The exported release zip (a Save-As dialog).
    ExportZip,
    /// A `Write` step of the pack file-op executor (reorder / undo / redo).
    PackOp,
}

/// The stages shared by File > Split Channels and File > Split Songs: choose a
/// folder, render into it, write the files out. Both go through the one output
/// folder picker, so at most one runs at a time.
#[derive(Debug, Clone)]
enum SplitFlow {
    /// The options are chosen; the folder picker is up.
    AwaitingFolder(PendingSplit),
    /// The split is rendering, bound for `dir`. `songs` distinguishes the two
    /// kinds once the folder is chosen, for the completion offer.
    Rendering { dir: PathBuf, songs: bool },
    /// Writing the outputs, counting them off as their saves land.
    Writing {
        dir: PathBuf,
        written: usize,
        failed: bool,
        songs: bool,
    },
}

/// Which split the folder picker is being asked about: one file per channel, or
/// one file per song in a capture.
#[derive(Debug, Clone)]
enum PendingSplit {
    Channels {
        options: SplitOptions,
    },
    Songs {
        threshold_native: u32,
        included: Vec<bool>,
        trailing_tail: u32,
    },
}

impl PendingSplit {
    /// Whether this is a Split Songs request (drives the completion offer).
    fn is_songs(&self) -> bool {
        matches!(self, Self::Songs { .. })
    }
}

/// Whether the running file-op sequence is a fresh edit, a redo, or an undo --
/// deciding which stack its transaction lands on when the sequence completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackRunKind {
    /// A brand-new edit (reorder): push to undo, clear the redo stack.
    NewEdit,
    /// Re-applying a previously undone edit: push back to undo.
    Redo,
    /// Reverting an edit: push to redo.
    Undo,
}

/// A path's file name, for a status line or an undo label.
fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A screenshot named and ready to go into the pack folder: where it lands, and
/// the bytes as picked (the fallback if the recompression fails or gains
/// nothing).
#[derive(Debug, Clone)]
struct PendingAdd {
    path: PathBuf,
    bytes: Vec<u8>,
}

/// What the screenshot picker's result will be used for. The pick is async, so
/// the intent has to outlive the click that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ScreenshotPick {
    /// Copy in as `<Game Name>.png` (the empty state's Add).
    Add,
    /// Overwrite this file, keeping its name (the inspector's Replace).
    Replace(PathBuf),
}

/// A pack file-op sequence in flight: the mutations still to run, the transaction
/// they belong to, and where it lands on completion. Runs one mutation at a time,
/// advancing as each rename/write/delete outcome arrives.
struct PackRun {
    queue: VecDeque<PackMutation>,
    transaction: PackTransaction,
    kind: PackRunKind,
    /// Set while a `Rename` mutation is awaiting its `poll_renamed`, so that
    /// outcome advances the run rather than the quick-edit rename path.
    rename_in_flight: bool,
}

/// What a background volume scan's [`Peak`](dro_synth::Peak) is for, so the app
/// routes the result to the control that asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumeScanPurpose {
    /// The transport "Match" button: set the playback volume lever.
    MatchBoost,
    /// The VGM dialog "Measure" button: fill the volume-modifier field.
    FillModifier,
}

pub struct DroApp {
    editor: Editor,
    files: Box<dyn FileService>,
    audio: Box<dyn AudioService>,
    tasks: Box<dyn TaskService>,
    pack_service: Box<dyn PackService>,
    config_store: Box<dyn ConfigStore>,
    config: AppConfig,

    status: String,
    alerts: VecDeque<Alert>,
    dialogs: Dialogs,

    /// The open pack project, if any.
    pack: Option<PackState>,
    /// The visible tab. Forced to `Editor` whenever no pack is open.
    active_tab: AppTab,
    /// One entry per outstanding `files.save`, in order, to route its outcome.
    pending_saves: VecDeque<SavePurpose>,
    /// What the in-flight screenshot pick, if any, will do when it lands.
    pending_screenshot: Option<ScreenshotPick>,
    /// A named screenshot waiting for its recompression to come back, so it can
    /// be written already-optimal instead of written and then rewritten.
    pending_add: Option<PendingAdd>,
    /// Where the last Alt+arrow reorder left the track, when the edit it started
    /// is still the top of the undo stack. A move that begins there continues
    /// the same run and folds into it; anything else starts a new one.
    coalesce_next_reorder: Option<usize>,
    /// Whether the pack edit currently running folds into the one below it on
    /// the undo stack. Set as the run starts, spent as it lands.
    pack_run_coalesces: bool,
    /// How far along File > Split Channels is, if it is running at all. Doubles
    /// as the in-flight guard and as the gate that drops a result belonging to a
    /// split the user has since abandoned.
    split_flow: Option<SplitFlow>,

    waveform: WaveformState,
    /// The stereo output peak meter beside the waveform.
    peak_meter: PeakMeterState,
    /// The volume factor at which the limiter began clipping this song, or `None`
    /// while it has not. The volume lever cannot rise above it (the clipping
    /// guard); it clears when a new song loads. Derived each frame in
    /// [`Self::playback_tick`] from the audio backend's sticky engaged flag.
    boost_ceiling: Option<f32>,
    /// What the in-flight (or most recent) volume scan is for, so its `Peak`
    /// result reaches the right place -- the volume lever or the VGM dialog. Both
    /// use one [`TaskKind::VolumeScan`], and submitting cancels the other, so a
    /// single value tracks the live purpose.
    volume_scan_purpose: VolumeScanPurpose,
    /// Whether the transport's volume field held keyboard focus as of the last
    /// frame, reported by the lever via [`Action::VolumeFieldFocused`]. While it
    /// does, [`Self::gather_key_input`] stands the editor shortcuts down so typed
    /// numbers edit the value instead of toggling channels.
    volume_field_editing: bool,
    position: PositionPanel,
    channels: ChipPanels,

    /// A row the table should scroll into view next frame.
    scroll_to: Option<table::ScrollTo>,
    /// The last first-selected row, to detect selection changes.
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
    /// so the pack's dirty flag is kept rather than cleared once the batch ends.
    pack_docs_failed: bool,
    /// The pack file-op sequence currently executing (reorder / undo / redo), if
    /// any. Only one runs at a time; edits are ignored while it is `Some`.
    pack_run: Option<PackRun>,
    /// Applied pack edits available to undo, oldest first.
    pack_undo: Vec<PackTransaction>,
    /// Undone pack edits available to redo.
    pack_redo: Vec<PackTransaction>,
    /// A quick-edit / optimise transaction whose forward ran through the bespoke
    /// save path; committed to the undo stack once that save succeeds (and
    /// dropped if it fails), so undo only ever reverses edits that landed.
    pending_pack_undo: Option<PackTransaction>,
    /// A skin the Settings dialog is showing but has not saved, as
    /// `(theme, pad_style, deck_style)`. `None` whenever the window is painted
    /// in the saved settings. See [`Self::preview_skin`].
    skin_preview: Option<(ThemeChoice, SurfaceChoice, SurfaceChoice)>,
}

impl DroApp {
    #[must_use]
    pub fn new(
        files: Box<dyn FileService>,
        audio: Box<dyn AudioService>,
        tasks: Box<dyn TaskService>,
        pack_service: Box<dyn PackService>,
        config_store: Box<dyn ConfigStore>,
        initial_file: Option<PickedFile>,
    ) -> Self {
        let config = config_store.load();
        // The registry-side copy of `audio.core.<slug>`: every engine built
        // from here on (playback, WAV render, waveform, peak scan) reads it
        // through `core_for`, so the cores the user chose are the cores that
        // actually play.
        dro_synth::registry::set_core_choices(config.audio.cores.clone());
        let initial_frequency = config.audio.frequency;
        Self {
            editor: Editor::new(),
            pending_screenshot: None,
            pending_add: None,
            coalesce_next_reorder: None,
            pack_run_coalesces: false,
            files,
            audio,
            tasks,
            pack_service,
            config_store,
            config,
            status: String::new(),
            alerts: VecDeque::new(),
            dialogs: Dialogs::default(),
            pack: None,
            active_tab: AppTab::Editor,
            pending_saves: VecDeque::new(),
            split_flow: None,
            waveform: WaveformState::default(),
            peak_meter: PeakMeterState::default(),
            boost_ceiling: None,
            volume_scan_purpose: VolumeScanPurpose::MatchBoost,
            volume_field_editing: false,
            position: PositionPanel::new(initial_frequency),
            channels: ChipPanels::new(),
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
            pack_docs_failed: false,
            pack_run: None,
            pack_undo: Vec::new(),
            pack_redo: Vec::new(),
            pending_pack_undo: None,
            skin_preview: None,
        }
    }

    /// The skin on screen: the Settings dialog's live preview if one is up,
    /// else the saved settings.
    fn shown_skin(&self) -> (ThemeChoice, SurfaceChoice, SurfaceChoice) {
        self.skin_preview.unwrap_or((
            self.config.ui.theme,
            self.config.ui.pad_style,
            self.config.ui.deck_style,
        ))
    }

    /// The active colour scheme, with the configured pad/deck overrides applied.
    /// Owned rather than borrowed: the overrides make it a per-config value, not
    /// one of the twelve static case palettes.
    fn palette(&self) -> Palette {
        let (theme, pad, deck) = self.shown_skin();
        theme::palette_with(theme, pad, deck)
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

        let active_palette = self.palette();
        let p = &active_palette;
        // Chrome panels are fascia plates: a transparent frame, with the plate
        // gradient painted behind the content inside each panel (see the
        // `theme::plate` calls below). The waveform is a data well, so its
        // margins take the main dark background rather than the chrome tint.
        let chrome = egui::Frame::side_top_panel(ui.style()).fill(egui::Color32::TRANSPARENT);
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
                theme::plate_panel(ui, p, |ui| {
                    menus::bar(ui, p, &self.menu_state(), &mut actions);
                });
            });
        // The tab strip switches the editor and pack views. It is always present,
        // so the app keeps one shape; Pack is simply greyed until a pack project
        // is open, which says the view exists rather than hiding it.
        let tabs = egui::Panel::top("tab-strip")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::plate_panel(ui, p, |ui| {
                    // The views, in strip order, with the labels naming them.
                    const VIEWS: [AppTab; 2] = [AppTab::Editor, AppTab::Pack];
                    let strip = [
                        theme::tabs::Tab::new("Editor"),
                        theme::tabs::Tab::new("Pack").enabled(self.pack.is_some()),
                    ];
                    let selected = VIEWS.iter().position(|t| *t == self.active_tab);
                    if let Some(i) = theme::tabs::strip(ui, p, &strip, selected.unwrap_or(0)) {
                        actions.push(Action::SelectTab(VIEWS[i]));
                    }
                });
            });
        // The editor-only panels (waveform, transport/boost, position) are hidden
        // on the pack tab, which owns the whole central area.
        let editor_tab = self.active_tab == AppTab::Editor;
        // A VGM for other chips has no OPL stream, so the panels that exist to show or
        // drive audio have nothing to say about it. They go, rather than sit
        // there as a dead transport over a permanently flat waveform.
        let playable = self.editor.capabilities().playable;
        let audio_panels = editor_tab && playable;
        let waveform = audio_panels.then(|| {
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
                        // Hardware output sends no samples through this program,
                        // so there is nothing to meter -- and a meter pinned at
                        // silence through a whole song reads as a fault. Drop it
                        // and give the waveform the room.
                        let metered = self.output_renders_samples();
                        // Reserve the peak meter's width up front: the waveform
                        // fills whatever space it is given.
                        let wave_width = if metered {
                            ui.available_width() - peak_meter::WIDTH - ui.spacing().item_spacing.x
                        } else {
                            ui.available_width()
                        };
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
                        if metered {
                            peak_meter::show(ui, &self.peak_meter, p);
                        }
                    });
                })
        });
        let status = egui::Panel::bottom("status-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::plate_panel(ui, p, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&self.status);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.pack_service.is_busy() {
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
            });
        let position = audio_panels.then(|| {
            egui::Panel::bottom("position-panel")
                .frame(chrome)
                .show_separator_line(false)
                .show(ui, |ui| {
                    theme::plate_panel(ui, p, |ui| {
                        self.position.show(ui, p);
                    });
                })
        });
        let controls = editor_tab.then(|| {
            // The controls own their vertical spacing (equal padding above and
            // below each row band), so drop the frame's vertical margin/spacing.
            let controls_frame = egui::Frame::side_top_panel(ui.style())
                .fill(egui::Color32::TRANSPARENT)
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
                    theme::deck_panel(ui, p, |ui| {
                        const PAD: f32 = 6.0;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(PAD);
                        ui.horizontal(|ui| {
                            ui.set_min_height(ui.spacing().interact_size.y);
                            ui.spacing_mut().item_spacing.x = 12.0;
                            if theme::bevel::icon_button(ui, p, theme::icon::Icon::Del, "Del.")
                                .on_hover_text("Delete the selected instruction(s)")
                                .clicked()
                            {
                                actions.push(Action::DeleteSelection);
                            }
                            // Delete applies to any document; everything
                            // after it drives playback, which needs a stream
                            // this app can render.
                            if playable {
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Play, "Play")
                                    .on_hover_text("Play the song from the current position")
                                    .clicked()
                                {
                                    actions.push(Action::Play);
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Stop, "Stop")
                                    .on_hover_text("Stop playback")
                                    .clicked()
                                {
                                    actions.push(Action::Stop);
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Tail, "Tail")
                                    .on_hover_text(self.play_tail_label())
                                    .clicked()
                                {
                                    actions.push(Action::PlayTail);
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Seam, "Seam")
                                    .on_hover_text(self.play_seam_label())
                                    .clicked()
                                {
                                    actions.push(Action::PlaySeam);
                                }
                                let mut looping = self.loop_enabled;
                                if theme::bevel::icon_toggle(
                                    ui,
                                    p,
                                    &mut looping,
                                    theme::icon::Icon::Loop,
                                    "Loop",
                                )
                                .on_hover_text(
                                    "Repeat the marked region. Shift+click the waveform to mark \
                                 the start and Shift+right-click the end; [ and ] use the \
                                 selected row.",
                                )
                                .clicked()
                                {
                                    actions.push(Action::ToggleLoopPlayback);
                                }
                                loop_stepper::loop_count_stepper(
                                    ui,
                                    p,
                                    self.loop_count,
                                    &mut actions,
                                );
                                // The boost is applied to rendered samples, of which
                                // hardware output produces none -- the board has its
                                // own volume.
                                let shapes_output = self.output_renders_samples();
                                ui.add_enabled_ui(shapes_output, |ui| {
                                    boost_stepper::boost_stepper(
                                        ui,
                                        p,
                                        self.config.audio.boost,
                                        self.boost_ceiling,
                                        self.config.audio.lock_boost,
                                        self.tasks.is_busy_kind(TaskKind::VolumeScan),
                                        &mut actions,
                                    );
                                });
                            }
                        });
                        ui.add_space(PAD);
                        theme::separator_full(ui, p);
                        ui.add_space(PAD);
                        // The panel hides its own high bank for a plain OPL2 song.
                        let channels = self.channels.show(ui, p, self.output_renders_samples());
                        if channels.muting_changed {
                            actions.push(Action::MutingChanged);
                        }
                        if channels.panning_changed {
                            actions.push(Action::PanningChanged);
                        }
                        ui.add_space(PAD);
                    });
                })
        });
        // The pack view's output deck: the readiness lamp and everything that
        // turns the folder into a submission, pinned to the foot of the window
        // so they stay reachable however far the form and track list scroll.
        // The editor's transport deck occupies the same slot on the other tab.
        let pack_deck = (!editor_tab && self.pack.is_some()).then(|| {
            let deck_frame = egui::Frame::side_top_panel(ui.style())
                .fill(egui::Color32::TRANSPARENT)
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 0,
                });
            egui::Panel::bottom("pack-deck")
                .frame(deck_frame)
                .show_separator_line(false)
                .show(ui, |ui| {
                    theme::deck_panel(ui, p, |ui| {
                        const PAD: f32 = 6.0;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(PAD);
                        if let Some(pack) = self.pack.as_mut() {
                            crate::pack::deck(ui, pack, p, &mut actions);
                        }
                        ui.add_space(PAD);
                    });
                })
        });
        // The editor's central panel is one big data well; the pack view sits on
        // the FT2 desktop tint, with its own sunken wells inside.
        let central_fill = if editor_tab { p.data_bg } else { p.desktop };
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(central_fill))
            .show(ui, |ui| match self.active_tab {
                AppTab::Editor => {
                    if self.editor.has_document() {
                        // Row hover reads `widgets.hovered.bg_fill`, which is the
                        // bright face colour; scope it to the data-well tone so it
                        // does not flash teal under the yellow text.
                        ui.visuals_mut().widgets.hovered.bg_fill = p.data_hover;
                        table::show(ui, &mut self.editor, self.scroll_to.take(), p);
                    } else {
                        ui.visuals_mut().override_text_color = Some(p.data_label);
                        ui.centered_and_justified(|ui| {
                            ui.label(
                                "Open a DRO, VGM or VGZ file (File > Open Song..., or drop it \
                                 here).",
                            );
                        });
                    }
                }
                AppTab::Pack => {
                    let scanning = self.tasks.is_busy_kind(TaskKind::PackVolumeScan);
                    // Whichever the user last touched is in charge: moving the
                    // pointer hands the row back to hover, so the keyboard's lit
                    // row does not linger under a mouse that has moved on.
                    // The event, not `pointer.is_moving()`: that reads a velocity
                    // averaged over a few frames, which a single deliberate move
                    // can leave at zero.
                    let pointer_moved = ui.input(|input| {
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::PointerMoved(_)))
                    });
                    if let Some(pack) = self.pack.as_mut() {
                        if pointer_moved {
                            pack.focused_track = None;
                        }
                        crate::pack::show(ui, pack, p, scanning, &mut actions);
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
        let mut seams = vec![menu.response.rect.bottom(), tabs.response.rect.bottom()];
        if let Some(waveform) = &waveform {
            seams.push(waveform.response.rect.bottom());
        }
        if let Some(controls) = &controls {
            seams.push(controls.response.rect.top());
        }
        if let Some(position) = &position {
            seams.push(position.response.rect.top());
        }
        if let Some(pack_deck) = &pack_deck {
            seams.push(pack_deck.response.rect.top());
        }
        seams.push(status.response.rect.top());
        for seam in seams {
            theme::bevel::groove_h(&divider, x_range, seam - 1.0, p);
        }

        // Keep the modeless dialogs off the menu bar and tab strip: since
        // egui 0.35 the panels above no longer reserve context space, so an
        // unconstrained window auto-places at the top of the viewport.
        let chrome_bottom = tabs.response.rect.bottom();
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
        if let Some(result) = self.files.poll_picked_image() {
            match result {
                Ok(file) => self.add_screenshot(file),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new("Failed to read image", message)),
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
            let is_pack_op = self
                .pack_run
                .as_ref()
                .is_some_and(|run| run.rename_in_flight);
            match result {
                Ok(()) if is_pack_op => {
                    if let Some(run) = self.pack_run.as_mut() {
                        run.rename_in_flight = false;
                    }
                    self.advance_pack_run();
                }
                Ok(()) => {
                    // A quick-edit rename paired with a byte rewrite: now that the
                    // file has its new name, write the target-format bytes to it
                    // (its own TrackRewrite outcome then rescans the folder).
                    if let Some((path, bytes)) = self.pending_rewrite.take() {
                        self.pending_saves.push_back(SavePurpose::TrackRewrite);
                        self.files.save(SaveRequest::InPlace { path, bytes });
                    } else {
                        self.rescan_pack_folder();
                        self.status = "Renamed track; pack folder rescanned.".to_owned();
                    }
                }
                Err(message) if is_pack_op => self.abort_pack_run(message),
                Err(message) => {
                    self.pending_rewrite = None;
                    self.alerts.push_back(Alert::new("Rename failed", message));
                }
            }
        }
        if let Some(result) = self.files.poll_deleted() {
            // Deletes are only ever issued by the pack file-op executor, so an
            // outcome always belongs to the run in flight.
            match result {
                Ok(()) => self.advance_pack_run(),
                Err(message) => self.abort_pack_run(message),
            }
        }
        if let Some(chosen) = self.files.poll_output_folder() {
            self.split_into(chosen);
        }
        if let Some(outcome) = self.files.poll_saved() {
            // Outcomes arrive in the order the saves were made, so a FIFO of
            // purposes routes each one to the editor or the pack project.
            let purpose = self.pending_saves.pop_front().unwrap_or(SavePurpose::Song);
            self.handle_save_outcome(purpose, outcome);
        }
        if let Some(outcome) = self.pack_service.poll() {
            match outcome {
                PackJobOutcome::Done {
                    zip_name,
                    bytes,
                    log,
                } => {
                    self.pending_saves.push_back(SavePurpose::ExportZip);
                    self.files.save(SaveRequest::Dialog {
                        suggested_name: zip_name,
                        bytes,
                    });
                    // The zip exists in memory, not on disk: the picker is still
                    // to come, and saying "built" without saying "choose where"
                    // reads as finished (which is what a cancel then contradicts).
                    self.status = if log.is_empty() {
                        "Built the pack zip -- choose where to save it.".to_owned()
                    } else {
                        format!(
                            "Built the pack zip. {} Choose where to save it.",
                            log.join(" ")
                        )
                    };
                }
                PackJobOutcome::Failed(message) => {
                    // Replace the stale "Building pack zip..." status (ux-11).
                    self.status = "Pack export failed.".to_owned();
                    self.alerts
                        .push_back(Alert::new("Pack export failed", message));
                }
            }
        }
        if let Some(result) = self.pack_service.poll_optimized() {
            // An add's recompression is a step on the way in, not an edit of a
            // file in the folder: it writes the file rather than rewriting it,
            // and a failed pass just means the picked bytes go in as they are.
            if let Some(add) = self.pending_add.take() {
                let smaller = result
                    .ok()
                    .filter(|optimized| optimized.bytes.len() < add.bytes.len())
                    .map(|optimized| optimized.bytes);
                self.write_added_screenshot(match smaller {
                    Some(bytes) => PendingAdd { bytes, ..add },
                    None => add,
                });
            } else {
                match result {
                    Ok(optimized) => self.image_optimized(optimized),
                    Err(message) => {
                        self.status = "Screenshot optimise failed.".to_owned();
                        self.alerts
                            .push_back(Alert::new("Optimise failed", message));
                    }
                }
            }
        }
        for result in self.tasks.poll() {
            match result {
                TaskResult::Waveform(buckets) => self.waveform.buckets = buckets,
                TaskResult::Wav(rendered) => self.handle_wav_result(rendered),
                TaskResult::Split(outputs) | TaskResult::SplitSongs(outputs) => {
                    self.write_split(outputs);
                }
                TaskResult::Peak(peak) => self.handle_volume_scan(peak),
                TaskResult::PackPeaks(peaks) => self.handle_pack_peaks(peaks),
                TaskResult::LoopCandidates(candidates) => self.handle_loop_candidates(candidates),
            }
        }
        // Keep the Find Loop dialog's progress state in step with the task, so its
        // spinner shows while a search runs and clears the moment it finishes.
        let searching = self.tasks.is_busy_kind(TaskKind::LoopSearch);
        if let Some(dialog) = self.dialogs.find_loop.as_mut() {
            dialog.set_busy(searching);
        }
    }

    /// Offers a finished render to the save dialog, or reports why there is
    /// nothing to offer.
    ///
    /// The picker blocks the UI thread, but only once the long part is done --
    /// the same shape as the pack zip export.
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

    /// Changes the live playback volume, updating the config, the audio engine and
    /// (when `persist`) `drotrim.ini`. The shared path behind the volume lever and
    /// the "Match Volume" scan.
    fn set_boost(&mut self, value: f32, persist: bool) {
        self.config.audio.boost = value;
        // A loaded stream gets the boost live via the command queue; an unloaded
        // one picks it up from `config.audio` on the next load, so this
        // deliberately does not force an audio reload.
        self.audio.set_boost(value);
        // Only write to drotrim.ini when the volume is locked: an unlocked boost
        // is per-song (re-derived from the modifier on the next open), so
        // persisting it would resurrect a stale value on the next launch.
        if persist
            && self.config.audio.lock_boost
            && let Err(error) = self.config_store.save(&self.config)
        {
            self.alerts
                .push_back(Alert::error(format!("Could not save settings: {error}")));
        }
    }

    /// The playback volume `song`'s header volume modifier asks for: unity for a
    /// DRO (no modifier) or a VGM whose modifier is `0`. What an unlocked song
    /// starts at, in the editor and in a pack preview.
    fn modifier_boost(song: &dro_core::Song) -> f32 {
        song.vgm_meta().map_or(1.0, |meta| {
            dro_core::volume_modifier_factor(meta.volume_modifier)
        })
    }

    /// The volume a freshly opened *editor* song should start at when the volume
    /// is not locked.
    fn song_modifier_boost(&self) -> f32 {
        self.editor.song().map_or(1.0, Self::modifier_boost)
    }

    /// Applies the "Lock" toggle. Locking remembers the current volume across
    /// songs (and persists it); unlocking hands control back to each song's
    /// header modifier, snapping the current song to its modifier now so the
    /// lever reflects the change immediately.
    fn set_lock_boost(&mut self, lock: bool) {
        self.config.audio.lock_boost = lock;
        if !lock {
            let boost = self.song_modifier_boost();
            self.config.audio.boost = boost;
            self.audio.set_boost(boost);
        }
        if let Err(error) = self.config_store.save(&self.config) {
            self.alerts
                .push_back(Alert::error(format!("Could not save settings: {error}")));
        }
    }

    /// Kicks off a background peak scan of the current song for the volume lever's
    /// "Match" button; the finished scan reaches [`Self::handle_volume_scan`]
    /// through `poll_services`. Cancels any scan already running (same
    /// [`TaskKind`]), so mashing the button just re-measures.
    fn match_volume(&mut self) {
        self.submit_volume_scan(VolumeScanPurpose::MatchBoost, "Measuring volume...");
    }

    /// Kicks off a background peak scan for the VGM dialog's "Measure" button; the
    /// finished scan fills the volume-modifier field via [`Self::handle_volume_scan`].
    fn measure_volume_modifier(&mut self) {
        self.submit_volume_scan(VolumeScanPurpose::FillModifier, "Measuring peak...");
    }

    /// Submits a volume scan of the current song for `purpose`, or asks for a song
    /// if none is loaded. Shared by the "Match" and "Measure" buttons; the purpose
    /// is remembered so [`Self::handle_volume_scan`] routes the result.
    fn submit_volume_scan(&mut self, purpose: VolumeScanPurpose, status: &str) {
        let Some(song) = self.editor.snapshot() else {
            self.require_song();
            return;
        };
        self.volume_scan_purpose = purpose;
        self.tasks.submit(
            TaskRequest::VolumeScan {
                song,
                sample_rate: self.config.audio.frequency,
            },
            None,
        );
        self.status = status.to_owned();
    }

    /// Applies a finished volume scan to whatever asked for it: the playback
    /// volume lever (the "Match" button) or the VGM dialog's volume-modifier field
    /// (the "Measure" button).
    fn handle_volume_scan(&mut self, peak: dro_synth::Peak) {
        match self.volume_scan_purpose {
            VolumeScanPurpose::MatchBoost => {
                if peak.max_level == 0 {
                    self.status = "The song is silent; volume left unchanged.".to_owned();
                    return;
                }
                // The modifier-ladder volume that lifts the peak to full scale.
                let volume = dro_core::matched_volume(peak.max_level);
                self.set_boost(volume, true);
                let dbfs = dro_core::peak_dbfs(peak.max_level);
                self.status = format!("Peak {dbfs:.1} dBFS \u{2192} volume {volume:.2}\u{00d7}");
            }
            VolumeScanPurpose::FillModifier => {
                // The dialog may have been closed while the scan ran; if so, the
                // result is simply dropped.
                if let Some(dialog) = self.dialogs.vgm_metadata.as_mut() {
                    dialog.apply_measured_peak(peak);
                    let modifier = dro_core::suggest_volume_modifier(peak.max_level, None);
                    let dbfs = dro_core::peak_dbfs(peak.max_level);
                    self.status =
                        format!("Peak {dbfs:.1} dBFS \u{2192} volume modifier {modifier}");
                }
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
                SavePurpose::PackDoc => {
                    // The description and playlist save back to back; report and
                    // clear the dirty flag once the last of them lands -- but only
                    // if none of the batch failed, so edits aren't lost (uishell-7).
                    let more = self
                        .pending_saves
                        .iter()
                        .any(|purpose| *purpose == SavePurpose::PackDoc);
                    if !more {
                        if self.pack_docs_failed {
                            self.status =
                                "Some package files could not be saved; changes kept.".to_owned();
                        } else {
                            if let Some(pack) = self.pack.as_mut() {
                                pack.dirty = false;
                            }
                            // Extensions only: the stem is the game's full name,
                            // and printing it twice ran the line off the status
                            // bar on any pack with a subtitle.
                            self.status = "Saved the package .txt and .m3u.".to_owned();
                        }
                    }
                }
                SavePurpose::TrackRewrite | SavePurpose::ImageWritten => {
                    // The file's bytes were rewritten; rescan so the list (or
                    // the inline screenshot and its size) reflects the change. A
                    // rename, if any, rescans on its own outcome too -- both
                    // refresh in place, harmlessly. The edit landed, so its undo
                    // transaction (stashed at submit) becomes reversible.
                    if let Some(transaction) = self.pending_pack_undo.take() {
                        self.pack_undo.push(transaction);
                        self.pack_redo.clear();
                    }
                    self.rescan_pack_folder();
                }
                SavePurpose::ScreenshotAdded => {
                    self.rescan_pack_folder();
                    self.status = format!("Added {name} to the pack folder.");
                }
                SavePurpose::PackOp => self.advance_pack_run(),
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
                SavePurpose::PackDoc => self.pack_docs_failed = true,
                SavePurpose::PackOp => self.abort_pack_run("The save was cancelled.".to_owned()),
                SavePurpose::TrackRewrite | SavePurpose::ImageWritten => {
                    self.pending_pack_undo = None;
                }
                // The build's status is still on the bar, reading as a finished
                // export -- gzipped tracks and all. Say what actually happened.
                SavePurpose::ExportZip => {
                    self.status = "Export cancelled; the zip was not saved.".to_owned();
                }
                // Split files save in place, so there is no picker to cancel --
                // but the tally still has to move on, or the batch never ends.
                SavePurpose::SplitFile => self.split_file_saved(false),
                _ => {}
            },
            SaveOutcome::Failed(message) => match purpose {
                SavePurpose::PackOp => self.abort_pack_run(message),
                SavePurpose::SplitFile => {
                    // One alert at the end for the whole batch, not eighteen.
                    log::warn!("split file could not be written: {message}");
                    self.split_file_saved(false);
                }
                other => {
                    if other == SavePurpose::PackDoc {
                        self.pack_docs_failed = true;
                    }
                    if matches!(other, SavePurpose::TrackRewrite | SavePurpose::ImageWritten) {
                        self.pending_pack_undo = None;
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
        // Only single-file drops; say so rather than silently
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
            // directory into pack mode. A junk file surfaces the usual "bad
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
        if self.editor.is_dirty() || self.pack_is_dirty() {
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
        // must undo the *text*, not the song. A blanket `egui_wants_keyboard_input`
        // gate would also swallow shortcuts whenever a chrome button merely holds
        // focus, so instead the one editor-view text input (the volume field)
        // reports its own focus (see the boost_stepper gate below).
        if !self.alerts.is_empty() || self.dialogs.any_open() {
            return;
        }
        // The pack tab hides the editor, so the editor's playback/navigation keys
        // must not fire there. Save (the package files), Undo/Redo (the file
        // edits) and Help remain.
        if self.active_tab == AppTab::Pack {
            // Alt+arrow reorders the focused track: the keyboard path to the
            // drag handle, and the only one, so it is offered wherever a pack is
            // open rather than only while the Tracks section is drawn.
            ctx.input_mut(|input| {
                for (shortcut, delta) in [
                    (menus::MOVE_TRACK_UP, -1_isize),
                    (menus::MOVE_TRACK_DOWN, 1),
                ] {
                    if input.consume_shortcut(&shortcut) {
                        actions.push(Action::PackMoveFocusedTrack { delta });
                    }
                }
                if input.consume_shortcut(&menus::SAVE) {
                    actions.push(Action::PackSaveDocs);
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
        // The transport's volume field is the editor view's one focusable text
        // input; while it holds keyboard focus it owns the keyboard, so typed
        // numbers edit the value instead of toggling channels 1-9 (and Delete /
        // arrows edit the text, not the song). Tab is intentionally left
        // unconsumed here so it can move focus out of the field as usual.
        if self.volume_field_editing {
            return;
        }
        ctx.input_mut(|input| {
            // Aside from the volume field handled just above, the editor view has
            // no focusable text, so swallow Tab/Shift+Tab: a stray Tab would
            // otherwise move focus onto a chrome button, where Space activates it
            // (e.g. "Del.") instead of toggling playback.
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
            if input.key_pressed(menus::DELETE_SELECTION.logical_key)
                || input.key_pressed(menus::DELETE_SELECTION_ALT.logical_key)
            {
                actions.push(Action::DeleteSelection);
            }
            if input.key_pressed(menus::PLAY_STOP.logical_key) {
                actions.push(Action::TogglePlayback);
            }
            if input.key_pressed(menus::PREVIOUS_DELAY.logical_key) {
                actions.push(Action::PreviousDelay);
            }
            if input.key_pressed(menus::NEXT_DELAY.logical_key) {
                actions.push(Action::NextDelay);
            }
            if input.key_pressed(menus::SELECTION_UP.logical_key) {
                actions.push(Action::SelectionMove {
                    delta: -1,
                    extend: mods.shift,
                });
            }
            if input.key_pressed(menus::SELECTION_DOWN.logical_key) {
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
                if input.key_pressed(menus::SET_LOOP_START.logical_key) {
                    actions.push(Action::SetLoopStart(row));
                }
                if input.key_pressed(menus::SET_LOOP_END.logical_key) {
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
        // An emptied selection leaves the indicator where it was.
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
        // A backend can fail away from any call we made -- a device unplugged
        // mid-song -- so its complaint has nowhere to surface but here.
        if let Some(error) = self.audio.last_error() {
            self.alerts
                .push_back(Alert::error(format!("Playback stopped: {error}")));
        }

        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        // The limiter's flag is read every tick, clip or not: it is destructive,
        // and left unread it would report a clip from a minute ago.
        let limited = self.audio.take_limited();
        self.peak_meter
            .update_with(self.audio.take_peaks(), dt, limited);
        if self.peak_meter.is_active() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        // Cap the volume where clipping starts. The backend reports the lowest
        // boost that has clipped this song (ratcheting down as quieter boosts
        // still clip), which is exactly the ceiling: the volume lever cannot rise
        // above it. A fresh (or unloaded) stream reports `None`, clearing the cap.
        self.boost_ceiling = self.audio.min_engaged_boost();

        let playing = self.audio.is_playing();
        if self.active_tab == AppTab::Editor {
            // One more update after playback ends, so the readout and cursor land
            // on the exact final position instead of freezing a buffer short of
            // it.
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
        } else if self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.preview.is_some())
        {
            // A pack preview: clear it once it finishes, and keep the frames
            // coming while it plays (the pack view has no position readout).
            if self.audio.is_finished() {
                self.stop_preview();
            } else if playing {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        self.was_playing = playing;
        if self.tasks.is_busy() || self.pack_service.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // -- actions ---------------------------------------------------------

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenFile => self.files.pick_open(),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::CloseFile => {
                if !self.require_document() {
                    return;
                }
                if self.editor.is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved changes?",
                        "The current song has unsaved changes. Close it anyway?",
                        Action::ConfirmCloseFile,
                    ));
                } else {
                    self.close_song();
                }
            }
            Action::ConfirmCloseFile => self.close_song(),
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
            Action::OpenSplitSongs => {
                if !self.require_document() {
                    return;
                }
                if self.split_is_running() {
                    self.status = "Already splitting.".to_owned();
                    return;
                }
                if let Some(source) = self.split_source() {
                    self.dialogs.split_songs = Some(SplitSongsDialog::new(source));
                }
            }
            Action::SplitSongsSubmitted {
                threshold_native,
                included,
                trailing_tail,
            } => self.start_split_songs(threshold_native, included, trailing_tail),
            Action::SplitSongsPreview { start_index } => self.preview_segment(start_index),
            Action::OpenSettings => {
                // Listed at open, so the picker offers what is plugged in now.
                self.dialogs.settings = Some(SettingsDialog::new(
                    &self.config,
                    self.audio.list_hardware_ports(),
                ));
            }
            Action::Exit => {
                if self.editor.is_dirty() || self.pack_is_dirty() {
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
                // On the pack tab, Undo reverses the last file edit; on the editor
                // tab it reverses the last song edit.
                if self.active_tab == AppTab::Pack {
                    self.undo_pack_edit();
                } else if self.require_document() {
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
                if self.active_tab == AppTab::Pack {
                    self.redo_pack_edit();
                } else if self.require_document() {
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
                if self.require_document() {
                    self.dialogs.goto = Some(GotoDialog::new());
                }
            }
            Action::OpenFindRegister => {
                if self.require_song() {
                    let song = self.editor.song().expect("gated");
                    self.dialogs.find_reg = Some(FindRegDialog::new(song));
                }
            }
            Action::OpenFindLoop => {
                // Either representation: the dialog wants a time per row and a
                // command density, both of which a VGM can give directly.
                let doc = match (self.editor.snapshot(), self.editor.vgm()) {
                    (Some(song), _) => Some(crate::dialogs::LoopSearchDoc::from_song(&song)),
                    (None, Some(file)) => Some(crate::dialogs::LoopSearchDoc::from_vgm(file)),
                    (None, None) => None,
                };
                match doc {
                    Some(doc) => self.dialogs.find_loop = Some(FindLoopDialog::new(doc)),
                    None => self.status = "Please open a song first.".to_owned(),
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
                if !self.require_document() {
                    return;
                }
                // The document itself, not its OPL projection: the tag lives
                // in the file, and the projection is only a view of the stream.
                match (self.editor.vgm(), self.editor.song()) {
                    (Some(file), _) => {
                        self.dialogs.gd3_tag = Some(Gd3TagDialog::new(file.tag.as_ref()));
                    }
                    (None, Some(song)) if song.is_vgm() => {
                        let tag = song.vgm_meta().and_then(|meta| meta.tag.as_ref());
                        self.dialogs.gd3_tag = Some(Gd3TagDialog::new(tag));
                    }
                    _ => self.status = "Only VGMs support tag editing".to_owned(),
                }
            }
            Action::OpenVgmMetadata => {
                if !self.require_document() {
                    return;
                }
                let dialog = match (self.editor.vgm(), self.editor.song()) {
                    (Some(file), _) => VgmMetadataDialog::for_vgm(file),
                    (None, Some(song)) => VgmMetadataDialog::new(song),
                    (None, None) => None,
                };
                match dialog {
                    Some(dialog) => self.dialogs.vgm_metadata = Some(dialog),
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
                        self.scroll_to = Some(table::ScrollTo::centered(0));
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
                        self.scroll_to = Some(table::ScrollTo::centered(0));
                        self.after_edit();
                    }
                    Err(message) => self.alerts.push_back(Alert::error(message)),
                }
            }
            Action::DeleteSelection => {
                if !self.require_document() {
                    return;
                }
                if self.editor.delete_selection() {
                    self.scroll_to = self.editor.selection.first().map(table::ScrollTo::centered);
                    self.after_edit();
                }
            }
            Action::AuditHeader => self.audit_header(),
            Action::ConfirmFixHeader => {
                let fixed = self.editor.fix_header();
                self.status = match fixed {
                    0 => "The header already agrees with the stream.".to_owned(),
                    1 => "Corrected 1 header field. Remember to save.".to_owned(),
                    count => format!("Corrected {count} header fields. Remember to save."),
                };
            }
            Action::OptimizeVgm => {
                if !self.editor.has_document() {
                    self.status = "Please open a song first.".to_owned();
                    return;
                }
                if self.editor.song().is_some_and(|song| !song.is_vgm()) {
                    self.status = "Only VGMs can be optimized".to_owned();
                    return;
                }
                match self.editor.optimize_vgm() {
                    Some((commands, bytes)) => {
                        self.status = format!(
                            "Optimized: removed {commands} command(s), saved {bytes} byte(s)"
                        );
                        self.scroll_to = Some(table::ScrollTo::centered(0));
                        self.after_edit();
                    }
                    None => {
                        self.status = "Nothing to optimize -- the VGM is already compact".to_owned()
                    }
                }
            }

            Action::OpenPackFolder => {
                if self.pack_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved package details?",
                        "This pack has unsaved changes. Open a different folder anyway?",
                        Action::ConfirmOpenPackFolder,
                    ));
                } else {
                    self.files.pick_folder();
                }
            }
            Action::ConfirmOpenPackFolder => self.files.pick_folder(),
            Action::OpenPackFolderAt(path) => self.files.open_folder_path(path),
            Action::SelectTab(tab) => self.select_tab(tab),
            Action::ClosePack => {
                if self.pack_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        "Discard unsaved package details?",
                        "This pack has unsaved changes. Close it anyway?",
                        Action::ConfirmClosePack,
                    ));
                } else {
                    self.close_pack();
                }
            }
            Action::ConfirmClosePack => self.close_pack(),
            Action::PackSaveDocs => self.save_pack_docs(),
            Action::PackScanVolumes => self.scan_pack_volumes(),
            Action::PackApplySuggestedModifiers { album } => self.apply_pack_modifiers(album),
            Action::PackConvertDatesToHyphens => self.convert_pack_dates_to_hyphens(),
            Action::PackRenameFromTags => self.rename_pack_tracks_from_tags(),
            Action::PackSelectSection(section) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.section = section;
                }
            }
            Action::PackAddScreenshot => {
                self.pending_screenshot = Some(ScreenshotPick::Add);
                self.files.pick_image();
            }
            Action::PackReplaceScreenshot(index) => self.replace_screenshot(index),
            Action::PackRenameScreenshotAt(index) => self.open_screenshot_rename(index),
            Action::PackAddScreenshotAs {
                file_name,
                bytes,
                recompress,
            } => self.add_screenshot_as(&file_name, bytes, recompress),
            Action::PackRenameScreenshot {
                original_name,
                file_name,
            } => self.rename_screenshot(&original_name, &file_name),
            Action::PackDeleteScreenshot(index) => self.confirm_delete_screenshot(index),
            Action::ConfirmDeleteScreenshot(name) => self.delete_screenshot(&name),
            Action::PackExportZip => self.export_pack_zip(false),
            Action::ConfirmExportZip => self.export_pack_zip(true),
            Action::PackTrackOpen(index) => self.open_track_in_editor(index),
            Action::PackTrackPreview(index) => self.preview_track(index),
            Action::PackStopPreview => self.stop_preview(),
            Action::OpenTrackQuickEdit(index) => self.open_track_quick_edit(index),
            Action::PackMoveTrack { index, delta } => self.move_pack_track(index, delta),
            Action::PackMoveTrackTo { from, to } => self.move_pack_track_to(from, to),
            Action::PackFocusTrack(index) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.focused_track = Some(index);
                }
            }
            Action::PackMoveFocusedTrack { delta } => self.move_focused_pack_track(delta),
            Action::OptimizeImage(index) => self.optimize_image(index),
            Action::QuickEditSubmitted {
                original_name,
                file_name,
                tag,
            } => self.quick_edit_submitted(original_name, file_name, *tag),
            Action::OpenBulkTag => self.open_bulk_tag(),
            Action::BulkTagSubmitted { targets, overlay } => {
                self.bulk_tag_submitted(targets, *overlay);
            }

            Action::Help => self.dialogs.help = Some(HelpDialog),
            Action::About => self.alerts.push_back(Alert::new("About", about_text())),

            Action::Play => self.do_play(),
            Action::Stop => self.do_stop(),
            Action::PlayTail => self.do_play_tail(),
            Action::PlaySeam => self.do_play_seam(),
            Action::TogglePlayback => {
                if !self.require_playable() {
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
                    self.scroll_to = Some(table::ScrollTo::centered(row));
                }
            }
            Action::WaveformClicked { index, ms } => {
                self.editor.selection.select_only(index);
                // Bring the table to where playback would start, that row at the
                // top: the click says "play from here", so what follows it is
                // what the user wants to read -- not the rows before it, which
                // is what centring would spend half the view on.
                self.scroll_to = Some(table::ScrollTo::to_top(index));
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
            Action::CropToMarkers => {
                if !self.require_document() {
                    return;
                }
                match self.editor.crop_to_markers() {
                    Some((kept, restored)) => {
                        // The restored writes are instructions the user did not
                        // put there, so they are worth accounting for -- but only
                        // when there were any; a "0" reads as a puzzle.
                        self.status = match restored {
                            0 => format!("Cropped to {kept} instruction(s)."),
                            n => format!(
                                "Cropped to {kept} instruction(s), including {n} that restore the chip state."
                            ),
                        };
                        self.after_region_edit();
                    }
                    None => self.status = NOTHING_MARKED.to_owned(),
                }
            }
            Action::DeleteMarkedRegion => {
                if !self.require_document() {
                    return;
                }
                match self.editor.delete_marked_region() {
                    Some((removed, bridged)) => {
                        self.status = match bridged {
                            0 => format!("Deleted {removed} instruction(s)."),
                            n => format!(
                                "Deleted {removed} instruction(s), leaving {n} write(s) to carry the chip state across the seam."
                            ),
                        };
                        self.after_region_edit();
                    }
                    None => self.status = NOTHING_MARKED.to_owned(),
                }
            }
            Action::FindLoopSearch { min_len_commands } => self.start_loop_search(min_len_commands),
            Action::CancelLoopSearch => {
                self.tasks.cancel(TaskKind::LoopSearch);
                self.status = "Loop search cancelled.".to_owned();
            }

            Action::ToggleChannel(channel) => {
                self.channels.opl().toggle_channel(channel);
                self.audio.set_muting(self.channels.muting());
            }
            Action::MutingChanged => self.audio.set_muting(self.channels.muting()),
            Action::PanningChanged => self.audio.set_panning(self.channels.panning()),
            Action::SetBoost { value, persist } => self.set_boost(value, persist),
            Action::SetLockBoost(lock) => self.set_lock_boost(lock),
            Action::MatchVolume => self.match_volume(),
            Action::MeasureVolumeModifier => self.measure_volume_modifier(),
            Action::VolumeFieldFocused(focused) => self.volume_field_editing = focused,

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
            Action::PreviewSkin {
                theme,
                pad_style,
                deck_style,
            } => self.preview_skin(ctx, theme, pad_style, deck_style),
            Action::PreviewCores(cores) => self.preview_cores(cores),
        }
    }

    // -- the workflows -----------------------------------------------------

    fn load_file(&mut self, file: PickedFile) {
        // Loading a song belongs to the editor: stop any pack preview and show
        // the editor tab so the load isn't invisible (menu Open, drag-and-drop,
        // and the CLI initial load can all fire while the pack tab is active).
        // Idempotent with open_track_in_editor, which also sets the tab.
        self.stop_preview();
        self.active_tab = AppTab::Editor;
        let name = file.name.clone();
        match self.editor.load(file) {
            Ok(report) => {
                self.status = format!("Successfully opened {name}.");
                // A dialog left open across a load would edit the wrong song
                // -- a stale Save silently corrupting it -- so anything
                // song-bound closes with the song.
                self.close_song_dialogs();
                self.waveform = WaveformState::default();
                // The exports belong to the song being replaced; drop them
                // rather than write out a song no longer on screen. (Their own
                // kinds, so this does not disturb the waveform render below.)
                self.tasks.cancel(TaskKind::RenderWav);
                self.tasks.cancel(TaskKind::Split);
                // Likewise a running volume scan: its peak is the old song's,
                // and landing late it would set this song's volume from it.
                self.tasks.cancel(TaskKind::VolumeScan);
                self.split_flow = None;
                self.submit_waveform(None);
                // Unload, not pause: the old stream's position must not leak
                // into the fresh cursor/readout via the end-of-playback
                // update below.
                self.audio.unload();
                self.peak_meter = PeakMeterState::default();
                // A new song starts with no clipping ceiling; its own limiter has
                // not engaged yet.
                self.boost_ceiling = None;
                self.audio_revision = None;
                self.was_playing = false;
                self.last_first_selected = None;
                self.scroll_to = Some(table::ScrollTo::centered(0));

                // What the document can do decides the rest. A VGM for chips
                // there is no core for is not a broken file -- it opens for
                // trimming, with the panels that need an OPL stream gone -- so
                // the difference is which of these two runs, not an error.
                match self.editor.song() {
                    Some(song) => {
                        // A fresh song starts with every channel audible and
                        // panning reset to Original (pans seeded from the song
                        // type); stale mute/pan state must not carry over.
                        self.channels = ChipPanels::for_song(song);
                        let file_version = song.file_version;
                        self.position.set_length_ms(song.total_delay_ms());
                        self.position.set_position_ms(0);
                        self.push_load_warnings(report, file_version);
                        // Unless the volume is locked, a freshly opened song
                        // starts at the volume its header modifier asks for
                        // (unity for a DRO), so the boost does not carry over.
                        if !self.config.audio.lock_boost {
                            let boost = self.song_modifier_boost();
                            self.set_boost(boost, false);
                        }
                    }
                    None => {
                        let file = self.editor.vgm().expect("just loaded");
                        let chips = file.chip_list();
                        self.channels = ChipPanels::for_vgm(file);
                        self.position.set_length_ms(file.total_ms());
                        self.position.set_position_ms(0);
                        // What the status promises has to match what the
                        // registry can actually build: most VGMs play in full
                        // now, and "not supported" is only true when *no* chip
                        // in the file has a core.
                        let kinds: Vec<_> =
                            file.header.chips().iter().map(|chip| chip.kind).collect();
                        self.status = match dro_synth::playability(&kinds) {
                            dro_synth::Playability::Full => {
                                format!("Successfully opened {name} ({chips}).")
                            }
                            dro_synth::Playability::Partial(missing) => {
                                let missing: Vec<&str> =
                                    missing.iter().map(|kind| kind.name()).collect();
                                format!(
                                    "Opened {name} ({chips}); no core yet for {}, which will \
                                     stay silent.",
                                    missing.join(", ")
                                )
                            }
                            dro_synth::Playability::None => {
                                format!("Opened {name} ({chips}); playback is not supported yet.")
                            }
                        };
                    }
                }
            }
            // Readable as a container, but its commands will not walk, so there
            // are no rows to show. The dialog says what the file is instead.
            Err(LoadFailure::Unwalkable { file, folder }) => {
                self.status = format!("{name} could not be read as commands.");
                self.dialogs.unwalkable_vgm = Some(UnwalkableVgmDialog::new(&file, folder));
            }
            Err(LoadFailure::Unreadable(message)) => self
                .alerts
                .push_back(Alert::new("Failed to load file", message)),
        }
    }

    /// Unloads the song, leaving the editor as it starts: the same teardown a
    /// load does before installing the next song, minus the song.
    fn close_song(&mut self) {
        self.editor.close();
        self.close_song_dialogs();
        // The exports and the analysis belong to a song that has gone.
        self.tasks.cancel(TaskKind::RenderWav);
        self.tasks.cancel(TaskKind::Split);
        self.tasks.cancel(TaskKind::VolumeScan);
        self.split_flow = None;
        self.audio.unload();
        self.audio_revision = None;
        self.was_playing = false;
        self.waveform = WaveformState::default();
        self.peak_meter = PeakMeterState::default();
        self.boost_ceiling = None;
        self.position.set_length_ms(0);
        self.position.set_position_ms(0);
        self.last_first_selected = None;
        self.status = "Closed the song.".to_owned();
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
        if !self.require_document() {
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

    // -- pack mode ----------------------------------------------------------

    fn pack_is_dirty(&self) -> bool {
        self.pack.as_ref().is_some_and(|pack| pack.dirty)
    }

    /// Whether any pack file mutation is in flight (a reorder/undo/redo sequence,
    /// or a quick-edit rewrite/rename), so a new one is deferred rather than
    /// interleaved with it.
    fn pack_busy(&self) -> bool {
        self.pack_run.is_some()
            || self.pending_pack_undo.is_some()
            || self.pending_rewrite.is_some()
            || self.pack_service.is_busy()
    }

    /// Starts running `transaction` -- its `forward` mutations, or (for `Undo`)
    /// its `inverse` -- one at a time through the file service.
    fn start_pack_run(&mut self, transaction: PackTransaction, kind: PackRunKind) {
        // Every pack edit passes through here, so this is where a run of
        // keyboard reorders ends: anything that is not the next press in that
        // run -- a drag, an undo, a batch rename -- is an edit of its own.
        if !self.pack_run_coalesces {
            self.coalesce_next_reorder = None;
        }
        self.stop_preview();
        let mutations = if kind == PackRunKind::Undo {
            transaction.inverse.clone()
        } else {
            transaction.forward.clone()
        };
        self.pack_run = Some(PackRun {
            queue: mutations.into(),
            transaction,
            kind,
            rename_in_flight: false,
        });
        self.advance_pack_run();
    }

    /// Runs the next mutation of the in-flight sequence, or -- once the queue
    /// drains -- lands its transaction on the right stack and rescans the folder.
    fn advance_pack_run(&mut self) {
        let next = match self.pack_run.as_mut() {
            Some(run) => run.queue.pop_front(),
            None => return,
        };
        match next {
            Some(PackMutation::Rename { from, to }) => {
                if let Some(run) = self.pack_run.as_mut() {
                    run.rename_in_flight = true;
                }
                self.files.rename(from, to);
            }
            Some(PackMutation::Write { path, bytes }) => {
                self.pending_saves.push_back(SavePurpose::PackOp);
                self.files.save(SaveRequest::InPlace { path, bytes });
            }
            Some(PackMutation::Delete { path }) => self.files.delete(path),
            None => {
                let Some(run) = self.pack_run.take() else {
                    return;
                };
                let PackRun {
                    mut transaction,
                    kind,
                    ..
                } = run;
                let label = transaction.label.clone();
                match kind {
                    PackRunKind::NewEdit => {
                        // A run of Alt+arrow presses on one track is one edit as
                        // far as the user is concerned: nine presses to lift a
                        // track to the top must not be nine undos back down.
                        if std::mem::take(&mut self.pack_run_coalesces)
                            && let Some(previous) = self.pack_undo.pop()
                        {
                            transaction = previous.then(transaction);
                        }
                        self.pack_undo.push(transaction);
                        self.pack_redo.clear();
                    }
                    PackRunKind::Redo => self.pack_undo.push(transaction),
                    PackRunKind::Undo => self.pack_redo.push(transaction),
                }
                self.rescan_pack_folder();
                self.status = match kind {
                    PackRunKind::Undo => format!("Undone: {label}."),
                    PackRunKind::Redo => format!("Redone: {label}."),
                    PackRunKind::NewEdit => format!("{label}."),
                };
            }
        }
    }

    /// Aborts the in-flight sequence after a failed rename/write, resyncing the
    /// folder to whatever actually landed. The transaction is discarded (not
    /// stacked), since it did not fully apply.
    fn abort_pack_run(&mut self, message: String) {
        self.pack_run = None;
        self.alerts
            .push_back(Alert::new("Track operation failed", message));
        self.rescan_pack_folder();
    }

    /// Drops the pack undo/redo history and any in-flight sequence -- for opening
    /// a new project or closing the current one. (A same-folder rescan keeps it.)
    fn clear_pack_edits(&mut self) {
        self.pack_run = None;
        self.pack_undo.clear();
        self.pack_redo.clear();
        self.pending_pack_undo = None;
    }

    /// Moves the track at `index` by `delta` (`-1` up, `+1` down), renumbering the
    /// affected files. Ignored while another sequence runs or the move is a no-op.
    fn move_pack_track(&mut self, index: usize, delta: isize) {
        if self.pack_busy() {
            return;
        }
        let Some(to) = index.checked_add_signed(delta) else {
            return;
        };
        self.move_pack_track_to(index, to);
    }

    /// Moves the focused track one slot: Alt+Up / Alt+Down, the keyboard path to
    /// the drag handle.
    ///
    /// The focus and the scroll request move with the track, so the keys can be
    /// pressed again immediately and the row cannot walk off the view. Nothing
    /// happens with no focused row, at the ends of the list, or while another
    /// file sequence is still running -- moving the focus then would put it on a
    /// track that did not move.
    fn move_focused_pack_track(&mut self, delta: isize) {
        if self.pack_busy() {
            return;
        }
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let Some(from) = pack.focused_track else {
            self.status = "Click a track first, then Alt+Up / Alt+Down to move it.".to_owned();
            return;
        };
        let Some(to) = from
            .checked_add_signed(delta)
            .filter(|to| *to < pack.tracks.len())
        else {
            return; // already at the end it is being pushed towards
        };
        // Picking up where the last keyboard move left off continues that edit
        // rather than starting another: the presses fold into one undo. Whether
        // it folds is decided when the run lands, since that is when there is a
        // transaction to fold into.
        self.pack_run_coalesces = self.coalesce_next_reorder == Some(from);
        if let Some(pack) = self.pack.as_mut() {
            pack.focused_track = Some(to);
            pack.scroll_to_track = Some(to);
        }
        self.move_pack_track_to(from, to);
        // Arm the *next* press to fold into this one.
        self.coalesce_next_reorder = Some(to);
    }

    /// Moves the track at `from` to `to`, renumbering every file the move
    /// displaces -- what dropping a dragged row runs. Ignored while another
    /// sequence runs or the move changes nothing.
    fn move_pack_track_to(&mut self, from: usize, to: usize) {
        if self.pack_busy() {
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.reorder_transaction(from, to));
        if let Some(transaction) = transaction {
            self.start_pack_run(transaction, PackRunKind::NewEdit);
        }
    }

    /// Undo the most recent pack edit, running its inverse. Ignored while busy.
    fn undo_pack_edit(&mut self) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        if let Some(transaction) = self.pack_undo.pop() {
            self.start_pack_run(transaction, PackRunKind::Undo);
        } else {
            self.status = "Nothing to undo.".to_owned();
        }
    }

    /// Redo the most recently undone pack edit, re-running its forward. Ignored
    /// while busy.
    fn redo_pack_edit(&mut self) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        if let Some(transaction) = self.pack_redo.pop() {
            self.start_pack_run(transaction, PackRunKind::Redo);
        } else {
            self.status = "Nothing to redo.".to_owned();
        }
    }

    /// Installs a freshly scanned folder as the pack project, or -- when it is a
    /// redelivery of the folder already open -- rescans in place, keeping the
    /// edited metadata.
    fn open_folder(&mut self, folder: PickedFolder) {
        // Any folder delivery invalidates a running whole-pack volume scan: a
        // rescan may have renamed or rewritten the files it snapshotted, and a
        // different folder's scan must never fill this one's Peak column. The
        // peaks map itself is pruned per track in `refresh_files`.
        self.tasks.cancel(TaskKind::PackVolumeScan);
        let same = self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.folder_path.is_some() && pack.folder_path == folder.path);
        if same {
            // Keep a running preview alive across an in-place rescan (e.g. after
            // a screenshot optimise redelivers the folder): refresh_files
            // re-matches it by name. Only stop the audio if that track vanished.
            let preview_lost = if let Some(pack) = self.pack.as_mut() {
                let had_preview = pack.preview.is_some();
                pack.refresh_files(folder);
                had_preview && pack.preview.is_none()
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
            self.close_pack_dialogs();
            return;
        }
        self.stop_preview();
        // A brand-new project starts with an empty edit history.
        self.clear_pack_edits();
        let today = self.pack_service.today();
        let state = PackState::from_folder(folder, today);
        let warning = state.parse_warning.clone();
        let name = state.folder_name.clone();
        self.pack = Some(state);
        self.active_tab = AppTab::Pack;
        self.close_song_dialogs();
        self.close_pack_dialogs();
        // The editor's audio must not keep playing under the pack view.
        self.audio.unload();
        self.audio_revision = None;
        self.status = format!("Opened pack project: {name}.");
        if let Some(warning) = warning {
            self.alerts.push_back(Alert::new(
                "Description not parsed",
                format!("{warning}\n\nSaving the package files will overwrite it."),
            ));
        }
    }

    fn select_tab(&mut self, tab: AppTab) {
        if self.pack.is_none() {
            self.active_tab = AppTab::Editor;
            return;
        }
        if self.active_tab == tab {
            return;
        }
        self.stop_preview();
        // Leaving the editor: its audio must not keep playing under the pack view
        // (mirrors open_folder's rule). Resetting the revision makes the next
        // editor Play reload cleanly.
        if self.active_tab == AppTab::Editor {
            self.audio.unload();
            self.audio_revision = None;
        }
        self.active_tab = tab;
        if tab == AppTab::Pack {
            // Song-bound dialogs (Find Register, DRO Info, GD3, VGM metadata)
            // and Goto don't belong on the pack tab -- mirror the menu gating
            // that disables them there.
            self.close_song_dialogs();
            self.dialogs.goto = None;
            // Returning to the pack tab re-scans the folder so edits made in the
            // editor (or renames) are reflected.
            if let Some(path) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) {
                self.files.open_folder_path(path);
            }
        }
    }

    fn close_pack(&mut self) {
        self.stop_preview();
        self.close_pack_dialogs();
        self.clear_pack_edits();
        self.pack = None;
        self.active_tab = AppTab::Editor;
        self.status = "Closed the pack project.".to_owned();
    }

    /// Saves `Game Name.txt` and `Game Name.m3u` into the folder.
    fn save_pack_docs(&mut self) {
        if !self.pack.as_ref().is_some_and(PackState::can_save) {
            if self.pack.is_some() {
                self.alerts.push_back(Alert::error(
                    "Enter a game name before saving the package files.",
                ));
            }
            return;
        }
        // Fresh batch: forget any failure from a previous save-docs run.
        self.pack_docs_failed = false;
        let pack = self.pack.as_ref().expect("checked");
        let stem = pack.doc_stem();
        let description = pack.description_text().into_bytes();
        let m3u = pack.m3u_text(false).into_bytes();
        let folder = pack.folder_path.clone();
        let docs = [
            (format!("{stem}.txt"), description),
            (format!("{stem}.m3u"), m3u),
        ];
        for (name, bytes) in docs {
            self.pending_saves.push_back(SavePurpose::PackDoc);
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
    fn export_pack_zip(&mut self, confirmed: bool) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let validations = pack.validations();
        let request = pack.export_request();
        // The `pack` borrow ends here (validations and request are owned).
        if !validations.errors.is_empty() {
            self.alerts
                .push_back(Alert::error(validations.errors.join("\n")));
            return;
        }
        // Only errors block and only warnings prompt; the note tier is for the
        // submission checklist and deliberately never reaches the export dialog.
        if !validations.warnings.is_empty() && !confirmed {
            // The title already asks the question, so the body is just the list
            // -- repeating it at the end would only bury it under a scroll when
            // a pack trips a lot of checks.
            let listed = validations
                .warnings
                .iter()
                .map(|warning| format!("\u{2022} {warning}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.alerts.push_back(Alert::confirm(
                "Export anyway?",
                format!("These submission checks did not pass:\n\n{listed}"),
                Action::ConfirmExportZip,
            ));
            return;
        }
        // Keep the folder's own docs in step with the zip's.
        if self.pack.as_ref().is_some_and(|pack| pack.dirty) {
            self.save_pack_docs();
        }
        self.pack_service.submit(request);
        self.status = "Building pack zip...".to_owned();
    }

    /// Previews a track through the audio output.
    fn preview_track(&mut self, index: usize) {
        let source = self
            .pack
            .as_ref()
            .and_then(|pack| pack.tracks.get(index))
            .and_then(crate::pack::PackTrack::preview_source);
        let Some(source) = source else {
            return;
        };
        // Preview with the track's own default panning and no channel mutes: the
        // editor's channel panel is for a different song, and its stored
        // panning/muting would otherwise leak into the preview (e.g. a dual-OPL2
        // editor song's fixed hard-L/R image applied to a mono track plays it
        // hard left). Panning is an OPL idea, so a track for other chips has
        // none to set.
        let preview_panning = source
            .opl()
            .map(|song| ChannelPanel::for_song(song).panning());
        // `load` below tears down the editor's stream the instant it runs --
        // success or not -- so the editor's audio snapshot is gone regardless.
        // Invalidate the revision *before* the load so the editor's next Play
        // reloads its own song instead of wedging on "No song is loaded" or
        // resuming this preview. Clear any prior preview marker up front too, so
        // a failure below can't strand a stop button on the old track.
        self.audio_revision = None;
        if let Some(pack) = self.pack.as_mut() {
            pack.preview = None;
        }
        // Preview at the track's own volume: unless the volume is locked, start it
        // from the track's header modifier -- on a copy of the config, so the
        // preview does not disturb the editor's volume.
        let mut preview_config = self.config.audio.clone();
        if !preview_config.lock_boost {
            preview_config.boost = source
                .opl()
                .map_or(preview_config.boost, |song| Self::modifier_boost(song));
        }
        self.audio.pause();
        if let Err(message) = self.audio.load(source, &preview_config) {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.set_muting(Muting::all());
        if let Some(panning) = preview_panning {
            self.audio.set_panning(panning);
        }
        if let Err(message) = self.audio.play() {
            // Load succeeded but playback won't start: drop the half-started
            // preview so the service isn't left holding it (and the editor's
            // next Play reloads cleanly via the reset revision above).
            self.audio.unload();
            self.alerts.push_back(Alert::error(message));
            return;
        }
        if let Some(pack) = self.pack.as_mut() {
            pack.preview = Some(index);
        }
    }

    fn stop_preview(&mut self) {
        if self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.preview.is_some())
        {
            self.audio.pause();
            self.audio.rewind();
            if let Some(pack) = self.pack.as_mut() {
                pack.preview = None;
            }
            self.audio_revision = None;
        }
    }

    /// Loads a track into the editor and switches to the editor tab. The pack
    /// project is retained; returning to it rescans the folder.
    fn open_track_in_editor(&mut self, index: usize) {
        let Some(track) = self.pack.as_ref().and_then(|pack| pack.tracks.get(index)) else {
            return;
        };
        // Every readable track opens: `load_file` sorts out which
        // representation it needs. Only one whose commands will not walk has
        // nothing to show, and that gets the dialog rather than an empty table.
        let file = PickedFile {
            name: track.file_name.clone(),
            path: track.path.clone(),
            bytes: track.bytes.clone(),
        };
        // load_file stops any preview and switches to the editor tab; the
        // discard-changes prompt (if the editor is dirty) defers both until the
        // load is confirmed.
        self.load_or_confirm(file);
    }

    fn open_track_quick_edit(&mut self, index: usize) {
        let dialog = self.pack.as_ref().and_then(|pack| {
            let track = pack.tracks.get(index)?;
            if !track.is_readable() {
                return None;
            }
            let tag = track.tag();
            // Every other track's name, so a rename can't collide with one.
            let siblings = pack
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
        if self.pack_busy() {
            return;
        }
        self.stop_preview();
        // Re-resolve the target by the name the dialog opened on: a rescan may
        // have reordered the list since, so the original index is unreliable.
        let Some(track) = self.pack.as_ref().and_then(|pack| {
            pack.tracks
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
        let Some(new_bytes) = track.retagged(&new_name, tag) else {
            return;
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
                vec![PackMutation::Write {
                    path: old_path.clone(),
                    bytes: new_bytes.clone(),
                }],
                vec![PackMutation::Write {
                    path: old_path.clone(),
                    bytes: old_bytes,
                }],
            )
        } else {
            (
                vec![
                    PackMutation::Rename {
                        from: old_path.clone(),
                        to: new_name.clone(),
                    },
                    PackMutation::Write {
                        path: new_path.clone(),
                        bytes: new_bytes.clone(),
                    },
                ],
                vec![
                    PackMutation::Rename {
                        from: new_path.clone(),
                        to: old_name.clone(),
                    },
                    PackMutation::Write {
                        path: old_path.clone(),
                        bytes: old_bytes,
                    },
                ],
            )
        };
        self.pending_pack_undo = Some(PackTransaction {
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

    /// Opens the bulk-tag dialog over every readable track, its fields seeded
    /// from the package metadata. A no-op with no pack open or no readable tracks.
    fn open_bulk_tag(&mut self) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let tracks: Vec<(String, String)> = pack
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| track.is_readable())
            .map(|(index, track)| {
                let title = track
                    .entry
                    .as_ref()
                    .map_or("", |entry| entry.title.as_str());
                (track.file_name.clone(), format!("{:02} {title}", index + 1))
            })
            .collect();
        if tracks.is_empty() {
            self.status = "No readable tracks to tag.".to_owned();
            return;
        }
        let overlay = crate::pack::seed_from_meta(&pack.meta);
        self.dialogs.bulk_tag = Some(BulkTagDialog::new(tracks, overlay));
    }

    /// Applies a bulk GD3 edit: overlay the checked fields onto each target
    /// track's existing tag and rewrite the files as one undoable batch. Tracks
    /// whose tag would not change (and any not currently VGMs) are skipped, so a
    /// no-op selection writes nothing.
    fn bulk_tag_submitted(&mut self, targets: Vec<String>, overlay: BulkTagOverlay) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        self.stop_preview();
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let mut forward = Vec::new();
        let mut inverse = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for name in &targets {
            let Some(track) = pack.tracks.iter().find(|track| &track.file_name == name) else {
                continue;
            };
            // Only VGMs carry a GD3 tag; the pack list is VGM/VGZ only, but guard
            // anyway so a non-VGM can never be rewritten by a bulk edit.
            let (true, Some(path)) = (track.is_readable(), track.path.clone()) else {
                continue;
            };
            let current = track.tag().cloned().unwrap_or_default();
            let new_tag = overlay.apply_to(&current);
            if new_tag == current {
                continue; // nothing changed for this track
            }
            let Some(written) = track.retagged(&track.file_name, new_tag) else {
                continue;
            };
            match written {
                Ok(bytes) => {
                    forward.push(PackMutation::Write {
                        path: path.clone(),
                        bytes,
                    });
                    inverse.push(PackMutation::Write {
                        path,
                        bytes: track.bytes.clone(),
                    });
                }
                Err(message) => errors.push(format!("{name}: {message}")),
            }
        }

        if !errors.is_empty() {
            self.alerts.push_back(Alert::error(errors.join("\n")));
        }
        if forward.is_empty() {
            self.status = "Bulk tag: nothing changed.".to_owned();
            return;
        }
        let count = forward.len();
        let transaction = PackTransaction {
            label: format!(
                "Bulk tag {count} track{}",
                if count == 1 { "" } else { "s" }
            ),
            forward,
            inverse,
        };
        self.start_pack_run(transaction, PackRunKind::NewEdit);
    }

    /// Measures every readable track's peak in one background task (so the pack's
    /// many songs never freeze the UI); the results reach
    /// [`Self::handle_pack_peaks`] through `poll_services` and fill the Peak column.
    fn scan_pack_volumes(&mut self) {
        let sample_rate = self.config.audio.frequency;
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let tracks: Vec<(String, std::sync::Arc<dro_core::Song>)> = pack
            .tracks
            .iter()
            .filter_map(|track| {
                // Measuring a peak means rendering, so only the tracks this app
                // has a core for can be scanned.
                Some((track.file_name.clone(), track.playable_song()?))
            })
            .collect();
        if tracks.is_empty() {
            self.status = "No tracks this app can render to scan.".to_owned();
            return;
        }
        let count = tracks.len();
        self.tasks.submit(
            TaskRequest::PackVolumeScan {
                tracks,
                sample_rate,
            },
            None,
        );
        self.status = format!("Scanning {count} track volume(s)...");
    }

    /// Routes a streamed loop-search snapshot into the Find Loop dialog, if it is
    /// still open (it may have been closed mid-search, in which case the result is
    /// simply dropped, like the volume scan's).
    fn handle_loop_candidates(&mut self, candidates: Vec<dro_core::Candidate>) {
        let count = candidates.len();
        if let Some(dialog) = self.dialogs.find_loop.as_mut() {
            dialog.set_candidates(candidates);
        }
        self.status = format!("Found {count} loop candidate(s).");
    }

    /// Stores a finished pack volume scan's peaks (keyed by file name) for the Peak
    /// column and the suggested modifiers.
    fn handle_pack_peaks(&mut self, peaks: Vec<(String, dro_synth::Peak)>) {
        let Some(pack) = self.pack.as_mut() else {
            return;
        };
        let count = peaks.len();
        for (name, peak) in peaks {
            pack.peaks.insert(name, peak);
        }
        self.status = format!("Scanned {count} track volume(s).");
    }

    /// Sets each scanned track's VGM volume modifier so the pack is levelled, as
    /// one undoable batch. The skip-unchanged logic and the serialisation live in
    /// [`PackState::suggested_modifier_transaction`].
    ///
    /// `album` levels the whole pack by its loudest track (the VGMRips
    /// convention); otherwise each track is normalised to its own peak. It is
    /// written back to the Album latch, so the pad reflects what actually ran
    /// when the menu item was the one that asked.
    fn apply_pack_modifiers(&mut self, album: bool) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        if let Some(pack) = self.pack.as_mut() {
            pack.album_normalize = album;
        }
        // Applying mid-scan would use the peaks from *before* the scan and then
        // discard its result (the rewrite's rescan cancels it) -- confusing both
        // ways, so wait it out.
        if self.tasks.is_busy_kind(TaskKind::PackVolumeScan) {
            self.status = "Still scanning volumes...".to_owned();
            return;
        }
        self.stop_preview();
        let Some(transaction) = self
            .pack
            .as_ref()
            .and_then(PackState::suggested_modifier_transaction)
        else {
            self.status = "Volume modifiers: nothing to change (scan volumes first).".to_owned();
            return;
        };
        self.start_pack_run(transaction, PackRunKind::NewEdit);
    }

    /// The checklist's date fix-assist: rewrite every slash-separated release date
    /// to hyphens. The pack meta's own date is a form-level edit (applied at once,
    /// like typing); every track's GD3 date is rewritten as one undoable file
    /// batch, mirroring [`Self::apply_pack_modifiers`].
    fn convert_pack_dates_to_hyphens(&mut self) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        self.stop_preview();
        let meta_changed = self
            .pack
            .as_mut()
            .is_some_and(PackState::hyphenate_meta_date);
        match self
            .pack
            .as_ref()
            .and_then(PackState::date_hyphenation_transaction)
        {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            None if meta_changed => {
                self.status = "Converted the pack date to hyphens.".to_owned();
            }
            None => self.status = "No slash-separated dates to convert.".to_owned(),
        }
    }

    /// The name fix-assist: rename every file whose name has drifted from its GD3
    /// Track Name to the one `vgm_ren` would give it, as one undoable batch --
    /// the bulk counterpart of the quick-edit dialog's per-track rename.
    fn rename_pack_tracks_from_tags(&mut self) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        self.stop_preview();
        match self
            .pack
            .as_ref()
            .and_then(PackState::rename_from_tags_transaction)
        {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            None => self.status = "Every file name already matches its tag.".to_owned(),
        }
    }

    /// Kicks off an explicit lossless recompression of a screenshot.
    fn optimize_image(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let image = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .cloned();
        let Some(image) = image else {
            return;
        };
        self.status = format!("Optimising {}...", image.name);
        self.pack_service.optimize(image.name, image.bytes.to_vec());
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
        let found = self.pack.as_ref().and_then(|pack| {
            pack.images
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
        self.pending_pack_undo = Some(PackTransaction {
            label: format!("Optimise {}", optimized.name),
            forward: vec![PackMutation::Write {
                path: path.clone(),
                bytes: optimized.bytes.clone(),
            }],
            inverse: vec![PackMutation::Write {
                path: path.clone(),
                bytes: old_bytes,
            }],
        });
        self.pending_saves.push_back(SavePurpose::ImageWritten);
        self.files.save(SaveRequest::InPlace {
            path,
            bytes: optimized.bytes,
        });
    }

    /// Copies a picked screenshot into the open pack's folder, then rescans so
    /// the Screenshots section picks it up.
    ///
    /// It lands as `<Game Name>.png`, joining the `.txt` and `.m3u` the pack
    /// already names that way -- a screenshot straight out of DOSBox is called
    /// something like `dosbox_000.png`, and renaming it by hand is a step this
    /// tool exists to save. With no game name yet there is nothing to rename it
    /// to, so it keeps its own.
    ///
    /// Either way the name is made unique against the folder (`... (2).png`), so
    /// a second screenshot never silently overwrites the first; Rename... is
    /// then how it earns a name of its own ("Cool Game (Japan).png").
    fn add_screenshot(&mut self, file: PickedFile) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        // The picker filters to .png, but a determined user can still get past
        // that -- and a non-PNG here would ship in the zip and fail review.
        if dro_core::pack::PngInfo::parse(&file.bytes).is_none() {
            self.pending_screenshot = None;
            self.alerts.push_back(Alert::new(
                "Not a PNG",
                format!("{} is not a readable PNG image.", file.name),
            ));
            return;
        }
        match self.pending_screenshot.take() {
            // Replace keeps the existing file's name: that is what makes it a
            // replacement rather than a second screenshot. Its old bytes are the
            // inverse, so it lands on the undo stack like any other rewrite.
            Some(ScreenshotPick::Replace(path)) => {
                let old_bytes = pack
                    .images
                    .iter()
                    .find(|image| image.path.as_ref() == Some(&path))
                    .map(|image| image.bytes.to_vec());
                let Some(old_bytes) = old_bytes else {
                    return; // rescanned away while the picker was open
                };
                self.pending_pack_undo = Some(PackTransaction {
                    label: format!("Replace {}", file_label(&path)),
                    forward: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: file.bytes.clone(),
                    }],
                    inverse: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: old_bytes,
                    }],
                });
                self.status = format!("Replacing {}...", file_label(&path));
                self.pending_saves.push_back(SavePurpose::ImageWritten);
                self.files.save(SaveRequest::InPlace {
                    path,
                    bytes: file.bytes,
                });
            }
            _ => {
                if pack.folder_path.is_none() {
                    // Native-only; a folder with no path cannot be written to.
                    return;
                }
                let proposed = pack.next_screenshot_name().unwrap_or_else(|| {
                    let (stem, ext) = file
                        .name
                        .rsplit_once('.')
                        .unwrap_or((file.name.as_str(), "png"));
                    pack.free_image_name(stem, ext)
                });
                let siblings = pack.images.iter().map(|image| image.name.clone()).collect();
                // The name is settled before anything is written: the dialog
                // holds the bytes, so closing it leaves the folder untouched.
                self.dialogs.screenshot_rename = Some(ScreenshotRenameDialog::adding(
                    file.name, &proposed, file.bytes, siblings,
                ));
            }
        }
    }

    /// Opens the picker to overwrite the screenshot at `index`.
    fn replace_screenshot(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let path = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .and_then(|image| image.path.clone());
        let Some(path) = path else {
            return;
        };
        self.pending_screenshot = Some(ScreenshotPick::Replace(path));
        self.files.pick_image();
    }

    /// Writes a picked screenshot into the pack folder under the name the naming
    /// dialog settled on -- the second half of [`Self::add_screenshot`], once the
    /// user has seen and approved where it lands.
    /// With `recompress` the bytes go through oxipng *first* and the result is
    /// what lands, so the file is optimal in one write. Rewriting it afterwards
    /// would work too, but it would touch the disk twice and push a
    /// recompression of a file nobody has seen onto the undo stack.
    fn add_screenshot_as(&mut self, file_name: &str, bytes: Vec<u8>, recompress: bool) {
        let Some(folder) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) else {
            return;
        };
        let add = PendingAdd {
            path: folder.join(file_name),
            bytes,
        };
        if recompress {
            self.status = format!("Recompressing {file_name}...");
            self.pack_service
                .optimize(file_name.to_owned(), add.bytes.clone());
            self.pending_add = Some(add);
            return;
        }
        self.write_added_screenshot(add);
    }

    /// Writes an added screenshot's bytes into the pack folder.
    fn write_added_screenshot(&mut self, add: PendingAdd) {
        self.status = format!("Adding {}...", file_label(&add.path));
        self.pending_saves.push_back(SavePurpose::ScreenshotAdded);
        self.files.save(SaveRequest::InPlace {
            path: add.path,
            bytes: add.bytes,
        });
    }

    /// Opens the rename dialog on the screenshot at `index`, proposing the
    /// pack's own file-name stem.
    fn open_screenshot_rename(&mut self, index: usize) {
        let dialog = self.pack.as_ref().and_then(|pack| {
            let image = pack.images.get(index)?;
            // Every other screenshot's name, so a rename can't collide with one.
            let siblings = pack
                .images
                .iter()
                .filter(|other| other.name != image.name)
                .map(|other| other.name.clone())
                .collect();
            Some(ScreenshotRenameDialog::new(
                image.name.clone(),
                &pack.doc_stem(),
                siblings,
            ))
        });
        if let Some(dialog) = dialog {
            self.dialogs.screenshot_rename = Some(dialog);
        }
    }

    /// Runs the screenshot rename as a pack transaction, so Edit > Undo puts the
    /// old name back.
    fn rename_screenshot(&mut self, original_name: &str, file_name: &str) {
        if self.pack_busy() {
            self.status = "A track operation is still running.".to_owned();
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.rename_image_transaction(original_name, file_name));
        match transaction {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            // Rescanned away while the dialog was open.
            None => self.alerts.push_back(Alert::error(format!(
                "\"{original_name}\" is no longer in the folder; it was not renamed."
            ))),
        }
    }

    /// Asks before removing a screenshot from the folder. Undo can put it back
    /// while the pack stays open, but the file does leave the disk, so this is
    /// not something to do on a stray click.
    fn confirm_delete_screenshot(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let Some(name) = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .map(|image| image.name.clone())
        else {
            return;
        };
        self.alerts.push_back(Alert::confirm(
            "Delete screenshot?",
            format!("{name} will be deleted from the pack folder."),
            Action::ConfirmDeleteScreenshot(name),
        ));
    }

    /// Runs the delete as a pack transaction, so Edit > Undo writes it back.
    fn delete_screenshot(&mut self, name: &str) {
        if self.pack_busy() {
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.delete_image_transaction(name));
        if let Some(transaction) = transaction {
            self.start_pack_run(transaction, PackRunKind::NewEdit);
        }
    }

    fn rescan_pack_folder(&mut self) {
        if let Some(path) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) {
            self.files.open_folder_path(path);
        }
    }

    /// Closes pack-bound dialogs (quick-edit, bulk-tag and screenshot rename),
    /// analogous to [`Self::close_song_dialogs`]. Each binds to the folder's
    /// current contents, so a rescan that can reorder or drop files must dismiss
    /// them.
    fn close_pack_dialogs(&mut self) {
        self.dialogs.track_edit = None;
        self.dialogs.bulk_tag = None;
        self.dialogs.screenshot_rename = None;
    }

    fn do_play(&mut self) {
        if !self.require_playable() {
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
        if !self.require_playable() {
            return;
        }
        self.audio.pause();
        self.audio.rewind();
    }

    fn do_play_tail(&mut self) {
        if !self.require_playable() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Measured length, not the header's: on a DRO whose header overstates
        // the length, the header value would seek past the end and play nothing.
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
        if !self.require_playable() {
            return;
        }
        self.loop_enabled = true;
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Where the seam is, in milliseconds. Either representation can say:
        // a `Song` has the prefix sums already, and a VGM's is its own waits.
        let end = self.editor.markers.end();
        let Some(end_ms) = self.editor.song().map_or_else(
            || {
                self.editor.vgm().map(|file| {
                    let elapsed = file.stream().map_or(0, |stream| {
                        stream.total_samples() - stream.samples_from(end)
                    });
                    dro_core::util::smp_to_ms(
                        u32::try_from(elapsed).unwrap_or(u32::MAX),
                        dro_core::vgm::VGM_SAMPLE_RATE,
                    )
                })
            },
            |song| song.ms_offset_at(end),
        ) else {
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
        if !self.require_document() {
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

    /// Submits a background loop search of the current song. The streamed
    /// candidates reach the Find Loop dialog through [`Self::handle_loop_candidates`];
    /// cancel-on-resubmit means clicking Search again just restarts it.
    fn start_loop_search(&mut self, min_len_commands: usize) {
        // Either representation: a loop is a repeated block, which is not an
        // OPL idea.
        let source = match (self.editor.snapshot(), self.editor.vgm()) {
            (Some(song), _) => crate::tasks::LoopSearchSource::Opl(song),
            (None, Some(file)) => {
                crate::tasks::LoopSearchSource::Vgm(std::sync::Arc::new(file.clone()))
            }
            (None, None) => {
                self.status = "Please open a song first.".to_owned();
                return;
            }
        };
        self.tasks.submit(
            TaskRequest::LoopSearch {
                source,
                min_len_commands,
            },
            None,
        );
        self.status = "Searching for loops...".to_owned();
    }

    fn delay_navigate(&mut self, backwards: bool) {
        if !self.require_song() {
            return;
        }
        match self.editor.find_next(FindTarget::AnyDelay, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(table::ScrollTo::centered(index));
            }
            None => self.status = "No more delays found.".to_owned(),
        }
    }

    fn goto_submitted(&mut self, text: &str) {
        if !self.require_document() {
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
                self.scroll_to = Some(table::ScrollTo::centered(position));
                self.status = format!("Gone to position: {position:04X}");
            }
        }
    }

    fn find_register(&mut self, target: &str, backwards: bool) {
        // An empty choice is a silent no-op.
        if target.is_empty() || !self.require_song() {
            return;
        }
        let Ok(parsed) = target.parse::<FindTarget>() else {
            return;
        };
        match self.editor.find_next(parsed, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(table::ScrollTo::centered(index));
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
        // Changing where or through what playback goes has to take effect now,
        // not at the next Play: otherwise the old backend keeps playing and
        // keeps hold of its device, so a user switching away from hardware
        // output cannot get the serial port back until they press Play again.
        // The rebuild happens below, once the new config is in force.
        let output_changed = config.audio.cores != self.config.audio.cores
            || config.audio.resampling != self.config.audio.resampling
            || config.audio.retrowave_port != self.config.audio.retrowave_port;
        // Keep the registry's copy of the choices current, so the reload (and
        // every offline render) builds the cores just saved.
        dro_synth::registry::set_core_choices(config.audio.cores.clone());
        if let Err(error) = self.config_store.save(&config) {
            self.alerts
                .push_back(Alert::error(format!("Could not save settings: {error}")));
        }
        // Repaint the whole UI in the new scheme before anything else reads it.
        // Compare against what is *on screen*, which a live preview may already
        // have moved to the saved theme -- reapplying it then is just a no-op.
        if config.ui.theme != self.shown_skin().0 {
            theme::apply_palette(ctx, config.ui.theme);
        }
        // Whatever was being previewed is now the saved skin.
        self.skin_preview = None;
        // Only an audio change needs an output reload or a fresh waveform; a
        // theme-only change keeps the existing buckets and just recolours them.
        let audio_changed = config.audio != self.config.audio;
        let waveform_changed = config.audio.frequency != self.config.audio.frequency;
        let new_frequency = config.audio.frequency;
        self.config = config;
        // Don't retune the position panel to the configured rate while a stream
        // is live: it reports frames at the stream's real (still-old) rate, so
        // the readout would mix a new-rate length with old-rate frames. On the
        // next reload, ensure_audio adopts the new rate from output_rate (ux-16).
        if self.audio.output_rate().is_none() {
            self.position.set_frequency(new_frequency);
        }
        if let Some(song) = self.editor.song() {
            self.position.set_length_ms(song.total_delay_ms());
        }
        if audio_changed {
            // Reload the audio output lazily on the next play.
            self.audio_revision = None;
        }
        if output_changed {
            // A live stream is rebuilt in place -- the saved cores are heard
            // from where playback had reached, and a backend switch releases
            // the device it walked away from. An idle transport stays lazy.
            self.reload_audio_in_place();
        }
        if waveform_changed {
            self.submit_waveform(None);
        }
        self.status = "Settings saved.".to_owned();
    }

    /// Repaints in a skin without committing it. A colour scheme can only really
    /// be judged on the whole window, so the Settings dropdowns apply as they are
    /// picked; Close re-previews the settings the dialog opened with, putting the
    /// old skin back.
    ///
    /// Deliberately *not* written into `config`: that is what reaches the ini,
    /// and the volume lever saves it from under us (see [`Self::set_boost`]), so
    /// a preview parked there would persist itself behind the user's back.
    fn preview_skin(
        &mut self,
        ctx: &egui::Context,
        theme: ThemeChoice,
        pad_style: SurfaceChoice,
        deck_style: SurfaceChoice,
    ) {
        if self.shown_skin().0 != theme {
            theme::apply_palette(ctx, theme);
        }
        // Matching the saved settings *is* no preview, so Close leaves nothing
        // behind to go stale.
        let ui = &self.config.ui;
        self.skin_preview = ((theme, pad_style, deck_style)
            != (ui.theme, ui.pad_style, ui.deck_style))
            .then_some((theme, pad_style, deck_style));
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

    /// Auditions a core map without saving it: the Settings picker's live
    /// preview. The registry choices are replaced and the loaded stream --
    /// which holds the cores it was built with -- is rebuilt in place, so the
    /// picked core is heard from the position the old one had reached.
    fn preview_cores(&mut self, cores: std::collections::BTreeMap<String, String>) {
        dro_synth::registry::set_core_choices(cores);
        self.reload_audio_in_place();
    }

    /// Rebuilds the loaded audio stream with today's cores and config, keeping
    /// the playback position and the playing/paused state.
    ///
    /// A stopped or unloaded transport has nothing to rebuild: the next Play
    /// builds its stream lazily and picks everything up then, as it always has.
    fn reload_audio_in_place(&mut self) {
        let position_ms = self.audio.position().map(|position| position.elapsed_ms);
        let playing = self.audio.is_playing();
        if position_ms.is_none() {
            return;
        }
        self.audio.pause();
        self.audio.unload();
        self.audio_revision = None;
        if self.ensure_audio().is_err() {
            // Nothing to rebuild after all (the document went away, or the
            // device did); the lazy path will say so when Play is next pressed.
            return;
        }
        if let Some(ms) = position_ms {
            self.audio.seek_ms(ms);
        }
        if playing && let Err(error) = self.audio.play() {
            self.alerts
                .push_back(Alert::error(format!("Could not resume playback: {error}")));
        }
    }

    /// Gates an action on a loaded song, setting a status message asking the
    /// user to open a file when none is loaded.
    fn require_song(&mut self) -> bool {
        if self.editor.has_song() {
            true
        } else {
            self.status = "Please open a DRO file first.".to_owned();
            false
        }
    }

    /// Whether the loaded document's sound passes through this program as
    /// samples, and can therefore be metered, boosted and panned.
    ///
    /// The config's own answer is about *OPL* output: hardware output sends the
    /// board's own sound out its own socket, so nothing here can measure or
    /// shape it. A document that is not OPL never reaches that board -- it is
    /// routed to the emulator whatever the setting says -- so for one of those
    /// the answer is yes regardless.
    fn output_renders_samples(&self) -> bool {
        self.config.audio.renders_samples() || !self.editor.has_song()
    }

    /// [`Self::output_renders_samples`] for tests, which have only a shared
    /// reference and no frame to draw.
    #[cfg(test)]
    pub(crate) fn output_renders_samples_for_test(&self) -> bool {
        self.output_renders_samples()
    }

    /// The gate for the transport: is there anything to hear?
    ///
    /// Between [`Self::require_song`] (an OPL stream) and
    /// [`Self::require_document`] (anything open). Playing needs neither of
    /// those exactly -- it needs a chip this app has a core for, which an OPL
    /// song always is and a VGM sometimes is.
    fn require_playable(&mut self) -> bool {
        if self.editor.capabilities().playable && self.editor.has_document() {
            true
        } else {
            self.status = "There is nothing here this app can play.".to_owned();
            false
        }
    }

    /// The gate for everything that works on a document of either kind.
    ///
    /// [`Self::require_song`] is the narrower one: it asks for an OPL stream,
    /// which is what rendering, splitting and the register analyser need. Saving,
    /// deleting, cropping and undo are not OPL ideas, so they ask this instead --
    /// otherwise a VGM for a chip we have no core for would open in the editor
    /// and then refuse to be edited.
    fn require_document(&mut self) -> bool {
        if self.editor.has_document() {
            true
        } else {
            self.status = "Please open a file first.".to_owned();
            false
        }
    }

    /// Everything every edit needs: stale audio paused, the length readout
    /// refreshed, and the waveform re-rendered (debounced, so holding Delete
    /// does not thrash the renderer -- a 1 s debounce).
    /// Reports where the loaded VGM's header disagrees with its stream, and
    /// offers to correct it.
    ///
    /// Offers, never does: a header is a claim about the file, and rewriting
    /// one the user did not ask about is how a pack of carefully-made rips
    /// quietly becomes a pack of subtly different ones.
    fn audit_header(&mut self) {
        let findings = self.editor.audit_header();
        if findings.is_empty() {
            self.status = "The header agrees with the stream; nothing to fix.".to_owned();
            return;
        }
        let mut message = String::from("This file's header disagrees with its own music:\n\n");
        for finding in &findings {
            message.push_str("  - ");
            message.push_str(&finding.describe());
            message.push('\n');
        }
        message.push_str("\nCorrect them? The stream is taken as the truth.");
        self.alerts.push_back(Alert::confirm(
            "Fix Header",
            message,
            Action::ConfirmFixHeader,
        ));
    }

    fn after_edit(&mut self) {
        self.audio.pause();
        self.audio_revision = None;
        // Playback starts where the cursor is -- the selected row, and the time
        // the position readout and the waveform cursor show. A crop or a delete
        // can leave any of them past the end of what is left, so anything now
        // outside the song comes back to the top, the one position every song is
        // guaranteed to have.
        let len = self.editor.len();
        let length_ms = self.editor.song().map_or(0, |song| song.total_delay_ms());
        let row_outside = self.editor.selection.first().is_some_and(|row| row >= len);
        if row_outside || self.position.position_ms() > length_ms {
            if len == 0 {
                self.editor.selection.clear();
            } else if row_outside {
                self.editor.selection.select_only(0);
                self.scroll_to = Some(table::ScrollTo::to_top(0));
            }
            self.reset_playback_start();
        }
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
        let source = match (self.editor.snapshot(), self.editor.vgm()) {
            (Some(song), _) => crate::tasks::WavSource::Opl(song),
            (None, Some(file)) => crate::tasks::WavSource::Vgm(std::sync::Arc::new(file.clone())),
            (None, None) => {
                self.require_document();
                return;
            }
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
                source,
                mix,
                sample_rate: self.config.audio.frequency,
                bit_depth: self.config.audio.bit_depth,
                resampling: self.resample_mode(),
            },
            None,
        );
        self.status = "Rendering to WAV...".to_owned();
    }

    /// Whether a split (of either kind) is somewhere between its dialog and its
    /// last written file.
    fn split_is_running(&self) -> bool {
        self.split_flow.is_some()
            || self.tasks.is_busy_kind(TaskKind::Split)
            || self.tasks.is_busy_kind(TaskKind::SplitSongs)
    }

    /// Asks where the channel split's files should go. The split itself starts
    /// once the answer arrives in `poll_services`.
    fn start_split(&mut self, format: SplitFormat, isolate_percussion: bool) {
        self.begin_split(PendingSplit::Channels {
            options: SplitOptions {
                format,
                isolate_percussion,
                audio: self.config.audio.clone(),
            },
        });
    }

    /// The loaded document as something a song split can run over, of either
    /// kind. `None` with nothing open.
    fn split_source(&self) -> Option<crate::tasks::SplitSource> {
        match (self.editor.snapshot(), self.editor.vgm()) {
            (Some(song), _) => Some(crate::tasks::SplitSource::Opl(song)),
            (None, Some(file)) => Some(crate::tasks::SplitSource::Vgm(std::sync::Arc::new(
                file.clone(),
            ))),
            (None, None) => None,
        }
    }

    /// Asks where the song split's files should go, then starts on the answer.
    fn start_split_songs(
        &mut self,
        threshold_native: u32,
        included: Vec<bool>,
        trailing_tail: u32,
    ) {
        self.begin_split(PendingSplit::Songs {
            threshold_native,
            included,
            trailing_tail,
        });
    }

    /// The shared entry both splits use: guard, stash the request, open the
    /// output-folder picker.
    fn begin_split(&mut self, pending: PendingSplit) {
        // The channel split decides which channel each register write belongs
        // to, so it needs an OPL stream; the song split only needs a document.
        let gate = if pending.is_songs() {
            Self::require_document
        } else {
            Self::require_song
        };
        if !gate(self) || self.split_is_running() {
            return;
        }
        self.split_flow = Some(SplitFlow::AwaitingFolder(pending));
        self.files.pick_output_folder();
    }

    /// Starts the split now that `dir` is known, or gives up if the picker was
    /// dismissed.
    fn split_into(&mut self, dir: Option<PathBuf>) {
        let Some(SplitFlow::AwaitingFolder(pending)) = self.split_flow.clone() else {
            // A folder arrived with no split waiting for it; nothing to do.
            return;
        };
        let songs = pending.is_songs();
        let request = match pending {
            PendingSplit::Channels { options } => self.editor.snapshot().map(|song| {
                (
                    TaskRequest::Split { song, options },
                    "Splitting channels...",
                )
            }),
            PendingSplit::Songs {
                threshold_native,
                included,
                trailing_tail,
            } => self.split_source().map(|source| {
                (
                    TaskRequest::SplitSongs {
                        source,
                        threshold_native,
                        included,
                        trailing_tail,
                    },
                    "Splitting songs...",
                )
            }),
        };
        let (Some(dir), Some((request, status))) = (dir, request) else {
            self.split_flow = None;
            self.status = "Split cancelled.".to_owned();
            return;
        };
        self.tasks.submit(request, None);
        self.split_flow = Some(SplitFlow::Rendering { dir, songs });
        self.status = status.to_owned();
    }

    /// Writes a finished split's files into the folder chosen for it.
    fn write_split(&mut self, outputs: Result<Vec<(String, Vec<u8>)>, String>) {
        // Only the split still being waited on: a result from one the user
        // abandoned (by loading another song) has nowhere to go.
        let Some(SplitFlow::Rendering { dir, songs }) = self.split_flow.clone() else {
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
            self.status = if songs {
                "No songs to split.".to_owned()
            } else {
                "No channels to split.".to_owned()
            };
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
            songs,
        });
    }

    /// Counts off one split file's save, reporting once the last one lands.
    fn split_file_saved(&mut self, ok: bool) {
        let Some(SplitFlow::Writing {
            dir,
            written,
            failed,
            songs,
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
        // no `SplitFile` left behind it -- the same rule pack mode's docs use.
        if self
            .pending_saves
            .iter()
            .any(|purpose| *purpose == SavePurpose::SplitFile)
        {
            return;
        }
        let (dir, written, failed, songs) = (dir.clone(), *written, *failed, *songs);
        self.split_flow = None;
        if failed {
            self.status = "Some split files could not be written.".to_owned();
            return;
        }
        self.finish_split(&dir, written, songs);
    }

    /// The success report once every split file has landed. A song split also
    /// offers to open the folder it filled as a pack project.
    fn finish_split(&mut self, dir: &Path, written: usize, songs: bool) {
        if songs {
            self.status = format!("Wrote {written} song(s) to {}.", dir.display());
            self.alerts.push_back(Alert::confirm(
                "Songs exported",
                format!(
                    "Wrote {written} song(s) to {}.\n\nOpen the folder as a pack project?",
                    dir.display()
                ),
                Action::OpenPackFolderAt(dir.to_path_buf()),
            ));
        } else {
            self.status = format!("Wrote {written} file(s) to {}.", dir.display());
        }
    }

    /// Previews a detected song: seek playback to its first instruction and play.
    fn preview_segment(&mut self, start_index: usize) {
        if !self.require_playable() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.rewind();
        self.audio.seek_pos(start_index);
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    fn submit_waveform(&mut self, debounce: Option<Duration>) {
        let Some(source) = self.audio_source() else {
            return;
        };
        self.tasks.submit(
            TaskRequest::RenderWaveform {
                source,
                num_buckets: waveform::NUM_BUCKETS,
                sample_rate: self.config.audio.frequency,
                resampling: self.resample_mode(),
            },
            debounce,
        );
    }

    /// The loaded document as something an engine can play, of either kind.
    /// `None` with nothing open.
    /// The configured resampling method, decoded from its config slug. An
    /// unknown spelling -- a config written by a newer build -- falls back to
    /// the accurate default rather than failing the whole config.
    fn resample_mode(&self) -> dro_synth::resample::ResampleMode {
        dro_synth::resample::ResampleMode::from_slug(&self.config.audio.resampling)
            .unwrap_or_default()
    }

    fn audio_source(&self) -> Option<dro_synth::AudioSource> {
        match (self.editor.snapshot(), self.editor.vgm()) {
            (Some(song), _) => Some(dro_synth::AudioSource::Opl(song)),
            (None, Some(file)) => Some(dro_synth::AudioSource::Vgm(std::sync::Arc::new(
                file.clone(),
            ))),
            (None, None) => None,
        }
    }

    /// Loads the current song into the audio output if it is not already
    /// there. Cheap when nothing changed.
    fn ensure_audio(&mut self) -> Result<(), String> {
        if self.audio_revision == Some(self.editor.revision()) {
            return Ok(());
        }
        let source = self
            .audio_source()
            .ok_or_else(|| "No song is loaded.".to_owned())?;
        self.audio.load(source, &self.config.audio)?;
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

    /// The housekeeping after a crop or a cut: the usual post-edit refresh, plus
    /// the loop config, since both edits reset the markers.
    ///
    /// The whole stream was rebuilt, so everything that pointed into the old one
    /// goes back to the top: the view, and the playback start (its marker on the
    /// waveform, the readout, and the stream's own position). Row 400 of the old
    /// numbering is a different instruction now -- or no instruction at all.
    fn after_region_edit(&mut self) {
        self.scroll_to = Some(table::ScrollTo::centered(0));
        self.reset_playback_start();
        self.after_edit();
        self.push_loop_config();
    }

    /// Puts the playback start back at the beginning of the song: the waveform's
    /// start marker and cursor, the position readout, and the audio stream.
    fn reset_playback_start(&mut self) {
        self.waveform.start_ms = 0;
        self.waveform.cursor_ms = 0;
        self.position.set_position_ms(0);
        self.audio.rewind();
    }

    /// Hands the audio service the region to repeat, or `None` when looping is
    /// off. Cheap and idempotent; call it after anything that moves the markers,
    /// changes the count, or reloads the stream.
    fn push_loop_config(&mut self) {
        // The stream's real rate while one is live, else the configured one --
        // the same rule the position readout follows. `ensure_audio` re-pushes
        // once a device has negotiated its rate, so a mismatch cannot outlive
        // the next load.
        let rate = self
            .audio
            .output_rate()
            .unwrap_or(self.config.audio.frequency);
        let markers = self.editor.markers;
        let config = self
            .loop_enabled
            .then(|| match (self.editor.song(), self.editor.vgm()) {
                (Some(song), _) => Some(LoopConfig::for_song(
                    song,
                    markers.start(),
                    markers.end(),
                    self.loop_count,
                    rate,
                )),
                (None, Some(file)) => Some(LoopConfig::for_vgm(
                    file,
                    markers.start(),
                    markers.end(),
                    self.loop_count,
                    rate,
                )),
                (None, None) => None,
            });
        self.audio.set_loop(config.flatten());
    }

    fn menu_state(&self) -> MenuState {
        let on_pack_tab = self.active_tab == AppTab::Pack;
        // Undo/Redo act on whichever tab shows: the pack file-edit stacks on the
        // pack tab, the editor's song-undo stack otherwise. On the pack tab they are
        // held off while a sequence is still running.
        let (can_undo, can_redo, undo_description, redo_description) = if on_pack_tab {
            let idle = !self.pack_busy();
            (
                idle && !self.pack_undo.is_empty(),
                idle && !self.pack_redo.is_empty(),
                self.pack_undo.last().map(|txn| txn.label.clone()),
                self.pack_redo.last().map(|txn| txn.label.clone()),
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
            pack_has_peaks: self
                .pack
                .as_ref()
                .is_some_and(|pack| !pack.peaks.is_empty()),
            can_undo,
            can_redo,
            undo_description,
            redo_description,
            on_pack_tab,
            focused_row: self.editor.selection.first(),
            // Any document, not just an OPL one: cropping a Mega Drive rip is
            // the same operation, and `dro_core` carries the chip state across
            // the cut for either.
            has_marked_region: self.editor.has_document()
                && !self.editor.markers.is_full(self.editor.len()),
            // A VGM is a VGM whichever slot holds it: the format-gated items
            // (Edit Tag, VGM Metadata, Optimize, Fix Header) apply either way.
            song_type: self
                .editor
                .song()
                .map(|song| song.file_type)
                .or_else(|| self.editor.vgm().map(|_| SongFileType::Vgm)),
            is_dro_v2: self.editor.song().is_some_and(|song| {
                song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V2
            }),
            can_render: self.editor.capabilities().renderable,
            // Specifically an OPL stream, not merely something audible: the
            // channel split decides which OPL channel each register write
            // belongs to. Shown for an empty editor, like the rest of the menu.
            can_split_channels: self.editor.has_song() || !self.editor.has_document(),
        }
    }

    /// `"Play last 3 seconds"`, formatted with two
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

#[cfg(test)]
mod about_tests {
    use super::about_text;

    #[test]
    fn the_about_box_credits_every_compiled_core() {
        // The LGPL and GPL cores this program links require their notice to
        // reach the user, and the About box is where it does. Driving it from
        // `dro_synth::credits` rather than typed copy is what stops a new core
        // from shipping uncredited -- this test is that guarantee's teeth.
        //
        // Installed first so both reads below see the same registry: the GUI
        // tests install it concurrently, and text rendered from the ambient
        // fallback compared against credits read after the install would
        // disagree about cores neither is wrong about.
        crate::widgets::chip_output::install_test_cores();
        let text = about_text();
        for core in dro_synth::credits() {
            assert!(
                text.contains(&core.label),
                "{} is compiled in but not credited in the About box",
                core.label
            );
            assert!(
                text.contains(&core.license),
                "{} is credited without its license",
                core.label
            );
        }
    }

    #[test]
    fn the_about_box_states_the_binarys_license_not_a_crates() {
        // The distributed program is the GPL-licensed combination, whatever the
        // permissive halves say about themselves. Getting this backwards would
        // under-state the obligation to whoever redistributes a build.
        let text = about_text();
        assert!(text.contains("GNU General Public License"));
        assert!(
            text.contains("https://github.com/laurence-myers/dro-trimmer"),
            "GPL section 3 wants the corresponding source pointed at"
        );
        assert!(
            text.contains("MIT OR Apache-2.0"),
            "the permissive half is worth telling a would-be reuser about"
        );
    }
}
