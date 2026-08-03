//! The Split Channels dialog: one file per channel the song actually uses, as
//! `vgmstudio split` writes them.

use std::collections::BTreeMap;

use vgms_core::vgm::ChipKind;
use vgms_synth::{ChannelGate, SplitFormat};

use crate::action::Action;
use crate::theme::{Palette, bevel};
use crate::widgets::chip_output;

#[derive(Debug)]
pub struct SplitDialog {
    format: SplitFormat,
    isolate_percussion: bool,
    /// Whether the Song format is offered: an OPL document always is (it is
    /// captured), and a generic VGM is when at least one of its chips has a
    /// write-gate table (see [`ChannelGate::exists`]). When false, the split is
    /// WAV-only and the format radio is replaced by a static note.
    song_capable: bool,
    /// Whether this is an OPL document, so the percussion ("each drum its own
    /// file") option -- an OPL idea -- is offered.
    is_opl: bool,
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
            song_capable: true,
            is_opl: true,
            chips: Vec::new(),
            cores: BTreeMap::new(),
        }
    }
}

impl SplitDialog {
    /// The dialog for the loaded document. `is_opl` decides the OPL-only
    /// percussion option; the Song format is offered for an OPL document or a
    /// VGM whose chips a write-gate covers. `chips` are the document's chip slots
    /// and `settings_cores` the current Settings core map; together they seed the
    /// per-render core picker (session-sticky, never written to vgmstudio.ini).
    #[must_use]
    pub fn new(
        is_opl: bool,
        chips: Vec<ChipKind>,
        settings_cores: BTreeMap<String, String>,
    ) -> Self {
        let song_capable = is_opl || chips.iter().any(|&kind| ChannelGate::exists(kind));
        Self {
            song_capable,
            is_opl,
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
                // The format radio is offered when a Song split is possible (an
                // OPL document, or a VGM with a gate-covered chip); otherwise the
                // split is WAV-only.
                if self.song_capable {
                    ui.label(crate::strings::SPLIT_WRITE_EACH_AS);
                    ui.add_space(4.0);
                    ui.radio_value(&mut self.format, SplitFormat::Wav, "Audio (WAV)")
                        .on_hover_text(crate::strings::SPLIT_AUDIO_HOVER);
                    let song_label = if self.is_opl {
                        "Song data (DRO or VGM)"
                    } else {
                        "Song data (VGM)"
                    };
                    ui.radio_value(&mut self.format, SplitFormat::Song, song_label)
                        .on_hover_text(crate::strings::SPLIT_SONG_HOVER);
                } else {
                    ui.label(crate::strings::SPLIT_WAV_ONLY);
                }

                // Percussion isolation is an OPL idea (the five rhythm voices of
                // register 0xBD), so it is offered for OPL documents only.
                if self.is_opl {
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

    /// An OPL dialog with no core-picker seed -- the format/percussion options
    /// under test do not touch the picker, which is exercised on its own below.
    fn dialog(is_opl: bool) -> SplitDialog {
        SplitDialog::new(is_opl, Vec::new(), BTreeMap::new())
    }

    /// A WAV split of the whole percussion channel: what `vgmstudio split` does
    /// with no flags.
    #[test]
    fn the_defaults_match_the_bare_cli_command() {
        let dialog = dialog(true);
        assert_eq!(dialog.format, SplitFormat::Wav);
        assert!(!dialog.isolate_percussion);
    }

    #[test]
    fn the_defaults_are_what_a_bare_split_requests() {
        let mut actions = Vec::new();
        dialog(true).save(&mut actions);
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
        let mut dialog = dialog(true);
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

    /// An OPL document offers the Song format; so does a VGM with a gate-covered
    /// chip; a VGM with none is WAV-only.
    #[test]
    fn the_song_format_is_offered_when_a_channel_can_be_rewritten() {
        assert!(dialog(true).song_capable, "an OPL document is captured");
        assert!(
            SplitDialog::new(false, vec![ChipKind::Sn76489], BTreeMap::new()).song_capable,
            "a gated chip can be rewritten to song data"
        );
        assert!(
            !SplitDialog::new(false, vec![ChipKind::C352], BTreeMap::new()).song_capable,
            "an ungated chip is WAV-only"
        );
        // A mix offers the format -- the split refuses the ungated chip per-chip.
        assert!(
            SplitDialog::new(
                false,
                vec![ChipKind::C352, ChipKind::Ym2612],
                BTreeMap::new()
            )
            .song_capable
        );
    }

    /// The percussion option is OPL-only, whatever the VGM's chips.
    #[test]
    fn percussion_is_offered_for_opl_documents_only() {
        assert!(dialog(true).is_opl);
        assert!(!SplitDialog::new(false, vec![ChipKind::Ym2612], BTreeMap::new()).is_opl);
    }

    /// The picker's chosen core rides the split request, seeded from Settings
    /// and carried without ever touching the saved config.
    #[test]
    fn the_picker_core_reaches_the_request() {
        let mut dialog = SplitDialog::new(true, Vec::new(), BTreeMap::new());
        dialog.cores.insert("opl3".to_owned(), "cqm".to_owned());

        let mut actions = Vec::new();
        dialog.save(&mut actions);
        let [Action::SplitSubmitted { core_choices, .. }] = &actions[..] else {
            panic!("expected a split request, got {actions:?}")
        };
        assert_eq!(core_choices.get("opl3").map(String::as_str), Some("cqm"));
    }
}
