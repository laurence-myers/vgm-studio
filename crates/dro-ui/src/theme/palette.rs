//! The two DOS-tracker colour schemes, as static [`Palette`]s.
//!
//! Every colour the theme needs is a named role here rather than a raw
//! `Color32` scattered through the widgets, so the two presets are just two
//! tables of the same shape and a future third scheme is one more `const`.

use dro_core::config::ThemeChoice;
use egui::Color32;

/// A complete colour scheme. All FastTracker II-ish: a beige/steel "face" with
/// two-tone bevels for chrome, and a near-black "data" area with bright text
/// for the pattern/table/waveform, plus the six waveform-specific colours.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    // -- chrome (panels, buttons, windows, menus) --
    /// Button and panel face.
    pub face: Color32,
    /// Face under a hovering pointer.
    pub face_hover: Color32,
    /// Face while pressed / active / a window's title bar.
    pub face_active: Color32,
    /// The desktop behind the panels (`panel_fill`).
    pub desktop: Color32,
    /// The lit bevel edge (top-left when raised).
    pub bevel_light: Color32,
    /// The shadowed bevel edge (bottom-right when raised); also separators and
    /// window outlines.
    pub bevel_dark: Color32,
    /// The near-black keyline framing a raised button's shadow side.
    pub bevel_border: Color32,

    // -- data areas (table, text fields, waveform well) --
    /// The sunken data background.
    pub data_bg: Color32,
    /// The alternate (striped) data row.
    pub data_stripe: Color32,
    /// A hovered data row.
    pub data_hover: Color32,
    /// Bright primary data text (register values -- the tracker "yellow").
    pub data_text: Color32,
    /// Secondary data text (descriptions, the empty-state hint).
    pub data_label: Color32,
    /// The scrollbar trough (`extreme_bg_color`).
    pub trough: Color32,

    // -- text on chrome --
    /// Text on the face (menus, dialogs, status bar).
    pub label: Color32,
    /// Dimmed text (`weak_text_color`).
    pub muted: Color32,

    // -- push buttons (grey, as a real FT2 button, whatever the panel tint) --
    /// The grey button face.
    pub button_face: Color32,
    /// The button face under a hovering pointer.
    pub button_hover: Color32,
    /// The button face while pressed.
    pub button_active: Color32,
    /// The lit inner bevel of a raised button (top-left).
    pub button_light: Color32,
    /// The shadowed inner bevel of a raised button (bottom-right).
    pub button_shadow: Color32,
    /// Text on a button; near-black, as on a real FT2 button.
    pub button_text: Color32,

    // -- selection --
    /// The selection bar fill.
    pub accent: Color32,
    /// Text drawn over the selection bar.
    pub selection_text: Color32,

    // -- waveform (the six former `waveform.rs` constants) --
    /// Waveform panel background.
    pub wf_bg: Color32,
    /// The wave itself.
    pub wf_wave: Color32,
    /// The snap-to-instruction hover line.
    pub wf_hover: Color32,
    /// The playback-start line.
    pub wf_start: Color32,
    /// The playback cursor.
    pub wf_cursor: Color32,
    /// The half-black overlay left of the start line.
    pub wf_dim: Color32,

    // -- peak meter (the well behind it reuses `wf_bg`) --
    /// An unlit meter segment.
    pub meter_off: Color32,
    /// Lit segments in the lower zone.
    pub meter_low: Color32,
    /// Lit segments in the upper-middle zone.
    pub meter_mid: Color32,
    /// Lit segments in the top zone.
    pub meter_high: Color32,
    /// The peak-hold marker.
    pub meter_hold: Color32,
}

/// The ft2-clone dark teal scheme (the default).
pub(crate) const CLONE_DARK: Palette = Palette {
    face: Color32::from_rgb(0x4A, 0x73, 0x73),
    face_hover: Color32::from_rgb(0x57, 0x82, 0x82),
    face_active: Color32::from_rgb(0x3E, 0x61, 0x61),
    desktop: Color32::from_rgb(0x26, 0x3A, 0x3A),
    bevel_light: Color32::from_rgb(0x8F, 0xBF, 0xBF),
    bevel_dark: Color32::from_rgb(0x1A, 0x29, 0x29),
    bevel_border: Color32::from_rgb(0x07, 0x0D, 0x0D),

    data_bg: Color32::from_rgb(0x0C, 0x14, 0x14),
    data_stripe: Color32::from_rgb(0x14, 0x20, 0x20),
    data_hover: Color32::from_rgb(0x1B, 0x2C, 0x2C),
    data_text: Color32::from_rgb(0xF1, 0xE6, 0x7B),
    data_label: Color32::from_rgb(0xB8, 0xD0, 0xD0),
    trough: Color32::from_rgb(0x18, 0x26, 0x26),

    label: Color32::from_rgb(0xDC, 0xEF, 0xEF),
    // Light enough to read as menu shortcut text on the teal face and as the
    // Bank column on the near-black data background.
    muted: Color32::from_rgb(0x86, 0xA6, 0xA6),

    button_face: Color32::from_rgb(0x9C, 0xA7, 0xA7),
    button_hover: Color32::from_rgb(0xAB, 0xB6, 0xB6),
    button_active: Color32::from_rgb(0x88, 0x93, 0x93),
    button_light: Color32::from_rgb(0xE2, 0xEA, 0xEA),
    button_shadow: Color32::from_rgb(0x53, 0x5F, 0x5F),
    button_text: Color32::BLACK,

    accent: Color32::from_rgb(0x33, 0x55, 0xAA),
    selection_text: Color32::WHITE,

    wf_bg: Color32::from_rgb(0x0A, 0x10, 0x24),
    wf_wave: Color32::from_rgb(0xF1, 0xE6, 0x7B),
    wf_hover: Color32::from_rgb(0xAA, 0xCC, 0xCC),
    wf_start: Color32::WHITE,
    // The wave is yellow, so the cursor moves to cyan to stay visible over it.
    wf_cursor: Color32::from_rgb(0x7C, 0xE0, 0xE0),
    wf_dim: Color32::from_rgba_premultiplied(0, 0, 0, 0x7F),

    meter_off: Color32::from_rgb(0x1C, 0x28, 0x3C),
    meter_low: Color32::from_rgb(0x3C, 0xC8, 0x50),
    meter_mid: Color32::from_rgb(0xE6, 0xC8, 0x46),
    meter_high: Color32::from_rgb(0xE0, 0x4A, 0x4A),
    meter_hold: Color32::from_rgb(0xEA, 0xF4, 0xF4),
};

/// The original DOS FastTracker II steel-blue scheme.
pub(crate) const FT2_CLASSIC: Palette = Palette {
    face: Color32::from_rgb(0x6E, 0x82, 0xA0),
    face_hover: Color32::from_rgb(0x7B, 0x90, 0xB0),
    face_active: Color32::from_rgb(0x5F, 0x73, 0x90),
    desktop: Color32::from_rgb(0x30, 0x3E, 0x58),
    bevel_light: Color32::from_rgb(0xC6, 0xD2, 0xE4),
    bevel_dark: Color32::from_rgb(0x2E, 0x3A, 0x4C),
    bevel_border: Color32::from_rgb(0x0A, 0x0D, 0x14),

    data_bg: Color32::from_rgb(0x1A, 0x22, 0x38),
    data_stripe: Color32::from_rgb(0x22, 0x2C, 0x46),
    data_hover: Color32::from_rgb(0x2A, 0x36, 0x54),
    data_text: Color32::from_rgb(0xEF, 0xE2, 0x7A),
    data_label: Color32::from_rgb(0xC8, 0xD4, 0xE8),
    trough: Color32::from_rgb(0x23, 0x2D, 0x42),

    label: Color32::BLACK,
    muted: Color32::from_rgb(0x4A, 0x5A, 0x74),

    button_face: Color32::from_rgb(0xAC, 0xB1, 0xB8),
    button_hover: Color32::from_rgb(0xBA, 0xBF, 0xC6),
    button_active: Color32::from_rgb(0x99, 0x9E, 0xA6),
    button_light: Color32::from_rgb(0xEC, 0xEF, 0xF4),
    button_shadow: Color32::from_rgb(0x58, 0x5E, 0x68),
    button_text: Color32::BLACK,

    accent: Color32::from_rgb(0x40, 0x56, 0xA0),
    selection_text: Color32::WHITE,

    wf_bg: Color32::from_rgb(0x0A, 0x10, 0x24),
    wf_wave: Color32::from_rgb(0xC8, 0xD8, 0xF0),
    wf_hover: Color32::from_rgb(0xAA, 0xCC, 0xCC),
    wf_start: Color32::WHITE,
    wf_cursor: Color32::from_rgb(0xFF, 0xFF, 0x00),
    wf_dim: Color32::from_rgba_premultiplied(0, 0, 0, 0x7F),

    // Steel-tinted takes on the classic green/amber/red zones.
    meter_off: Color32::from_rgb(0x26, 0x30, 0x4A),
    meter_low: Color32::from_rgb(0x55, 0xC8, 0x6E),
    meter_mid: Color32::from_rgb(0xE0, 0xC8, 0x55),
    meter_high: Color32::from_rgb(0xDC, 0x55, 0x55),
    meter_hold: Color32::from_rgb(0xF0, 0xF2, 0xF8),
};

/// The palette for a configured theme.
#[must_use]
pub fn palette(choice: ThemeChoice) -> &'static Palette {
    match choice {
        ThemeChoice::CloneDark => &CLONE_DARK,
        ThemeChoice::Ft2Classic => &FT2_CLASSIC,
    }
}
