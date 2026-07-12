//! The Settings dialog. New in the Rust port: the Python only ever *read*
//! `drotrim.ini`. The web build (Step 8) has no ini file at all, so the same
//! dialog writes through whatever `ConfigStore` the platform injected.

use dro_core::config::AppConfig;

use crate::action::Action;

#[derive(Debug)]
pub struct SettingsDialog {
    frequency: String,
    buffer_size: String,
    bit_depth: u16,
    chip_write_delay: String,
    tail_length: String,
    maximize_window: bool,
    dro_info_edit_enabled: bool,
}

impl SettingsDialog {
    #[must_use]
    pub fn new(config: &AppConfig) -> Self {
        Self {
            frequency: config.audio.frequency.to_string(),
            buffer_size: config.audio.buffer_size.to_string(),
            bit_depth: config.audio.bit_depth,
            chip_write_delay: config.audio.chip_write_delay.to_string(),
            tail_length: config.ui.tail_length.to_string(),
            maximize_window: config.ui.maximize_window,
            dro_info_edit_enabled: config.ui.dro_info_edit_enabled,
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) -> bool {
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
                            egui::TextEdit::singleline(&mut self.frequency).desired_width(100.0),
                        );
                        ui.end_row();

                        ui.label("Buffer size");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.buffer_size).desired_width(100.0),
                        );
                        ui.end_row();

                        ui.label("Bit depth").on_hover_text("WAV export only");
                        egui::ComboBox::from_id_salt("settings-bit-depth")
                            .selected_text(self.bit_depth.to_string())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.bit_depth, 8, "8");
                                ui.selectable_value(&mut self.bit_depth, 16, "16");
                            });
                        ui.end_row();

                        ui.label("Chip write delay (\u{00b5}s)").on_hover_text(
                            "Microseconds after each chip write, to imitate real hardware. \
                             0 = perfect timing; OPL2 wants at least 26.6.",
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.chip_write_delay)
                                .desired_width(100.0),
                        );
                        ui.end_row();

                        ui.label("Tail length (ms)")
                            .on_hover_text("How much the \"play last X seconds\" button plays");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.tail_length).desired_width(100.0),
                        );
                        ui.end_row();

                        ui.label("Maximize window at launch");
                        ui.checkbox(&mut self.maximize_window, "");
                        ui.end_row();

                        ui.label("Allow editing in DRO Info");
                        ui.checkbox(&mut self.dro_info_edit_enabled, "");
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() && self.save(actions) {
                        close = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        open && !close
    }

    /// Parses, validates and emits the new settings; `false` (with an error
    /// box queued) if anything is invalid.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        let mut config = AppConfig::default();
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
