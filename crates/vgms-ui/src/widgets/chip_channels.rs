//! The channel panel: a mute/solo (and, where the core allows, pan) panel for
//! one chip instance.
//!
//! It knows only what [`vgms_core::vgm::channels_of`] says -- a flat list of
//! named channels -- so it serves every chip, OPL included: a DRO drives it
//! through its OPL projection just as a VGM drives it for each of its chips.
//! Left-click a toggle to mute, right-click to solo; the "All" button unmutes
//! everything. Pan knobs appear only when the chip's core can place channels in
//! the stereo image (see
//! [`CoreRegistry::pan_capable`](vgms_synth::CoreRegistry::pan_capable)); when it
//! cannot, they are omitted rather than shown inert.
//!
//! Its output is a mute mask and a pan array for one `(kind, instance)`, which
//! [`ChipPanels`](super::chip_panels::ChipPanels) folds into the whole
//! document's [`ChipMuting`](vgms_synth::ChipMuting) / [`ChipPanning`] -- or, for
//! a DRO, bridges back to the OPL `Muting`/`Panning` its audio path consumes.

use vgms_core::vgm::{ChannelInfo, ChipKind, channels_of};

use super::pan_controls::{self, PAN_CENTER};
use super::pan_knob;
use crate::theme::{Palette, bevel, icon::Icon};

/// What a channel panel (and the chip deck's selector) changed this frame,
/// split so a pan drag never resends muting mid-note, a mute toggle never
/// resends panning, and a trim drag resends only the trims.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ChannelsResponse {
    pub(crate) muting_changed: bool,
    pub(crate) panning_changed: bool,
    pub(crate) trim_changed: bool,
}

/// Channels drawn per row before wrapping -- the OPL panel's bank width, so a
/// 16-channel chip reads as two familiar rows rather than one long one.
const ROW: usize = 9;

/// The full trim, and the level a fresh panel rests at: 100%, the reference
/// balance untouched. Mirrors [`vgms_synth::ChipTrims`]'s full.
const TRIM_FULL: u8 = 100;

/// How long a keyboard-toggled pad's acknowledgment glow takes to fade.
const FLASH_SECS: f64 = 0.4;

/// Converts a pan byte (`0x00` left .. `0x80` centre .. `0xFF` right) to
/// libvgm's `-0x100 ..= 0x100` position.
///
/// Anchored on `0x80`, with each side scaled independently (128 steps left, 127
/// right) so both extremes reach full magnitude -- the same asymmetric mapping
/// [`pan_knob::dot_angle`](super::pan_knob) and the R/L readout use. The old
/// `(byte - 128) * 2` fell two short on the right (`0xFF` -> 254, so a channel
/// the readout called "R100" was really at 98%).
fn pan_to_i16(byte: u8) -> i16 {
    const CENTER: i16 = 0x80;
    const FULL: i16 = 0x100;
    let value = i16::from(byte);
    match value.cmp(&CENTER) {
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Less => -FULL * (CENTER - value) / CENTER,
        std::cmp::Ordering::Greater => FULL * (value - CENTER) / (255 - CENTER),
    }
}

/// One chip instance's channel controls.
#[derive(Debug, Clone)]
pub(crate) struct GenericChannelPanel {
    kind: ChipKind,
    instance: u8,
    channels: &'static [ChannelInfo],
    /// Audible per channel; muting un-lights a toggle.
    audible: Vec<bool>,
    /// The whole chip muted by the user -- the chip lamp's left-click, over and
    /// above the per-channel toggles. Kept separate from `audible` so un-muting
    /// the chip restores the channel pattern, and separate from `soloed` so
    /// "muted by you" stays distinct from "silenced for you".
    chip_muted: bool,
    /// The chip soloed by the user -- the chip lamp's right-click. An explicit
    /// per-chip flag, so a solo never touches a sibling's `chip_muted`; the deck
    /// keeps it exclusive (soloing one chip clears the rest) in
    /// [`ChipPanels::solo_only`](super::chip_panels). Effective silence, folded
    /// in by [`Self::mask_effective`], is `chip_muted || (any_solo && !soloed)`.
    soloed: bool,
    /// The user's listening trim for this chip, `0..=100`% (100% the reference
    /// balance). Set from the chip lamp's knob; folded into the document's
    /// [`ChipTrims`](vgms_synth::ChipTrims) by the deck.
    trim: u8,
    /// Pan byte per channel, edited under Custom mode.
    pans: Vec<u8>,
    /// The last strength applied via the Spread knob (`-1.0..=1.0`, `0.0` mono),
    /// kept so the knob shows where it was left -- as the OPL panel's does.
    spread: f32,
    /// Whether the pan knobs drive the output (Custom) or the chip's own image
    /// does (Original).
    custom: bool,
    /// A pad to glow briefly: armed when a number key toggles a channel, so the
    /// keystroke has a visible acknowledgment on the pad it hit. The start time
    /// is stamped on the first frame the pad draws (the toggle happens outside
    /// the draw, with no clock in reach); the deck clears it while folded, so a
    /// stale glow never greets a later unfold.
    flash: Option<(usize, Option<f64>)>,
}

impl GenericChannelPanel {
    /// A panel for `(kind, instance)`, every channel audible and centred.
    #[must_use]
    pub(crate) fn new(kind: ChipKind, instance: u8, variant: bool) -> Self {
        let channels = channels_of(kind, variant);
        Self {
            kind,
            instance,
            channels,
            audible: vec![true; channels.len()],
            chip_muted: false,
            soloed: false,
            trim: TRIM_FULL,
            pans: vec![PAN_CENTER; channels.len()],
            spread: 0.0,
            custom: false,
            flash: None,
        }
    }

    /// The chip this panel controls.
    #[must_use]
    pub(crate) const fn kind(&self) -> ChipKind {
        self.kind
    }

    /// Which instance of that chip.
    #[must_use]
    pub(crate) const fn instance(&self) -> u8 {
        self.instance
    }

    /// The mute mask: bit `i` set for each muted channel, in the canonical
    /// [`channels_of`] order. A chip-level mute covers every bit -- which the
    /// engine also reads as "silence this voice entirely", so it holds even
    /// for a core that cannot mute single channels.
    #[must_use]
    pub(crate) fn mask(&self) -> u32 {
        if self.chip_muted {
            return (1u32 << self.channels.len()) - 1;
        }
        let mut mask = 0u32;
        for (index, &audible) in self.audible.iter().enumerate() {
            if !audible {
                mask |= 1 << index;
            }
        }
        mask
    }

    /// The effective mute mask for the engine, given whether *any* chip in the
    /// document is soloed: a chip that is not itself soloed while a solo is
    /// active is silenced whole, on top of its own [`mask`](Self::mask). This is
    /// the `chip_muted || (any_solo && !soloed)` rule -- a document-level fact,
    /// so the deck passes `any_solo` in rather than the panel guessing it.
    #[must_use]
    pub(crate) fn mask_effective(&self, any_solo: bool) -> u32 {
        if any_solo && !self.soloed {
            return (1u32 << self.channels.len()) - 1;
        }
        self.mask()
    }

    /// Whether the whole chip is muted by the user (the lamp's left-click).
    #[must_use]
    pub(crate) const fn chip_muted(&self) -> bool {
        self.chip_muted
    }

    /// Mutes or unmutes the whole chip, leaving the per-channel pattern to
    /// come back when it is unmuted.
    pub(crate) fn set_chip_muted(&mut self, muted: bool) {
        self.chip_muted = muted;
    }

    /// Whether the user has soloed this chip (the lamp's right-click).
    #[must_use]
    pub(crate) const fn soloed(&self) -> bool {
        self.soloed
    }

    /// Sets this chip's solo flag -- only this chip's, never a sibling's mute.
    /// The deck applies the exclusive-solo rule (soloing one chip clears the
    /// rest) in [`ChipPanels::solo_only`](super::chip_panels).
    pub(crate) fn set_soloed(&mut self, soloed: bool) {
        self.soloed = soloed;
    }

    /// This chip's listening trim, `0..=100`%.
    #[must_use]
    pub(crate) const fn trim(&self) -> u8 {
        self.trim
    }

    /// Sets this chip's listening trim, clamped to `0..=100`%.
    pub(crate) fn set_trim(&mut self, trim: u8) {
        self.trim = trim.min(TRIM_FULL);
    }

    /// The pan positions to apply, or `None` for "leave the chip's own image
    /// alone" (Original mode).
    #[must_use]
    pub(crate) fn pan_entry(&self) -> Option<Vec<i16>> {
        self.custom
            .then(|| self.pans.iter().copied().map(pan_to_i16).collect())
    }

    /// This panel's raw pan bytes while it is in Custom mode, or `None` in
    /// Original mode. The OPL bridge reads these directly (rather than the
    /// converted [`pan_entry`](Self::pan_entry)) because the OPL audio path
    /// re-derives its `i16` positions from the byte image itself, matching the
    /// old OPL panel's exact round trip.
    #[must_use]
    pub(crate) fn custom_pan_bytes(&self) -> Option<&[u8]> {
        self.custom.then_some(self.pans.as_slice())
    }

    /// Test-only: engage Custom mode and set one channel's pan byte, for the
    /// panning showcase and the app-level panning tests.
    #[cfg(test)]
    pub(crate) fn set_showcase_pan(&mut self, channel: usize, byte: u8) {
        self.custom = true;
        if let Some(pan) = self.pans.get_mut(channel) {
            *pan = byte;
        }
    }

    /// Toggles channel `index`, for the number-key shortcuts. Out-of-range
    /// indices (a key past this chip's channel count) are ignored. Arms the
    /// pad's acknowledgment glow, so the keystroke is visible on the pad it hit.
    pub(crate) fn toggle_channel(&mut self, index: usize) {
        if let Some(audible) = self.audible.get_mut(index) {
            *audible = !*audible;
            self.flash = Some((index, None));
        }
    }

    /// Drops any armed pad glow. The deck calls this while it is folded, so a
    /// keystroke made then does not flash a pad long after, on unfold.
    pub(crate) fn clear_flash(&mut self) {
        self.flash = None;
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

    /// Applies a stereo-spread `strength` (`-1.0..=1.0`) across this chip's
    /// channels and engages Custom mode so it takes effect -- the OPL panel's
    /// Spread knob, over however many voices this chip has.
    fn set_spread(&mut self, strength: f32) {
        self.spread = strength.clamp(-1.0, 1.0);
        pan_controls::spread_into(&mut self.pans, self.spread);
        self.custom = true;
    }

    /// Resets panning to centred and returns to Original mode (the chip's own
    /// image), with the spread back to mono. Returns whether the *effective*
    /// panning changed, so the caller only resends when it must.
    fn reset_pans(&mut self) -> bool {
        let before = self.pan_entry();
        self.pans.fill(PAN_CENTER);
        self.spread = 0.0;
        self.custom = false;
        before != self.pan_entry()
    }

    /// Draws the panel: each group of channels is a pan row directly above its
    /// toggle row, "All" leads the first toggle row, and the Custom latch sits
    /// above it in the pan row's "All" column, with the Spread knob and Reset
    /// button closing the first pan row past the knobs.
    ///
    /// `pan_supported` decides whether the pan controls appear at all -- omitted,
    /// not greyed, when the core cannot pan. When it can, the knobs are always
    /// *shown*, live only under Custom: a control you can see and reach for is
    /// how Custom is discovered in the first place. `mute_supported` decides
    /// whether the channel toggles are live: a core with no channel-mute (the
    /// Nuked family) gets *disabled* toggles with an explaining tooltip, rather
    /// than toggles that light up and silence nothing. Returns which of
    /// muting/panning changed this frame.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        pan_supported: bool,
        mute_supported: bool,
    ) -> ChannelsResponse {
        let mut response = ChannelsResponse::default();
        let row_height = ui.spacing().interact_size.y;

        egui::Grid::new(("chip-channel-grid", self.kind, self.instance))
            .min_row_height(row_height)
            .min_col_width(0.0)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                for (row_start, chunk) in self.channels.chunks(ROW).enumerate() {
                    let base = row_start * ROW;
                    // The pan row above this group of toggles.
                    if pan_supported {
                        ui.label(if row_start == 0 { "Pan:" } else { "" });
                        // The "All" column: the Custom latch on the first row,
                        // directly above the "All" button; empty on the rest.
                        let live = self.custom;
                        if row_start == 0 {
                            response.panning_changed |= self.custom_toggle(ui, palette);
                        } else {
                            ui.label("");
                        }
                        for offset in 0..chunk.len() {
                            let label = self.channels[base + offset].name;
                            response.panning_changed |= pan_knob::show(
                                ui,
                                palette,
                                &mut self.pans[base + offset],
                                live,
                                label,
                            )
                            .changed();
                        }
                        // The Spread knob and Reset button close the first pan
                        // row, past the knobs they govern.
                        if row_start == 0 {
                            response.panning_changed |= self.spread_reset(ui, palette);
                        }
                        ui.end_row();
                    }
                    // The toggle row: the channel's short label, hover its name.
                    ui.label(if row_start == 0 { "Channels:" } else { "" });
                    if row_start == 0 {
                        response.muting_changed |= self.all_button(ui, palette, mute_supported);
                    } else {
                        ui.label(""); // one "All" covers every row
                    }
                    for offset in 0..chunk.len() {
                        response.muting_changed |=
                            self.channel_toggle(ui, palette, base + offset, mute_supported);
                    }
                    ui.end_row();
                }
            });

        response
    }

    /// The Custom/Original latch, in the pan row's "All" column above the "All"
    /// button. Returns whether the effective panning changed.
    fn custom_toggle(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut custom = self.custom;
        if pan_controls::custom_toggle(
            ui,
            palette,
            &mut custom,
            crate::strings::CHIP_CHANNELS_CUSTOM,
        ) {
            self.custom = custom;
            return true;
        }
        false
    }

    /// The global Spread knob and Reset button, closing the first pan row.
    /// Returns whether the effective panning changed.
    fn spread_reset(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut spread = self.spread;
        let mode = pan_controls::spread_reset(
            ui,
            palette,
            &mut spread,
            crate::strings::CHIP_CHANNELS_RESET,
        );
        let mut changed = false;
        if mode.spread_changed {
            self.set_spread(spread);
            changed = true;
        }
        if mode.reset {
            changed |= self.reset_pans();
        }
        changed
    }

    /// "All": unmutes every channel. Moot when muting does nothing, so it is
    /// disabled with the toggles it leads. Returns whether the muting changed.
    fn all_button(&mut self, ui: &mut egui::Ui, palette: &Palette, mute_supported: bool) -> bool {
        let all = ui
            .add_enabled_ui(mute_supported, |ui| {
                bevel::icon_button(ui, palette, Icon::All, "All")
            })
            .inner;
        let all = if mute_supported {
            all.on_hover_text(crate::strings::CHIP_CHANNELS_UNMUTE_ALL)
        } else {
            all.on_disabled_hover_text(crate::strings::CHIP_CHANNELS_MUTE_UNAVAILABLE)
        };
        if !all.clicked() {
            return false;
        }
        let changed = self.audible.iter().any(|&on| !on);
        self.audible.fill(true);
        changed
    }

    /// One channel toggle: audible is lit, muting un-lights it. Left-click
    /// mutes, right-click solos. When `mute_supported` is false the toggle is
    /// disabled and only explains why on hover -- the resolved core cannot mute.
    fn channel_toggle(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        index: usize,
        mute_supported: bool,
    ) -> bool {
        let channel = self.channels[index];
        let side = ui.spacing().interact_size.y.max(pan_knob::SIZE);
        let response = ui
            .add_enabled_ui(mute_supported, |ui| {
                bevel::toggle_sized(
                    ui,
                    palette,
                    &mut self.audible[index],
                    channel.short,
                    egui::vec2(side, side),
                )
            })
            .inner;
        self.draw_flash(ui, palette, index, response.rect);
        if !mute_supported {
            response.on_disabled_hover_text(crate::strings::CHIP_CHANNELS_MUTE_UNAVAILABLE);
            return false;
        }
        let response =
            response.on_hover_text(crate::strings::chip_channels_channel_hover(channel.name));
        let mut changed = response.changed();
        if response.secondary_clicked() {
            self.toggle_solo(index);
            changed = true;
        }
        changed
    }

    /// The keyboard acknowledgment: a brief amber glow over the pad a number
    /// key toggled. Stamps its start on the first frame the pad draws, fades
    /// over [`FLASH_SECS`], and keeps the repaints coming while it lives.
    fn draw_flash(&mut self, ui: &egui::Ui, palette: &Palette, index: usize, rect: egui::Rect) {
        let Some((at, started)) = self.flash else {
            return;
        };
        if at != index {
            return;
        }
        let now = ui.input(|input| input.time);
        let started = started.unwrap_or_else(|| {
            self.flash = Some((at, Some(now)));
            now
        });
        let age = now - started;
        if age >= FLASH_SECS {
            self.flash = None;
            return;
        }
        // A quadratic fade reads more like a phosphor than a linear one.
        let strength = (1.0 - (age / FLASH_SECS) as f32).powi(2);
        ui.painter().rect_filled(
            rect.expand(2.0),
            egui::CornerRadius::same(3),
            palette.latch_bottom.gamma_multiply(strength * 0.45),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keyboard toggle arms the pad glow (unstamped until it first draws);
    /// the deck's fold-time clear disarms it.
    #[test]
    fn a_keyboard_toggle_arms_the_pad_flash() {
        let mut panel = GenericChannelPanel::new(ChipKind::Sn76489, 0, false);
        panel.toggle_channel(1);
        assert_eq!(panel.flash, Some((1, None)));
        panel.clear_flash();
        assert_eq!(panel.flash, None);
        // An out-of-range key toggles nothing and arms nothing.
        panel.toggle_channel(99);
        assert_eq!(panel.flash, None);
    }

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
    fn the_pan_extremes_reach_full_left_and_full_right() {
        assert_eq!(pan_to_i16(0x80), 0, "centre");
        assert_eq!(pan_to_i16(0x00), -0x100, "hard left is full -0x100");
        assert_eq!(
            pan_to_i16(0xFF),
            0x100,
            "hard right is full +0x100, not 254"
        );
    }

    #[test]
    fn a_number_key_past_the_channel_count_is_ignored() {
        let mut panel = GenericChannelPanel::new(ChipKind::Sn76489, 0, false);
        panel.toggle_channel(17); // shift+9 on a 4-channel chip
        assert_eq!(panel.mask(), 0, "no channel to toggle, no change");
    }

    #[test]
    fn a_solo_elsewhere_silences_this_chip_whole() {
        let mut panel = GenericChannelPanel::new(ChipKind::Ym2612, 0, false);
        let full = (1u32 << panel.channels.len()) - 1;
        // No solo active: the panel plays its own (empty) mask.
        assert_eq!(panel.mask_effective(false), 0);
        // A solo is active and this chip is not the soloed one: silenced whole.
        assert_eq!(
            panel.mask_effective(true),
            full,
            "silenced by another's solo"
        );
        // Now this chip is the soloed one: it plays.
        panel.set_soloed(true);
        assert_eq!(panel.mask_effective(true), 0, "the soloed chip plays");
        // A user mute always silences, solo or not.
        panel.set_chip_muted(true);
        assert_eq!(panel.mask_effective(true), full, "an explicit mute wins");
    }

    #[test]
    fn a_trim_defaults_full_and_clamps() {
        let mut panel = GenericChannelPanel::new(ChipKind::Sn76489, 0, false);
        assert_eq!(
            panel.trim(),
            100,
            "a fresh panel is at the reference balance"
        );
        panel.set_trim(40);
        assert_eq!(panel.trim(), 40);
        panel.set_trim(250);
        assert_eq!(panel.trim(), 100, "a trim over 100% is clamped");
    }

    #[test]
    fn the_fds_variant_gives_the_nes_its_sixth_channel() {
        let plain = GenericChannelPanel::new(ChipKind::NesApu, 0, false);
        assert_eq!(plain.audible.len(), 5);
        let fds = GenericChannelPanel::new(ChipKind::NesApu, 0, true);
        assert_eq!(fds.audible.len(), 6);
    }
}
