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
//!
//! Everything here is denominated in [`Song::total_delay_ms`], never the
//! header's `ms_length`, so a DRO with a lying header still maps clicks to the
//! right instructions.

use dro_core::Song;
use dro_core::util::ms_to_timestr;
use dro_synth::WaveformBucket;
use egui::epaint::Mesh;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2};

use crate::theme::Palette;
use crate::theme::bevel::{self, Bevel};
use crate::theme::paint::{gradient_quad, lerp_color};

/// The fixed bucket count. The painted panel stretches these to its width.
pub const NUM_BUCKETS: usize = 768;

/// Headroom above the tallest bucket.
const HEADROOM: f32 = 5.0;

/// The waveform's displayed state, owned by the app.
#[derive(Debug, Default)]
pub struct WaveformState {
    pub buckets: Vec<WaveformBucket>,
    /// The white playback-start indicator, from the selected row.
    pub start_ms: u32,
    /// The yellow cursor. Only playback moves it; it survives edits, and is
    /// reset explicitly on file load.
    pub cursor_ms: u32,
    /// The loop brackets, when there is a region worth showing.
    pub loop_overlay: Option<LoopOverlay>,
}

/// The loop region as the panel needs it: in milliseconds, plus the two facts
/// that change how it is drawn.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LoopOverlay {
    pub start_ms: u32,
    pub end_ms: u32,
    /// Loop playback is on, so the region is washed as well as bracketed --
    /// the difference between "marked" and "actually repeating".
    pub active: bool,
    /// The markers differ from the loop the song stores, so the flags are drawn
    /// hollow: the cue that there is something to apply.
    pub unapplied: bool,
}

/// What the panel reported this frame.
#[derive(Debug, Default)]
pub struct WaveformResponse {
    /// A click, already snapped to an instruction and its time.
    pub clicked: Option<(usize, u32)>,
    /// Whether `clicked` was the secondary (right) button.
    pub secondary: bool,
    /// The modifiers held for `clicked`, so the caller can tell a plain seek from
    /// a loop-marking gesture. The panel itself stays ignorant of what they mean.
    pub modifiers: egui::Modifiers,
}

pub fn show(
    ui: &mut egui::Ui,
    state: &WaveformState,
    song: Option<&Song>,
    palette: &Palette,
) -> WaveformResponse {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let rect = response.rect;
    paint_background(&painter, rect, palette);

    let mut out = WaveformResponse::default();
    let Some(song) = song else {
        // Still frame the empty well, so the panel reads as a sunken area.
        bevel::paint_bevel(&painter, rect, palette, Bevel::Sunken);
        return out;
    };
    let total_ms = song.total_delay_ms();

    draw_buckets(&painter, rect, &state.buckets, palette);

    // Pen width scales with the panel: `width // 768 + 1`.
    let pen = (rect.width() / NUM_BUCKETS as f32 + 1.0).floor();

    // Hover: snap to the instruction under the pointer, preview line + time.
    let mut hover_x = None;
    if let Some(pointer) = response.hover_pos() {
        let pct = f64::from((pointer.x - rect.left()) / rect.width());
        if let Some((_, ms)) = song.index_and_ms_offset_at_pct(pct) {
            hover_x = Some(x_for_ms(rect, ms, total_ms));
            response
                .clone()
                .on_hover_text(format!("{ms} ms ({})", ms_to_timestr(ms)));
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
        out.clicked = song.index_and_ms_offset_at_pct(pct);
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
/// along the centre line and darker toward the top and bottom edges.
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
