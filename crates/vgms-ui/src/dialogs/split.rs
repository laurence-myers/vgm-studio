//! The Split Channels dialog: one file per channel the song actually uses, as
//! `drotrim split` writes them.

use vgms_synth::SplitFormat;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct SplitDialog {
    format: SplitFormat,
    isolate_percussion: bool,
    /// A generic VGM splits to WAV per chip channel; the format and percussion
    /// choices are OPL-only, so they are hidden for one.
    wav_only: bool,
}

impl Default for SplitDialog {
    fn default() -> Self {
        Self {
            format: SplitFormat::Wav,
            isolate_percussion: false,
            wav_only: false,
        }
    }
}

impl SplitDialog {
    /// The dialog for an OPL document, with the format and percussion options.
    #[must_use]
    pub fn new(wav_only: bool) -> Self {
        Self {
            wav_only,
            ..Self::default()
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        // The body borrows `self` mutably, so the footer reports clicks through
        // cells and the split is emitted after the call returns.
        let close = std::cell::Cell::new(false);
        let split_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "split-modal",
            "Split Channels",
            palette,
            |ui| {
                // The format and percussion options are OPL ideas: a generic
                // VGM renders each chip channel to WAV, so they are hidden.
                if self.wav_only {
                    ui.label(crate::strings::SPLIT_WAV_ONLY);
                } else {
                    ui.label(crate::strings::SPLIT_WRITE_EACH_AS);
                    ui.add_space(4.0);
                    ui.radio_value(&mut self.format, SplitFormat::Wav, "Audio (WAV)")
                        .on_hover_text(crate::strings::SPLIT_AUDIO_HOVER);
                    ui.radio_value(
                        &mut self.format,
                        SplitFormat::Song,
                        "Song data (DRO or VGM)",
                    )
                    .on_hover_text(crate::strings::SPLIT_SONG_HOVER);

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.isolate_percussion, "");
                        if ui
                            .add(
                                egui::Label::new("Give each drum its own file")
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text(crate::strings::SPLIT_ISOLATE_PERCUSSION_HOVER)
                            .clicked()
                        {
                            self.isolate_percussion = !self.isolate_percussion;
                        }
                    });
                }

                ui.add_space(8.0);
                ui.label(egui::RichText::new(crate::strings::SPLIT_SKIPPED_NOTE).small());
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    if bevel::button(ui, palette, "Split").clicked() {
                        split_clicked.set(true);
                    }
                });
            },
        );
        if split_clicked.get() {
            self.save(actions);
        }
        open && !(close.get() || split_clicked.get())
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
        let dialog = SplitDialog::new(false);
        assert_eq!(dialog.format, SplitFormat::Wav);
        assert!(!dialog.isolate_percussion);
    }

    #[test]
    fn the_defaults_are_what_a_bare_split_requests() {
        let mut actions = Vec::new();
        SplitDialog::new(false).save(&mut actions);
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
        let mut dialog = SplitDialog::new(false);
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
