//! Rip mode's quick-edit dialog: rename a track's file and edit its GD3 tag
//! without loading it into the editor. It is the GD3 dialog plus a leading
//! "File name" field; Save emits [`Action::QuickEditSubmitted`], which rewrites
//! the file's bytes and, if the name changed, renames it.

use dro_core::{Gd3Tag, vgm::data::GD3_FIELD_COUNT};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct TrackEditDialog {
    /// The file name the dialog opened on -- the track's stable identity. A
    /// rescan can reorder the list, so Save re-resolves the target by this name
    /// rather than a since-stale index.
    original_name: String,
    file_name: String,
    fields: [String; GD3_FIELD_COUNT],
    /// The other tracks' file names, for the rename-collision check.
    sibling_names: Vec<String>,
}

impl TrackEditDialog {
    #[must_use]
    pub fn new(file_name: String, tag: Option<&Gd3Tag>, sibling_names: Vec<String>) -> Self {
        let fields = match tag {
            Some(tag) => tag.fields().map(str::to_owned),
            None => core::array::from_fn(|_| String::new()),
        };
        Self {
            original_name: file_name.clone(),
            file_name,
            fields,
            sibling_names,
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_window(ctx, "Quick Edit Track", area, |ui| {
            egui::Grid::new("track-edit-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("File name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.file_name)
                            .text_color(palette.data_text)
                            .desired_width(250.0),
                    );
                    ui.end_row();

                    super::gd3_tag::gd3_fields(ui, palette, &mut self.fields);
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
        });
        open && !close
    }

    /// Validates the file name, then emits the quick edit; returns `false`
    /// (with an error alert queued, leaving the dialog open) if the name is
    /// empty, not a `.vgm`/`.vgz`, or collides with another track in the pack.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        let name = self.file_name.trim();
        let ext_ok = {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".vgm") || lower.ends_with(".vgz")
        };
        if name.is_empty() || !ext_ok {
            actions.push(Action::Alert {
                title: "Invalid file name".to_owned(),
                message: "The file name must not be empty and must end in .vgm or .vgz."
                    .to_owned(),
            });
            return false;
        }
        // A rename onto another track would clobber it. The case-only rename of
        // *this* file back onto itself is fine -- original_name is not a sibling.
        if !name.eq_ignore_ascii_case(&self.original_name)
            && self
                .sibling_names
                .iter()
                .any(|sibling| sibling.eq_ignore_ascii_case(name))
        {
            actions.push(Action::Alert {
                title: "Invalid file name".to_owned(),
                message: format!("Another track in this pack is already named \"{name}\"."),
            });
            return false;
        }
        actions.push(Action::QuickEditSubmitted {
            original_name: self.original_name.clone(),
            file_name: name.to_owned(),
            tag: Box::new(Gd3Tag::from_fields(self.fields.clone())),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(file_name: &str, siblings: &[&str]) -> TrackEditDialog {
        TrackEditDialog::new(
            file_name.to_owned(),
            None,
            siblings.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    #[test]
    fn save_rejects_a_non_vgm_extension() {
        let mut dialog = make("01 Intro.vgz", &[]);
        dialog.file_name = "01 Intro.txt".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_rejects_an_empty_name() {
        let mut dialog = make("01 Intro.vgz", &[]);
        dialog.file_name = "   ".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_rejects_a_collision_with_another_track() {
        let mut dialog = make("01 Intro.vgz", &["02 Boss.vgm"]);
        dialog.file_name = "02 Boss.vgm".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_accepts_a_valid_rename() {
        let mut dialog = make("01 Intro.vgz", &["02 Boss.vgm"]);
        dialog.file_name = "01 Intro Redux.vgm".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::QuickEditSubmitted { .. }]
        ));
    }

    #[test]
    fn save_allows_a_case_only_rename_of_the_same_file() {
        // Same file, different case: must not read as a sibling collision.
        let mut dialog = make("01 Intro.vgz", &["02 Boss.vgm"]);
        dialog.file_name = "01 INTRO.vgz".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::QuickEditSubmitted { .. }]
        ));
    }
}
