//! The virtual instruction table (`tables.py`'s `DTSongDataList`).
//!
//! Same six columns as the Python. Only visible rows are built each frame
//! (`egui_extras` virtual rows), so the 100k+-row requirement holds. One
//! difference by construction: the Bank and Description columns are computed
//! synchronously from the replay cursor, so there is no "`?` until the
//! analysis task finishes" phase.

use egui::Sense;
use egui_extras::{Column, TableBuilder};

use crate::editor::Editor;
use crate::selection::ClickModifiers;

pub fn show(ui: &mut egui::Ui, editor: &mut Editor, scroll_to: Option<usize>) {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
    let len = editor.len();

    let mut builder = TableBuilder::new(ui)
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .column(Column::auto().at_least(50.0)) // Pos.
        .column(Column::auto().at_least(40.0)) // Bank
        .column(Column::auto().at_least(45.0)) // Reg.
        .column(Column::auto().at_least(80.0)) // Value
        .column(Column::remainder().at_least(120.0)) // Description
        .column(Column::remainder().at_least(120.0)) // Description (all register options)
        .min_scrolled_height(0.0);

    if let Some(row) = scroll_to {
        builder = builder.scroll_to_row(row, Some(egui::Align::Center));
    }

    builder
        .header(row_height + 2.0, |mut header| {
            for title in [
                "Pos.",
                "Bank",
                "Reg.",
                "Value",
                "Description",
                "Description (all register options)",
            ] {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, len, |mut row| {
                let index = row.index();
                row.set_selected(editor.selection.contains(index));

                let analysis = editor.row_analysis(index);
                let song = editor
                    .song()
                    .expect("rows are only built while a song is loaded");

                cell(&mut row, format!("{index:04}>"));
                cell(
                    &mut row,
                    analysis
                        .as_ref()
                        .map_or_else(String::new, |a| a.bank.index().to_string()),
                );
                cell(&mut row, song.register_display(index).unwrap_or_default());
                cell(&mut row, song.value_display(index).unwrap_or_default());
                cell(
                    &mut row,
                    analysis.map_or_else(String::new, |a| a.description.into_owned()),
                );
                cell(
                    &mut row,
                    song.instruction_description(index)
                        .unwrap_or_default()
                        .to_owned(),
                );

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
}

fn cell(row: &mut egui_extras::TableRow<'_, '_>, text: String) {
    row.col(|ui| {
        ui.add(egui::Label::new(egui::RichText::new(text).monospace()).selectable(false));
    });
}
