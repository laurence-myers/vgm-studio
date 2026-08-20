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
/// Half-width of the snap-to-centre band, in pan units: a few degrees of turn to
/// escape the detent.
const SNAP_BAND: u8 = 8;
/// The dot's angular sweep from hard left to hard right (270 degrees).
const SWEEP: f32 = 1.5 * std::f32::consts::PI;

/// Half-width of the snap-to-mono band, in spread units.
const SPREAD_SNAP: f32 = 0.06;

/// The full trim, and the value it rests at: 100%, the reference balance
/// untouched. Mirrors [`vgms_synth::ChipTrims`]'s full; the widget keeps its own
/// so it stays a plain `0..=100` control.
const TRIM_FULL: u8 = 100;
/// Half-width of the snap-to-full band, in trim percent: nudging the trim near
/// the top settles it at exactly 100%.
const TRIM_SNAP: u8 = 3;
/// The angle of the trim arc's 0% end: 7:30, the same corner hard-left pan
/// reaches. The lit arc fills from here to the value, so 100% is a full ring.
const TRIM_MIN_ANGLE: f32 = -SWEEP / 2.0;

/// Radius around the knob centre, in points, inside which the pointer's angle is
/// too unstable to read; motion there sweeps nothing.
const GESTURE_DEADZONE: f32 = 6.0;
/// How much Shift slows a gesture, for fine adjustment. Shift also lifts the
/// snap detents, so a fine turn can settle anywhere.
const FINE_FACTOR: f32 = 5.0;
/// Fraction of the full range one point of scroll moves: a ~50-point wheel notch
/// steps 10% of the range.
const SCROLL_FRACTION_PER_POINT: f32 = 0.1 / 50.0;

/// The angle the pointer swept around `center` moving from `prev` to `cur`, in
/// radians, clockwise positive (egui's y grows downward). The shortest arc, so
/// any per-frame jump under a half-turn reads correctly; motion inside
/// [`GESTURE_DEADZONE`] sweeps nothing.
fn swept_angle(center: Pos2, prev: Pos2, cur: Pos2) -> f32 {
    let a = prev - center;
    let b = cur - center;
    if a.length() < GESTURE_DEADZONE || b.length() < GESTURE_DEADZONE {
        return 0.0;
    }
    (a.x * b.y - a.y * b.x).atan2(a.x * b.x + a.y * b.y)
}

/// Applies this frame's knob gestures to the continuous raw value the widget
/// keeps in per-widget memory, returning the new raw while an input moved it.
///
/// - **Drag**: the pointer's swept angle around the knob turns it -- clockwise
///   raises, anticlockwise lowers -- scaled so one full 270-degree sweep spans
///   `min..=max`, at any drag radius.
/// - **Wheel**: hovering and scrolling steps the value (up raises).
/// - **Shift** slows either gesture by [`FINE_FACTOR`].
///
/// The raw accumulates un-snapped, so a caller's detent can hold the *output*
/// without the gesture sticking there.
fn gesture(
    ui: &mut Ui,
    response: &Response,
    center: Pos2,
    current: f32,
    min: f32,
    max: f32,
) -> Option<f32> {
    let range = max - min;
    if response.drag_started() {
        ui.data_mut(|d| d.insert_temp(response.id, current));
    }
    let fine = ui.input(|i| i.modifiers.shift);
    let mut moved = None;
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let prev = pos - response.drag_delta();
        let mut swept = swept_angle(center, prev, pos);
        if fine {
            swept /= FINE_FACTOR;
        }
        let raw = ui.data_mut(|d| {
            let seed = d.get_temp::<f32>(response.id).unwrap_or(current);
            let raw = (seed + swept / SWEEP * range).clamp(min, max);
            d.insert_temp(response.id, raw);
            raw
        });
        moved = Some(raw);
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<f32>(response.id));
    }
    if moved.is_none() && response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let mut step = scroll * SCROLL_FRACTION_PER_POINT * range;
            if fine {
                step /= FINE_FACTOR;
            }
            moved = Some((current + step).clamp(min, max));
        }
    }
    moved
}

/// Whether Shift is held: fine-adjust mode, which also lifts the snap detents so
/// a precise value near one can actually be set.
fn fine_mode(ui: &Ui) -> bool {
    ui.input(|i| i.modifiers.shift)
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

/// Snaps a trim within [`TRIM_SNAP`] of full to exactly 100%.
fn snap_to_full(value: u8) -> u8 {
    if value >= TRIM_FULL - TRIM_SNAP {
        TRIM_FULL
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
/// When `enabled`, dragging in a circle around the knob turns it -- clockwise
/// pans right, anticlockwise pans left, one full 270-degree sweep for the full
/// range -- and the wheel steps it while hovered. Shift makes either gesture
/// fine; without Shift a snap-to-centre detent holds. Double-click or
/// right-click recentres. When disabled it is inert and dimmed, showing the pan
/// the policy implies. `label` names it for accessibility and the headless
/// tests. Returns the [`Response`]; `response.changed()` is true on the frames
/// the pan moved.
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
        // The gesture tracks a continuous raw value in per-widget memory so the
        // centre detent can hold the *output* at centre without the turn sticking
        // there: the raw keeps moving and the output escapes once it leaves the
        // band. Shift lifts the detent, so a fine turn can settle just off centre.
        if let Some(raw) = gesture(ui, &response, rect.center(), f32::from(*value), 0.0, 255.0) {
            let stepped = raw.round() as u8;
            let snapped = if fine_mode(ui) {
                stepped
            } else {
                snap_to_center(stepped)
            };
            if snapped != *value {
                *value = snapped;
                response.mark_changed();
            }
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
/// `+1.0`). Dragging in a circle turns it -- clockwise widens one way,
/// anticlockwise the other -- and the wheel steps it while hovered; Shift makes
/// either gesture fine and lifts the snap-to-mono detent. Double-click or
/// right-click returns to mono. `label` names it for accessibility and the
/// headless tests. Always live -- a turn engages Custom panning in the caller.
/// Returns the [`Response`]; `response.changed()` is true on the frames the
/// spread moved.
pub(crate) fn show_spread(
    ui: &mut Ui,
    palette: &Palette,
    spread: &mut f32,
    label: &str,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());

    // A continuous raw value in per-widget memory, so the snap-to-mono detent can
    // hold the output at 0 without the turn sticking there (as the pan knob does).
    if let Some(raw) = gesture(ui, &response, rect.center(), *spread, -1.0, 1.0) {
        let snapped = if !fine_mode(ui) && raw.abs() <= SPREAD_SNAP {
            0.0
        } else {
            raw
        };
        if snapped != *spread {
            *spread = snapped;
            response.mark_changed();
        }
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
/// visibly shortens it. Dragging in a circle turns it -- clockwise raises,
/// anticlockwise lowers -- and the wheel steps it while hovered; Shift makes
/// either gesture fine and lifts the snap-to-full detent. Double-click or
/// right-click resets to 100%. `label` names it for accessibility and the
/// headless tests. Returns the [`Response`]; `response.changed()` is true on
/// the frames the trim moved.
pub(crate) fn show_trim(ui: &mut Ui, palette: &Palette, value: &mut u8, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(SIZE, SIZE), Sense::click_and_drag());

    // A continuous raw value in per-widget memory, so the turn accumulates
    // smoothly across frames as the pan knob's does. The detent holds the output
    // at the 100% reference; Shift lifts it for a trim just under full.
    if let Some(raw) = gesture(
        ui,
        &response,
        rect.center(),
        f32::from(*value),
        0.0,
        f32::from(TRIM_FULL),
    ) {
        let stepped = raw.round() as u8;
        let snapped = if fine_mode(ui) {
            stepped
        } else {
            snap_to_full(stepped)
        };
        if snapped != *value {
            *value = snapped;
            response.mark_changed();
        }
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
    fn swept_angle_reads_the_turn_about_the_centre() {
        let c = Pos2::new(0.0, 0.0);
        // A quarter-turn from 12 o'clock to 3 o'clock is clockwise on screen
        // (y grows downward): +90 degrees.
        let quarter = swept_angle(c, Pos2::new(0.0, -30.0), Pos2::new(30.0, 0.0));
        assert!(
            (quarter - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "clockwise quarter-turn, got {quarter}"
        );
        // The same arc the other way is anticlockwise: -90 degrees.
        let back = swept_angle(c, Pos2::new(30.0, 0.0), Pos2::new(0.0, -30.0));
        assert!((back + std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        // The shortest arc, so a jump across the downward axis never reads as
        // a near-full turn the long way round.
        let wrap = swept_angle(c, Pos2::new(-10.0, 30.0), Pos2::new(10.0, 30.0));
        assert!(
            wrap < 0.0,
            "crossing 6 o'clock rightward turns anticlockwise"
        );
        assert!(wrap.abs() < 1.0, "and by the short arc");
        // The turn reads the same at any radius.
        let wide = swept_angle(c, Pos2::new(0.0, -300.0), Pos2::new(300.0, 0.0));
        assert!((wide - quarter).abs() < 1e-4, "radius-independent");
    }

    #[test]
    fn swept_angle_ignores_the_deadzone() {
        let c = Pos2::new(0.0, 0.0);
        assert_eq!(
            swept_angle(c, Pos2::new(0.0, -2.0), Pos2::new(2.0, 0.0)),
            0.0,
            "both points inside the deadzone"
        );
        assert_eq!(
            swept_angle(c, c, Pos2::new(30.0, 0.0)),
            0.0,
            "a move out from the exact centre sweeps nothing"
        );
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
    fn trim_snaps_to_full_near_the_top() {
        assert_eq!(snap_to_full(100), 100);
        assert_eq!(snap_to_full(TRIM_FULL - TRIM_SNAP), 100, "inclusive edge");
        assert_eq!(
            snap_to_full(TRIM_FULL - TRIM_SNAP - 1),
            TRIM_FULL - TRIM_SNAP - 1
        );
        assert_eq!(snap_to_full(0), 0);
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
