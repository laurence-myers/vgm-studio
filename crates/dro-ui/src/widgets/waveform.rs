//! The waveform panel (`waveform.py`).
//!
//! Same visual language as the Python: navy background, green waveform,
//! white playback-start line, yellow playback cursor, pale-cyan hover line
//! that snaps to instruction offsets, and a half-black dim over everything
//! left of the start line. Differences, both deliberate:
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
use egui::{Color32, Pos2, Rect, Sense, Stroke, pos2};

/// The fixed bucket count, as the Python's `x_resolution`. The painted panel
/// stretches these to its width.
pub const NUM_BUCKETS: usize = 768;

const BACKGROUND: Color32 = Color32::from_rgb(0x11, 0x22, 0x55);
const WAVE: Color32 = Color32::from_rgb(0x22, 0xFF, 0x22);
const HOVER: Color32 = Color32::from_rgb(0xAA, 0xCC, 0xCC);
const START: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
const CURSOR: Color32 = Color32::from_rgb(0xFF, 0xFF, 0x00);
const DIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 0x7F);
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

pub fn show(ui: &mut egui::Ui, state: &WaveformState, song: Option<&Song>) -> WaveformResponse {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, BACKGROUND);

    let mut out = WaveformResponse::default();
    let Some(song) = song else {
        return out;
    };
    let total_ms = song.total_delay_ms();

    draw_buckets(&painter, rect, &state.buckets);

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
        vertical_line(&painter, rect, x, pen, HOVER);
    }

    let start_x = x_for_ms(rect, state.start_ms, total_ms);
    vertical_line(&painter, rect, start_x, pen, START);
    vertical_line(
        &painter,
        rect,
        x_for_ms(rect, state.cursor_ms, total_ms),
        pen,
        CURSOR,
    );

    // Dim everything left of the start indicator, over the lines too.
    if start_x > rect.left() {
        let dimmed = Rect::from_min_max(rect.min, pos2(start_x, rect.bottom()));
        painter.rect_filled(dimmed, 0.0, DIM);
    }

    if response.clicked() {
        if let Some(pointer) = response.interact_pointer_pos() {
            let pct = f64::from((pointer.x - rect.left()) / rect.width());
            out.clicked = song.index_and_ms_offset_at_pct(pct);
        }
    }
    out
}

fn draw_buckets(painter: &egui::Painter, rect: Rect, buckets: &[WaveformBucket]) {
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
    for (i, bucket) in buckets.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) * step;
        let top = centre - f32::from(bucket.max) * scale;
        let bottom = centre - f32::from(bucket.min) * scale;
        painter.line_segment(
            [pos2(x, top), pos2(x, bottom.max(top + 1.0))],
            Stroke::new(width, WAVE),
        );
    }
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
