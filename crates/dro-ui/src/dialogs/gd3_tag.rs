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
        let mut open = true;
        let mut close = false;
        egui::Window::new("GD3 Tag")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .constrain_to(area)
            .show(ctx, |ui| {
                egui::Grid::new("gd3-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        for (index, label) in LABELS.iter().enumerate() {
                            ui.label(*label);
                            let is_notes = index == GD3_FIELD_COUNT - 1;
                            if is_notes {
                                ui.add(
                                    egui::TextEdit::multiline(&mut self.fields[index])
                                        .text_color(palette.data_text)
                                        .desired_width(250.0)
                                        .desired_rows(4),
                                );
                            } else {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.fields[index])
                                        .text_color(palette.data_text)
                                        .desired_width(250.0),
                                );
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
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
