//! The button chrome: backlit "pads" (rounded keycaps) plus the sunken two-tone
//! bevel still used for wells (the waveform, data fields, the scrollbar channel).
//!
//! A pad is a rounded cap with a 1px border, a lit inset line along its top edge
//! and a shaded one along its bottom, inked with a dark glyph or label. Held
//! pushes it in (the top line flips to a shadow and the content nudges down); a
//! latched toggle lights its cap warm amber. Every effect paints **inside** the
//! widget rect -- no outer glow or drop shadow -- so tight groups (the channel
//! digits, the steppers) never clip a neighbour's halo.

use egui::{
    Color32, CornerRadius, Painter, Rangef, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
    vec2,
};

use super::icon::{self, Icon};
use super::paint::{darken, lerp_color, lighten};
use super::palette::Palette;

/// The pad corner radius, in points.
const RADIUS: u8 = 3;
/// The icon stroke weight at the 16px pad size.
const ICON_STROKE: f32 = 1.5;

/// Which way the surface catches the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bevel {
    /// Lit top-left, shadowed bottom-right (a panel).
    Raised,
    /// Shadowed top-left, lit bottom-right (a sunken well).
    Sunken,
}

/// Paints the two-tone bevel just inside `rect`'s edges. Uses `hline`/`vline`,
/// which land on the pixel grid (line rounding is on by default), so the edges
/// stay crisp with feathering on.
pub fn paint_bevel(painter: &Painter, rect: Rect, palette: &Palette, bevel: Bevel) {
    let (lit, shadow) = match bevel {
        Bevel::Raised => (palette.bevel_light, palette.bevel_dark),
        Bevel::Sunken => (palette.bevel_dark, palette.bevel_light),
    };
    // Top and left catch the light...
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0_f32, lit));
    painter.vline(rect.left() + 0.5, rect.y_range(), Stroke::new(1.0_f32, lit));
    // ...bottom and right fall into shadow.
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, shadow),
    );
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(1.0_f32, shadow),
    );
}

/// A horizontal 2px groove between two raised surfaces: a shadow line with a
/// highlight line just below it, both on the pixel grid.
pub fn groove_h(painter: &Painter, x_range: Rangef, y: f32, palette: &Palette) {
    painter.hline(x_range, y + 0.5, Stroke::new(1.0_f32, palette.bevel_dark));
    painter.hline(x_range, y + 1.5, Stroke::new(1.0_f32, palette.bevel_light));
}

/// A vertical 2px groove: a shadow line with a highlight line just to its right.
pub fn groove_v(painter: &Painter, x: f32, y_range: Rangef, palette: &Palette) {
    painter.vline(x + 0.5, y_range, Stroke::new(1.0_f32, palette.bevel_dark));
    painter.vline(x + 1.5, y_range, Stroke::new(1.0_f32, palette.bevel_light));
}

// -- pads ------------------------------------------------------------------

/// One pad's visual state for a frame. `latched` (an engage toggle that is on)
/// lights the cap amber; `muted` (a mute toggle that is off) recesses it into a
/// dim dark cap. The two never apply at once.
#[derive(Debug, Clone, Copy, Default)]
struct PadState {
    hovered: bool,
    held: bool,
    latched: bool,
    muted: bool,
}

/// The paint result of a pad: the ink for its content and a downward nudge to
/// apply while it is held.
struct PadInk {
    color: Color32,
    offset: Vec2,
}

/// Paints one pad into `rect` and returns its content ink + press nudge. An idle
/// cap reads as a slightly domed keycap (lit top line, shaded bottom line);
/// `held` presses it in, `latched` lights the cap amber, and `muted` recesses it
/// into a dim dark cap (an off channel).
fn paint_pad(painter: &Painter, rect: Rect, p: &Palette, state: PadState) -> PadInk {
    let radius = CornerRadius::same(RADIUS);
    let idle_mid = lerp_color(p.pad_cap_top, p.pad_cap_bottom, 0.5);
    let (mut mid, border, ink) = if state.latched {
        (
            lerp_color(p.latch_top, p.latch_bottom, 0.5),
            p.latch_border,
            p.latch_ink,
        )
    } else if state.muted {
        // Recessed and dim: the channel is off. Ink lifts back off the dark cap
        // so the digit stays legible.
        let cap = darken(idle_mid, 0.34);
        (cap, p.pad_border, lighten(cap, 0.5))
    } else {
        (idle_mid, p.pad_border, p.pad_ink)
    };
    if state.hovered && !state.held && !state.muted {
        mid = lighten(mid, 0.07);
    }
    if state.held {
        mid = darken(mid, 0.10);
    }
    painter.rect_filled(rect, radius, mid);

    // Inset dimensionality, kept clear of the rounded corners. A pressed or muted
    // pad reads as pushed in: a shadow along the top instead of a highlight.
    let inset = Rangef::new(
        rect.left() + f32::from(RADIUS),
        rect.right() - f32::from(RADIUS),
    );
    if state.held || state.muted {
        let depth = if state.muted { 95 } else { 70 };
        painter.hline(
            inset,
            rect.top() + 1.5,
            Stroke::new(1.0, Color32::from_black_alpha(depth)),
        );
    } else {
        let glint = if state.latched { 128 } else { 140 };
        painter.hline(
            inset,
            rect.top() + 1.5,
            Stroke::new(1.0, Color32::from_white_alpha(glint)),
        );
        painter.hline(
            inset,
            rect.bottom() - 1.5,
            Stroke::new(1.0, Color32::from_black_alpha(30)),
        );
    }
    painter.rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);

    PadInk {
        color: ink,
        offset: if state.held {
            vec2(0.0, 1.0)
        } else {
            Vec2::ZERO
        },
    }
}

/// The default square pad footprint (a `.sq` cap), the row height on a side.
fn square(ui: &Ui) -> Vec2 {
    Vec2::splat(ui.spacing().interact_size.y)
}

// -- text pads (push buttons) ----------------------------------------------

/// A pad push-button sized to its label: raised at rest, pressed while held.
pub fn button(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    button_impl(ui, palette, text, None)
}

/// As [`button`] but allocated at exactly `size`.
pub fn button_sized(ui: &mut Ui, palette: &Palette, text: &str, size: Vec2) -> Response {
    button_impl(ui, palette, text, Some(size))
}

/// Lays `text` out as a button galley (the "recolour me later" sentinel, so the
/// pad ink is applied at paint time).
fn button_galley(ui: &mut Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER))
}

/// A content-sized pad: the galley plus button padding, floored to the interact
/// size so buttons, toggles and combos all settle at the same row height.
fn content_size(ui: &Ui, galley: &egui::Galley) -> Vec2 {
    let padding = ui.spacing().button_padding;
    let min = ui.spacing().interact_size;
    (galley.size() + padding * 2.0).max(min)
}

fn button_impl(ui: &mut Ui, palette: &Palette, text: &str, fixed: Option<Vec2>) -> Response {
    let galley = button_galley(ui, text);
    let desired = fixed.unwrap_or_else(|| content_size(ui, &galley));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));

    if ui.is_rect_visible(rect) {
        let held = response.is_pointer_button_down_on();
        let state = PadState {
            hovered: response.hovered(),
            held,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        let text_pos = rect.center() - galley.size() * 0.5 + ink.offset;
        ui.painter().galley(text_pos, galley, ink.color);
    }
    response
}

// -- icon pads -------------------------------------------------------------

/// A square icon push-button. `label` is the accessible name (the button draws
/// only the glyph, so the label carries its meaning to tooltips and tests).
pub fn icon_button(ui: &mut Ui, palette: &Palette, glyph: Icon, label: &str) -> Response {
    icon_button_sized(ui, palette, glyph, label, square(ui))
}

/// As [`icon_button`] but allocated at exactly `size` (e.g. a stepper arrow).
pub fn icon_button_sized(
    ui: &mut Ui,
    palette: &Palette,
    glyph: Icon,
    label: &str,
    size: Vec2,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let held = response.is_pointer_button_down_on();
        let state = PadState {
            hovered: response.hovered(),
            held,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        paint_icon(ui.painter(), glyph, rect, ink);
    }
    response
}

/// Centres a 16px glyph in `rect`, nudged with the pad's press offset.
fn paint_icon(painter: &Painter, glyph: Icon, rect: Rect, ink: PadInk) {
    let icon_rect = Rect::from_center_size(rect.center() + ink.offset, Vec2::splat(16.0));
    icon::draw(painter, glyph, icon_rect, ink.color, ICON_STROKE);
}

// -- toggles ---------------------------------------------------------------

/// The shared toggle interaction: flip `selected` on a primary click (right
/// clicks pass through on the [`Response`]), report as a checkbox for
/// accessibility and the headless tests, and read out the press state.
fn interact_toggle(
    ui: &mut Ui,
    selected: &mut bool,
    size: Vec2,
    label: &str,
) -> (Response, Rect, bool, bool) {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *selected = !*selected;
        response.mark_changed();
    }
    let on = *selected;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, label)
    });
    let held = response.is_pointer_button_down_on();
    (response, rect, on, held)
}

/// A latching text toggle drawn as a pad: raised while off, lit amber while on,
/// an identical footprint in every state. Sized to its label, like [`button`].
pub fn toggle(ui: &mut Ui, palette: &Palette, selected: &mut bool, text: &str) -> Response {
    toggle_impl(ui, palette, selected, text, None)
}

/// As [`toggle`] but allocated at exactly `size` -- e.g. a channel digit pinned
/// to the pan-knob width so it stays centred under its knob whatever its state.
pub fn toggle_sized(
    ui: &mut Ui,
    palette: &Palette,
    selected: &mut bool,
    text: &str,
    size: Vec2,
) -> Response {
    toggle_impl(ui, palette, selected, text, Some(size))
}

fn toggle_impl(
    ui: &mut Ui,
    palette: &Palette,
    selected: &mut bool,
    text: &str,
    fixed: Option<Vec2>,
) -> Response {
    let galley = button_galley(ui, text);
    let desired = fixed.unwrap_or_else(|| content_size(ui, &galley));
    let (response, rect, on, held) = interact_toggle(ui, selected, desired, text);
    if ui.is_rect_visible(rect) {
        let state = PadState {
            hovered: response.hovered(),
            held,
            latched: on,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        let text_pos = rect.center() - galley.size() * 0.5 + ink.offset;
        ui.painter().galley(text_pos, galley, ink.color);
    }
    response
}

/// A square icon toggle: a latching pad drawing `glyph`, lit amber while on.
pub fn icon_toggle(
    ui: &mut Ui,
    palette: &Palette,
    selected: &mut bool,
    glyph: Icon,
    label: &str,
) -> Response {
    let size = square(ui);
    let (response, rect, on, held) = interact_toggle(ui, selected, size, label);
    if ui.is_rect_visible(rect) {
        let state = PadState {
            hovered: response.hovered(),
            held,
            latched: on,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        paint_icon(ui.painter(), glyph, rect, ink);
    }
    response
}

// -- mute toggles (channels, percussion) -----------------------------------

/// A "mute" toggle: `on` (audible) shows a plain idle cap, `off` (muted) a dark
/// recessed one -- the inverse emphasis of an engage toggle ([`toggle`]), which
/// lights amber when on. Sized to `size`, like [`toggle_sized`].
pub fn mute_toggle_sized(
    ui: &mut Ui,
    palette: &Palette,
    on: &mut bool,
    text: &str,
    size: Vec2,
) -> Response {
    let galley = button_galley(ui, text);
    let (response, rect, audible, held) = interact_toggle(ui, on, size, text);
    if ui.is_rect_visible(rect) {
        let state = PadState {
            hovered: response.hovered(),
            held,
            muted: !audible,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        let text_pos = rect.center() - galley.size() * 0.5 + ink.offset;
        ui.painter().galley(text_pos, galley, ink.color);
    }
    response
}

/// A square icon mute toggle: `on` (audible) a plain idle cap, `off` (muted) a
/// dark recessed one.
pub fn icon_mute_toggle(
    ui: &mut Ui,
    palette: &Palette,
    on: &mut bool,
    glyph: Icon,
    label: &str,
) -> Response {
    let size = square(ui);
    let (response, rect, audible, held) = interact_toggle(ui, on, size, label);
    if ui.is_rect_visible(rect) {
        let state = PadState {
            hovered: response.hovered(),
            held,
            muted: !audible,
            ..PadState::default()
        };
        let ink = paint_pad(ui.painter(), rect, palette, state);
        paint_icon(ui.painter(), glyph, rect, ink);
    }
    response
}
