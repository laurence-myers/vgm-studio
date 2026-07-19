//! Everything the UI can ask the application to do.
//!
//! Menus, buttons, shortcuts and dialogs all *emit* actions while the frame is
//! being drawn; the app processes the queue afterwards. This replaces wx's
//! event/id indirection (`gui_id`) and keeps the "menu item, button and
//! accelerator share one handler" aliasing the Python relied on.

use dro_core::config::AppConfig;
use dro_core::{Gd3Tag, OplType};

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
    DeleteSelection,

    // Rip mode
    /// Open the folder picker for a rip project (prompts first if the current
    /// rip has unsaved edits).
    OpenRipFolder,
    /// Open the folder picker after the user confirmed discarding a dirty rip.
    ConfirmOpenRipFolder,
    /// Switch the active tab (only meaningful while a rip is open).
    SelectTab(AppTab),
    /// Close the rip project (prompts first if it has unsaved edits).
    CloseRip,
    /// Close the rip project after the user confirmed discarding it.
    ConfirmCloseRip,
    /// Write the generated Game Name.txt and Game Name.m3u into the folder.
    RipSaveDocs,
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
    RipMoveTrack { index: usize, delta: isize },
    /// Losslessly recompress a screenshot and save it in place.
    OptimizeImage(usize),
    /// Apply a quick edit: rewrite the file's GD3 tag and, if changed, its name.
    QuickEditSubmitted {
        original_name: String,
        file_name: String,
        tag: Box<Gd3Tag>,
    },

    // Help
    Help,
    About,

    // Playback
    Play,
    Stop,
    PlayTail,
    TogglePlayback,
    /// Jump the play position back to the very start.
    RewindToStart,
    /// The boost slider moved. `persist` is set once the interaction ends,
    /// so drotrim.ini is written once per adjustment, not per frame.
    SetBoost {
        value: f32,
        persist: bool,
    },

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
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
    },
    ApplySettings(Box<AppConfig>),
}
