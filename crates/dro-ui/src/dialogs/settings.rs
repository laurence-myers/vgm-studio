//! The Settings dialog. The web build (Step 8) has no ini file at all, so the
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
    buffer_size: u32,
    bit_depth: u16,
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
            buffer_size: config.audio.buffer_size,
            bit_depth: config.audio.bit_depth,
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

                    ui.label("Buffer size").on_hover_text(
                        "Frames per audio callback. Smaller responds to seeking and \
                         muting sooner; larger is safer against dropouts.",
                    );
                    ui.scope(|ui| {
                        crate::theme::style_dropdown(ui, palette);
                        egui::ComboBox::from_id_salt("settings-buffer-size")
                            .selected_text(self.buffer_size.to_string())
                            .show_ui(ui, |ui| {
                                for size in BUFFER_SIZES {
                                    ui.selectable_value(
                                        &mut self.buffer_size,
                                        size,
                                        size.to_string(),
                                    );
                                }
                            });
                    });
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
                            for choice in ThemeChoice::ALL {
                                ui.selectable_value(&mut self.theme, choice, theme_label(choice));
                            }
                        });
                    ui.end_row();

                    checkbox_row(ui, "Maximize window at launch", &mut self.maximize_window);
                    ui.end_row();

                    checkbox_row(
                        ui,
                        "Allow editing in DRO Info",
                        &mut self.dro_info_edit_enabled,
                    );
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
        let Ok(tail_length) = self.tail_length.trim().parse::<u32>() else {
            actions.push(Action::Alert {
                title: "Invalid settings".to_owned(),
                message: "Check that the entered values are numbers.".to_owned(),
            });
            return false;
        };
        config.audio.frequency = self.frequency;
        config.audio.buffer_size = self.buffer_size;
        config.audio.bit_depth = self.bit_depth;
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

/// A settings row whose caption toggles the checkbox beside it.
///
/// The grid puts captions in the left column, where a plain `ui.label` is inert
/// -- so the only way to change one of these settings was to land on the box
/// itself, and clicking the words did nothing at all. Every other toolkit
/// toggles on the caption, which makes a setting that reads as broken when it
/// isn't.
fn checkbox_row(ui: &mut egui::Ui, caption: &str, value: &mut bool) {
    if ui
        .add(egui::Label::new(caption).sense(egui::Sense::click()))
        .clicked()
    {
        *value = !*value;
    }
    ui.checkbox(value, "");
}

/// The rates the dropdown offers: CD rate, the usual device rate, and the
/// OPL3's own. Anything else in a hand-edited ini is still shown and kept --
/// see [`frequency_label`] -- it just isn't one of the offered choices.
const FREQUENCIES: [u32; 3] = [44_100, 48_000, 49_716];

/// The buffer sizes the dropdown offers: the powers of two audio devices
/// actually accept, from a low-latency 64 up to a very safe 4096. The device's
/// own supported range clamps whatever is chosen, so an unusable extreme here
/// costs nothing. As with the rates, a value from a hand-edited ini is shown and
/// kept even though it is not offered.
const BUFFER_SIZES: [u32; 7] = [64, 128, 256, 512, 1024, 2048, 4096];

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
        ThemeChoice::Navy => "Navy",
        ThemeChoice::Cream => "Cream",
        ThemeChoice::Verdigris => "Verdigris",
        ThemeChoice::Moss => "Moss",
        ThemeChoice::Plum => "Plum",
        ThemeChoice::Rust => "Rust",
        ThemeChoice::Petrol => "Petrol",
        ThemeChoice::Slate => "Slate",
        ThemeChoice::Olive => "Olive",
        ThemeChoice::Wine => "Wine",
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

    #[test]
    fn the_checkbox_settings_reach_the_saved_config() {
        let mut dialog = SettingsDialog::new(&AppConfig::default());
        assert!(!dialog.dro_info_edit_enabled, "off by default");
        dialog.dro_info_edit_enabled = true;
        dialog.maximize_window = true;

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert!(saved.ui.dro_info_edit_enabled);
        assert!(saved.ui.maximize_window);
    }

    #[test]
    fn an_unlisted_buffer_size_survives_a_save() {
        let mut config = AppConfig::default();
        config.audio.buffer_size = 384;
        let mut dialog = SettingsDialog::new(&config);
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.buffer_size, 384, "the unlisted size is kept");

        dialog.buffer_size = 1024;
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.buffer_size, 1024);
    }
}
