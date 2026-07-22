//! Shared low-level painting helpers for the theme's custom chrome: colour
//! interpolation used by the pad caps, the plate gradients and the waveform.

use egui::Color32;

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
