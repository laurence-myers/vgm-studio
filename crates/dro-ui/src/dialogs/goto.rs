//! The Goto dialog (`dialogs.DTDialogGoto`). Modeless; stays open after Go so
//! the user can jump repeatedly. Validation (and its exact status-bar
//! messages) lives in the app, as it did in `wxapp.button_goto`.

use crate::action::Action;

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
    pub fn show(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) -> bool {
        let mut open = true;
        let mut close_clicked = false;
        egui::Window::new("Goto Position")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.input)
                        .hint_text("position")
                        .desired_width(120.0),
                );
                let submitted = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("Go").clicked() || submitted {
                        actions.push(Action::GotoSubmitted(self.input.clone()));
                    }
                    if ui.button("Close").clicked() {
                        close_clicked = true;
                    }
                });
            });
        open && !close_clicked
    }
}
