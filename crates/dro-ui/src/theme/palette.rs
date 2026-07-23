//! The DOS-tracker colour schemes, as static [`Palette`]s composed from a
//! [`Skin`].
//!
//! Every colour the theme needs is a named role rather than a raw `Color32`
//! scattered through the widgets. The roles split by *who may override them*:
//! [`CaseColors`] are the per-case fascia colours (panels, buttons, chrome
//! text, selection), and [`HardwareColors`] are the fixed "display" colours
//! (the dark data wells, the tracker-yellow readouts, the scope, the VU meter)
//! that stay put as the case changes. A [`Skin`] pairs one of each and
//! [`Skin::compose`]s them into the flat [`Palette`] the widgets consume, so a
//! future case is one more `CaseColors` const over a shared `HardwareColors`.

use dro_core::config::ThemeChoice;
use egui::Color32;

use super::paint::{darken, lighten};

/// Shorthand for an opaque sRGB colour, to keep the dense case tables legible.
const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// How a case paints its pads and its deck, independently of the plate.
/// `Tint` follows the plate (the default look); `Light`, `Dark` and `Grey` are
/// fixed treatments that ignore it, so a case can, say, mount cream keys on a
/// dark deck over a navy plate, or plain grey keys on a teal plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A fixed light (bone/cream) treatment.
    Light,
    /// A fixed dark (charcoal/rubber) treatment.
    Dark,
    /// A fixed neutral grey treatment, ignoring the plate.
    Grey,
    /// Tinted to match the case's plate.
    Tint,
}

/// A resolved pad cap: the two gradient stops, the border, and the content ink.
pub(crate) struct PadCaps {
    pub top: Color32,
    pub bottom: Color32,
    pub border: Color32,
    pub ink: Color32,
}

/// Resolves a palette's pad cap colours from its [`Surface`] mode. `Tint`
/// derives from the plate; `Light`/`Dark` are fixed presets.
#[must_use]
pub(crate) fn pad_caps(p: &Palette) -> PadCaps {
    match p.pad {
        // Neutral white/light grey, so "light" reads as a plain pale keycap
        // rather than a cream one that fights a cool case.
        Surface::Light => PadCaps {
            top: Color32::from_rgb(0xF2, 0xF3, 0xF3),
            bottom: Color32::from_rgb(0xDA, 0xDD, 0xDD),
            border: Color32::from_rgb(0x48, 0x4C, 0x4C),
            ink: Color32::from_rgb(0x1C, 0x1E, 0x1E),
        },
        Surface::Dark => PadCaps {
            top: Color32::from_rgb(0x41, 0x4B, 0x4B),
            bottom: Color32::from_rgb(0x2E, 0x38, 0x38),
            border: Color32::from_rgb(0x0A, 0x0F, 0x0F),
            ink: Color32::from_rgb(0xC6, 0xD0, 0xD0),
        },
        Surface::Grey => PadCaps {
            top: Color32::from_rgb(0xB6, 0xBA, 0xBA),
            bottom: Color32::from_rgb(0x9E, 0xA2, 0xA2),
            border: Color32::from_rgb(0x0B, 0x0D, 0x0D),
            ink: Color32::from_rgb(0x18, 0x1C, 0x1C),
        },
        Surface::Tint => PadCaps {
            top: lighten(p.plate_top, 0.42),
            bottom: lighten(p.plate_top, 0.26),
            border: p.plate_border,
            ink: darken(p.plate_bottom, 0.58),
        },
    }
}

/// Resolves a palette's deck gradient (top, bottom) from its [`Surface`] mode.
/// `Tint` is exactly the plate; `Light`/`Dark` are fixed presets.
#[must_use]
pub(crate) fn deck_stops(p: &Palette) -> (Color32, Color32) {
    match p.deck {
        Surface::Light => (
            Color32::from_rgb(0xE8, 0xE0, 0xCC),
            Color32::from_rgb(0xD4, 0xCA, 0xAE),
        ),
        Surface::Dark => (
            Color32::from_rgb(0x24, 0x30, 0x2F),
            Color32::from_rgb(0x15, 0x1D, 0x1D),
        ),
        Surface::Grey => (
            Color32::from_rgb(0x50, 0x56, 0x56),
            Color32::from_rgb(0x38, 0x3C, 0x3C),
        ),
        Surface::Tint => (p.plate_top, p.plate_bottom),
    }
}

/// A complete colour scheme. All FastTracker II-ish: a beige/steel "face" with
/// two-tone bevels for chrome, and a near-black "data" area with bright text
/// for the pattern/table/waveform, plus the six waveform-specific colours.
///
/// This is the flat, widget-facing form. It is *composed* from a [`Skin`]
/// ([`CaseColors`] + [`HardwareColors`]); the split lives in the authoring
/// consts below, not in the widgets, which each take one `&Palette`.
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

    // -- plate fascia (the panel gradient) --
    /// The lit top stop of a fascia plate's vertical gradient.
    pub plate_top: Color32,
    /// The shaded bottom stop of a fascia plate's vertical gradient.
    pub plate_bottom: Color32,
    /// The dark keyline framing a plate.
    pub plate_border: Color32,

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
    /// The face of a latched (pushed-in) toggle. Distinctly darker than
    /// [`Self::button_face`], so "engaged" reads from the shading rather than
    /// from a colour -- these are buttons, not selections, and sharing the
    /// selection accent made a muted channel look like a selected row.
    pub button_pressed: Color32,
    /// Text over [`Self::button_pressed`].
    pub button_pressed_text: Color32,

    // -- pads and deck (the backlit-keycap button chrome) --
    /// How the pad keycaps are coloured, independently of the plate. Resolve to
    /// actual cap colours with [`pad_caps`].
    pub pad: Surface,
    /// How the deck (the control-panel surface the pads sit on) is coloured,
    /// independently of the plate. Resolve with [`deck_stops`].
    pub deck: Surface,
    /// A latched pad's lit cap, top (hot) stop -- fixed hardware amber.
    pub latch_top: Color32,
    /// A latched pad's lit cap, bottom stop -- fixed hardware amber.
    pub latch_bottom: Color32,
    /// A latched pad's border -- fixed hardware amber.
    pub latch_border: Color32,
    /// The ink on a latched (lit) pad -- fixed hardware dark amber.
    pub latch_ink: Color32,

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
    /// The loop-region brackets and their flags.
    pub wf_loop: Color32,
    /// The translucent wash over the looped region. Premultiplied, and kept
    /// faint: it lies over the wave, not behind it.
    pub wf_loop_region: Color32,

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

/// The per-case fascia colours: everything a case colour-scheme may override.
/// Panels, buttons, chrome text and selection -- the "outside of the box".
#[derive(Debug, Clone, Copy)]
pub(crate) struct CaseColors {
    /// Button and panel face.
    pub face: Color32,
    /// Face under a hovering pointer.
    pub face_hover: Color32,
    /// Face while pressed / active / a window's title bar.
    pub face_active: Color32,
    /// The desktop behind the panels.
    pub desktop: Color32,
    /// The lit bevel edge.
    pub bevel_light: Color32,
    /// The shadowed bevel edge; also separators and window outlines.
    pub bevel_dark: Color32,
    /// The near-black keyline framing a raised button's shadow side.
    pub bevel_border: Color32,
    /// The lit top stop of a fascia plate's vertical gradient.
    pub plate_top: Color32,
    /// The shaded bottom stop of a fascia plate's vertical gradient.
    pub plate_bottom: Color32,
    /// The dark keyline framing a plate.
    pub plate_border: Color32,
    /// Text on the face (menus, dialogs, status bar).
    pub label: Color32,
    /// Dimmed text.
    pub muted: Color32,
    /// Bright primary data text (register values, readouts) -- case-tinted so
    /// each palette reads in its own ink rather than one shared yellow.
    pub data_text: Color32,
    /// Secondary data text (descriptions, the empty-state hint) -- case-tinted
    /// even though it sits over a hardware well.
    pub data_label: Color32,
    /// The scope (waveform) screen background.
    pub wf_bg: Color32,
    /// The wave itself.
    pub wf_wave: Color32,
    /// The playback cursor. Case-owned so it keeps reading against the wave.
    pub wf_cursor: Color32,
    /// The loop-region brackets and their flags. Likewise case-owned, so they
    /// stay distinct from both the wave and the cursor.
    pub wf_loop: Color32,
    /// The scrollbar trough.
    pub trough: Color32,
    /// The grey button face.
    pub button_face: Color32,
    /// The button face under a hovering pointer.
    pub button_hover: Color32,
    /// The button face while pressed.
    pub button_active: Color32,
    /// The lit inner bevel of a raised button.
    pub button_light: Color32,
    /// The shadowed inner bevel of a raised button.
    pub button_shadow: Color32,
    /// Text on a button.
    pub button_text: Color32,
    /// The face of a latched (pushed-in) toggle.
    pub button_pressed: Color32,
    /// Text over [`Self::button_pressed`].
    pub button_pressed_text: Color32,
    /// How the pad keycaps are coloured, independently of the plate.
    pub pad: Surface,
    /// How the deck (the control-panel surface) is coloured, independently of
    /// the plate.
    pub deck: Surface,
    /// The selection bar fill.
    pub accent: Color32,
    /// Text drawn over the selection bar.
    pub selection_text: Color32,
}

/// The fixed "display" colours: the dark data wells, the tracker-yellow
/// readouts, the scope and the VU meter. These are the "inside the box"
/// hardware -- they do **not** change as the case colour changes, which is the
/// "case changes, displays don't" rule from the mock-ups made structural.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HardwareColors {
    /// The sunken data background.
    pub data_bg: Color32,
    /// The alternate (striped) data row.
    pub data_stripe: Color32,
    /// A hovered data row.
    pub data_hover: Color32,
    /// The snap-to-instruction hover line.
    pub wf_hover: Color32,
    /// The playback-start line.
    pub wf_start: Color32,
    /// The half-black overlay left of the start line.
    pub wf_dim: Color32,
    /// The translucent wash over the looped region.
    pub wf_loop_region: Color32,
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
    /// A latched pad's lit cap, top (hot) stop.
    pub latch_top: Color32,
    /// A latched pad's lit cap, bottom stop.
    pub latch_bottom: Color32,
    /// A latched pad's border.
    pub latch_border: Color32,
    /// The ink on a latched (lit) pad.
    pub latch_ink: Color32,
}

/// A case paired with the hardware it sits in. [`compose`](Self::compose)s into
/// the flat [`Palette`] the widgets consume.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Skin {
    /// The per-case fascia colours.
    pub case: CaseColors,
    /// The fixed display colours.
    pub hardware: HardwareColors,
}

impl Skin {
    /// Flattens the case + hardware split into the widget-facing [`Palette`].
    #[must_use]
    pub(crate) const fn compose(&self) -> Palette {
        Palette {
            face: self.case.face,
            face_hover: self.case.face_hover,
            face_active: self.case.face_active,
            desktop: self.case.desktop,
            bevel_light: self.case.bevel_light,
            bevel_dark: self.case.bevel_dark,
            bevel_border: self.case.bevel_border,
            plate_top: self.case.plate_top,
            plate_bottom: self.case.plate_bottom,
            plate_border: self.case.plate_border,

            data_bg: self.hardware.data_bg,
            data_stripe: self.hardware.data_stripe,
            data_hover: self.hardware.data_hover,
            data_text: self.case.data_text,
            data_label: self.case.data_label,
            trough: self.case.trough,

            label: self.case.label,
            muted: self.case.muted,

            button_face: self.case.button_face,
            button_hover: self.case.button_hover,
            button_active: self.case.button_active,
            button_light: self.case.button_light,
            button_shadow: self.case.button_shadow,
            button_text: self.case.button_text,
            button_pressed: self.case.button_pressed,
            button_pressed_text: self.case.button_pressed_text,

            pad: self.case.pad,
            deck: self.case.deck,
            latch_top: self.hardware.latch_top,
            latch_bottom: self.hardware.latch_bottom,
            latch_border: self.hardware.latch_border,
            latch_ink: self.hardware.latch_ink,

            accent: self.case.accent,
            selection_text: self.case.selection_text,

            wf_bg: self.case.wf_bg,
            wf_wave: self.case.wf_wave,
            wf_hover: self.hardware.wf_hover,
            wf_start: self.hardware.wf_start,
            wf_cursor: self.case.wf_cursor,
            wf_dim: self.hardware.wf_dim,
            wf_loop: self.case.wf_loop,
            wf_loop_region: self.hardware.wf_loop_region,

            meter_off: self.hardware.meter_off,
            meter_low: self.hardware.meter_low,
            meter_mid: self.hardware.meter_mid,
            meter_high: self.hardware.meter_high,
            meter_hold: self.hardware.meter_hold,
        }
    }
}

// -------------------------------------------------------------------------
// clone-dark: the ft2-clone dark teal scheme (the default).
// -------------------------------------------------------------------------

const CLONE_DARK_CASE: CaseColors = CaseColors {
    face: Color32::from_rgb(0x4A, 0x73, 0x73),
    face_hover: Color32::from_rgb(0x57, 0x82, 0x82),
    face_active: Color32::from_rgb(0x3E, 0x61, 0x61),
    desktop: Color32::from_rgb(0x26, 0x3A, 0x3A),
    bevel_light: Color32::from_rgb(0x8F, 0xBF, 0xBF),
    bevel_dark: Color32::from_rgb(0x1A, 0x29, 0x29),
    bevel_border: Color32::from_rgb(0x07, 0x0D, 0x0D),

    // A teal plate: lit along the top, sinking to a darker teal at the bottom.
    plate_top: Color32::from_rgb(0x58, 0x84, 0x84),
    plate_bottom: Color32::from_rgb(0x38, 0x59, 0x59),
    plate_border: Color32::from_rgb(0x0A, 0x14, 0x14),

    label: Color32::from_rgb(0xDC, 0xEF, 0xEF),
    // Light enough to read as menu shortcut text on the teal face and as the
    // Bank column on the near-black data background.
    muted: Color32::from_rgb(0x86, 0xA6, 0xA6),
    // The classic tracker yellow.
    data_text: Color32::from_rgb(0xF1, 0xE6, 0x7B),
    data_label: Color32::from_rgb(0xB8, 0xD0, 0xD0),

    // The classic scope: yellow wave, cyan cursor, warm orange brackets, on a
    // screen tinted to the teal case.
    wf_bg: Color32::from_rgb(0x08, 0x16, 0x18),
    wf_wave: Color32::from_rgb(0xF1, 0xE6, 0x7B),
    wf_cursor: Color32::from_rgb(0x7C, 0xE0, 0xE0),
    wf_loop: Color32::from_rgb(0xFF, 0x9E, 0x3D),
    trough: Color32::from_rgb(0x18, 0x26, 0x26),

    button_face: Color32::from_rgb(0x9C, 0xA7, 0xA7),
    button_hover: Color32::from_rgb(0xAB, 0xB6, 0xB6),
    button_active: Color32::from_rgb(0x88, 0x93, 0x93),
    button_light: Color32::from_rgb(0xE2, 0xEA, 0xEA),
    button_shadow: Color32::from_rgb(0x53, 0x5F, 0x5F),
    button_text: Color32::BLACK,
    // Between `button_active` and `button_shadow`: clearly sunk, still a button.
    button_pressed: Color32::from_rgb(0x6E, 0x79, 0x79),
    button_pressed_text: Color32::WHITE,

    // Neutral grey keycaps (the classic FT2 button), on the teal plate/deck.
    pad: Surface::Grey,
    deck: Surface::Tint,

    accent: Color32::from_rgb(0x33, 0x55, 0xAA),
    selection_text: Color32::WHITE,
};

const CLONE_DARK_HW: HardwareColors = HardwareColors {
    data_bg: Color32::from_rgb(0x0C, 0x14, 0x14),
    data_stripe: Color32::from_rgb(0x14, 0x20, 0x20),
    data_hover: Color32::from_rgb(0x1B, 0x2C, 0x2C),

    wf_hover: Color32::from_rgb(0xAA, 0xCC, 0xCC),
    wf_start: Color32::WHITE,
    wf_dim: Color32::from_rgba_premultiplied(0, 0, 0, 0x7F),
    wf_loop_region: Color32::from_rgba_premultiplied(0x24, 0x14, 0x05, 0x24),

    meter_off: Color32::from_rgb(0x1C, 0x28, 0x3C),
    meter_low: Color32::from_rgb(0x3C, 0xC8, 0x50),
    meter_mid: Color32::from_rgb(0xE6, 0xC8, 0x46),
    meter_high: Color32::from_rgb(0xE0, 0x4A, 0x4A),
    meter_hold: Color32::from_rgb(0xEA, 0xF4, 0xF4),

    // Warm amber "lamp behind the key" for a latched pad (shared hardware).
    latch_top: Color32::from_rgb(0xFB, 0xE3, 0x8C),
    latch_bottom: Color32::from_rgb(0xDD, 0xB0, 0x47),
    latch_border: Color32::from_rgb(0x8A, 0x6D, 0x28),
    latch_ink: Color32::from_rgb(0x3F, 0x2E, 0x08),
};

/// The ft2-clone dark teal scheme (the default).
pub(crate) const CLONE_DARK: Palette = Skin {
    case: CLONE_DARK_CASE,
    hardware: CLONE_DARK_HW,
}
.compose();

// -------------------------------------------------------------------------
// ft2-classic: the original DOS FastTracker II steel-blue scheme.
// -------------------------------------------------------------------------

const FT2_CLASSIC_CASE: CaseColors = CaseColors {
    face: Color32::from_rgb(0x6E, 0x82, 0xA0),
    face_hover: Color32::from_rgb(0x7B, 0x90, 0xB0),
    face_active: Color32::from_rgb(0x5F, 0x73, 0x90),
    desktop: Color32::from_rgb(0x30, 0x3E, 0x58),
    bevel_light: Color32::from_rgb(0xC6, 0xD2, 0xE4),
    bevel_dark: Color32::from_rgb(0x2E, 0x3A, 0x4C),
    bevel_border: Color32::from_rgb(0x0A, 0x0D, 0x14),

    // A steel-blue plate.
    plate_top: Color32::from_rgb(0x80, 0x94, 0xB2),
    plate_bottom: Color32::from_rgb(0x54, 0x67, 0x83),
    plate_border: Color32::from_rgb(0x0A, 0x0D, 0x14),

    label: Color32::BLACK,
    muted: Color32::from_rgb(0x4A, 0x5A, 0x74),
    data_text: Color32::from_rgb(0xEF, 0xE2, 0x7A),
    data_label: Color32::from_rgb(0xC8, 0xD4, 0xE8),

    // The classic FT2 scope: pale blue wave, yellow cursor.
    wf_bg: Color32::from_rgb(0x0A, 0x10, 0x24),
    wf_wave: Color32::from_rgb(0xC8, 0xD8, 0xF0),
    wf_cursor: Color32::from_rgb(0xFF, 0xFF, 0x00),
    wf_loop: Color32::from_rgb(0xFF, 0x7A, 0x45),
    trough: Color32::from_rgb(0x23, 0x2D, 0x42),

    button_face: Color32::from_rgb(0xAC, 0xB1, 0xB8),
    button_hover: Color32::from_rgb(0xBA, 0xBF, 0xC6),
    button_active: Color32::from_rgb(0x99, 0x9E, 0xA6),
    button_light: Color32::from_rgb(0xEC, 0xEF, 0xF4),
    button_shadow: Color32::from_rgb(0x58, 0x5E, 0x68),
    button_text: Color32::BLACK,
    // Between `button_active` and `button_shadow`: clearly sunk, still a button.
    button_pressed: Color32::from_rgb(0x78, 0x7E, 0x87),
    button_pressed_text: Color32::WHITE,

    // Neutral grey keycaps (the classic FT2 button), on the steel plate/deck.
    pad: Surface::Grey,
    deck: Surface::Tint,

    accent: Color32::from_rgb(0x40, 0x56, 0xA0),
    selection_text: Color32::WHITE,
};

const FT2_CLASSIC_HW: HardwareColors = HardwareColors {
    data_bg: Color32::from_rgb(0x1A, 0x22, 0x38),
    data_stripe: Color32::from_rgb(0x22, 0x2C, 0x46),
    data_hover: Color32::from_rgb(0x2A, 0x36, 0x54),

    wf_hover: Color32::from_rgb(0xAA, 0xCC, 0xCC),
    wf_start: Color32::WHITE,
    wf_dim: Color32::from_rgba_premultiplied(0, 0, 0, 0x7F),
    wf_loop_region: Color32::from_rgba_premultiplied(0x24, 0x10, 0x06, 0x24),

    // Steel-tinted takes on the classic green/amber/red zones.
    meter_off: Color32::from_rgb(0x26, 0x30, 0x4A),
    meter_low: Color32::from_rgb(0x55, 0xC8, 0x6E),
    meter_mid: Color32::from_rgb(0xE0, 0xC8, 0x55),
    meter_high: Color32::from_rgb(0xDC, 0x55, 0x55),
    meter_hold: Color32::from_rgb(0xF0, 0xF2, 0xF8),

    // Warm amber "lamp behind the key" for a latched pad (shared hardware).
    latch_top: Color32::from_rgb(0xFB, 0xE3, 0x8C),
    latch_bottom: Color32::from_rgb(0xDD, 0xB0, 0x47),
    latch_border: Color32::from_rgb(0x8A, 0x6D, 0x28),
    latch_ink: Color32::from_rgb(0x3F, 0x2E, 0x08),
};

/// The original DOS FastTracker II steel-blue scheme.
pub(crate) const FT2_CLASSIC: Palette = Skin {
    case: FT2_CLASSIC_CASE,
    hardware: FT2_CLASSIC_HW,
}
.compose();

// -------------------------------------------------------------------------
// The Bassoon "Variation 2" cases. These share ONE hardware table -- the
// "case changes, displays don't" rule -- and differ only in the fascia plate,
// the keycaps and the chrome ink.
// -------------------------------------------------------------------------

/// The shared display hardware for the Bassoon cases: tracker-yellow readouts,
/// the amber latch lamp, a green/amber/red VU and a cool scope.
const HARDWARE: HardwareColors = HardwareColors {
    data_bg: Color32::from_rgb(0x0C, 0x14, 0x14),
    data_stripe: Color32::from_rgb(0x14, 0x20, 0x20),
    data_hover: Color32::from_rgb(0x1B, 0x2C, 0x2C),

    wf_hover: Color32::from_rgb(0xAA, 0xCC, 0xCC),
    wf_start: Color32::WHITE,
    wf_dim: Color32::from_rgba_premultiplied(0, 0, 0, 0x7F),
    wf_loop_region: Color32::from_rgba_premultiplied(0x24, 0x14, 0x05, 0x24),

    meter_off: Color32::from_rgb(0x1C, 0x28, 0x3C),
    meter_low: Color32::from_rgb(0x3C, 0xC8, 0x50),
    meter_mid: Color32::from_rgb(0xE6, 0xC8, 0x46),
    meter_high: Color32::from_rgb(0xE0, 0x4A, 0x4A),
    meter_hold: Color32::from_rgb(0xEA, 0xF4, 0xF4),

    latch_top: Color32::from_rgb(0xFB, 0xE3, 0x8C),
    latch_bottom: Color32::from_rgb(0xDD, 0xB0, 0x47),
    latch_border: Color32::from_rgb(0x8A, 0x6D, 0x28),
    latch_ink: Color32::from_rgb(0x3F, 0x2E, 0x08),
};

/// Navy: a dark blue plate with light silkscreen and mid-blue keycaps.
const NAVY_CASE: CaseColors = CaseColors {
    face: Color32::from_rgb(0x34, 0x48, 0x6A),
    face_hover: Color32::from_rgb(0x3E, 0x54, 0x78),
    face_active: Color32::from_rgb(0x2A, 0x3C, 0x5C),
    desktop: Color32::from_rgb(0x17, 0x1F, 0x30),
    bevel_light: Color32::from_rgb(0x5E, 0x76, 0xA0),
    bevel_dark: Color32::from_rgb(0x14, 0x20, 0x3A),
    bevel_border: Color32::from_rgb(0x0A, 0x0F, 0x1A),

    plate_top: Color32::from_rgb(0x3E, 0x52, 0x73),
    plate_bottom: Color32::from_rgb(0x28, 0x3A, 0x57),
    plate_border: Color32::from_rgb(0x10, 0x19, 0x2A),

    label: Color32::from_rgb(0xD6, 0xE2, 0xF5),
    muted: Color32::from_rgb(0x7E, 0x8F, 0xB0),
    // Ice blue, to sit with the navy plate.
    data_text: Color32::from_rgb(0xAE, 0xCD, 0xF5),
    data_label: Color32::from_rgb(0xB8, 0xC6, 0xE0),

    // Ice-blue wave on a deep navy screen; warm cursor and brackets against it.
    wf_bg: Color32::from_rgb(0x08, 0x10, 0x22),
    wf_wave: Color32::from_rgb(0xAE, 0xCD, 0xF5),
    wf_cursor: Color32::from_rgb(0xFF, 0xD3, 0x5E),
    wf_loop: Color32::from_rgb(0xFF, 0x6B, 0x4C),
    trough: Color32::from_rgb(0x1A, 0x24, 0x40),

    button_face: Color32::from_rgb(0x8F, 0xA2, 0xC2),
    button_hover: Color32::from_rgb(0x9D, 0xB0, 0xD0),
    button_active: Color32::from_rgb(0x7E, 0x93, 0xB4),
    button_light: Color32::from_rgb(0xC2, 0xD2, 0xEC),
    button_shadow: Color32::from_rgb(0x4A, 0x5A, 0x7A),
    button_text: Color32::from_rgb(0x14, 0x1E, 0x30),
    button_pressed: Color32::from_rgb(0x5A, 0x6E, 0x92),
    button_pressed_text: Color32::WHITE,

    // Pale keys on a dark rubber deck, over the navy plate.
    pad: Surface::Light,
    deck: Surface::Dark,

    accent: Color32::from_rgb(0x33, 0x55, 0xAA),
    selection_text: Color32::WHITE,
};

/// The navy Bassoon case.
pub(crate) const NAVY: Palette = Skin {
    case: NAVY_CASE,
    hardware: HARDWARE,
}
.compose();

/// Cream: a light plate. The silkscreen flips to dark ink and the keycaps go
/// tone-on-tone, so the amber latch and the dark wells still carry the eye.
const CREAM_CASE: CaseColors = CaseColors {
    face: Color32::from_rgb(0xE8, 0xDF, 0xC6),
    face_hover: Color32::from_rgb(0xF0, 0xE8, 0xD3),
    face_active: Color32::from_rgb(0xDC, 0xD2, 0xB8),
    desktop: Color32::from_rgb(0x39, 0x35, 0x2A),
    bevel_light: Color32::from_rgb(0xFF, 0xFB, 0xF0),
    bevel_dark: Color32::from_rgb(0xA8, 0x9E, 0x82),
    bevel_border: Color32::from_rgb(0x6E, 0x66, 0x4C),

    plate_top: Color32::from_rgb(0xF0, 0xE8, 0xD3),
    plate_bottom: Color32::from_rgb(0xDB, 0xD1, 0xB5),
    plate_border: Color32::from_rgb(0x7C, 0x74, 0x58),

    // The emboss flip: dark ink on the light plate.
    label: Color32::from_rgb(0x38, 0x35, 0x2A),
    muted: Color32::from_rgb(0x8A, 0x82, 0x66),
    // Light: it still sits on the dark data well.
    // Warm amber, matching the cream plate.
    data_text: Color32::from_rgb(0xEB, 0xCF, 0x95),
    data_label: Color32::from_rgb(0xC8, 0xBE, 0x9E),

    // Warm amber wave on a dark brown screen; cool cursor to cut through it.
    wf_bg: Color32::from_rgb(0x17, 0x13, 0x10),
    wf_wave: Color32::from_rgb(0xEB, 0xCF, 0x95),
    wf_cursor: Color32::from_rgb(0x7C, 0xD8, 0xE0),
    wf_loop: Color32::from_rgb(0xE8, 0x5D, 0x5D),
    trough: Color32::from_rgb(0xBE, 0xB4, 0x98),

    button_face: Color32::from_rgb(0xD8, 0xCD, 0xB0),
    button_hover: Color32::from_rgb(0xE2, 0xD7, 0xBB),
    button_active: Color32::from_rgb(0xCA, 0xBF, 0xA2),
    button_light: Color32::from_rgb(0xF4, 0xEE, 0xDC),
    button_shadow: Color32::from_rgb(0x8A, 0x82, 0x66),
    button_text: Color32::from_rgb(0x33, 0x30, 0x1F),
    button_pressed: Color32::from_rgb(0xB8, 0xAE, 0x90),
    button_pressed_text: Color32::from_rgb(0x33, 0x30, 0x1F),

    // Cream keys and deck, tone-on-tone with the light plate.
    pad: Surface::Tint,
    deck: Surface::Tint,

    accent: Color32::from_rgb(0x40, 0x56, 0xA0),
    selection_text: Color32::WHITE,
};

/// The cream Bassoon case.
pub(crate) const CREAM: Palette = Skin {
    case: CREAM_CASE,
    hardware: HARDWARE,
}
.compose();

/// Verdigris: a patinated-copper metal plate with light silkscreen and
/// patina-green keycaps.
const VERDIGRIS_CASE: CaseColors = CaseColors {
    face: Color32::from_rgb(0x3E, 0x69, 0x59),
    face_hover: Color32::from_rgb(0x4A, 0x77, 0x68),
    face_active: Color32::from_rgb(0x34, 0x5A, 0x4C),
    desktop: Color32::from_rgb(0x1C, 0x2A, 0x24),
    bevel_light: Color32::from_rgb(0x6F, 0xA0, 0x89),
    bevel_dark: Color32::from_rgb(0x1E, 0x36, 0x2C),
    bevel_border: Color32::from_rgb(0x0E, 0x1A, 0x14),

    plate_top: Color32::from_rgb(0x4C, 0x7C, 0x6B),
    plate_bottom: Color32::from_rgb(0x35, 0x59, 0x4A),
    plate_border: Color32::from_rgb(0x15, 0x2A, 0x22),

    label: Color32::from_rgb(0xE2, 0xED, 0xE2),
    muted: Color32::from_rgb(0x8A, 0xA8, 0x98),
    // Pale mint, matching the patina plate.
    data_text: Color32::from_rgb(0xA6, 0xE2, 0xC6),
    data_label: Color32::from_rgb(0xB8, 0xD0, 0xC2),

    // Mint wave on a dark patina screen.
    wf_bg: Color32::from_rgb(0x08, 0x1A, 0x14),
    wf_wave: Color32::from_rgb(0xA6, 0xE2, 0xC6),
    wf_cursor: Color32::from_rgb(0xFF, 0xD3, 0x5E),
    wf_loop: Color32::from_rgb(0xE8, 0x72, 0x4C),
    trough: Color32::from_rgb(0x1E, 0x36, 0x2C),

    button_face: Color32::from_rgb(0x9A, 0xBB, 0xA8),
    button_hover: Color32::from_rgb(0xA8, 0xC8, 0xB6),
    button_active: Color32::from_rgb(0x88, 0xAC, 0x97),
    button_light: Color32::from_rgb(0xC6, 0xDE, 0xD0),
    button_shadow: Color32::from_rgb(0x4E, 0x6E, 0x5E),
    button_text: Color32::from_rgb(0x16, 0x28, 0x1F),
    button_pressed: Color32::from_rgb(0x60, 0x82, 0x72),
    button_pressed_text: Color32::WHITE,

    // Patina keys and deck, tone-on-tone with the plate.
    pad: Surface::Tint,
    deck: Surface::Tint,

    accent: Color32::from_rgb(0xB5, 0x84, 0x3C),
    selection_text: Color32::from_rgb(0x16, 0x28, 0x1F),
};

/// The verdigris Bassoon case.
pub(crate) const VERDIGRIS: Palette = Skin {
    case: VERDIGRIS_CASE,
    hardware: HARDWARE,
}
.compose();

/// A dark-plate Bassoon case: a tone-on-tone tinted skin (pad + deck follow the
/// plate), light silkscreen, a hue-tinted knob-cap ramp. Only the eight anchor
/// colours differ between these cases; the rest are their light/dark relatives.
macro_rules! dark_plate_case {
    (
        plate: ($pt:expr, $pb:expr, $pbd:expr),
        face: ($f:expr, $fh:expr, $fa:expr),
        desktop: $desk:expr,
        bevel: ($bl:expr, $bd:expr, $bb:expr),
        label: $lab:expr,
        muted: $mut:expr,
        ink: $ink:expr,
        data_label: $dl:expr,
        scope: ($wbg:expr, $wave:expr, $wcur:expr, $wloop:expr),
        trough: $tr:expr,
        knob: ($kf:expr, $kh:expr, $ka:expr, $kl:expr, $ks:expr, $kt:expr, $kp:expr),
    ) => {
        CaseColors {
            face: $f,
            face_hover: $fh,
            face_active: $fa,
            desktop: $desk,
            bevel_light: $bl,
            bevel_dark: $bd,
            bevel_border: $bb,
            plate_top: $pt,
            plate_bottom: $pb,
            plate_border: $pbd,
            label: $lab,
            muted: $mut,
            data_text: $ink,
            data_label: $dl,
            wf_bg: $wbg,
            wf_wave: $wave,
            wf_cursor: $wcur,
            wf_loop: $wloop,
            trough: $tr,
            button_face: $kf,
            button_hover: $kh,
            button_active: $ka,
            button_light: $kl,
            button_shadow: $ks,
            button_text: $kt,
            button_pressed: $kp,
            button_pressed_text: Color32::WHITE,
            pad: Surface::Tint,
            deck: Surface::Tint,
            accent: Color32::from_rgb(0x33, 0x55, 0xAA),
            selection_text: Color32::WHITE,
        }
    };
}

/// Composes a dark-plate case over the shared [`HARDWARE`].
macro_rules! bassoon {
    ($case:expr) => {
        Skin {
            case: $case,
            hardware: HARDWARE,
        }
        .compose()
    };
}

const MOSS_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x4E, 0x6B, 0x44), rgb(0x33, 0x4A, 0x2C), rgb(0x12, 0x1E, 0x0E)),
    face: (rgb(0x3E, 0x56, 0x36), rgb(0x48, 0x62, 0x40), rgb(0x34, 0x4A, 0x2E)),
    desktop: rgb(0x14, 0x1E, 0x10),
    bevel: (rgb(0x6C, 0x8E, 0x5E), rgb(0x18, 0x28, 0x12), rgb(0x0A, 0x12, 0x08)),
    label: rgb(0xDC, 0xEB, 0xD0),
    muted: rgb(0x88, 0xA4, 0x7C),
    ink: rgb(0xCB, 0xE4, 0x9C),
    data_label: rgb(0xBE, 0xD4, 0xB2),
    scope: (rgb(0x0B, 0x14, 0x08), rgb(0xCB, 0xE4, 0x9C), rgb(0x8F, 0xD0, 0xE8), rgb(0xFF, 0x9E, 0x3D)),
    trough: rgb(0x18, 0x26, 0x12),
    knob: (rgb(0x9A, 0xB0, 0x8C), rgb(0xA8, 0xBE, 0x9A), rgb(0x88, 0x9E, 0x7C), rgb(0xC6, 0xD8, 0xBA), rgb(0x50, 0x62, 0x44), rgb(0x16, 0x22, 0x10), rgb(0x60, 0x76, 0x52)),
};
/// The moss Bassoon case.
pub(crate) const MOSS: Palette = bassoon!(MOSS_CASE);

const PLUM_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x5E, 0x46, 0x66), rgb(0x3E, 0x2E, 0x45), rgb(0x1C, 0x14, 0x20)),
    face: (rgb(0x4C, 0x38, 0x54), rgb(0x56, 0x42, 0x5E), rgb(0x40, 0x30, 0x48)),
    desktop: rgb(0x1A, 0x12, 0x1E),
    bevel: (rgb(0x86, 0x6C, 0x90), rgb(0x24, 0x1A, 0x2A), rgb(0x10, 0x0A, 0x14)),
    label: rgb(0xE6, 0xDC, 0xEC),
    muted: rgb(0x9C, 0x86, 0xA6),
    ink: rgb(0xD9, 0xBE, 0xEC),
    data_label: rgb(0xCC, 0xBC, 0xD6),
    scope: (rgb(0x14, 0x0C, 0x18), rgb(0xD9, 0xBE, 0xEC), rgb(0xFF, 0xD3, 0x5E), rgb(0x6F, 0xE0, 0xB0)),
    trough: rgb(0x22, 0x18, 0x28),
    knob: (rgb(0xAC, 0x9A, 0xB4), rgb(0xB8, 0xA8, 0xC0), rgb(0x9A, 0x88, 0xA2), rgb(0xD4, 0xC6, 0xDC), rgb(0x5E, 0x50, 0x66), rgb(0x1E, 0x16, 0x22), rgb(0x72, 0x60, 0x7C)),
};
/// The plum Bassoon case.
pub(crate) const PLUM: Palette = bassoon!(PLUM_CASE);

const RUST_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x7A, 0x4E, 0x36), rgb(0x55, 0x34, 0x1F), rgb(0x26, 0x16, 0x0C)),
    face: (rgb(0x64, 0x40, 0x2C), rgb(0x70, 0x4A, 0x34), rgb(0x54, 0x36, 0x24)),
    desktop: rgb(0x22, 0x16, 0x0E),
    bevel: (rgb(0xA0, 0x74, 0x56), rgb(0x30, 0x1E, 0x12), rgb(0x14, 0x0C, 0x06)),
    label: rgb(0xF0, 0xE0, 0xD2),
    muted: rgb(0xB0, 0x8E, 0x78),
    ink: rgb(0xF0, 0xC2, 0x89),
    data_label: rgb(0xDC, 0xC2, 0xB0),
    scope: (rgb(0x16, 0x0C, 0x06), rgb(0xF0, 0xC2, 0x89), rgb(0x7C, 0xD8, 0xE0), rgb(0xC0, 0x8C, 0xE8)),
    trough: rgb(0x2C, 0x1C, 0x12),
    knob: (rgb(0xC2, 0xA0, 0x88), rgb(0xCE, 0xAE, 0x98), rgb(0xB0, 0x90, 0x78), rgb(0xE2, 0xCC, 0xBA), rgb(0x74, 0x56, 0x42), rgb(0x24, 0x16, 0x0E), rgb(0x8A, 0x66, 0x4E)),
};
/// The rust Bassoon case.
pub(crate) const RUST: Palette = bassoon!(RUST_CASE);

const PETROL_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x2E, 0x5A, 0x5E), rgb(0x1C, 0x3C, 0x40), rgb(0x0A, 0x1A, 0x1C)),
    face: (rgb(0x28, 0x4C, 0x50), rgb(0x30, 0x56, 0x5A), rgb(0x20, 0x40, 0x44)),
    desktop: rgb(0x10, 0x1E, 0x20),
    bevel: (rgb(0x50, 0x82, 0x86), rgb(0x12, 0x28, 0x2A), rgb(0x08, 0x12, 0x14)),
    label: rgb(0xD4, 0xEC, 0xEC),
    muted: rgb(0x76, 0xA2, 0xA4),
    ink: rgb(0x8F, 0xD9, 0xDD),
    data_label: rgb(0xB2, 0xD4, 0xD4),
    scope: (rgb(0x06, 0x14, 0x16), rgb(0x8F, 0xD9, 0xDD), rgb(0xFF, 0xD3, 0x5E), rgb(0xFF, 0x6B, 0x6B)),
    trough: rgb(0x14, 0x28, 0x2A),
    knob: (rgb(0x86, 0xAC, 0xAE), rgb(0x96, 0xBA, 0xBC), rgb(0x76, 0x9E, 0xA0), rgb(0xBC, 0xD8, 0xD8), rgb(0x44, 0x64, 0x66), rgb(0x0E, 0x20, 0x22), rgb(0x54, 0x78, 0x7A)),
};
/// The petrol Bassoon case.
pub(crate) const PETROL: Palette = bassoon!(PETROL_CASE);

const SLATE_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x4A, 0x54, 0x64), rgb(0x31, 0x39, 0x47), rgb(0x14, 0x18, 0x20)),
    face: (rgb(0x3E, 0x46, 0x54), rgb(0x48, 0x52, 0x60), rgb(0x34, 0x3C, 0x48)),
    desktop: rgb(0x16, 0x1A, 0x20),
    bevel: (rgb(0x74, 0x80, 0x92), rgb(0x1E, 0x24, 0x2E), rgb(0x0C, 0x10, 0x14)),
    label: rgb(0xDE, 0xE4, 0xEC),
    muted: rgb(0x8C, 0x96, 0xA4),
    ink: rgb(0xC8, 0xD6, 0xEA),
    data_label: rgb(0xC2, 0xCA, 0xD4),
    scope: (rgb(0x0B, 0x0F, 0x16), rgb(0xC8, 0xD6, 0xEA), rgb(0xFF, 0xD3, 0x5E), rgb(0x6F, 0xE0, 0xA8)),
    trough: rgb(0x1E, 0x24, 0x2C),
    knob: (rgb(0xA2, 0xAA, 0xB6), rgb(0xB0, 0xB8, 0xC2), rgb(0x90, 0x98, 0xA4), rgb(0xCE, 0xD4, 0xDC), rgb(0x56, 0x5E, 0x68), rgb(0x18, 0x1C, 0x22), rgb(0x68, 0x70, 0x7C)),
};
/// The slate Bassoon case.
pub(crate) const SLATE: Palette = bassoon!(SLATE_CASE);

const OLIVE_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x5E, 0x5E, 0x38), rgb(0x3F, 0x3F, 0x20), rgb(0x1A, 0x1A, 0x0C)),
    face: (rgb(0x4E, 0x4E, 0x2E), rgb(0x58, 0x58, 0x38), rgb(0x42, 0x42, 0x26)),
    desktop: rgb(0x1A, 0x1A, 0x0E),
    bevel: (rgb(0x88, 0x88, 0x5A), rgb(0x24, 0x24, 0x12), rgb(0x10, 0x10, 0x08)),
    label: rgb(0xEC, 0xEC, 0xD2),
    muted: rgb(0xA4, 0xA4, 0x78),
    ink: rgb(0xE2, 0xDE, 0x9A),
    data_label: rgb(0xD4, 0xD4, 0xB0),
    scope: (rgb(0x11, 0x11, 0x0A), rgb(0xE2, 0xDE, 0x9A), rgb(0x8F, 0xD0, 0xE8), rgb(0xE8, 0x72, 0x4C)),
    trough: rgb(0x24, 0x24, 0x12),
    knob: (rgb(0xB2, 0xB2, 0x88), rgb(0xC0, 0xC0, 0x96), rgb(0xA0, 0xA0, 0x78), rgb(0xD8, 0xD8, 0xBA), rgb(0x62, 0x62, 0x44), rgb(0x22, 0x22, 0x10), rgb(0x78, 0x78, 0x52)),
};
/// The olive Bassoon case.
pub(crate) const OLIVE: Palette = bassoon!(OLIVE_CASE);

const WINE_CASE: CaseColors = dark_plate_case! {
    plate: (rgb(0x66, 0x38, 0x4A), rgb(0x43, 0x22, 0x2F), rgb(0x1E, 0x0E, 0x14)),
    face: (rgb(0x54, 0x2E, 0x3E), rgb(0x5E, 0x38, 0x48), rgb(0x48, 0x26, 0x34)),
    desktop: rgb(0x1E, 0x10, 0x16),
    bevel: (rgb(0x90, 0x64, 0x74), rgb(0x28, 0x16, 0x1E), rgb(0x12, 0x08, 0x0C)),
    label: rgb(0xEE, 0xDA, 0xE0),
    muted: rgb(0xAA, 0x82, 0x90),
    ink: rgb(0xEE, 0xB8, 0xC6),
    data_label: rgb(0xD8, 0xBC, 0xC4),
    scope: (rgb(0x16, 0x0A, 0x10), rgb(0xEE, 0xB8, 0xC6), rgb(0x8F, 0xD0, 0xE8), rgb(0xE8, 0xC2, 0x4C)),
    trough: rgb(0x28, 0x16, 0x1E),
    knob: (rgb(0xBC, 0x98, 0xA4), rgb(0xC8, 0xA6, 0xB0), rgb(0xAA, 0x88, 0x94), rgb(0xDC, 0xC4, 0xCA), rgb(0x6E, 0x50, 0x5A), rgb(0x24, 0x12, 0x18), rgb(0x84, 0x60, 0x6C)),
};
/// The wine Bassoon case.
pub(crate) const WINE: Palette = bassoon!(WINE_CASE);

/// The palette for a configured theme.
#[must_use]
pub fn palette(choice: ThemeChoice) -> &'static Palette {
    match choice {
        ThemeChoice::CloneDark => &CLONE_DARK,
        ThemeChoice::Ft2Classic => &FT2_CLASSIC,
        ThemeChoice::Navy => &NAVY,
        ThemeChoice::Cream => &CREAM,
        ThemeChoice::Verdigris => &VERDIGRIS,
        ThemeChoice::Moss => &MOSS,
        ThemeChoice::Plum => &PLUM,
        ThemeChoice::Rust => &RUST,
        ThemeChoice::Petrol => &PETROL,
        ThemeChoice::Slate => &SLATE,
        ThemeChoice::Olive => &OLIVE,
        ThemeChoice::Wine => &WINE,
    }
}
