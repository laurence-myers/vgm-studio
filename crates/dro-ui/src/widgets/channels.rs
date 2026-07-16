//! Channel and percussion soloing.
//!
//! New in the Rust port: the Python CLI player's interactive soloing was
//! deliberately dropped in Step 5 because its home is the GUI. Eighteen
//! melodic-channel toggles (nine per bank) plus a drums toggle per bank, all
//! applied live through `AudioService::set_muting`.

use dro_core::Bank;
use dro_synth::Muting;

use crate::theme::{Palette, bevel};

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

    /// Right-click solo for melodic channel `index`: makes it the only audible
    /// voice, or restores everything if it already is the only one.
    pub fn toggle_solo_channel(&mut self, index: usize) {
        if index >= self.channels.len() {
            return;
        }
        if self.is_soloed_channel(index) {
            *self = Self::default();
        } else {
            self.channels = [false; 18];
            self.percussion = [false; 2];
            self.channels[index] = true;
        }
    }

    /// Right-click solo for the drums on `bank` (`0` low, `1` high).
    pub fn toggle_solo_percussion(&mut self, bank: usize) {
        if bank >= self.percussion.len() {
            return;
        }
        if self.is_soloed_percussion(bank) {
            *self = Self::default();
        } else {
            self.channels = [false; 18];
            self.percussion = [false; 2];
            self.percussion[bank] = true;
        }
    }

    fn is_soloed_channel(&self, index: usize) -> bool {
        self.channels
            .iter()
            .enumerate()
            .all(|(i, &on)| on == (i == index))
            && self.percussion.iter().all(|&on| !on)
    }

    fn is_soloed_percussion(&self, bank: usize) -> bool {
        self.channels.iter().all(|&on| !on)
            && self
                .percussion
                .iter()
                .enumerate()
                .all(|(i, &on)| on == (i == bank))
    }

    /// Draws the strip. Low bank (channels 1-9, Drums, All) on the first row;
    /// with `show_high_bank` the high bank (1-9, Drums) follows on a second row.
    /// Hidden high-bank state survives for the next two-bank song.
    ///
    /// A grid keeps the two banks column-aligned: channel `1` sits directly above
    /// channel `1`, and so on. Left-click a toggle to mute it; right-click to solo
    /// it. Returns `true` if anything changed.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &Palette, show_high_bank: bool) -> bool {
        let mut changed = false;
        let row_height = ui.spacing().interact_size.y;
        egui::Grid::new("channel-grid")
            .min_row_height(row_height)
            // Fit every column to its content (egui defaults to interact_size),
            // so the narrow digit toggles don't leave slack that widens their
            // gaps -- every column gap is then the uniform 6px spacing.
            .min_col_width(0.0)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                // Perc. and All are ordinary grid columns, so the gap on either
                // side of Perc. matches the gap between the channel toggles.
                ui.label("Channels:");
                for index in 0..9 {
                    changed |= self.channel_toggle(ui, index);
                }
                changed |= self.percussion_toggle(ui, 0, "Perc.", "Percussion (low bank)");
                if bevel::button(ui, palette, "All")
                    .on_hover_text("Unmute everything")
                    .clicked()
                {
                    let all_on = Self::default();
                    changed |=
                        self.channels != all_on.channels || self.percussion != all_on.percussion;
                    *self = all_on;
                }
                ui.end_row();

                if show_high_bank {
                    ui.label("High bank:");
                    for index in 9..18 {
                        changed |= self.channel_toggle(ui, index);
                    }
                    changed |= self.percussion_toggle(ui, 1, "Perc.", "Percussion (high bank)");
                    ui.end_row();
                }
            });
        changed
    }

    /// One melodic-channel toggle: left-click mutes, right-click solos.
    fn channel_toggle(&mut self, ui: &mut egui::Ui, index: usize) -> bool {
        let label = (index % 9 + 1).to_string();
        let response = ui
            .toggle_value(&mut self.channels[index], label)
            .on_hover_text(format!(
                "Channel {} ({} bank). Left-click mutes, right-click solos.",
                index % 9 + 1,
                if index < 9 { "low" } else { "high" },
            ));
        let mut changed = response.changed();
        if response.secondary_clicked() {
            self.toggle_solo_channel(index);
            changed = true;
        }
        changed
    }

    /// One drums toggle: left-click mutes, right-click solos.
    fn percussion_toggle(
        &mut self,
        ui: &mut egui::Ui,
        bank: usize,
        label: &str,
        hover: &str,
    ) -> bool {
        let response = ui
            .toggle_value(&mut self.percussion[bank], label)
            .on_hover_text(format!("{hover}. Left-click mutes, right-click solos."));
        let mut changed = response.changed();
        if response.secondary_clicked() {
            self.toggle_solo_percussion(bank);
            changed = true;
        }
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

    #[test]
    fn soloing_a_channel_isolates_it() {
        let mut panel = ChannelPanel::new();
        panel.toggle_solo_channel(3); // low bank, 0xB3

        let muting = panel.muting();
        // The soloed channel plays; a sibling and the drums are silenced.
        assert_eq!(muting.gate(Bank::Low, 0xB3, 0xFF), Some(0xFF));
        assert_eq!(muting.gate(Bank::Low, 0xB0, 0xFF), None);
        assert_eq!(muting.gate(Bank::Low, 0xBD, 0xFF), Some(0xE0));
        assert_eq!(muting.gate(Bank::High, 0xB3, 0xFF), None);
    }

    #[test]
    fn soloing_the_same_channel_twice_restores_everything() {
        let mut panel = ChannelPanel::new();
        panel.toggle_solo_channel(3);
        panel.toggle_solo_channel(3);
        assert_eq!(panel.muting(), Muting::all());
    }

    #[test]
    fn soloing_a_second_channel_moves_the_solo() {
        let mut panel = ChannelPanel::new();
        panel.toggle_solo_channel(3);
        panel.toggle_solo_channel(5);

        let muting = panel.muting();
        assert_eq!(muting.gate(Bank::Low, 0xB5, 0xFF), Some(0xFF));
        assert_eq!(muting.gate(Bank::Low, 0xB3, 0xFF), None);
    }

    #[test]
    fn soloing_drums_isolates_that_bank() {
        let mut panel = ChannelPanel::new();
        panel.toggle_solo_percussion(0); // low-bank drums

        let muting = panel.muting();
        assert_eq!(muting.gate(Bank::Low, 0xBD, 0xFF), Some(0xFF));
        assert_eq!(muting.gate(Bank::High, 0xBD, 0xFF), Some(0xE0));
        assert_eq!(muting.gate(Bank::Low, 0xB0, 0xFF), None);
    }
}
