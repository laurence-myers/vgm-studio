//! A mute/solo (and, where the core allows, pan) panel for one chip instance.
//!
//! The chip-agnostic counterpart of [`ChannelPanel`](super::channels::ChannelPanel):
//! where that one knows OPL's two banks, drum groups and stereo-ext image, this
//! one knows only what [`dro_core::vgm::channels_of`] says -- a flat list of
//! named channels. Left-click a toggle to mute, right-click to solo; the "All"
//! button unmutes everything. Pan knobs appear only when the chip's core can
//! place channels in the stereo image (see
//! [`CoreRegistry::pan_capable`](dro_synth::CoreRegistry::pan_capable)); when it
//! cannot, they are omitted rather than shown inert.
//!
//! Its output is a mute mask and a pan array for one `(kind, instance)`, which
//! [`ChipPanels`](super::chip_panels::ChipPanels) folds into the whole
//! document's [`ChipMuting`](dro_synth::ChipMuting) / [`ChipPanning`].

use dro_core::vgm::{ChannelInfo, ChipKind, channels_of};

use super::channels::ChannelsResponse;
use super::pan_knob;
use crate::theme::{Palette, bevel, icon::Icon};

/// The centred pan byte (`0x80`), matching the OPL panel and the knob widget.
const PAN_CENTER: u8 = 0x80;

/// Channels drawn per row before wrapping -- the OPL panel's bank width, so a
/// 16-channel chip reads as two familiar rows rather than one long one.
const ROW: usize = 9;

/// Converts a pan byte (`0x00` left .. `0x80` centre .. `0xFF` right) to
/// libvgm's `-0x100 ..= 0x100` position.
fn pan_to_i16(byte: u8) -> i16 {
    (i16::from(byte) - 128) * 2
}

/// One chip instance's channel controls.
#[derive(Debug, Clone)]
pub struct GenericChannelPanel {
    kind: ChipKind,
    instance: u8,
    channels: &'static [ChannelInfo],
    /// Audible per channel; muting un-lights a toggle.
    audible: Vec<bool>,
    /// Pan byte per channel, edited under Custom mode.
    pans: Vec<u8>,
    /// Whether the pan knobs drive the output (Custom) or the chip's own image
    /// does (Original).
    custom: bool,
}

impl GenericChannelPanel {
    /// A panel for `(kind, instance)`, every channel audible and centred.
    #[must_use]
    pub fn new(kind: ChipKind, instance: u8, variant: bool) -> Self {
        let channels = channels_of(kind, variant);
        Self {
            kind,
            instance,
            channels,
            audible: vec![true; channels.len()],
            pans: vec![PAN_CENTER; channels.len()],
            custom: false,
        }
    }

    /// The chip this panel controls.
    #[must_use]
    pub const fn kind(&self) -> ChipKind {
        self.kind
    }

    /// Which instance of that chip.
    #[must_use]
    pub const fn instance(&self) -> u8 {
        self.instance
    }

    /// The mute mask: bit `i` set for each muted channel, in the canonical
    /// [`channels_of`] order.
    #[must_use]
    pub fn mask(&self) -> u32 {
        let mut mask = 0u32;
        for (index, &audible) in self.audible.iter().enumerate() {
            if !audible {
                mask |= 1 << index;
            }
        }
        mask
    }

    /// The pan positions to apply, or `None` for "leave the chip's own image
    /// alone" (Original mode).
    #[must_use]
    pub fn pan_entry(&self) -> Option<Vec<i16>> {
        self.custom
            .then(|| self.pans.iter().copied().map(pan_to_i16).collect())
    }

    /// Toggles channel `index`, for the number-key shortcuts. Out-of-range
    /// indices (a key past this chip's channel count) are ignored.
    pub fn toggle_channel(&mut self, index: usize) {
        if let Some(audible) = self.audible.get_mut(index) {
            *audible = !*audible;
        }
    }

    /// Solo channel `index`: the only audible voice, or everything back if it
    /// already is. Right-click.
    fn toggle_solo(&mut self, index: usize) {
        if index >= self.audible.len() {
            return;
        }
        if self.is_soloed(index) {
            self.audible.fill(true);
        } else {
            self.audible.fill(false);
            self.audible[index] = true;
        }
    }

    fn is_soloed(&self, index: usize) -> bool {
        self.audible
            .iter()
            .enumerate()
            .all(|(i, &on)| on == (i == index))
    }

    /// Draws the panel. `pan_supported` decides whether the pan knobs and the
    /// Custom/Original toggle appear at all -- omitted, not greyed, when the
    /// core cannot pan. Returns which of muting/panning changed this frame.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        pan_supported: bool,
    ) -> ChannelsResponse {
        let mut response = ChannelsResponse::default();
        let row_height = ui.spacing().interact_size.y;

        egui::Grid::new(("chip-channel-grid", self.kind, self.instance))
            .min_row_height(row_height)
            .min_col_width(0.0)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                let show_pans = pan_supported && self.custom;
                for (row_start, chunk) in self.channels.chunks(ROW).enumerate() {
                    let base = row_start * ROW;
                    // The pan row above this group of toggles, when panning.
                    if show_pans {
                        ui.label(if row_start == 0 { "Pan:" } else { "" });
                        for offset in 0..chunk.len() {
                            let label = self.channels[base + offset].name;
                            response.panning_changed |=
                                pan_knob::show(ui, palette, &mut self.pans[base + offset], true, label)
                                    .changed();
                        }
                        ui.end_row();
                    }
                    // The toggle row: the channel's short label, hover its name.
                    ui.label(if row_start == 0 { "Channels:" } else { "" });
                    for offset in 0..chunk.len() {
                        response.muting_changed |= self.channel_toggle(ui, palette, base + offset);
                    }
                    ui.end_row();
                }

                // The mode/All controls sit on their own row beneath.
                ui.label("");
                if pan_supported {
                    let mut custom = self.custom;
                    if bevel::icon_toggle(ui, palette, &mut custom, Icon::Custom, "Custom")
                        .on_hover_text(crate::strings::CHIP_CHANNELS_CUSTOM)
                        .changed()
                    {
                        self.custom = custom;
                        response.panning_changed = true;
                    }
                }
                if bevel::icon_button(ui, palette, Icon::All, "All")
                    .on_hover_text(crate::strings::CHIP_CHANNELS_UNMUTE_ALL)
                    .clicked()
                {
                    response.muting_changed |= self.audible.iter().any(|&on| !on);
                    self.audible.fill(true);
                }
                ui.end_row();
            });

        response
    }

    /// One channel toggle: audible is lit, muting un-lights it. Left-click
    /// mutes, right-click solos.
    fn channel_toggle(&mut self, ui: &mut egui::Ui, palette: &Palette, index: usize) -> bool {
        let channel = self.channels[index];
        let side = ui.spacing().interact_size.y.max(pan_knob::SIZE);
        let response = bevel::toggle_sized(
            ui,
            palette,
            &mut self.audible[index],
            channel.short,
            egui::vec2(side, side),
        )
        .on_hover_text(crate::strings::chip_channels_channel_hover(channel.name));
        let mut changed = response.changed();
        if response.secondary_clicked() {
            self.toggle_solo(index);
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_sets_a_bit_per_muted_channel() {
        let mut panel = GenericChannelPanel::new(ChipKind::Sn76489, 0, false);
        assert_eq!(panel.mask(), 0, "everything audible");
        panel.toggle_channel(0); // mute Tone 1
        panel.toggle_channel(3); // mute Noise
        assert_eq!(panel.mask(), 0b1001);
    }

    #[test]
    fn solo_leaves_one_channel_and_toggles_back() {
        let mut panel = GenericChannelPanel::new(ChipKind::Ym2612, 0, false);
        panel.toggle_solo(2);
        // 7 channels, only index 2 audible -> the other six muted.
        assert_eq!(panel.mask().count_ones(), 6);
        assert_eq!(panel.mask() & (1 << 2), 0, "the soloed channel plays");
        panel.toggle_solo(2);
        assert_eq!(panel.mask(), 0, "soloing the soloed channel unmutes all");
    }

    #[test]
    fn pans_are_only_reported_in_custom_mode() {
        let mut panel = GenericChannelPanel::new(ChipKind::Ay8910, 0, false);
        assert_eq!(panel.pan_entry(), None, "Original mode: the chip's image");
        panel.custom = true;
        let pans = panel.pan_entry().expect("Custom reports pans");
        assert_eq!(pans.len(), 3, "one per AY channel");
        assert!(pans.iter().all(|&p| p == 0), "centred by default");
    }

    #[test]
    fn a_number_key_past_the_channel_count_is_ignored() {
        let mut panel = GenericChannelPanel::new(ChipKind::Sn76489, 0, false);
        panel.toggle_channel(17); // shift+9 on a 4-channel chip
        assert_eq!(panel.mask(), 0, "no channel to toggle, no change");
    }

    #[test]
    fn the_fds_variant_gives_the_nes_its_sixth_channel() {
        let plain = GenericChannelPanel::new(ChipKind::NesApu, 0, false);
        assert_eq!(plain.audible.len(), 5);
        let fds = GenericChannelPanel::new(ChipKind::NesApu, 0, true);
        assert_eq!(fds.audible.len(), 6);
    }
}
