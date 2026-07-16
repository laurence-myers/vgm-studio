//! Modal message boxes (wx `MessageDialog` / `error_alert`).
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

    /// `ui_util.error_alert`'s default title.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::new("Error", message)
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
    let modal = egui::Modal::new(egui::Id::new("alert-modal")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(&alert.title);
        ui.separator();
        ui.label(&alert.message);
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_confirm {
                if bevel::button(ui, palette, "Cancel").clicked() {
                    dismissed = true;
                }
                if bevel::button(ui, palette, "OK").clicked() {
                    confirmed = true;
                }
            } else if bevel::button(ui, palette, "OK").clicked() {
                dismissed = true;
            }
        });
    });
    if confirmed {
        if let Some(alert) = alerts.pop_front() {
            if let Some(action) = alert.confirm {
                actions.push(*action);
            }
        }
    } else if dismissed || modal.should_close() {
        // Esc or a backdrop click cancels a confirm box (no action runs).
        alerts.pop_front();
    }
}
