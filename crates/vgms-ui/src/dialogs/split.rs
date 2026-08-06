//! The Split Channels dialog: one file per channel the song actually uses, as
//! `vgmstudio split` writes them.

use std::collections::BTreeMap;

use vgms_core::vgm::ChipKind;
use vgms_synth::{ChannelGate, SplitFormat};

use crate::action::Action;
use crate::theme::Palette;
use crate::widgets::chip_output;

/// The boost range the audio config accepts. A stem at 1.0 is bit-transparent.
const MIN_BOOST: f32 = 1.0;
const MAX_BOOST: f32 = 16.0;

#[derive(Debug)]
pub struct SplitDialog {
    format: SplitFormat,
    /// Whether the Song format is offered: an OPL document always is (its
    /// projection splits to a per-channel VGM), and a generic VGM is when at
    /// least one of its chips has a write-gate table (see [`ChannelGate::exists`]).
    /// When false, the split is WAV-only and the format radio is replaced by a
    /// static note.
    song_capable: bool,
    /// Skip the channels the mixer has muted (decision 9): a live mute leaves a
    /// channel out of the output set. Applies to both formats.
    use_skip_muted: bool,
    /// Apply the mixer's pan knobs to each rendered stem. WAV renders only.
    use_panning: bool,
    /// Whether [`Self::boost`] is applied at all. WAV renders only.
    use_boost: bool,
    /// Held as text so a half-typed value is not clamped out from under the
    /// user; parsed and range-checked on Split.
    boost: String,
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
            song_capable: true,
            use_skip_muted: false,
            use_panning: false,
            use_boost: false,
            boost: format_boost(1.0),
            chips: Vec::new(),
            cores: BTreeMap::new(),
        }
    }
}

impl SplitDialog {
    /// The dialog for the loaded document. `is_opl` marks an OPL document, which
    /// is always Song-capable; the Song format is otherwise offered for a VGM
    /// whose chips a write-gate covers. `current_boost` seeds the boost field from
    /// live playback. `chips` are the document's chip slots and `settings_cores`
    /// the current Settings core map; together they seed the per-render core
    /// picker (session-sticky, never written to vgmstudio.ini).
    #[must_use]
    pub fn new(
        is_opl: bool,
        chips: Vec<ChipKind>,
        settings_cores: BTreeMap<String, String>,
        current_boost: f32,
    ) -> Self {
        let song_capable = is_opl || chips.iter().any(|&kind| ChannelGate::exists(kind));
        Self {
            song_capable,
            boost: format_boost(current_boost),
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
        let footer = super::Footer::new(palette, "Split");
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
                    // Every song-format stem is a VGM now (ou-4): a DRO projects,
                    // an OPL/multichip VGM keeps its own format, which is a VGM.
                    ui.radio_value(&mut self.format, SplitFormat::Song, "Song data (VGM)")
                        .on_hover_text(crate::strings::SPLIT_SONG_HOVER);
                } else {
                    ui.label(crate::strings::SPLIT_WAV_ONLY);
                }

                // The mix opt-ins, all off = the faithful split. Skipping muted
                // channels applies to both formats; panning and boost are
                // render-time, so they are offered only for a WAV split.
                ui.add_space(8.0);
                ui.label(crate::strings::SPLIT_MIX_APPLY);
                ui.add_space(4.0);
                super::caption_checkbox(
                    ui,
                    "Skip muted channels",
                    crate::strings::SPLIT_SKIP_MUTED_HOVER,
                    &mut self.use_skip_muted,
                    super::CaptionSide::Row,
                );
                if self.format == SplitFormat::Wav {
                    super::caption_checkbox(
                        ui,
                        "Channel panning",
                        crate::strings::SPLIT_PANNING_HOVER,
                        &mut self.use_panning,
                        super::CaptionSide::Row,
                    );
                    ui.horizontal(|ui| {
                        super::caption_checkbox(
                            ui,
                            "Boost",
                            crate::strings::SPLIT_BOOST_HOVER,
                            &mut self.use_boost,
                            super::CaptionSide::Row,
                        );
                        ui.add_enabled_ui(self.use_boost, |ui| {
                            super::text_field(ui, palette, &mut self.boost, 44.0).on_hover_text(
                                crate::strings::render_wav_boost_range(MIN_BOOST, MAX_BOOST),
                            );
                            ui.label("x");
                        });
                    });
                }

                core_picker(ui, palette, &self.chips, &mut self.cores);

                ui.add_space(8.0);
                ui.label(egui::RichText::new(crate::strings::SPLIT_SKIPPED_NOTE).small());
            },
            |ui| footer.show(ui),
        );
        // Only a clicked Split with a valid boost runs the save; a refused one
        // leaves the dialog open (like the Render dialog).
        let split_done = footer.primary_clicked() && self.save(actions);
        open && !(footer.closed() || split_done)
    }

    /// Emits the split request, or queues an error box and stays open when the
    /// boost field is enabled but out of range. Pan and boost are dropped for a
    /// song split, which cannot render them.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        let is_wav = self.format == SplitFormat::Wav;
        let boost = if self.use_boost && is_wav {
            match self.boost.trim().parse::<f32>() {
                Ok(boost) if (MIN_BOOST..=MAX_BOOST).contains(&boost) => boost,
                _ => {
                    actions.push(Action::Alert {
                        title: crate::strings::RENDER_WAV_INVALID_TITLE.to_owned(),
                        message: crate::strings::render_wav_boost_message(MIN_BOOST, MAX_BOOST),
                    });
                    return false;
                }
            }
        } else {
            1.0
        };
        actions.push(Action::SplitSubmitted {
            format: self.format,
            use_skip_muted: self.use_skip_muted,
            use_panning: self.use_panning && is_wav,
            boost,
            core_choices: self.cores.clone(),
        });
        true
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

/// Shows a whole-number boost without a pointless `.0`.
fn format_boost(boost: f32) -> String {
    let clamped = boost.clamp(MIN_BOOST, MAX_BOOST);
    if clamped.fract() == 0.0 {
        format!("{clamped:.0}")
    } else {
        format!("{clamped}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OPL dialog with no core-picker seed -- the format and mix options under
    /// test do not touch the picker, which is exercised on its own below.
    fn dialog(is_opl: bool) -> SplitDialog {
        SplitDialog::new(is_opl, Vec::new(), BTreeMap::new(), 1.0)
    }

    /// A WAV split with no opt-ins: what `vgmstudio split` does with no flags.
    #[test]
    fn the_defaults_match_the_bare_cli_command() {
        let dialog = dialog(true);
        assert_eq!(dialog.format, SplitFormat::Wav);
        assert!(!dialog.use_skip_muted && !dialog.use_panning && !dialog.use_boost);
    }

    #[test]
    fn the_defaults_are_what_a_bare_split_requests() {
        let mut actions = Vec::new();
        assert!(dialog(true).save(&mut actions));
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Wav,
                use_skip_muted: false,
                use_panning: false,
                boost: 1.0,
                core_choices: BTreeMap::new(),
            }]
        );
    }

    #[test]
    fn the_chosen_options_reach_the_request() {
        let mut dialog = dialog(true);
        dialog.format = SplitFormat::Song;
        dialog.use_skip_muted = true;

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert_eq!(
            actions,
            [Action::SplitSubmitted {
                format: SplitFormat::Song,
                use_skip_muted: true,
                // Pan/boost are dropped for a song split, which cannot render.
                use_panning: false,
                boost: 1.0,
                core_choices: BTreeMap::new(),
            }]
        );
    }

    /// The pan and boost opt-ins reach a WAV split's request.
    #[test]
    fn the_wav_mix_opt_ins_reach_the_request() {
        let mut dialog = dialog(true);
        dialog.use_panning = true;
        dialog.use_boost = true;
        dialog.boost = "2.5".to_owned();

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let [
            Action::SplitSubmitted {
                use_panning, boost, ..
            },
        ] = actions[..]
        else {
            panic!("expected a split request, got {actions:?}")
        };
        assert!(use_panning);
        assert_eq!(boost, 2.5);
    }

    /// Pan and boost are dropped when the song format is chosen -- even if the
    /// (now-hidden) toggles were left on.
    #[test]
    fn a_song_split_drops_pan_and_boost() {
        let mut dialog = dialog(true);
        dialog.format = SplitFormat::Song;
        dialog.use_panning = true;
        dialog.use_boost = true;
        dialog.boost = "not a number".to_owned(); // must not block the split

        let mut actions = Vec::new();
        assert!(
            dialog.save(&mut actions),
            "a song split ignores the boost field"
        );
        let [
            Action::SplitSubmitted {
                use_panning, boost, ..
            },
        ] = actions[..]
        else {
            panic!("expected a split request, got {actions:?}")
        };
        assert!(!use_panning);
        assert_eq!(boost, 1.0);
    }

    /// An enabled but out-of-range boost is refused with an alert (WAV only).
    #[test]
    fn an_invalid_boost_is_refused() {
        for typed in ["", "nope", "0", "17"] {
            let mut dialog = dialog(true);
            dialog.use_boost = true;
            dialog.boost = typed.to_owned();

            let mut actions = Vec::new();
            assert!(!dialog.save(&mut actions), "{typed:?} should be refused");
            assert!(matches!(actions[0], Action::Alert { .. }));
        }
    }

    /// An OPL document offers the Song format; so does a VGM with a gate-covered
    /// chip; a VGM with none is WAV-only.
    #[test]
    fn the_song_format_is_offered_when_a_channel_can_be_rewritten() {
        assert!(dialog(true).song_capable, "an OPL document is captured");
        assert!(
            SplitDialog::new(false, vec![ChipKind::Sn76489], BTreeMap::new(), 1.0).song_capable,
            "a gated chip can be rewritten to song data"
        );
        assert!(
            !SplitDialog::new(false, vec![ChipKind::C352], BTreeMap::new(), 1.0).song_capable,
            "an ungated chip is WAV-only"
        );
        // A mix offers the format -- the split refuses the ungated chip per-chip.
        assert!(
            SplitDialog::new(
                false,
                vec![ChipKind::C352, ChipKind::Ym2612],
                BTreeMap::new(),
                1.0,
            )
            .song_capable
        );
    }

    /// An OPL document is always Song-capable (its projection splits to a
    /// per-channel VGM); a non-OPL document with no gate-covered chip is WAV-only.
    #[test]
    fn an_opl_document_is_song_capable() {
        assert!(dialog(true).song_capable);
        assert!(!SplitDialog::new(false, Vec::new(), BTreeMap::new(), 1.0).song_capable);
    }

    /// The picker's chosen core rides the split request, seeded from Settings
    /// and carried without ever touching the saved config.
    #[test]
    fn the_picker_core_reaches_the_request() {
        let mut dialog = SplitDialog::new(true, Vec::new(), BTreeMap::new(), 1.0);
        dialog.cores.insert("opl3".to_owned(), "cqm".to_owned());

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let [Action::SplitSubmitted { core_choices, .. }] = &actions[..] else {
            panic!("expected a split request, got {actions:?}")
        };
        assert_eq!(core_choices.get("opl3").map(String::as_str), Some("cqm"));
    }
}
