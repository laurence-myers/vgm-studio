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

// The unmodified editor keys. They are consumed as plain `key_pressed` checks
// rather than shortcuts (Shift is meaningful on the arrows, and the handler
// bails out early when Command or Alt is held), so what these consts give is a
// name for the binding -- one the Help dialog and its guard can both read.
pub const PLAY_STOP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Space);
pub const DELETE_SELECTION: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
/// The second delete binding, for keyboards where Del is a stretch.
pub const DELETE_SELECTION_ALT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::Backspace);
pub const SELECTION_UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowUp);
pub const SELECTION_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowDown);
pub const PREVIOUS_DELAY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowLeft);
pub const NEXT_DELAY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowRight);
pub const SET_LOOP_START: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::OpenBracket);
pub const SET_LOOP_END: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::CloseBracket);

// Pack mode's own keys.
pub const MOVE_TRACK_UP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowUp);
pub const MOVE_TRACK_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::ArrowDown);

/// Every key binding the app makes, in one list.
///
/// This is what the Help dialog's guard reads: a binding added above and left
/// out of the dialog's tables fails a test rather than quietly going
/// undocumented. The channel-toggle digits are the one omission -- they are a
/// range of nine keys (and nine more with Shift) rather than a binding, and the
/// dialog lists them as such.
pub const ALL_SHORTCUTS: &[KeyboardShortcut] = &[
    OPEN,
    SAVE,
    SAVE_AS,
    UNDO,
    REDO,
    REDO_ALT,
    GOTO,
    FIND_REGISTER,
    DRO_INFO,
    HELP,
    PLAY_STOP,
    DELETE_SELECTION,
    DELETE_SELECTION_ALT,
    SELECTION_UP,
    SELECTION_DOWN,
    PREVIOUS_DELAY,
    NEXT_DELAY,
    SET_LOOP_START,
    SET_LOOP_END,
    MOVE_TRACK_UP,
    MOVE_TRACK_DOWN,
];

/// A shortcut written the way the menus and the Help dialog show it
/// (`Ctrl+Shift+S`, `Space`, `Alt+Up`, `[`).
///
/// [`egui::Context::format_shortcut`] would nearly do, but it needs a context
/// (the Help dialog's guard runs without one) and it picks either words or
/// symbols for *everything*. This takes the symbol only where it is ASCII, so
/// the bracket keys read as `[` and `]` while the arrows stay "Up" and "Down"
/// -- egui's arrow glyphs are not in the bundled VGA font and would draw as
/// tofu boxes.
#[must_use]
pub fn shortcut_text(shortcut: &KeyboardShortcut) -> String {
    let names = egui::ModifierNames::NAMES;
    let symbol = shortcut.logical_key.symbol_or_name();
    let key = if symbol.is_ascii() {
        symbol
    } else {
        shortcut.logical_key.name()
    };
    let modifiers = names.format(&shortcut.modifiers, cfg!(target_os = "macos"));
    if modifiers.is_empty() {
        key.to_owned()
    } else {
        format!("{modifiers}{}{key}", names.concat)
    }
}

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
    /// Whether any pack track has a measured peak. The levelling commands have
    /// nothing to level from until one does.
    pub pack_has_peaks: bool,
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
            widen(ui);
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
                crate::theme::separator(ui, palette);
                if item(ui, "Close Song", None) {
                    actions.push(Action::CloseFile);
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
            widen(ui);
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
            // On the Pack tab the rest of this menu edits a song that is not on
            // screen. What belongs there instead are the batch operations the
            // Tracks section carries, in the two groups its silkscreen names.
            if !editor {
                crate::theme::separator(ui, palette);
                ui.menu_button("Levels", |ui| {
                    widen(ui);
                    if item(ui, "Scan Volumes", None) {
                        actions.push(Action::PackScanVolumes);
                    }
                    // Two items rather than one plus a mode: which levelling ran
                    // is the whole question, and a menu cannot show a latch.
                    // Both need peaks to level *from*, so both wait for the scan.
                    let scanned = state.pack_has_peaks;
                    if enabled_item(ui, scanned, "Apply Album Level", None) {
                        actions.push(Action::PackApplySuggestedModifiers { album: true });
                    }
                    if enabled_item(ui, scanned, "Apply Track Levels", None) {
                        actions.push(Action::PackApplySuggestedModifiers { album: false });
                    }
                });
                // "Track Tags", not "Tags": the pack's section strip has a Tags
                // tab holding the *package* metadata, and one label must not
                // name two different things on the same screen.
                ui.menu_button("Track Tags", |ui| {
                    widen(ui);
                    if item(ui, "Bulk Tag...", None) {
                        actions.push(Action::OpenBulkTag);
                    }
                    if item(ui, "Fix Dates", None) {
                        actions.push(Action::PackConvertDatesToHyphens);
                    }
                    if item(ui, "Fix File Names", None) {
                        actions.push(Action::PackRenameFromTags);
                    }
                });
                return;
            }
            crate::theme::separator(ui, palette);
            // Navigation and search, which find a row rather than change one.
            ui.menu_button("Find", |ui| {
                widen(ui);
                if item(ui, "Goto...", Some(&GOTO)) {
                    actions.push(Action::OpenGoto);
                }
                if item(ui, "Find Register...", Some(&FIND_REGISTER)) {
                    actions.push(Action::OpenFindRegister);
                }
                // Marking and looping work on a DRO too, so Find Loop is offered
                // for both formats; only the dialog's Apply button is VGM-gated.
                if item(ui, "Find Loop...", None) {
                    actions.push(Action::OpenFindLoop);
                }
            });
            // Everything about the loop region in one place: the two markers, the
            // clear, and the one thing that writes them into the file. The
            // gestures ([ and ], and modifier-clicks on the waveform) are the
            // fast path; this submenu is how they are discovered.
            ui.menu_button("Loop", |ui| {
                widen(ui);
                if ui
                    .add(
                        egui::Button::new("Set Loop Start")
                            .shortcut_text(shortcut_text(&SET_LOOP_START)),
                    )
                    .clicked()
                    && let Some(row) = state.focused_row
                {
                    actions.push(Action::SetLoopStart(row));
                }
                if ui
                    .add(
                        egui::Button::new("Set Loop End")
                            .shortcut_text(shortcut_text(&SET_LOOP_END)),
                    )
                    .clicked()
                    && let Some(row) = state.focused_row
                {
                    actions.push(Action::SetLoopEnd(row + 1));
                }
                if item(ui, "Clear Loop Markers", None) {
                    actions.push(Action::ClearLoopMarkers);
                }
                // Marking and looping work on a DRO too -- auditioning a region
                // is useful whatever the format -- but only a VGM has anywhere
                // to store the result.
                if is_vgm && item(ui, "Apply Loop to Metadata", None) {
                    actions.push(Action::ApplyLoopToMetadata);
                }
            });
            crate::theme::separator(ui, palette);
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
            // The three ways to take instructions out of the song, together: two
            // act on the marked region (and need one marked), one on the
            // selected rows. They edit the stream rather than the metadata, so
            // unlike Apply Loop they work on a DRO as well -- cropping a DRO down
            // to its good part is the reason this app exists.
            let marked = editor && state.has_marked_region;
            if enabled_item(ui, marked, "Crop to Marked Region", None) {
                actions.push(Action::CropToMarkers);
            }
            if enabled_item(ui, marked, "Delete Marked Region", None) {
                actions.push(Action::DeleteMarkedRegion);
            }
            // The Del key is handled as a plain key, not a shortcut; the hint
            // matches the label "&Delete Instruction(s)\tDEL".
            if ui
                .add_enabled(
                    editor,
                    egui::Button::new("Delete Instruction(s)")
                        .shortcut_text(shortcut_text(&DELETE_SELECTION)),
                )
                .clicked()
            {
                actions.push(Action::DeleteSelection);
            }
        });

        ui.menu_button("Help", |ui| {
            widen(ui);
            if item(ui, "Help...", Some(&HELP)) {
                actions.push(Action::Help);
            }
            if item(ui, "About...", None) {
                actions.push(Action::About);
            }
        });
    });
}

/// Claims a width the longest item in the menu fits on one line.
///
/// A menu sizes itself to the widest item drawn *so far*, and the first two are
/// Undo and Redo -- so "Delete Instruction(s) Del" wrapped to three lines in a
/// box shaped by "Undo Ctrl+Z". A menu that grows as it is read is worse than
/// one that starts wide, and at a 16px DOS font the longest label here is around
/// 34 characters including its shortcut.
fn widen(ui: &mut egui::Ui) {
    ui.set_min_width(300.0);
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
        // The app's own formatter, not the context's: the menu hint and the Help
        // dialog's key column must read the same.
        button = button.shortcut_text(shortcut_text(shortcut));
    }
    ui.add_enabled(enabled, button).clicked()
}
