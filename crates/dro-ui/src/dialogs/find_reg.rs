//! The Find Register dialog (`dialogs.DTDialogFindReg`). Modeless. The choice
//! list is fixed at open time: the delay tokens, `BANK` only where bank-switch
//! instructions can exist (DRO v1), then every register `0x00`..`0xFF`.
//!
//! The Python offered `BANK` for anything that was not DRO v2 -- including
//! VGMs, where no instruction can ever match it. Gating on "is actually v1"
//! drops that dead entry.

use dro_core::song::DRO_FILE_V1;
use dro_core::{Song, SongFileType};

use crate::action::Action;

#[derive(Debug)]
pub struct FindRegDialog {
    choices: Vec<String>,
    selected: String,
}

impl FindRegDialog {
    #[must_use]
    pub fn new(song: &Song) -> Self {
        let mut choices: Vec<String> = ["DLYS", "DLYL", "DALL"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        if song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V1 {
            choices.push("BANK".to_owned());
        }
        choices.extend((0..=0xFFu16).map(|reg| format!("0x{reg:02X}")));
        Self {
            choices,
            // Nothing selected initially (the Python combobox started at -1);
            // searching with no choice is a silent no-op.
            selected: String::new(),
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) -> bool {
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new("Find Register")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Instruction:");
                    egui::ComboBox::from_id_salt("find-reg-choice")
                        .selected_text(&self.selected)
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
                ui.horizontal(|ui| {
                    if ui.button("Find Previous").clicked() {
                        actions.push(Action::FindRegister {
                            target: self.selected.clone(),
                            backwards: true,
                        });
                    }
                    if ui.button("Find Next").clicked() {
                        actions.push(Action::FindRegister {
                            target: self.selected.clone(),
                            backwards: false,
                        });
                    }
                    if ui.button("Close").clicked() {
                        close_clicked = true;
                    }
                });
            });
        open && !close_clicked
    }
}
