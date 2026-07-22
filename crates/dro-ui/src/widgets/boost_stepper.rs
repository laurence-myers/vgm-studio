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
use dro_core::{nearest_volume_modifier, volume_modifier_factor, volume_step_down, volume_step_up};

/// Snaps a desired factor onto the modifier ladder, then caps it at `ceiling`
/// (the clipping-guard level) when one is set.
///
/// The cap is the level that first drove the limiter into clipping this song, so
/// a typed value above it is pulled back down to exactly that level -- you can
/// return to the trigger point but not push past it. Below the cap, the value
/// just snaps to its nearest modifier position.
fn snapped_within_ceiling(desired: f32, ceiling: Option<f32>) -> f32 {
    let factor = volume_modifier_factor(nearest_volume_modifier(desired));
    match ceiling {
        Some(cap) if factor > cap => volume_modifier_factor(nearest_volume_modifier(cap)),
        _ => factor,
    }
}

/// Draws the volume lever right-aligned in the transport row: up/down arrows, an
/// editable factor (`0.25x`..=`64.00x`), a "Volume" label, and a dividing
/// groove. Emits [`Action::SetBoost`] on any change.
///
/// `boost` is the current factor. `ceiling`, when set, is the level at which the
/// limiter began clipping this song; the up arrow and typed input are capped
/// there so the user cannot push further into the limiter. Lowering is always
/// allowed, and a new song clears the ceiling. `lock` drives the "Lock" toggle:
/// when set, the volume is kept across songs; when clear, each song sets its own
/// from its header modifier.
pub fn boost_stepper(
    ui: &mut egui::Ui,
    palette: &Palette,
    boost: f32,
    ceiling: Option<f32>,
    lock: bool,
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

        // The "Lock" toggle (rightmost): on keeps this volume across songs; off
        // lets each song set its own from its header modifier.
        let mut locked = lock;
        if theme::bevel::toggle(ui, palette, &mut locked, "Lock")
            .on_hover_text(if lock {
                "Volume is kept across songs. Click to let each song start from \
                 its header modifier instead."
            } else {
                "Each song starts from its header volume modifier. Click to keep \
                 this volume across songs."
            })
            .clicked()
        {
            actions.push(Action::SetLockBoost(locked));
        }

        // The lever sits on the modifier factor ladder: snap the stored factor to
        // its nearest ladder position, then step and display from there. The
        // arrows move ~1.0 at unity and above, ~0.1 below it (both snapped).
        let factor = volume_modifier_factor(nearest_volume_modifier(boost));
        let up_factor = volume_step_up(factor);
        let down_factor = volume_step_down(factor);
        // The ceiling blocks raising past the level that clipped (a hair of slack
        // for the float compare). A step that does not move (the ladder end)
        // disables its arrow. Lowering is always allowed.
        let can_raise = up_factor != factor && ceiling.is_none_or(|c| up_factor <= c * 1.001);
        let can_lower = down_factor != factor;

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
                    value: down_factor,
                    persist: true,
                });
            }
            // The up arrow stays interactive so its hover can explain the cap (the
            // bevel button does not grey when disabled), but a click at the
            // ceiling is ignored. It is blocked only *at* the trigger level: from
            // any lower value the arrow climbs back up to it.
            let up_hover = if can_raise || ceiling.is_none() {
                "Louder"
            } else {
                "At this song's clipping limit -- lower the volume to go quieter"
            };
            if theme::bevel::button_sized(ui, palette, "\u{25B2}", arrow)
                .on_hover_text(up_hover)
                .clicked()
                && can_raise
            {
                actions.push(Action::SetBoost {
                    value: up_factor,
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
            // Report whether this field holds keyboard focus, so the app yields
            // the keyboard to it (see `gather_key_input`) -- a number typed here
            // must change the volume, not toggle a channel. Routed as an action
            // rather than written to egui memory here, which would deadlock the
            // memory lock held during the draw.
            actions.push(Action::VolumeFieldFocused(response.has_focus()));
            // No continuous drag (speed 0), so a change is always a committed edit
            // -- persist it once, like an arrow click. A typed value is snapped to
            // the ladder and capped at the clipping ceiling.
            if response.changed() {
                actions.push(Action::SetBoost {
                    value: snapped_within_ceiling(value as f32, ceiling),
                    persist: true,
                });
            }
        });

        // The label sits left of the value...
        ui.label("Volume");
        // ...then the "Match" button (further left in this right-to-left row, so it
        // reads "Match Volume 1.00x"): it measures the song's peak and sets the
        // volume to bring it to full scale.
        if theme::bevel::button(ui, palette, "Match")
            .on_hover_text(
                "Measure the song's loudest peak and set the volume to bring it to \
                 full scale without clipping",
            )
            .clicked()
        {
            actions.push(Action::MatchVolume);
        }
        // ...and a 2px beveled groove at full row height separates the volume
        // section from the transport buttons, matching the grooves between the
        // stacked panels.
        theme::separator(ui, palette);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn typed_input_snaps_to_the_ladder_without_a_ceiling() {
        // With no clipping ceiling, any factor just snaps to its nearest modifier
        // value -- exactly what the ladder produces.
        assert!(approx(snapped_within_ceiling(2.0, None), 2.0));
        assert!(approx(snapped_within_ceiling(0.25, None), 0.25));
        // Out-of-range input saturates at the ladder ends, never beyond.
        assert!(approx(snapped_within_ceiling(1000.0, None), 64.0));
        assert!(approx(snapped_within_ceiling(0.0, None), 0.25));
    }

    #[test]
    fn typed_input_is_clamped_to_the_trigger_level() {
        // The limiter fired at 2.00x, so that is the cap.
        let ceiling = Some(2.0);
        // Anything above the cap is pulled back to exactly it...
        assert!(
            approx(snapped_within_ceiling(5.0, ceiling), 2.0),
            "5x clamps to 2x"
        );
        assert!(
            approx(snapped_within_ceiling(64.0, ceiling), 2.0),
            "64x clamps to 2x"
        );
        // ...the cap itself passes through...
        assert!(approx(snapped_within_ceiling(2.0, ceiling), 2.0));
        // ...and anything below is left alone (snapped), never raised to the cap.
        assert!(snapped_within_ceiling(1.5, ceiling) < 2.0);
        assert!(approx(snapped_within_ceiling(0.5, ceiling), 0.5));
    }
}
