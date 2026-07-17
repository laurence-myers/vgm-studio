//! A rotary pan knob: a circle with a position dot, for per-channel panning.
//!
//! Hand-painted in the DOS style of the peak meter and bevel buttons -- a
//! `button_face` disc with the near-black keyline, a reference tick at 12
//! o'clock, and a bright dot orbiting the centre across a 270-degree sweep (hard
//! left at 7:30, centre at 12:00, hard right at 4:30). It reports itself as a
//! slider to accessibility and egui_kittest, so the GUI tests can find and drag
//! it. Pans are bytes: `0x00` hard left, `0x80` centre, `0xFF` hard right.

use core::cmp::Ordering;

use egui::{Response, Sense, Stroke, Ui, vec2};

use crate::theme::Palette;

/// The knob's square side, in points -- no wider than a digit toggle, so the
/// channel grid's columns stay snug.
const SIZE: f32 = 18.0;
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

/// The new raw pan after dragging `dx` points from `raw`, clamped to `0..=255`.
fn drag_value(raw: f32, dx: f32) -> f32 {
    (raw + dx * DRAG_UNITS_PER_POINT).clamp(0.0, 255.0)
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
/// independently (128 steps left, 127 right), like [`readout`].
fn dot_angle(value: u8) -> f32 {
    let half = SWEEP / 2.0;
    match value.cmp(&CENTER) {
        Ordering::Equal => 0.0,
        Ordering::Less => -half * f32::from(CENTER - value) / f32::from(CENTER),
        Ordering::Greater => half * f32::from(value - CENTER) / f32::from(255 - CENTER),
    }
}

/// `num / den` as a rounded percentage.
fn percent(num: u32, den: u32) -> u32 {
    (num * 100 + den / 2) / den
}

/// The hover readout: `"C"` at centre, `"L1".."L100"` left, `"R1".."R100"` right.
fn readout(value: u8) -> String {
    match value.cmp(&CENTER) {
        Ordering::Equal => "C".to_owned(),
        Ordering::Less => format!("L{}", percent(u32::from(CENTER - value), u32::from(CENTER))),
        Ordering::Greater => format!(
            "R{}",
            percent(u32::from(value - CENTER), u32::from(255 - CENTER))
        ),
    }
}

/// Draws a pan knob for `value` (`0x00` left .. `0x80` centre .. `0xFF` right).
///
/// When `enabled`, dragging left/right repositions the pan (relative, ~64 points
/// for the full range, with a snap-to-centre detent); double-click or right-click
/// recentres. When disabled it is inert and dimmed, showing the pan the policy
/// implies. `label` names it for accessibility and the headless tests. Returns the
/// [`Response`]; `response.changed()` is true on the frames the pan moved.
pub fn show(
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
            let dx = response.drag_delta().x;
            let raw = ui.data_mut(|d| {
                let seed = d.get_temp::<f32>(response.id).unwrap_or(f32::from(*value));
                let raw = drag_value(seed, dx);
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
    response.on_hover_text(readout(*value))
}

/// Paints the disc, keyline, 12 o'clock tick, and position dot.
fn paint(ui: &Ui, rect: egui::Rect, palette: &Palette, value: u8, enabled: bool) {
    let painter = ui.painter();
    let center = rect.center();
    let radius = rect.width().min(rect.height()) / 2.0 - 1.0;

    let face = if enabled {
        palette.button_face
    } else {
        palette.face
    };
    painter.circle_filled(center, radius, face);
    painter.circle_stroke(center, radius, Stroke::new(1.0, palette.bevel_border));

    // The 12 o'clock reference tick on the rim.
    let tick = if enabled {
        palette.bevel_dark
    } else {
        palette.muted
    };
    painter.line_segment(
        [
            center + vec2(0.0, -radius),
            center + vec2(0.0, -radius + 3.0),
        ],
        Stroke::new(1.0, tick),
    );

    // The position dot, orbiting just inside the rim.
    let theta = dot_angle(value);
    let orbit = radius - 3.0;
    let dot = center + vec2(theta.sin() * orbit, -theta.cos() * orbit);
    let dot_color = if enabled {
        palette.data_text
    } else {
        palette.muted
    };
    painter.circle_filled(dot, 1.8, dot_color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_value_scales_and_clamps() {
        assert_eq!(drag_value(128.0, 0.0), 128.0);
        assert_eq!(drag_value(128.0, 10.0), 128.0 + 40.0); // 4 units per point
        assert_eq!(drag_value(0.0, -5.0), 0.0, "clamps at the left");
        assert_eq!(drag_value(255.0, 5.0), 255.0, "clamps at the right");
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
        assert_eq!(readout(0x80), "C");
        assert_eq!(readout(0x00), "L100");
        assert_eq!(readout(0xFF), "R100");
        assert_eq!(readout(0x40), "L50");
        assert_eq!(readout(0xC0), "R50");
    }
}
