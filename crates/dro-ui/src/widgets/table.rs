//! The virtual instruction table.
//!
//! Six columns. Only visible rows are built each frame (`egui_extras` virtual
//! rows), so the 100k+-row requirement holds. The Bank and Description columns
//! are computed synchronously from the replay cursor, so there is no "`?` until
//! the analysis task finishes" phase.

use egui::{Color32, Sense};
use egui_extras::{Column, TableBuilder};

use crate::editor::Editor;
use crate::selection::ClickModifiers;
use crate::theme::Palette;

pub fn show(ui: &mut egui::Ui, editor: &mut Editor, scroll_to: Option<usize>, palette: &Palette) {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let len = editor.len();

    // The area the table fills; used afterwards to frame the scrollbar channel.
    let area = ui.max_rect();
    let bar_width = ui.spacing().scroll.bar_width;
    let header_height = row_height + 2.0;

    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .column(Column::auto().at_least(50.0)) // Pos.
        .column(Column::auto().at_least(40.0)) // Bank
        .column(Column::auto().at_least(45.0)) // Reg.
        .column(Column::auto().at_least(80.0)) // Value
        .column(Column::remainder().at_least(120.0)) // Description (all options on hover)
        .min_scrolled_height(0.0);

    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
    }

    builder
        .header(row_height + 2.0, |mut header| {
            for title in ["Pos (hex)", "Bank", "Reg.", "Value", "Description"] {
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
                let index = row.index();
                let selected = editor.selection.contains(index);
                row.set_selected(selected);

                // A selected row paints `selection.stroke` over its cells via
                // `override_text_color`; an explicit per-cell colour would beat
                // that and clash with the accent bar, so drop it when selected.
                let tint = |color: Color32| (!selected).then_some(color);

                let analysis = editor.row_analysis(index);
                let song = editor
                    .song()
                    .expect("rows are only built while a song is loaded");

                cell(&mut row, format!("{index:04X}"), tint(palette.data_text));
                cell(
                    &mut row,
                    analysis
                        .as_ref()
                        .map_or_else(String::new, |a| a.bank.index().to_string()),
                    tint(palette.muted),
                );
                cell(
                    &mut row,
                    song.register_display(index).unwrap_or_default(),
                    tint(palette.data_text),
                );
                cell(
                    &mut row,
                    song.value_display(index).unwrap_or_default(),
                    tint(palette.data_text),
                );
                // The Description cell, with the full "all register options" text
                // (formerly its own column) shown on hover.
                let description = analysis.map_or_else(String::new, |a| a.description.into_owned());
                let all_options = song.instruction_description(index).unwrap_or_default();
                row.col(|ui| {
                    let mut rich = egui::RichText::new(description).monospace();
                    if let Some(color) = tint(palette.data_label) {
                        rich = rich.color(color);
                    }
                    let response = ui.add(egui::Label::new(rich).selectable(false));
                    if !all_options.is_empty() {
                        response.on_hover_text(all_options);
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

fn cell(row: &mut egui_extras::TableRow<'_, '_>, text: String, color: Option<Color32>) {
    row.col(|ui| {
        let mut rich = egui::RichText::new(text).monospace();
        if let Some(color) = color {
            rich = rich.color(color);
        }
        ui.add(egui::Label::new(rich).selectable(false));
    });
}
