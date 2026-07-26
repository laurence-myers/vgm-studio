//! Everything the UI can ask the application to do.
//!
//! Menus, buttons, shortcuts and dialogs all *emit* actions while the frame is
//! being drawn; the app processes the queue afterwards. A menu item, its
//! button and its keyboard shortcut all emit the same action, so they share
//! one handler.

use dro_core::config::{AppConfig, SurfaceChoice, ThemeChoice};
use dro_core::{Gd3Tag, OplType};
use dro_synth::LoopCount;

use crate::pack::BulkTagOverlay;

/// Which top-level view is showing. The tab strip appears only while a pack is
/// open; otherwise the app is always on [`AppTab::Editor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Editor,
    Pack,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // File
    OpenFile,
    Save,
    SaveAs,
    /// Open the Render to WAV options dialog.
    OpenRenderWav,
    /// Open the Split Channels options dialog.
    OpenSplit,
    /// Split the song into one file per channel used, once a folder is chosen.
    SplitSubmitted {
        format: dro_synth::SplitFormat,
        isolate_percussion: bool,
    },
    /// Open the Split Songs dialog (a long capture into its per-song files).
    OpenSplitSongs,
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
    /// Preview a detected song by seeking playback to its first instruction and
    /// playing from there.
    SplitSongsPreview {
        start_index: usize,
    },
    /// Render the song to a WAV with the chosen options applied, then offer to
    /// save it. `boost` is already resolved to `1.0` when it was switched off.
    RenderWavSubmitted {
        use_toggles: bool,
        use_panning: bool,
        boost: f32,
    },
    OpenSettings,
    Exit,
    /// Quit past the unsaved-changes prompt (sets the quitting flag, closes).
    ConfirmExit,
    /// Load the file stashed in `pending_load`, discarding the current song.
    ConfirmDiscardAndLoad,

    // Edit
    Undo,
    Redo,
    OpenGoto,
    OpenFindRegister,
    /// Open the Find Loop dialog (search for loop points). Works for a DRO or a
    /// VGM; only the Apply button inside it is VGM-only.
    OpenFindLoop,
    OpenDroInfo,
    OpenEditTag,
    OpenVgmMetadata,
    ConvertToVgm,
    /// Convert the loaded DRO v2 down to DRO v1.
    ConvertToDro1,
    DeleteSelection,
    /// Strip redundant OPL writes and merge the delays left behind (VGM only).
    OptimizeVgm,

    // Pack mode
    /// Open the folder picker for a pack project (prompts first if the current
    /// pack has unsaved edits).
    OpenPackFolder,
    /// Open the folder picker after the user confirmed discarding a dirty pack.
    ConfirmOpenPackFolder,
    /// Open a specific folder as a pack project (the Split Songs completion offer).
    OpenPackFolderAt(std::path::PathBuf),
    /// Switch the active tab (only meaningful while a pack is open).
    SelectTab(AppTab),
    /// Close the pack project (prompts first if it has unsaved edits).
    ClosePack,
    /// Close the pack project after the user confirmed discarding it.
    ConfirmClosePack,
    /// Write the generated Game Name.txt and Game Name.m3u into the folder.
    PackSaveDocs,
    /// Measure every pack track's peak in the background, for the Peak column.
    PackScanVolumes,
    /// Set each track's VGM volume modifier from the scanned peaks, as one
    /// undoable batch of header rewrites. `album` levels the pack by its loudest
    /// track; otherwise each track is normalised to its own peak. The pad passes
    /// the Album latch; the menu items say which they mean.
    PackApplySuggestedModifiers {
        album: bool,
    },
    /// Rewrite every slash-separated release date (pack meta and each track's
    /// GD3) to hyphens -- the checklist's one-click date fix-assist.
    PackConvertDatesToHyphens,
    /// Rename every track file that has drifted from its GD3 Track Name to
    /// `NN Title.ext` (`vgm_ren`'s rules), as one undoable batch of renames.
    PackRenameFromTags,
    /// Show a pack sub-section (the section tabs, and the deck's verdict link).
    PackSelectSection(crate::pack::PackSection),
    /// Pick a screenshot and copy it into the open pack's folder.
    PackAddScreenshot,
    /// Pick a screenshot to overwrite the one at this index.
    PackReplaceScreenshot(usize),
    /// Open the rename dialog on the screenshot at this index.
    PackRenameScreenshotAt(usize),
    /// Rename a screenshot, as one undoable step. Carries the name the dialog
    /// opened on, not an index: a rescan in between can reorder the list.
    PackRenameScreenshot {
        original_name: String,
        file_name: String,
    },
    /// Copy a picked screenshot into the pack folder under the name the naming
    /// dialog settled on. It carries the bytes because nothing was written while
    /// the dialog was open -- cancelling it leaves the folder untouched.
    PackAddScreenshotAs {
        file_name: String,
        bytes: Vec<u8>,
        /// Losslessly recompress before writing, so the file lands optimal
        /// rather than being rewritten a moment later.
        recompress: bool,
    },
    /// Ask whether to delete the screenshot at this index.
    PackDeleteScreenshot(usize),
    /// Delete the named screenshot after the user confirmed it. Carries the file
    /// name, not the index: a rescan between the prompt and the answer can
    /// reorder the list.
    ConfirmDeleteScreenshot(String),
    /// Build the release zip and save it (prompts first on soft warnings).
    PackExportZip,
    /// Build the release zip after the user accepted the warnings.
    ConfirmExportZip,
    /// Open a track in the editor (double-click / button in the track list).
    PackTrackOpen(usize),
    /// Preview a track through the audio output.
    PackTrackPreview(usize),
    /// Stop the track preview.
    PackStopPreview,
    /// Open the quick-edit dialog (rename + GD3) for a track.
    OpenTrackQuickEdit(usize),
    /// Move a track up (`-1`) or down (`+1`) one slot, renumbering the files.
    PackMoveTrack {
        index: usize,
        delta: isize,
    },
    /// Move a track to a position, renumbering the files: what dropping a dragged
    /// row emits. `to` is the destination index in the final list.
    PackMoveTrackTo {
        from: usize,
        to: usize,
    },
    /// Make a track the row the keyboard acts on (a click on it).
    PackFocusTrack(usize),
    /// Move the focused track one slot (`-1` up, `+1` down): Alt+arrow. The focus
    /// travels with it, so the keys can be pressed again straight away.
    PackMoveFocusedTrack {
        delta: isize,
    },
    /// Losslessly recompress a screenshot and save it in place.
    OptimizeImage(usize),
    /// Apply a quick edit: rewrite the file's GD3 tag and, if changed, its name.
    QuickEditSubmitted {
        original_name: String,
        file_name: String,
        tag: Box<Gd3Tag>,
    },
    /// Open the bulk GD3 tag editor (chosen fields written to chosen tracks).
    OpenBulkTag,
    /// Apply a bulk GD3 edit: overlay the checked fields onto each target track's
    /// existing tag and rewrite the files as one undoable batch. Targets are file
    /// names (the stable identity re-resolved against the current list).
    BulkTagSubmitted {
        targets: Vec<String>,
        overlay: Box<BulkTagOverlay>,
    },

    // Help
    Help,
    About,

    // Playback
    Play,
    Stop,
    PlayTail,
    /// Play the loop join: `tail_length` ms before the loop end, looping, so the
    /// seam can be heard on its own.
    PlaySeam,
    TogglePlayback,
    /// Jump the play position back to the very start.
    RewindToStart,
    /// The boost slider moved. `persist` is set once the interaction ends,
    /// so drotrim.ini is written once per adjustment, not per frame.
    SetBoost {
        value: f32,
        persist: bool,
    },
    /// The volume lever's "Match" button: measure the song's peak in the
    /// background, then set the volume to bring it to full scale.
    MatchVolume,
    /// The volume lever's "Lock" toggle: `true` keeps the volume across songs
    /// (and persists it); `false` lets each song set its own from its header
    /// modifier.
    SetLockBoost(bool),
    /// The VGM metadata dialog's "Measure" button: measure the song's peak in the
    /// background, then fill the volume-modifier field with a suggestion.
    MeasureVolumeModifier,
    /// Reported each frame by the volume field: whether it currently holds
    /// keyboard focus. While it does, the editor's key shortcuts stand down so a
    /// typed number edits the volume instead of toggling a channel.
    VolumeFieldFocused(bool),

    // Loop points
    /// Mark the loop start at an instruction index.
    SetLoopStart(usize),
    /// Mark the loop end at an instruction index (exclusive).
    SetLoopEnd(usize),
    /// Reset the marked region to the whole song.
    ClearLoopMarkers,
    /// Turn loop playback on or off. Takes effect immediately, mid-playback.
    ToggleLoopPlayback,
    /// Change how many times the marked region repeats.
    SetLoopCount(LoopCount),
    /// Write the marked region into the song's VGM loop metadata.
    ApplyLoopToMetadata,
    /// Keep only the marked region, deleting everything outside it.
    CropToMarkers,
    /// Delete the marked region, keeping everything outside it.
    DeleteMarkedRegion,
    /// Search the command stream for loop candidates at least `min_len_commands`
    /// delay-stripped commands long (the Find Loop dialog's Search button).
    FindLoopSearch {
        min_len_commands: usize,
    },
    /// Cancel a running loop search.
    CancelLoopSearch,

    // Table navigation
    NextDelay,
    PreviousDelay,
    /// Arrow-key selection movement; `extend` ranges with Shift.
    SelectionMove {
        delta: isize,
        extend: bool,
    },
    /// The waveform was clicked at an instruction.
    WaveformClicked {
        index: usize,
        ms: u32,
    },

    // Channel soloing
    ToggleChannel(usize),
    MutingChanged,
    /// A pan knob or the Original/Custom mode toggle moved.
    PanningChanged,

    // Dialog submissions
    /// Queue a modal message box.
    Alert {
        title: String,
        message: String,
    },
    /// Set the status-bar text.
    Status(String),
    GotoSubmitted(String),
    FindRegister {
        target: String,
        backwards: bool,
    },
    UpdateHeader {
        opl_type: OplType,
        ms_length: u32,
    },
    SaveGd3(Box<Gd3Tag>),
    SaveVgmMetadata {
        loop_point: Option<usize>,
        /// Exclusive; `None` for the end of the song.
        loop_end: Option<usize>,
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
    },
    ApplySettings(Box<AppConfig>),
    /// Repaint in a different skin without saving it, so the Settings dropdowns
    /// show what they mean on the real UI rather than on a description of it.
    /// Closing the dialog re-emits the settings it opened with, which reverts.
    PreviewSkin {
        theme: ThemeChoice,
        pad_style: SurfaceChoice,
        deck_style: SurfaceChoice,
    },
}
