//! Channel and percussion muting, soloing, and per-channel panning.
//!
//! New in the Rust port: the Python CLI player's interactive soloing was
//! deliberately dropped in Step 5 because its home is the GUI. Eighteen
//! melodic-channel toggles (nine per bank) plus a drums toggle per bank, applied
//! live through `AudioService::set_muting`; and, above each toggle, a pan knob
//! applied through `AudioService::set_panning` when the panel is in Custom mode.

use dro_core::{Bank, OplType, Song};
use dro_synth::{Muting, Panning};

use crate::theme::{Palette, bevel};
use crate::widgets::pan_knob;

/// The centred pan byte (`0x80`), and the hard-left / hard-right extremes.
const PAN_CENTER: u8 = 0x80;
const PAN_LEFT: u8 = 0x00;
const PAN_RIGHT: u8 = 0xFF;

/// Auto-pan: the gentlest nudge off centre, and how much each successive channel
/// widens (within a repeating group of five), so neighbours never share a value.
const AUTO_PAN_BASE: u8 = 0x10;
const AUTO_PAN_STEP: u8 = 6;

/// What [`ChannelPanel::show`] changed this frame, split so a pan drag never
/// resends muting mid-note and a mute toggle never resends panning.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChannelsResponse {
    pub muting_changed: bool,
    pub panning_changed: bool,
}

#[derive(Debug)]
pub struct ChannelPanel {
    /// Channel `bank * 9 + n` audible? Registers `0xB0 + n` on that bank.
    channels: [bool; 18],
    /// Drums audible, per bank.
    percussion: [bool; 2],
    /// The per-channel pan bytes edited under Custom mode, indexed `bank * 9 + n`
    /// (`0x00` hard left .. `0x80` centre .. `0xFF` hard right).
    pans: [u8; 18],
    /// The pans the current song type implies, shown (greyed) under Original and
    /// restored by "All".
    default_pans: [u8; 18],
    /// Whether the pan knobs drive the output (Custom) or the song's own image
    /// does (Original).
    custom: bool,
    /// The loaded song's chip type, or `None` before any song. Decides the high
    /// bank's visibility and the Original panning policy.
    opl_type: Option<OplType>,
}

impl Default for ChannelPanel {
    fn default() -> Self {
        Self {
            channels: [true; 18],
            percussion: [true; 2],
            pans: [PAN_CENTER; 18],
            default_pans: [PAN_CENTER; 18],
            custom: false,
            opl_type: None,
        }
    }
}

/// The pan image a song type implies at load: OPL2 centres everything, dual-OPL2
/// puts chip 1 (bank 0) hard left and chip 2 (bank 1) hard right (the authentic
/// SB Pro 1 image), and OPL3 mirrors the song's own first `0xC0` writes.
fn default_pans_for(opl_type: OplType, song: &Song) -> [u8; 18] {
    match opl_type {
        OplType::Opl2 => [PAN_CENTER; 18],
        OplType::DualOpl2 => dual_opl2_image(),
        OplType::Opl3 => dro_core::initial_channel_pans(song),
    }
}

/// The fixed hard-L/R panning image a dual-OPL2 song plays: chip 1 (bank 0,
/// channels 0..9) hard left, chip 2 (bank 1, channels 9..18) hard right -- the
/// authentic SB Pro 1 image. Shared by the load-time default and the live
/// `Original`-mode panning, so the split lives in one place.
fn dual_opl2_image() -> [u8; 18] {
    let mut pans = [PAN_LEFT; 18];
    pans[9..].fill(PAN_RIGHT);
    pans
}

/// A subtle alternating left/right pan image for the auto-pan preset: even
/// channels lean left, odd channels lean right, and the amount widens gently
/// across each group of five so no two neighbours share a value -- a wide-but-
/// subtle stereo spread rather than a hard split.
fn auto_pan_image() -> [u8; 18] {
    let mut pans = [PAN_CENTER; 18];
    for (slot, pan) in pans.iter_mut().enumerate() {
        let amount = AUTO_PAN_BASE + (slot % 5) as u8 * AUTO_PAN_STEP;
        *pan = if slot % 2 == 0 {
            PAN_CENTER - amount // even channels lean left
        } else {
            PAN_CENTER + amount // odd channels lean right
        };
    }
    pans
}

impl ChannelPanel {
    /// A panel with no song: everything audible, centred, Original mode.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A panel seeded for `song`: audible, mode Original, pans defaulted to the
    /// song type's image.
    #[must_use]
    pub fn for_song(song: &Song) -> Self {
        let opl_type = song.opl_type;
        let default_pans = default_pans_for(opl_type, song);
        Self {
            channels: [true; 18],
            percussion: [true; 2],
            pans: default_pans,
            default_pans,
            custom: false,
            opl_type: Some(opl_type),
        }
    }

    /// Adopts a new chip type after a live DRO Info edit, recomputing the pan
    /// defaults (and, while Original, the shown pans) from `song`.
    pub fn set_opl_type(&mut self, opl_type: OplType, song: Option<&Song>) {
        self.opl_type = Some(opl_type);
        self.default_pans = match song {
            Some(song) => default_pans_for(opl_type, song),
            None => [PAN_CENTER; 18],
        };
        if !self.custom {
            self.pans = self.default_pans;
        }
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

    /// The panning the current mode and song type describe.
    ///
    /// Custom always uses the edited pans. Original is policy-driven: a dual-OPL2
    /// song plays the fixed hard-L/R chip image (ignoring the knobs), while OPL2
    /// and OPL3 defer to the song's own output (`Panning::Original`).
    #[must_use]
    pub fn panning(&self) -> Panning {
        if self.custom {
            return Panning::Custom(self.pans);
        }
        match self.opl_type {
            Some(OplType::DualOpl2) => Panning::Custom(dual_opl2_image()),
            _ => Panning::Original,
        }
    }

    /// Toggles melodic channel `index` (`0..18`). The number keys use this.
    pub fn toggle_channel(&mut self, index: usize) {
        if let Some(channel) = self.channels.get_mut(index) {
            *channel = !*channel;
        }
    }

    /// Right-click solo for melodic channel `index`: makes it the only audible
    /// voice, or restores everything if it already is the only one. Pans are left
    /// untouched -- soloing is a muting concern.
    pub fn toggle_solo_channel(&mut self, index: usize) {
        if index >= self.channels.len() {
            return;
        }
        if self.is_soloed_channel(index) {
            self.unmute_all();
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
            self.unmute_all();
        } else {
            self.channels = [false; 18];
            self.percussion = [false; 2];
            self.percussion[bank] = true;
        }
    }

    /// Restores every melodic channel and drum to audible (pans untouched).
    fn unmute_all(&mut self) {
        self.channels = [true; 18];
        self.percussion = [true; 2];
    }

    /// Applies the auto-pan spread and engages Custom mode so it takes effect.
    fn apply_auto_pan(&mut self) {
        self.pans = auto_pan_image();
        self.custom = true;
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

    /// Draws the strip. Each bank is a pan row (nine knobs) directly above its
    /// toggle row (channels 1-9, Drums), so a knob sits over its channel's digit.
    /// The low bank always shows; the high bank shows for dual-OPL2 and OPL3. The
    /// Original/Custom mode toggle sits above "All".
    ///
    /// Left-click a toggle to mute it; right-click to solo it. Knobs are live only
    /// under Custom; under Original they show the policy pan, greyed. Returns which
    /// of muting/panning changed this frame.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &Palette) -> ChannelsResponse {
        let show_high_bank = self.opl_type != Some(OplType::Opl2);
        let mut response = ChannelsResponse::default();
        let row_height = ui.spacing().interact_size.y;
        egui::Grid::new("channel-grid")
            .min_row_height(row_height)
            // Label/Perc./All columns fit their content; the nine channel columns
            // are each pinned to the knob width (see `channel_toggle`), so a knob
            // sits centred over its digit and the columns never shift.
            .min_col_width(0.0)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                // Low bank: pan row, then the toggle row with Perc. and All. The
                // toggles are `bevel::toggle`s, each pinned to a fixed rect, so
                // they never grow or jostle their neighbours across states.
                response.panning_changed |= self.pan_row(ui, palette, 0, true);
                ui.end_row();

                ui.label("Channels:");
                for index in 0..9 {
                    response.muting_changed |= self.channel_toggle(ui, palette, index);
                }
                response.muting_changed |=
                    self.percussion_toggle(ui, palette, 0, "Perc.", "Percussion (low bank)");
                if bevel::button(ui, palette, "All")
                    .on_hover_text("Unmute everything and recentre pans")
                    .clicked()
                {
                    response.muting_changed |=
                        self.channels != [true; 18] || self.percussion != [true; 2];
                    self.unmute_all();
                    response.panning_changed |= self.pans != self.default_pans;
                    self.pans = self.default_pans;
                }
                ui.end_row();

                if show_high_bank {
                    response.panning_changed |= self.pan_row(ui, palette, 1, false);
                    ui.end_row();

                    ui.label("High bank:");
                    for index in 9..18 {
                        response.muting_changed |= self.channel_toggle(ui, palette, index);
                    }
                    response.muting_changed |=
                        self.percussion_toggle(ui, palette, 1, "Perc.", "Percussion (high bank)");
                    ui.end_row();
                }
            });
        response
    }

    /// One bank's pan row: a label, nine pan knobs (over the toggle digits), an
    /// empty cell in the Perc. column, and -- on the low bank -- the
    /// Original/Custom mode toggle under "All". Returns whether panning changed.
    fn pan_row(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        bank: usize,
        with_mode_toggle: bool,
    ) -> bool {
        let mut changed = false;
        ui.label(if bank == 0 { "Pan:" } else { "" });
        let bank_name = if bank == 0 { "low" } else { "high" };
        for channel in 0..9 {
            let slot = bank * 9 + channel;
            let label = format!("Pan {} ({bank_name} bank)", channel + 1);
            if self.custom {
                changed |=
                    pan_knob::show(ui, palette, &mut self.pans[slot], true, &label).changed();
            } else {
                // Original: show the policy pan, inert and greyed. The knob does
                // not mutate a disabled value, so a throwaway copy is safe.
                let mut shown = self.default_pans[slot];
                pan_knob::show(ui, palette, &mut shown, false, &label);
            }
        }
        // The Perc. column has no pan knob (drums pan through channels 7-9).
        ui.label("");
        if with_mode_toggle {
            let hint = match self.opl_type {
                Some(OplType::DualOpl2) => "Original: chip 1 left, chip 2 right",
                Some(OplType::Opl3) => "Original: the song's own panning",
                _ => "Original: mono",
            };
            let mut custom = self.custom;
            if bevel::toggle(ui, palette, &mut custom, "Custom")
                .on_hover_text(hint)
                .changed()
            {
                self.custom = custom;
                // Switching mode changes the effective panning.
                changed = true;
            }
            // A one-click preset: a subtle alternating L/R spread that also
            // engages Custom so it is heard immediately.
            if bevel::button(ui, palette, "Auto")
                .on_hover_text("Auto-pan: a subtle alternating left/right spread")
                .clicked()
            {
                self.apply_auto_pan();
                changed = true;
            }
        }
        changed
    }

    /// One melodic-channel toggle: left-click mutes, right-click solos.
    ///
    /// Allocated at a fixed cell size (the knob width above it) so the digit stays
    /// centred under its knob and the column never resizes with the button state.
    fn channel_toggle(&mut self, ui: &mut egui::Ui, palette: &Palette, index: usize) -> bool {
        let label = (index % 9 + 1).to_string();
        let row_h = ui.spacing().interact_size.y;
        let response = bevel::toggle_sized(
            ui,
            palette,
            &mut self.channels[index],
            &label,
            egui::vec2(pan_knob::SIZE, row_h),
        )
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
        palette: &Palette,
        bank: usize,
        label: &str,
        hover: &str,
    ) -> bool {
        let response = bevel::toggle(ui, palette, &mut self.percussion[bank], label).on_hover_text(
            format!(
                "{hover}. Drums sound through channels 7-9's pans. \
                 Left-click mutes, right-click solos."
            ),
        );
        let mut changed = response.changed();
        if response.secondary_clicked() {
            self.toggle_solo_percussion(bank);
            changed = true;
        }
        changed
    }

    /// Test-only: engage Custom mode with `pans`, for the theme showcase.
    #[cfg(test)]
    pub(crate) fn set_showcase_pans(&mut self, pans: [u8; 18]) {
        self.custom = true;
        self.pans = pans;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV1, Song};

    fn opl2_song() -> Song {
        Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01]).unwrap(),
            0,
            OplType::Opl2,
        )
    }

    fn dual_opl2_song() -> Song {
        Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01]).unwrap(),
            0,
            OplType::DualOpl2,
        )
    }

    fn opl3_song_panned() -> Song {
        // ch0 low hard-left (0xC0 bit4), ch0 high hard-right (0xC0 bit5).
        Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![
                0xC0, 0x10, // ch0 low: left only
                0x03, // bank high
                0xC0, 0x20, // ch0 high: right only
            ])
            .unwrap(),
            0,
            OplType::Opl3,
        )
    }

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

    #[test]
    fn soloing_leaves_pans_untouched() {
        let mut panel = ChannelPanel::for_song(&opl3_song_panned());
        panel.set_showcase_pans([0x10; 18]);
        panel.toggle_solo_channel(3);
        panel.toggle_solo_channel(3); // un-solo
        // The pans (and Custom mode) survive a solo round-trip.
        assert_eq!(panel.panning(), Panning::Custom([0x10; 18]));
    }

    #[test]
    fn for_song_defaults_pans_per_type() {
        assert_eq!(
            ChannelPanel::for_song(&opl2_song()).default_pans,
            [0x80; 18]
        );

        let dual = ChannelPanel::for_song(&dual_opl2_song());
        assert_eq!(&dual.default_pans[..9], &[0x00; 9]);
        assert_eq!(&dual.default_pans[9..], &[0xFF; 9]);

        let opl3 = ChannelPanel::for_song(&opl3_song_panned());
        assert_eq!(opl3.default_pans[0], 0x00, "ch0 low left");
        assert_eq!(opl3.default_pans[9], 0xFF, "ch0 high right");
        assert_eq!(opl3.default_pans[1], 0x80, "unwritten channel centred");
    }

    #[test]
    fn panning_policy_matches_the_song_type() {
        // OPL2 Original -> the song's own (mono) output.
        assert_eq!(
            ChannelPanel::for_song(&opl2_song()).panning(),
            Panning::Original
        );
        // OPL3 Original -> the song's own C0 panning.
        assert_eq!(
            ChannelPanel::for_song(&opl3_song_panned()).panning(),
            Panning::Original
        );
        // Dual-OPL2 Original -> the fixed hard-L/R image, ignoring the knobs.
        let mut expected = [0x00u8; 18];
        expected[9..].fill(0xFF);
        assert_eq!(
            ChannelPanel::for_song(&dual_opl2_song()).panning(),
            Panning::Custom(expected)
        );
    }

    #[test]
    fn custom_mode_uses_the_edited_pans() {
        let mut panel = ChannelPanel::for_song(&dual_opl2_song());
        panel.set_showcase_pans([0x55; 18]);
        assert_eq!(panel.panning(), Panning::Custom([0x55; 18]));
    }

    #[test]
    fn set_opl_type_snaps_pans_while_original() {
        let mut panel = ChannelPanel::for_song(&opl2_song()); // all centred
        panel.set_opl_type(OplType::DualOpl2, Some(&dual_opl2_song()));
        // Original mode: the shown pans snap to the new type's defaults.
        assert_eq!(&panel.default_pans[9..], &[0xFF; 9]);
        assert_eq!(&panel.pans[9..], &[0xFF; 9]);

        // Under Custom, an opl_type change updates defaults but not the edited pans.
        panel.set_showcase_pans([0x11; 18]);
        panel.set_opl_type(OplType::Opl2, Some(&opl2_song()));
        assert_eq!(panel.default_pans, [0x80; 18]);
        assert_eq!(panel.pans, [0x11; 18], "Custom pans are preserved");
    }

    #[test]
    fn auto_pan_image_is_a_subtle_alternating_spread() {
        let pans = auto_pan_image();
        for (slot, &pan) in pans.iter().enumerate() {
            if slot % 2 == 0 {
                assert!(pan < PAN_CENTER, "slot {slot} leans left");
            } else {
                assert!(pan > PAN_CENTER, "slot {slot} leans right");
            }
            assert!(pan.abs_diff(PAN_CENTER) <= 0x28, "slot {slot} stays subtle");
        }
        // Alternating sides means every neighbour differs.
        for slot in 0..17 {
            assert_ne!(pans[slot], pans[slot + 1], "slots {slot}/{}", slot + 1);
        }
    }

    #[test]
    fn auto_pan_engages_custom_with_the_spread() {
        let mut panel = ChannelPanel::for_song(&opl2_song());
        panel.apply_auto_pan();
        assert_eq!(panel.panning(), Panning::Custom(auto_pan_image()));
    }
}
