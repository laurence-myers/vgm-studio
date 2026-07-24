//! Pack mode's bulk GD3 tag editor: write chosen fields to many tracks at once.
//!
//! Where the quick-edit dialog ([`super::track_edit`]) rewrites one track, this
//! one writes a chosen set of GD3 fields across a chosen set of tracks. Each
//! field has its own apply checkbox, so a bulk edit touches only what is checked
//! and leaves every other field at each track's own value -- which is how you
//! stamp the shared game name onto all tracks, or fix the composer on just the
//! half a different musician wrote, without disturbing anything else.
//!
//! Apply emits [`Action::BulkTagSubmitted`]; the app overlays the checked fields
//! onto each target's existing tag and rewrites the files as one undoable batch.

use dro_core::vgm::data::GD3_FIELD_COUNT;

use super::gd3_tag::LABELS;
use crate::action::Action;
use crate::pack::BulkTagOverlay;
use crate::theme::{Palette, bevel};

/// One candidate track: its file name is the stable identity the app re-resolves
/// against (a rescan can reorder the list), and `label` is what the checkbox shows.
#[derive(Debug, Clone)]
struct Target {
    file_name: String,
    label: String,
    selected: bool,
}

#[derive(Debug)]
pub struct BulkTagDialog {
    /// Which fields to write and their values, seeded from the package metadata.
    overlay: BulkTagOverlay,
    /// Every readable track, all selected initially ("apply to all").
    targets: Vec<Target>,
}

impl BulkTagDialog {
    /// `tracks`: `(file_name, display_label)` for every readable track, in list
    /// order. `overlay`: the seeded field values and checks (see
    /// [`crate::pack::seed_from_meta`]).
    #[must_use]
    pub fn new(tracks: Vec<(String, String)>, overlay: BulkTagOverlay) -> Self {
        let targets = tracks
            .into_iter()
            .map(|(file_name, label)| Target {
                file_name,
                label,
                selected: true,
            })
            .collect();
        Self { overlay, targets }
    }

    /// Draws the dialog. Returns `false` once closed.
    ///
    /// A modal rather than a floating window, and taken to most of `area`:
    /// eleven GD3 fields plus a track list is more than the app's default 800pt
    /// window is tall, and as a plain window the box simply ran off the bottom
    /// of the screen with its Apply button beyond reach. The heading and the
    /// footer are pinned; only the middle scrolls, so the buttons stay put
    /// however many tracks the pack has.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        // Leave a margin of screen around it, and room for the pinned chrome.
        let width = (area.width() * 0.9).min(720.0);
        let body_height = (area.height() * 0.9 - 150.0).max(160.0);
        let modal = egui::Modal::new(egui::Id::new("bulk-tag-modal")).show(ctx, |ui| {
            ui.set_width(width);
            ui.heading("Bulk Tag Tracks");
            ui.colored_label(
                palette.muted,
                "Check the fields to write, then choose the tracks. Unchecked fields keep \
                 each track's own value.",
            );
            crate::theme::separator_clipped(ui, palette);
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("bulk-tag-fields")
                        .num_columns(3)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            for (index, label) in LABELS.iter().enumerate() {
                                ui.checkbox(&mut self.overlay.apply[index], "");
                                ui.label(*label);
                                let value = &mut self.overlay.values[index];
                                if index == GD3_FIELD_COUNT - 1 {
                                    ui.add(
                                        egui::TextEdit::multiline(value)
                                            .text_color(palette.data_text)
                                            .desired_width(250.0)
                                            .desired_rows(3),
                                    );
                                } else {
                                    ui.add(
                                        egui::TextEdit::singleline(value)
                                            .text_color(palette.data_text)
                                            .desired_width(250.0),
                                    );
                                }
                                ui.end_row();
                            }
                        });

                    ui.add_space(8.0);
                    crate::theme::separator_clipped(ui, palette);
                    self.target_picker(ui, palette);
                });

            ui.add_space(8.0);
            crate::theme::separator_clipped(ui, palette);
            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Apply").clicked() && self.apply(actions) {
                    close = true;
                }
            });
        });
        // Esc or a click on the backdrop dismisses it, like the alert modal.
        !close && !modal.should_close()
    }

    /// The "Apply to:" heading, All/None buttons, a running count, and the
    /// scrollable list of per-track checkboxes.
    fn target_picker(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Apply to:")
                    .color(palette.data_label)
                    .strong(),
            );
            if bevel::button(ui, palette, "All").clicked() {
                self.set_all(true);
            }
            if bevel::button(ui, palette, "None").clicked() {
                self.set_all(false);
            }
            let selected = self.selected_count();
            ui.colored_label(
                palette.muted,
                format!("{selected}/{} selected", self.targets.len()),
            );
        });
        // No scroll area of its own: the whole dialog body scrolls now, and a
        // nested one would trap the wheel over the track list.
        for target in &mut self.targets {
            // Clone the label so the immutable borrow ends before the checkbox
            // takes `selected` mutably (both live on `target`).
            let label = target.label.clone();
            ui.checkbox(&mut target.selected, label);
        }
    }

    fn set_all(&mut self, selected: bool) {
        for target in &mut self.targets {
            target.selected = selected;
        }
    }

    fn selected_count(&self) -> usize {
        self.targets.iter().filter(|t| t.selected).count()
    }

    /// Validates the edit, then emits it; returns `false` (with an alert queued,
    /// leaving the dialog open) if no field is checked or no track is selected.
    fn apply(&mut self, actions: &mut Vec<Action>) -> bool {
        if !self.overlay.writes_anything() {
            actions.push(Action::Alert {
                title: "Nothing to write".to_owned(),
                message: "Check at least one field to write to the selected tracks.".to_owned(),
            });
            return false;
        }
        let targets: Vec<String> = self
            .targets
            .iter()
            .filter(|t| t.selected)
            .map(|t| t.file_name.clone())
            .collect();
        if targets.is_empty() {
            actions.push(Action::Alert {
                title: "No tracks selected".to_owned(),
                message: "Select at least one track to tag.".to_owned(),
            });
            return false;
        }
        actions.push(Action::BulkTagSubmitted {
            targets,
            overlay: Box::new(self.overlay.clone()),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> BulkTagDialog {
        let overlay = BulkTagOverlay::default();
        BulkTagDialog::new(
            vec![
                ("01 Intro.vgz".to_owned(), "01 Intro".to_owned()),
                ("02 Boss.vgm".to_owned(), "02 Boss".to_owned()),
                ("03 Ending.vgz".to_owned(), "03 Ending".to_owned()),
            ],
            overlay,
        )
    }

    #[test]
    fn new_selects_every_track() {
        let dialog = dialog();
        assert_eq!(dialog.selected_count(), 3);
    }

    #[test]
    fn apply_rejects_when_no_field_is_checked() {
        let mut dialog = dialog();
        let mut actions = Vec::new();
        assert!(!dialog.apply(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn apply_rejects_when_no_track_is_selected() {
        let mut dialog = dialog();
        dialog.overlay.apply[2] = true; // a field is checked...
        dialog.set_all(false); // ...but nothing is selected
        let mut actions = Vec::new();
        assert!(!dialog.apply(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn apply_submits_the_selected_targets_and_overlay() {
        let mut dialog = dialog();
        dialog.overlay.apply[6] = true; // Track Author (EN)
        dialog.overlay.values[6] = "New Composer".to_owned();
        // Deselect the middle track: only 01 and 03 get the edit.
        dialog.targets[1].selected = false;

        let mut actions = Vec::new();
        assert!(dialog.apply(&mut actions));
        match actions.as_slice() {
            [Action::BulkTagSubmitted { targets, overlay }] => {
                assert_eq!(targets, &["01 Intro.vgz", "03 Ending.vgz"]);
                assert!(overlay.apply[6]);
                assert_eq!(overlay.values[6], "New Composer");
            }
            other => panic!("expected a bulk submit, got {other:?}"),
        }
    }
}
