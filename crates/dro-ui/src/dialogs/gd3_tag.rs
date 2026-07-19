//! The GD3 tag editor (`gd3_tag_dialog.py`). Modeless; Save applies the tag
//! (not undoably, as in Python) and closes.

use dro_core::{Gd3Tag, vgm::data::GD3_FIELD_COUNT};

use crate::action::Action;
use crate::theme::{Palette, bevel};

/// Labels in `Gd3Tag` field order; "orig" is GD3's original-language variant.
const LABELS: [&str; GD3_FIELD_COUNT] = [
    "Track Name (EN):",
    "Track Name (orig):",
    "Game Name (EN):",
    "Game Name (orig):",
    "System Name (EN):",
    "System Name (orig):",
    "Track Author (EN):",
    "Track Author (orig):",
    "Release Date:",
    "Creator:",
    "Notes:",
];

#[derive(Debug)]
pub struct Gd3TagDialog {
    fields: [String; GD3_FIELD_COUNT],
}

impl Gd3TagDialog {
    #[must_use]
    pub fn new(tag: Option<&Gd3Tag>) -> Self {
        let fields = match tag {
            Some(tag) => tag.fields().map(str::to_owned),
            None => core::array::from_fn(|_| String::new()),
        };
        Self { fields }
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
        let open = super::dialog_window(ctx, "GD3 Tag", area, |ui| {
            egui::Grid::new("gd3-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    gd3_fields(ui, palette, &mut self.fields);
                });
            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Save").clicked() {
                    actions.push(Action::SaveGd3(Box::new(Gd3Tag::from_fields(
                        self.fields.clone(),
                    ))));
                    close = true;
                }
            });
        });
        open && !close
    }
}

/// Draws the GD3 tag fields as label + text-edit rows into an already-open
/// two-column grid: the Notes field (last) is a 4-row multiline, the rest are
/// single-line. Shared with the rip quick-edit dialog, which prepends its own
/// File-name row before calling this.
pub(crate) fn gd3_fields(
    ui: &mut egui::Ui,
    palette: &Palette,
    fields: &mut [String; GD3_FIELD_COUNT],
) {
    for (index, label) in LABELS.iter().enumerate() {
        ui.label(*label);
        let is_notes = index == GD3_FIELD_COUNT - 1;
        if is_notes {
            ui.add(
                egui::TextEdit::multiline(&mut fields[index])
                    .text_color(palette.data_text)
                    .desired_width(250.0)
                    .desired_rows(4),
            );
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut fields[index])
                    .text_color(palette.data_text)
                    .desired_width(250.0),
            );
        }
        ui.end_row();
    }
}
