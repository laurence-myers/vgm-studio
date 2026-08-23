//! The virtual instruction table.
//!
//! Five columns. Only visible rows are built each frame (`egui_extras` virtual
//! rows), so the 100k+-row requirement holds. The Bank and Description columns
//! are computed synchronously from the replay cursor, so there is no "`?` until
//! the analysis task finishes" phase.

use egui::{Color32, Sense};
use egui_extras::{Column, TableBuilder};

use crate::editor::{Editor, FoldSummary, VisibleRow};
use crate::selection::ClickModifiers;
use crate::theme::Palette;

/// A row the table should bring into view, and where in the view it should land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollTo {
    pub(crate) row: usize,
    pub(crate) align: egui::Align,
}

impl ScrollTo {
    /// Centred: for a jump to a row the user is about to work *on* -- a search
    /// hit, a selection moved by the keyboard -- where the rows either side are
    /// the context that matters.
    #[must_use]
    pub(crate) fn centered(row: usize) -> Self {
        Self {
            row,
            align: egui::Align::Center,
        }
    }

    /// At the top of the view: for a jump to a row the user is about to play
    /// *from*, where what follows it is what matters. egui clamps the scroll at
    /// the end of the list, so a row with too few after it to fill the view
    /// simply sits as high as it can.
    #[must_use]
    pub(crate) fn to_top(row: usize) -> Self {
        Self {
            row,
            align: egui::Align::TOP,
        }
    }
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    scroll_to: Option<ScrollTo>,
    palette: &Palette,
) {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    // Fold contiguous runs of waits/DAC writes before counting rows, so the table
    // draws structure rather than a wall of them. `len` is the *visible* count.
    editor.ensure_folds();
    let len = editor.visible_len();

    // The area the table fills; used afterwards to frame the scrollbar channel.
    let area = ui.max_rect();
    let bar_width = ui.spacing().scroll.bar_width;
    let header_height = row_height + 2.0;

    // Size the chip column to the widest label the file can show, so a name like
    // "YM2612 #2 p1" never wraps as it scrolls in. OPL bank rows keep the narrow
    // default (`widest_chip_label` returns `None` for them).
    let second_col_min = editor.widest_chip_label().map_or(40.0, |label| {
        let mono = egui::TextStyle::Monospace.resolve(ui.style());
        let text_w = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(label, mono, Color32::PLACEHOLDER)
                .size()
                .x
        });
        (text_w + 8.0).ceil()
    });

    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .column(Column::auto().at_least(50.0)) // Pos.
        .column(Column::auto().at_least(second_col_min)) // Chip / Bank
        .column(Column::auto().at_least(45.0)) // Reg.
        .column(Column::auto().at_least(80.0)) // Value
        .column(Column::remainder().at_least(120.0)) // Description (all options on hover)
        .min_scrolled_height(0.0);

    if let Some(scroll_to) = scroll_to {
        // `scroll_to.row` is an instruction index; a folded run may show it under
        // a summary row, so translate to the visible row it lives on.
        let visible = editor.visible_of(scroll_to.row);
        builder = builder.scroll_to_row(visible, Some(scroll_to.align));
    }

    builder
        .header(header_height, |mut header| {
            for title in editor.column_titles() {
                header.col(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .monospace()
                            .color(palette.data_text),
                    );
                });
            }
        })
        .body(|body| {
            body.rows(row_height, len, |mut row| {
                let visible = row.index();
                match editor.visible_row(visible) {
                    Some(VisibleRow::Instruction(index)) => {
                        instruction_row(&mut row, editor, index, palette);
                    }
                    Some(VisibleRow::Summary(summary)) => {
                        summary_row(&mut row, editor, visible, &summary, palette);
                    }
                    None => {}
                }
            });
        });

    // When the body overflows, egui shows a vertical scrollbar down the right of
    // the body; frame that channel with the sunken well bevel so it is not a flat
    // strip with gaps around it.
    let body_top = area.top() + header_height;
    let overflows = len as f32 * row_height > (area.bottom() - body_top);
    if overflows {
        let bar =
            egui::Rect::from_min_max(egui::pos2(area.right() - bar_width, body_top), area.max);
        crate::theme::frame_scrollbar(ui, palette, bar);
    }
}

/// One real instruction row: the five cells, selectable and click-to-select,
/// exactly as before folding.
fn instruction_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    editor: &mut Editor,
    index: usize,
    palette: &Palette,
) {
    let selected = editor.selection.contains(index);
    row.set_selected(selected);

    // A selected row paints `selection.stroke` over its cells via
    // `override_text_color`; an explicit per-cell colour would beat that and
    // clash with the accent bar, so drop it when selected.
    let tint = |color: Color32| (!selected).then_some(color);

    let cells = editor.row_cells(index);
    cell(row, cells.position, tint(palette.data_text));
    cell(row, cells.bank, tint(palette.muted));
    cell(row, cells.register, tint(palette.data_text));
    cell(row, cells.value, tint(palette.data_text));
    // The Description cell, with the full "all register options" text (formerly
    // its own column) shown on hover.
    row.col(|ui| {
        let mut rich = egui::RichText::new(cells.description).monospace();
        if let Some(color) = tint(palette.data_label) {
            rich = rich.color(color);
        }
        let response = ui.add(egui::Label::new(rich).selectable(false));
        if !cells.hover.is_empty() {
            response.on_hover_text(cells.hover);
        }
    });

    let response = row.response();
    if response.clicked() {
        let modifiers = response.ctx.input(|i| i.modifiers);
        editor.selection.click(
            index,
            ClickModifiers {
                toggle: modifiers.command,
                extend: modifiers.shift,
            },
        );
    }
}

/// A fold's summary row: a disclosure chevron and the run's count in the
/// description, its start in the position column, the rest blank. Clicking the
/// row toggles the fold; it never enters the selection.
fn summary_row(
    row: &mut egui_extras::TableRow<'_, '_>,
    editor: &mut Editor,
    visible: usize,
    summary: &FoldSummary,
    palette: &Palette,
) {
    cell(row, format!("{:#06X}", summary.start), Some(palette.muted));
    cell(row, String::new(), None);
    cell(row, String::new(), None);
    cell(row, String::new(), None);
    row.col(|ui| {
        // CP437 triangles, as the chip deck uses: down = expanded, right =
        // collapsed (the tree-view disclosure idiom).
        let chevron = if summary.expanded {
            '\u{25BC}'
        } else {
            '\u{25BA}'
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("{chevron} {}", summary.label()))
                    .monospace()
                    .color(palette.muted),
            )
            .selectable(false)
            .wrap_mode(egui::TextWrapMode::Extend),
        );
    });

    let response = row.response();
    if response.clicked() {
        editor.toggle_fold(visible);
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if summary.expanded {
            crate::strings::TABLE_FOLD_COLLAPSE
        } else {
            crate::strings::TABLE_FOLD_EXPAND
        });
}

fn cell(row: &mut egui_extras::TableRow<'_, '_>, text: String, color: Option<Color32>) {
    row.col(|ui| {
        let mut rich = egui::RichText::new(text).monospace();
        if let Some(color) = color {
            rich = rich.color(color);
        }
        // Rows are a fixed height, so a wrapped cell spills into a clipped second
        // line -- keep these single-token cells on one line (they extend, and the
        // column is sized to fit) rather than wrapping awkwardly.
        ui.add(
            egui::Label::new(rich)
                .selectable(false)
                .wrap_mode(egui::TextWrapMode::Extend),
        );
    });
}
