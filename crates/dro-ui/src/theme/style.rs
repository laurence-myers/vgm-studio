//! Building an [`egui::Style`] from a [`Palette`].
//!
//! The whole DOS look lives here: square corners, no shadows, two-tone bevel
//! strokes on every widget state, a sunken data palette for text fields and
//! scrollbar troughs, and a chunky always-visible scrollbar.

use egui::style::{ScrollStyle, Selection, WidgetVisuals, Widgets};
use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, Vec2, Visuals};

use super::fonts;
use super::palette::Palette;

/// One widget state: flat fill, a 1px bevel-coloured outline, no rounding, no
/// hover expansion (square edges must not wobble).
fn widget(fill: Color32, bevel: Color32, text: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, bevel),
        corner_radius: CornerRadius::ZERO,
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

/// The full DOS-tracker style for a palette.
#[must_use]
pub(crate) fn style_for(palette: &Palette) -> Style {
    let Palette {
        face,
        face_hover,
        face_active,
        desktop,
        bevel_light,
        bevel_dark,
        data_bg,
        data_stripe,
        data_text,
        trough,
        label,
        muted,
        accent,
        selection_text,
        ..
    } = *palette;

    let mut open = widget(face, bevel_dark, label);
    // `open` paints window title bars and the open-combo button; a slightly
    // pressed face reads as "this is the active surface".
    open.weak_bg_fill = face_active;

    let widgets = Widgets {
        noninteractive: widget(face, bevel_dark, label),
        inactive: widget(face, bevel_dark, label),
        hovered: widget(face_hover, bevel_light, label),
        active: widget(face_active, bevel_light, label),
        open,
    };

    let mut visuals = Visuals::dark();
    visuals.dark_mode = true;
    visuals.widgets = widgets;
    visuals.selection = Selection {
        bg_fill: accent,
        stroke: Stroke::new(1.0, selection_text),
    };
    visuals.weak_text_color = Some(muted);
    visuals.faint_bg_color = data_stripe; // table / grid stripes
    visuals.extreme_bg_color = trough; // scrollbar trough
    visuals.text_edit_bg_color = Some(data_bg); // decoupled from the trough
    visuals.code_bg_color = data_bg;
    visuals.hyperlink_color = bevel_light;

    visuals.window_fill = face; // windows, menus, popups, tooltips
    visuals.window_stroke = Stroke::new(1.0, bevel_dark);
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::ZERO;
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow::NONE;
    visuals.window_highlight_topmost = false;
    visuals.panel_fill = desktop;
    visuals.text_cursor.stroke = Stroke::new(2.0, data_text);
    // Dialog grids opt in per-instance; keep the global default off.
    visuals.striped = false;

    let mut style = Style {
        visuals,
        ..Style::default()
    };

    style.text_styles = fonts::text_styles();
    style.animation_time = 0.0; // instant, DOS-like state changes

    let spacing = &mut style.spacing;
    spacing.button_padding = Vec2::new(8.0, 3.0);
    // A hair taller than the 16px font's button height, so buttons, toggles and
    // combo boxes all settle at exactly this height and rows align cleanly.
    spacing.interact_size = Vec2::new(40.0, 26.0);
    spacing.menu_margin = Margin::same(4);
    spacing.window_margin = Margin::same(8);
    spacing.icon_width = 16.0;
    spacing.icon_width_inner = 8.0;
    spacing.scroll = ScrollStyle {
        bar_width: 14.0,
        handle_min_length: 24.0,
        bar_inner_margin: 0.0,
        bar_outer_margin: 0.0,
        ..ScrollStyle::solid()
    };

    style
}
