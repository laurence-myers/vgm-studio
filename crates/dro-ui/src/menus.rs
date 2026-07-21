//! The menu bar and keyboard shortcuts.

use dro_core::SongFileType;
use egui::{Key, KeyboardShortcut, Modifiers};

use crate::action::Action;
use crate::theme::Palette;

pub const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
pub const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
pub const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
pub const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
/// The conventional second redo binding. Must be consumed *before* [`UNDO`]:
/// egui's shortcut matching ignores a surplus Shift, so Ctrl+Shift+Z would
/// otherwise register as Undo.
pub const REDO_ALT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
pub const GOTO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::G);
pub const FIND_REGISTER: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
pub const DRO_INFO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::I);
pub const HELP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::H);

/// What the menu bar needs to know about the app to draw itself.
#[derive(Debug, Clone, Default)]
pub struct MenuState {
    pub can_undo: bool,
    pub can_redo: bool,
    /// The command the next Undo would revert, for the item label.
    pub undo_description: Option<String>,
    pub redo_description: Option<String>,
    /// Whether a rip project is open (enables the Rip menu's project items).
    pub has_rip: bool,
    /// Whether the Rip tab is showing (disables the song-bound File/Edit items).
    pub on_rip_tab: bool,
    /// The row the loop-marker items act on -- the same one `[` and `]` use.
    pub focused_row: Option<usize>,
    /// The loaded song's format, if any. Items that only make sense for one
    /// format are hidden for the other rather than shown greyed: a VGM has no
    /// DRO header to inspect and a DRO has nowhere to put a tag, so offering
    /// them at all is just noise to read past.
    pub song_type: Option<SongFileType>,
    /// Whether the loaded song is a DRO **v2** specifically -- the only thing
    /// there is to convert down to v1.
    pub is_dro_v2: bool,
}

/// Draws the bar, pushing whatever the user picked onto `actions`.
pub fn bar(ui: &mut egui::Ui, palette: &Palette, state: &MenuState, actions: &mut Vec<Action>) {
    // The song-bound File and Edit items act on the editor's song, which the
    // Rip tab hides; disable them there so they cannot edit an unseen song.
    let editor = !state.on_rip_tab;
    // Format-specific items are shown only for the format they apply to: a VGM
    // has no DRO header to inspect, a DRO has nowhere to store a tag, and only a
    // DRO can be converted to another format.
    let is_dro = state.song_type == Some(SongFileType::Dro);
    let is_vgm = state.song_type == Some(SongFileType::Vgm);
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if item(ui, "Open...", Some(&OPEN)) {
                actions.push(Action::OpenFile);
            }
            if enabled_item(ui, editor, "Save", Some(&SAVE)) {
                actions.push(Action::Save);
            }
            if enabled_item(ui, editor, "Save As...", Some(&SAVE_AS)) {
                actions.push(Action::SaveAs);
            }
            crate::theme::separator(ui, palette);
            if enabled_item(ui, editor, "Render to WAV...", None) {
                actions.push(Action::OpenRenderWav);
            }
            if enabled_item(ui, editor, "Split Channels...", None) {
                actions.push(Action::OpenSplit);
            }
            // Convert to another format, in an expanding submenu. DRO only: a VGM
            // has no format this app can convert it to, and which conversions a
            // DRO offers depends on its version. Disabled on the rip tab, like its
            // File-menu siblings.
            if is_dro {
                ui.add_enabled_ui(editor, |ui| {
                    ui.menu_button("Convert", |ui| {
                        if item(ui, "Convert to VGM", None) {
                            actions.push(Action::ConvertToVgm);
                        }
                        // Only a v2 has anywhere to go: v1 is already the older
                        // format.
                        if state.is_dro_v2 && item(ui, "Convert to DRO v1", None) {
                            actions.push(Action::ConvertToDro1);
                        }
                    });
                });
            }
            crate::theme::separator(ui, palette);
            if item(ui, "Settings...", None) {
                actions.push(Action::OpenSettings);
            }
            crate::theme::separator(ui, palette);
            if item(ui, "Exit", None) {
                actions.push(Action::Exit);
            }
        });

        ui.menu_button("Edit", |ui| {
            let undo_label = match &state.undo_description {
                Some(description) => format!("Undo {description}"),
                None => "Undo".to_owned(),
            };
            let redo_label = match &state.redo_description {
                Some(description) => format!("Redo {description}"),
                None => "Redo".to_owned(),
            };
            // Undo/Redo work on both tabs: the app fills can_undo/can_redo from
            // the editor's song history or the rip file-edit history to match.
            if enabled_item(ui, state.can_undo, &undo_label, Some(&UNDO)) {
                actions.push(Action::Undo);
            }
            if enabled_item(ui, state.can_redo, &redo_label, Some(&REDO)) {
                actions.push(Action::Redo);
            }
            crate::theme::separator(ui, palette);
            if enabled_item(ui, editor, "Goto...", Some(&GOTO)) {
                actions.push(Action::OpenGoto);
            }
            if enabled_item(ui, editor, "Find Register...", Some(&FIND_REGISTER)) {
                actions.push(Action::OpenFindRegister);
            }
            if is_dro && enabled_item(ui, editor, "DRO Info...", Some(&DRO_INFO)) {
                actions.push(Action::OpenDroInfo);
            }
            if is_vgm && enabled_item(ui, editor, "Edit Tag", None) {
                actions.push(Action::OpenEditTag);
            }
            if is_vgm && enabled_item(ui, editor, "Edit VGM Metadata", None) {
                actions.push(Action::OpenVgmMetadata);
            }
            crate::theme::separator(ui, palette);
            // The loop markers. The gestures ([ and ], and modifier-clicks on the
            // waveform) are the fast path; these are how they are discovered, and
            // the only way to reach Apply at all.
            if ui
                .add_enabled(
                    editor,
                    egui::Button::new("Set Loop Start").shortcut_text("["),
                )
                .clicked()
                && let Some(row) = state.focused_row
            {
                actions.push(Action::SetLoopStart(row));
            }
            if ui
                .add_enabled(editor, egui::Button::new("Set Loop End").shortcut_text("]"))
                .clicked()
                && let Some(row) = state.focused_row
            {
                actions.push(Action::SetLoopEnd(row + 1));
            }
            if enabled_item(ui, editor, "Clear Loop Markers", None) {
                actions.push(Action::ClearLoopMarkers);
            }
            // Marking and looping work on a DRO too -- auditioning a region is
            // useful whatever the format -- but only a VGM has anywhere to store
            // the result.
            if is_vgm && enabled_item(ui, editor, "Apply Loop to Metadata", None) {
                actions.push(Action::ApplyLoopToMetadata);
            }
            crate::theme::separator(ui, palette);
            // The Del key is handled as a plain key, not a shortcut; the hint
            // matches the label "&Delete Instruction(s)\tDEL".
            if ui
                .add_enabled(
                    editor,
                    egui::Button::new("Delete Instruction(s)").shortcut_text("Del"),
                )
                .clicked()
            {
                actions.push(Action::DeleteSelection);
            }
        });

        ui.menu_button("Rip", |ui| {
            if item(ui, "Open Rip Folder...", None) {
                actions.push(Action::OpenRipFolder);
            }
            if enabled_item(ui, state.has_rip, "Save Package Files", None) {
                actions.push(Action::RipSaveDocs);
            }
            if enabled_item(ui, state.has_rip, "Export Zip...", None) {
                actions.push(Action::RipExportZip);
            }
            crate::theme::separator(ui, palette);
            if enabled_item(ui, state.has_rip, "Close Rip", None) {
                actions.push(Action::CloseRip);
            }
        });

        ui.menu_button("Help", |ui| {
            if item(ui, "Help...", Some(&HELP)) {
                actions.push(Action::Help);
            }
            if item(ui, "About...", None) {
                actions.push(Action::About);
            }
        });
    });
}

fn item(ui: &mut egui::Ui, label: &str, shortcut: Option<&KeyboardShortcut>) -> bool {
    enabled_item(ui, true, label, shortcut)
}

fn enabled_item(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: Option<&KeyboardShortcut>,
) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(ui.ctx().format_shortcut(shortcut));
    }
    ui.add_enabled(enabled, button).clicked()
}
