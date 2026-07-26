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
pub mod screenshot_rename;
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
pub use screenshot_rename::ScreenshotRenameDialog;
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
    let width = modal_width(ctx);
    let modal = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading(title);
        crate::theme::separator_clipped(ui, palette);
        ui.add_space(6.0);
        body(ui);
    });
    !modal.should_close()
}

/// The width a modal lays its content out at: [`MODAL_WIDTH`], or as much of a
/// narrower window as fits.
///
/// Stated rather than measured. The free-text fields inside fill the dialog and
/// wrap at its edge, so letting the content decide the width would be circular
/// -- and a box that resized itself as you typed into it would be worse than
/// either.
fn modal_width(ctx: &egui::Context) -> f32 {
    /// Wide enough for a label column plus a value that reads as a line of
    /// text, and still a dialog rather than a window on a laptop screen.
    const MODAL_WIDTH: f32 = 560.0;
    /// Screen margin left around a modal on a window too narrow for the above.
    const SCREEN_MARGIN: f32 = 48.0;
    (ctx.content_rect().width() - SCREEN_MARGIN).clamp(240.0, MODAL_WIDTH)
}

/// A dialog text field that wraps instead of hiding what does not fit.
///
/// `egui`'s single-line edit clips: a value wider than its box scrolls out of
/// sight, and can only be read by dragging the cursor through it. This is the
/// multiline edit -- the only one that wraps -- held to a one-line value: it
/// starts one row tall, grows downwards as the text wraps, and takes no line
/// breaks (Enter does nothing; a pasted one is dropped).
///
/// `width` is where the text wraps. Free-text fields pass [`f32::INFINITY`],
/// which fills the dialog, so they wrap at its edge; fields holding a number
/// keep a width of their own.
pub(crate) fn text_field(
    ui: &mut egui::Ui,
    palette: &crate::theme::Palette,
    value: &mut String,
    width: f32,
) -> egui::Response {
    let response = ui.add(wrapping_edit(value, palette, width, 1).return_key(None));
    if response.changed() {
        value.retain(|c| c != '\n' && c != '\r');
    }
    response
}

/// The widget behind [`text_field`], for the callers that dress it further --
/// a hint, a disabled state, a colour of their own, or (the GD3 notes) real
/// multi-line editing at `rows` tall.
pub(crate) fn wrapping_edit<'t>(
    value: &'t mut String,
    palette: &crate::theme::Palette,
    width: f32,
    rows: usize,
) -> egui::TextEdit<'t> {
    egui::TextEdit::multiline(value)
        .text_color(palette.data_text)
        .desired_width(width)
        .desired_rows(rows)
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
    /// Pack mode's screenshot rename (named after the game, or a variant of it).
    pub screenshot_rename: Option<ScreenshotRenameDialog>,
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
            || self.screenshot_rename.is_some()
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
        retain(&mut self.screenshot_rename, |d| {
            d.show(ctx, palette, actions)
        });
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

#[cfg(test)]
mod tests {
    use dro_core::config::ThemeChoice;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable as _;

    /// Drives a lone [`super::text_field`] over `value`.
    fn harness(value: &str) -> egui_kittest::Harness<'static, String> {
        let palette = crate::theme::palette(ThemeChoice::Navy);
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, value: &mut String| {
                super::text_field(ui, palette, value, 200.0);
            },
            value.to_owned(),
        );
        // The field must have the keyboard before it can be typed into.
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run();
        harness
    }

    /// The one-line fields are built on egui's *multiline* edit, since that is
    /// the only one that wraps. Enter must not come with it: a game name split
    /// across two lines is not a value any of these dialogs can write.
    #[test]
    fn a_one_line_field_takes_no_typed_newline() {
        let mut harness = harness("Boss");
        harness.key_press(egui::Key::Enter);
        harness.run();
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("Battle");
        harness.run();
        assert_eq!(harness.state(), "BossBattle");
    }

    /// ...nor a pasted one, which the widget would otherwise take whole.
    #[test]
    fn a_one_line_field_drops_pasted_newlines() {
        let mut harness = harness("");
        harness
            .input_mut()
            .events
            .push(egui::Event::Paste("Boss\r\nBattle".to_owned()));
        harness.run();
        assert_eq!(harness.state(), "BossBattle");
    }
}
