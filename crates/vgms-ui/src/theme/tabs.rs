//! The display tab strip: view switching that lives *in* the instrument's
//! screen rather than on its casework.
//!
//! The labels sit in a dark readout well -- the same `data_bg` surface as the
//! table and the scope, sunk with a shadowed top lip -- and the active one burns
//! in the case's complementary `data_text`: gold on navy, tracker yellow on
//! teal, cyan on cream and rust. Inactive views stay in the dimmer `data_label`,
//! and a disabled strip falls toward the well itself, which is what an undriven
//! LCD cell actually looks like. Nothing here is a pad, so the amber latch keeps
//! meaning "engaged" everywhere else in the app.
//!
//! Cells report themselves as selectable labels to accessibility and
//! egui_kittest, so `get_by_label("Editor").click()` still drives them.

use egui::{
    Color32, CornerRadius, Rangef, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::paint::lerp_color;
use super::palette::Palette;

/// Padding between the well's edge and the cells.
const WELL_PAD: f32 = 3.0;
/// Gap between adjacent cells.
const GAP: f32 = 3.0;
/// Horizontal padding inside a cell, around its label.
const CELL_PAD_X: f32 = 10.0;
/// Vertical padding inside a cell.
const CELL_PAD_Y: f32 = 3.0;
/// The well's corner radius.
const WELL_RADIUS: u8 = 3;
/// A cell's corner radius, a shade tighter so it nests inside the well.
const CELL_RADIUS: u8 = 2;
/// How far a disabled label falls toward the well behind it.
const DISABLED_SINK: f32 = 0.45;

/// One tab in a [`strip`]: the view's name, and whether it can be entered.
#[derive(Debug, Clone, Copy)]
pub struct Tab<'a> {
    /// The label, and the accessible name the GUI tests find the cell by.
    pub label: &'a str,
    /// Whether the view can be entered right now. A disabled tab keeps its place
    /// in the strip -- so the app's shape does not change as state comes and
    /// goes -- but is greyed and inert, saying the view exists yet is not
    /// available.
    pub enabled: bool,
}

impl<'a> Tab<'a> {
    /// A tab whose view can be entered.
    #[must_use]
    pub const fn new(label: &'a str) -> Self {
        Self {
            label,
            enabled: true,
        }
    }

    /// The same tab, enterable only when `enabled`.
    #[must_use]
    pub const fn enabled(self, enabled: bool) -> Self {
        Self {
            label: self.label,
            enabled,
        }
    }
}

/// Draws the display tab strip for `tabs`, with `selected` lit. Returns the
/// index clicked this frame, if any; a disabled tab never reports a click.
///
/// Disable a single view with [`Tab::enabled`], or wrap the whole call in
/// `ui.add_enabled_ui(false, ..)` to grey the strip entire. Either way the
/// affected labels sink toward the well and stop answering the pointer.
pub fn strip(ui: &mut Ui, palette: &Palette, tabs: &[Tab], selected: usize) -> Option<usize> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    // `PLACEHOLDER` is the recolour sentinel: the ink is chosen per cell at paint
    // time, once its lit/dim state is known.
    let galleys: Vec<_> = tabs
        .iter()
        .map(|tab| {
            ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(tab.label.to_owned(), font.clone(), Color32::PLACEHOLDER)
            })
        })
        .collect();

    let cell_h = galleys.iter().map(|g| g.size().y).fold(0.0_f32, f32::max) + CELL_PAD_Y * 2.0;
    let cell_w: Vec<f32> = galleys
        .iter()
        .map(|g| g.size().x + CELL_PAD_X * 2.0)
        .collect();
    let inner_w = cell_w.iter().sum::<f32>() + GAP * tabs.len().saturating_sub(1) as f32;
    // The well's own response carries a unique auto id; the cells hang off it, so
    // two strips in one `Ui` never collide.
    let (well, well_response) = ui.allocate_exact_size(
        vec2(inner_w + WELL_PAD * 2.0, cell_h + WELL_PAD * 2.0),
        Sense::hover(),
    );

    let strip_live = ui.is_enabled();
    if ui.is_rect_visible(well) {
        paint_well(ui, well, palette);
    }

    let mut clicked = None;
    let mut x = well.left() + WELL_PAD;
    for (i, galley) in galleys.into_iter().enumerate() {
        let tab = tabs[i];
        // A cell answers the pointer only if the strip is live *and* its own view
        // is available.
        let live = strip_live && tab.enabled;
        let cell = Rect::from_min_size(pos2(x, well.top() + WELL_PAD), vec2(cell_w[i], cell_h));
        x += cell_w[i] + GAP;

        // A dead cell senses hover only, so it cannot be clicked and shows no
        // interaction cursor.
        let sense = if live { Sense::click() } else { Sense::hover() };
        let response = ui.interact(cell, well_response.id.with(i), sense);
        let lit = i == selected;
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, live, lit, tab.label)
        });
        if response.clicked() {
            clicked = Some(i);
        }

        if !ui.is_rect_visible(cell) {
            continue;
        }
        let painter = ui.painter();
        let radius = CornerRadius::same(CELL_RADIUS);
        if lit {
            painter.rect_filled(cell, radius, palette.data_stripe);
            // A faint ring in the display ink, so the lit cell reads as a driven
            // segment rather than just brighter text.
            let ring = lerp_color(
                palette.data_text,
                palette.data_bg,
                if live { 0.72 } else { 0.86 },
            );
            painter.rect_stroke(cell, radius, Stroke::new(1.0, ring), StrokeKind::Inside);
        } else if live && response.hovered() {
            painter.rect_filled(cell, radius, Color32::from_white_alpha(13));
        }

        let base = if lit {
            palette.data_text
        } else {
            palette.data_label
        };
        let ink = if live {
            base
        } else {
            lerp_color(base, palette.data_bg, DISABLED_SINK)
        };
        painter.galley(cell.center() - galley.size() * 0.5, galley, ink);
    }
    clicked
}

/// A single display tab as a self-contained button at the cursor, in the same
/// well-cell chrome [`strip`] draws: `data_stripe` fill and a display-ink ring
/// when `selected`, a faint hover wash otherwise, the label in `data_text` (lit)
/// or `data_label` (dim). For composite strips like the chip mixer, whose cells
/// carry a lamp and a trim knob beside the name and so cannot use `strip`'s
/// single-galley layout. Reports itself as a selectable label, so
/// `get_by_label` still drives it.
pub(crate) fn tab_button(ui: &mut Ui, palette: &Palette, label: &str, selected: bool) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley =
        ui.fonts_mut(|fonts| fonts.layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER));
    let size = galley.size() + vec2(CELL_PAD_X * 2.0, CELL_PAD_Y * 2.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let live = ui.is_enabled();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, live, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(CELL_RADIUS);
        if selected {
            painter.rect_filled(rect, radius, palette.data_stripe);
            let ring = lerp_color(
                palette.data_text,
                palette.data_bg,
                if live { 0.72 } else { 0.86 },
            );
            painter.rect_stroke(rect, radius, Stroke::new(1.0, ring), StrokeKind::Inside);
        } else if live && response.hovered() {
            painter.rect_filled(rect, radius, Color32::from_white_alpha(13));
        }
        let base = if selected {
            palette.data_text
        } else {
            palette.data_label
        };
        let ink = if live {
            base
        } else {
            lerp_color(base, palette.data_bg, DISABLED_SINK)
        };
        painter.galley(rect.center() - galley.size() * 0.5, galley, ink);
    }
    response
}

/// Paints the readout well the cells sit in: the dark data surface, a shadowed
/// top lip and a faint lit bottom edge (so it reads sunk into the plate), framed
/// by the case's plate keyline.
fn paint_well(ui: &Ui, well: Rect, palette: &Palette) {
    let painter = ui.painter();
    let radius = CornerRadius::same(WELL_RADIUS);
    painter.rect_filled(well, radius, palette.data_bg);
    // Kept clear of the rounded corners, as the pad painter does with its glint.
    let inset = Rangef::new(
        well.left() + f32::from(WELL_RADIUS),
        well.right() - f32::from(WELL_RADIUS),
    );
    painter.hline(
        inset,
        well.top() + 0.5,
        Stroke::new(1.0, Color32::from_black_alpha(120)),
    );
    painter.hline(
        inset,
        well.top() + 1.5,
        Stroke::new(1.0, Color32::from_black_alpha(60)),
    );
    painter.hline(
        inset,
        well.bottom() - 0.5,
        Stroke::new(1.0, Color32::from_white_alpha(16)),
    );
    painter.rect_stroke(
        well,
        radius,
        Stroke::new(1.0, palette.plate_border),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeChoice;
    use egui_kittest::kittest::Queryable as _;

    /// What the harness drives: which view is lit, whether the whole strip is
    /// live, and whether the second view can be entered.
    struct State {
        selected: usize,
        strip_enabled: bool,
        second_enabled: bool,
    }

    impl State {
        fn new() -> Self {
            Self {
                selected: 0,
                strip_enabled: true,
                second_enabled: true,
            }
        }
    }

    /// Renders one strip, feeding clicks back into `selected` the way the app does.
    fn show(ui: &mut Ui, state: &mut State) {
        let palette = super::super::palette::palette(ThemeChoice::Navy);
        let tabs = [
            Tab::new("Editor"),
            Tab::new("Pack").enabled(state.second_enabled),
        ];
        let enabled = state.strip_enabled;
        ui.add_enabled_ui(enabled, |ui| {
            if let Some(i) = strip(ui, palette, &tabs, state.selected) {
                state.selected = i;
            }
        });
    }

    /// A harness over [`show`], with `tweak` applied to the initial state.
    fn harness(tweak: impl FnOnce(&mut State)) -> egui_kittest::Harness<'static, State> {
        let mut state = State::new();
        tweak(&mut state);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(240.0, 60.0))
            .build_ui_state(show, state);
        harness.run();
        harness
    }

    #[test]
    fn clicking_a_tab_selects_it() {
        let mut harness = harness(|_| {});
        // The cells report as selectable labels, so the app's GUI tests can keep
        // driving them by name.
        harness.get_by_label("Pack").click();
        harness.run();
        assert_eq!(harness.state().selected, 1, "clicking Pack lights Pack");

        harness.get_by_label("Editor").click();
        harness.run();
        assert_eq!(harness.state().selected, 0, "and back again");
    }

    #[test]
    fn a_disabled_strip_ignores_clicks() {
        let mut harness = harness(|s| s.strip_enabled = false);
        harness.get_by_label("Pack").click();
        harness.run();
        assert_eq!(
            harness.state().selected,
            0,
            "a greyed strip cannot change the view"
        );
    }

    #[test]
    fn a_disabled_tab_ignores_clicks_while_its_neighbour_still_works() {
        let mut harness = harness(|s| s.second_enabled = false);
        harness.get_by_label("Pack").click();
        harness.run();
        assert_eq!(
            harness.state().selected,
            0,
            "an unavailable view cannot be entered"
        );

        // The rest of the strip is unaffected -- only the one view is barred.
        harness.state_mut().selected = 1;
        harness.run();
        harness.get_by_label("Editor").click();
        harness.run();
        assert_eq!(harness.state().selected, 0, "its neighbour still selects");
    }
}
