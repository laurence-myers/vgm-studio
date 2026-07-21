//! The live-playback volume lever, extracted from the transport row to keep
//! `DroApp::update_impl` lean (uishell-6).
//!
//! A bidirectional volume control sitting on the VGM volume-modifier factor
//! ladder: every position is a real modifier value (see [`dro_core::volume`]),
//! shown as the playback factor it produces -- `0.25x` to `64.00x`, with `1.00x`
//! bit-transparent. A peak limiter behind it prevents clipping; once that
//! limiter engages, the ceiling stops the value rising further (the clipping
//! guard).

use crate::action::Action;
use crate::theme::{self, Palette};
use dro_core::{nearest_volume_modifier, nudge_volume_modifier, volume_modifier_factor};

/// One up/down click moves this many positions along the factor ladder. 32
/// positions is a doubling, so a single step is about `0.22` dB -- fine, but it
/// lands exactly on a modifier value and the 2-dp readout still moves each click.
const ARROW_STEP: i32 = 1;

/// Draws the volume lever right-aligned in the transport row: up/down arrows, an
/// editable factor (`0.25x`..=`64.00x`), a "Volume" label, and a dividing
/// groove. Emits [`Action::SetBoost`] on any change.
///
/// `boost` is the current factor. `ceiling`, when set, is the level at which the
/// limiter began clipping this song; the up arrow and typed input are capped
/// there so the user cannot push further into the limiter. Lowering is always
/// allowed, and a new song clears the ceiling.
pub fn boost_stepper(
    ui: &mut egui::Ui,
    palette: &Palette,
    boost: f32,
    ceiling: Option<f32>,
    actions: &mut Vec<Action>,
) {
    // Live playback volume, right-aligned in the row. A limiter behind it
    // prevents clipping; the WAV render and the waveform stay at the un-boosted
    // level. Built right-to-left: the up/down arrows, the editable value, the
    // "Volume" label, then a full-height groove dividing it from the transport
    // buttons.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let row_h = ui.spacing().interact_size.y;

        // The lever sits on the modifier factor ladder: snap the stored factor to
        // its nearest ladder position, then step and display from there.
        let byte = nearest_volume_modifier(boost);
        let factor = volume_modifier_factor(byte);
        let up = nudge_volume_modifier(byte, ARROW_STEP);
        let down = nudge_volume_modifier(byte, -ARROW_STEP);
        // The ceiling blocks raising past the level that clipped (a hair of slack
        // for the float compare, since the ladder steps are ~2% apart). Lowering
        // is always allowed.
        let can_raise =
            up != byte && ceiling.is_none_or(|c| volume_modifier_factor(up) <= c * 1.001);
        let can_lower = down != byte;

        // Up/down arrows, snug together (rightmost in the row). A nested
        // `ui.horizontal` inherits the enclosing right-to-left layout, so add down
        // first and up second and they come out up-on-the-left, down-on-the-right,
        // like a stepper. (Forcing left-to-right here corrupts the parent.)
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let arrow = egui::vec2(20.0, row_h);
            if theme::bevel::button_sized(ui, palette, "\u{25BC}", arrow)
                .on_hover_text("Quieter")
                .clicked()
                && can_lower
            {
                actions.push(Action::SetBoost {
                    value: volume_modifier_factor(down),
                    persist: true,
                });
            }
            // The up arrow stays interactive so its hover can explain the cap (the
            // bevel button does not grey when disabled), but a click past the
            // ceiling is ignored.
            let up_hover = if can_raise || ceiling.is_none() {
                "Louder"
            } else {
                "Limiter engaged -- lower the volume to stop clipping"
            };
            if theme::bevel::button_sized(ui, palette, "\u{25B2}", arrow)
                .on_hover_text(up_hover)
                .clicked()
                && can_raise
            {
                actions.push(Action::SetBoost {
                    value: volume_modifier_factor(up),
                    persist: true,
                });
            }
        });

        // The value: a dark well with the tracker-yellow digit, click to type.
        // Typed input is read as a factor and snapped to the nearest modifier
        // value, then capped at the ceiling.
        ui.scope(|ui| {
            let widgets = &mut ui.visuals_mut().widgets;
            for w in [
                &mut widgets.inactive,
                &mut widgets.hovered,
                &mut widgets.active,
            ] {
                w.weak_bg_fill = palette.data_bg;
                w.bg_fill = palette.data_bg;
                w.fg_stroke.color = palette.data_text;
            }
            let mut value = f64::from(factor);
            let db = 20.0 * factor.log10();
            let response = ui
                .add(
                    egui::DragValue::new(&mut value)
                        .speed(0.0)
                        .update_while_editing(false)
                        .custom_formatter(|n, _| {
                            // Show the snapped ladder factor to two decimals, e.g.
                            // "0.25x", "1.00x", "16.00x".
                            let snapped = volume_modifier_factor(nearest_volume_modifier(n as f32));
                            format!("{snapped:.2}\u{00d7}")
                        })
                        .custom_parser(|s| {
                            s.trim()
                                .trim_end_matches(['\u{00d7}', 'x', 'X', ' '])
                                .parse::<f64>()
                                .ok()
                        }),
                )
                .on_hover_text(format!("{factor:.2}\u{00d7} ({db:+.1} dB)"));
            // No continuous drag (speed 0), so a change is always a committed edit
            // -- persist it once, like an arrow click.
            if response.changed() {
                let mut snapped = volume_modifier_factor(nearest_volume_modifier(value as f32));
                if let Some(cap) = ceiling
                    && snapped > cap
                {
                    snapped = volume_modifier_factor(nearest_volume_modifier(cap));
                }
                actions.push(Action::SetBoost {
                    value: snapped,
                    persist: true,
                });
            }
        });

        // The label sits left of the value...
        ui.label("Volume");
        // ...and a 2px beveled groove at full row height separates the volume
        // section from the transport buttons, matching the grooves between the
        // stacked panels.
        theme::separator(ui, palette);
    });
}
