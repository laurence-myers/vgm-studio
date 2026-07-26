//! What to say when a perfectly good VGM is not one the editor can open. Modal.
//!
//! The editor decodes every command into an OPL register write or a delay, so a
//! file for other chips has nothing it can show. That is a limitation, not a
//! fault in the file -- and a bare "Failed to load" alert says the opposite.
//! This reports what the file *is*, and points at pack mode, where its tags are
//! editable today.

use std::path::PathBuf;

use dro_core::VgmFile;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct ForeignVgmDialog {
    file_name: String,
    facts: Vec<(&'static str, String)>,
    /// The file's folder, if it has one: the pack this file belongs to.
    folder: Option<PathBuf>,
}

impl ForeignVgmDialog {
    #[must_use]
    pub fn new(file: &VgmFile, folder: Option<PathBuf>) -> Self {
        let mut facts = vec![
            ("Chips", file.chip_list()),
            ("VGM version", file.header.version_string()),
            ("Length", dro_core::util::ms_to_timestr(file.total_ms())),
        ];
        if let Some(samples) = file.loop_samples() {
            facts.push((
                "Loop",
                dro_core::util::ms_to_timestr(dro_core::util::smp_to_ms(
                    samples,
                    dro_core::vgm::VGM_SAMPLE_RATE,
                )),
            ));
        }
        if let Some(title) = file
            .tag
            .as_ref()
            .map(|tag| tag.track_name_en.trim())
            .filter(|title| !title.is_empty())
        {
            facts.push(("Title", title.to_owned()));
        }
        Self {
            file_name: file.name.clone(),
            facts,
            folder,
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
        let open =
            super::dialog_modal(ctx, "foreign-vgm-modal", "Not an OPL song", palette, |ui| {
                ui.label(
                    egui::RichText::new(&self.file_name)
                        .monospace()
                        .color(palette.data_text)
                        .strong(),
                );
                ui.add_space(8.0);

                egui::Grid::new("foreign-vgm-grid")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for (key, value) in &self.facts {
                            ui.colored_label(palette.data_label, *key);
                            ui.label(
                                egui::RichText::new(value)
                                    .monospace()
                                    .color(palette.data_text),
                            );
                            ui.end_row();
                        }
                    });

                ui.add_space(10.0);
                ui.label(
                    "The editor works on OPL2 and OPL3 songs only. Open the folder as a pack \
                     to edit this file's tags.",
                );

                ui.add_space(8.0);
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        keep_open = false;
                    }
                    // Straight to this file's own folder when it has one;
                    // otherwise the picker, which is all the web build can do.
                    let (label, action) = match &self.folder {
                        Some(folder) => (
                            "Open This Folder as a Pack",
                            Action::OpenPackFolderAt(folder.clone()),
                        ),
                        None => ("Open a Pack\u{2026}", Action::OpenPackFolder),
                    };
                    if bevel::button(ui, palette, label)
                        .on_hover_text("Pack mode can edit this file's tags")
                        .clicked()
                    {
                        actions.push(action);
                        keep_open = false;
                    }
                });
            });
        open && keep_open
    }
}
