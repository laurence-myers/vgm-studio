//! Rotary knobs: a domed keycap whose value reads as a 270-degree amber arc. A
//! per-channel pan knob, a bipolar stereo-spread knob that drives all the pans at
//! once, and a per-chip trim knob for the chip mixer.
//!
//! Painted in the pad chrome's language, so the knob follows the selected pad
//! surface (theme default, or a Light/Dark/Grey/Tint override) exactly as the
//! channel digits under it do: a domed cap in the [`pad_caps`] colours, sunk into
//! a dark channel, lit from the upper-left. The value is a full-radius arc in the
//! hardware latch amber, filling from an anchor toward the position (hard left at
//! 7:30, hard right at 4:30). A slim line on the cap marks the exact position,
//! lining up with the arc's tip. There is no centre hub. They report themselves as
//! sliders to accessibility and egui_kittest, so the GUI tests can find and drag
//! them. Pans are bytes: `0x00` hard left, `0x80` centre, `0xFF` hard right, and
//! fill from 12 o'clock so the knob rests unlit at centre. Spread is `-1.0..=1.0`:
//! `0.0` mono, the extremes a wide image (its sign mirrors the sides). The trim is
//! a `0..=100`% level filling from the 0% end (7:30) upward, so it rests *fully
//! lit* at its 100% default -- the opposite resting state, on purpose.

use core::cmp::Ordering;

use egui::epaint::Mesh;
use egui::{Color32, Pos2, Response, Sense, Shape, Stroke, Ui, vec2};

use crate::theme::paint::{darken, lerp_color, lighten};
use crate::theme::{Palette, deck_stops, pad_caps};

/// The knob's square side, in points. Shared with the channel grid so each digit
/// toggle sits in a cell of exactly this width, centred under its knob.
pub(crate) const SIZE: f32 = 20.0;
/// The centred pan value.
const CENTER: u8 = 0x80;
/// Half-width of the snap-to-centre band, in pan units. At [`DRAG_UNITS_PER_POINT`]
/// per point that is ~2 points of travel to escape the detent.
const SNAP_BAND: u8 = 8;
/// Pan units moved per point of horizontal drag: ~64 points spans the full range,
/// regardless of the knob's on-screen size.
const DRAG_UNITS_PER_POINT: f32 = 4.0;
/// The dot's angular sweep from hard left to hard right (270 degrees).
const SWEEP: f32 = 1.5 * std::f32::consts::PI;

/// Spread units moved per point of drag: ~128 points spans the full `-1..=1`.
const SPREAD_UNITS_PER_POINT: f32 = 1.0 / 64.0;
/// Half-width of the snap-to-mono band, in spread units.
const SPREAD_SNAP: f32 = 0.06;

/// The full trim, and the value it rests at: 100%, the reference balance
/// untouched. Mirrors [`vgms_synth::ChipTrims`]'s full; the widget keeps its own
/// so it stays a plain `0..=100` control.
const TRIM_FULL: u8 = 100;
/// Trim percent moved per point of drag: ~64 points spans the full `0..=100`%,
/// matching the pan knob's feel.
const TRIM_UNITS_PER_POINT: f32 = 100.0 / 64.0;
/// The angle of the trim arc's 0% end: 7:30, the same corner hard-left pan
/// reaches. The lit arc fills from here to the value, so 100% is a full ring.
const TRIM_MIN_ANGLE: f32 = -SWEEP / 2.0;

/// The new raw pan after a drag of `dx` (rightward) and `dy` (downward) points
/// from `raw`, clamped to `0..=255`. Right and down both pan right; left and up
/// both pan left -- the two axes add, so the knob answers whichever way you drag.
fn drag_value(raw: f32, dx: f32, dy: f32) -> f32 {
    (raw + (dx + dy) * DRAG_UNITS_PER_POINT).clamp(0.0, 255.0)
}

/// The new raw trim after a drag of `dx` (rightward) and `dy` (downward) points
/// from `raw`, clamped to `0..=100`. The axes add exactly as the pan knob's do,
/// so the trim answers a drag identically to the pan knobs beside it: right or
/// down raises the level, left or up lowers it.
fn trim_drag_value(raw: f32, dx: f32, dy: f32) -> f32 {
    (raw + (dx + dy) * TRIM_UNITS_PER_POINT).clamp(0.0, f32::from(TRIM_FULL))
}

/// The trim marker's angle in radians, clockwise from 12 o'clock: `0` -> -135 deg
/// (7:30), `50` -> 0 (12:00), `100` -> +135 deg (4:30). Unlike the pan knob it
/// rests at the +135-degree extreme (100%), not at centre.
fn trim_angle(percent: u8) -> f32 {
    SWEEP * (f32::from(percent.min(TRIM_FULL)) / f32::from(TRIM_FULL) - 0.5)
}

/// Snaps a pan within [`SNAP_BAND`] of centre to exactly centre.
fn snap_to_center(value: u8) -> u8 {
    if value.abs_diff(CENTER) <= SNAP_BAND {
        CENTER
    } else {
        value
    }
}

/// The dot's angle in radians, clockwise from straight up (12 o'clock):
/// `0x00` -> -135 deg (7:30), `0x80` -> 0 (12:00), `0xFF` -> +135 deg (4:30).
///
/// Anchored on `0x80` (the semantic centre that recentre and the snap detent
/// target), so the dot points exactly up there; the two sides scale
/// independently (128 steps left, 127 right), like [`crate::strings::pan_knob_readout`].
fn dot_angle(value: u8) -> f32 {
    let half = SWEEP / 2.0;
    match value.cmp(&CENTER) {
        Ordering::Equal => 0.0,
        Ordering::Less => -half * f32::from(CENTER - value) / f32::from(CENTER),
        Ordering::Greater => half * f32::from(value - CENTER) / f32::from(255 - CENTER),
    }
}

/// Draws a pan knob for `value` (`0x00` left .. `0x80` centre .. `0xFF` right).
///
/// When `enabled`, dragging repositions the pan (relative, ~64 points for the
/// full range, with a snap-to-centre detent): left or up pans left, right or down
/// pans right. Double-click or right-click recentres. When disabled it is inert and dimmed, showing the pan the policy
/// implies. `label` names it for accessibility and the headless tests. Returns the
/// [`Response`]; `response.changed()` is true on the frames the pan moved.
pub(crate) fn show(
    ui: &mut Ui,
    palette: &Palette,
    value: &mut u8,
    enabled: bool,
    label: &str,
) -> Response {
    let sense = if enabled {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), sense);

    if enabled {
        // The drag tracks a continuous raw value in per-widget memory so the
        // centre detent can hold the *output* at centre without the drag sticking
        // there: the raw keeps moving and the output escapes once it leaves the
        // band.
        if response.drag_started() {
            ui.data_mut(|d| d.insert_temp(response.id, f32::from(*value)));
        }
        if response.dragged() {
            let delta = response.drag_delta();
            let raw = ui.data_mut(|d| {
                let seed = d.get_temp::<f32>(response.id).unwrap_or(f32::from(*value));
                let raw = drag_value(seed, delta.x, delta.y);
                d.insert_temp(response.id, raw);
                raw
            });
            let snapped = snap_to_center(raw.round() as u8);
            if snapped != *value {
                *value = snapped;
                response.mark_changed();
            }
        }
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<f32>(response.id));
        }
        if (response.double_clicked() || response.secondary_clicked()) && *value != CENTER {
            *value = CENTER;
            response.mark_changed();
        }
    }

    response.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*value), label));

    if ui.is_rect_visible(rect) {
        paint(ui, rect, palette, *value, enabled);
    }
    response.on_hover_text(crate::strings::pan_knob_readout(*value))
}

/// Draws the bipolar stereo-spread knob for `spread` (`-1.0` .. `0.0` mono ..
/// `+1.0`). Dragging right or up widens one way, left or down the other (the
/// axes add, like the pan knob); double-click or right-click returns to mono.
/// `label` names it for accessibility and the headless tests. Always live -- a
/// drag engages Custom panning in the caller. Returns the [`Response`];
/// `response.changed()` is true on the frames the spread moved.
pub(crate) fn show_spread(
    ui: &mut Ui,
    palette: &Palette,
    spread: &mut f32,
    label: &str,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());

    // A continuous raw value in per-widget memory, so the snap-to-mono detent can
    // hold the output at 0 without the drag sticking there (as the pan knob does).
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(response.id, *spread));
    }
    if response.dragged() {
        let delta = response.drag_delta();
        let raw = ui.data_mut(|d| {
            let seed = d.get_temp::<f32>(response.id).unwrap_or(*spread);
            let raw = (seed + (delta.x - delta.y) * SPREAD_UNITS_PER_POINT).clamp(-1.0, 1.0);
            d.insert_temp(response.id, raw);
            raw
        });
        let snapped = if raw.abs() <= SPREAD_SNAP { 0.0 } else { raw };
        if snapped != *spread {
            *spread = snapped;
            response.mark_changed();
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(response.id));
    }
    if (response.double_clicked() || response.secondary_clicked()) && *spread != 0.0 {
        *spread = 0.0;
        response.mark_changed();
    }

    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(*spread), label));

    if ui.is_rect_visible(rect) {
        // Reuse the pan dial: 0 points straight up, the extremes reach the same
        // 135-degree corners as hard left / hard right, filling from 12 o'clock.
        paint_dial(ui, rect, palette, *spread * (SWEEP / 2.0), 0.0, true);
    }
    response.on_hover_text(crate::strings::pan_knob_spread_readout(*spread))
}

/// Draws the per-chip trim knob for `value` (`0` silent .. `100` the reference
/// balance). Always live; the lit arc fills from the 0% end (7:30) up to the
/// value, so the whole ring is lit at the 100% default and pulling a chip down
/// visibly shortens it. Dragging raises or lowers the level, relative and in the
/// pan knob's units; double-click or right-click resets to 100%. `label` names it
/// for accessibility and the headless tests. Returns the [`Response`];
/// `response.changed()` is true on the frames the trim moved.
pub(crate) fn show_trim(ui: &mut Ui, palette: &Palette, value: &mut u8, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());

    // A continuous raw value in per-widget memory, so relative drag accumulates
    // smoothly across frames as the pan knob's does. No detent: the reset gesture
    // covers returning to the 100% extreme, so the drag writes the value straight.
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(response.id, f32::from(*value)));
    }
    if response.dragged() {
        let delta = response.drag_delta();
        let raw = ui.data_mut(|d| {
            let seed = d.get_temp::<f32>(response.id).unwrap_or(f32::from(*value));
            let raw = trim_drag_value(seed, delta.x, delta.y);
            d.insert_temp(response.id, raw);
            raw
        });
        let stepped = raw.round() as u8;
        if stepped != *value {
            *value = stepped;
            response.mark_changed();
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(response.id));
    }
    if (response.double_clicked() || response.secondary_clicked()) && *value != TRIM_FULL {
        *value = TRIM_FULL;
        response.mark_changed();
    }

    response.widget_info(|| egui::WidgetInfo::slider(true, f64::from(*value), label));

    if ui.is_rect_visible(rect) {
        // Fill from the 0% end, not 12 o'clock: a full ring at 100%, empty at 0%.
        paint_dial(ui, rect, palette, trim_angle(*value), TRIM_MIN_ANGLE, true);
    }
    response.on_hover_text(crate::strings::pan_knob_trim_readout(*value))
}

/// A circular arc as a stroked polyline (the value arc, the cap glint and rim).
/// Angles are in degrees, clockwise from 3 o'clock in egui's y-down space.
fn arc(center: Pos2, radius: f32, from_deg: f32, to_deg: f32, stroke: Stroke) -> Shape {
    const SEGMENTS: usize = 20;
    let points = (0..=SEGMENTS)
        .map(|i| {
            let t = (from_deg + (to_deg - from_deg) * (i as f32 / SEGMENTS as f32)).to_radians();
            center + vec2(t.cos() * radius, t.sin() * radius)
        })
        .collect();
    Shape::line(points, stroke)
}

/// A radially-shaded filled disc: `inner` at the centre fading to `outer` at the
/// rim, as a triangle fan. Fakes the domed keycap's sheen (epaint has no radial
/// gradient primitive), the way the pad caps read as slightly domed.
fn disc(center: Pos2, radius: f32, inner: Color32, outer: Color32) -> Shape {
    const SEGMENTS: usize = 32;
    let mut mesh = Mesh::default();
    let hub = mesh.vertices.len() as u32;
    mesh.colored_vertex(center, inner);
    for i in 0..=SEGMENTS {
        let t = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        mesh.colored_vertex(center + vec2(t.cos() * radius, t.sin() * radius), outer);
    }
    for i in 0..SEGMENTS as u32 {
        mesh.add_triangle(hub, hub + 1 + i, hub + 2 + i);
    }
    Shape::mesh(mesh)
}

/// Paints the domed cap and its value arc for a pan `value`.
fn paint(ui: &Ui, rect: egui::Rect, palette: &Palette, value: u8, enabled: bool) {
    // Pan fills from 12 o'clock (`from_theta == 0`), so it rests unlit at centre.
    paint_dial(ui, rect, palette, dot_angle(value), 0.0, enabled);
}

/// Paints the knob with its value arc filling from `from_theta` to `theta`, both
/// radians clockwise from straight up. When `theta == from_theta` the arc is
/// unlit. The pan and spread knobs anchor at `0.0` (12 o'clock, unlit at rest);
/// the trim anchors at [`TRIM_MIN_ANGLE`] (7:30), so it rests fully lit at 100%.
fn paint_dial(
    ui: &Ui,
    rect: egui::Rect,
    palette: &Palette,
    theta: f32,
    from_theta: f32,
    enabled: bool,
) {
    let painter = ui.painter();
    let c = rect.center();
    // Scale every measure off the drawn size, so the recipe holds at any DPI.
    let s = rect.width().min(rect.height()) / SIZE;
    let r = rect.width().min(rect.height()) / 2.0 - s;
    let r_track = r - 1.7 * s;
    let track_w = 3.0 * s;
    let r_cap = r - 3.6 * s;

    // Cap colours follow the selected pad surface, as the digit pads do. Disabled
    // sinks the cap toward the deck it sits on and shows the position in a muted
    // ink rather than lighting the amber.
    let caps = pad_caps(palette);
    let (cap_top, cap_bottom, border, arc_ink, pointer_ink) = if enabled {
        (
            caps.top,
            caps.bottom,
            caps.border,
            palette.latch_bottom,
            caps.ink,
        )
    } else {
        let (deck_top, deck_bottom) = deck_stops(palette);
        let deck_mid = lerp_color(deck_top, deck_bottom, 0.5);
        let cap_mid = lerp_color(caps.top, caps.bottom, 0.5);
        let dim = lerp_color(cap_mid, deck_mid, 0.45);
        (
            lighten(dim, 0.06),
            darken(dim, 0.06),
            lerp_color(caps.border, deck_mid, 0.45),
            palette.muted,
            lerp_color(caps.ink, dim, 0.5),
        )
    };

    // The dark channel the cap sinks into: a translucent recess over the deck,
    // framed by the cap keyline.
    painter.circle_filled(c, r, Color32::from_black_alpha(0x60));
    painter.circle_stroke(c, r, Stroke::new(s, border));

    // The value: a full-radius arc from the anchor to the position, round-capped.
    // Nothing paints when the position is at the anchor, so the pan/spread knobs
    // rest unlit at centre and the trim rests empty at 0%.
    let from = from_theta.to_degrees() - 90.0;
    if (theta - from_theta).abs() > 0.01 {
        let to = theta.to_degrees() - 90.0;
        painter.add(arc(c, r_track, from, to, Stroke::new(track_w, arc_ink)));
        let cap_r = track_w / 2.0;
        let start = c + vec2(
            from.to_radians().cos() * r_track,
            from.to_radians().sin() * r_track,
        );
        painter.circle_filled(start, cap_r, arc_ink);
        let end = c + vec2(
            to.to_radians().cos() * r_track,
            to.to_radians().sin() * r_track,
        );
        painter.circle_filled(end, cap_r, arc_ink);
    }

    // The domed cap: a radial disc lit from the upper-left (glint) and shaded on
    // the lower-right (rim), in the pad language.
    painter.add(disc(
        c,
        r_cap,
        lighten(cap_top, 0.30),
        darken(cap_bottom, 0.18),
    ));
    painter.circle_stroke(c, r_cap, Stroke::new(0.8 * s, darken(cap_bottom, 0.40)));
    painter.add(arc(
        c,
        r_cap - 1.1 * s,
        -155.0,
        -45.0,
        Stroke::new(s, Color32::from_white_alpha(115)),
    ));
    painter.add(arc(
        c,
        r_cap,
        -10.0,
        100.0,
        Stroke::new(s, Color32::from_black_alpha(64)),
    ));

    // A slim position marker on the cap, pointing at the value so it lines up with
    // the arc's tip (straight up, marking centre, when the arc is unlit). Drawn
    // last, on top of the dome.
    let dir = vec2(theta.sin(), -theta.cos());
    let tip = c + dir * (r_cap - 0.6 * s);
    painter.line_segment(
        [c + dir * (1.2 * s), tip],
        Stroke::new(1.6 * s, pointer_ink),
    );
    painter.circle_filled(tip, 0.9 * s, pointer_ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_value_scales_and_clamps() {
        assert_eq!(drag_value(128.0, 0.0, 0.0), 128.0);
        assert_eq!(drag_value(128.0, 10.0, 0.0), 128.0 + 40.0); // right: 4 units/point
        assert_eq!(
            drag_value(128.0, 0.0, 10.0),
            128.0 + 40.0,
            "down pans right"
        );
        assert_eq!(drag_value(128.0, -5.0, 0.0), 128.0 - 20.0, "left pans left");
        assert_eq!(drag_value(128.0, 0.0, -5.0), 128.0 - 20.0, "up pans left");
        // The axes add, so a diagonal that cancels leaves the pan put.
        assert_eq!(drag_value(128.0, 8.0, -8.0), 128.0, "opposed axes cancel");
        assert_eq!(drag_value(0.0, -5.0, 0.0), 0.0, "clamps at the left");
        assert_eq!(drag_value(255.0, 5.0, 5.0), 255.0, "clamps at the right");
    }

    #[test]
    fn snap_to_center_holds_the_detent() {
        assert_eq!(snap_to_center(CENTER), CENTER);
        assert_eq!(snap_to_center(CENTER + SNAP_BAND), CENTER, "inclusive edge");
        assert_eq!(snap_to_center(CENTER - SNAP_BAND), CENTER);
        assert_eq!(
            snap_to_center(CENTER + SNAP_BAND + 1),
            CENTER + SNAP_BAND + 1
        );
        assert_eq!(snap_to_center(0), 0);
        assert_eq!(snap_to_center(255), 255);
    }

    #[test]
    fn dot_angle_sweeps_270_degrees_about_the_top() {
        assert!(dot_angle(0x80).abs() < 1e-6, "centre points straight up");
        let left = dot_angle(0x00);
        let right = dot_angle(0xFF);
        assert!(left < 0.0 && right > 0.0, "left is CCW of top, right is CW");
        assert!(
            (right - left - SWEEP).abs() < 1e-3,
            "endpoints span the sweep"
        );
        assert!(
            (left + SWEEP / 2.0).abs() < 1e-2,
            "hard left is half the sweep CCW"
        );
    }

    #[test]
    fn readout_labels_the_pan_position() {
        use crate::strings::pan_knob_readout;
        assert_eq!(pan_knob_readout(0x80), "C");
        assert_eq!(pan_knob_readout(0x00), "L100");
        assert_eq!(pan_knob_readout(0xFF), "R100");
        assert_eq!(pan_knob_readout(0x40), "L50");
        assert_eq!(pan_knob_readout(0xC0), "R50");
    }

    #[test]
    fn spread_readout_labels_the_width() {
        use crate::strings::pan_knob_spread_readout;
        assert_eq!(pan_knob_spread_readout(0.0), "Mono");
        assert_eq!(pan_knob_spread_readout(1.0), "+100%");
        assert_eq!(pan_knob_spread_readout(-1.0), "-100%");
        assert_eq!(pan_knob_spread_readout(0.5), "+50%");
        assert_eq!(pan_knob_spread_readout(-0.25), "-25%");
    }

    #[test]
    fn trim_drag_scales_and_clamps() {
        // Same axes and ~64pt-spans-the-range feel as the pan knob.
        assert_eq!(trim_drag_value(50.0, 0.0, 0.0), 50.0);
        assert!(
            (trim_drag_value(50.0, 6.4, 0.0) - 60.0).abs() < 1e-4,
            "right raises"
        );
        assert!(
            (trim_drag_value(50.0, 0.0, 6.4) - 60.0).abs() < 1e-4,
            "down raises"
        );
        assert!(
            (trim_drag_value(50.0, -6.4, 0.0) - 40.0).abs() < 1e-4,
            "left lowers"
        );
        assert_eq!(trim_drag_value(0.0, -5.0, 0.0), 0.0, "clamps at 0%");
        assert_eq!(trim_drag_value(100.0, 5.0, 5.0), 100.0, "clamps at 100%");
    }

    #[test]
    fn trim_angle_rests_at_the_full_extreme() {
        // 0% at 7:30 (-135deg), 50% straight up, 100% at 4:30 (+135deg) -- the
        // opposite resting state from the pan knob's unlit centre.
        assert!(
            (trim_angle(0) + SWEEP / 2.0).abs() < 1e-6,
            "0% is half the sweep CCW"
        );
        assert!(trim_angle(50).abs() < 1e-2, "50% points straight up");
        assert!(
            (trim_angle(100) - SWEEP / 2.0).abs() < 1e-6,
            "100% is half the sweep CW"
        );
    }

    #[test]
    fn trim_readout_is_a_plain_percentage() {
        use crate::strings::pan_knob_trim_readout;
        assert_eq!(pan_knob_trim_readout(0), "0%");
        assert_eq!(pan_knob_trim_readout(71), "71%");
        assert_eq!(pan_knob_trim_readout(100), "100%");
    }
}
