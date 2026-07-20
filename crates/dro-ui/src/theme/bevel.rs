//! The DOS 3D bevel: a 1px lit edge on two sides, a 1px shadow on the other
//! two. Raised for buttons and panels, sunken for wells (the waveform, data
//! fields). Also a from-scratch bevelled [`button`], since egui's stock button
//! draws a single-stroke outline, not a two-tone bevel.

use egui::{Painter, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::palette::Palette;

/// Which way the surface catches the light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bevel {
    /// Lit top-left, shadowed bottom-right (a button at rest, a panel).
    Raised,
    /// Shadowed top-left, lit bottom-right (a pressed button, a sunken well).
    Sunken,
}

/// Paints the two-tone bevel just inside `rect`'s edges. Uses `hline`/`vline`,
/// which land on the pixel grid (line rounding is on by default), so the edges
/// stay crisp with feathering off.
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
pub fn groove_h(painter: &Painter, x_range: egui::Rangef, y: f32, palette: &Palette) {
    painter.hline(x_range, y + 0.5, Stroke::new(1.0_f32, palette.bevel_dark));
    painter.hline(x_range, y + 1.5, Stroke::new(1.0_f32, palette.bevel_light));
}

/// A vertical 2px groove: a shadow line with a highlight line just to its right.
pub fn groove_v(painter: &Painter, x: f32, y_range: egui::Rangef, palette: &Palette) {
    painter.vline(x + 0.5, y_range, Stroke::new(1.0_f32, palette.bevel_dark));
    painter.vline(x + 1.5, y_range, Stroke::new(1.0_f32, palette.bevel_light));
}

/// The lit L along `rect`'s top and left edges.
fn top_left(painter: &Painter, rect: Rect, color: egui::Color32) {
    painter.hline(
        rect.x_range(),
        rect.top() + 0.5,
        Stroke::new(1.0_f32, color),
    );
    painter.vline(
        rect.left() + 0.5,
        rect.y_range(),
        Stroke::new(1.0_f32, color),
    );
}

/// The shadowed L along `rect`'s bottom and right edges.
fn bottom_right(painter: &Painter, rect: Rect, color: egui::Color32) {
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, color),
    );
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(1.0_f32, color),
    );
}

/// A button's FT2 bevel: a near-black keyline framing the whole button, then a
/// two-tone inner bevel (bright top-left, shadow bottom-right). Inverted when
/// `pressed`, so the button reads as pushed in.
fn paint_button_bevel(painter: &Painter, rect: Rect, palette: &Palette, pressed: bool) {
    // The black keyline on all four sides.
    top_left(painter, rect, palette.bevel_border);
    bottom_right(painter, rect, palette.bevel_border);
    // The inner two-tone bevel, one pixel in.
    let inner = rect.shrink(1.0);
    let (lit, shadow) = if pressed {
        (palette.button_shadow, palette.button_light)
    } else {
        (palette.button_light, palette.button_shadow)
    };
    top_left(painter, inner, lit);
    bottom_right(painter, inner, shadow);
}

/// A bevelled push-button sized to its label: flat face, raised at rest, sunken
/// with a 1px text shift while held. Text is the palette's near-black
/// `button_text`, as on a real FT2 button.
pub fn button(ui: &mut Ui, palette: &Palette, text: &str) -> Response {
    let padding = ui.spacing().button_padding;
    let min = ui.spacing().interact_size;
    button_impl(ui, palette, text, |galley| {
        (galley + padding * 2.0).max(min)
    })
}

/// As [`button`] but allocated at exactly `size` (e.g. a full-height transport
/// button), the label centred within it.
pub fn button_sized(ui: &mut Ui, palette: &Palette, text: &str, size: Vec2) -> Response {
    button_impl(ui, palette, text, |_galley| size)
}

fn button_impl(
    ui: &mut Ui,
    palette: &Palette,
    text: &str,
    size: impl FnOnce(Vec2) -> Vec2,
) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley =
        ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, palette.button_text));

    let desired = size(galley.size());
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));

    if ui.is_rect_visible(rect) {
        let pressed = response.is_pointer_button_down_on();
        let fill = if pressed {
            palette.button_active
        } else if response.hovered() {
            palette.button_hover
        } else {
            palette.button_face
        };
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
        paint_button_bevel(painter, rect, palette, pressed);
        let offset = if pressed {
            Vec2::splat(1.0)
        } else {
            Vec2::ZERO
        };
        let text_pos = rect.center() - galley.size() * 0.5 + offset;
        painter.galley(text_pos, galley, palette.button_text);
    }

    response
}

/// A latching toggle drawn as an FT2 button: an identical footprint in every
/// state, raised while off and sunken with an accent face while on. A primary
/// click flips `selected` (right-clicks pass through on the [`Response`], for the
/// channel-solo gesture). Reports itself as a checkbox to accessibility and the
/// headless tests, so `get_by_label` still finds it. Sized to its label, like
/// [`button`], so it never grows or shrinks with hover or selection.
pub fn toggle(ui: &mut Ui, palette: &Palette, selected: &mut bool, text: &str) -> Response {
    let padding = ui.spacing().button_padding;
    let min = ui.spacing().interact_size;
    toggle_impl(ui, palette, selected, text, |galley| {
        (galley + padding * 2.0).max(min)
    })
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
    toggle_impl(ui, palette, selected, text, |_galley| size)
}

fn toggle_impl(
    ui: &mut Ui,
    palette: &Palette,
    selected: &mut bool,
    text: &str,
    size: impl FnOnce(Vec2) -> Vec2,
) -> Response {
    // The label's colour tracks the state, so lay it out with the "recolour me
    // later" sentinel and pass the real ink to `galley` at paint time.
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER));

    let desired = size(galley.size());
    let (rect, mut response) = ui.allocate_exact_size(desired, Sense::click());
    if response.clicked() {
        *selected = !*selected;
        response.mark_changed();
    }
    let on = *selected;
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, text)
    });

    if ui.is_rect_visible(rect) {
        let held = response.is_pointer_button_down_on();
        // On reads as pushed in; a live press deepens the bevel and nudges the
        // label, exactly as a momentary button does.
        let sunken = on || held;
        let fill = if on {
            palette.accent
        } else if response.hovered() {
            palette.button_hover
        } else {
            palette.button_face
        };
        let ink = if on {
            palette.selection_text
        } else {
            palette.button_text
        };
        let painter = ui.painter();
        painter.rect_filled(rect, egui::CornerRadius::ZERO, fill);
        paint_button_bevel(painter, rect, palette, sunken);
        let offset = if held { Vec2::splat(1.0) } else { Vec2::ZERO };
        let text_pos = rect.center() - galley.size() * 0.5 + offset;
        painter.galley(text_pos, galley, ink);
    }

    response
}
