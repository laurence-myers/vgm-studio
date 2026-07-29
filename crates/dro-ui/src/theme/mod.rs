//! A FastTracker II-inspired egui theme: a DOS bitmap font, square bevelled
//! chrome, and two colour presets (a dark ft2-clone scheme and the original
//! steel-blue one). [`install`] wires it into a [`egui::Context`] at startup;
//! [`apply_palette`] switches presets live from the Settings dialog.
//!
//! The palette is passed explicitly to the widgets that need custom colours
//! ([`Palette`]); everything else inherits the [`egui::Style`] built here.

use std::sync::Arc;

use egui::Color32;

pub mod bevel;
mod fonts;
pub mod icon;
pub(crate) mod paint;
mod palette;
mod style;
pub mod tabs;

pub use dro_core::config::{SurfaceChoice, ThemeChoice};
pub use palette::{Palette, Surface, palette};
pub(crate) use palette::{deck_stops, pad_caps};

/// The palette for `choice` with the configured pad/deck overrides applied.
/// `SurfaceChoice::ThemeDefault` leaves the case's own treatment alone, so a
/// theme only changes where the user has asked it to.
#[must_use]
pub fn palette_with(choice: ThemeChoice, pad: SurfaceChoice, deck: SurfaceChoice) -> Palette {
    let forced = |c: SurfaceChoice| match c {
        SurfaceChoice::ThemeDefault => None,
        SurfaceChoice::Light => Some(Surface::Light),
        SurfaceChoice::Dark => Some(Surface::Dark),
        SurfaceChoice::Grey => Some(Surface::Grey),
        SurfaceChoice::Tint => Some(Surface::Tint),
    };
    let mut p = *palette(choice);
    if let Some(s) = forced(pad) {
        p.pad = s;
    }
    // Grey is not one of the deck's treatments; `for_deck` folds it away.
    if let Some(s) = forced(deck.for_deck()) {
        p.deck = s;
    }
    p
}

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

/// [`frame_scrollbar`] for a scroll area that was just shown: works the bar's
/// rect out from the viewport (`inner_rect`) and the style's bar metrics, and
/// paints nothing when the content fits -- there is no bar to frame then.
///
/// The bar sits `bar_inner_margin` to the right of the viewport, which is the
/// breathing room [`style_for`](style::style_for) opens between content and
/// channel so text never runs hard up against the well.
pub fn frame_scroll_output(
    ui: &egui::Ui,
    palette: &Palette,
    inner_rect: egui::Rect,
    content_size: egui::Vec2,
) {
    if content_size.y <= inner_rect.height() {
        return;
    }
    let scroll = ui.spacing().scroll;
    let left = inner_rect.right() + scroll.bar_inner_margin;
    let bar = egui::Rect::from_min_max(
        egui::pos2(left, inner_rect.top()),
        egui::pos2(left + scroll.bar_width, inner_rect.bottom()),
    );
    frame_scrollbar(ui, palette, bar);
}

/// A fascia plate as a single [`egui::Shape`]: the case's vertical brushed-metal
/// gradient from `plate_top` down to `plate_bottom`. The panel-seam grooves
/// supply the lit/shadow plate edges, so this is the gradient fill only.
pub fn plate_shape(rect: egui::Rect, palette: &Palette) -> egui::Shape {
    paint::plate_mesh(rect, palette.plate_top, palette.plate_bottom)
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

/// A deck surface as a single [`egui::Shape`]: the gradient the case's `deck`
/// mode resolves to (the plate when `Tint`, else a fixed light/dark preset).
/// The deck is the control-panel surface the pads sit on.
pub fn deck_shape(rect: egui::Rect, palette: &Palette) -> egui::Shape {
    let (top, bottom) = deck_stops(palette);
    paint::plate_mesh(rect, top, bottom)
}

/// The ink for chrome text sitting on the deck. The deck is coloured
/// independently of the plate, so the case's own label colour can be quite
/// wrong on it -- a light deck needs dark text. Picks by the deck's luminance.
#[must_use]
pub fn deck_ink(palette: &Palette) -> Color32 {
    let (top, bottom) = deck_stops(palette);
    let mid = paint::lerp_color(top, bottom, 0.5);
    if paint::is_light(mid) {
        paint::darken(mid, 0.80)
    } else {
        palette.label
    }
}

/// Runs `add_contents` inside a deck (the control panel's surface), like
/// [`plate_panel`] but coloured by the case's `deck` mode, so the pads can sit
/// on a surface distinct from the surrounding plate.
pub fn deck_panel<R>(
    ui: &mut egui::Ui,
    palette: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    // Plain labels on the deck take the deck's own ink, so a light deck reads.
    // Text that sets its own colour (the wells' tracker digits) is untouched.
    ui.visuals_mut().widgets.noninteractive.fg_stroke.color = deck_ink(palette);
    let inner = add_contents(ui);
    let rect =
        egui::Rect::from_x_y_ranges(ui.clip_rect().x_range(), ui.min_rect().y_range()).expand(4.0);
    ui.painter().set(slot, deck_shape(rect, palette));
    inner
}

/// Padding between a [`silkscreen_group`]'s keyline and the controls inside it,
/// even on all four sides, and the caption's inset from the left keyline.
const GROUP_PAD: f32 = 10.0;
/// The gap between adjacent controls inside a group.
const GROUP_GAP: f32 = 4.0;
/// A group keyline's corner radius, matching the view tabs' well so the two read
/// as the same family of chrome.
const GROUP_RADIUS: f32 = 3.0;

/// Runs `add_contents` in a silkscreen control group: a keyline box with its
/// caption cut into the top edge, the way a fascia labels a cluster of controls.
/// The contents are laid out in a row.
///
/// `ink` is the silkscreen colour, chosen by the caller to suit the surface
/// underneath -- [`Palette::label`] on a plate, `data_label` on the desktop the
/// pack view sits on. The keyline is the same ink dimmed, so the group is
/// printed *on* the surface rather than cut into it, and needs to know nothing
/// about what it sits on: the caption's gap is drawn as two top segments rather
/// than by erasing a slot in the line.
pub fn silkscreen_group<R>(
    ui: &mut egui::Ui,
    ink: Color32,
    caption: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(caption.to_owned(), font, ink));
    // The keyline runs through the caption's middle, so half of it overhangs.
    let overhang = (galley.size().y * 0.5).round();

    // `add_space` only advances the cursor, but the cursor already sits one
    // `item_spacing` past the last *widget* -- so a trailing space measures that
    // much wider than an identical leading one. Zero the vertical spacing and
    // discount the row gap on the trailing side, and all four paddings come out
    // equal.
    let out = ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.add_space(overhang + GROUP_PAD);
        let inner = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GROUP_GAP;
                ui.add_space(GROUP_PAD);
                let inner = add_contents(ui);
                ui.add_space(GROUP_PAD - GROUP_GAP);
                inner
            })
            .inner;
        ui.add_space(GROUP_PAD);
        inner
    });

    let rect = out.response.rect;
    let box_rect =
        egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + overhang), rect.max);
    let caption_pos = egui::pos2(rect.left() + GROUP_PAD, rect.top());
    let gap = egui::Rangef::new(caption_pos.x - 4.0, caption_pos.x + galley.size().x + 4.0);
    let painter = ui.painter();
    gapped_outline(painter, box_rect, gap, ink.gamma_multiply(0.4));
    painter.galley(caption_pos, galley, ink);
    out.inner
}

/// A rounded 1px rectangle outline centred on `rect`'s edges, opened by `gap` in
/// the top edge -- the hole a group caption sits in. One open polyline, walked
/// clockwise from the gap's right lip all the way round to its left lip, since a
/// rounded `rect_stroke` cannot be broken.
fn gapped_outline(painter: &egui::Painter, rect: egui::Rect, gap: egui::Rangef, color: Color32) {
    let r = GROUP_RADIUS;
    let (left, right) = (rect.left() + 0.5, rect.right() - 0.5);
    let (top, bottom) = (rect.top() + 0.5, rect.bottom() - 0.5);
    let mut points = vec![egui::pos2(gap.max, top)];
    // Each corner is a quarter turn, entered and left on the edges it joins.
    let mut corner = |cx: f32, cy: f32, from_deg: f32| {
        for step in 0..=4_u8 {
            let angle = (from_deg + 22.5 * f32::from(step)).to_radians();
            points.push(egui::pos2(cx + r * angle.cos(), cy + r * angle.sin()));
        }
    };
    corner(right - r, top + r, -90.0);
    corner(right - r, bottom - r, 0.0);
    corner(left + r, bottom - r, 90.0);
    corner(left + r, top + r, 180.0);
    points.push(egui::pos2(gap.min, top));
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.0, color)));
}

/// A status lamp: a domed dot in `color`, bezelled and blooming onto the surface
/// around it, for a state that must read at a glance. Hover-only, so it takes a
/// tooltip but is never a click target -- the lamp reports, the control beside it
/// acts. The bezel is a shadow rather than a palette role, so the lamp sits on
/// any surface.
pub fn led(ui: &mut egui::Ui, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let centre = rect.center();
        // The bloom sits inside the allocated box, so a lamp never bleeds over
        // its neighbour -- the same rule the pads' lighting follows.
        painter.circle_filled(centre, 6.0, color.gamma_multiply(0.22));
        painter.circle_filled(centre, 4.0, color);
        painter.circle_stroke(
            centre,
            4.0,
            egui::Stroke::new(1.0, Color32::from_black_alpha(150)),
        );
        // A specular glint up-left, so the lamp reads as a dome rather than a dot.
        painter.circle_filled(
            centre - egui::vec2(1.2, 1.2),
            1.1,
            Color32::from_white_alpha(110),
        );
    }
    response
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
    // Pack mode shows the pack's screenshots inline; the loader decodes the PNG
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
