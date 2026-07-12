//! Modal message boxes (wx `MessageDialog` / `error_alert`).
//!
//! Alerts queue up -- a load can produce both the auto-trim and the mismatch
//! box -- and are shown one at a time, frontmost first.

use std::collections::VecDeque;

use crate::theme::{Palette, bevel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub message: String,
}

impl Alert {
    #[must_use]
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
        }
    }

    /// `ui_util.error_alert`'s default title.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::new("Error", message)
    }
}

/// Renders the front of the queue as a modal, popping it when dismissed.
pub fn show_front(ctx: &egui::Context, palette: &Palette, alerts: &mut VecDeque<Alert>) {
    let Some(alert) = alerts.front() else {
        return;
    };
    let mut dismissed = false;
    let modal = egui::Modal::new(egui::Id::new("alert-modal")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(&alert.title);
        ui.separator();
        ui.label(&alert.message);
        ui.add_space(8.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if bevel::button(ui, palette, "OK").clicked() {
                dismissed = true;
            }
        });
    });
    if dismissed || modal.should_close() {
        alerts.pop_front();
    }
}
