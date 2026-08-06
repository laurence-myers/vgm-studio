//! Pack mode's quick-edit dialog: rename a track's file and edit its GD3 tag
//! without loading it into the editor. It is the GD3 dialog plus a leading
//! "File name" field; Save emits [`Action::QuickEditSubmitted`], which rewrites
//! the file's bytes and, if the name changed, renames it.

use vgms_core::{Gd3Tag, vgm::data::GD3_FIELD_COUNT};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct TrackEditDialog {
    /// The file name the dialog opened on -- the track's stable identity. A
    /// rescan can reorder the list, so Save re-resolves the target by this name
    /// rather than a since-stale index.
    original_name: String,
    /// The track's 1-based position, one half of the derived file name.
    track_number: usize,
    /// The original file's extension (without the dot), preserved by the rename.
    ext: String,
    fields: [String; GD3_FIELD_COUNT],
    /// The other tracks' file names, for the rename-collision check.
    sibling_names: Vec<String>,
}

impl TrackEditDialog {
    #[must_use]
    pub fn new(
        track_number: usize,
        file_name: String,
        tag: Option<&Gd3Tag>,
        sibling_names: Vec<String>,
    ) -> Self {
        let ext = file_name
            .rsplit_once('.')
            .map_or_else(|| "vgz".to_owned(), |(_, ext)| ext.to_owned());
        let mut fields = match tag {
            Some(tag) => tag.fields().map(str::to_owned),
            None => core::array::from_fn(|_| String::new()),
        };
        // The file name derives from the Track Name (EN); if the tag has none,
        // seed it from the original file name's title so the derived name starts
        // out matching the file on disk rather than blank.
        if fields[0].trim().is_empty() {
            fields[0] = vgms_core::pack::naming::title_from_filename(&file_name).to_owned();
        }
        Self {
            original_name: file_name,
            track_number,
            ext,
            fields,
            sibling_names,
        }
    }

    /// The file name the dialog opened on -- the track's identity. Exposed for
    /// tests that assert a click opened the quick-edit on the right track.
    #[cfg(test)]
    pub(crate) fn original_name(&self) -> &str {
        &self.original_name
    }

    /// The file name derived from the track number and the Track Name (EN)
    /// field, keeping the original extension. This is what Save writes.
    fn derived_name(&self) -> String {
        vgms_core::pack::naming::track_file_name(self.track_number, &self.fields[0], &self.ext)
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        // The body borrows `self` mutably, so the footer reports clicks through
        // cells and the save runs after the call returns.
        let close = std::cell::Cell::new(false);
        let save_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "track-edit-modal",
            "Quick Edit Track",
            palette,
            |ui| {
                egui::Grid::new("track-edit-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        // What the file is called now, so the rename below can be
                        // read as a change rather than an assertion.
                        ui.label("Current name:");
                        let mut current = self.original_name.clone();
                        ui.add(
                            super::wrapping_edit(&mut current, palette, f32::INFINITY, 1)
                                .interactive(false)
                                .text_color(palette.muted),
                        )
                        .on_hover_text(crate::strings::TRACK_EDIT_CURRENT_NAME_HINT);
                        ui.end_row();

                        ui.label("New name:");
                        // Read-only: the name is derived from the track number
                        // and the Track Name (EN) field below, so it always
                        // stays in step.
                        let mut derived = self.derived_name();
                        ui.add(
                            super::wrapping_edit(&mut derived, palette, f32::INFINITY, 1)
                                .interactive(false)
                                .text_color(palette.data_text),
                        )
                        .on_hover_text(crate::strings::TRACK_EDIT_NEW_NAME_HINT);
                        ui.end_row();

                        super::gd3_tag::gd3_fields(ui, palette, &mut self.fields);
                    });
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    if bevel::button(ui, palette, "Save").clicked() {
                        save_clicked.set(true);
                    }
                });
            },
        );
        // Only a clicked Save runs the validation; a refused one leaves the dialog open.
        let saved = save_clicked.get() && self.save(actions);
        open && !(close.get() || saved)
    }

    /// Validates the derived name, then emits the quick edit; returns `false`
    /// (with an error alert queued, leaving the dialog open) if the track name is
    /// empty (so the derived file name would be blank) or the derived name
    /// collides with another track in the pack.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        // The file name is derived from the Track Name (EN), so require one that
        // leaves something behind: "?!" is a track name, but every character of
        // it is dropped on the way into a file name.
        if vgms_core::pack::naming::vgm_ren_title(&self.fields[0]).is_empty() {
            actions.push(Action::Alert {
                title: crate::strings::TRACK_EDIT_TRACK_NAME_REQUIRED_TITLE.to_owned(),
                message: crate::strings::TRACK_EDIT_TRACK_NAME_REQUIRED_MESSAGE.to_owned(),
            });
            return false;
        }
        let name = self.derived_name();
        // A rename onto another track would clobber it. The case-only rename of
        // *this* file back onto itself is fine -- original_name is not a sibling.
        if !name.eq_ignore_ascii_case(&self.original_name)
            && self
                .sibling_names
                .iter()
                .any(|sibling| sibling.eq_ignore_ascii_case(&name))
        {
            actions.push(Action::Alert {
                title: crate::strings::TRACK_EDIT_DUPLICATE_TITLE.to_owned(),
                message: crate::strings::track_edit_duplicate_message(&name),
            });
            return false;
        }
        actions.push(Action::QuickEditSubmitted {
            original_name: self.original_name.clone(),
            file_name: name,
            tag: Box::new(Gd3Tag::from_fields(self.fields.clone())),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(number: usize, file_name: &str, siblings: &[&str]) -> TrackEditDialog {
        TrackEditDialog::new(
            number,
            file_name.to_owned(),
            None,
            siblings.iter().map(|s| (*s).to_owned()).collect(),
        )
    }

    #[test]
    fn derives_the_file_name_from_number_and_track_name() {
        let mut dialog = make(1, "01 Old.vgz", &[]);
        dialog.fields[0] = "Boss Battle".to_owned();
        assert_eq!(dialog.derived_name(), "01 Boss Battle.vgz");
        // The original extension is preserved.
        let mut vgm = make(9, "09 X.vgm", &[]);
        vgm.fields[0] = "Ending".to_owned();
        assert_eq!(vgm.derived_name(), "09 Ending.vgm");
    }

    #[test]
    fn seeds_the_track_name_from_the_file_name_when_the_tag_is_empty() {
        // No tag -> the Track Name (EN) starts from the file name's title, so
        // the derived name matches the file on disk out of the box.
        let dialog = make(1, "01 Intro.vgz", &[]);
        assert_eq!(dialog.fields[0], "Intro");
        assert_eq!(dialog.derived_name(), "01 Intro.vgz");
    }

    #[test]
    fn save_rejects_an_empty_track_name() {
        let mut dialog = make(1, "01 Intro.vgz", &[]);
        dialog.fields[0] = "   ".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_rejects_a_track_name_a_file_name_cannot_keep() {
        // Every character of "?!" is dropped by vgm_ren's rules, which would
        // leave the file called "01 .vgz".
        let mut dialog = make(1, "01 Intro.vgz", &[]);
        dialog.fields[0] = "?!".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn the_derived_name_follows_the_vgm_ren_rules() {
        let mut dialog = make(2, "02 Old.vgz", &[]);
        dialog.fields[0] = "Doom II: Hell on Earth".to_owned();
        assert_eq!(dialog.derived_name(), "02 Doom II - Hell on Earth.vgz");
    }

    #[test]
    fn save_rejects_a_collision_with_another_track() {
        // Number 1 + "Intro" derives "01 Intro.vgz", colliding with the sibling.
        let mut dialog = make(1, "01 Old.vgz", &["01 Intro.vgz"]);
        dialog.fields[0] = "Intro".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn save_submits_the_derived_name() {
        let mut dialog = make(1, "01 Intro.vgz", &["02 Boss.vgm"]);
        dialog.fields[0] = "Intro Redux".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        match actions.as_slice() {
            [Action::QuickEditSubmitted { file_name, .. }] => {
                assert_eq!(file_name, "01 Intro Redux.vgz");
            }
            other => panic!("expected a submit, got {other:?}"),
        }
    }

    #[test]
    fn save_allows_a_case_only_rename_of_the_same_file() {
        // Same file, different case: must not read as a sibling collision.
        let mut dialog = make(1, "01 Intro.vgz", &["02 Boss.vgm"]);
        dialog.fields[0] = "INTRO".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::QuickEditSubmitted { .. }]
        ));
    }
}
