//! Help: what the app is for, and every key and gesture it listens to.
//!
//! The shortcuts are grouped by *where* they work, because that is the question
//! being asked -- "what can I press here" -- and because several keys mean
//! different things on the two tabs (Ctrl+S saves a song in the editor and the
//! package files in pack mode). Anything the app listens for is listed; a
//! shortcut that is not in this file is not implemented.

use egui::KeyboardShortcut;

use crate::action::Action;
use crate::menus::{self, shortcut_text};
use crate::theme::Palette;

/// What a row's key column says.
///
/// `Bound` names the app's own binding constants, so the text shown is
/// generated from the binding itself: rebind a key and this dialog follows it,
/// and [`menus::ALL_SHORTCUTS`] is checked against these entries so a new
/// binding cannot go undocumented. `Text` is for the gestures and key *ranges*
/// that are not single bindings -- a mouse click, the nine channel digits.
enum Keys {
    /// One binding, or several shown as alternatives (`Del / Backspace`).
    Bound(&'static [KeyboardShortcut]),
    Text(&'static str),
}

impl Keys {
    /// The key column's text: the bindings formatted and joined, or the literal.
    fn text(&self) -> String {
        match self {
            Self::Bound(shortcuts) => shortcuts
                .iter()
                .map(shortcut_text)
                .collect::<Vec<_>>()
                .join(" / "),
            Self::Text(text) => (*text).to_owned(),
        }
    }
}

/// One table in the dialog: a heading, an optional line of context, and its
/// key/meaning rows.
struct Section {
    title: &'static str,
    note: Option<&'static str>,
    rows: &'static [(Keys, &'static str)],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Anywhere",
        note: None,
        rows: &[
            (
                Keys::Bound(&[menus::OPEN]),
                "Open a song (.dro, .vgm, .vgz)",
            ),
            (
                Keys::Bound(&[menus::UNDO]),
                "Undo -- the song in the editor, the file edits in pack mode",
            ),
            (Keys::Bound(&[menus::REDO]), "Redo"),
            (
                Keys::Bound(&[menus::REDO_ALT]),
                "Redo (the other convention)",
            ),
            (Keys::Bound(&[menus::HELP]), "This dialog"),
            (Keys::Text("Esc"), "Close the dialog on screen"),
        ],
    },
    Section {
        title: "Editor: playback",
        note: None,
        rows: &[
            (Keys::Bound(&[menus::PLAY_STOP]), "Play / stop"),
            (
                Keys::Text("1 - 9"),
                "Mute or unmute channels 1 to 9 of the selected chip",
            ),
            (
                Keys::Text("Shift+1 - 9"),
                "Mute or unmute channels 10 to 18 (chips with more)",
            ),
            (
                Keys::Text("Right-click a channel"),
                "Solo it (again to bring the rest back)",
            ),
            (
                Keys::Text("Mute / Solo by the chip tabs"),
                "Mute or solo a whole chip of a multi-chip file, whatever its core",
            ),
        ],
    },
    Section {
        title: "Editor: moving about",
        note: Some("The selected row is where playback starts."),
        rows: &[
            (
                Keys::Bound(&[menus::SELECTION_UP, menus::SELECTION_DOWN]),
                "Move the selection one instruction",
            ),
            (Keys::Text("Shift+Up / Down"), "Extend the selection"),
            (
                Keys::Bound(&[menus::PREVIOUS_DELAY, menus::NEXT_DELAY]),
                "Jump to the previous / next delay",
            ),
            (
                Keys::Bound(&[menus::GOTO]),
                "Go to an instruction by position",
            ),
            (
                Keys::Bound(&[menus::FIND_REGISTER]),
                "Find a register write",
            ),
            (
                Keys::Bound(&[menus::DRO_INFO]),
                "DRO header info (DRO only)",
            ),
        ],
    },
    Section {
        title: "Editor: editing",
        note: Some("Every edit here is undoable."),
        rows: &[
            (
                Keys::Bound(&[menus::DELETE_SELECTION, menus::DELETE_SELECTION_ALT]),
                "Delete the selected instructions",
            ),
            (
                Keys::Bound(&[menus::SET_LOOP_START]),
                "Set the loop start at the selected row",
            ),
            (
                Keys::Bound(&[menus::SET_LOOP_END]),
                "Set the loop end just past the selected row",
            ),
            (Keys::Bound(&[menus::SAVE]), "Save"),
            (Keys::Bound(&[menus::SAVE_AS]), "Save as..."),
        ],
    },
    Section {
        title: "Editor: the mouse",
        note: Some("The waveform is the strip above the instruction table."),
        rows: &[
            (
                Keys::Text("Click the waveform"),
                "Start playback there, and scroll the table to it",
            ),
            (Keys::Text("Shift+click"), "Set the loop start"),
            (Keys::Text("Shift+right-click"), "Set the loop end"),
            (Keys::Text("Click a row"), "Select it"),
            (
                Keys::Text("Ctrl+click a row"),
                "Add it to (or take it out of) the selection",
            ),
            (
                Keys::Text("Shift+click a row"),
                "Extend the selection to it",
            ),
        ],
    },
    Section {
        title: "Pack mode",
        note: Some("The tab appears once a pack folder is open."),
        rows: &[
            (
                Keys::Bound(&[menus::SAVE]),
                "Save the package .txt and .m3u",
            ),
            (
                Keys::Bound(&[menus::UNDO, menus::REDO]),
                "Undo / redo the folder's file edits",
            ),
            (Keys::Text("Click a track"), "Focus it for the keys below"),
            (
                Keys::Bound(&[menus::MOVE_TRACK_UP, menus::MOVE_TRACK_DOWN]),
                "Move the focused track up or down the order",
            ),
            (Keys::Text("Drag the grip"), "Reorder a track by hand"),
            (Keys::Text("Double-click a track"), "Open it in the editor"),
        ],
    },
];

const ONLINE: &str = "https://github.com/laurence-myers/vgm-studio";

/// The dialog holds nothing: what it shows is the same table every time.
#[derive(Debug)]
pub struct HelpDialog;

impl HelpDialog {
    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        _actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::close_only(palette);
        // Wider than the app's other dialogs: this one is a reference table read
        // across, not a form filled in down.
        let open = super::dialog_modal_sized(
            ctx,
            "help-modal",
            "Help",
            palette,
            820.0,
            |ui| {
                // A reference table needs a readable width; on a narrow window
                // the modal shrinks (down to ~240px), and a two-column Grid does
                // not wrap or scroll -- it just crushes the columns together. So
                // hold the body at a minimum width and let it scroll sideways.
                egui::ScrollArea::horizontal()
                    .id_salt("help-hscroll")
                    .show(ui, |ui| {
                        ui.set_min_width(600.0);
                        ui.colored_label(palette.muted, crate::strings::HELP_ADVICE);
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(palette.muted, crate::strings::HELP_FULL_INSTRUCTIONS);
                            ui.hyperlink(ONLINE);
                        });

                        for section in SECTIONS {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(section.title)
                                    .color(palette.data_label)
                                    .strong(),
                            );
                            if let Some(note) = section.note {
                                ui.colored_label(palette.muted, note);
                            }
                            ui.add_space(2.0);
                            egui::Grid::new(section.title)
                                .num_columns(2)
                                .striped(true)
                                .spacing([16.0, 3.0])
                                .min_col_width(150.0)
                                .show(ui, |ui| {
                                    for (keys, meaning) in section.rows {
                                        ui.label(
                                            egui::RichText::new(keys.text())
                                                .monospace()
                                                .color(palette.data_text),
                                        );
                                        ui.label(
                                            egui::RichText::new(*meaning).color(palette.label),
                                        );
                                        ui.end_row();
                                    }
                                });
                        }
                    });
            },
            |ui| footer.show(ui),
        );
        open && !footer.closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_says_a_key_and_what_it_does() {
        for section in SECTIONS {
            assert!(!section.rows.is_empty(), "{} has no rows", section.title);
            for (keys, meaning) in section.rows {
                assert!(
                    !keys.text().trim().is_empty(),
                    "{} has a blank key",
                    section.title
                );
                assert!(
                    !meaning.trim().is_empty(),
                    "{} in {} says nothing",
                    keys.text(),
                    section.title
                );
            }
        }
    }

    #[test]
    fn every_binding_the_app_makes_is_documented() {
        // Derived from the bindings themselves: a shortcut added to
        // `ALL_SHORTCUTS` and left out of the tables above fails here rather
        // than going quietly undocumented.
        let documented: Vec<KeyboardShortcut> = SECTIONS
            .iter()
            .flat_map(|section| section.rows)
            .filter_map(|(keys, _)| match keys {
                Keys::Bound(shortcuts) => Some(shortcuts.iter().copied()),
                Keys::Text(_) => None,
            })
            .flatten()
            .collect();
        for shortcut in menus::ALL_SHORTCUTS {
            assert!(
                documented.contains(shortcut),
                "{} is bound but the Help dialog does not list it",
                shortcut_text(shortcut)
            );
        }
    }

    #[test]
    fn the_key_column_is_written_from_the_binding() {
        // Not a literal that happens to match: rebinding a key has to move the
        // text in the dialog with it.
        assert_eq!(Keys::Bound(&[menus::SAVE_AS]).text(), "Ctrl+Shift+S");
        assert_eq!(
            Keys::Bound(&[menus::MOVE_TRACK_UP, menus::MOVE_TRACK_DOWN]).text(),
            "Alt+Up / Alt+Down"
        );
        assert_eq!(Keys::Bound(&[menus::PLAY_STOP]).text(), "Space");
    }
}
