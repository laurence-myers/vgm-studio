//! Rip mode's quick-edit dialog: rename a track's file and edit its GD3 tag
//! without loading it into the editor. It is the GD3 dialog plus a leading
//! "File name" field; Save emits [`Action::QuickEditSubmitted`], which rewrites
//! the file's bytes and, if the name changed, renames it.

use dro_core::{Gd3Tag, vgm::data::GD3_FIELD_COUNT};

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct TrackEditDialog {
    index: usize,
    file_name: String,
    fields: [String; GD3_FIELD_COUNT],
}

impl TrackEditDialog {
    #[must_use]
    pub fn new(index: usize, file_name: String, tag: Option<&Gd3Tag>) -> Self {
        let fields = match tag {
            Some(tag) => tag.fields().map(str::to_owned),
            None => core::array::from_fn(|_| String::new()),
        };
        Self {
            index,
            file_name,
            fields,
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_window(ctx, "Quick Edit Track", area, |ui| {
            egui::Grid::new("track-edit-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("File name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.file_name)
                            .text_color(palette.data_text)
                            .desired_width(250.0),
                    );
                    ui.end_row();

                    super::gd3_tag::gd3_fields(ui, palette, &mut self.fields);
                });
            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Save").clicked() {
                    actions.push(Action::QuickEditSubmitted {
                        index: self.index,
                        file_name: self.file_name.clone(),
                        tag: Box::new(Gd3Tag::from_fields(self.fields.clone())),
                    });
                    close = true;
                }
            });
        });
        open && !close
    }
}
