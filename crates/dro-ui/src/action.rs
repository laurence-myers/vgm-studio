//! Everything the UI can ask the application to do.
//!
//! Menus, buttons, shortcuts and dialogs all *emit* actions while the frame is
//! being drawn; the app processes the queue afterwards. A menu item, its
//! button and its keyboard shortcut all emit the same action, so they share
//! one handler.

use dro_core::config::AppConfig;
use dro_core::{Gd3Tag, OplType};
use dro_synth::LoopCount;

use crate::rip::BulkTagOverlay;

/// Which top-level view is showing. The tab strip appears only while a rip is
/// open; otherwise the app is always on [`AppTab::Editor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppTab {
    Editor,
    Rip,
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
    OpenDroInfo,
    OpenEditTag,
    OpenVgmMetadata,
    ConvertToVgm,
    /// Convert the loaded DRO v2 down to DRO v1.
    ConvertToDro1,
    DeleteSelection,
    /// Strip redundant OPL writes and merge the delays left behind (VGM only).
    OptimizeVgm,

    // Rip mode
    /// Open the folder picker for a rip project (prompts first if the current
    /// rip has unsaved edits).
    OpenRipFolder,
    /// Open the folder picker after the user confirmed discarding a dirty rip.
    ConfirmOpenRipFolder,
    /// Open a specific folder as a rip project (the Split Songs completion offer).
    OpenRipFolderAt(std::path::PathBuf),
    /// Switch the active tab (only meaningful while a rip is open).
    SelectTab(AppTab),
    /// Close the rip project (prompts first if it has unsaved edits).
    CloseRip,
    /// Close the rip project after the user confirmed discarding it.
    ConfirmCloseRip,
    /// Write the generated Game Name.txt and Game Name.m3u into the folder.
    RipSaveDocs,
    /// Measure every rip track's peak in the background, for the Peak column.
    RipScanVolumes,
    /// Set each track's VGM volume modifier from the scanned peaks (album or
    /// per-track), as one undoable batch of header rewrites.
    RipApplySuggestedModifiers,
    /// Build the release zip and save it (prompts first on soft warnings).
    RipExportZip,
    /// Build the release zip after the user accepted the warnings.
    ConfirmExportZip,
    /// Open a track in the editor (double-click / button in the track list).
    RipTrackOpen(usize),
    /// Preview a track through the audio output.
    RipTrackPreview(usize),
    /// Stop the track preview.
    RipStopPreview,
    /// Open the quick-edit dialog (rename + GD3) for a track.
    OpenTrackQuickEdit(usize),
    /// Move a track up (`-1`) or down (`+1`) one slot, renumbering the files.
    RipMoveTrack {
        index: usize,
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
}
