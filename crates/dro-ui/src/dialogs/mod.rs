//! The application's dialogs.
//!
//! All of them are modal except Goto and Find Register, which stay modeless
//! windows: those two drive the editor behind them (jumping to a position,
//! stepping through matches), so blocking it would defeat the point. Each
//! dialog is a plain struct created at open (capturing whatever song state it
//! edits), drawn every frame while open, and emitting [`Action`]s.
//!
//! [`Action`]: crate::action::Action

pub mod bulk_tag;
pub mod dro_info;
pub mod find_loop;
pub mod find_reg;
pub mod gd3_tag;
pub mod goto;
pub mod render_wav;
pub mod settings;
pub mod split;
pub mod split_songs;
pub mod track_edit;
pub mod vgm_metadata;

pub use bulk_tag::BulkTagDialog;
pub use dro_info::DroInfoDialog;
pub use find_loop::FindLoopDialog;
pub use find_reg::FindRegDialog;
pub use gd3_tag::Gd3TagDialog;
pub use goto::GotoDialog;
pub use render_wav::RenderWavDialog;
pub use settings::SettingsDialog;
pub use split::SplitDialog;
pub use split_songs::SplitSongsDialog;
pub use track_edit::TrackEditDialog;
pub use vgm_metadata::VgmMetadataDialog;

/// Shared modeless-dialog chrome, used by Goto and Find Register: a
/// non-resizable, non-collapsible egui window with a native close (✕) button,
/// constrained to `area`. Runs `body`, then returns whether the window is still
/// open (its ✕ was not clicked). Each dialog ANDs this with its own Close-button
/// flag: `dialog_window(..) && !close`. Every other dialog is a
/// [`dialog_modal`].
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

/// Shared modal-dialog chrome: a centred [`egui::Modal`] with the dialog's
/// title as a heading, the usual groove under it, and `body` below. Esc or a
/// click on the backdrop dismisses it, as on the alert boxes -- a modal has no
/// title bar, so there is no ✕ to close it with. Returns whether the dialog is
/// still open; each dialog ANDs this with its own Close-button flag:
/// `dialog_modal(..) && !close`.
///
/// `id` must be unique per dialog. The body is *not* wrapped in a scroll area:
/// one around the whole modal stops clicks inside it registering at all. A
/// modal cannot be dragged out of the way either, so a dialog whose content
/// grows with the song caps and scrolls that part itself -- Find Loop and Split
/// Songs cap their result tables, and Bulk Tag scrolls its own body between a
/// pinned heading and footer.
pub(crate) fn dialog_modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    palette: &crate::theme::Palette,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let modal = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.heading(title);
        crate::theme::separator_clipped(ui, palette);
        ui.add_space(6.0);
        body(ui);
    });
    !modal.should_close()
}

/// The shared right-aligned footer button row: laid out right-to-left (so the
/// first button drawn sits rightmost) with 10px between buttons.
///
/// Wrapped in a `horizontal` so the right-to-left layout is confined to one
/// row's height. On its own it claims whatever vertical space is left in the
/// window and centres the buttons in it, which a dialog with little content
/// renders as a tall box with its buttons floating in the middle.
pub(crate) fn dialog_footer(ui: &mut egui::Ui, buttons: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            buttons(ui);
        });
    });
}

/// The open dialogs. One of each at most -- reopening replaces the instance.
#[derive(Debug, Default)]
pub struct Dialogs {
    pub goto: Option<GotoDialog>,
    pub find_reg: Option<FindRegDialog>,
    /// Edit > Find Loop: the loop-point search and its results.
    pub find_loop: Option<FindLoopDialog>,
    pub dro_info: Option<DroInfoDialog>,
    pub gd3_tag: Option<Gd3TagDialog>,
    pub vgm_metadata: Option<VgmMetadataDialog>,
    pub settings: Option<SettingsDialog>,
    /// Pack mode's per-track quick edit (rename + GD3).
    pub track_edit: Option<TrackEditDialog>,
    /// Pack mode's bulk GD3 editor (chosen fields, chosen tracks).
    pub bulk_tag: Option<BulkTagDialog>,
    /// File > Render to WAV: which of the editor's mix settings to apply.
    pub render_wav: Option<RenderWavDialog>,
    /// File > Split Channels: the output format and percussion handling.
    pub split: Option<SplitDialog>,
    /// File > Split Songs: the gap threshold and per-song include flags.
    pub split_songs: Option<SplitSongsDialog>,
}

impl Dialogs {
    /// Whether any dialog is open. While one is, the editor's keyboard shortcuts
    /// are suppressed: its text fields own the keyboard, and -- unlike
    /// `egui_wants_keyboard_input` -- a chrome button merely holding focus (after
    /// a stray Tab) must not, since the editor view has no text inputs of its own.
    #[must_use]
    pub fn any_open(&self) -> bool {
        self.goto.is_some()
            || self.find_reg.is_some()
            || self.find_loop.is_some()
            || self.dro_info.is_some()
            || self.gd3_tag.is_some()
            || self.vgm_metadata.is_some()
            || self.settings.is_some()
            || self.track_edit.is_some()
            || self.bulk_tag.is_some()
            || self.render_wav.is_some()
            || self.split.is_some()
            || self.split_songs.is_some()
    }

    /// Draws every open dialog, dropping the ones that closed.
    ///
    /// `area`: where the two modeless windows may live. Since egui 0.35, panels
    /// drawn into the app's `Ui` no longer reserve space, so a window would
    /// otherwise auto-place at the top of the viewport, over the menu bar. The
    /// modals are centred on the viewport and ignore it, like the alerts.
    pub fn show_all(
        &mut self,
        ctx: &egui::Context,
        palette: &crate::theme::Palette,
        area: egui::Rect,
        actions: &mut Vec<crate::action::Action>,
    ) {
        retain(&mut self.goto, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.find_reg, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.find_loop, |d| d.show(ctx, palette, actions));
        retain(&mut self.dro_info, |d| d.show(ctx, palette, actions));
        retain(&mut self.gd3_tag, |d| d.show(ctx, palette, actions));
        retain(&mut self.vgm_metadata, |d| d.show(ctx, palette, actions));
        retain(&mut self.settings, |d| d.show(ctx, palette, actions));
        retain(&mut self.track_edit, |d| d.show(ctx, palette, actions));
        retain(&mut self.bulk_tag, |d| d.show(ctx, palette, area, actions));
        retain(&mut self.render_wav, |d| d.show(ctx, palette, actions));
        retain(&mut self.split, |d| d.show(ctx, palette, actions));
        retain(&mut self.split_songs, |d| d.show(ctx, palette, actions));
    }
}

fn retain<T>(slot: &mut Option<T>, mut show: impl FnMut(&mut T) -> bool) {
    if let Some(dialog) = slot.as_mut()
        && !show(dialog)
    {
        *slot = None;
    }
}
