//! The Render to WAV dialog: what `vgmstudio render` does, plus the mix the editor
//! is set up with.
//!
//! All three options are off by default, which renders exactly what the CLI
//! does -- every voice audible, the song's own stereo image, no boost. Each can
//! be turned on independently; "All of the above" is the one-click "render what
//! I'm hearing".

use crate::action::Action;
use crate::theme::{Palette, bevel};

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
}

impl RenderWavDialog {
    /// `current_boost` seeds the boost field from live playback, so ticking all
    /// three options renders what the user is currently hearing.
    #[must_use]
    pub fn new(current_boost: f32) -> Self {
        Self {
            use_toggles: false,
            use_panning: false,
            use_boost: false,
            boost: format_boost(current_boost),
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
        // cells and the render is emitted after the call returns.
        let close = std::cell::Cell::new(false);
        let render_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "render-wav-modal",
            "Render to WAV",
            palette,
            |ui| {
                ui.label(crate::strings::RENDER_WAV_APPLY);
                ui.add_space(4.0);

                option_row(
                    ui,
                    "Channel toggles",
                    crate::strings::RENDER_WAV_TOGGLES_HOVER,
                    &mut self.use_toggles,
                );
                option_row(
                    ui,
                    "Channel panning",
                    crate::strings::RENDER_WAV_PANNING_HOVER,
                    &mut self.use_panning,
                );

                ui.horizontal(|ui| {
                    option_row(
                        ui,
                        "Boost",
                        crate::strings::RENDER_WAV_BOOST_HOVER,
                        &mut self.use_boost,
                    );
                    ui.add_enabled_ui(self.use_boost, |ui| {
                        super::text_field(ui, palette, &mut self.boost, 44.0).on_hover_text(
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

                ui.add_space(8.0);
                ui.label(egui::RichText::new(crate::strings::RENDER_WAV_FREQ_NOTE).small());
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    if bevel::button(ui, palette, "Render").clicked() {
                        render_clicked.set(true);
                    }
                });
            },
        );
        // Only a clicked Render runs the save; a refused one leaves the dialog open.
        let rendered = render_clicked.get() && self.save(actions);
        open && !(close.get() || rendered)
    }

    /// Emits the render request, or queues an error box and stays open.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        // A boost that is switched off is 1.0 whatever the field says, so a
        // nonsense value in a disabled field cannot block the render.
        let boost = if self.use_boost {
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

        actions.push(Action::RenderWavSubmitted {
            use_toggles: self.use_toggles,
            use_panning: self.use_panning,
            boost,
        });
        true
    }
}

/// A checkbox whose caption toggles it, as the Settings rows do.
fn option_row(ui: &mut egui::Ui, caption: &str, hover: &str, value: &mut bool) {
    ui.horizontal(|ui| {
        ui.checkbox(value, "");
        if ui
            .add(egui::Label::new(caption).sense(egui::Sense::click()))
            .on_hover_text(hover)
            .clicked()
        {
            *value = !*value;
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

    /// The default is the faithful render `vgmstudio render` produces.
    #[test]
    fn nothing_is_applied_by_default() {
        let mut dialog = RenderWavDialog::new(3.0);
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert_eq!(
            actions,
            [Action::RenderWavSubmitted {
                use_toggles: false,
                use_panning: false,
                boost: 1.0,
            }]
        );
    }

    #[test]
    fn the_boost_field_starts_at_the_playback_boost() {
        assert_eq!(RenderWavDialog::new(3.0).boost, "3");
        assert_eq!(RenderWavDialog::new(1.0).boost, "1");
        // Out-of-range values from a hand-edited ini are pulled into range.
        assert_eq!(RenderWavDialog::new(99.0).boost, "16");
    }

    #[test]
    fn each_option_reaches_the_request() {
        let mut dialog = RenderWavDialog::new(1.0);
        dialog.use_toggles = true;
        dialog.use_panning = true;
        dialog.use_boost = true;
        dialog.boost = "2.5".to_owned();

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert_eq!(
            actions,
            [Action::RenderWavSubmitted {
                use_toggles: true,
                use_panning: true,
                boost: 2.5,
            }]
        );
    }

    #[test]
    fn a_boost_left_switched_off_renders_unboosted() {
        // Even with a value typed in: the checkbox is what decides.
        let mut dialog = RenderWavDialog::new(1.0);
        dialog.boost = "8".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let [Action::RenderWavSubmitted { boost, .. }] = actions[..] else {
            panic!("expected a render request, got {actions:?}")
        };
        assert_eq!(boost, 1.0);
    }

    /// A disabled field cannot be a reason to refuse the render.
    #[test]
    fn nonsense_in_a_disabled_boost_field_is_ignored() {
        let mut dialog = RenderWavDialog::new(1.0);
        dialog.boost = "not a number".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(actions[0], Action::RenderWavSubmitted { .. }));
    }

    #[test]
    fn an_invalid_boost_is_refused_with_an_alert() {
        for typed in ["", "nope", "0", "17", "-3"] {
            let mut dialog = RenderWavDialog::new(1.0);
            dialog.use_boost = true;
            dialog.boost = typed.to_owned();

            let mut actions = Vec::new();
            assert!(!dialog.save(&mut actions), "{typed:?} should be refused");
            assert!(
                matches!(actions[0], Action::Alert { .. }),
                "{typed:?} produced {actions:?}"
            );
        }
    }
}
