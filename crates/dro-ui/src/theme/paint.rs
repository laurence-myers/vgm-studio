//! Shared low-level painting helpers for the theme's custom chrome: colour
//! interpolation and the vertical-gradient fill used by the pad caps, the plate
//! fascia and the waveform.

use egui::epaint::Mesh;
use egui::{Color32, Rect, Shape, pos2};

/// Componentwise (ignoring alpha) colour interpolation, `t` of the way from `a`
/// to `b`. `t` is clamped to `0..=1`.
#[must_use]
pub(crate) fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// `c` lightened `t` of the way toward white.
#[must_use]
pub(crate) fn lighten(c: Color32, t: f32) -> Color32 {
    lerp_color(c, Color32::WHITE, t)
}

/// `c` darkened `t` of the way toward black.
#[must_use]
pub(crate) fn darken(c: Color32, t: f32) -> Color32 {
    lerp_color(c, Color32::BLACK, t)
}

/// Adds a vertical-gradient rectangle to `mesh`: `top` colour at `y_top`,
/// `bottom` at `y_bottom`, interpolated linearly down the quad.
pub(crate) fn gradient_quad(
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

/// A vertical-gradient fill of `rect` from `top` (top edge) to `bottom` (bottom
/// edge) as a [`Shape`] -- the plate fascia's brushed-metal sheen.
pub(crate) fn plate_mesh(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = Mesh::default();
    gradient_quad(
        &mut mesh,
        rect.left(),
        rect.right(),
        rect.top(),
        rect.bottom(),
        top,
        bottom,
    );
    Shape::mesh(mesh)
}
