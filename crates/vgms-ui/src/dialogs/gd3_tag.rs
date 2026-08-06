//! The GD3 tag editor. Modal; Save applies the tag (not undoably) and closes.

use vgms_core::{Gd3Tag, vgm::data::GD3_FIELD_COUNT};

use crate::action::{Action, EditAction};
use crate::theme::Palette;

/// Labels in `Gd3Tag` field order; "orig" is GD3's original-language variant.
/// Shared with the bulk-tag dialog, which lays the same labels out with a
/// per-field apply checkbox.
pub(crate) const LABELS: [&str; GD3_FIELD_COUNT] = [
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

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::new(palette, "Save");
        let open = super::dialog_modal(
            ctx,
            "gd3-tag-modal",
            "GD3 Tag",
            palette,
            |ui| {
                egui::Grid::new("gd3-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        gd3_fields(ui, palette, &mut self.fields);
                    });
            },
            |ui| footer.show(ui),
        );
        if footer.primary_clicked() {
            actions.push(Action::Edit(EditAction::SaveGd3(Box::new(
                Gd3Tag::from_fields(self.fields.clone()),
            ))));
        }
        open && footer.clicked() == super::FooterClick::None
    }
}

/// Draws the GD3 tag fields as label + text-edit rows into an already-open
/// two-column grid: the Notes field (last) is a 4-row multiline, the rest are
/// one-line [`super::text_field`]s. Shared with the pack quick-edit dialog,
/// which prepends its own File-name row before calling this.
///
/// Every field fills the rest of the dialog and wraps at its edge, growing
/// downwards, so a long game name or credit line is not hidden off the edge.
pub(crate) fn gd3_fields(
    ui: &mut egui::Ui,
    palette: &Palette,
    fields: &mut [String; GD3_FIELD_COUNT],
) {
    for (index, label) in LABELS.iter().enumerate() {
        ui.label(*label);
        let is_notes = index == GD3_FIELD_COUNT - 1;
        if is_notes {
            // Notes is the one field whose value really is several lines, so it
            // keeps its Enter and opens at four rows.
            ui.add(super::wrapping_edit(
                &mut fields[index],
                palette,
                f32::INFINITY,
                4,
            ));
        } else {
            super::text_field(ui, palette, &mut fields[index], f32::INFINITY);
        }
        ui.end_row();
    }
}
