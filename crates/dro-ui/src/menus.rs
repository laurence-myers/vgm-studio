//! The menu bar and keyboard shortcuts (`menus.py` + the wx accelerator table).

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
}

/// Draws the bar, pushing whatever the user picked onto `actions`.
pub fn bar(ui: &mut egui::Ui, palette: &Palette, state: &MenuState, actions: &mut Vec<Action>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if item(ui, "Open...", Some(&OPEN)) {
                actions.push(Action::OpenFile);
            }
            if item(ui, "Save", Some(&SAVE)) {
                actions.push(Action::Save);
            }
            if item(ui, "Save As...", Some(&SAVE_AS)) {
                actions.push(Action::SaveAs);
            }
            crate::theme::separator(ui, palette);
            // New in the Rust port: the Python read drotrim.ini only.
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
            if enabled_item(ui, state.can_undo, &undo_label, Some(&UNDO)) {
                actions.push(Action::Undo);
            }
            if enabled_item(ui, state.can_redo, &redo_label, Some(&REDO)) {
                actions.push(Action::Redo);
            }
            crate::theme::separator(ui, palette);
            if item(ui, "Goto...", Some(&GOTO)) {
                actions.push(Action::OpenGoto);
            }
            if item(ui, "Find Register...", Some(&FIND_REGISTER)) {
                actions.push(Action::OpenFindRegister);
            }
            if item(ui, "DRO Info...", Some(&DRO_INFO)) {
                actions.push(Action::OpenDroInfo);
            }
            if item(ui, "Edit Tag", None) {
                actions.push(Action::OpenEditTag);
            }
            if item(ui, "Edit VGM Metadata", None) {
                actions.push(Action::OpenVgmMetadata);
            }
            if item(ui, "Convert to VGM", None) {
                actions.push(Action::ConvertToVgm);
            }
            crate::theme::separator(ui, palette);
            // The Del key is handled as a plain key, not a shortcut; the hint
            // matches the Python label "&Delete Instruction(s)\tDEL".
            if ui
                .add(egui::Button::new("Delete Instruction(s)").shortcut_text("Del"))
                .clicked()
            {
                actions.push(Action::DeleteSelection);
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
