//! Pack mode's per-track optimiser-options dialog.
//!
//! The optimiser knobs (which optimiser, and the two external tool stages) are a
//! global default in Settings; this overrides them for one track, so a pack can
//! optimise each song its own way. Save writes the override; "Use global default"
//! clears it back to Settings. Keyed by the file name the dialog opened on, so a
//! rescan that reorders the list still targets the right track.

use vgms_core::config::{OptimizeOptions, OptimizerChoice};

use crate::action::{Action, PackAction};
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct TrackOptimizeDialog {
    file_name: String,
    options: OptimizeOptions,
    /// Whether the track already had an override when opened, so "Use global
    /// default" is only offered when there is one to clear.
    had_override: bool,
}

impl TrackOptimizeDialog {
    #[must_use]
    pub fn new(file_name: String, options: OptimizeOptions, had_override: bool) -> Self {
        Self {
            file_name,
            options,
            had_override,
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::new(palette, "Save");
        let mut use_global = false;
        let open = super::dialog_modal(
            ctx,
            "track-optimize-modal",
            "Track Optimize Options",
            palette,
            |ui| {
                ui.label(crate::strings::track_optimize_intro(&self.file_name));
                ui.add_space(6.0);
                egui::Grid::new("track-optimize-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Optimizer")
                            .on_hover_text(crate::strings::SETTINGS_OPTIMIZER_HOVER);
                        ui.scope(|ui| {
                            crate::theme::style_dropdown(ui, palette);
                            egui::ComboBox::from_id_salt("track-optimizer")
                                .selected_text(self.options.optimizer.label())
                                .show_ui(ui, |ui| {
                                    for choice in OptimizerChoice::ALL {
                                        ui.selectable_value(
                                            &mut self.options.optimizer,
                                            choice,
                                            choice.label(),
                                        );
                                    }
                                });
                        });
                        ui.end_row();

                        // The tool stages only bite when the external tools run,
                        // so they grey out for the built-in-only choice.
                        let tools_run = self.options.optimizer != OptimizerChoice::BuiltInOnly;
                        ui.label("Tool stages")
                            .on_hover_text(crate::strings::SETTINGS_TOOL_STAGES_HOVER);
                        ui.add_enabled_ui(tools_run, |ui| {
                            ui.vertical(|ui| {
                                ui.checkbox(
                                    &mut self.options.sample_roms,
                                    "Trim sample ROMs (vgm_sro)",
                                )
                                .on_hover_text(crate::strings::SETTINGS_SAMPLE_ROMS_HOVER);
                                ui.checkbox(
                                    &mut self.options.dac_runs,
                                    "Collapse DAC runs (optdac)",
                                )
                                .on_hover_text(crate::strings::SETTINGS_DAC_RUNS_HOVER);
                            });
                        });
                        ui.end_row();
                    });
                if self.had_override {
                    ui.add_space(6.0);
                    if bevel::button(ui, palette, "Use global default")
                        .on_hover_text(crate::strings::TRACK_OPTIMIZE_USE_GLOBAL_HINT)
                        .clicked()
                    {
                        use_global = true;
                    }
                }
            },
            |ui| footer.show(ui),
        );

        // "Use global default" clears the override and closes.
        if use_global {
            actions.push(Action::Pack(PackAction::SetTrackOptimizeOptions {
                file_name: self.file_name.clone(),
                options: None,
            }));
            return false;
        }
        let saved = footer.primary_clicked();
        if saved {
            actions.push(Action::Pack(PackAction::SetTrackOptimizeOptions {
                file_name: self.file_name.clone(),
                options: Some(self.options),
            }));
        }
        open && !(footer.closed() || saved)
    }
}
