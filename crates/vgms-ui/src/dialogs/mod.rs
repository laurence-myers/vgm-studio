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
pub mod help;
pub mod render_wav;
pub mod screenshot_rename;
pub mod settings;
pub mod split;
pub mod split_songs;
pub mod track_edit;
pub mod track_optimize;
pub mod unwalkable_vgm;
pub mod vgm_metadata;

pub use bulk_tag::BulkTagDialog;
pub use dro_info::DroInfoDialog;
pub use find_loop::{FindLoopDialog, LoopSearchDoc};
pub use find_reg::FindRegDialog;
pub use gd3_tag::Gd3TagDialog;
pub use goto::GotoDialog;
pub use help::HelpDialog;
pub use render_wav::RenderWavDialog;
pub use screenshot_rename::ScreenshotRenameDialog;
pub use settings::{SettingsDialog, SongContext};
pub use split::SplitDialog;
pub use split_songs::SplitSongsDialog;
pub use track_edit::TrackEditDialog;
pub use track_optimize::TrackOptimizeDialog;
pub use unwalkable_vgm::UnwalkableVgmDialog;
pub use vgm_metadata::VgmMetadataDialog;

/// Shared modeless-dialog chrome, used by Goto and Find Register: a
/// non-resizable, non-collapsible egui window with a native close (✕) button,
/// constrained to `area`. Runs `body`, then returns whether the window is still
/// open (its ✕ was not clicked). Each dialog ANDs this with its own Close-button
/// flag: `dialog_window(..) && !close`. Every other dialog is a
/// [`dialog_modal`].
pub(crate) fn dialog_window(
    ctx: &egui::Context,
    palette: &crate::theme::Palette,
    title: &str,
    area: egui::Rect,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut open = true;
    // Keep the window on the panel `face`, as [`dialog_modal_sized`] does: the
    // lifted `window_fill` is for small tooltips, not whole dialogs.
    let style = ctx.style_of(ctx.theme());
    let frame = egui::Frame::window(&style)
        .fill(palette.face)
        .stroke(egui::Stroke::new(1.0, palette.bevel_dark));
    egui::Window::new(title)
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .frame(frame)
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
/// `id` must be unique per dialog.
///
/// The body scrolls when it is taller than the window can hold, with the heading
/// and `footer` (the button row) pinned above and below so Save and Close stay
/// reachable however short the window gets. This is the backstop for when the
/// *window* is too small; dialogs whose content grows with the song still cap
/// their own tables (Find Loop, Split Songs) and Bulk Tag scrolls its own list.
pub(crate) fn dialog_modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    palette: &crate::theme::Palette,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) -> bool {
    dialog_modal_sized(ctx, id, title, palette, MODAL_WIDTH, body, footer)
}

/// As [`dialog_modal`], but laid out at `width` rather than the usual one --
/// for a dialog that is a reference table read across rather than a form filled
/// in down. Still narrowed to fit a window that cannot hold it.
pub(crate) fn dialog_modal_sized(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    palette: &crate::theme::Palette,
    width: f32,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) -> bool {
    let width = modal_width(ctx, width);
    // What is left for the body once the window's margins, the heading, its
    // groove and the pinned footer are accounted for. Generous rather than
    // exact: the cost of over-estimating is a dialog that runs a little closer
    // to the window edge.
    let max_height = (ctx.content_rect().height() - 140.0).max(120.0);
    let id = egui::Id::new(id);
    // Keep the modal on the panel `face`: the theme lifts `window_fill` off it
    // to give small tooltips a contrasting surface, but a whole dialog reads
    // fine flush with the panels and should not shift with that.
    let style = ctx.style_of(ctx.theme());
    let frame = egui::Frame::popup(&style)
        .fill(palette.face)
        .stroke(egui::Stroke::new(1.0, palette.bevel_dark));
    let modal = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        ui.heading(title);
        crate::theme::separator_clipped(ui, palette);
        ui.add_space(6.0);
        // The body always lives in the same scroll viewport, capped at
        // `max_height`: it shrinks to the content when that fits (a short dialog
        // stays short) and scrolls when it does not (a tall one never runs off
        // the screen). The wrapper is *unconditional* on purpose. Switching
        // between a scroll area and a bare scope by last frame's height gave any
        // id-keyed widget inside -- a `CollapsingHeader`'s open state -- a
        // different id per branch: it read open in one and closed in the other,
        // so the measured height flipped every frame and the dialog flickered
        // between the two. `auto_shrink([false, true])`: fill the width, shrink
        // the height to the content up to the cap.
        let output = egui::ScrollArea::vertical()
            .max_height(max_height)
            .auto_shrink([false, true])
            .show(ui, body);
        crate::theme::frame_scroll_output(ui, palette, output.inner_rect, output.content_size);
        // The footer sits outside the scrolled viewport, so the buttons are on
        // screen whatever the body's height.
        ui.add_space(8.0);
        footer(ui);
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
fn modal_width(ctx: &egui::Context, wanted: f32) -> f32 {
    /// Screen margin left around a modal on a window too narrow for `wanted`.
    const SCREEN_MARGIN: f32 = 48.0;
    (ctx.content_rect().width() - SCREEN_MARGIN).clamp(240.0, wanted)
}

/// Wide enough for a label column plus a value that reads as a line of text,
/// and still a dialog rather than a window on a laptop screen.
const MODAL_WIDTH: f32 = 560.0;

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

/// Which of a [`Footer`]'s buttons the user pressed this frame.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FooterClick {
    /// Neither button was pressed.
    #[default]
    None,
    /// The primary (affirmative) button -- Save, Render, Split, and so on.
    Primary,
    /// The Close button.
    Close,
}

/// The common dialog footer: an optional primary (affirmative) button and a
/// Close button, drawn in the shared right-to-left row so the primary sits left
/// of Close, and reporting which was pressed.
///
/// This is the answer to the borrow puzzle every modal used to hand-roll: the
/// body closure passed to [`dialog_modal`] borrows the dialog `&mut`, so the
/// footer closure cannot also touch it. A `Footer` is built *before* the modal,
/// its [`show`](Footer::show) drawn inside the footer closure, and the button
/// pressed read back with [`clicked`](Footer::clicked) *after* the modal returns
/// -- so each dialog decides for itself what a click means (whether Save closes,
/// what to emit) without a scatter of `Cell<bool>` flags.
///
/// Dialogs that need more than one action plus Close (Find Loop's Apply and
/// Audition, DRO Info's Edit/Save toggle) draw [`dialog_footer`] themselves.
pub(crate) struct Footer<'a> {
    palette: &'a crate::theme::Palette,
    /// The affirmative button's label; `None` for a Close-only footer.
    primary: Option<&'a str>,
    click: std::cell::Cell<FooterClick>,
}

impl<'a> Footer<'a> {
    /// A footer with an affirmative button labelled `primary`, plus Close.
    pub(crate) fn new(palette: &'a crate::theme::Palette, primary: &'a str) -> Self {
        Self {
            palette,
            primary: Some(primary),
            click: std::cell::Cell::new(FooterClick::None),
        }
    }

    /// A footer with only a Close button.
    pub(crate) fn close_only(palette: &'a crate::theme::Palette) -> Self {
        Self {
            palette,
            primary: None,
            click: std::cell::Cell::new(FooterClick::None),
        }
    }

    /// Draws the footer. Call inside [`dialog_modal`]'s footer closure.
    pub(crate) fn show(&self, ui: &mut egui::Ui) {
        dialog_footer(ui, |ui| {
            if crate::theme::bevel::button(ui, self.palette, "Close").clicked() {
                self.click.set(FooterClick::Close);
            }
            if let Some(label) = self.primary
                && crate::theme::bevel::button(ui, self.palette, label).clicked()
            {
                self.click.set(FooterClick::Primary);
            }
        });
    }

    /// Which button was pressed this frame.
    pub(crate) fn clicked(&self) -> FooterClick {
        self.click.get()
    }

    /// Whether the primary (affirmative) button was pressed.
    pub(crate) fn primary_clicked(&self) -> bool {
        self.click.get() == FooterClick::Primary
    }

    /// Whether the Close button was pressed.
    pub(crate) fn closed(&self) -> bool {
        self.click.get() == FooterClick::Close
    }
}

/// Where a [`caption_checkbox`]'s caption sits relative to the box it toggles.
pub(crate) enum CaptionSide {
    /// Checkbox first, then the caption, wrapped in a `horizontal` -- a standalone
    /// row, as the Render and Split dialogs use.
    Row,
    /// Caption first, then the checkbox, emitted as two cells of the enclosing
    /// grid, as the Settings rows use.
    GridLeft,
}

/// A checkbox whose caption also toggles it. egui's plain label is inert, so
/// without this clicking the caption does nothing; every toolkit lets you hit the
/// label. `hover` is shown on the caption (empty for none, as the grid rows use).
pub(crate) fn caption_checkbox(
    ui: &mut egui::Ui,
    caption: &str,
    hover: &str,
    value: &mut bool,
    side: CaptionSide,
) {
    fn clickable_caption(ui: &mut egui::Ui, caption: &str, hover: &str, value: &mut bool) {
        let response = ui.add(egui::Label::new(caption).sense(egui::Sense::click()));
        let response = if hover.is_empty() {
            response
        } else {
            response.on_hover_text(hover)
        };
        if response.clicked() {
            *value = !*value;
        }
    }
    match side {
        CaptionSide::Row => {
            ui.horizontal(|ui| {
                ui.checkbox(value, "");
                clickable_caption(ui, caption, hover, value);
            });
        }
        CaptionSide::GridLeft => {
            clickable_caption(ui, caption, hover, value);
            ui.checkbox(value, "");
        }
    }
}

/// Formats a `native`-unit time as `M:SS.s`, given the unit's per-second `rate`.
///
/// Shared by Find Loop (millisecond offsets, `rate = 1000`) and Split Songs
/// (sample offsets, `rate` = the song's sample rate). Distinct from
/// [`vgms_core::util::ms_to_timestr`] (`MM:SS`, no fraction) and
/// [`vgms_core::pack::format_track_time`] (`M:SS` from a sample count): this one
/// keeps the tenth-of-a-second the loop and split tables read to.
pub(crate) fn fmt_time(native: u32, rate: u32) -> String {
    let total_secs = f64::from(native) / f64::from(rate);
    let minutes = (total_secs / 60.0).floor() as u32;
    let seconds = total_secs - f64::from(minutes) * 60.0;
    format!("{minutes}:{seconds:04.1}")
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
    /// Pack mode's per-track optimiser-options override.
    pub track_optimize: Option<TrackOptimizeDialog>,
    /// Pack mode's bulk GD3 editor (chosen fields, chosen tracks).
    pub bulk_tag: Option<BulkTagDialog>,
    /// Pack mode's screenshot rename (named after the game, or a variant of it).
    pub screenshot_rename: Option<ScreenshotRenameDialog>,
    /// Help > Help: what every key and gesture does.
    pub help: Option<HelpDialog>,
    /// File > Render to WAV: which of the editor's mix settings to apply.
    pub render_wav: Option<RenderWavDialog>,
    /// File > Split Channels: the output format and percussion handling.
    pub split: Option<SplitDialog>,
    /// File > Split Songs: the gap threshold and per-song include flags.
    pub split_songs: Option<SplitSongsDialog>,
    /// What a VGM the editor cannot open actually is, and where to edit it.
    pub unwalkable_vgm: Option<UnwalkableVgmDialog>,
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
            || self.track_optimize.is_some()
            || self.bulk_tag.is_some()
            || self.screenshot_rename.is_some()
            || self.help.is_some()
            || self.render_wav.is_some()
            || self.split.is_some()
            || self.split_songs.is_some()
            || self.unwalkable_vgm.is_some()
    }

    /// Draws every open dialog, dropping the ones that closed.
    ///
    /// `area`: where the two modeless windows may live. Panels drawn into the
    /// app's `Ui` no longer reserve space, so a window would otherwise auto-place
    /// at the top of the viewport, over the menu bar. The modals are centred on
    /// the viewport and ignore it, like the alerts.
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
        retain(&mut self.track_optimize, |d| d.show(ctx, palette, actions));
        retain(&mut self.bulk_tag, |d| d.show(ctx, palette, actions));
        retain(&mut self.screenshot_rename, |d| {
            d.show(ctx, palette, actions)
        });
        retain(&mut self.help, |d| d.show(ctx, palette, actions));
        retain(&mut self.render_wav, |d| d.show(ctx, palette, actions));
        retain(&mut self.split, |d| d.show(ctx, palette, actions));
        retain(&mut self.split_songs, |d| d.show(ctx, palette, actions));
        retain(&mut self.unwalkable_vgm, |d| d.show(ctx, palette, actions));
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
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable as _;
    use vgms_core::config::ThemeChoice;

    #[test]
    fn fmt_time_reads_as_minutes_and_seconds() {
        // Samples at 44100 Hz.
        assert_eq!(super::fmt_time(0, 44_100), "0:00.0");
        assert_eq!(super::fmt_time(44_100, 44_100), "0:01.0");
        assert_eq!(super::fmt_time(44_100 * 75, 44_100), "1:15.0");
        // Milliseconds.
        assert_eq!(super::fmt_time(1000, 1000), "0:01.0");
        assert_eq!(super::fmt_time(75_000, 1000), "1:15.0");
    }

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
