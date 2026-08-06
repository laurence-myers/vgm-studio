//! Everything the UI can ask the application to do.
//!
//! Menus, buttons, shortcuts and dialogs all *emit* actions while the frame is
//! being drawn; the app processes the queue afterwards. A menu item, its
//! button and its keyboard shortcut all emit the same action, so they share
//! one handler.

use vgms_core::config::{AppConfig, SurfaceChoice, ThemeChoice};
use vgms_core::{Gd3Tag, OplType};
use vgms_synth::LoopCount;

use crate::pack::BulkTagOverlay;

/// Which top-level view is showing. The tab strip appears only while a pack is
/// open; otherwise the app is always on [`AppTab::Editor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Editor,
    Pack,
}

/// What the Find Register dialog is searching for: a DRO/OPL token or hex
/// register (parsed by [`FindTarget`](vgms_core::FindTarget)), or a multichip
/// target for a VGM the editor holds as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindQuery {
    Dro(String),
    Vgm(vgms_core::vgm::VgmFindTarget),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// The song file: open/save/close, quit, and the render and split exports.
    File(FileAction),

    /// Editing the document: undo/redo, deletion, conversion, the header and
    /// tag dialogs with their saves, and the find dialogs.
    Edit(EditAction),

    /// Pack mode: the VGMRips submission project and its file operations.
    Pack(PackAction),

    /// The mixer: channel toggles, panning, and the volume lever.
    Mixer(MixerAction),
    /// Transport and position: play/stop, seeking, and row navigation.
    Playback(PlaybackAction),

    /// Loop points: the marked region, loop playback, and the loop search.
    Loop(LoopAction),

    /// Settings: opening the dialog, saving it, and its live previews.
    Settings(SettingsAction),
    /// App chrome: message boxes, the status bar, and the Help menu.
    Ui(UiAction),
}

/// Editing the document: undo/redo, deletion, conversion, the header and tag
/// dialogs with their saves, and the find dialogs. Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum EditAction {
    /// Edit > Fix Header: audit the loaded VGM's header against its stream and
    /// offer to correct what disagrees.
    AuditHeader,
    /// Apply the corrections the audit found, after the user confirmed them.
    ConfirmFixHeader,
    /// Convert the loaded DRO v2 down to DRO v1.
    ConvertToDro1,
    ConvertToVgm,
    DeleteSelection,
    FindRegister {
        query: FindQuery,
        backwards: bool,
    },
    GotoSubmitted(String),
    OpenDroInfo,
    OpenEditTag,
    OpenFindRegister,
    OpenGoto,
    OpenVgmMetadata,
    /// Strip redundant register writes and merge the delays left behind (VGM only).
    OptimizeVgm,
    Redo,
    SaveGd3(Box<Gd3Tag>),
    SaveVgmMetadata {
        loop_point: Option<usize>,
        /// Exclusive; `None` for the end of the song.
        loop_end: Option<usize>,
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
    },
    Undo,
    UpdateHeader {
        opl_type: OplType,
        ms_length: u32,
    },
}

/// The song file: open/save/close, quit, and the render and split exports.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum FileAction {
    /// Close the loaded song (prompts first if it has unsaved edits).
    Close,
    /// Close the song after the user confirmed discarding it.
    ConfirmClose,
    /// Load the file stashed in `pending_load`, discarding the current song.
    ConfirmDiscardAndLoad,
    /// Quit past the unsaved-changes prompt (sets the quitting flag, closes).
    ConfirmExit,
    Exit,
    /// Open the file picker for a song.
    Open,
    /// Open the Render to WAV options dialog.
    OpenRenderWav,
    /// Open the Split Channels options dialog.
    OpenSplit,
    /// Open the Split Songs dialog (a long capture into its per-song files).
    OpenSplitSongs,
    /// Render the song to a WAV with the chosen options applied, then offer to
    /// save it. `boost` is already resolved to `1.0` when it was switched off.
    RenderWavSubmitted {
        use_toggles: bool,
        use_panning: bool,
        boost: f32,
        /// The per-render core choices the dialog's picker settled on, seeded
        /// from Settings and never persisted. Empty means the configured cores.
        core_choices: std::collections::BTreeMap<String, String>,
    },
    Save,
    SaveAs,
    /// Preview a detected song by seeking playback to its first instruction and
    /// playing from there.
    SplitSongsPreview {
        start_index: usize,
    },
    /// Split the capture into per-song files at the silent gaps, once a folder is
    /// chosen. `threshold_native` and `trailing_tail` are in the song's native
    /// delay unit (samples for a VGM, milliseconds for a DRO); `included` is one
    /// flag per detected segment, in detection order, so the user can drop false
    /// positives before exporting.
    SplitSongsSubmitted {
        threshold_native: u32,
        included: Vec<bool>,
        trailing_tail: u32,
    },
    /// Split the song into one file per channel used, once a folder is chosen.
    SplitSubmitted {
        format: vgms_synth::SplitFormat,
        /// Exclude the channels the mixer has muted from the output set
        /// (decision 9).
        use_skip_muted: bool,
        /// Apply the mixer's pan knobs to each rendered stem (WAV only).
        use_panning: bool,
        /// The boost applied to each rendered WAV stem; `1.0` when off.
        boost: f32,
        /// The per-render core choices the dialog's picker settled on, seeded
        /// from Settings and never persisted. Empty means the configured cores.
        core_choices: std::collections::BTreeMap<String, String>,
    },
}

/// Loop points: the marked region, loop playback, and the loop search.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopAction {
    /// Write the marked region into the song's VGM loop metadata.
    ApplyToMetadata,
    /// Cancel a running loop search.
    CancelSearch,
    /// Reset the marked region to the whole song.
    ClearMarkers,
    /// Keep only the marked region, deleting everything outside it.
    CropToMarkers,
    /// Delete the marked region, keeping everything outside it.
    DeleteMarkedRegion,
    /// Open the Find Loop dialog (search for loop points). Works for a DRO or a
    /// VGM; only the Apply button inside it is VGM-only.
    OpenSearch,
    /// Search the command stream for loop candidates at least `min_len_commands`
    /// delay-stripped commands long (the Find Loop dialog's Search button).
    Search { min_len_commands: usize },
    /// Change how many times the marked region repeats.
    SetCount(LoopCount),
    /// Mark the loop end at an instruction index (exclusive).
    SetEnd(usize),
    /// Mark the loop start at an instruction index.
    SetStart(usize),
    /// Turn loop playback on or off. Takes effect immediately, mid-playback.
    TogglePlayback,
}

/// The mixer: channel toggles, panning, and the volume lever.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum MixerAction {
    /// The volume lever's "Match" button: measure the song's peak in the
    /// background, then set the volume to bring it to full scale.
    MatchVolume,
    /// The VGM metadata dialog's "Measure" button: measure the song's peak in the
    /// background, then fill the volume-modifier field with a suggestion.
    MeasureVolumeModifier,
    MutingChanged,
    /// A pan knob or the Original/Custom mode toggle moved.
    PanningChanged,
    /// The boost slider moved. `persist` is set once the interaction ends,
    /// so vgmstudio.ini is written once per adjustment, not per frame.
    SetBoost {
        value: f32,
        persist: bool,
    },
    /// The volume lever's "Lock" toggle: `true` keeps the volume across songs
    /// (and persists it); `false` lets each song set its own from its header
    /// modifier.
    SetLockBoost(bool),
    ToggleChannel(usize),
    /// Reported each frame by the volume field: whether it currently holds
    /// keyboard focus. While it does, the editor's key shortcuts stand down so a
    /// typed number edits the volume instead of toggling a channel.
    VolumeFieldFocused(bool),
}

/// Pack mode: the VGMRips submission project and its file operations.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum PackAction {
    /// Pick a screenshot and copy it into the open pack's folder.
    AddScreenshot,
    /// Copy a picked screenshot into the pack folder under the name the naming
    /// dialog settled on. It carries the bytes because nothing was written while
    /// the dialog was open -- cancelling it leaves the folder untouched.
    AddScreenshotAs {
        file_name: String,
        bytes: Vec<u8>,
        /// Losslessly recompress before writing, so the file lands optimal
        /// rather than being rewritten a moment later.
        recompress: bool,
    },
    /// Set each track's VGM volume modifier from the scanned peaks, as one
    /// undoable batch of header rewrites. `album` levels the pack by its loudest
    /// track; otherwise each track is normalised to its own peak. The pad passes
    /// the Album latch; the menu items say which they mean.
    ApplySuggestedModifiers { album: bool },
    /// Apply a bulk GD3 edit: overlay the checked fields onto each target track's
    /// existing tag and rewrite the files as one undoable batch. Targets are file
    /// names (the stable identity re-resolved against the current list).
    BulkTagSubmitted {
        targets: Vec<String>,
        overlay: Box<BulkTagOverlay>,
    },
    /// Close the pack project (prompts first if it has unsaved edits).
    Close,
    /// Close the pack project after the user confirmed discarding it.
    ConfirmClose,
    /// Delete the named screenshot after the user confirmed it. Carries the file
    /// name, not the index: a rescan between the prompt and the answer can
    /// reorder the list.
    ConfirmDeleteScreenshot(String),
    /// Build the release zip after the user accepted the warnings.
    ConfirmExportZip,
    /// Open the folder picker after the user confirmed discarding a dirty pack.
    ConfirmOpenFolder,
    /// Open the `.zip` held in `pending_zip` as a pack, after the user confirmed
    /// discarding the current pack's unsaved edits (wt-8).
    ConfirmOpenZip,
    /// Rewrite every slash-separated release date (pack meta and each track's
    /// GD3) to hyphens -- the checklist's one-click date fix-assist.
    ConvertDatesToHyphens,
    /// Ask whether to delete the screenshot at this index.
    DeleteScreenshot(usize),
    /// Build the release zip and save it (prompts first on soft warnings).
    ExportZip,
    /// Make a track the row the keyboard acts on (a click on it).
    FocusTrack(usize),
    /// Move the focused track one slot (`-1` up, `+1` down): Alt+arrow. The focus
    /// travels with it, so the keys can be pressed again straight away.
    MoveFocusedTrack { delta: isize },
    /// Move a track up (`-1`) or down (`+1`) one slot, renumbering the files.
    MoveTrack { index: usize, delta: isize },
    /// Move a track to a position, renumbering the files: what dropping a dragged
    /// row emits. `to` is the destination index in the final list.
    MoveTrackTo { from: usize, to: usize },
    /// Open the bulk GD3 tag editor (chosen fields written to chosen tracks).
    OpenBulkTag,
    /// Open the folder picker for a pack project (prompts first if the current
    /// pack has unsaved edits).
    OpenFolder,
    /// Open a specific folder as a pack project (the Split Songs completion offer).
    OpenFolderAt(std::path::PathBuf),
    /// Open the quick-edit dialog (rename + GD3) for a track.
    OpenTrackQuickEdit(usize),
    /// Open the file picker for a `.zip` to edit as an in-memory pack (wt-8).
    /// The picked file arrives on the ordinary picked-file channel, so the
    /// discard-changes prompt is the same one a dropped `.zip` raises.
    OpenZip,
    /// Apply a quick edit: rewrite the file's GD3 tag and, if changed, its name.
    QuickEditSubmitted {
        original_name: String,
        file_name: String,
        tag: Box<Gd3Tag>,
    },
    /// Losslessly recompress a screenshot and save it in place.
    RecompressImage(usize),
    /// Rename every track file that has drifted from its GD3 Track Name to
    /// `NN Title.ext` (`vgm_ren`'s rules), as one undoable batch of renames.
    RenameFromTags,
    /// Rename a screenshot, as one undoable step. Carries the name the dialog
    /// opened on, not an index: a rescan in between can reorder the list.
    RenameScreenshot {
        original_name: String,
        file_name: String,
    },
    /// Open the rename dialog on the screenshot at this index.
    RenameScreenshotAt(usize),
    /// Pick a screenshot to overwrite the one at this index.
    ReplaceScreenshot(usize),
    /// Save a memory-backed (zip-opened) pack: re-export the archive, in place to
    /// its source `.zip` on native, or as a download on the web (wt-8).
    SaveArchive,
    /// Write the generated Game Name.txt and Game Name.m3u into the folder.
    SaveDocs,
    /// Measure every pack track's peak in the background, for the Peak column.
    ScanVolumes,
    /// Show a pack sub-section (the section tabs, and the deck's verdict link).
    SelectSection(crate::pack::PackSection),
    /// Switch the active tab (only meaningful while a pack is open).
    SelectTab(AppTab),
    /// Stop the track preview.
    StopPreview,
    /// Open a track in the editor (double-click / button in the track list).
    TrackOpen(usize),
    /// Preview a track through the audio output.
    TrackPreview(usize),
}

/// Transport and position: play/stop, seeking, and moving through the rows.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackAction {
    NextDelay,
    Play,
    /// Play the loop join: `tail_length` ms before the loop end, looping, so the
    /// seam can be heard on its own.
    PlaySeam,
    PlayTail,
    PreviousDelay,
    /// Jump the play position back to the very start.
    RewindToStart,
    /// Arrow-key selection movement; `extend` ranges with Shift.
    SelectionMove {
        delta: isize,
        extend: bool,
    },
    Stop,
    TogglePlayback,
    /// The waveform was clicked at an instruction.
    WaveformClicked {
        index: usize,
        ms: u32,
    },
}

/// Settings: opening the dialog, saving it, and the live previews it drives.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    Apply(Box<AppConfig>),
    /// Open the Settings dialog.
    Open,
    /// Audition a core map without saving it: the registry choices are
    /// replaced and the loaded stream rebuilt in place at its position, so a
    /// core picked in Settings is heard at once. Closing the dialog re-emits
    /// the saved map, which reverts.
    PreviewCores(std::collections::BTreeMap<String, String>),
    /// Audition a resampling mode (the `sinc`/`linear` slug) without saving it:
    /// the loaded stream is rebuilt in place with it, so a mode picked in
    /// Settings is heard at once. Closing re-emits the saved mode, which reverts.
    PreviewResampling(String),
    /// Repaint in a different skin without saving it, so the Settings dropdowns
    /// show what they mean on the real UI rather than on a description of it.
    /// Closing the dialog re-emits the settings it opened with, which reverts.
    PreviewSkin {
        theme: ThemeChoice,
        pad_style: SurfaceChoice,
        deck_style: SurfaceChoice,
    },
}

/// App chrome: message boxes, the status bar, and the Help menu's dialogs.
/// Variants are alphabetical.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    About,
    /// Queue a modal message box.
    Alert {
        title: String,
        message: String,
    },
    Help,
    /// Set the status-bar text.
    Status(String),
}
