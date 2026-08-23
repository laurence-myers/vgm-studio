//! The application, as one
//! `eframe::App` driven entirely through the platform-service traits.

use core::fmt;
use core::time::Duration;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use egui::Key;
use vgms_core::config::{AppConfig, ConfigStore, SurfaceChoice, ThemeChoice};
use vgms_core::song::{DRO_FILE_V2, SongFileType};
use vgms_core::{FindTarget, Gd3Tag};
use vgms_synth::{
    ChipMuting, ChipPanning, ChipTrims, LoopConfig, LoopCount, Muting, SplitFormat, VgmRenderMix,
    VgmSplitOptions,
};

use crate::action::{
    Action, AppTab, EditAction, FileAction, LoopAction, MixerAction, PackAction, PlaybackAction,
    SettingsAction, UiAction,
};
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
    boost_stepper, chip_panels::ChipPanels, loop_stepper, peak_meter, table, waveform,
};

/// The About box: who wrote it, and -- because this program links copyleft
/// emulator cores -- what it is licensed under and where each core came from.
///
/// The core table is generated from [`vgms_synth::credits_text`] rather than
/// typed here, so a core cannot be linked in without being credited.
fn about_text() -> String {
    crate::strings::app_about_text(
        env!("CARGO_PKG_VERSION"),
        vgms_synth::credits_text(),
        crate::optimize::credit(),
    )
}

/// What a click on the waveform means, given the button and whether Shift was
/// held. `None` for a gesture that does nothing.
///
/// Shift brackets the loop -- left marks the start, right the end. The end is
/// the *time* clicked, hence that instruction's index taken exclusively:
/// everything sounding before the click is inside the loop.
fn waveform_action(index: usize, ms: u32, secondary: bool, shift: bool) -> Option<Action> {
    match (shift, secondary) {
        (true, false) => Some(Action::Loop(LoopAction::SetStart(index))),
        (true, true) => Some(Action::Loop(LoopAction::SetEnd(index))),
        // A plain right-click marks nothing; seeking is the left button's job.
        (false, true) => None,
        (false, false) => Some(Action::Playback(PlaybackAction::WaveformClicked {
            index,
            ms,
        })),
    }
}

/// How a multichip find target reads in the status line.
fn describe_target(target: vgms_core::vgm::VgmFindTarget) -> String {
    use vgms_core::vgm::VgmFindTarget;
    match target {
        VgmFindTarget::AnyDelay => crate::strings::APP_TARGET_ANY_DELAY.to_owned(),
        VgmFindTarget::Write {
            kind,
            instance,
            addr,
        } => {
            let inst = instance
                .filter(|&i| i > 0)
                .map_or_else(String::new, |i| format!(" #{}", i + 1));
            match addr {
                Some(addr) => format!("{}{inst} {addr:#06X}", kind.name()),
                None => crate::strings::app_target_write(kind.name(), &inst),
            }
        }
    }
}

/// How a DRO find target reads in the status line -- the OPL counterpart of
/// [`describe_target`]. The picker only ever builds a register, "any write" or
/// "any delay"; the remaining variants are covered so the match stays total.
fn describe_dro_target(target: FindTarget) -> String {
    match target {
        FindTarget::Register(reg) => format!("register {reg:#04X}"),
        FindTarget::AnyRegister => crate::strings::APP_TARGET_ANY_WRITE.to_owned(),
        FindTarget::AnyDelay | FindTarget::ShortDelay | FindTarget::LongDelay => {
            crate::strings::APP_TARGET_ANY_DELAY.to_owned()
        }
        FindTarget::BankSwitch => crate::strings::APP_TARGET_BANK_SWITCH.to_owned(),
    }
}

fn mismatch_alert(auto_trimmed: bool, file_version: u32) -> Alert {
    let prefix = if auto_trimmed {
        crate::strings::APP_MISMATCH_PREFIX_TRIMMED
    } else {
        crate::strings::APP_MISMATCH_PREFIX_PLAIN
    };
    let advice = if file_version == vgms_core::song::DRO_FILE_V1 {
        crate::strings::APP_MISMATCH_ADVICE_V1
    } else {
        crate::strings::APP_MISMATCH_ADVICE_V2
    };
    Alert::new(
        crate::strings::APP_MISMATCH_TITLE,
        crate::strings::app_mismatch_body(prefix, advice),
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
    /// A screenshot copied into the pack folder. Unlike a replace, there is no
    /// previous file whose bytes could serve as an inverse, so it rescans
    /// without touching the undo stack.
    ScreenshotAdded,
    /// A screenshot rewritten in place -- a recompress, or a replace. Both hold
    /// the old bytes, so both land a reversible transaction.
    ImageWritten,
    /// The exported release zip (a Save-As dialog).
    ExportZip,
    /// Save Pack: re-exporting a memory-backed (zip) pack. On success the pack's
    /// dirty flag clears; on cancel it stays, so nothing is silently lost (wt-8).
    SaveArchive,
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
        format: SplitFormat,
        boost: f32,
        core_choices: std::collections::BTreeMap<String, String>,
        /// Apply the mixer's pans to each stem -- resolved against the live
        /// mixer once the document kind is known (`split_into`).
        use_panning: bool,
        /// Skip the mixer's muted channels -- likewise resolved per kind.
        use_skip_muted: bool,
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

/// Whether a name is a `.zip` (a pack archive, not a song) -- wt-8.
fn is_zip_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}

/// Whether a folder path is an in-memory zip pack's synthetic token
/// (`/vgms-zip-N`), the marker every file service mints for one -- wt-8.
fn folder_is_archive(path: Option<&Path>) -> bool {
    path.and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("vgms-zip-"))
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

/// What a background volume scan's [`Peak`](vgms_synth::Peak) is for, so the app
/// routes the result to the control that asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VolumeScanPurpose {
    /// The transport "Match" button: set the playback volume lever.
    MatchBoost,
    /// The VGM dialog "Measure" button: fill the volume-modifier field.
    FillModifier,
}

pub struct VgmStudioApp {
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
    /// frame, reported by the lever via [`MixerAction::VolumeFieldFocused`]. While it
    /// does, [`Self::gather_key_input`] stands the editor shortcuts down so typed
    /// numbers edit the value instead of toggling channels.
    volume_field_editing: bool,
    position: PositionPanel,
    channels: ChipPanels,
    /// The status text as of the last frame, so the status bar can flash when
    /// it changes. Display-only state; `status` itself stays the truth.
    status_shown: String,
    /// How far the pack volume scan has got (`done`, `total`), shown in the
    /// status bar while it runs and cleared when it finishes or is cancelled.
    pack_scan_progress: Option<(usize, usize)>,
    /// Whether the selected chip's control panel is unfolded below the chip
    /// strip. Folded by default; the strip (lamps, trims, tabs) always shows,
    /// and folding hides only the controls: mutes, pans and trims keep
    /// applying, and the number keys still toggle the selected chip's channels.
    chips_expanded: bool,

    /// A row the table should scroll into view next frame.
    scroll_to: Option<table::ScrollTo>,
    /// The last first-selected row, to detect selection changes.
    last_first_selected: Option<usize>,
    /// The editor revision currently loaded into the audio service, if any.
    audio_revision: Option<u64>,
    /// Whether playback repeats the marked region. Off by default: Play means
    /// "play the song" until asked otherwise.
    loop_enabled: bool,
    /// How many times the region repeats while looping. The user's chosen
    /// target, shown by the stepper.
    loop_count: LoopCount,
    /// The count the engine actually plays: `loop_count` after the file's own
    /// loop base/modifier scale it (`GetModifiedLoopCount`). Shown by the
    /// progress readout, so it agrees with what is heard. Equal to `loop_count`
    /// for a user-drawn region or a DRO, which are not rescaled.
    loop_total: LoopCount,
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
    /// `(theme, pad_style)`. `None` whenever the window is painted in the saved
    /// settings. See [`Self::preview_skin`].
    skin_preview: Option<(ThemeChoice, SurfaceChoice)>,

    /// A `.zip` held behind the discard-changes prompt when a dirty pack is open.
    pending_zip: Option<PickedFile>,
    /// Set while a Save Pack export is in flight, so its delivered outcome clears
    /// the memory pack's dirty flag rather than reading as a plain Export Zip.
    pack_saving_archive: bool,

    /// Actions injected by the e2e hook, drained into the per-frame queue at the
    /// top of [`Self::update_impl`] so they run through the ordinary handler with
    /// a live `Context`. Present only in test / `e2e` builds; see
    /// [`Self::e2e_enqueue_action`].
    #[cfg(any(test, feature = "e2e"))]
    e2e_actions: VecDeque<Action>,
}

// The `impl VgmStudioApp` is split across `src/app/*.rs` by concern; each child
// file re-opens the impl and reaches the shared imports through `use super::*`.
// (The gui tests keep their own `#[path]` mount below, at `src/app_gui_tests/`.)
mod actions;
mod audio;
mod frame;
mod guards;
mod pack;
mod playback;
mod settings;
mod setup;
mod split;
mod workflows;

/// A read-only view of the state the web e2e specs assert on, produced by
/// [`VgmStudioApp::e2e_snapshot`]. Plain owned data with no serde: the web hook
/// reflects it into a JS object by hand (`vgms-web` has no serde in its tree).
/// Test / `e2e` builds only.
#[cfg(any(test, feature = "e2e"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eSnapshot {
    pub has_document: bool,
    pub document_name: Option<String>,
    pub row_count: usize,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub playing: bool,
    pub status: String,
    /// `"editor"` or `"pack"`.
    pub active_tab: &'static str,
    /// The front alert's message, if a modal alert is up.
    pub alert: Option<String>,
    /// Whether any dialog window is open.
    pub dialog_open: bool,
    /// The open pack project, if any.
    pub pack: Option<E2ePackSnapshot>,
}

/// The pack half of [`E2eSnapshot`]. Test / `e2e` builds only.
#[cfg(any(test, feature = "e2e"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2ePackSnapshot {
    pub name: String,
    pub dirty: bool,
    /// Track file names, in list order.
    pub track_names: Vec<String>,
    /// Screenshot file names, in list order.
    pub image_names: Vec<String>,
}

impl eframe::App for VgmStudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.update_impl(ui);
    }

    fn on_exit(&mut self) {
        self.audio.unload();
        self.tasks.shutdown();
    }
}

// The headless GUI tests live in their own file but mount here, as a child
// module of `app`, so they can read `VgmStudioApp`'s private fields directly.
#[cfg(test)]
#[path = "app_gui_tests/mod.rs"]
mod gui_tests;

impl fmt::Debug for VgmStudioApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VgmStudioApp")
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
        // The About box must credit every linked LGPL/GPL core; driving it from
        // `vgms_synth::credits` rather than typed copy is what stops a new core
        // from shipping uncredited.
        //
        // Installed first so both reads below see the same registry: the GUI
        // tests install it concurrently, and comparing text from the ambient
        // fallback against credits read after the install would disagree.
        crate::widgets::chip_output::install_test_cores();
        let text = about_text();
        for core in vgms_synth::credits() {
            assert!(
                text.contains(&core.label),
                "{} is compiled in but not credited in the About box",
                core.label
            );
            assert!(
                text.contains(vgms_synth::short_license(&core.license)),
                "{} is credited without its license",
                core.label
            );
        }
    }

    #[test]
    fn the_about_box_states_the_binarys_license_not_a_crates() {
        // The distributed binary is the GPL-licensed combination, whatever the
        // permissive halves say about themselves.
        let text = about_text();
        assert!(text.contains("GNU General Public License"));
        assert!(
            text.contains("https://github.com/laurence-myers/vgm-studio"),
            "GPL section 3 wants the corresponding source pointed at"
        );
        assert!(
            text.contains("MIT OR Apache-2.0"),
            "the permissive half is worth telling a would-be reuser about"
        );
    }
}
