//! The Licenses dialog: every emulator core compiled into this build, credited
//! in a striped table with its chips, license and authors, plus the one
//! dependency note.
//!
//! Rendered from [`vgms_synth::credits`], which is derived from the registry --
//! so a core cannot be linked in without appearing here. Opened from the About
//! dialog's "Licenses…" button.

use crate::action::Action;
use crate::theme::Palette;

/// The column gap, also the amount subtracted between the four columns when
/// bounding each so the table never scrolls sideways.
const COL_GAP: f32 = 16.0;

#[derive(Debug)]
pub struct LicensesDialog;

impl LicensesDialog {
    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        _actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::close_only(palette);
        let open = super::dialog_modal_sized(
            ctx,
            "licenses-modal",
            "Licenses",
            palette,
            super::WIDE_MODAL_WIDTH,
            |ui| {
                ui.label(
                    egui::RichText::new("Emulator cores in this build")
                        .color(palette.data_label)
                        .strong(),
                );
                ui.add_space(4.0);

                // Bound every column to a quarter of the width so long chip lists
                // or author strings wrap inside their cell rather than pushing the
                // table wider than the modal.
                let col_max = ((ui.available_width() - 3.0 * COL_GAP) / 4.0).max(60.0);
                egui::Grid::new("licenses-grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([COL_GAP, 4.0])
                    .max_col_width(col_max)
                    .show(ui, |ui| {
                        for heading in ["Core", "Chips", "License", "Authors"] {
                            ui.label(
                                egui::RichText::new(heading)
                                    .color(palette.data_label)
                                    .strong(),
                            );
                        }
                        ui.end_row();

                        for credit in vgms_synth::credits() {
                            cell(ui, &credit.label, palette.data_text);
                            cell(ui, &credit.chips, palette.label);
                            cell(
                                ui,
                                vgms_synth::short_license(&credit.license),
                                palette.label,
                            );
                            cell(ui, &credit.authors, palette.label);
                            ui.end_row();
                        }
                    });

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(crate::strings::APP_LICENSES_NOTE).color(palette.muted),
                );
            },
            |ui| footer.show(ui),
        );
        open && !footer.closed()
    }
}

/// A wrapping table cell, so a long value fills its bounded column across several
/// lines rather than widening the table.
fn cell(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
}
