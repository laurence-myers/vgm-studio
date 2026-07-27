//! The Settings dialog. The web build (Step 8) has no ini file at all, so the
//! dialog writes through whatever `ConfigStore` the platform injected.

use std::collections::BTreeMap;

use dro_core::config::{AppConfig, OutputBackend, SurfaceChoice, ThemeChoice};

use crate::action::Action;
use crate::platform::HardwarePortInfo;
use crate::theme::{Palette, bevel};
use crate::widgets::chip_output;

/// The appearance settings, as `(theme, pad_style, deck_style)`. These three
/// preview live, so they travel together.
type Skin = (ThemeChoice, SurfaceChoice, SurfaceChoice);

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
    pad_style: SurfaceChoice,
    deck_style: SurfaceChoice,
    /// The core chosen per chip slot, edited in place by the picker. The whole
    /// map, not just OPL's row: every chip's core is a setting now.
    cores: BTreeMap<String, String>,
    /// The chosen port, or empty for "whichever one is found".
    retrowave_port: String,
    /// The ports offered in the picker, listed when the dialog opened.
    ports: Vec<HardwarePortInfo>,
}

impl SettingsDialog {
    #[must_use]
    pub fn new(config: &AppConfig, ports: Vec<HardwarePortInfo>) -> Self {
        Self {
            original: config.clone(),
            frequency: config.audio.frequency,
            buffer_size: config.audio.buffer_size,
            bit_depth: config.audio.bit_depth,
            tail_length: config.ui.tail_length.to_string(),
            maximize_window: config.ui.maximize_window,
            dro_info_edit_enabled: config.ui.dro_info_edit_enabled,
            theme: config.ui.theme,
            pad_style: config.ui.pad_style,
            // The deck has no grey treatment, so a hand-edited ini naming one
            // must not show a choice the dropdown cannot offer back.
            deck_style: config.ui.deck_style.for_deck(),
            cores: config.audio.cores.clone(),
            retrowave_port: config.audio.retrowave_port.clone().unwrap_or_default(),
            ports,
        }
    }

    /// Whether the settings that only shape the rendered signal still apply.
    ///
    /// Read back from the OPL slot rather than kept alongside it, so the greying
    /// cannot disagree with the picker that drives it.
    fn emulating(&self) -> bool {
        self.backend() == OutputBackend::Emulated
    }

    /// Points a chip slot at a core, as the picker's dropdown does.
    ///
    /// Exists for the tests: the picker itself edits the map through
    /// [`chip_output::show`], and a test that reached into the field would be
    /// asserting about a `BTreeMap` rather than about a setting.
    #[cfg(test)]
    fn choose_core(&mut self, slot: &str, core: &str) {
        self.cores.insert(slot.to_owned(), core.to_owned());
    }

    /// The OPL slot as a backend: hardware, or anything else.
    fn backend(&self) -> OutputBackend {
        match self
            .cores
            .get(dro_core::config::OPL_SLOT)
            .map(String::as_str)
        {
            Some(dro_core::config::RETROWAVE_CORE) => OutputBackend::RetroWave,
            _ => OutputBackend::Emulated,
        }
    }

    /// The three appearance settings, which preview live rather than waiting for
    /// Save -- a colour scheme can only be judged on the whole window, not on a
    /// dropdown's label.
    fn skin(&self) -> Skin {
        (self.theme, self.pad_style, self.deck_style)
    }

    /// The appearance the dialog opened with, restored when it is closed without
    /// saving.
    fn original_skin(&self) -> Skin {
        let ui = &self.original.ui;
        (ui.theme, ui.pad_style, ui.deck_style.for_deck())
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let mut saved = false;
        let opened_with = self.skin();
        let open = super::dialog_modal(ctx, "settings-modal", "Settings", palette, |ui| {
            ui.label("Output").on_hover_text(
                "Which core plays each chip, and where its sound goes. Rendering, \
                 splitting and the waveform always use an emulator.",
            );
            // **Its own scroll area, and that is not cosmetic.** One row per
            // chip with a core means this section grows with every core added
            // -- it was two rows when there was one chip and is a dozen now --
            // and a dialog that outgrows the window puts Save off the bottom of
            // it. Bounding the section rather than the dialog keeps the buttons
            // where they have always been.
            egui::ScrollArea::vertical()
                .id_salt("settings-output-scroll")
                .max_height(CHIP_ROWS_MAX_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Grid::new("settings-output-grid")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            let ports = &self.ports;
                            let port = &mut self.retrowave_port;
                            chip_output::show(ui, palette, &mut self.cores, &mut |ui| {
                                ui.label("Device").on_hover_text(
                                    "The board's serial port. On Windows these usually report \
                             a generic name, so a recognised board is matched by its \
                             USB ID instead.",
                                );
                                ui.scope(|ui| {
                                    crate::theme::style_dropdown(ui, palette);
                                    egui::ComboBox::from_id_salt("settings-retrowave-port")
                                        .selected_text(port_label(port, ports))
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(port, String::new(), AUTO_DETECT);
                                            for offered in ports {
                                                ui.selectable_value(
                                                    port,
                                                    offered.port_name.clone(),
                                                    offered_label(offered),
                                                );
                                            }
                                        });
                                });
                                ui.end_row();
                            });
                        });
                });
            ui.add_space(6.0);

            egui::Grid::new("settings-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    // What follows shapes the rendered signal, which hardware
                    // output does not produce.
                    let emulating = self.emulating();
                    ui.add_enabled_ui(emulating, |ui| {
                        ui.label("Frequency")
                            .on_hover_text("49716 Hz is the OPL3's native rate");
                    });
                    ui.add_enabled_ui(emulating, |ui| {
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

                    ui.add_enabled_ui(emulating, |ui| {
                        ui.label("Buffer size").on_hover_text(
                            "Frames per audio callback. Smaller responds to seeking and \
                             muting sooner; larger is safer against dropouts.",
                        );
                    });
                    ui.add_enabled_ui(emulating, |ui| {
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
                    super::text_field(ui, palette, &mut self.tail_length, 100.0);
                    ui.end_row();

                    ui.label("Theme").on_hover_text(
                        "The case colour. Applies as you pick it; Close puts the old one back.",
                    );
                    egui::ComboBox::from_id_salt("settings-theme")
                        .selected_text(theme_label(self.theme))
                        .show_ui(ui, |ui| {
                            for choice in ThemeChoice::ALL {
                                ui.selectable_value(&mut self.theme, choice, theme_label(choice));
                            }
                        });
                    ui.end_row();

                    ui.label("Pad style").on_hover_text(
                        "The keycap colour: follow the theme, or force light, dark \
                         or plate-tinted keys on any theme.",
                    );
                    egui::ComboBox::from_id_salt("settings-pad-style")
                        .selected_text(surface_label(self.pad_style))
                        .show_ui(ui, |ui| {
                            for choice in SurfaceChoice::ALL {
                                ui.selectable_value(
                                    &mut self.pad_style,
                                    choice,
                                    surface_label(choice),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Deck style").on_hover_text(
                        "The control panel the pads sit on: follow the theme, or \
                         force a light, dark or plate-tinted deck.",
                    );
                    egui::ComboBox::from_id_salt("settings-deck-style")
                        .selected_text(surface_label(self.deck_style))
                        .show_ui(ui, |ui| {
                            for choice in SurfaceChoice::DECK {
                                ui.selectable_value(
                                    &mut self.deck_style,
                                    choice,
                                    surface_label(choice),
                                );
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
                    saved = true;
                }
            });
        });
        // `open` is false when Esc or a backdrop click dismissed the modal,
        // which means the same as Close.
        let closing = close || !open || saved;
        actions.extend(self.preview(opened_with, closing, saved));
        !closing
    }

    /// The preview to emit for a frame that started on `opened_with`, if any.
    ///
    /// Picking a skin previews it. Closing without saving discards the edits, so
    /// the preview goes back to what the dialog opened from -- not to whatever is
    /// picked now. Saving hands the skin to `ApplySettings` instead, so a preview
    /// after it would only be redundant.
    fn preview(&self, opened_with: Skin, closing: bool, saved: bool) -> Option<Action> {
        if saved {
            return None;
        }
        let wanted = if closing {
            self.original_skin()
        } else {
            self.skin()
        };
        let (theme, pad_style, deck_style) = wanted;
        (wanted != opened_with).then_some(Action::PreviewSkin {
            theme,
            pad_style,
            deck_style,
        })
    }

    /// Parses, validates and emits the new settings; `false` (with an error
    /// box queued) if anything is invalid.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        // Start from the config the dialog opened with, so fields it does not
        // edit (like `audio.boost`, driven by the transport slider) survive.
        let mut config = self.original.clone();
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
        config.audio.cores = self.cores.clone();
        let port = self.retrowave_port.trim();
        config.audio.retrowave_port = (!port.is_empty()).then(|| port.to_owned());
        config.ui.tail_length = tail_length;
        config.ui.maximize_window = self.maximize_window;
        config.ui.dro_info_edit_enabled = self.dro_info_edit_enabled;
        config.ui.theme = self.theme;
        config.ui.pad_style = self.pad_style;
        config.ui.deck_style = self.deck_style;

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
/// What an unset port means: pick one at load time.
/// How tall the per-chip output list may grow before it scrolls.
///
/// About eight rows. Chosen so the dialog's overall height stays roughly what
/// it was when the section was short, because Save and Close live below it.
const CHIP_ROWS_MAX_HEIGHT: f32 = 190.0;

const AUTO_DETECT: &str = "Detect automatically";

/// A port as offered in the picker, ticked when we recognise the hardware.
fn offered_label(port: &HardwarePortInfo) -> String {
    if port.recognised {
        format!("{} ✓", port.label)
    } else {
        port.label.clone()
    }
}

/// The picker's closed-state text.
///
/// A port saved from another machine may not be present here, so name it
/// anyway rather than silently showing something else.
fn port_label(selected: &str, ports: &[HardwarePortInfo]) -> String {
    if selected.is_empty() {
        return AUTO_DETECT.to_owned();
    }
    ports
        .iter()
        .find(|port| port.port_name == selected)
        .map_or_else(|| format!("{selected} (not connected)"), offered_label)
}

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

/// The dropdown label for a pad/deck style override.
fn surface_label(choice: SurfaceChoice) -> &'static str {
    match choice {
        SurfaceChoice::ThemeDefault => "Theme default",
        SurfaceChoice::Light => "Light",
        SurfaceChoice::Dark => "Dark",
        SurfaceChoice::Grey => "Grey",
        SurfaceChoice::Tint => "Tint",
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
        let mut dialog = SettingsDialog::new(&config, Vec::new());
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
        let mut dialog = SettingsDialog::new(&dialog_config, Vec::new());
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
        let mut dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
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
        let mut dialog = SettingsDialog::new(&config, Vec::new());
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

    fn port(name: &str, recognised: bool) -> HardwarePortInfo {
        HardwarePortInfo {
            port_name: name.to_owned(),
            label: name.to_owned(),
            recognised,
        }
    }

    /// Choosing the board is now choosing a *core* for the OPL slot, so the
    /// saved config must carry it in the same map every other chip uses -- and
    /// still read back as the backend the audio service switches on.
    #[test]
    fn the_output_backend_and_port_reach_the_saved_config() {
        let mut dialog = SettingsDialog::new(&AppConfig::default(), vec![port("COM3", true)]);
        assert_eq!(dialog.backend(), OutputBackend::Emulated);

        dialog.choose_core(dro_core::config::OPL_SLOT, dro_core::config::RETROWAVE_CORE);
        dialog.retrowave_port = "COM3".to_owned();
        assert_eq!(dialog.backend(), OutputBackend::RetroWave);

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.output_backend(), OutputBackend::RetroWave);
        assert_eq!(
            saved.audio.core(dro_core::config::OPL_SLOT),
            Some(dro_core::config::RETROWAVE_CORE)
        );
        assert_eq!(saved.audio.retrowave_port.as_deref(), Some("COM3"));
    }

    /// A core chosen for a chip that is not OPL travels the same road, which is
    /// the whole point of the map: the OPL row stopped being a special case.
    #[test]
    fn a_core_chosen_for_any_chip_reaches_the_saved_config() {
        let mut dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
        dialog.choose_core("sn76489", "native");

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.core("sn76489"), Some("native"));
        assert_eq!(
            saved.audio.output_backend(),
            OutputBackend::Emulated,
            "another chip's core must not disturb where OPL plays"
        );
    }

    /// An unset port means "find one at load time", not a port named "".
    #[test]
    fn an_unchosen_port_saves_as_no_port() {
        let mut config = AppConfig::default();
        config.audio.retrowave_port = Some("COM9".to_owned());
        let mut dialog = SettingsDialog::new(&config, Vec::new());
        assert_eq!(dialog.retrowave_port, "COM9");

        dialog.retrowave_port = String::new();
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        let Some(Action::ApplySettings(saved)) = actions.pop() else {
            panic!("expected the settings to be applied");
        };
        assert_eq!(saved.audio.retrowave_port, None);
    }

    #[test]
    fn the_signal_settings_only_apply_to_the_emulator() {
        let mut dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
        assert!(dialog.emulating());
        dialog.choose_core(dro_core::config::OPL_SLOT, dro_core::config::RETROWAVE_CORE);
        assert!(!dialog.emulating(), "hardware output renders no signal");
    }

    fn previewed(action: Option<Action>) -> Option<Skin> {
        match action {
            Some(Action::PreviewSkin {
                theme,
                pad_style,
                deck_style,
            }) => Some((theme, pad_style, deck_style)),
            Some(other) => panic!("expected a preview, got {other:?}"),
            None => None,
        }
    }

    /// The three appearance settings apply as they are picked, so the whole
    /// window shows what the choice actually looks like.
    #[test]
    fn picking_an_appearance_previews_it() {
        let mut dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
        let opened_with = dialog.skin();

        // A frame that changes nothing re-previews nothing.
        assert_eq!(previewed(dialog.preview(opened_with, false, false)), None);

        dialog.theme = ThemeChoice::Wine;
        dialog.pad_style = SurfaceChoice::Grey;
        assert_eq!(
            previewed(dialog.preview(opened_with, false, false)),
            Some((ThemeChoice::Wine, SurfaceChoice::Grey, dialog.deck_style)),
        );
        // ...and only once: the next frame opens on what is already shown.
        assert_eq!(previewed(dialog.preview(dialog.skin(), false, false)), None);
    }

    /// Close means "I didn't want any of this", including the previews it
    /// applied on the way -- otherwise trying themes out would silently keep the
    /// last one.
    #[test]
    fn closing_reverts_the_preview() {
        let mut config = AppConfig::default();
        config.ui.theme = ThemeChoice::Petrol;
        config.ui.deck_style = SurfaceChoice::Dark;
        let mut dialog = SettingsDialog::new(&config, Vec::new());

        dialog.theme = ThemeChoice::Olive;
        dialog.deck_style = SurfaceChoice::Light;
        assert_eq!(
            previewed(dialog.preview(dialog.skin(), true, false)),
            Some((
                ThemeChoice::Petrol,
                SurfaceChoice::ThemeDefault,
                SurfaceChoice::Dark
            )),
            "the settings the dialog opened with come back",
        );
    }

    /// Picking a theme, changing your mind, then closing: there is nothing left
    /// to revert, so nothing is emitted.
    #[test]
    fn closing_on_the_original_appearance_previews_nothing() {
        let dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
        assert_eq!(previewed(dialog.preview(dialog.skin(), true, false)), None);
    }

    /// Grey is a pad treatment only. An ini naming a grey deck must not leave
    /// the dropdown showing a choice it cannot offer back.
    #[test]
    fn a_grey_deck_opens_as_the_theme_default() {
        let mut config = AppConfig::default();
        config.ui.pad_style = SurfaceChoice::Grey;
        config.ui.deck_style = SurfaceChoice::Grey;
        let dialog = SettingsDialog::new(&config, Vec::new());

        assert_eq!(dialog.pad_style, SurfaceChoice::Grey, "pads keep grey");
        assert_eq!(dialog.deck_style, SurfaceChoice::ThemeDefault);
        assert_eq!(dialog.original_skin().2, SurfaceChoice::ThemeDefault);
        // ...and opening then closing changes nothing.
        assert_eq!(previewed(dialog.preview(dialog.skin(), true, false)), None);
    }

    /// Save already carries the appearance in `ApplySettings`; a preview beside
    /// it would be redundant.
    #[test]
    fn saving_previews_nothing() {
        let mut dialog = SettingsDialog::new(&AppConfig::default(), Vec::new());
        let opened_with = dialog.skin();
        dialog.theme = ThemeChoice::Moss;
        assert_eq!(previewed(dialog.preview(opened_with, true, true)), None);
    }

    /// A port saved on another machine, or since unplugged, must still be named
    /// rather than silently reading as some other port.
    #[test]
    fn a_missing_port_is_named_as_missing() {
        let ports = vec![port("COM3", true), port("COM4", false)];
        assert_eq!(port_label("", &ports), AUTO_DETECT);
        assert_eq!(port_label("COM3", &ports), "COM3 ✓");
        assert_eq!(port_label("COM4", &ports), "COM4");
        assert_eq!(port_label("COM12", &ports), "COM12 (not connected)");
    }
}
