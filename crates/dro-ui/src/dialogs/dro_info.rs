//! The DRO Info dialog. Modal.
//!
//! View-only unless `ui.dro_info_edit_enabled` is set, in which case an
//! Edit/Save toggle unlocks the hardware type and length. Saving goes through
//! the undoable [`Action::UpdateHeader`] and the dialog stays open in edit mode
//! afterwards.

use dro_core::{OplType, Song};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct DroInfoDialog {
    file_version: u32,
    opl_type: OplType,
    length_text: String,
    calculated_ms: u32,
    edit_allowed: bool,
    edit_mode: bool,
}

impl DroInfoDialog {
    #[must_use]
    pub fn new(song: &Song, edit_allowed: bool) -> Self {
        Self {
            file_version: song.file_version,
            opl_type: song.opl_type,
            length_text: song.ms_length.to_string(),
            calculated_ms: song.total_delay_ms(),
            edit_allowed,
            edit_mode: false,
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut keep_open = true;
        let open = super::dialog_modal(ctx, "dro-info-modal", "DRO Info", palette, |ui| {
            egui::Grid::new("dro-info-grid")
                .num_columns(2)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.label("DRO Version");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut self.file_version.to_string())
                            .text_color(palette.data_text),
                    );
                    ui.end_row();

                    ui.label("Hardware Type");
                    ui.add_enabled_ui(self.edit_mode, |ui| {
                        crate::theme::style_dropdown(ui, palette);
                        egui::ComboBox::from_id_salt("dro-info-hardware")
                            .selected_text(self.opl_type.name())
                            .show_ui(ui, |ui| {
                                for opl_type in OplType::ALL {
                                    ui.selectable_value(
                                        &mut self.opl_type,
                                        opl_type,
                                        opl_type.name(),
                                    );
                                }
                            });
                    });
                    ui.end_row();

                    ui.label("Length (MS)");
                    ui.add_enabled(
                        self.edit_mode,
                        egui::TextEdit::singleline(&mut self.length_text)
                            .text_color(palette.data_text),
                    );
                    ui.end_row();

                    ui.label("Calculated Length (MS)");
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut self.calculated_ms.to_string())
                            .text_color(palette.data_text),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                let close_label = if self.edit_mode { "Cancel" } else { "Close" };
                if bevel::button(ui, palette, close_label).clicked() {
                    keep_open = false;
                }
                if self.edit_allowed {
                    let label = if self.edit_mode { "Save" } else { "Edit" };
                    if bevel::button(ui, palette, label).clicked() {
                        if self.edit_mode {
                            self.save(actions);
                        } else {
                            self.edit_mode = true;
                            actions.push(Action::Status("DRO Info edit mode enabled.".to_owned()));
                        }
                    }
                }
            });
        });
        open && keep_open
    }

    fn save(&mut self, actions: &mut Vec<Action>) {
        match self.length_text.trim().parse::<u32>() {
            Ok(ms_length) => {
                actions.push(Action::UpdateHeader {
                    opl_type: self.opl_type,
                    ms_length,
                });
                actions.push(Action::Alert {
                    title: "DRO Info".to_owned(),
                    message: "DRO info updated.\nRemember to save the file.".to_owned(),
                });
                // The dialog stays open, still in edit mode.
            }
            Err(_) => {
                actions.push(Action::Alert {
                    title: "Error".to_owned(),
                    message: "Error updating DRO info, check that the entered values are correct."
                        .to_owned(),
                });
            }
        }
    }
}
