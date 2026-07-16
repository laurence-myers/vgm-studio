//! The waveform panel (`waveform.py`).
//!
//! Same visual language as the Python -- a dark well, a bright wave, a white
//! playback-start line, a bright playback cursor, a pale hover line that snaps
//! to instruction offsets, and a half-black dim over everything left of the
//! start line -- but the exact colours now come from the active [`Palette`], so
//! each theme tints it. Differences from the Python, both deliberate:
//!
//! - The buckets are true min/max (the Python tracked only the positive
//!   peak), so the wave is drawn symmetrically around a centre line rather
//!   than as bars growing from the bottom.
//! - Hovering also shows the snapped time as a tooltip -- the plan calls for
//!   one; the Python had none.
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

/// The fixed bucket count, as the Python's `x_resolution`. The painted panel
/// stretches these to its width.
pub const NUM_BUCKETS: usize = 768;

/// Headroom above the tallest bucket, as the Python's fixed 10px gap.
const HEADROOM: f32 = 5.0;

/// The waveform's displayed state, owned by the app.
#[derive(Debug, Default)]
pub struct WaveformState {
    pub buckets: Vec<WaveformBucket>,
    /// The white playback-start indicator, from the selected row.
    pub start_ms: u32,
    /// The yellow cursor. Only playback moves it; it survives edits, as in
    /// Python, and is reset explicitly on file load.
    pub cursor_ms: u32,
}

/// What the panel reported this frame.
#[derive(Debug, Default)]
pub struct WaveformResponse {
    /// A click, already snapped to an instruction and its time.
    pub clicked: Option<(usize, u32)>,
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

    // Pen width scales with the panel, as the Python's `width // 768 + 1`.
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

    // The sunken well frame, on top of the dim so the bevel is never buried.
    bevel::paint_bevel(&painter, rect, palette, Bevel::Sunken);

    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let pct = f64::from((pointer.x - rect.left()) / rect.width());
            out.clicked = song.index_and_ms_offset_at_pct(pct);
        }
    }
    out
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
    // Auto-scale to the loudest bucket, as the Python did (`max_value or 1`).
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

/// A vertical-gradient rectangle: `top` colour at `y_top`, `bottom` at `y_bottom`.
fn gradient_quad(
    mesh: &mut Mesh,
    x0: f32,
    x1: f32,
    y_top: f32,
    y_bottom: f32,
    top: Color32,
    bottom: Color32,
) {
    let base = mesh.vertices.len() as u32;
    mesh.colored_vertex(pos2(x0, y_top), top);
    mesh.colored_vertex(pos2(x1, y_top), top);
    mesh.colored_vertex(pos2(x1, y_bottom), bottom);
    mesh.colored_vertex(pos2(x0, y_bottom), bottom);
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
}

/// Componentwise colour interpolation, `t` of the way from `a` to `b`.
fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
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
