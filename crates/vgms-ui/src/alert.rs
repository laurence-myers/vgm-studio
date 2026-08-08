//! Modal message boxes.
//!
//! Alerts queue up -- a load can produce both the auto-trim and the mismatch
//! box -- and are shown one at a time, frontmost first. A confirm alert also
//! carries an [`Action`] to run if the user accepts (a dirty-discard prompt, a
//! pre-export warning).

use std::collections::VecDeque;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub message: String,
    /// `Some` makes this a confirm box (OK/Cancel); accepting runs the action.
    pub confirm: Option<Box<Action>>,
}

impl Alert {
    #[must_use]
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm: None,
        }
    }

    /// The default error-alert title.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(crate::strings::ALERT_ERROR_TITLE, message)
    }

    /// A confirm box: OK runs `action`, Cancel (or Esc) dismisses it.
    #[must_use]
    pub fn confirm(title: impl Into<String>, message: impl Into<String>, action: Action) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm: Some(Box::new(action)),
        }
    }
}

/// Renders the front of the queue as a modal, popping it when dismissed. A
/// confirmed confirm-box pushes its carried action onto `actions`.
pub fn show_front(
    ctx: &egui::Context,
    palette: &Palette,
    alerts: &mut VecDeque<Alert>,
    actions: &mut Vec<Action>,
) {
    let Some(alert) = alerts.front() else {
        return;
    };
    let is_confirm = alert.confirm.is_some();
    let mut dismissed = false;
    let mut confirmed = false;
    // Leave room for the heading, the divider, the button row and the window
    // margins; the body gets whatever is left of the screen.
    const CHROME_HEIGHT: f32 = 180.0;
    let body_height = (ctx.content_rect().height() - CHROME_HEIGHT).max(120.0);
    // Keep the alert on the panel `face`; the lifted `window_fill` is for
    // small tooltips, not whole message boxes.
    let style = ctx.style_of(ctx.theme());
    let frame = egui::Frame::popup(&style)
        .fill(palette.face)
        .stroke(egui::Stroke::new(1.0, palette.bevel_dark));
    let modal = egui::Modal::new(egui::Id::new("alert-modal"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(&alert.title);
            ui.separator();
            // The message can be a long list -- the pre-export prompt names every
            // check that did not pass -- so cap it and scroll rather than letting
            // the box grow until its buttons are off the bottom of the screen.
            // Height shrinks to fit, so a one-line alert is still a small box.
            let output = egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.label(&alert.message);
                });
            crate::theme::frame_scroll_output(ui, palette, output.inner_rect, output.content_size);
            ui.add_space(8.0);
            // Wrapped in `ui.horizontal` so the right-to-left button layout is
            // confined to a single row. Without it the layout claims all the
            // vertical space left in the modal and centres the buttons in it -- a
            // one-line prompt then renders as a tall box with OK/Cancel floating in
            // the middle. (The same rule the shared `dialog_footer` documents.)
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_confirm && bevel::button(ui, palette, "Cancel").clicked() {
                        dismissed = true;
                    }
                    // OK is drawn last (rightmost of the pair in this right-to-left
                    // row) and focused on open, so Enter accepts the box.
                    let ok = bevel::button(ui, palette, "OK");
                    if ok.clicked() {
                        if is_confirm {
                            confirmed = true;
                        } else {
                            dismissed = true;
                        }
                    }
                    if ui.memory(|memory| memory.focused().is_none()) {
                        ok.request_focus();
                    }
                });
            });
        });
    // Enter is OK: it confirms a confirm box and dismisses an info box.
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    if confirmed || (is_confirm && enter) {
        if let Some(alert) = alerts.pop_front()
            && let Some(action) = alert.confirm
        {
            actions.push(*action);
        }
    } else if dismissed || (!is_confirm && enter) || modal.should_close() {
        // Esc or a backdrop click cancels a confirm box (no action runs).
        alerts.pop_front();
    }
}
