//! The application's dialogs (`dialogs.py`, `gd3_tag_dialog.py`,
//! `vgm_metadata_dialog.py`, plus the new Settings dialog).
//!
//! Modality follows the Python: DRO Info is modal; the rest are modeless
//! windows. Each is a plain struct created at open (capturing whatever song
//! state it edits), drawn every frame while open, and emitting [`Action`]s.
//!
//! [`Action`]: crate::action::Action

pub mod dro_info;
pub mod find_reg;
pub mod gd3_tag;
pub mod goto;
pub mod settings;
pub mod track_edit;
pub mod vgm_metadata;

pub use dro_info::DroInfoDialog;
pub use find_reg::FindRegDialog;
pub use gd3_tag::Gd3TagDialog;
pub use goto::GotoDialog;
pub use settings::SettingsDialog;
pub use track_edit::TrackEditDialog;
pub use vgm_metadata::VgmMetadataDialog;

/// Shared modeless-dialog chrome: a non-resizable, non-collapsible egui window
/// with a native close (✕) button, constrained to `area`. Runs `body`, then
/// returns whether the window is still open (its ✕ was not clicked). Each
/// dialog ANDs this with its own Close-button flag: `dialog_window(..) && !close`.
pub(crate) fn dialog_window(
    ctx: &egui::Context,
    title: &str,
    area: egui::Rect,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut open = true;
    egui::Window::new(title)
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .constrain_to(area)
        .show(ctx, body);
    open
}

/// The shared right-aligned footer button row: laid out right-to-left (so the
/// first button drawn sits rightmost) with 10px between buttons.
pub(crate) fn dialog_footer(ui: &mut egui::Ui, buttons: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        buttons(ui);
    });
}

/// The open dialogs. One of each at most -- reopening replaces the instance,
/// as the Python did for Goto and Find Register.
#[derive(Debug, Default)]
pub struct Dialogs {
    pub goto: Option<GotoDialog>,
    pub find_reg: Option<FindRegDialog>,
    pub dro_info: Option<DroInfoDialog>,
    pub gd3_tag: Option<Gd3TagDialog>,
    pub vgm_metadata: Option<VgmMetadataDialog>,
    pub settings: Option<SettingsDialog>,
    /// Rip mode's per-track quick edit (rename + GD3).
    pub track_edit: Option<TrackEditDialog>,
}

impl Dialogs {
    /// Draws every open dialog, dropping the ones that closed.
    ///
    /// `area`: where the modeless windows may live. Since egui 0.35, panels
    /// drawn into the app's `Ui` no longer reserve space, so windows would
    /// otherwise auto-place at the top of the viewport, over the menu bar.
    /// (DRO Info is a centred modal and ignores it, like the alerts.)
    pub fn show_all(
        &mut self,
        ctx: &egui::Context,
        palette: &crate::theme::Palette,
        area: egui::Rect,
        actions: &mut Vec<crate::action::Action>,
    ) {
        retain(&mut self.goto, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.find_reg, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.dro_info, |d| d.show(ctx, palette, actions));
        retain(&mut self.gd3_tag, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.vgm_metadata, |d| {
            d.show(ctx, palette, area, actions)
        });
        retain(&mut self.settings, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.track_edit, |d| {
            d.show(ctx, palette, area, actions)
        });
    }
}

fn retain<T>(slot: &mut Option<T>, mut show: impl FnMut(&mut T) -> bool) {
    if let Some(dialog) = slot.as_mut()
        && !show(dialog)
    {
        *slot = None;
    }
}
