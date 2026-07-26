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
    /// Whether the Pack tab is showing. File and Edit follow it: each offers the
    /// commands for the screen you are on, rather than a long list of items
    /// greyed out because the other tab owns them.
    pub on_pack_tab: bool,
    /// The row the loop-marker items act on -- the same one `[` and `]` use.
    pub focused_row: Option<usize>,
    /// Whether the markers actually mark something out, rather than covering the
    /// whole song. The crop and cut items need a region to act on, and this is
    /// the very predicate the editor declines them by.
    pub has_marked_region: bool,
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
///
/// File and Edit serve both screens: they open with the commands that work
/// anywhere (the two openers; Undo/Redo, which the app points at whichever
/// history the tab owns), then the group belonging to the tab on show. The
/// commands for the *other* tab are left out rather than greyed -- a Pack-mode
/// Edit menu of nine dead song-editing items said nothing but "not here", and
/// a command one tab-click away is not lost.
pub fn bar(ui: &mut egui::Ui, palette: &Palette, state: &MenuState, actions: &mut Vec<Action>) {
    // Which screen's commands to draw. The song-bound items act on the editor's
    // song, which the Pack tab hides; the pack items need a pack, which only the
    // Pack tab can be showing.
    let editor = !state.on_pack_tab;
    // Format-specific items are shown only for the format they apply to: a VGM
    // has no DRO header to inspect, a DRO has nowhere to store a tag, and only a
    // DRO can be converted to another format.
    let is_dro = state.song_type == Some(SongFileType::Dro);
    let is_vgm = state.song_type == Some(SongFileType::Vgm);
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            // Both openers, on both screens: opening the other kind of project is
            // how you get to the other tab in the first place.
            if item(ui, "Open Song...", Some(&OPEN)) {
                actions.push(Action::OpenFile);
            }
            if item(ui, "Open Pack Folder...", None) {
                actions.push(Action::OpenPackFolder);
            }
            crate::theme::separator(ui, palette);
            if editor {
                if item(ui, "Save", Some(&SAVE)) {
                    actions.push(Action::Save);
                }
                if item(ui, "Save As...", Some(&SAVE_AS)) {
                    actions.push(Action::SaveAs);
                }
                crate::theme::separator(ui, palette);
                if item(ui, "Render to WAV...", None) {
                    actions.push(Action::OpenRenderWav);
                }
                if item(ui, "Split Channels...", None) {
                    actions.push(Action::OpenSplit);
                }
                // Split one capture into its per-song files (VGM or DRO).
                if item(ui, "Split Songs...", None) {
                    actions.push(Action::OpenSplitSongs);
                }
                // Convert to another format, in an expanding submenu. DRO only: a
                // VGM has no format this app can convert it to, and which
                // conversions a DRO offers depends on its version.
                if is_dro {
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
                }
            } else {
                // The pack's own outputs, in the order they are produced.
                if item(ui, "Save Package Files", None) {
                    actions.push(Action::PackSaveDocs);
                }
                if item(ui, "Export Zip...", None) {
                    actions.push(Action::PackExportZip);
                }
                crate::theme::separator(ui, palette);
                if item(ui, "Close Pack", None) {
                    actions.push(Action::ClosePack);
                }
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
            // the editor's song history or the pack file-edit history to match.
            if enabled_item(ui, state.can_undo, &undo_label, Some(&UNDO)) {
                actions.push(Action::Undo);
            }
            if enabled_item(ui, state.can_redo, &redo_label, Some(&REDO)) {
                actions.push(Action::Redo);
            }
            // Everything below edits the loaded song, which the Pack tab neither
            // shows nor has; there, Undo/Redo are the whole menu.
            if !editor {
                return;
            }
            crate::theme::separator(ui, palette);
            if enabled_item(ui, editor, "Goto...", Some(&GOTO)) {
                actions.push(Action::OpenGoto);
            }
            if enabled_item(ui, editor, "Find Register...", Some(&FIND_REGISTER)) {
                actions.push(Action::OpenFindRegister);
            }
            // Marking and looping work on a DRO too, so Find Loop is offered for
            // both formats; only the dialog's Apply button is VGM-gated.
            if enabled_item(ui, editor, "Find Loop...", None) {
                actions.push(Action::OpenFindLoop);
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
            if is_vgm && enabled_item(ui, editor, "Optimize VGM", None) {
                actions.push(Action::OptimizeVgm);
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
            // These edit the stream rather than the metadata, so unlike Apply they
            // work on a DRO as well -- cropping a DRO down to its good part is the
            // reason the app exists. Both need a region actually marked out.
            let marked = editor && state.has_marked_region;
            if enabled_item(ui, marked, "Crop to Marked Region", None) {
                actions.push(Action::CropToMarkers);
            }
            if enabled_item(ui, marked, "Delete Marked Region", None) {
                actions.push(Action::DeleteMarkedRegion);
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
