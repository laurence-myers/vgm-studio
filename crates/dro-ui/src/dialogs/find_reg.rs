//! The Find Register dialog. Modeless. The choice
//! list is fixed at open time: the delay tokens, `BANK` only where bank-switch
//! instructions can exist (DRO v1), then every register `0x00`..`0xFF`.

use dro_core::song::DRO_FILE_V1;
use dro_core::{FindTarget, Song, SongFileType};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct FindRegDialog {
    choices: Vec<String>,
    selected: String,
}

impl FindRegDialog {
    #[must_use]
    pub fn new(song: &Song) -> Self {
        let is_v1 = song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V1;
        // The tokens come from dro-core (shared with `FindTarget::from_str`), so
        // the dialog can't offer one the parser rejects. BANK is dropped for
        // anything but DRO v1, where no instruction could ever match it.
        let mut choices: Vec<String> = FindTarget::TOKENS
            .iter()
            .filter(|(_, target)| *target != FindTarget::BankSwitch || is_v1)
            .map(|(token, _)| (*token).to_owned())
            .collect();
        // Bare hex, matching the table's Reg. column; `FindTarget::from_str`
        // accepts it (an optional `0x` is stripped).
        choices.extend((0..=0xFFu16).map(|reg| format!("{reg:02X}")));
        Self {
            choices,
            // Nothing selected initially; searching with no choice is a silent
            // no-op.
            selected: String::new(),
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
        let mut close_clicked = false;
        let open = super::dialog_window(ctx, "Find Register", area, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("Instruction:");
                ui.scope(|ui| {
                    crate::theme::style_dropdown(ui, palette);
                    egui::ComboBox::from_id_salt("find-reg-choice")
                        .selected_text(&self.selected)
                        .width(120.0)
                        .height(300.0)
                        .show_ui(ui, |ui| {
                            for choice in &self.choices {
                                if ui
                                    .selectable_label(*choice == self.selected, choice)
                                    .clicked()
                                {
                                    self.selected = choice.clone();
                                }
                            }
                        });
                });
            });
            crate::theme::separator(ui, palette);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close_clicked = true;
                }
                if bevel::button(ui, palette, "Find Next").clicked() {
                    actions.push(Action::FindRegister {
                        target: self.selected.clone(),
                        backwards: false,
                    });
                }
                if bevel::button(ui, palette, "Find Previous").clicked() {
                    actions.push(Action::FindRegister {
                        target: self.selected.clone(),
                        backwards: true,
                    });
                }
            });
        });
        open && !close_clicked
    }
}
