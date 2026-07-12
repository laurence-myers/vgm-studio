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
pub mod vgm_metadata;

pub use dro_info::DroInfoDialog;
pub use find_reg::FindRegDialog;
pub use gd3_tag::Gd3TagDialog;
pub use goto::GotoDialog;
pub use settings::SettingsDialog;
pub use vgm_metadata::VgmMetadataDialog;

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
}

impl Dialogs {
    /// Draws every open dialog, dropping the ones that closed.
    pub fn show_all(&mut self, ctx: &egui::Context, actions: &mut Vec<crate::action::Action>) {
        retain(&mut self.goto, |d| d.show(ctx, actions));
        retain(&mut self.find_reg, |d| d.show(ctx, actions));
        retain(&mut self.dro_info, |d| d.show(ctx, actions));
        retain(&mut self.gd3_tag, |d| d.show(ctx, actions));
        retain(&mut self.vgm_metadata, |d| d.show(ctx, actions));
        retain(&mut self.settings, |d| d.show(ctx, actions));
    }
}

fn retain<T>(slot: &mut Option<T>, mut show: impl FnMut(&mut T) -> bool) {
    if let Some(dialog) = slot.as_mut() {
        if !show(dialog) {
            *slot = None;
        }
    }
}
