//! Everything the UI can ask the application to do.
//!
//! Menus, buttons, shortcuts and dialogs all *emit* actions while the frame is
//! being drawn; the app processes the queue afterwards. This replaces wx's
//! event/id indirection (`gui_id`) and keeps the "menu item, button and
//! accelerator share one handler" aliasing the Python relied on.

use dro_core::config::AppConfig;
use dro_core::{Gd3Tag, OplType};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // File
    OpenFile,
    Save,
    SaveAs,
    OpenSettings,
    Exit,

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

    // Help
    Help,
    About,

    // Playback
    Play,
    Stop,
    PlayTail,
    TogglePlayback,

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
