//! The DRO Info dialog. Modal.
//!
//! View-only unless `ui.dro_info_edit_enabled` is set, in which case an
//! Edit/Save toggle unlocks the hardware type and length. Saving goes through
//! the undoable [`EditAction::UpdateHeader`] and the dialog stays open in edit mode
//! afterwards.

use vgms_core::{DroSong, OplType};

use crate::action::{Action, EditAction, UiAction};
use crate::theme::{Palette, bevel};

/// Every value here is a number, so the fields are sized for one rather than
/// stretched across the dialog like the free-text fields elsewhere. They still
/// wrap and grow if something longer lands in one.
const FIELD_WIDTH: f32 = 160.0;

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
    pub fn new(song: &DroSong, edit_allowed: bool) -> Self {
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
        // The body borrows `self` mutably, so the footer works from copies of
        // the mode flags and reports clicks through cells, handled after the
        // call returns.
        let edit_allowed = self.edit_allowed;
        let edit_mode = self.edit_mode;
        let close = std::cell::Cell::new(false);
        let toggle_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "dro-info-modal",
            "DRO Info",
            palette,
            |ui| {
                egui::Grid::new("dro-info-grid")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("DRO Version");
                        ui.add_enabled(
                            false,
                            super::wrapping_edit(
                                &mut self.file_version.to_string(),
                                palette,
                                FIELD_WIDTH,
                                1,
                            ),
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
                            super::wrapping_edit(&mut self.length_text, palette, FIELD_WIDTH, 1)
                                .return_key(None),
                        );
                        ui.end_row();

                        ui.label("Calculated Length (MS)");
                        ui.add_enabled(
                            false,
                            super::wrapping_edit(
                                &mut self.calculated_ms.to_string(),
                                palette,
                                FIELD_WIDTH,
                                1,
                            ),
                        );
                        ui.end_row();
                    });
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    // The dismiss button reads "Close" in both modes, where every
                    // other dialog puts its Close -- editing then discarding is a
                    // Close like any other, not a distinct "Cancel". Only the
                    // affirmative button toggles its label, Edit -> Save.
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    if edit_allowed {
                        let label = if edit_mode { "Save" } else { "Edit" };
                        if bevel::button(ui, palette, label).clicked() {
                            toggle_clicked.set(true);
                        }
                    }
                });
            },
        );
        if toggle_clicked.get() {
            if self.edit_mode {
                self.save(actions);
            } else {
                self.edit_mode = true;
                actions.push(Action::Ui(UiAction::Status(
                    crate::strings::DRO_INFO_EDIT_MODE_ENABLED.to_owned(),
                )));
            }
        }
        open && !close.get()
    }

    fn save(&mut self, actions: &mut Vec<Action>) {
        match self.length_text.trim().parse::<u32>() {
            Ok(ms_length) => {
                actions.push(Action::Edit(EditAction::UpdateHeader {
                    opl_type: self.opl_type,
                    ms_length,
                }));
                actions.push(Action::Ui(UiAction::Alert {
                    title: crate::strings::DRO_INFO_ALERT_TITLE.to_owned(),
                    message: crate::strings::DRO_INFO_UPDATED_MESSAGE.to_owned(),
                }));
                // The dialog stays open, still in edit mode.
            }
            Err(_) => {
                actions.push(Action::Ui(UiAction::Alert {
                    title: crate::strings::DRO_INFO_ERROR_TITLE.to_owned(),
                    message: crate::strings::DRO_INFO_ERROR_MESSAGE.to_owned(),
                }));
            }
        }
    }
}
