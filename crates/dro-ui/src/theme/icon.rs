//! Line-icon glyphs for the transport, pan and channel pads, painted with
//! epaint primitives (no SVG dependency). Every glyph is authored on a 16x16
//! grid matching the mock-up's `<defs>` (see `docs/button-chrome-2026-07`), then
//! mapped into whatever square rect it is drawn in. Strokes are butt-capped
//! line segments and polylines; a few glyphs add filled triangles/rects.
//!
//! The stroke width is a single inherited parameter (`1.5` at the shipped 16px
//! size), so the whole set can be made heavier or lighter in one place.

use egui::{Color32, Pos2, Rect, Shape, Stroke, Vec2, vec2};

/// The transport / pan / channel verbs, each a `currentColor` line glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Delete: a waste bin with two slots.
    Del,
    /// Play: a filled right-pointing triangle.
    Play,
    /// Stop: a filled square.
    Stop,
    /// Tail: play pressed against the end bar.
    Tail,
    /// Seam: outward brackets `] [` with an arrow across the join.
    Seam,
    /// Loop: two chasing arcs (circular repeat).
    Loop,
    /// Lock: a closed padlock with a slot keyhole.
    Lock,
    /// Match: expand-to-rails (bring the peak to full scale).
    Match,
    /// Custom: three mixer sliders with square thumbs.
    Custom,
    /// Reset: a counter-clockwise return arrow.
    Reset,
    /// Percussion: a drum with crossed sticks.
    Perc,
    /// All: a full 3x3 grid of cells.
    All,
    /// Up chevron (louder).
    Up,
    /// Down chevron (quieter).
    Dn,
}

/// Draws `icon` centred in `rect` (the largest centred square is used), inked in
/// `color` at `stroke_width` pixels. `rect` need not be square; the glyph keeps
/// its aspect on the 16-grid.
pub(crate) fn draw(
    painter: &egui::Painter,
    icon: Icon,
    rect: Rect,
    color: Color32,
    stroke_width: f32,
) {
    // Fit the 16-grid into the largest centred square of `rect`.
    let side = rect.width().min(rect.height());
    let s = side / 16.0;
    let origin = rect.center() - Vec2::splat(side / 2.0);
    // Map a 16-grid point into paint space.
    let p = |x: f32, y: f32| origin + vec2(x * s, y * s);
    let pen = Stroke::new(stroke_width, color);

    match icon {
        Icon::Del => del(painter, &p, pen),
        Icon::Play => fill_tri(painter, &p, color, [(4.5, 2.75), (13.0, 8.0), (4.5, 13.25)]),
        Icon::Stop => fill_rect(painter, &p, color, 4.25, 4.25, 7.5, 7.5),
        Icon::Tail => tail(painter, &p, color),
        Icon::Seam => seam(painter, &p, pen),
        Icon::Loop => loop_icon(painter, &p, pen),
        Icon::Lock => lock(painter, &p, color, pen),
        Icon::Match => match_icon(painter, &p, pen),
        Icon::Custom => custom(painter, &p, color, pen),
        Icon::Reset => reset(painter, &p, pen),
        Icon::Perc => perc(painter, &p, pen),
        Icon::All => all(painter, &p, color),
        Icon::Up => poly(painter, &p, pen, &[(3.0, 10.5), (8.0, 5.5), (13.0, 10.5)]),
        Icon::Dn => poly(painter, &p, pen, &[(3.0, 5.5), (8.0, 10.5), (13.0, 5.5)]),
    }
}

// -- primitives (all on the 16-grid; `p` maps into paint space) ------------

/// An open polyline through grid points.
fn poly(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke, pts: &[(f32, f32)]) {
    let points: Vec<Pos2> = pts.iter().map(|&(x, y)| p(x, y)).collect();
    painter.add(Shape::line(points, pen));
}

/// A single straight segment.
fn seg(
    painter: &egui::Painter,
    p: &impl Fn(f32, f32) -> Pos2,
    pen: Stroke,
    a: (f32, f32),
    b: (f32, f32),
) {
    painter.line_segment([p(a.0, a.1), p(b.0, b.1)], pen);
}

/// A filled triangle.
fn fill_tri(
    painter: &egui::Painter,
    p: &impl Fn(f32, f32) -> Pos2,
    fill: Color32,
    pts: [(f32, f32); 3],
) {
    let points = pts.iter().map(|&(x, y)| p(x, y)).collect();
    painter.add(Shape::convex_polygon(points, fill, Stroke::NONE));
}

/// A filled axis-aligned rect given as grid `x, y, w, h`.
fn fill_rect(
    painter: &egui::Painter,
    p: &impl Fn(f32, f32) -> Pos2,
    fill: Color32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) {
    painter.add(Shape::convex_polygon(
        vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
        fill,
        Stroke::NONE,
    ));
}

/// Samples a circular arc (grid space), centre `(cx, cy)` radius `r`, from angle
/// `a0` to `a1` (radians, screen space: +y down), into a polyline.
fn arc_pts(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, segments: usize) -> Vec<(f32, f32)> {
    (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let a = a0 + (a1 - a0) * t;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// A stroked open arc, centre `(cx, cy)` radius `r`, sweeping `angles.0..angles.1`.
fn arc(
    painter: &egui::Painter,
    p: &impl Fn(f32, f32) -> Pos2,
    pen: Stroke,
    center: (f32, f32),
    r: f32,
    angles: (f32, f32),
) {
    let pts = arc_pts(center.0, center.1, r, angles.0, angles.1, 24);
    poly(painter, p, pen, &pts);
}

/// The minor circular arc between `p0` and `p1` of the given `radius`, bulging
/// toward `toward` (all grid space). Used for the loop's two chasing arcs, whose
/// centres are implied by their endpoints rather than given.
fn arc_between(
    p0: (f32, f32),
    p1: (f32, f32),
    radius: f32,
    toward: (f32, f32),
    segments: usize,
) -> Vec<(f32, f32)> {
    let (mx, my) = ((p0.0 + p1.0) / 2.0, (p0.1 + p1.1) / 2.0);
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let half = len / 2.0;
    let r = radius.max(half);
    let d = (r * r - half * half).max(0.0).sqrt();
    // Unit perpendicular to the chord; the two candidate centres straddle it.
    let (px, py) = (-dy / len, dx / len);
    let ca = (mx + px * d, my + py * d);
    let cb = (mx - px * d, my - py * d);
    let dist2 = |c: (f32, f32)| (c.0 - toward.0).powi(2) + (c.1 - toward.1).powi(2);
    // The arc bulges away from its centre, so pick the centre farther from
    // `toward` to bulge toward it.
    let c = if dist2(ca) >= dist2(cb) { ca } else { cb };
    let a0 = (p0.1 - c.1).atan2(p0.0 - c.0);
    let a1 = (p1.1 - c.1).atan2(p1.0 - c.0);
    let mut delta = a1 - a0;
    while delta > PI {
        delta -= 2.0 * PI;
    }
    while delta < -PI {
        delta += 2.0 * PI;
    }
    (0..=segments)
        .map(|i| {
            let a = a0 + delta * (i as f32 / segments as f32);
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

// -- glyphs ----------------------------------------------------------------

use std::f32::consts::PI;

fn del(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    // Lid line, handle, body trapezoid, two slots.
    seg(painter, p, pen, (2.5, 4.25), (13.5, 4.25));
    poly(
        painter,
        p,
        pen,
        &[(6.0, 4.25), (6.0, 2.5), (10.0, 2.5), (10.0, 4.25)],
    );
    poly(
        painter,
        p,
        pen,
        &[(4.25, 4.25), (4.8, 13.5), (11.2, 13.5), (11.75, 4.25)],
    );
    seg(painter, p, pen, (6.9, 6.75), (6.9, 11.25));
    seg(painter, p, pen, (9.1, 6.75), (9.1, 11.25));
}

fn tail(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, fill: Color32) {
    fill_tri(painter, p, fill, [(3.5, 4.25), (10.0, 8.0), (3.5, 11.75)]);
    fill_rect(painter, p, fill, 12.0, 3.0, 1.8, 10.0);
}

fn seam(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    // Left bracket ], right bracket [, arrow shaft, arrowhead.
    poly(
        painter,
        p,
        pen,
        &[(2.0, 1.5), (4.25, 1.5), (4.25, 8.5), (2.0, 8.5)],
    );
    poly(
        painter,
        p,
        pen,
        &[(14.0, 1.5), (11.75, 1.5), (11.75, 8.5), (14.0, 8.5)],
    );
    seg(painter, p, pen, (3.5, 13.25), (10.75, 13.25));
    poly(
        painter,
        p,
        pen,
        &[(10.0, 11.5), (12.25, 13.25), (10.0, 15.0)],
    );
}

fn loop_icon(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    // Two chasing radius-5 arcs (each its own circle) with squared arrowheads.
    // Top arc bulges up and ends at the right, where its arrowhead sits.
    poly(
        painter,
        p,
        pen,
        &arc_between((3.5, 6.5), (12.5, 5.0), 5.0, (8.0, 0.0), 20),
    );
    poly(painter, p, pen, &[(12.5, 1.75), (12.5, 5.0), (9.25, 5.0)]);
    // Bottom arc bulges down and ends at the left.
    poly(
        painter,
        p,
        pen,
        &arc_between((12.5, 9.5), (3.5, 11.0), 5.0, (8.0, 16.0), 20),
    );
    poly(painter, p, pen, &[(3.5, 14.25), (3.5, 11.0), (6.75, 11.0)]);
}

fn lock(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, fill: Color32, pen: Stroke) {
    // Body (stroked rect), a closed shackle (two uprights + a semicircle over
    // the top), and a filled slot keyhole.
    const SHACKLE_TOP: f32 = 5.25;
    poly(
        painter,
        p,
        pen,
        &[
            (3.25, 7.0),
            (12.75, 7.0),
            (12.75, 13.5),
            (3.25, 13.5),
            (3.25, 7.0),
        ],
    );
    seg(painter, p, pen, (5.5, 7.0), (5.5, SHACKLE_TOP));
    seg(painter, p, pen, (10.5, 7.0), (10.5, SHACKLE_TOP));
    arc(painter, p, pen, (8.0, SHACKLE_TOP), 2.5, (PI, 2.0 * PI));
    fill_rect(painter, p, fill, 7.35, 9.25, 1.3, 2.5);
}

fn match_icon(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    seg(painter, p, pen, (2.5, 2.25), (13.5, 2.25));
    seg(painter, p, pen, (2.5, 13.75), (13.5, 13.75));
    seg(painter, p, pen, (8.0, 4.5), (8.0, 11.5));
    poly(painter, p, pen, &[(5.75, 6.5), (8.0, 4.25), (10.25, 6.5)]);
    poly(painter, p, pen, &[(5.75, 9.5), (8.0, 11.75), (10.25, 9.5)]);
}

fn custom(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, fill: Color32, pen: Stroke) {
    seg(painter, p, pen, (4.0, 2.5), (4.0, 13.5));
    seg(painter, p, pen, (8.0, 2.5), (8.0, 13.5));
    seg(painter, p, pen, (12.0, 2.5), (12.0, 13.5));
    fill_rect(painter, p, fill, 2.75, 4.25, 2.5, 2.5);
    fill_rect(painter, p, fill, 6.75, 8.75, 2.5, 2.5);
    fill_rect(painter, p, fill, 10.75, 5.25, 2.5, 2.5);
}

fn reset(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    // A 270-degree CCW ring open at the top-right, centred on (8,8) radius 5,
    // then the return arrowhead at the top.
    arc(painter, p, pen, (8.0, 8.0), 5.0, (PI, -PI * 0.5));
    poly(painter, p, pen, &[(10.25, 1.25), (8.0, 3.0), (10.25, 4.75)]);
}

fn perc(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, pen: Stroke) {
    // Two sticks meeting at the drum, the drum head (ellipse) and its body.
    seg(painter, p, pen, (2.5, 1.5), (7.0, 7.5));
    seg(painter, p, pen, (13.5, 1.5), (9.0, 7.5));
    // Head: an ellipse rx5 ry1.75 about (8,9.5); sample directly (non-uniform).
    let head: Vec<(f32, f32)> = (0..=32)
        .map(|i| {
            let a = i as f32 / 32.0 * 2.0 * PI;
            (8.0 + 5.0 * a.cos(), 9.5 + 1.75 * a.sin())
        })
        .collect();
    poly(painter, p, pen, &head);
    // Body: verticals down from the head rim, closed across the bottom.
    poly(
        painter,
        p,
        pen,
        &[
            (3.0, 9.5),
            (3.0, 12.75),
            (8.0, 13.9),
            (13.0, 12.75),
            (13.0, 9.5),
        ],
    );
}

fn all(painter: &egui::Painter, p: &impl Fn(f32, f32) -> Pos2, fill: Color32) {
    for row in 0..3 {
        for col in 0..3 {
            let x = 2.0 + col as f32 * 4.75;
            let y = 2.0 + row as f32 * 4.75;
            fill_rect(painter, p, fill, x, y, 2.5, 2.5);
        }
    }
}
