//! The Render to WAV dialog: what `vgmstudio render` does, plus the mix the editor
//! is set up with.
//!
//! All three options are off by default, which renders exactly what the CLI
//! does -- every voice audible, the song's own stereo image, no boost. Each can
//! be turned on independently; "All of the above" is the one-click "render what
//! I'm hearing".

use std::collections::BTreeMap;

use vgms_core::vgm::ChipKind;

use crate::action::{Action, FileAction, UiAction};
use crate::theme::{Palette, bevel};
use crate::widgets::chip_output;

/// The boost range the audio config accepts. A render at 1.0 is bit-transparent.
const MIN_BOOST: f32 = 1.0;
const MAX_BOOST: f32 = 16.0;

#[derive(Debug)]
pub struct RenderWavDialog {
    /// Mute and solo the channel panel is set to.
    use_toggles: bool,
    /// The channel panel's pan knobs.
    use_panning: bool,
    /// Whether [`Self::boost`] is applied at all.
    use_boost: bool,
    /// Held as text so a half-typed value is not clamped out from under the
    /// user; parsed and range-checked on Render.
    boost: String,
    /// The document's chip slots, for the core picker rows.
    chips: Vec<ChipKind>,
    /// The core chosen per chip slot for this render, seeded from Settings and
    /// edited in place by the picker. Rides the request; never persisted.
    cores: BTreeMap<String, String>,
}

impl RenderWavDialog {
    /// `current_boost` seeds the boost field from live playback, so ticking all
    /// three options renders what the user is currently hearing. The channel
    /// toggle and panning options apply to every document now -- an OPL render
    /// gates registers, a generic one carries per-chip masks -- so they are
    /// always offered. `chips` are the document's chip slots and `settings_cores`
    /// the current Settings core map, which together seed the per-render core
    /// picker (session-sticky, never written to vgmstudio.ini).
    #[must_use]
    pub fn new(
        current_boost: f32,
        chips: Vec<ChipKind>,
        settings_cores: BTreeMap<String, String>,
    ) -> Self {
        Self {
            use_toggles: false,
            use_panning: false,
            use_boost: false,
            boost: format_boost(current_boost),
            chips,
            cores: settings_cores,
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::new(palette, "Render");
        let open = super::dialog_modal(
            ctx,
            "render-wav-modal",
            "Render to WAV",
            palette,
            |ui| {
                ui.label(crate::strings::RENDER_WAV_APPLY);
                ui.add_space(4.0);

                // The channel toggle/pan mix applies to either document: an OPL
                // render gates registers, a generic one carries per-chip masks.
                super::caption_checkbox(
                    ui,
                    "Channel toggles",
                    crate::strings::RENDER_WAV_TOGGLES_HOVER,
                    &mut self.use_toggles,
                    super::CaptionSide::Row,
                );
                super::caption_checkbox(
                    ui,
                    "Channel panning",
                    crate::strings::RENDER_WAV_PANNING_HOVER,
                    &mut self.use_panning,
                    super::CaptionSide::Row,
                );

                ui.horizontal(|ui| {
                    super::caption_checkbox(
                        ui,
                        "Boost",
                        crate::strings::RENDER_WAV_BOOST_HOVER,
                        &mut self.use_boost,
                        super::CaptionSide::Row,
                    );
                    ui.add_enabled_ui(self.use_boost, |ui| {
                        super::text_field(ui, palette, &mut self.boost, 80.0).on_hover_text(
                            crate::strings::render_wav_boost_range(MIN_BOOST, MAX_BOOST),
                        );
                        ui.label("x");
                    });
                });

                ui.add_space(6.0);
                if bevel::button(ui, palette, "All of the above").clicked() {
                    self.use_toggles = true;
                    self.use_panning = true;
                    self.use_boost = true;
                }

                core_picker(ui, palette, &self.chips, &mut self.cores);

                ui.add_space(8.0);
                ui.label(egui::RichText::new(crate::strings::RENDER_WAV_FREQ_NOTE).small());
            },
            |ui| footer.show(ui),
        );
        // Only a clicked Render runs the save; a refused one leaves the dialog open.
        let rendered = footer.primary_clicked() && self.save(actions);
        open && !(footer.closed() || rendered)
    }

    /// Emits the render request, or queues an error box and stays open.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        // A boost that is switched off is 1.0 whatever the field says, so a
        // nonsense value in a disabled field cannot block the render.
        let boost = if self.use_boost {
            match self.boost.trim().parse::<f32>() {
                Ok(boost) if (MIN_BOOST..=MAX_BOOST).contains(&boost) => boost,
                _ => {
                    actions.push(Action::Ui(UiAction::Alert {
                        title: crate::strings::RENDER_WAV_INVALID_TITLE.to_owned(),
                        message: crate::strings::render_wav_boost_message(MIN_BOOST, MAX_BOOST),
                    }));
                    return false;
                }
            }
        } else {
            1.0
        };

        actions.push(Action::File(FileAction::RenderWavSubmitted {
            use_toggles: self.use_toggles,
            use_panning: self.use_panning,
            boost,
            core_choices: self.cores.clone(),
        }));
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
    let plan = chip_output::plan(chips, chip_output::opl_split(cores));
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
    ui.label(crate::strings::RENDER_WAV_CORE)
        .on_hover_text(crate::strings::RENDER_WAV_CORE_HOVER);
    ui.add_space(4.0);
    egui::Grid::new("render-wav-core-grid")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for entry in choosable {
                chip_output::song_chip_row(ui, palette, "render", cores, entry);
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

    /// A dialog for a document with no core-picker seed -- the render options
    /// under test do not touch the picker, which is exercised on its own below.
    fn dialog(boost: f32) -> RenderWavDialog {
        RenderWavDialog::new(boost, Vec::new(), BTreeMap::new())
    }

    /// The default is the faithful render `vgmstudio render` produces.
    #[test]
    fn nothing_is_applied_by_default() {
        let mut dialog = dialog(3.0);
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert_eq!(
            actions,
            [Action::File(FileAction::RenderWavSubmitted {
                use_toggles: false,
                use_panning: false,
                boost: 1.0,
                core_choices: BTreeMap::new(),
            })]
        );
    }

    #[test]
    fn the_boost_field_starts_at_the_playback_boost() {
        assert_eq!(dialog(3.0).boost, "3");
        assert_eq!(dialog(1.0).boost, "1");
        // Out-of-range values from a hand-edited ini are pulled into range.
        assert_eq!(dialog(99.0).boost, "16");
    }

    #[test]
    fn each_option_reaches_the_request() {
        let mut dialog = dialog(1.0);
        dialog.use_toggles = true;
        dialog.use_panning = true;
        dialog.use_boost = true;
        dialog.boost = "2.5".to_owned();

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert_eq!(
            actions,
            [Action::File(FileAction::RenderWavSubmitted {
                use_toggles: true,
                use_panning: true,
                boost: 2.5,
                core_choices: BTreeMap::new(),
            })]
        );
    }

    /// The picker's chosen core rides the request, seeded from Settings and
    /// carried without ever touching the saved config.
    #[test]
    fn the_picker_core_reaches_the_request() {
        // Seed as the constructor would from Settings, then edit as the picker
        // does; the edited map is what the render must carry.
        let mut dialog = RenderWavDialog::new(1.0, Vec::new(), BTreeMap::new());
        dialog.cores.insert("opl3".to_owned(), "cqm".to_owned());

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let [Action::File(FileAction::RenderWavSubmitted { core_choices, .. })] = &actions[..]
        else {
            panic!("expected a render request, got {actions:?}")
        };
        assert_eq!(core_choices.get("opl3").map(String::as_str), Some("cqm"));
    }

    #[test]
    fn a_boost_left_switched_off_renders_unboosted() {
        // Even with a value typed in: the checkbox is what decides.
        let mut dialog = dialog(1.0);
        dialog.boost = "8".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let [Action::File(FileAction::RenderWavSubmitted { boost, .. })] = actions[..] else {
            panic!("expected a render request, got {actions:?}")
        };
        assert_eq!(boost, 1.0);
    }

    /// A disabled field cannot be a reason to refuse the render.
    #[test]
    fn nonsense_in_a_disabled_boost_field_is_ignored() {
        let mut dialog = dialog(1.0);
        dialog.boost = "not a number".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions[0],
            Action::File(FileAction::RenderWavSubmitted { .. })
        ));
    }

    #[test]
    fn an_invalid_boost_is_refused_with_an_alert() {
        for typed in ["", "nope", "0", "17", "-3"] {
            let mut dialog = dialog(1.0);
            dialog.use_boost = true;
            dialog.boost = typed.to_owned();

            let mut actions = Vec::new();
            assert!(!dialog.save(&mut actions), "{typed:?} should be refused");
            assert!(
                matches!(actions[0], Action::Ui(UiAction::Alert { .. })),
                "{typed:?} produced {actions:?}"
            );
        }
    }
}
