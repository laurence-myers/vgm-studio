//! Channel and percussion soloing.
//!
//! New in the Rust port: the Python CLI player's interactive soloing was
//! deliberately dropped in Step 5 because its home is the GUI. Eighteen
//! melodic-channel toggles (nine per bank) plus a drums toggle per bank, all
//! applied live through `AudioService::set_muting`.

use dro_core::Bank;
use dro_synth::Muting;

#[derive(Debug)]
pub struct ChannelPanel {
    /// Channel `bank * 9 + n` audible? Registers `0xB0 + n` on that bank.
    channels: [bool; 18],
    /// Drums audible, per bank.
    percussion: [bool; 2],
}

impl Default for ChannelPanel {
    fn default() -> Self {
        Self {
            channels: [true; 18],
            percussion: [true; 2],
        }
    }
}

impl ChannelPanel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The muting the current toggles describe.
    #[must_use]
    pub fn muting(&self) -> Muting {
        let mut muting = Muting::all();
        for (index, &audible) in self.channels.iter().enumerate() {
            if !audible {
                let bank = if index < 9 { Bank::Low } else { Bank::High };
                muting.mute_channel(bank, 0xB0 + (index % 9) as u8);
            }
        }
        for (index, &audible) in self.percussion.iter().enumerate() {
            if !audible {
                let bank = if index == 0 { Bank::Low } else { Bank::High };
                // Silence the five drums but keep the control bits, as
                // `Muting::silent()` does.
                muting.set_percussion(bank, 0xE0);
            }
        }
        muting
    }

    /// Toggles melodic channel `index` (`0..18`). The number keys use this.
    pub fn toggle_channel(&mut self, index: usize) {
        if let Some(channel) = self.channels.get_mut(index) {
            *channel = !*channel;
        }
    }

    /// Draws the strip. With `show_high_bank` false (a plain OPL2 song, which
    /// has only one bank), the nine high-bank toggles and the high-bank drums
    /// are hidden; their state survives for the next two-bank song.
    ///
    /// Returns `true` if any toggle changed.
    pub fn show(&mut self, ui: &mut egui::Ui, show_high_bank: bool) -> bool {
        let visible_channels = if show_high_bank { 18 } else { 9 };
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("Channels:");
            for index in 0..visible_channels {
                if index == 9 {
                    ui.separator();
                }
                let label = (index + 1).to_string();
                changed |= ui
                    .toggle_value(&mut self.channels[index], label)
                    .on_hover_text(format!(
                        "Channel {} ({} bank). Keys 1-9 toggle the low bank.",
                        index % 9 + 1,
                        if index < 9 { "low" } else { "high" },
                    ))
                    .changed();
            }
            ui.separator();
            changed |= ui
                .toggle_value(&mut self.percussion[0], "Drums")
                .on_hover_text("Percussion (low bank)")
                .changed();
            if show_high_bank {
                changed |= ui
                    .toggle_value(&mut self.percussion[1], "Drums hi")
                    .on_hover_text("Percussion (high bank)")
                    .changed();
            }
            ui.separator();
            if ui
                .button("All")
                .on_hover_text("Unmute everything")
                .clicked()
            {
                let all_on = Self::default();
                changed |= self.channels != all_on.channels || self.percussion != all_on.percussion;
                *self = all_on;
            }
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_on_is_muting_all() {
        assert_eq!(ChannelPanel::new().muting(), Muting::all());
    }

    #[test]
    fn toggles_map_to_the_right_bank_and_register() {
        let mut panel = ChannelPanel::new();
        panel.toggle_channel(0); // low bank, 0xB0
        panel.toggle_channel(17); // high bank, 0xB8

        let mut expected = Muting::all();
        expected.mute_channel(Bank::Low, 0xB0);
        expected.mute_channel(Bank::High, 0xB8);
        assert_eq!(panel.muting(), expected);

        panel.toggle_channel(0);
        panel.toggle_channel(17);
        assert_eq!(panel.muting(), Muting::all());
    }

    #[test]
    fn percussion_toggles_mask_the_drums_but_keep_control_bits() {
        let mut panel = ChannelPanel::new();
        panel.percussion[0] = false;
        let muting = panel.muting();
        // A full 0xBD write comes through with the drum bits stripped.
        assert_eq!(muting.gate(Bank::Low, 0xBD, 0xFF), Some(0xE0));
        assert_eq!(muting.gate(Bank::High, 0xBD, 0xFF), Some(0xFF));
    }

    #[test]
    fn out_of_range_toggles_are_ignored() {
        let mut panel = ChannelPanel::new();
        panel.toggle_channel(99);
        assert_eq!(panel.muting(), Muting::all());
    }
}
