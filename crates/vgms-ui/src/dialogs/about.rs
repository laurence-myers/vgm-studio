//! The About dialog: who wrote the app and what the binary is licensed under,
//! with a button to the separate Licenses list of the emulator cores it links.
//!
//! Wide like Help (a block of text read across, not a form filled in down) --
//! it used to ride the narrow generic alert box.

use crate::action::{Action, UiAction};
use crate::theme::Palette;

#[derive(Debug)]
pub struct AboutDialog;

impl AboutDialog {
    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::new(palette, "Licenses\u{2026}");
        let open = super::dialog_modal_sized(
            ctx,
            "about-modal",
            "About",
            palette,
            super::WIDE_MODAL_WIDTH,
            |ui| {
                ui.label(crate::app::about_text());
            },
            |ui| footer.show(ui),
        );
        // The primary button opens the Licenses list; About stays open behind it.
        if footer.primary_clicked() {
            actions.push(Action::Ui(UiAction::Licenses));
        }
        open && !footer.closed()
    }
}
