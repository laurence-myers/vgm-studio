//! The Settings dialog. New in the Rust port: the Python only ever *read*
//! `drotrim.ini`. The web build (Step 8) has no ini file at all, so the same
//! dialog writes through whatever `ConfigStore` the platform injected.

use dro_core::config::{AppConfig, ThemeChoice};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct SettingsDialog {
    /// The config the dialog opened from. `save` starts here, not from the
    /// defaults, so fields the dialog does not expose (e.g. `audio.boost`) are
    /// preserved rather than silently reset.
    original: AppConfig,
    frequency: u32,
    buffer_size: String,
    bit_depth: u16,
    chip_write_delay: String,
    tail_length: String,
    maximize_window: bool,
    dro_info_edit_enabled: bool,
    theme: ThemeChoice,
}

impl SettingsDialog {
    #[must_use]
    pub fn new(config: &AppConfig) -> Self {
        Self {
            original: *config,
            frequency: config.audio.frequency,
            buffer_size: config.audio.buffer_size.to_string(),
            bit_depth: config.audio.bit_depth,
            chip_write_delay: config.audio.chip_write_delay.to_string(),
            tail_length: config.ui.tail_length.to_string(),
            maximize_window: config.ui.maximize_window,
            dro_info_edit_enabled: config.ui.dro_info_edit_enabled,
            theme: config.ui.theme,
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_window(ctx, "Settings", area, |ui| {
            egui::Grid::new("settings-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Frequency")
                        .on_hover_text("49716 Hz is the OPL3's native rate");
                    ui.scope(|ui| {
                        crate::theme::style_dropdown(ui, palette);
                        egui::ComboBox::from_id_salt("settings-frequency")
                            .selected_text(frequency_label(self.frequency))
                            .show_ui(ui, |ui| {
                                for rate in FREQUENCIES {
                                    ui.selectable_value(
                                        &mut self.frequency,
                                        rate,
                                        frequency_label(rate),
                                    );
                                }
                            });
                    });
                    ui.end_row();

                    ui.label("Buffer size");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.buffer_size)
                            .text_color(palette.data_text)
                            .desired_width(100.0),
                    );
                    ui.end_row();

                    ui.label("Bit depth").on_hover_text("WAV export only");
                    ui.scope(|ui| {
                        crate::theme::style_dropdown(ui, palette);
                        egui::ComboBox::from_id_salt("settings-bit-depth")
                            .selected_text(self.bit_depth.to_string())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.bit_depth, 8, "8");
                                ui.selectable_value(&mut self.bit_depth, 16, "16");
                            });
                    });
                    ui.end_row();

                    ui.label("Chip write delay (\u{00b5}s)").on_hover_text(
                        "Microseconds after each chip write, to imitate real hardware. \
                             0 = perfect timing; OPL2 wants at least 26.6.",
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.chip_write_delay)
                            .text_color(palette.data_text)
                            .desired_width(100.0),
                    );
                    ui.end_row();

                    ui.label("Tail length (ms)")
                        .on_hover_text("How much the \"play last X seconds\" button plays");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tail_length)
                            .text_color(palette.data_text)
                            .desired_width(100.0),
                    );
                    ui.end_row();

                    ui.label("Theme")
                        .on_hover_text("Applied on Save; no restart needed");
                    egui::ComboBox::from_id_salt("settings-theme")
                        .selected_text(theme_label(self.theme))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.theme,
                                ThemeChoice::CloneDark,
                                theme_label(ThemeChoice::CloneDark),
                            );
                            ui.selectable_value(
                                &mut self.theme,
                                ThemeChoice::Ft2Classic,
                                theme_label(ThemeChoice::Ft2Classic),
                            );
                        });
                    ui.end_row();

                    ui.label("Maximize window at launch");
                    ui.checkbox(&mut self.maximize_window, "");
                    ui.end_row();

                    ui.label("Allow editing in DRO Info");
                    ui.checkbox(&mut self.dro_info_edit_enabled, "");
                    ui.end_row();
                });
            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Save").clicked() && self.save(actions) {
                    close = true;
                }
            });
        });
        open && !close
    }

    /// Parses, validates and emits the new settings; `false` (with an error
    /// box queued) if anything is invalid.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        // Start from the config the dialog opened with, so fields it does not
        // edit (like `audio.boost`, driven by the transport slider) survive.
        let mut config = self.original;
        let parsed = (
            self.buffer_size.trim().parse::<u32>(),
            self.chip_write_delay.trim().parse::<f64>(),
            self.tail_length.trim().parse::<u32>(),
        );
        let (Ok(buffer_size), Ok(chip_write_delay), Ok(tail_length)) = parsed else {
            actions.push(Action::Alert {
                title: "Invalid settings".to_owned(),
                message: "Check that the entered values are numbers.".to_owned(),
            });
            return false;
        };
        config.audio.frequency = self.frequency;
        config.audio.buffer_size = buffer_size;
        config.audio.bit_depth = self.bit_depth;
        config.audio.chip_write_delay = chip_write_delay;
        config.ui.tail_length = tail_length;
        config.ui.maximize_window = self.maximize_window;
        config.ui.dro_info_edit_enabled = self.dro_info_edit_enabled;
        config.ui.theme = self.theme;

        if let Err(error) = config.validate() {
            actions.push(Action::Alert {
                title: "Invalid settings".to_owned(),
                message: error.to_string(),
            });
            return false;
        }
        actions.push(Action::ApplySettings(Box::new(config)));
        true
    }
}

/// The rates the dropdown offers: CD rate, the usual device rate, and the
/// OPL3's own. Anything else in a hand-edited ini is still shown and kept --
/// see [`frequency_label`] -- it just isn't one of the offered choices.
const FREQUENCIES: [u32; 3] = [44_100, 48_000, 49_716];

/// The dropdown label for a sample rate. Round thousands read as kHz; anything
/// else (the OPL3's 49716, or a hand-edited value) stays in Hz rather than
/// become a misleading "49.7 kHz".
fn frequency_label(rate: u32) -> String {
    match rate {
        44_100 => "44.1 kHz".to_owned(),
        rate if rate.is_multiple_of(1000) => format!("{} kHz", rate / 1000),
        rate => format!("{rate} Hz"),
    }
}

/// The dropdown label for a theme.
fn theme_label(theme: ThemeChoice) -> &'static str {
    match theme {
        ThemeChoice::CloneDark => "Clone (dark)",
        ThemeChoice::Ft2Classic => "FastTracker II (classic)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_labels_read_as_rates() {
        assert_eq!(frequency_label(44_100), "44.1 kHz");
        assert_eq!(frequency_label(48_000), "48 kHz");
        // The OPL3's own rate is not a round number of kHz, and rounding it to
        // "49.7 kHz" would misreport the one value people pick deliberately.
        assert_eq!(frequency_label(49_716), "49716 Hz");
        assert_eq!(frequency_label(22_050), "22050 Hz");
    }

    #[test]
    fn a_rate_outside_the_offered_set_survives_a_save() {
        // The dropdown offers three rates, but a hand-edited drotrim.ini may
        // hold another. Opening Settings and saving something else must not
        // silently retune the output.
        let mut config = AppConfig::default();
        config.audio.frequency = 22_050;
        let mut dialog = SettingsDialog::new(&config);
        assert_eq!(frequency_label(dialog.frequency), "22050 Hz");

        dialog.tail_length = "2000".to_owned();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.frequency, 22_050, "the unlisted rate is kept");
        assert_eq!(saved.ui.tail_length, 2000);
    }

    #[test]
    fn picking_a_rate_applies_it() {
        let dialog_config = AppConfig::default();
        let mut dialog = SettingsDialog::new(&dialog_config);
        dialog.frequency = 49_716;
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.frequency, 49_716);
    }
}
