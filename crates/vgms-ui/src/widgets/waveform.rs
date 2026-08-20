//! The waveform panel.
//!
//! A dark well, a bright wave, a white playback-start line, a bright playback
//! cursor, a pale hover line that snaps to instruction offsets, and a half-black
//! dim over everything left of the start line -- the exact colours come from the
//! active [`Palette`], so each theme tints it.
//!
//! - The buckets are true min/max, so the wave is drawn symmetrically around a
//!   centre line rather than as bars growing from the bottom.
//! - Hovering also shows the snapped time as a tooltip.
//! - A time scale runs along the bottom edge: a tick and an MM:SS label at
//!   each multiple of a round step, the step chosen so labels never crowd.
//!
//! Everything here is denominated in the document's own summed delays -- via
//! [`TimeSource`], the OPL song's or the whole VGM's -- never the header's
//! `ms_length`, so a file with a lying header still maps clicks to the right
//! commands, and a Mega Drive rip is as clickable as a DRO.

use egui::epaint::Mesh;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2};
use vgms_synth::WaveformBucket;

use crate::editor::TimeSource;
use crate::theme::Palette;
use crate::theme::bevel::{self, Bevel};
use crate::theme::paint::{gradient_quad, lerp_color};

/// The fixed bucket count. The painted panel stretches these to its width.
pub(crate) const NUM_BUCKETS: usize = 768;

/// Headroom above the tallest bucket.
const HEADROOM: f32 = 5.0;

/// The waveform's displayed state, owned by the app.
#[derive(Debug, Default)]
pub(crate) struct WaveformState {
    pub(crate) buckets: Vec<WaveformBucket>,
    /// The white playback-start indicator, from the selected row.
    pub(crate) start_ms: u32,
    /// The yellow cursor. Only playback moves it; it survives edits, and is
    /// reset explicitly on file load.
    pub(crate) cursor_ms: u32,
    /// The loop brackets, when there is a region worth showing.
    pub(crate) loop_overlay: Option<LoopOverlay>,
}

/// The loop region as the panel needs it: in milliseconds, plus the two facts
/// that change how it is drawn.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoopOverlay {
    pub(crate) start_ms: u32,
    pub(crate) end_ms: u32,
    /// Loop playback is on, so the region is washed as well as bracketed --
    /// the difference between "marked" and "actually repeating".
    pub(crate) active: bool,
    /// The markers differ from the loop the song stores, so the flags are drawn
    /// hollow: the cue that there is something to apply.
    pub(crate) unapplied: bool,
}

/// What the panel reported this frame.
#[derive(Debug, Default)]
pub(crate) struct WaveformResponse {
    /// A click, already snapped to an instruction and its time.
    pub(crate) clicked: Option<(usize, u32)>,
    /// Whether `clicked` was the secondary (right) button.
    pub(crate) secondary: bool,
    /// The modifiers held for `clicked`, so the caller can tell a plain seek from
    /// a loop-marking gesture. The panel itself stays ignorant of what they mean.
    pub(crate) modifiers: egui::Modifiers,
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &WaveformState,
    timeline: Option<TimeSource<'_>>,
    palette: &Palette,
) -> WaveformResponse {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let rect = response.rect;
    paint_background(&painter, rect, palette);

    let mut out = WaveformResponse::default();
    let Some(timeline) = timeline else {
        // Still frame the empty well, so the panel reads as a sunken area.
        bevel::paint_bevel(&painter, rect, palette, Bevel::Sunken);
        return out;
    };
    let total_ms = timeline.total_ms();

    draw_buckets(&painter, rect, &state.buckets, palette);
    draw_time_markers(ui, &painter, rect, total_ms, palette);

    // Pen width scales with the panel: `width // 768 + 1`.
    let pen = (rect.width() / NUM_BUCKETS as f32 + 1.0).floor();

    // Hover: snap to the instruction under the pointer, preview line + time.
    let mut hover_x = None;
    if let Some(pointer) = response.hover_pos() {
        let pct = f64::from((pointer.x - rect.left()) / rect.width());
        if let Some((_, ms)) = timeline.index_and_ms_offset_at_pct(pct) {
            hover_x = Some(x_for_ms(rect, ms, total_ms));
            response
                .clone()
                .on_hover_text(crate::strings::waveform_hover(ms));
        }
    }
    if let Some(x) = hover_x {
        vertical_line(&painter, rect, x, pen, palette.wf_hover);
    }

    let start_x = x_for_ms(rect, state.start_ms, total_ms);
    vertical_line(&painter, rect, start_x, pen, palette.wf_start);
    vertical_line(
        &painter,
        rect,
        x_for_ms(rect, state.cursor_ms, total_ms),
        pen,
        palette.wf_cursor,
    );

    // Dim everything left of the start indicator, over the lines too.
    if start_x > rect.left() {
        let dimmed = Rect::from_min_max(rect.min, pos2(start_x, rect.bottom()));
        painter.rect_filled(dimmed, 0.0, palette.wf_dim);
    }

    // The loop brackets sit above the dim -- a marked region outside the played
    // span still has to be visible.
    if let Some(overlay) = state.loop_overlay {
        draw_loop_overlay(&painter, rect, overlay, total_ms, pen, palette);
    }

    // The sunken well frame, on top of the dim so the bevel is never buried.
    bevel::paint_bevel(&painter, rect, palette, Bevel::Sunken);

    // Either button reports; which one it was is the caller's to interpret, and
    // is what tells a loop start from a loop end.
    let secondary = response.secondary_clicked();
    if (response.clicked() || secondary)
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let pct = f64::from((pointer.x - rect.left()) / rect.width());
        out.clicked = timeline.index_and_ms_offset_at_pct(pct);
        out.secondary = secondary;
        out.modifiers = ui.input(|input| input.modifiers);
    }
    out
}

/// Height of the marker flags, in points.
const FLAG_HEIGHT: f32 = 9.0;
/// How far a flag juts inward from its bracket.
const FLAG_WIDTH: f32 = 7.0;

/// Draws the loop region: a bracket at each end, flags pointing inward so the
/// pair reads as enclosing what lies between them, and -- while looping is
/// actually on -- a faint wash over the region itself.
///
/// An unapplied region gets hollow flags: the same silhouette, outline only, so
/// the difference is legible without adding another colour to the panel.
fn draw_loop_overlay(
    painter: &egui::Painter,
    rect: Rect,
    overlay: LoopOverlay,
    total_ms: u32,
    pen: f32,
    palette: &Palette,
) {
    let start_x = x_for_ms(rect, overlay.start_ms, total_ms);
    let end_x = x_for_ms(rect, overlay.end_ms, total_ms);
    if overlay.active && end_x > start_x {
        let region = Rect::from_min_max(pos2(start_x, rect.top()), pos2(end_x, rect.bottom()));
        painter.rect_filled(region, 0.0, palette.wf_loop_region);
    }

    let colour = palette.wf_loop;
    vertical_line(painter, rect, start_x, pen, colour);
    vertical_line(painter, rect, end_x, pen, colour);
    // Flags point at each other, so a narrow region still reads as a pair rather
    // than two unrelated lines.
    flag(
        painter,
        start_x,
        rect.top(),
        FLAG_WIDTH,
        colour,
        overlay.unapplied,
    );
    flag(
        painter,
        end_x,
        rect.top(),
        -FLAG_WIDTH,
        colour,
        overlay.unapplied,
    );
}

/// One triangular flag hanging from `top` at `x`, pointing `width` points along
/// the x axis (negative points left). Filled normally, outlined when `hollow`.
fn flag(painter: &egui::Painter, x: f32, top: f32, width: f32, colour: Color32, hollow: bool) {
    let points = vec![
        pos2(x, top),
        pos2(x + width, top),
        pos2(x, top + FLAG_HEIGHT),
    ];
    let stroke = Stroke::new(1.0, colour);
    if hollow {
        painter.add(Shape::closed_line(points, stroke));
    } else {
        painter.add(Shape::convex_polygon(points, colour, stroke));
    }
}

/// Minimum space between two time labels, so the scale never crowds.
const MARKER_SPACING: f32 = 80.0;
/// The tick stub each label sits on, up from the bottom edge.
const TICK_HEIGHT: f32 = 5.0;
/// The round steps the time scale may use, finest first.
const MARKER_STEPS: [u32; 12] = [
    1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
    1_800_000,
];

/// The finest round step that keeps labels at least [`MARKER_SPACING`] apart,
/// or `None` when even the coarsest would crowd (a very narrow panel) or there
/// is no duration to divide.
fn marker_step(total_ms: u32, width: f32) -> Option<u32> {
    if total_ms == 0 {
        return None;
    }
    MARKER_STEPS
        .into_iter()
        .find(|&step| width * (step as f32 / total_ms as f32) >= MARKER_SPACING)
}

/// Draws the time scale: a tick and an MM:SS label at each multiple of the
/// round step. Tinted between the background and the wave, like the grid, so
/// the scale reads as part of the graticule; the start dim and the loop wash
/// paint over it like everything else.
fn draw_time_markers(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    total_ms: u32,
    palette: &Palette,
) {
    let Some(step) = marker_step(total_ms, rect.width()) else {
        return;
    };
    let colour = lerp_color(palette.wf_bg, palette.wf_wave, 0.55);
    let font = egui::TextStyle::Small.resolve(ui.style());
    // u64 so a pathological near-u32::MAX duration cannot overflow the walk.
    let mut ms = u64::from(step);
    while ms < u64::from(total_ms) {
        let x = x_for_ms(rect, ms as u32, total_ms);
        // The last label would jam against the well's right edge; the frame is
        // scale enough there.
        if x > rect.right() - MARKER_SPACING * 0.5 {
            break;
        }
        painter.line_segment(
            [pos2(x, rect.bottom() - TICK_HEIGHT), pos2(x, rect.bottom())],
            Stroke::new(1.0, colour),
        );
        painter.text(
            pos2(x, rect.bottom() - TICK_HEIGHT),
            egui::Align2::CENTER_BOTTOM,
            vgms_core::util::ms_to_timestr(ms as u32),
            font.clone(),
            colour,
        );
        ms += u64::from(step);
    }
}

fn draw_buckets(
    painter: &egui::Painter,
    rect: Rect,
    buckets: &[WaveformBucket],
    palette: &Palette,
) {
    if buckets.is_empty() {
        return;
    }
    // Auto-scale to the loudest bucket.
    let peak = buckets
        .iter()
        .map(|b| i32::from(b.max).max(-i32::from(b.min)))
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let centre = rect.center().y;
    let scale = (rect.height() / 2.0 - HEADROOM).max(0.0) / peak;

    let step = rect.width() / buckets.len() as f32;
    let width = step.ceil().max(1.0);
    let half = width * 0.5;
    let bright = palette.wf_wave;
    // A very subtle fade toward the background near the centre line.
    let dim = lerp_color(palette.wf_wave, palette.wf_bg, 0.30);

    let mut mesh = Mesh::default();
    for (i, bucket) in buckets.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * step;
        let top = centre - f32::from(bucket.max) * scale;
        let bottom = (centre - f32::from(bucket.min) * scale).max(top + 1.0);
        let split = centre.clamp(top, bottom);
        // Bright at the peaks, dim where the wave crosses the centre line.
        gradient_quad(&mut mesh, x - half, x + half, top, split, bright, dim);
        gradient_quad(&mut mesh, x - half, x + half, split, bottom, dim, bright);
    }
    painter.add(Shape::mesh(mesh));
}

/// The sunken-screen background: a subtle vertical gradient, a touch lighter
/// along the centre line and darker toward the top and bottom edges, overlaid
/// with a faint oscilloscope grid and a brighter centre line.
fn paint_background(painter: &egui::Painter, rect: Rect, palette: &Palette) {
    let edge = palette.wf_bg;
    let centre_colour = lerp_color(palette.wf_bg, Color32::WHITE, 0.07);
    let centre = rect.center().y;
    let mut mesh = Mesh::default();
    gradient_quad(
        &mut mesh,
        rect.left(),
        rect.right(),
        rect.top(),
        centre,
        edge,
        centre_colour,
    );
    gradient_quad(
        &mut mesh,
        rect.left(),
        rect.right(),
        centre,
        rect.bottom(),
        centre_colour,
        edge,
    );
    painter.add(Shape::mesh(mesh));

    // Scope grid: eight faint verticals and the quarter horizontals, tinted
    // toward the wave, then a brighter centre line -- all behind the wave.
    let grid = lerp_color(palette.wf_bg, palette.wf_wave, 0.09);
    let centre_line = lerp_color(palette.wf_bg, Color32::WHITE, 0.18);
    for i in 1..8 {
        let x = rect.left() + rect.width() * i as f32 / 8.0;
        painter.vline(x, rect.y_range(), Stroke::new(1.0, grid));
    }
    for i in [1.0_f32, 3.0] {
        let y = rect.top() + rect.height() * i / 4.0;
        painter.hline(rect.x_range(), y, Stroke::new(1.0, grid));
    }
    painter.hline(rect.x_range(), centre, Stroke::new(1.0, centre_line));
}

fn x_for_ms(rect: Rect, ms: u32, total_ms: u32) -> f32 {
    let pct = if total_ms == 0 {
        0.0
    } else {
        ms as f32 / total_ms as f32
    };
    rect.left() + pct.clamp(0.0, 1.0) * rect.width()
}

fn vertical_line(painter: &egui::Painter, rect: Rect, x: f32, pen: f32, color: Color32) {
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(pen, color),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scale picks the finest round step that keeps labels apart, and
    /// declines to draw at all when nothing fits.
    #[test]
    fn the_time_scale_picks_the_finest_uncrowded_step() {
        // A minute across 900px: 5s markers sit 75px apart (crowded), 10s fit.
        assert_eq!(marker_step(60_000, 900.0), Some(10_000));
        // A short jingle gets one-second markers.
        assert_eq!(marker_step(8_000, 800.0), Some(1_000));
        // Nothing to divide: no scale.
        assert_eq!(marker_step(0, 800.0), None);
        // A hundred-hour log squeezed into 100px: even the coarsest step
        // crowds, so the scale stands down rather than smearing labels.
        assert_eq!(marker_step(100 * 3_600_000, 100.0), None);
    }
}
