//! The application: `wxapp.DTApp` + `containers.DTMainFrame`, as one
//! `eframe::App` driven entirely through the platform-service traits.

use core::fmt;
use core::time::Duration;
use std::collections::VecDeque;

use dro_core::FindTarget;
use dro_core::config::{AppConfig, ConfigStore};
use egui::Key;

use crate::action::Action;
use crate::alert::{self, Alert};
use crate::dialogs::{
    Dialogs, DroInfoDialog, FindRegDialog, Gd3TagDialog, GotoDialog, SettingsDialog,
    VgmMetadataDialog,
};
use crate::editor::{Editor, LoadReport};
use crate::menus::{self, MenuState};
use crate::platform::{AudioService, FileService, PickedFile, SaveOutcome, SaveRequest};
use crate::tasks::{TaskRequest, TaskResult, TaskService};
use crate::theme::{self, Palette};
use crate::widgets::position_panel::PositionPanel;
use crate::widgets::waveform::WaveformState;
use crate::widgets::{channels::ChannelPanel, table, waveform};

const AUTO_TRIM_TITLE: &str = "DRO auto-trimmed";
const AUTO_TRIM_TEXT: &str = "The DRO was found to contain a bogus delay as\n\
                              its first instruction. It has been automatically\n\
                              removed. (Don't forget to save!)";
const MISMATCH_TITLE: &str = "DRO timing mismatch";

const HELP_TITLE: &str = "Help";
const HELP_TEXT: &str = "Full instructions are available online.\n\
    https://github.com/laurence-myers/dro-trimmer\n\
    \n\
    1) Select an instruction.\n\
    2) Delete via button or the Del key.\n\
    3) Profit!\n\
    \n\
    If you're trimming a looping song, look for a\n\
    whole bunch of instructions with no delays, as\n\
    this might be where the instruments are set up.";

fn about_text() -> String {
    format!(
        "DRO Trimmer v{}\n\
         Laurence Dougal Myers\n\
         Web: http://www.jestarjokin.net/apps/drotrimmer\n\
         Web: https://github.com/laurence-myers/dro-trimmer\n\
         E-Mail: jestarjokin@jestarjokin.net\n\
         \n\
         Thanks to:\n\
         The DOSBOX team\n\
         The AdPlug team\n\
         Adam Nielsen for PyOPL\n\
         Nuke.YKT for Nuked-OPL3\n\
         Wraithverge for testing, feedback and contributions\n\
         pi-r-squared for their original attempt at a DRO editor\n\
         \n\
         This build embeds the Nuked-OPL3 emulator and is licensed\n\
         under the LGPL-2.1-or-later. Complete source code:\n\
         https://github.com/laurence-myers/dro-trimmer",
        env!("CARGO_PKG_VERSION"),
    )
}

/// The DRO timing mismatch box (`wxapp.__load_file`), version-specific advice
/// and all. The v2 advice now points at the Settings dialog instead of a hand
/// edit of drotrim.ini, since the port has one.
fn mismatch_alert(auto_trimmed: bool, file_version: u32) -> Alert {
    let prefix = if auto_trimmed {
        "Despite auto-trimming, t"
    } else {
        "T"
    };
    let advice = if file_version == dro_core::song::DRO_FILE_V1 {
        "Please re-save the file to use the calculated value."
    } else {
        "Please enable \"Allow editing in DRO Info\" in the\n\
         Settings dialog, then edit the song length on\n\
         the DRO Info screen."
    };
    Alert::new(
        MISMATCH_TITLE,
        format!(
            "{prefix}here was a mismatch between\n\
             the measured length of the song in milliseconds,\n\
             and the length stored in the DRO file.\n\
             {advice}"
        ),
    )
}

pub struct DroApp {
    editor: Editor,
    files: Box<dyn FileService>,
    audio: Box<dyn AudioService>,
    tasks: Box<dyn TaskService>,
    config_store: Box<dyn ConfigStore>,
    config: AppConfig,

    status: String,
    alerts: VecDeque<Alert>,
    dialogs: Dialogs,

    waveform: WaveformState,
    position: PositionPanel,
    channels: ChannelPanel,

    /// A row the table should scroll into view next frame (Python's
    /// `EnsureVisible`).
    scroll_to: Option<usize>,
    /// The last first-selected row, to detect selection changes (the Python
    /// list's `FirstSelectedItemChangedEvent`).
    last_first_selected: Option<usize>,
    /// The editor revision currently loaded into the audio service, if any.
    audio_revision: Option<u64>,
    /// Whether the previous frame was playing, so the frame after playback
    /// ends can display the exact final position.
    was_playing: bool,
    /// A file passed on the command line, loaded on the first frame.
    pending_open: Option<PickedFile>,
}

impl DroApp {
    #[must_use]
    pub fn new(
        files: Box<dyn FileService>,
        audio: Box<dyn AudioService>,
        tasks: Box<dyn TaskService>,
        config_store: Box<dyn ConfigStore>,
        initial_file: Option<PickedFile>,
    ) -> Self {
        let config = config_store.load();
        Self {
            editor: Editor::new(),
            files,
            audio,
            tasks,
            config_store,
            config,
            status: String::new(),
            alerts: VecDeque::new(),
            dialogs: Dialogs::default(),
            waveform: WaveformState::default(),
            position: PositionPanel::new(config.audio.frequency),
            channels: ChannelPanel::new(),
            scroll_to: None,
            last_first_selected: None,
            audio_revision: None,
            was_playing: false,
            pending_open: initial_file,
        }
    }

    /// The active colour scheme.
    fn palette(&self) -> &'static Palette {
        theme::palette(self.config.ui.theme)
    }

    fn update_impl(&mut self, ctx: &egui::Context) {
        if let Some(file) = self.pending_open.take() {
            self.load_file(file);
        }
        self.poll_services();
        self.handle_drops(ctx);

        let mut actions: Vec<Action> = Vec::new();
        self.gather_key_input(ctx, &mut actions);

        let p = self.palette();
        // Chrome panels sit on the face colour; the waveform is a data well, so
        // its margins take the main dark background rather than the chrome tint.
        let chrome = egui::Frame::side_top_panel(&ctx.style()).fill(p.face);
        // No side margins: the reset button owns the left edge and the waveform
        // runs flush to the right edge.
        let well = egui::Frame::side_top_panel(&ctx.style())
            .fill(p.data_bg)
            .inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 2,
                bottom: 2,
            });

        let menu = egui::TopBottomPanel::top("menu-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| {
                menus::bar(ui, p, &self.menu_state(), &mut actions);
            });
        let waveform = egui::TopBottomPanel::top("waveform")
            .frame(well)
            .resizable(true)
            .default_height(150.0)
            .min_height(80.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                let height = ui.available_height();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // A full-height "skip to start" transport button on the left.
                    if theme::bevel::button_sized(ui, p, "\u{23EE}", egui::vec2(34.0, height))
                        .on_hover_text("Rewind to the start")
                        .clicked()
                    {
                        actions.push(Action::RewindToStart);
                    }
                    let response = waveform::show(ui, &self.waveform, self.editor.song(), p);
                    if let Some((index, ms)) = response.clicked {
                        actions.push(Action::WaveformClicked { index, ms });
                    }
                });
            });
        let status = egui::TopBottomPanel::bottom("status-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.tasks.is_busy() {
                            ui.label("Rendering waveform...");
                        }
                    });
                });
            });
        let position = egui::TopBottomPanel::bottom("position-panel")
            .frame(chrome)
            .show_separator_line(false)
            .show(ctx, |ui| {
                self.position.show(ui, p);
            });
        // The controls own their vertical spacing (equal padding above and below
        // each row band), so drop the frame's vertical margin and item spacing.
        let controls_frame = egui::Frame::side_top_panel(&ctx.style())
            .fill(p.face)
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 0,
                bottom: 0,
            });
        let controls = egui::TopBottomPanel::bottom("controls")
            .frame(controls_frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                const PAD: f32 = 6.0;
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(PAD);
                ui.horizontal(|ui| {
                    ui.set_min_height(ui.spacing().interact_size.y);
                    ui.spacing_mut().item_spacing.x = 12.0;
                    if theme::bevel::button(ui, p, "Del.")
                        .on_hover_text("Delete the selected instruction(s)")
                        .clicked()
                    {
                        actions.push(Action::DeleteSelection);
                    }
                    if theme::bevel::button(ui, p, "Play")
                        .on_hover_text("Play the song from the current position")
                        .clicked()
                    {
                        actions.push(Action::Play);
                    }
                    if theme::bevel::button(ui, p, "Stop")
                        .on_hover_text("Stop playback")
                        .clicked()
                    {
                        actions.push(Action::Stop);
                    }
                    if theme::bevel::button(ui, p, "Tail")
                        .on_hover_text(self.play_tail_label())
                        .clicked()
                    {
                        actions.push(Action::PlayTail);
                    }
                });
                ui.add_space(PAD);
                theme::separator_full(ui, p);
                ui.add_space(PAD);
                // A plain OPL2 song has only one bank; hide the high-bank toggles.
                let show_high_bank = self
                    .editor
                    .song()
                    .is_none_or(|song| song.opl_type != dro_core::OplType::Opl2);
                if self.channels.show(ui, p, show_high_bank) {
                    actions.push(Action::MutingChanged);
                }
                ui.add_space(PAD);
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(p.data_bg))
            .show(ctx, |ui| {
                if self.editor.has_song() {
                    // Row hover reads `widgets.hovered.bg_fill`, which is the
                    // bright face colour; scope it to the data-well tone so it
                    // does not flash teal under the yellow text.
                    ui.visuals_mut().widgets.hovered.bg_fill = p.data_hover;
                    table::show(ui, &mut self.editor, self.scroll_to.take(), p);
                } else {
                    ui.visuals_mut().override_text_color = Some(p.data_label);
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            "Open a DRO, VGM or VGZ file (File > Open..., or drop it here).",
                        );
                    });
                }
            });

        // 2px beveled grooves at the seams between the stacked panels. Painted
        // into the shared background layer *after* the panels, so they sit over
        // the panel content but below every Window/menu/popup (which live in
        // higher orders) -- an ad-hoc Middle layer would draw over dialogs. The
        // waveform panel is resizable, so the seams are recomputed each frame.
        let divider = ctx.layer_painter(egui::LayerId::background());
        let x_range = ctx.screen_rect().x_range();
        for seam in [
            menu.response.rect.bottom(),
            waveform.response.rect.bottom(),
            controls.response.rect.top(),
            position.response.rect.top(),
            status.response.rect.top(),
        ] {
            theme::bevel::groove_h(&divider, x_range, seam - 1.0, p);
        }

        self.dialogs.show_all(ctx, p, &mut actions);
        alert::show_front(ctx, p, &mut self.alerts);

        for action in actions {
            self.handle_action(ctx, action);
        }

        self.sync_selection_indicator();
        self.playback_tick(ctx);
    }

    // -- frame plumbing ------------------------------------------------------

    fn poll_services(&mut self) {
        if let Some(result) = self.files.poll_picked() {
            match result {
                Ok(file) => self.load_file(file),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new("Failed to open file", message)),
            }
        }
        if let Some(outcome) = self.files.poll_saved() {
            match outcome {
                SaveOutcome::Saved { name, path } => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    // Save As can change the .vgm/.vgz extension after the
                    // bytes were serialised; re-save once so the compression
                    // matches the chosen name.
                    if self.editor.record_saved(name, path) {
                        if let (Ok(bytes), Some(path)) =
                            (self.editor.save_bytes(), self.editor.path.clone())
                        {
                            self.files.save(SaveRequest::InPlace { path, bytes });
                        }
                    }
                    self.status = format!("File saved to {shown}.");
                }
                SaveOutcome::Cancelled => {}
                SaveOutcome::Failed(message) => self
                    .alerts
                    .push_back(Alert::new("Failed to save file", message)),
            }
        }
        for result in self.tasks.poll() {
            match result {
                TaskResult::Waveform(buckets) => self.waveform.buckets = buckets,
            }
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        // Only single-file drops, as in Python.
        if dropped.len() != 1 {
            return;
        }
        let file = dropped.into_iter().next().expect("len checked");
        let name = file
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.name.clone());
        let lower = name.to_ascii_lowercase();
        // Divergence: the Python accepted only .dro drops; every format the
        // open dialog supports is accepted here.
        if !(lower.ends_with(".dro") || lower.ends_with(".vgm") || lower.ends_with(".vgz")) {
            return;
        }
        if let Some(bytes) = file.bytes {
            // The web path: eframe delivers the dropped file's contents.
            self.load_file(PickedFile {
                name,
                path: None,
                bytes: bytes.to_vec(),
            });
        } else if let Some(path) = file.path {
            self.files.open_path(path);
        }
    }

    fn gather_key_input(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // A modal (an alert box, or the DRO Info dialog) blocks the app, as
        // wx's modal dialogs did -- without this, Space would start playback
        // behind the message box.
        if !self.alerts.is_empty() || self.dialogs.dro_info.is_some() {
            return;
        }
        // A focused widget owns the keyboard: Ctrl+Z in a tag field must undo
        // the *text*, not the song. (The wx accelerators likewise never fired
        // inside the dialogs' own text fields.)
        if ctx.wants_keyboard_input() {
            return;
        }
        ctx.input_mut(|input| {
            // egui's shortcut matching ignores a surplus Shift, so the
            // shifted variants must be consumed before their plain forms.
            if input.consume_shortcut(&menus::SAVE_AS) {
                actions.push(Action::SaveAs);
            }
            if input.consume_shortcut(&menus::SAVE) {
                actions.push(Action::Save);
            }
            if input.consume_shortcut(&menus::REDO_ALT) {
                actions.push(Action::Redo);
            }
            if input.consume_shortcut(&menus::UNDO) {
                actions.push(Action::Undo);
            }
            if input.consume_shortcut(&menus::REDO) {
                actions.push(Action::Redo);
            }
            if input.consume_shortcut(&menus::OPEN) {
                actions.push(Action::OpenFile);
            }
            if input.consume_shortcut(&menus::GOTO) {
                actions.push(Action::OpenGoto);
            }
            if input.consume_shortcut(&menus::FIND_REGISTER) {
                actions.push(Action::OpenFindRegister);
            }
            if input.consume_shortcut(&menus::DRO_INFO) {
                actions.push(Action::OpenDroInfo);
            }
            if input.consume_shortcut(&menus::HELP) {
                actions.push(Action::Help);
            }
        });

        ctx.input(|input| {
            if input.key_pressed(Key::Delete) || input.key_pressed(Key::Backspace) {
                actions.push(Action::DeleteSelection);
            }
            if input.key_pressed(Key::Space) {
                actions.push(Action::TogglePlayback);
            }
            if input.key_pressed(Key::ArrowLeft) {
                actions.push(Action::PreviousDelay);
            }
            if input.key_pressed(Key::ArrowRight) {
                actions.push(Action::NextDelay);
            }
            if input.key_pressed(Key::ArrowUp) {
                actions.push(Action::SelectionMove {
                    delta: -1,
                    extend: input.modifiers.shift,
                });
            }
            if input.key_pressed(Key::ArrowDown) {
                actions.push(Action::SelectionMove {
                    delta: 1,
                    extend: input.modifiers.shift,
                });
            }
            const NUMBER_KEYS: [Key; 9] = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            for (channel, key) in NUMBER_KEYS.into_iter().enumerate() {
                if input.key_pressed(key) {
                    actions.push(Action::ToggleChannel(channel));
                }
            }
        });
    }

    fn sync_selection_indicator(&mut self) {
        let first = self.editor.selection.first();
        if first == self.last_first_selected {
            return;
        }
        self.last_first_selected = first;
        // An emptied selection leaves the indicator where it was, as in Python.
        let Some(index) = first else {
            return;
        };
        let Some(song) = self.editor.song() else {
            return;
        };
        if let Some(ms) = song.ms_offset_at(index) {
            self.waveform.start_ms = ms;
            self.position.set_position_ms(ms);
        }
    }

    fn playback_tick(&mut self, ctx: &egui::Context) {
        let playing = self.audio.is_playing();
        // One more update after playback ends, so the readout and cursor land
        // on the exact final position instead of freezing a buffer short of
        // it. (The Python's timer kept firing after the song finished.)
        if playing || self.was_playing {
            // A song that reached its end lands ~1 ms short of its length,
            // because the frame counter and the ms readout each floor at a rate
            // that need not divide evenly. Snap to the exact end so the ms and
            // sample counters agree. A manual Stop is not `is_finished`, so its
            // position is left exactly where playback paused.
            let ended = !playing && self.was_playing && self.audio.is_finished();
            if let Some(end) = ended
                .then(|| self.editor.song().map(|song| song.total_delay_ms()))
                .flatten()
            {
                self.waveform.cursor_ms = end;
                self.position.set_position_ms(end);
            } else if let Some(position) = self.audio.position() {
                self.waveform.cursor_ms = position.elapsed_ms;
                self.position.set_position(position);
            }
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        self.was_playing = playing;
        if self.tasks.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // -- actions ---------------------------------------------------------

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenFile => self.files.pick_open(),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::OpenSettings => {
                self.dialogs.settings = Some(SettingsDialog::new(&self.config));
            }
            Action::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),

            Action::Undo => {
                if !self.require_song() {
                    return;
                }
                match self.editor.undo() {
                    Some(description) => {
                        self.status = format!("Undone: {description}");
                        self.after_edit();
                    }
                    None => self.status = "Nothing to undo.".to_owned(),
                }
            }
            Action::Redo => {
                if !self.require_song() {
                    return;
                }
                match self.editor.redo() {
                    Some(description) => {
                        self.status = format!("Redone: {description}");
                        self.after_edit();
                    }
                    None => self.status = "Nothing to redo.".to_owned(),
                }
            }
            Action::OpenGoto => {
                if self.require_song() {
                    self.dialogs.goto = Some(GotoDialog::new());
                }
            }
            Action::OpenFindRegister => {
                if self.require_song() {
                    let song = self.editor.song().expect("gated");
                    self.dialogs.find_reg = Some(FindRegDialog::new(song));
                }
            }
            Action::OpenDroInfo => {
                if self.require_song() {
                    let song = self.editor.song().expect("gated");
                    // Never editable for a VGM: its length is derived from
                    // the sample delays (an edit would silently evaporate),
                    // and re-typing its chip corrupts the header's clocks.
                    let edit_allowed = self.config.ui.dro_info_edit_enabled && !song.is_vgm();
                    self.dialogs.dro_info = Some(DroInfoDialog::new(song, edit_allowed));
                }
            }
            Action::OpenEditTag => {
                if !self.require_song() {
                    return;
                }
                let song = self.editor.song().expect("gated");
                if song.is_vgm() {
                    let tag = song.vgm_meta().and_then(|meta| meta.tag.as_ref());
                    self.dialogs.gd3_tag = Some(Gd3TagDialog::new(tag));
                } else {
                    self.status = "Only VGMs support tag editing".to_owned();
                }
            }
            Action::OpenVgmMetadata => {
                if !self.require_song() {
                    return;
                }
                let song = self.editor.song().expect("gated");
                match VgmMetadataDialog::new(song) {
                    Some(dialog) => self.dialogs.vgm_metadata = Some(dialog),
                    // "Songs is not a VGM" in the Python -- typo fixed.
                    None => self.status = "Song is not a VGM".to_owned(),
                }
            }
            Action::ConvertToVgm => {
                if !self.require_song() {
                    return;
                }
                if self.editor.song().expect("gated").is_vgm() {
                    self.status = "File is already in VGM format".to_owned();
                    return;
                }
                match self.editor.convert_to_vgm() {
                    Ok(()) => {
                        self.status = "Successfully converted to VGM".to_owned();
                        self.close_song_dialogs();
                        self.scroll_to = Some(0);
                        self.after_edit();
                    }
                    Err(message) => self.alerts.push_back(Alert::error(message)),
                }
            }
            Action::DeleteSelection => {
                if !self.require_song() {
                    return;
                }
                if self.editor.delete_selection() {
                    self.scroll_to = self.editor.selection.first();
                    self.after_edit();
                }
            }

            Action::Help => self.alerts.push_back(Alert::new(HELP_TITLE, HELP_TEXT)),
            Action::About => self.alerts.push_back(Alert::new("About", about_text())),

            Action::Play => self.do_play(),
            Action::Stop => self.do_stop(),
            Action::PlayTail => self.do_play_tail(),
            Action::TogglePlayback => {
                if !self.require_song() {
                    return;
                }
                if self.audio.is_playing() {
                    self.do_stop();
                } else {
                    self.do_play();
                }
            }

            Action::NextDelay => self.delay_navigate(false),
            Action::PreviousDelay => self.delay_navigate(true),
            Action::SelectionMove { delta, extend } => {
                if let Some(row) = self
                    .editor
                    .selection
                    .key_move(delta, extend, self.editor.len())
                {
                    self.scroll_to = Some(row);
                }
            }
            Action::WaveformClicked { index, ms } => {
                self.editor.selection.select_only(index);
                // No scroll-into-view here, matching the Python's click path.
                if self.audio.is_playing() {
                    self.audio.seek_pos(index);
                }
                self.position.set_position_ms(ms);
            }
            Action::RewindToStart => {
                // Restart live playback from the top; snap the cursor and the
                // readout to zero whether or not anything is playing.
                self.audio.rewind();
                self.waveform.cursor_ms = 0;
                self.editor.selection.select_only(0);
                if self.audio.is_playing() {
                    self.audio.seek_pos(0);
                }
                self.position.set_position_ms(0);
            }

            Action::ToggleChannel(channel) => {
                self.channels.toggle_channel(channel);
                self.audio.set_muting(self.channels.muting());
            }
            Action::MutingChanged => self.audio.set_muting(self.channels.muting()),

            Action::Alert { title, message } => self.alerts.push_back(Alert::new(title, message)),
            Action::Status(message) => self.status = message,
            Action::GotoSubmitted(text) => self.goto_submitted(&text),
            Action::FindRegister { target, backwards } => self.find_register(&target, backwards),
            Action::UpdateHeader {
                opl_type,
                ms_length,
            } => {
                self.editor.update_header(opl_type, ms_length);
                self.after_edit();
            }
            Action::SaveGd3(tag) => self.editor.set_gd3_tag(*tag),
            Action::SaveVgmMetadata {
                loop_point,
                loop_base,
                loop_modifier,
                volume_modifier,
            } => {
                self.editor
                    .set_vgm_metadata(loop_point, loop_base, loop_modifier, volume_modifier)
            }
            Action::ApplySettings(config) => self.apply_settings(ctx, *config),
        }
    }

    // -- the workflows -----------------------------------------------------

    fn load_file(&mut self, file: PickedFile) {
        let name = file.name.clone();
        match self.editor.load(file) {
            Ok(report) => {
                self.status = format!("Successfully opened {name}.");
                // The Python left these dialogs open, still bound to the old
                // song object -- a stale Save then silently edited the wrong
                // song. Here they would edit the *new* one, which is worse,
                // so anything song-bound closes with the song.
                self.close_song_dialogs();
                self.waveform = WaveformState::default();
                self.submit_waveform(None);
                // Unload, not pause: the old stream's position must not leak
                // into the fresh cursor/readout via the end-of-playback
                // update below.
                self.audio.unload();
                self.audio_revision = None;
                self.was_playing = false;
                let song = self.editor.song().expect("just loaded");
                let file_version = song.file_version;
                self.position.set_length_ms(song.total_delay_ms());
                self.position.set_position_ms(0);
                self.last_first_selected = None;
                self.scroll_to = Some(0);
                self.push_load_warnings(report, file_version);
            }
            Err(message) => self
                .alerts
                .push_back(Alert::new("Failed to load file", message)),
        }
    }

    fn push_load_warnings(&mut self, report: LoadReport, file_version: u32) {
        if report.auto_trimmed {
            self.alerts
                .push_back(Alert::new(AUTO_TRIM_TITLE, AUTO_TRIM_TEXT));
        }
        if report.delay_mismatch {
            self.alerts
                .push_back(mismatch_alert(report.auto_trimmed, file_version));
        }
    }

    fn save(&mut self, force_dialog: bool) {
        if !self.require_song() {
            return;
        }
        let bytes = match self.editor.save_bytes() {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        let request = match (&self.editor.path, force_dialog) {
            (Some(path), false) => SaveRequest::InPlace {
                path: path.clone(),
                bytes,
            },
            _ => SaveRequest::Dialog {
                suggested_name: self.editor.song().expect("gated").name.clone(),
                bytes,
            },
        };
        self.files.save(request);
    }

    fn do_play(&mut self) {
        if !self.require_song() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.rewind();
        if let Some(first) = self.editor.selection.first() {
            self.audio.seek_pos(first);
        }
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    fn do_stop(&mut self) {
        if !self.require_song() {
            return;
        }
        self.audio.pause();
        self.audio.rewind();
    }

    fn do_play_tail(&mut self) {
        if !self.require_song() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Measured length, not the header's: on a DRO whose header overstates
        // the length, the Python seeked past the end and played nothing.
        let total = self.editor.song().expect("gated").total_delay_ms();
        self.audio.rewind();
        self.audio
            .seek_ms(total.saturating_sub(self.config.ui.tail_length));
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    fn delay_navigate(&mut self, backwards: bool) {
        if !self.require_song() {
            return;
        }
        match self.editor.find_next(FindTarget::AnyDelay, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(index);
            }
            None => self.status = "No more delays found.".to_owned(),
        }
    }

    fn goto_submitted(&mut self, text: &str) {
        if !self.require_song() {
            return;
        }
        let len = self.editor.len() as i64;
        match text.trim().parse::<i64>() {
            Err(_) => self.status = format!("Invalid position for goto: {text}"),
            Ok(position) if position < 0 || position >= len => {
                self.status = format!("Position for goto is out of range: {position}");
            }
            Ok(position) => {
                let position = usize::try_from(position).expect("bounds checked");
                self.editor.selection.select_only(position);
                self.scroll_to = Some(position);
                self.status = format!("Gone to position: {position}");
            }
        }
    }

    fn find_register(&mut self, target: &str, backwards: bool) {
        // An empty choice is a silent no-op, as in Python.
        if target.is_empty() || !self.require_song() {
            return;
        }
        let Ok(parsed) = target.parse::<FindTarget>() else {
            return;
        };
        match self.editor.find_next(parsed, backwards) {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(index);
                self.status = format!("Occurrence of {target} found at position {index}.");
            }
            None => self.status = format!("Could not find another occurrence of {target}."),
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context, config: AppConfig) {
        if let Err(error) = self.config_store.save(&config) {
            self.alerts
                .push_back(Alert::error(format!("Could not save settings: {error}")));
        }
        // Repaint the whole UI in the new scheme before anything else reads it.
        if config.ui.theme != self.config.ui.theme {
            theme::apply_palette(ctx, config.ui.theme);
        }
        // Only an audio change needs an output reload or a fresh waveform; a
        // theme-only change keeps the existing buckets and just recolours them.
        let audio_changed = config.audio != self.config.audio;
        let waveform_changed = config.audio.frequency != self.config.audio.frequency
            || config.audio.chip_write_delay != self.config.audio.chip_write_delay;
        self.config = config;
        self.position.set_frequency(config.audio.frequency);
        if let Some(song) = self.editor.song() {
            self.position.set_length_ms(song.total_delay_ms());
        }
        if audio_changed {
            // Reload the audio output lazily on the next play.
            self.audio_revision = None;
        }
        if waveform_changed {
            self.submit_waveform(None);
        }
        self.status = "Settings saved.".to_owned();
    }

    // -- helpers -------------------------------------------------------------

    /// Closes every dialog bound to the current song. Goto (validated live)
    /// and Settings (song-independent) survive.
    fn close_song_dialogs(&mut self) {
        self.dialogs.find_reg = None;
        self.dialogs.dro_info = None;
        self.dialogs.gd3_tag = None;
        self.dialogs.vgm_metadata = None;
    }

    /// The `@requires_dro_loaded` decorator: gates an action on a loaded song,
    /// with the Python's exact status message.
    fn require_song(&mut self) -> bool {
        if self.editor.has_song() {
            true
        } else {
            self.status = "Please open a DRO file first.".to_owned();
            false
        }
    }

    /// Everything every edit needs: stale audio paused, the length readout
    /// refreshed, and the waveform re-rendered (debounced, so holding Delete
    /// does not thrash the renderer -- the Python used the same 1 s debounce).
    fn after_edit(&mut self) {
        self.audio.pause();
        self.audio_revision = None;
        if let Some(song) = self.editor.song() {
            self.position.set_length_ms(song.total_delay_ms());
        }
        self.waveform.buckets.clear();
        self.submit_waveform(Some(Duration::from_secs(1)));
        // The selected row's time may have changed; force the indicator sync.
        self.last_first_selected = None;
    }

    fn submit_waveform(&mut self, debounce: Option<Duration>) {
        let Some(snapshot) = self.editor.snapshot() else {
            return;
        };
        self.tasks.submit(
            TaskRequest::RenderWaveform {
                song: snapshot,
                num_buckets: waveform::NUM_BUCKETS,
                sample_rate: self.config.audio.frequency,
                chip_write_delay: self.config.audio.chip_write_delay,
            },
            debounce,
        );
    }

    /// Loads the current song into the audio output if it is not already
    /// there. Cheap when nothing changed.
    fn ensure_audio(&mut self) -> Result<(), String> {
        if self.audio_revision == Some(self.editor.revision()) {
            return Ok(());
        }
        let snapshot = self
            .editor
            .snapshot()
            .ok_or_else(|| "No song is loaded.".to_owned())?;
        self.audio.load(snapshot, &self.config.audio)?;
        self.audio.set_muting(self.channels.muting());
        self.audio_revision = Some(self.editor.revision());
        // The device may have rejected the configured frequency; positions
        // report frames at the stream's real rate, so the panel must too.
        if let Some(rate) = self.audio.output_rate() {
            self.position.set_frequency(rate);
            if let Some(song) = self.editor.song() {
                self.position.set_length_ms(song.total_delay_ms());
            }
        }
        Ok(())
    }

    fn menu_state(&self) -> MenuState {
        MenuState {
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            undo_description: self.editor.undo_description().map(str::to_owned),
            redo_description: self.editor.redo_description().map(str::to_owned),
        }
    }

    /// `"Play last 3 seconds"`, with the Python's exact formatting: two
    /// decimals only for fractional lengths, singular for exactly one second.
    fn play_tail_label(&self) -> String {
        let ms = self.config.ui.tail_length;
        let value = if ms % 1000 == 0 {
            (ms / 1000).to_string()
        } else {
            format!("{:.2}", f64::from(ms) / 1000.0)
        };
        let plural = if ms == 1000 { "" } else { "s" };
        format!("Play last {value} second{plural}")
    }
}

impl eframe::App for DroApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_impl(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.audio.unload();
        self.tasks.shutdown();
    }
}

impl fmt::Debug for DroApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DroApp")
            .field("editor", &self.editor)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
