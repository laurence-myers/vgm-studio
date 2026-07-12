//! The Goto dialog (`dialogs.DTDialogGoto`). Modeless; stays open after Go so
//! the user can jump repeatedly. Validation (and its exact status-bar
//! messages) lives in the app, as it did in `wxapp.button_goto`.

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug, Default)]
pub struct GotoDialog {
    input: String,
}

impl GotoDialog {
    #[must_use]
    pub fn new() -> Self {
        // The Python spinner started visually empty.
        Self::default()
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new("Goto Position")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.add_space(2.0);
                ui.label("Go to instruction:");
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("position")
                        .text_color(palette.data_text)
                        .desired_width(160.0),
                );
                let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                crate::theme::separator(ui, palette);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if bevel::button(ui, palette, "Close").clicked() {
                        close_clicked = true;
                    }
                    if bevel::button(ui, palette, "Go").clicked() || submitted {
                        actions.push(Action::GotoSubmitted(self.input.clone()));
                    }
                });
            });
        open && !close_clicked
    }
}
