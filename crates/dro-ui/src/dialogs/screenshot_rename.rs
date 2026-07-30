//! Pack mode's screenshot naming dialog: name a screenshot after the pack, or
//! after a variant of it. It runs both when renaming a file already in the
//! folder and when adding one -- an added screenshot is named *before* it is
//! copied in, so nothing lands under a name the user has not seen.
//!
//! The name is prefilled from the game name -- a screenshot straight out of
//! DOSBox is called something like `dosbox_000.png`, and the pack's other files
//! are all named after the game -- but it is editable, because a pack may carry
//! more than one: a title screen per region, or per game variation.

use crate::action::Action;
use crate::theme::{Palette, bevel};

/// What Save does: rename the file the dialog opened on, or write the picked
/// file it is holding into the pack folder.
#[derive(Debug)]
enum Job {
    Rename,
    /// The picked file's bytes, held until the name is settled. Owning them here
    /// means a cancelled add leaves nothing behind: the bytes are dropped with
    /// the dialog.
    Add(Vec<u8>),
}

#[derive(Debug)]
pub struct ScreenshotRenameDialog {
    /// The file the dialog opened on: the screenshot being renamed, or the file
    /// being copied in. Rename re-resolves its target by this name rather than a
    /// since-stale index, since a rescan can reorder the list.
    original_name: String,
    /// The extension the file keeps: this dialog names a screenshot, it does not
    /// change what it is.
    ext: String,
    /// The editable stem, the only thing the user types.
    stem: String,
    /// The other screenshots' names, for the collision check.
    sibling_names: Vec<String>,
    job: Job,
    /// Whether an added file is losslessly recompressed on the way in. On by
    /// default: a DOSBox capture is rarely optimal, the submission ships the
    /// bytes as they land, and it costs nothing to look at. Ignored by a rename.
    recompress: bool,
}

impl ScreenshotRenameDialog {
    /// Opens on `file_name`, proposing `game_stem` (the pack's own file-name
    /// stem) as the name.
    ///
    /// A stem that already builds on the game name is kept as it is: reopening
    /// the dialog on `Cool Game (Japan).png` must not offer to throw the
    /// `(Japan)` away. With no game name yet there is nothing to propose, so the
    /// current name stands.
    #[must_use]
    pub fn new(file_name: String, game_stem: &str, sibling_names: Vec<String>) -> Self {
        let (stem, ext) = match file_name.rsplit_once('.') {
            Some((stem, ext)) => (stem.to_owned(), ext.to_owned()),
            None => (file_name.clone(), "png".to_owned()),
        };
        let stem = if game_stem.is_empty() || stem.starts_with(game_stem) {
            stem
        } else {
            game_stem.to_owned()
        };
        Self {
            original_name: file_name,
            ext,
            stem,
            sibling_names,
            job: Job::Rename,
            recompress: false,
        }
    }

    /// Opens on a picked file about to be copied into the pack, proposing
    /// `proposed_name` (the free name the pack would have given it) and holding
    /// its `bytes` until Save. Nothing is written to the folder until then, so
    /// Close leaves the pack exactly as it was.
    #[must_use]
    pub fn adding(
        source_name: String,
        proposed_name: &str,
        bytes: Vec<u8>,
        sibling_names: Vec<String>,
    ) -> Self {
        let (stem, ext) = match proposed_name.rsplit_once('.') {
            Some((stem, ext)) => (stem.to_owned(), ext.to_owned()),
            None => (proposed_name.to_owned(), "png".to_owned()),
        };
        Self {
            original_name: source_name,
            ext,
            stem,
            sibling_names,
            job: Job::Add(bytes),
            recompress: true,
        }
    }

    /// The file name the dialog opened on -- the screenshot's identity. Exposed
    /// for tests that assert a click opened the dialog on the right image.
    #[cfg(test)]
    pub(crate) fn original_name(&self) -> &str {
        &self.original_name
    }

    /// The file name Save writes: the typed stem through the pack's file-name
    /// rules (so a subtitle's colon becomes " - ", as everywhere else), keeping
    /// the original extension.
    pub(crate) fn derived_name(&self) -> String {
        format!("{}.{}", dro_core::pack::vgm_ren_title(&self.stem), self.ext)
    }

    /// The dialog's wording, which is all that differs between the two jobs: its
    /// title, the label on the file it opened on, and the button that commits.
    fn words(&self) -> (&'static str, &'static str, &'static str) {
        match self.job {
            Job::Rename => ("Rename Screenshot", "Current name:", "Save"),
            Job::Add(_) => ("Add Screenshot", "Copying:", "Add"),
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let (title, current_label, commit) = self.words();
        // The body borrows `self` mutably, so the footer reports clicks through
        // cells and the save runs after the call returns.
        let close = std::cell::Cell::new(false);
        let commit_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "screenshot-rename-modal",
            title,
            palette,
            |ui| {
                egui::Grid::new("screenshot-rename-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        // The file as it stands: what is on disk (a rename), or what
                        // was picked (an add). Either way the new name below reads as
                        // a change from something rather than out of nowhere.
                        ui.label(current_label);
                        let mut current = self.original_name.clone();
                        ui.add(
                            super::wrapping_edit(&mut current, palette, f32::INFINITY, 1)
                                .interactive(false)
                                .text_color(palette.muted),
                        );
                        ui.end_row();

                        ui.label("Name:");
                        super::text_field(ui, palette, &mut self.stem, f32::INFINITY)
                            .on_hover_text(crate::strings::SCREENSHOT_RENAME_NAME_HOVER);
                        ui.end_row();

                        // What actually lands on disk, since the rules may have
                        // rewritten what was typed.
                        ui.label("New name:");
                        let mut derived = self.derived_name();
                        ui.add(
                            super::wrapping_edit(&mut derived, palette, f32::INFINITY, 1)
                                .interactive(false)
                                .text_color(palette.data_text),
                        );
                        ui.end_row();

                        // Only an add writes new bytes, so only an add can choose
                        // how they are packed.
                        if matches!(self.job, Job::Add(_)) {
                            ui.label("Recompress:");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.recompress, "");
                                if ui
                                .add(
                                    egui::Label::new("Losslessly, with oxipng")
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text(crate::strings::SCREENSHOT_RENAME_RECOMPRESS_HOVER)
                                .clicked()
                            {
                                self.recompress = !self.recompress;
                            }
                            });
                            ui.end_row();
                        }
                    });
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    if bevel::button(ui, palette, commit).clicked() {
                        commit_clicked.set(true);
                    }
                });
            },
        );
        // Only a clicked commit runs the save; a refused one leaves the dialog open.
        let committed = commit_clicked.get() && self.save(actions);
        open && !(close.get() || committed)
    }

    /// Validates the derived name, then emits the rename or the add; returns
    /// `false` (with an error alert queued, leaving the dialog open) if the name
    /// would be empty or would collide with a screenshot already in the pack. A
    /// *rename* to the name the file already has closes without touching the
    /// folder; an add still has a file to write, so it goes ahead.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        if dro_core::pack::vgm_ren_title(&self.stem).is_empty() {
            actions.push(Action::Alert {
                title: crate::strings::SCREENSHOT_RENAME_NAME_REQUIRED_TITLE.to_owned(),
                message: crate::strings::SCREENSHOT_RENAME_NAME_REQUIRED_MESSAGE.to_owned(),
            });
            return false;
        }
        let name = self.derived_name();
        if matches!(self.job, Job::Rename) && name == self.original_name {
            return true; // nothing to do
        }
        if self
            .sibling_names
            .iter()
            .any(|sibling| sibling.eq_ignore_ascii_case(&name))
        {
            actions.push(Action::Alert {
                title: crate::strings::SCREENSHOT_RENAME_DUPLICATE_TITLE.to_owned(),
                message: crate::strings::screenshot_rename_duplicate_message(&name),
            });
            return false;
        }
        actions.push(match &mut self.job {
            Job::Rename => Action::PackRenameScreenshot {
                original_name: self.original_name.clone(),
                file_name: name,
            },
            // Taken, not cloned: the dialog closes on the way out of this.
            Job::Add(bytes) => Action::PackAddScreenshotAs {
                file_name: name,
                bytes: std::mem::take(bytes),
                recompress: self.recompress,
            },
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(file_name: &str, game_stem: &str, siblings: &[&str]) -> ScreenshotRenameDialog {
        ScreenshotRenameDialog::new(
            file_name.to_owned(),
            game_stem,
            siblings.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    #[test]
    fn the_name_is_prefilled_from_the_game_name() {
        // The DOSBox capture name is exactly what this dialog exists to replace.
        let dialog = make("dosbox_000.png", "Cool Game", &[]);
        assert_eq!(dialog.stem, "Cool Game");
        assert_eq!(dialog.derived_name(), "Cool Game.png");
    }

    #[test]
    fn a_variant_name_survives_reopening() {
        // Already built on the game name: offering to drop the "(Japan)" would
        // undo the very thing the field is editable for.
        let dialog = make("Cool Game (Japan).png", "Cool Game", &[]);
        assert_eq!(dialog.stem, "Cool Game (Japan)");
    }

    #[test]
    fn with_no_game_name_the_current_name_stands() {
        let dialog = make("dosbox_000.png", "", &[]);
        assert_eq!(dialog.stem, "dosbox_000");
    }

    #[test]
    fn the_derived_name_follows_the_pack_file_name_rules_and_keeps_the_extension() {
        let mut dialog = make("shot.PNG", "", &[]);
        dialog.stem = "Doom II: Hell on Earth".to_owned();
        assert_eq!(dialog.derived_name(), "Doom II - Hell on Earth.PNG");
    }

    #[test]
    fn save_rejects_a_name_a_file_cannot_keep() {
        let mut dialog = make("Cool Game.png", "Cool Game", &[]);
        dialog.stem = "?!".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_rejects_a_collision_with_another_screenshot() {
        let mut dialog = make("dosbox_000.png", "Cool Game", &["Cool Game.png"]);
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_submits_the_rename() {
        let mut dialog = make("dosbox_000.png", "Cool Game", &[]);
        dialog.stem = "Cool Game (Japan)".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        match actions.as_slice() {
            [
                Action::PackRenameScreenshot {
                    original_name,
                    file_name,
                },
            ] => {
                assert_eq!(original_name, "dosbox_000.png");
                assert_eq!(file_name, "Cool Game (Japan).png");
            }
            other => panic!("expected a rename, got {other:?}"),
        }
    }

    #[test]
    fn save_on_an_unchanged_name_closes_without_renaming() {
        let mut dialog = make("Cool Game.png", "Cool Game", &[]);
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(actions.is_empty(), "nothing to rename");
    }
}
