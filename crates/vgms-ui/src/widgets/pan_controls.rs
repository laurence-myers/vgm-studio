//! The panning controls both channel panels share: the auto-spread image, and
//! the Custom / Spread / Reset group that drives it.
//!
//! The OPL panel ([`ChannelPanel`](super::channels::ChannelPanel)) and the
//! any-chip one ([`GenericChannelPanel`](super::chip_channels::GenericChannelPanel))
//! hold their own pans -- eighteen fixed slots against a chip's channel list --
//! but the *controls* over them are one design: a mode latch, one global width
//! knob, and a reset. They live here so a core that pans gets exactly what OPL
//! gets, rather than a second implementation that drifts.

use crate::theme::{Palette, bevel, icon::Icon};
use crate::widgets::pan_knob;

/// The centred pan byte (`0x80`), and the hard-left / hard-right extremes.
pub const PAN_CENTER: u8 = 0x80;
pub const PAN_LEFT: u8 = 0x00;
pub const PAN_RIGHT: u8 = 0xFF;

/// The auto-spread template (scaled by the Spread knob's strength): how far the
/// first channel of each group of five leans off centre at full strength, and how
/// much each successive channel widens, so neighbours never share a value. Wide,
/// but short of a hard split -- `84 + 4*9 = 120`, so `centre +/- 120` never clips.
const SPREAD_BASE: f32 = 84.0;
const SPREAD_STEP: f32 = 9.0;

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

/// Writes the pan image for a spread `strength` in `-1.0..=1.0` into `pans`:
/// `centre + strength * template_delta`, clamped to a byte. `0.0` is mono
/// (everything centred); the extremes give a wide stereo image, its sign
/// mirroring which side each channel leans.
///
/// Takes a slice rather than returning an array, so the same image serves OPL's
/// eighteen slots and a chip's however-many.
pub fn spread_into(pans: &mut [u8], strength: f32) {
    for (slot, pan) in pans.iter_mut().enumerate() {
        let value = f32::from(PAN_CENTER) + strength * spread_delta(slot);
        *pan = value.round().clamp(0.0, 255.0) as u8;
    }
}

/// What [`mode_controls`] changed this frame. The fields are separate because the
/// caller answers each differently: a mode flip only changes which image applies,
/// a spread drag rewrites every pan, and a reset restores the defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct PanModeResponse {
    /// The Custom/Original latch flipped (`custom` now holds the new mode).
    pub mode_toggled: bool,
    /// The Spread knob moved (`spread` now holds the new strength).
    pub spread_changed: bool,
    /// Reset was clicked.
    pub reset: bool,
}

impl PanModeResponse {
    /// Whether anything at all changed -- what a caller with a single "panning
    /// changed" flag reports.
    #[must_use]
    pub const fn any(self) -> bool {
        self.mode_toggled || self.spread_changed || self.reset
    }
}

/// Draws the Custom latch, the Spread knob and the Reset button in a row,
/// writing the latch and the strength back through `custom` / `spread`.
///
/// `custom_hover` differs per panel (OPL names what Original means for the song
/// type; a generic chip says "the chip's own image"), as does `reset_hover`.
pub fn mode_controls(
    ui: &mut egui::Ui,
    palette: &Palette,
    custom: &mut bool,
    spread: &mut f32,
    custom_hover: &str,
    reset_hover: &str,
) -> PanModeResponse {
    let mut response = PanModeResponse::default();
    ui.horizontal(|ui| {
        if bevel::icon_toggle(ui, palette, custom, Icon::Custom, "Custom")
            .on_hover_text(custom_hover)
            .changed()
        {
            response.mode_toggled = true;
        }
        // One global stereo-width control, -1..+1. 0 is mono, the extremes a wide
        // image. A drag engages Custom in the caller, so it is heard at once.
        ui.label("Spread:");
        if pan_knob::show_spread(ui, palette, spread, "Spread")
            .on_hover_text(crate::strings::CHANNELS_SPREAD)
            .changed()
        {
            response.spread_changed = true;
        }
        if bevel::icon_button(ui, palette, Icon::Reset, "Reset")
            .on_hover_text(reset_hover)
            .clicked()
        {
            response.reset = true;
        }
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spread_image_is_mono_at_zero_and_wide_at_the_extremes() {
        let mut pans = [0u8; 18];
        spread_into(&mut pans, 0.0);
        assert_eq!(pans, [PAN_CENTER; 18], "mono: everything dead centre");

        spread_into(&mut pans, 1.0);
        for (slot, &pan) in pans.iter().enumerate() {
            if slot % 2 == 0 {
                assert!(pan < PAN_CENTER, "slot {slot} leans left");
            } else {
                assert!(pan > PAN_CENTER, "slot {slot} leans right");
            }
            assert!(pan > PAN_LEFT && pan < PAN_RIGHT, "slot {slot} never clips");
        }
        for slot in 0..17 {
            assert_ne!(pans[slot], pans[slot + 1], "slots {slot}/{}", slot + 1);
        }
    }

    /// The image fits whatever it is given -- a three-voice AY as readily as
    /// OPL's eighteen slots.
    #[test]
    fn a_short_channel_list_takes_the_first_slots_of_the_template() {
        let mut wide = [0u8; 18];
        spread_into(&mut wide, 1.0);
        let mut ay = [0u8; 3];
        spread_into(&mut ay, 1.0);
        assert_eq!(ay, wide[..3]);
    }

    #[test]
    fn the_sign_mirrors_the_image() {
        let (mut right, mut left) = ([0u8; 18], [0u8; 18]);
        spread_into(&mut right, 1.0);
        spread_into(&mut left, -1.0);
        for slot in 0..18 {
            assert_eq!(
                i16::from(left[slot]) - 128,
                128 - i16::from(right[slot]),
                "slot {slot} mirrors"
            );
        }
    }
}
