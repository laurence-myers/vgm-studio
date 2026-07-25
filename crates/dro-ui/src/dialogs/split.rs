//! The Split Channels dialog: one file per channel the song actually uses, as
//! `drotrim split` writes them.

use dro_synth::SplitFormat;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct SplitDialog {
    format: SplitFormat,
    isolate_percussion: bool,
}

impl Default for SplitDialog {
    fn default() -> Self {
        Self {
            format: SplitFormat::Wav,
            isolate_percussion: false,
        }
    }
}

impl SplitDialog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_modal(ctx, "split-modal", "Split Channels", palette, |ui| {
            ui.label("Write each channel as:");
            ui.add_space(4.0);
            ui.radio_value(&mut self.format, SplitFormat::Wav, "Audio (WAV)")
                .on_hover_text("Render each channel on its own");
            ui.radio_value(
                &mut self.format,
                SplitFormat::Song,
                "Song data (DRO or VGM)",
            )
            .on_hover_text("Re-record each channel in the song's own format");

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.isolate_percussion, "");
                if ui
                    .add(
                        egui::Label::new("Give each drum its own file").sense(egui::Sense::click()),
                    )
                    .on_hover_text("Splits the percussion channel per drum, not as one")
                    .clicked()
                {
                    self.isolate_percussion = !self.isolate_percussion;
                }
            });

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("Channels the song never uses are skipped.\nFiles already in the chosen folder are overwritten.")
                    .small(),
            );

            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Split").clicked() {
                    self.save(actions);
                    close = true;
                }
            });
        });
        open && !close
    }

    /// Emits the split request. Nothing to validate: both options are choices,
    /// not typed values, and where to put the files is asked next.
    fn save(&self, actions: &mut Vec<Action>) {
        actions.push(Action::SplitSubmitted {
            format: self.format,
            isolate_percussion: self.isolate_percussion,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV split of the whole percussion channel: what `drotrim split` does
    /// with no flags.
    #[test]
    fn the_defaults_match_the_bare_cli_command() {
        let dialog = SplitDialog::new();
        assert_eq!(dialog.format, SplitFormat::Wav);
        assert!(!dialog.isolate_percussion);
    }

    #[test]
    fn the_defaults_are_what_a_bare_split_requests() {
        let mut actions = Vec::new();
        SplitDialog::new().save(&mut actions);
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Wav,
                isolate_percussion: false,
            }]
        );
    }

    #[test]
    fn the_chosen_options_reach_the_request() {
        let mut dialog = SplitDialog::new();
        dialog.format = SplitFormat::Song;
        dialog.isolate_percussion = true;

        let mut actions = Vec::new();
        dialog.save(&mut actions);
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Song,
                isolate_percussion: true,
            }]
        );
    }
}
