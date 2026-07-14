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
    frequency: String,
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
            frequency: config.audio.frequency.to_string(),
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
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut open = true;
        let mut close = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Frequency (Hz)")
                            .on_hover_text("49716 is the OPL3's native rate");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.frequency)
                                .text_color(palette.data_text)
                                .desired_width(100.0),
                        );
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

                        ui.label("Theme").on_hover_text("Takes effect immediately");
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
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
            self.frequency.trim().parse::<u32>(),
            self.buffer_size.trim().parse::<u32>(),
            self.chip_write_delay.trim().parse::<f64>(),
            self.tail_length.trim().parse::<u32>(),
        );
        let (Ok(frequency), Ok(buffer_size), Ok(chip_write_delay), Ok(tail_length)) = parsed else {
            actions.push(Action::Alert {
                title: "Invalid settings".to_owned(),
                message: "Check that the entered values are numbers.".to_owned(),
            });
            return false;
        };
        config.audio.frequency = frequency;
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

/// The dropdown label for a theme.
fn theme_label(theme: ThemeChoice) -> &'static str {
    match theme {
        ThemeChoice::CloneDark => "Clone (dark)",
        ThemeChoice::Ft2Classic => "FastTracker II (classic)",
    }
}
