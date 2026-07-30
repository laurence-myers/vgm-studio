//! Channel and percussion muting, soloing, and per-channel panning.
//!
//! Eighteen melodic-channel toggles (nine per bank) plus a drums toggle per bank,
//! applied live through `AudioService::set_muting`; and, above each toggle, a pan
//! knob applied through `AudioService::set_panning` when the panel is in Custom
//! mode.

use vgms_core::{Bank, OplType, Song};
use vgms_synth::{Muting, Panning};

use crate::theme::{Palette, bevel, icon::Icon};
use crate::widgets::pan_knob;

/// The centred pan byte (`0x80`), and the hard-left / hard-right extremes.
const PAN_CENTER: u8 = 0x80;
const PAN_LEFT: u8 = 0x00;
const PAN_RIGHT: u8 = 0xFF;

/// The auto-spread template (scaled by the Spread knob's strength): how far the
/// first channel of each group of five leans off centre at full strength, and how
/// much each successive channel widens, so neighbours never share a value. Wide,
/// but short of a hard split -- `84 + 4*9 = 120`, so `centre +/- 120` never clips.
const SPREAD_BASE: f32 = 84.0;
const SPREAD_STEP: f32 = 9.0;

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
    /// restored by "Reset".
    default_pans: [u8; 18],
    /// The last strength applied via the Spread knob (`-1.0..=1.0`, `0.0` mono).
    /// Drives all the pans through [`spread_pans`]; kept so the knob shows where
    /// it was left.
    spread: f32,
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
            spread: 0.0,
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
        OplType::Opl3 => vgms_core::initial_channel_pans(song),
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

/// One channel's signed distance from centre in the auto-spread template, before
/// the Spread knob's strength scales it: even channels lean left (negative), odd
/// lean right (positive), widening gently across each group of five.
fn spread_delta(slot: usize) -> f32 {
    let amount = SPREAD_BASE + (slot % 5) as f32 * SPREAD_STEP;
    if slot.is_multiple_of(2) {
        -amount // even channels lean left
    } else {
        amount // odd channels lean right
    }
}

/// The pan image for a spread `strength` in `-1.0..=1.0`: `centre + strength *
/// template_delta`, clamped to a byte. `0.0` is mono (everything centred); the
/// extremes give a wide stereo image, its sign mirroring which side each channel
/// leans.
fn spread_pans(strength: f32) -> [u8; 18] {
    let mut pans = [PAN_CENTER; 18];
    for (slot, pan) in pans.iter_mut().enumerate() {
        let value = f32::from(PAN_CENTER) + strength * spread_delta(slot);
        *pan = value.round().clamp(0.0, 255.0) as u8;
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
            spread: 0.0,
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

    /// Applies a stereo-spread `strength` (`-1.0..=1.0`) to the pans and engages
    /// Custom mode so it takes effect. Remembers the strength for the knob.
    fn set_spread(&mut self, strength: f32) {
        self.spread = strength.clamp(-1.0, 1.0);
        self.pans = spread_pans(self.spread);
        self.custom = true;
    }

    /// Resets panning to the song type's default image and returns to Original
    /// mode, with the spread back to mono. Returns whether the *effective*
    /// panning changed (so the caller only resends when it must).
    fn reset_pans(&mut self) -> bool {
        let before = self.panning();
        self.pans = self.default_pans;
        self.spread = 0.0;
        self.custom = false;
        before != self.panning()
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
    /// of muting/panning changed this frame. `panning_supported` is false when the
    /// output cannot pan (hardware playback mixes on the chip), which omits the pan
    /// controls.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        panning_supported: bool,
    ) -> ChannelsResponse {
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
                // Low bank: the pan row (only when the output can pan), then the
                // toggle row with Perc. and All. The toggles are pinned to a fixed
                // rect so they never grow or jostle their neighbours.
                if panning_supported {
                    response.panning_changed |= self.pan_row(ui, palette, 0, true);
                    ui.end_row();
                }

                ui.label("Channels:");
                for index in 0..9 {
                    response.muting_changed |= self.channel_toggle(ui, palette, index);
                }
                response.muting_changed |= self.percussion_toggle(
                    ui,
                    palette,
                    0,
                    "Perc.",
                    crate::strings::CHANNELS_PERCUSSION_LOW,
                );
                if bevel::icon_button(ui, palette, Icon::All, "All")
                    .on_hover_text(crate::strings::CHANNELS_UNMUTE_ALL)
                    .clicked()
                {
                    response.muting_changed |=
                        self.channels != [true; 18] || self.percussion != [true; 2];
                    self.unmute_all();
                }
                ui.end_row();

                if show_high_bank {
                    if panning_supported {
                        response.panning_changed |= self.pan_row(ui, palette, 1, false);
                        ui.end_row();
                    }

                    ui.label("High bank:");
                    for index in 9..18 {
                        response.muting_changed |= self.channel_toggle(ui, palette, index);
                    }
                    response.muting_changed |= self.percussion_toggle(
                        ui,
                        palette,
                        1,
                        "Perc.",
                        crate::strings::CHANNELS_PERCUSSION_HIGH,
                    );
                    ui.end_row();
                }
            });
        response
    }

    /// One bank's pan row: a label, nine pan knobs (over the toggle digits), an
    /// empty cell in the Perc. column, and -- on the low bank -- the
    /// Original/Custom mode toggle under "All". Returns whether panning changed.
    ///
    /// Only called when the output can pan; when it cannot the whole row is
    /// omitted (see [`Self::show`]), so there is no disabled state to draw here.
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
            let label = crate::strings::channels_pan_label(channel, bank_name);
            if self.custom {
                changed |=
                    pan_knob::show(ui, palette, &mut self.pans[slot], true, &label).changed();
            } else {
                // Original mode: the knob shows the policy pan, inert. It does
                // not mutate a disabled value, so a throwaway copy is safe.
                let mut shown = self.default_pans[slot];
                pan_knob::show(ui, palette, &mut shown, false, &label);
            }
        }
        // The Perc. column has no pan knob (drums pan through channels 7-9).
        ui.label("");
        if with_mode_toggle {
            let hint = match self.opl_type {
                Some(OplType::DualOpl2) => crate::strings::CHANNELS_ORIGINAL_DUAL_OPL2,
                Some(OplType::Opl3) => crate::strings::CHANNELS_ORIGINAL_OPL3,
                _ => crate::strings::CHANNELS_ORIGINAL_MONO,
            };
            let mut custom = self.custom;
            if bevel::icon_toggle(ui, palette, &mut custom, Icon::Custom, "Custom")
                .on_hover_text(hint)
                .changed()
            {
                self.custom = custom;
                // Switching mode changes the effective panning.
                changed = true;
            }
            // The Spread knob: one global stereo-width control, -1..+1. 0 is mono,
            // the extremes a wide image. Engages Custom so it is heard at once.
            ui.horizontal(|ui| {
                ui.label("Spread:");
                let mut spread = self.spread;
                if pan_knob::show_spread(ui, palette, &mut spread, "Spread")
                    .on_hover_text(crate::strings::CHANNELS_SPREAD)
                    .changed()
                {
                    self.set_spread(spread);
                    changed = true;
                }
                if bevel::icon_button(ui, palette, Icon::Reset, "Reset")
                    .on_hover_text(crate::strings::CHANNELS_RESET)
                    .clicked()
                {
                    changed |= self.reset_pans();
                }
            });
        }
        changed
    }

    /// One melodic-channel toggle: audible is lit (amber), muting un-lights it
    /// to a plain pad. Left-click mutes, right-click solos.
    ///
    /// A fixed square cell (at least the row height wide) so the digit never
    /// resizes with the button state and the column stays put; the pan knob
    /// above it centres in the same column.
    fn channel_toggle(&mut self, ui: &mut egui::Ui, palette: &Palette, index: usize) -> bool {
        let label = (index % 9 + 1).to_string();
        let row_h = ui.spacing().interact_size.y;
        let side = row_h.max(pan_knob::SIZE);
        let response = bevel::toggle_sized(
            ui,
            palette,
            &mut self.channels[index],
            &label,
            egui::vec2(side, side),
        )
        .on_hover_text(crate::strings::channels_channel_hover(index));
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
        let response =
            bevel::icon_toggle(ui, palette, &mut self.percussion[bank], Icon::Perc, label)
                .on_hover_text(crate::strings::channels_percussion_hover(hover));
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
    use vgms_core::{DroDataV1, Song};

    /// The panel keeps its edited pans while the output cannot use them, so
    /// switching back to the emulator restores the image rather than resetting it.
    /// Only the controls are disabled; `panning()` still reports what it holds.
    #[test]
    fn an_output_that_cannot_pan_does_not_discard_the_panel_state() {
        let mut panel = ChannelPanel::for_song(&opl2_song());
        panel.custom = true;
        panel.pans[0] = PAN_LEFT;

        let Panning::Custom(pans) = panel.panning() else {
            panic!("Custom mode should report custom pans");
        };
        assert_eq!(pans[0], PAN_LEFT);
    }

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
    fn spread_pans_is_mono_at_zero_and_wide_at_the_extremes() {
        // Mono: everything dead centre.
        assert_eq!(spread_pans(0.0), [PAN_CENTER; 18]);

        // Full strength: even channels lean left, odd right, neighbours differ,
        // and it is genuinely wide (well past the old subtle range) without
        // clipping at the byte extremes.
        let wide = spread_pans(1.0);
        for (slot, &pan) in wide.iter().enumerate() {
            if slot % 2 == 0 {
                assert!(pan < PAN_CENTER, "slot {slot} leans left");
            } else {
                assert!(pan > PAN_CENTER, "slot {slot} leans right");
            }
            assert!(pan > PAN_LEFT && pan < PAN_RIGHT, "slot {slot} never clips");
        }
        assert!(
            wide.iter().any(|&pan| pan.abs_diff(PAN_CENTER) >= 0x50),
            "the extreme is genuinely wide"
        );
        for slot in 0..17 {
            assert_ne!(wide[slot], wide[slot + 1], "slots {slot}/{}", slot + 1);
        }

        // The sign mirrors the image: -1 is +1 reflected about centre.
        let mirror = spread_pans(-1.0);
        for slot in 0..18 {
            assert_eq!(
                i16::from(mirror[slot]) - 128,
                128 - i16::from(wide[slot]),
                "slot {slot} mirrors"
            );
        }
    }

    #[test]
    fn set_spread_engages_custom_with_the_spread_image() {
        let mut panel = ChannelPanel::for_song(&opl2_song());
        panel.set_spread(1.0);
        assert_eq!(panel.panning(), Panning::Custom(spread_pans(1.0)));
        // Dialling back to mono is still Custom (centred), not Original.
        panel.set_spread(0.0);
        assert_eq!(panel.panning(), Panning::Custom([PAN_CENTER; 18]));
    }

    #[test]
    fn reset_pans_returns_to_default_and_original() {
        let mut panel = ChannelPanel::for_song(&opl2_song());
        panel.set_spread(1.0);
        assert!(matches!(panel.panning(), Panning::Custom(_)));

        assert!(panel.reset_pans(), "leaving Custom changes the panning");
        assert_eq!(panel.panning(), Panning::Original);
        assert_eq!(panel.spread, 0.0, "spread is back to mono");
        assert!(!panel.custom);

        // Already at the default: a second reset changes nothing.
        assert!(!panel.reset_pans());
    }

    #[test]
    fn unmuting_all_leaves_the_pans_alone() {
        let mut panel = ChannelPanel::for_song(&opl2_song());
        panel.set_spread(1.0);
        let spread_image = panel.panning();
        panel.toggle_channel(3); // mute one

        panel.unmute_all();
        assert_eq!(panel.muting(), Muting::all(), "everything audible again");
        assert_eq!(panel.panning(), spread_image, "panning untouched by unmute");
    }
}
