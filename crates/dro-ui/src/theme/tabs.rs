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
    Color32, CornerRadius, Rangef, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
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

/// Draws the display tab strip for `labels`, with `selected` lit. Returns the
/// index clicked this frame, if any.
///
/// Wrap the call in `ui.add_enabled_ui(false, ..)` (or any disabled scope) to
/// grey the whole strip: the labels sink toward the well and the cells stop
/// responding to the pointer.
pub fn strip(ui: &mut Ui, palette: &Palette, labels: &[&str], selected: usize) -> Option<usize> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    // `PLACEHOLDER` is the recolour sentinel: the ink is chosen per cell at paint
    // time, once its lit/dim state is known.
    let galleys: Vec<_> = labels
        .iter()
        .map(|text| {
            ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap((*text).to_owned(), font.clone(), Color32::PLACEHOLDER)
            })
        })
        .collect();

    let cell_h = galleys
        .iter()
        .map(|g| g.size().y)
        .fold(0.0_f32, f32::max)
        + CELL_PAD_Y * 2.0;
    let cell_w: Vec<f32> = galleys
        .iter()
        .map(|g| g.size().x + CELL_PAD_X * 2.0)
        .collect();
    let inner_w =
        cell_w.iter().sum::<f32>() + GAP * labels.len().saturating_sub(1) as f32;
    // The well's own response carries a unique auto id; the cells hang off it, so
    // two strips in one `Ui` never collide.
    let (well, well_response) = ui.allocate_exact_size(
        vec2(inner_w + WELL_PAD * 2.0, cell_h + WELL_PAD * 2.0),
        Sense::hover(),
    );

    let enabled = ui.is_enabled();
    if ui.is_rect_visible(well) {
        paint_well(ui, well, palette);
    }

    let mut clicked = None;
    let mut x = well.left() + WELL_PAD;
    for (i, galley) in galleys.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x, well.top() + WELL_PAD), vec2(cell_w[i], cell_h));
        x += cell_w[i] + GAP;

        let response = ui.interact(cell, well_response.id.with(i), Sense::click());
        let lit = i == selected;
        let label = labels[i];
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, lit, label)
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
                if enabled { 0.72 } else { 0.86 },
            );
            painter.rect_stroke(cell, radius, Stroke::new(1.0, ring), StrokeKind::Inside);
        } else if response.hovered() {
            painter.rect_filled(cell, radius, Color32::from_white_alpha(13));
        }

        let base = if lit {
            palette.data_text
        } else {
            palette.data_label
        };
        let ink = if enabled {
            base
        } else {
            lerp_color(base, palette.data_bg, DISABLED_SINK)
        };
        painter.galley(cell.center() - galley.size() * 0.5, galley, ink);
    }
    clicked
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

    /// What the harness drives: which view is lit, and whether the strip is live.
    struct State {
        selected: usize,
        enabled: bool,
    }

    /// Renders one strip, feeding clicks back into `selected` the way the app does.
    fn show(ui: &mut Ui, state: &mut State) {
        let palette = super::super::palette::palette(ThemeChoice::Navy);
        let enabled = state.enabled;
        ui.add_enabled_ui(enabled, |ui| {
            if let Some(i) = strip(ui, palette, &["Editor", "Pack"], state.selected) {
                state.selected = i;
            }
        });
    }

    #[test]
    fn clicking_a_tab_selects_it() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(240.0, 60.0))
            .build_ui_state(show, State {
                selected: 0,
                enabled: true,
            });
        harness.run();
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
        let mut harness = egui_kittest::Harness::builder()
            .with_size(vec2(240.0, 60.0))
            .build_ui_state(show, State {
                selected: 0,
                enabled: false,
            });
        harness.run();
        harness.get_by_label("Pack").click();
        harness.run();
        assert_eq!(
            harness.state().selected,
            0,
            "a greyed strip cannot change the view"
        );
    }
}
