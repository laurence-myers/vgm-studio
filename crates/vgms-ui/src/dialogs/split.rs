//! The Split Channels dialog: one file per channel the song actually uses, as
//! `vgmstudio split` writes them.

use std::collections::BTreeMap;

use vgms_core::vgm::ChipKind;
use vgms_synth::SplitFormat;

use crate::action::Action;
use crate::theme::{Palette, bevel};
use crate::widgets::chip_output;

#[derive(Debug)]
pub struct SplitDialog {
    format: SplitFormat,
    isolate_percussion: bool,
    /// A generic VGM splits to WAV per chip channel; the format and percussion
    /// choices are OPL-only, so they are hidden for one.
    wav_only: bool,
    /// The document's chip slots, for the core picker rows.
    chips: Vec<ChipKind>,
    /// The core chosen per chip slot for this split, seeded from Settings and
    /// edited in place by the picker. Rides the request; never persisted.
    cores: BTreeMap<String, String>,
}

impl Default for SplitDialog {
    fn default() -> Self {
        Self {
            format: SplitFormat::Wav,
            isolate_percussion: false,
            wav_only: false,
            chips: Vec::new(),
            cores: BTreeMap::new(),
        }
    }
}

impl SplitDialog {
    /// The dialog for an OPL document, with the format and percussion options.
    /// `chips` are the document's chip slots and `settings_cores` the current
    /// Settings core map; together they seed the per-render core picker
    /// (session-sticky, never written to vgmstudio.ini).
    #[must_use]
    pub fn new(
        wav_only: bool,
        chips: Vec<ChipKind>,
        settings_cores: BTreeMap<String, String>,
    ) -> Self {
        Self {
            wav_only,
            chips,
            cores: settings_cores,
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

                core_picker(ui, palette, &self.chips, &mut self.cores);

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
            core_choices: self.cores.clone(),
        });
    }
}

/// Draws the per-render core picker: one row per document chip slot that offers
/// a choice, seeded from Settings. A document whose chips each have a single core
/// draws nothing, so the dialog looks exactly as it did before.
fn core_picker(
    ui: &mut egui::Ui,
    palette: &Palette,
    chips: &[ChipKind],
    cores: &mut BTreeMap<String, String>,
) {
    let plan = chip_output::plan(chips);
    let choosable: Vec<&chip_output::SongChipRow> = plan
        .song
        .iter()
        .filter(|entry| {
            entry
                .row
                .as_ref()
                .is_some_and(chip_output::ChipOutputRow::is_choice)
        })
        .collect();
    if choosable.is_empty() {
        return;
    }
    ui.add_space(8.0);
    ui.label(crate::strings::SPLIT_CORE)
        .on_hover_text(crate::strings::SPLIT_CORE_HOVER);
    ui.add_space(4.0);
    egui::Grid::new("split-core-grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for entry in choosable {
                chip_output::song_chip_row(ui, palette, "split", cores, entry);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialog with no core-picker seed -- the format/percussion options under
    /// test do not touch the picker, which is exercised on its own below.
    fn dialog(wav_only: bool) -> SplitDialog {
        SplitDialog::new(wav_only, Vec::new(), BTreeMap::new())
    }

    /// A WAV split of the whole percussion channel: what `vgmstudio split` does
    /// with no flags.
    #[test]
    fn the_defaults_match_the_bare_cli_command() {
        let dialog = dialog(false);
        assert_eq!(dialog.format, SplitFormat::Wav);
        assert!(!dialog.isolate_percussion);
    }

    #[test]
    fn the_defaults_are_what_a_bare_split_requests() {
        let mut actions = Vec::new();
        dialog(false).save(&mut actions);
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Wav,
                isolate_percussion: false,
                core_choices: BTreeMap::new(),
            }]
        );
    }

    #[test]
    fn the_chosen_options_reach_the_request() {
        let mut dialog = dialog(false);
        dialog.format = SplitFormat::Song;
        dialog.isolate_percussion = true;

        let mut actions = Vec::new();
        dialog.save(&mut actions);
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Song,
                isolate_percussion: true,
                core_choices: BTreeMap::new(),
            }]
        );
    }

    /// The picker's chosen core rides the split request, seeded from Settings
    /// and carried without ever touching the saved config.
    #[test]
    fn the_picker_core_reaches_the_request() {
        let mut dialog = SplitDialog::new(false, Vec::new(), BTreeMap::new());
        dialog.cores.insert("opl3".to_owned(), "cqm".to_owned());

        let mut actions = Vec::new();
        dialog.save(&mut actions);
        let [Action::SplitSubmitted { core_choices, .. }] = &actions[..] else {
            panic!("expected a split request, got {actions:?}")
        };
        assert_eq!(core_choices.get("opl3").map(String::as_str), Some("cqm"));
    }
}
