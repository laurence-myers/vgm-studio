//! Pack mode's screenshot rename dialog: name a screenshot after the pack, or
//! after a variant of it. Save emits [`Action::PackRenameScreenshot`], which
//! renames the file in the folder as one undoable step.
//!
//! The name is prefilled from the game name -- a screenshot straight out of
//! DOSBox is called something like `dosbox_000.png`, and the pack's other files
//! are all named after the game -- but it is editable, because a pack may carry
//! more than one: a title screen per region, or per game variation.

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct ScreenshotRenameDialog {
    /// The file name the dialog opened on -- the screenshot's identity. A rescan
    /// can reorder the list, so Save re-resolves the target by this name rather
    /// than a since-stale index.
    original_name: String,
    /// The original file's extension (without the dot), preserved by the rename:
    /// this dialog names a screenshot, it does not change what it is.
    ext: String,
    /// The editable stem, the only thing the user types.
    stem: String,
    /// The other screenshots' names, for the rename-collision check.
    sibling_names: Vec<String>,
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

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_modal(
            ctx,
            "screenshot-rename-modal",
            "Rename Screenshot",
            palette,
            |ui| {
                egui::Grid::new("screenshot-rename-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name:");
                        super::text_field(ui, palette, &mut self.stem, f32::INFINITY)
                            .on_hover_text(
                                "Prefilled from the game name. Add a suffix -- \"(Japan)\", \
                                 \"(EGA)\" -- to keep more than one screenshot in the pack.",
                            );
                        ui.end_row();

                        // What actually lands on disk, since the rules may have
                        // rewritten what was typed.
                        ui.label("Saves as:");
                        let mut derived = self.derived_name();
                        ui.add(
                            super::wrapping_edit(&mut derived, palette, f32::INFINITY, 1)
                                .interactive(false)
                                .text_color(palette.muted),
                        );
                        ui.end_row();
                    });
                ui.add_space(8.0);
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close = true;
                    }
                    if bevel::button(ui, palette, "Save").clicked() && self.save(actions) {
                        close = true;
                    }
                });
            },
        );
        open && !close
    }

    /// Validates the derived name, then emits the rename; returns `false` (with
    /// an error alert queued, leaving the dialog open) if the name would be
    /// empty or would collide with another screenshot. A name that has not
    /// changed closes the dialog without touching the folder.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        if dro_core::pack::vgm_ren_title(&self.stem).is_empty() {
            actions.push(Action::Alert {
                title: "Name required".to_owned(),
                message: "Enter a name for the screenshot file.".to_owned(),
            });
            return false;
        }
        let name = self.derived_name();
        if name == self.original_name {
            return true; // nothing to do
        }
        if self
            .sibling_names
            .iter()
            .any(|sibling| sibling.eq_ignore_ascii_case(&name))
        {
            actions.push(Action::Alert {
                title: "Duplicate file name".to_owned(),
                message: format!("Another screenshot in this pack is already named \"{name}\"."),
            });
            return false;
        }
        actions.push(Action::PackRenameScreenshot {
            original_name: self.original_name.clone(),
            file_name: name,
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
