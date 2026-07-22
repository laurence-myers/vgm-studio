//! A FastTracker II-inspired egui theme: a DOS bitmap font, square bevelled
//! chrome, and two colour presets (a dark ft2-clone scheme and the original
//! steel-blue one). [`install`] wires it into a [`egui::Context`] at startup;
//! [`apply_palette`] switches presets live from the Settings dialog.
//!
//! The palette is passed explicitly to the widgets that need custom colours
//! ([`Palette`]); everything else inherits the [`egui::Style`] built here.

use std::sync::Arc;

pub mod bevel;
mod fonts;
pub mod icon;
pub(crate) mod paint;
mod palette;
mod style;

pub use dro_core::config::ThemeChoice;
pub use palette::{Palette, palette};

/// Restyles `ui` so a `ComboBox` reads as a dark input (like a text field)
/// rather than a face-coloured button. Call inside a `ui.scope` around the
/// combo; leaves the bevel stroke so it still frames.
pub fn style_dropdown(ui: &mut egui::Ui, palette: &Palette) {
    let widgets = &mut ui.visuals_mut().widgets;
    for w in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.weak_bg_fill = palette.data_bg;
        w.bg_fill = palette.data_bg;
        w.fg_stroke.color = palette.data_label;
    }
}

/// A 2px beveled divider with a little breathing room, as a drop-in for
/// `ui.separator()`. Orients across the layout like egui's own separator: a
/// horizontal groove in a vertical layout, a vertical groove in a row.
pub fn separator(ui: &mut egui::Ui, palette: &Palette) {
    if ui.layout().main_dir().is_horizontal() {
        // Match the control height so the divider never stretches the row.
        let height = ui.spacing().interact_size.y;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(6.0, height), egui::Sense::hover());
        bevel::groove_v(ui.painter(), rect.center().x - 1.0, rect.y_range(), palette);
    } else {
        // Let egui size and place the separator (it spans the content width
        // without forcing a menu or dialog wider, which allocating
        // `available_width()` ourselves would), then overpaint it as a groove.
        let rect = ui.separator().rect;
        bevel::groove_h(ui.painter(), rect.x_range(), rect.center().y - 1.0, palette);
    }
}

/// A horizontal divider that spans the whole window width, ignoring the
/// enclosing panel's margin, so it lines up with the panel-boundary grooves.
/// For use inside a full-width panel. Allocates just the 2px groove; pad around
/// it with `add_space` to taste.
pub fn separator_full(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
    // The shared background layer is not clipped to the panel content, so the
    // groove reaches the window edges, and it sits below every Window/menu/popup
    // (higher orders) so dialogs are never sliced by it.
    let painter = ui.ctx().layer_painter(egui::LayerId::background());
    bevel::groove_h(
        &painter,
        ui.ctx().viewport_rect().x_range(),
        rect.center().y - 1.0,
        palette,
    );
}

/// A horizontal 2px groove spanning the current content width, painted through
/// the ui's own (clipped) painter. Unlike [`separator_full`] it stays inside a
/// scroll area's viewport -- it does not bleed over the panels below it or past
/// the scrollbar beside it -- so it is the divider to use *inside* a scroll area.
/// Allocates just the 2px; pad around it with `add_space` to taste.
pub fn separator_clipped(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), egui::Sense::hover());
    bevel::groove_h(ui.painter(), rect.x_range(), rect.center().y - 1.0, palette);
}

/// Frames a vertical scrollbar's channel with the sunken two-tone bevel used on
/// every other well, so the bar reads as recessed and flush rather than a flat
/// strip with gaps around it. `bar` is the rect the scrollbar occupies (the
/// rightmost `scroll.bar_width` of a scroll viewport).
pub fn frame_scrollbar(ui: &egui::Ui, palette: &Palette, bar: egui::Rect) {
    bevel::paint_bevel(ui.painter(), bar, palette, bevel::Bevel::Sunken);
}

/// A fascia plate as a single [`egui::Shape`]: a subtle vertical brushed-metal
/// gradient, a touch lighter at the top and darker at the bottom. The panel-seam
/// grooves supply the lit/shadow plate edges, so this is the gradient fill only.
/// Derived from the case's `face` for now; a future case may carry explicit
/// plate stops.
pub fn plate_shape(rect: egui::Rect, palette: &Palette) -> egui::Shape {
    let top = paint::lighten(palette.face, 0.07);
    let bottom = paint::darken(palette.face, 0.09);
    paint::plate_mesh(rect, top, bottom)
}

/// Runs `add_contents` inside a fascia plate. Reserves a background slot up
/// front (egui's own `Frame` trick), runs the content, then fills the panel
/// behind it with the plate gradient -- so the gradient sits *behind* the
/// widgets without a second layout pass. Use as the body of a chrome panel whose
/// `Frame` fill is transparent.
pub fn plate_panel<R>(
    ui: &mut egui::Ui,
    palette: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let inner = add_contents(ui);
    // Full panel width (the clip rect) at the content's height, grown a little to
    // cover the frame margin so no desk shows through inside the plate.
    let rect =
        egui::Rect::from_x_y_ranges(ui.clip_rect().x_range(), ui.min_rect().y_range()).expand(4.0);
    ui.painter().set(slot, plate_shape(rect, palette));
    inner
}

/// Installs the theme: pins the dark base (so an OS light/dark flip can't swap
/// it), loads the DOS font, turns off edge feathering for hard pixels, and
/// applies the chosen palette.
pub fn install(ctx: &egui::Context, choice: ThemeChoice) {
    // eframe defaults to `ThemePreference::System` and feeds the OS theme every
    // frame; pin Dark so our single style is always the active one.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_fonts(fonts::font_definitions());
    // The pad chrome (and, later, the plate chrome) is antialiased: rounded pad
    // corners, latched-cap gradients and the icon arcs all want feathering on.
    // It does not touch glyph rendering, so the DOS font stays hard-pixel, and
    // the edge painters' `hline`/`vline` tricks stay crisp on the pixel grid.
    ctx.tessellation_options_mut(|options| options.feathering = true);
    // Rip mode shows the pack's screenshots inline; the loader decodes the PNG
    // bytes. Installed here because every shell already calls `install`.
    egui_extras::install_image_loaders(ctx);
    apply_palette(ctx, choice);
}

/// Rebuilds the style for `choice` and writes it into both theme slots, so the
/// look is identical whichever slot egui happens to consult.
pub fn apply_palette(ctx: &egui::Context, choice: ThemeChoice) {
    let style = Arc::new(style::style_for(palette(choice)));
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}
