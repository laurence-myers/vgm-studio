//! Help: what the app is for, and every key and gesture it listens to.
//!
//! The shortcuts are grouped by *where* they work, because that is the question
//! being asked -- "what can I press here" -- and because several keys mean
//! different things on the two tabs (Ctrl+S saves a song in the editor and the
//! package files in pack mode). Anything the app listens for is listed; a
//! shortcut that is not in this file is not implemented.

use crate::action::Action;
use crate::theme::{Palette, bevel};

/// One table in the dialog: a heading, an optional line of context, and its
/// key/meaning rows.
struct Section {
    title: &'static str,
    note: Option<&'static str>,
    rows: &'static [(&'static str, &'static str)],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Anywhere",
        note: None,
        rows: &[
            ("Ctrl+O", "Open a song (.dro, .vgm, .vgz)"),
            (
                "Ctrl+Z",
                "Undo -- the song in the editor, the file edits in pack mode",
            ),
            ("Ctrl+Y", "Redo"),
            ("Ctrl+Shift+Z", "Redo (the other convention)"),
            ("Ctrl+H", "This dialog"),
            ("Esc", "Close the dialog on screen"),
        ],
    },
    Section {
        title: "Editor: playback",
        note: None,
        rows: &[
            ("Space", "Play / stop"),
            ("1 - 9", "Mute or unmute channels 1 to 9"),
            ("Shift+1 - 9", "Mute or unmute channels 10 to 18 (OPL3)"),
        ],
    },
    Section {
        title: "Editor: moving about",
        note: Some("The selected row is where playback starts."),
        rows: &[
            ("Up / Down", "Move the selection one instruction"),
            ("Shift+Up / Down", "Extend the selection"),
            ("Left / Right", "Jump to the previous / next delay"),
            ("Ctrl+G", "Go to an instruction by position"),
            ("Ctrl+F", "Find a register write"),
            ("Ctrl+I", "DRO header info (DRO only)"),
        ],
    },
    Section {
        title: "Editor: editing",
        note: Some("Every edit here is undoable."),
        rows: &[
            ("Del / Backspace", "Delete the selected instructions"),
            ("[", "Set the loop start at the selected row"),
            ("]", "Set the loop end just past the selected row"),
            ("Ctrl+S", "Save"),
            ("Ctrl+Shift+S", "Save as..."),
        ],
    },
    Section {
        title: "Editor: the mouse",
        note: Some("The waveform is the strip above the instruction table."),
        rows: &[
            (
                "Click the waveform",
                "Start playback there, and scroll the table to it",
            ),
            ("Shift+click", "Set the loop start"),
            ("Shift+right-click", "Set the loop end"),
            ("Click a row", "Select it"),
            (
                "Ctrl+click a row",
                "Add it to (or take it out of) the selection",
            ),
            ("Shift+click a row", "Extend the selection to it"),
        ],
    },
    Section {
        title: "Pack mode",
        note: Some("The tab appears once a pack folder is open."),
        rows: &[
            ("Ctrl+S", "Save the package .txt and .m3u"),
            ("Ctrl+Z / Ctrl+Y", "Undo / redo the folder's file edits"),
            ("Click a track", "Focus it for the keys below"),
            (
                "Alt+Up / Alt+Down",
                "Move the focused track up or down the order",
            ),
            ("Drag the grip", "Reorder a track by hand"),
            ("Double-click a track", "Open it in the editor"),
        ],
    },
];

/// The trimming advice the old help box carried, kept because it is the one
/// thing here that is about *using* the app rather than driving it.
const ADVICE: &str = "To trim a song: select the instructions to remove and press Del. On a \
                      looping capture, look for a run of instructions with no delays between \
                      them -- that is usually where the instruments are set up.";

const ONLINE: &str = "https://github.com/laurence-myers/dro-trimmer";

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
        let mut close = false;
        // Wider than the app's other dialogs: this one is a reference table read
        // across, not a form filled in down.
        let open = super::dialog_modal_sized(ctx, "help-modal", "Help", palette, 820.0, |ui| {
            ui.colored_label(palette.muted, ADVICE);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.colored_label(palette.muted, "Full instructions:");
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
                                egui::RichText::new(*keys)
                                    .monospace()
                                    .color(palette.data_text),
                            );
                            ui.label(egui::RichText::new(*meaning).color(palette.label));
                            ui.end_row();
                        }
                    });
            }

            ui.add_space(10.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
            });
        });
        open && !close
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
                assert!(!keys.trim().is_empty(), "{} has a blank key", section.title);
                assert!(
                    !meaning.trim().is_empty(),
                    "{keys} in {} says nothing",
                    section.title
                );
            }
        }
    }

    #[test]
    fn the_shortcuts_match_the_ones_the_app_binds() {
        // The menus own the real bindings; this dialog must not drift from them.
        let listed: Vec<&str> = SECTIONS
            .iter()
            .flat_map(|section| section.rows.iter().map(|(keys, _)| *keys))
            .collect();
        for expected in [
            "Ctrl+O",
            "Ctrl+S",
            "Ctrl+Shift+S",
            "Ctrl+Z",
            "Ctrl+Y",
            "Ctrl+G",
            "Ctrl+F",
            "Ctrl+I",
            "Ctrl+H",
            "Alt+Up / Alt+Down",
        ] {
            assert!(
                listed.contains(&expected),
                "{expected} is bound but not documented"
            );
        }
    }
}
