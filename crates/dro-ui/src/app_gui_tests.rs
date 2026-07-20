//! Headless GUI tests: drive the fully rendered `DroApp` through egui_kittest
//! and assert on editor state and what the fake platform services were asked to
//! do. Mounted as a child module of `app`, so it can read `DroApp`'s private
//! fields (`editor`, `dialogs`, `alerts`, `status`) directly.
//!
//! Interaction tests use the default (LazyRenderer) harness -- no GPU. The
//! snapshot tests at the bottom need the wgpu renderer and compare against PNG
//! baselines under `tests/snapshots/`; generate/refresh them with
//! `UPDATE_SNAPSHOTS=1 cargo test -p dro-ui`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use dro_core::Song;
use dro_core::config::{AppConfig, ThemeChoice};
use dro_synth::LoopCount;

use super::DroApp;
use crate::action::{Action, AppTab};
use crate::platform::{
    OptimizedImage, PickedFile, PickedFolder, RipJobOutcome, SaveOutcome, SaveRequest,
};
use crate::tasks::TaskKind;
use crate::test_song::{bogus_leading_delay_song, dual_tone_song, paced_song, tone_song};
use crate::test_support::{
    AudioLog, FakeAudioService, FakeFileService, FakeRipService, FileLog, InlineTaskService,
    MemoryConfigStore, NoopTaskService, RipLog, TaskLog,
};

/// Shared handles onto the fake services, for scripting and inspection.
struct Handles {
    files: Rc<RefCell<FileLog>>,
    audio: Rc<RefCell<AudioLog>>,
    tasks: Rc<RefCell<TaskLog>>,
    rip: Rc<RefCell<RipLog>>,
    saved_configs: Rc<RefCell<Vec<AppConfig>>>,
}

/// Serialise a fixture song back to bytes and wrap it as a picked file, exactly
/// as the editor's own unit tests do. The path gives Save somewhere to land.
fn picked(song: &Song) -> PickedFile {
    PickedFile {
        name: song.name.clone(),
        path: Some(PathBuf::from(format!("C:/songs/{}", song.name))),
        bytes: dro_core::io::write_song(song).unwrap(),
    }
}

/// Build a harness around a `DroApp` wired to fresh fakes.
///
/// - `inline_tasks`: run the waveform render synchronously (so it has pixels)
///   instead of dropping it on the floor.
/// - `wgpu`: use the wgpu renderer, required for snapshots.
fn build(
    initial: Option<PickedFile>,
    inline_tasks: bool,
    wgpu: bool,
) -> (Harness<'static, DroApp>, Handles) {
    // Tall enough for the five stacked panels plus table rows.
    build_sized(initial, inline_tasks, wgpu, egui::vec2(1000.0, 720.0))
}

fn build_sized(
    initial: Option<PickedFile>,
    inline_tasks: bool,
    wgpu: bool,
    size: egui::Vec2,
) -> (Harness<'static, DroApp>, Handles) {
    let files = Rc::new(RefCell::new(FileLog::default()));
    let audio = Rc::new(RefCell::new(AudioLog::default()));
    let tasks = Rc::new(RefCell::new(TaskLog::default()));
    let rip = Rc::new(RefCell::new(RipLog::default()));
    let saved_configs = Rc::new(RefCell::new(Vec::new()));

    let handles = Handles {
        files: files.clone(),
        audio: audio.clone(),
        tasks: tasks.clone(),
        rip: rip.clone(),
        saved_configs: saved_configs.clone(),
    };

    let app_builder = move |cc: &mut eframe::CreationContext<'_>| {
        // Match drotrim.rs startup: the embedded DOS font and feathering-off are
        // what make layout and snapshots deterministic.
        crate::theme::install(&cc.egui_ctx, ThemeChoice::default());
        DroApp::new(
            Box::new(FakeFileService(files)),
            Box::new(FakeAudioService(audio)),
            if inline_tasks {
                Box::new(InlineTaskService::new(tasks))
            } else {
                Box::new(NoopTaskService(tasks))
            },
            Box::new(FakeRipService(rip)),
            Box::new(MemoryConfigStore {
                initial: AppConfig::default(),
                saved: saved_configs,
            }),
            initial,
        )
    };

    // `max_steps` well above the default 4 gives settling room; playback tests
    // still avoid `run`.
    let builder = Harness::builder().with_size(size).with_max_steps(64);
    let mut harness = if wgpu {
        builder.wgpu().build_eframe(app_builder)
    } else {
        builder.build_eframe(app_builder)
    };
    harness.run();
    (harness, handles)
}

/// The common case: interaction harness, no song loaded.
fn empty_harness() -> (Harness<'static, DroApp>, Handles) {
    build(None, false, false)
}

/// Interaction harness with `song` already loaded (via the first-frame open).
fn harness_with_song(song: &Song) -> (Harness<'static, DroApp>, Handles) {
    build(Some(picked(song)), false, false)
}

// -- interaction tests -------------------------------------------------------

#[test]
fn starts_with_placeholder() {
    let (harness, handles) = empty_harness();

    assert!(
        harness.query_by_label_contains("Open a DRO").is_some(),
        "the empty-state placeholder should be shown"
    );
    assert!(!harness.state().editor.has_song());
    assert_eq!(handles.audio.borrow().load_count, 0);
    assert!(handles.tasks.borrow().submitted.is_empty());
}

#[test]
fn loading_a_dro_shows_table_and_status() {
    let song = tone_song();
    let (harness, handles) = harness_with_song(&song);

    assert_eq!(harness.state().status, "Successfully opened tone.dro.");
    assert_eq!(harness.state().editor.len(), song.len());
    for header in ["Pos (hex)", "Bank", "Reg.", "Value", "Description"] {
        assert!(
            harness.query_by_label(header).is_some(),
            "missing table header {header:?}"
        );
    }
    // Loading unloads any prior stream and kicks off a non-debounced render.
    assert_eq!(handles.audio.borrow().unload_calls, 1);
    assert_eq!(
        handles.tasks.borrow().submitted,
        vec![(TaskKind::RenderWaveform, None)]
    );
}

#[test]
fn play_button_loads_once_and_starts_playback() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.get_by_label("Play").click();
    harness.run_steps(3); // `run` would spin forever: playback requests repaints.

    {
        let audio = handles.audio.borrow();
        assert_eq!(
            audio.load_count, 1,
            "the song is loaded into the output once"
        );
        assert!(audio.rewind_calls >= 1);
        assert_eq!(audio.play_calls, 1);
        assert!(audio.playing);
    }

    // Playing again reuses the loaded stream (revision unchanged).
    harness.get_by_label("Play").click();
    harness.run_steps(3);
    let audio = handles.audio.borrow();
    assert_eq!(audio.load_count, 1, "no reload when nothing changed");
    assert_eq!(audio.play_calls, 2);
}

#[test]
fn stop_button_pauses_and_rewinds() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.get_by_label("Play").click();
    harness.run_steps(3);
    harness.get_by_label("Stop").click();
    harness.run_steps(3);

    let audio = handles.audio.borrow();
    assert!(audio.pause_calls >= 1, "Stop pauses");
    assert!(audio.rewind_calls >= 2, "one rewind on play, one on stop");
    assert!(!audio.playing);
}

#[test]
fn delete_key_removes_the_selected_row_and_ctrl_z_restores_it() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let full_len = song.len();

    harness.state_mut().editor.selection.select_only(0);
    harness.key_press(Key::Delete);
    harness.run();

    assert_eq!(harness.state().editor.len(), full_len - 1);
    // The edit pauses stale audio and schedules a debounced re-render.
    assert!(handles.audio.borrow().pause_calls >= 1);
    assert_eq!(
        handles.tasks.borrow().submitted.last().copied(),
        Some((
            TaskKind::RenderWaveform,
            Some(std::time::Duration::from_secs(1))
        ))
    );

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run();

    assert_eq!(
        harness.state().editor.len(),
        full_len,
        "undo restores the row"
    );
    assert!(harness.state().status.starts_with("Undone:"));
}

#[test]
fn goto_reads_hex_positions() {
    // parity-2: the Pos. column is hex, so Goto parses hex (and an optional 0x).
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert!(
        harness.state().editor.len() > 10,
        "fixture long enough for position 0xA"
    );
    harness.state_mut().goto_submitted("A"); // hex A = 10
    assert_eq!(harness.state().editor.selection.first(), Some(10));
    harness.state_mut().goto_submitted("0x3"); // 0x prefix
    assert_eq!(harness.state().editor.selection.first(), Some(3));
}

#[test]
fn edit_menu_opens_goto_dialog_and_it_jumps_the_selection() {
    let (mut harness, _handles) = harness_with_song(&tone_song());

    harness.get_by_label("Edit").click();
    harness.run();
    harness.get_by_label_contains("Goto").click();
    harness.run();

    assert!(
        harness.state().dialogs.goto.is_some(),
        "Goto dialog should open"
    );
    assert!(harness.query_by_label("Go to instruction (hex):").is_some());

    // Type a position into the dialog's field and submit it.
    let field = harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    harness.run();
    harness
        .get_by_role(egui::accesskit::Role::TextInput)
        .type_text("5");
    harness.run();
    harness.get_by_label("Go").click();
    harness.run();

    assert_eq!(harness.state().editor.selection.first(), Some(5));
    assert_eq!(harness.state().status, "Gone to position: 0005");
}

#[test]
fn dro_info_shortcut_opens_modal_and_blocks_playback_keys() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();

    assert!(harness.state().dialogs.dro_info.is_some());
    assert!(
        harness.query_by_label("DRO Info").is_some(),
        "heading should render"
    );

    // Space would normally start playback; the modal must swallow it.
    harness.key_press(Key::Space);
    harness.run();
    assert_eq!(handles.audio.borrow().play_calls, 0, "modal blocks Space");
}

#[test]
fn load_warnings_queue_and_dismiss_in_order() {
    let (mut harness, _handles) = harness_with_song(&bogus_leading_delay_song());

    // The bogus leading delay is trimmed (2 instructions remain) and both
    // warnings queue, auto-trim in front.
    assert_eq!(harness.state().editor.len(), 2);
    assert!(harness.query_by_label("DRO auto-trimmed").is_some());

    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        harness.query_by_label("DRO timing mismatch").is_some(),
        "dismissing the first alert reveals the second"
    );

    harness.get_by_label("OK").click();
    harness.run();
    assert!(harness.state().alerts.is_empty());
}

#[test]
fn number_key_toggles_channel_muting() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(handles.audio.borrow().mutings.is_empty());

    harness.key_press(Key::Num3);
    harness.run();
    {
        let audio = handles.audio.borrow();
        assert_eq!(audio.mutings.len(), 1);
        assert_ne!(
            *audio.mutings.last().unwrap(),
            dro_synth::Muting::all(),
            "muting a channel is not the all-audible state"
        );
    }

    harness.key_press(Key::Num3);
    harness.run();
    let audio = handles.audio.borrow();
    assert_eq!(audio.mutings.len(), 2);
    assert_eq!(
        *audio.mutings.last().unwrap(),
        dro_synth::Muting::all(),
        "toggling the same channel back restores everything"
    );
}

#[test]
fn a_held_modifier_suppresses_plain_editor_keys() {
    // ux-14: a plain editor key must not fire with Command/Alt held.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Space);
    harness.run_steps(3);
    assert_eq!(
        handles.audio.borrow().play_calls,
        0,
        "Cmd+Space does not toggle playback"
    );
    harness.key_press(Key::Space);
    harness.run_steps(3);
    assert!(
        handles.audio.borrow().play_calls >= 1,
        "a plain Space still plays"
    );
}

#[test]
fn shift_number_keys_toggle_the_high_channel_bank() {
    // ux-14: Shift+1..9 reach channels 9..17. Muting channel 0 (plain 1) then
    // Shift+1 must NOT restore all-audible -- if Shift+1 hit channel 0 again it
    // would. So two distinct channels end up muted.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.key_press(Key::Num1);
    harness.run();
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Num1);
    harness.run();
    assert_ne!(
        *handles.audio.borrow().mutings.last().unwrap(),
        dro_synth::Muting::all(),
        "Shift+1 targets channel 9, so channels 0 and 9 are both muted"
    );
}

#[test]
fn enter_dismisses_an_info_alert() {
    // ux-12: Enter is OK.
    let (mut harness, _handles) = empty_harness();
    harness
        .state_mut()
        .alerts
        .push_back(crate::alert::Alert::new("Heads up", "Something happened."));
    harness.run();
    assert!(!harness.state().alerts.is_empty());

    harness.key_press(Key::Enter);
    harness.run();
    assert!(
        harness.state().alerts.is_empty(),
        "Enter dismissed the info alert"
    );
}

#[test]
fn enter_confirms_a_confirm_alert_and_runs_its_action() {
    // ux-12: Enter accepts a confirm box and runs its carried action.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness
        .state_mut()
        .alerts
        .push_back(crate::alert::Alert::confirm(
            "Discard unsaved changes?",
            "Quit anyway?",
            Action::ConfirmExit,
        ));
    harness.run();

    harness.key_press(Key::Enter);
    harness.run();
    assert!(
        harness.state().alerts.is_empty(),
        "the confirm was accepted"
    );
    assert!(
        harness.state().quitting,
        "Enter ran the carried ConfirmExit action"
    );
}

// -- channel panning ---------------------------------------------------------

#[test]
fn custom_toggle_engages_and_disengages_panning() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(
        handles.audio.borrow().pannings.is_empty(),
        "loading a song pushes no panning"
    );

    harness.get_by_label("Custom").click();
    harness.run();
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&dro_synth::Panning::Custom([0x80; 18])),
        "engaging Custom pushes the centred pans"
    );

    harness.get_by_label("Custom").click();
    harness.run();
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&dro_synth::Panning::Original),
        "disengaging returns to Original"
    );
}

#[test]
fn pan_knob_drag_sends_custom_panning_without_resending_muting() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.get_by_label("Custom").click();
    harness.run();
    let mutings_before = handles.audio.borrow().mutings.len();

    // Drag channel 1's knob far to the left: the relative mapping clamps it hard
    // left regardless of the exact per-frame split.
    let center = harness.get_by_label("Pan 1 (low bank)").rect().center();
    harness.drag_at(center);
    harness.run();
    harness.hover_at(center - egui::vec2(200.0, 0.0));
    harness.run();
    harness.drop_at(center - egui::vec2(200.0, 0.0));
    harness.run();

    let audio = handles.audio.borrow();
    match audio.pannings.last().expect("a panning was pushed") {
        dro_synth::Panning::Custom(pans) => {
            assert_eq!(pans[0], 0x00, "channel 1 dragged hard left");
            assert_eq!(pans[1], 0x80, "channel 2 stays centred");
        }
        other => panic!("expected Custom panning, got {other:?}"),
    }
    assert_eq!(
        audio.mutings.len(),
        mutings_before,
        "a pan drag must not resend muting"
    );
}

#[test]
fn pan_knob_drag_up_pans_left_like_dragging_left() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.get_by_label("Custom").click();
    harness.run();

    // Dragging a knob straight up pans it hard left, exactly as dragging left
    // does: the vertical axis feeds the same relative mapping (wd-2).
    let center = harness.get_by_label("Pan 1 (low bank)").rect().center();
    harness.drag_at(center);
    harness.run();
    harness.hover_at(center - egui::vec2(0.0, 200.0));
    harness.run();
    harness.drop_at(center - egui::vec2(0.0, 200.0));
    harness.run();

    match handles
        .audio
        .borrow()
        .pannings
        .last()
        .expect("a panning was pushed")
    {
        dro_synth::Panning::Custom(pans) => {
            assert_eq!(pans[0], 0x00, "channel 1 dragged straight up = hard left");
            assert_eq!(pans[1], 0x80, "channel 2 stays centred");
        }
        other => panic!("expected Custom panning, got {other:?}"),
    }
}

#[test]
fn right_clicking_a_pan_knob_recenters_it() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    // Custom mode with channel 1 hard left, the rest centred.
    let mut pans = [0x80u8; 18];
    pans[0] = 0x00;
    harness.state_mut().channels.set_showcase_pans(pans);
    harness.run();

    harness.get_by_label("Pan 1 (low bank)").click_secondary();
    harness.run();

    assert_eq!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom([0x80; 18]),
        "right-click recentres the knob"
    );
}

#[test]
fn dual_opl2_original_pans_hard_left_and_right() {
    let (mut harness, handles) = harness_with_song(&dual_tone_song());
    harness.get_by_label("Play").click();
    harness.run_steps(3); // playback requests repaints; `run` would spin.

    let mut image = [0x00u8; 18];
    image[9..].fill(0xFF);
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&dro_synth::Panning::Custom(image)),
        "dual-OPL2 Original plays chip 1 left, chip 2 right"
    );
    assert_eq!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom(image),
        "the panel reports the fixed image while still in Original mode"
    );
}

#[test]
fn spread_knob_spreads_the_pans_and_engages_custom() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    // Drag the Spread knob to the right: a positive spread leans even channels
    // left, odd channels right, and engages Custom so the knobs go live (wd-4).
    let center = harness.get_by_label("Spread").rect().center();
    harness.drag_at(center);
    harness.run();
    harness.hover_at(center + egui::vec2(200.0, 0.0));
    harness.run();
    harness.drop_at(center + egui::vec2(200.0, 0.0));
    harness.run();

    match handles
        .audio
        .borrow()
        .pannings
        .last()
        .expect("a panning was pushed")
    {
        dro_synth::Panning::Custom(pans) => {
            assert!(pans[0] < 0x80, "channel 1 leans left");
            assert!(pans[1] > 0x80, "channel 2 leans right");
            assert_ne!(pans[0], pans[2], "channels get slightly different values");
        }
        other => panic!("expected Custom panning, got {other:?}"),
    }
    // The spread engaged Custom, so the knobs are now live.
    assert!(matches!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom(_)
    ));
}

#[test]
fn all_button_unmutes_but_leaves_panning() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Custom mode with off-centre pans, plus a muted channel.
    harness.state_mut().channels.set_showcase_pans([0x10; 18]);
    harness.key_press(Key::Num3);
    harness.run();

    harness.get_by_label("All").click();
    harness.run();

    assert_eq!(
        handles.audio.borrow().mutings.last(),
        Some(&dro_synth::Muting::all()),
        "All unmutes everything"
    );
    // The custom pan image is left untouched -- All is a muting control now (wd-5).
    assert_eq!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom([0x10; 18]),
        "All leaves the pans alone"
    );
}

#[test]
fn reset_button_restores_original_panning() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().channels.set_showcase_pans([0x10; 18]);
    assert!(matches!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom(_)
    ));

    harness.get_by_label("Reset").click();
    harness.run();

    // A plain OPL2 song's default is Original (mono).
    assert_eq!(
        harness.state().channels.panning(),
        dro_synth::Panning::Original,
        "Reset returns panning to the song type's default"
    );
}

#[test]
fn loading_a_song_resets_pan_mode_to_original() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().channels.set_showcase_pans([0x00; 18]);
    assert!(matches!(
        harness.state().channels.panning(),
        dro_synth::Panning::Custom(_)
    ));

    // Loading another song rebuilds the panel: Original mode, fresh defaults.
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&tone_song())));
    harness.run();

    assert_eq!(
        harness.state().channels.panning(),
        dro_synth::Panning::Original,
        "a fresh load returns to Original mode"
    );
}

#[test]
fn boost_up_arrow_sets_boost_and_persists_it() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.get_by_label("\u{25B2}").click(); // ▲ louder
    harness.run();

    assert_eq!(
        handles.audio.borrow().boosts.last().copied(),
        Some(2.0),
        "default boost 1 steps up to 2"
    );
    let saved = handles.saved_configs.borrow();
    assert_eq!(saved.len(), 1, "the change is persisted once");
    assert_eq!(saved[0].audio.boost, 2.0);
}

#[test]
fn settings_save_preserves_a_live_changed_boost() {
    // M4/ux-15: the Settings dialog snapshots the config at open and doesn't
    // expose the boost, so a boost changed via the transport meanwhile must not
    // be reverted on Save.
    let (mut harness, handles) = harness_with_song(&tone_song());
    let config = harness.state().config;
    harness.state_mut().dialogs.settings = Some(crate::dialogs::SettingsDialog::new(&config));
    harness.run();

    // The transport changes the boost while Settings is open.
    harness.state_mut().config.audio.boost = 2.0;

    harness.get_by_label("Save").click();
    harness.run();

    let saved = handles.saved_configs.borrow();
    assert_eq!(
        saved.last().unwrap().audio.boost,
        2.0,
        "Settings Save kept the live boost, not its stale snapshot"
    );
}

#[test]
fn settings_do_not_retune_the_position_panel_while_a_stream_is_live() {
    // ux-16: a frequency change must not retune the panel while a stream plays
    // at the old rate (the readout would mix a new-rate length with old-rate
    // frames). The panel keeps the live rate until the stream reloads.
    let (mut harness, handles) = harness_with_song(&tone_song());
    // A live stream reports 48 kHz; playing adopts it into the panel.
    handles.audio.borrow_mut().output_rate = Some(48_000);
    harness.state_mut().do_play();
    harness.run_steps(3);
    assert_eq!(
        harness.state().position.frequency(),
        48_000,
        "the panel adopts the live stream's rate"
    );

    // Apply settings with a different configured frequency.
    let mut config = harness.state().config;
    config.audio.frequency = 44_100;
    harness.state_mut().dialogs.settings = Some(crate::dialogs::SettingsDialog::new(&config));
    harness.run_steps(3); // the stream is playing, so `run` would spin
    harness.get_by_label("Save").click();
    harness.run_steps(3);

    assert_eq!(
        harness.state().position.frequency(),
        48_000,
        "the panel keeps the live rate, not the newly configured 44100"
    );
}

#[test]
fn ctrl_s_saves_in_place_and_reports_the_path() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let expected_path = PathBuf::from("C:/songs/tone.dro");

    harness.key_press_modifiers(Modifiers::COMMAND, Key::S);
    harness.run();

    let expected_bytes = harness.state().editor.save_bytes().unwrap();
    {
        let files = handles.files.borrow();
        assert_eq!(files.save_requests.len(), 1);
        match &files.save_requests[0] {
            SaveRequest::InPlace { path, bytes } => {
                assert_eq!(path, &expected_path);
                assert_eq!(bytes, &expected_bytes);
            }
            other => panic!("expected an in-place save, got {other:?}"),
        }
    }

    // The service reporting success updates the status bar.
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "tone.dro".to_owned(),
            path: Some(expected_path.clone()),
        });
    harness.run();
    assert!(
        harness.state().status.starts_with("File saved to"),
        "status was {:?}",
        harness.state().status
    );
}

// -- snapshot tests ----------------------------------------------------------
//
// Baselines live in tests/snapshots/. They render via wgpu (DX12 WARP on
// headless Windows) and are inherently machine/GPU-specific; regenerate with
// UPDATE_SNAPSHOTS=1 on the machine that runs them if they drift.

/// Settle the pointer out of the frame, then snapshot. Since 0.34, kittest
/// paints a synthetic mouse cursor whenever a pointer position is live (e.g.
/// after a click), which would bake a cursor triangle and hover state into
/// the baselines.
fn settled_snapshot(harness: &mut Harness<'static, DroApp>, name: &str) {
    harness.remove_cursor();
    harness.run();
    harness.snapshot(name);
}

#[test]
fn snapshot_empty_app() {
    let (mut harness, _handles) = build(None, false, true);
    settled_snapshot(&mut harness, "empty_app");
}

#[test]
fn snapshot_loaded_song() {
    // Inline tasks so the waveform is actually rendered for the snapshot.
    let (mut harness, _handles) = build(Some(picked(&tone_song())), true, true);
    harness.state_mut().editor.selection.select_only(0);
    harness.run();
    settled_snapshot(&mut harness, "loaded_tone_song");
}

#[test]
fn snapshot_dro_info_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();
    settled_snapshot(&mut harness, "dro_info_dialog");
}

#[test]
fn snapshot_auto_trim_alert() {
    let (mut harness, _handles) = build(Some(picked(&bogus_leading_delay_song())), false, true);
    settled_snapshot(&mut harness, "auto_trim_alert");
}

#[test]
fn snapshot_pan_strip_custom() {
    // A dual-OPL2 song shows both bank pan rows; engage Custom with a spread of
    // pans so the knobs render at distinct angles in the app's controls panel.
    let (mut harness, _handles) = build(Some(picked(&dual_tone_song())), false, true);
    let mut pans = [0x80u8; 18];
    for (slot, pan) in pans.iter_mut().enumerate() {
        *pan = [0x00, 0x40, 0x80, 0xC0, 0xFF][slot % 5];
    }
    harness.state_mut().channels.set_showcase_pans(pans);
    settled_snapshot(&mut harness, "pan_strip_custom");
}

#[test]
fn opening_a_file_over_unsaved_changes_prompts_first() {
    // H2: opening a file while the editor has unsaved edits holds it behind a
    // discard-changes confirm instead of clobbering the song.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.selection.select_only(0);
    harness.state_mut().editor.delete_selection();
    assert!(harness.state().editor.is_dirty());

    let other_name = dual_tone_song().name.clone();
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.run();
    assert!(
        harness.state().editor.is_dirty(),
        "the dirty song is untouched while the prompt is up"
    );
    assert!(harness.state().pending_load.is_some());
    assert!(harness.query_by_label("OK").is_some(), "a confirm is shown");

    // Confirming loads the pending file, replacing the editor song.
    harness.get_by_label("OK").click();
    harness.run();
    assert!(harness.state().pending_load.is_none());
    assert_eq!(harness.state().editor.song().unwrap().name, other_name);
    assert!(!harness.state().editor.is_dirty(), "freshly loaded = clean");
}

#[test]
fn opening_a_file_with_no_unsaved_changes_loads_immediately() {
    // H2: the guard must not prompt when there is nothing to lose.
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(!harness.state().editor.is_dirty());
    let other_name = dual_tone_song().name.clone();
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.run();
    assert!(
        harness.state().pending_load.is_none(),
        "loaded directly, nothing stashed"
    );
    assert_eq!(harness.state().editor.song().unwrap().name, other_name);
}

#[test]
fn exiting_with_unsaved_changes_prompts_then_sets_quitting_on_confirm() {
    // H2: File > Exit raises the discard-changes confirm rather than quitting;
    // confirming sets the quitting flag (and sends a Close to the viewport).
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.selection.select_only(0);
    harness.state_mut().editor.delete_selection();
    assert!(harness.state().editor.is_dirty());

    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Exit").click();
    harness.run();
    assert!(
        harness
            .query_by_label_contains("Discard unsaved changes")
            .is_some(),
        "the discard-changes confirm is shown"
    );
    assert!(!harness.state().quitting, "the app has not quit yet");

    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        harness.state().quitting,
        "confirming sets the quitting flag"
    );
}

#[test]
fn a_stray_tab_does_not_disable_the_keyboard() {
    // Regression (M3): Tab must not move focus onto a chrome button (where the
    // old egui_wants_keyboard_input gate would swallow every shortcut and Space
    // would "click" the focused button). Tab is consumed; Space still plays.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let len = harness.state().editor.len();
    harness.state_mut().editor.selection.select_only(0);

    harness.key_press(Key::Tab);
    harness.run();
    harness.key_press(Key::Space);
    harness.run_steps(3);

    assert!(
        handles.audio.borrow().play_calls >= 1,
        "Space reached the editor and toggled playback"
    );
    assert_eq!(
        harness.state().editor.len(),
        len,
        "Tab+Space did not delete a row"
    );
}

// -- rip mode ----------------------------------------------------------------

const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

/// A VGM fixture re-serialised with a file name and GD3 tag, wrapped as a picked
/// file for a rip folder.
fn tagged_vgm(name: &str, game: &str, author: &str, creator: &str) -> PickedFile {
    let mut song = dro_core::io::read_song(name, VGM_FIXTURE).unwrap();
    if let Some(meta) = song.vgm_meta_mut() {
        meta.tag = Some(dro_core::Gd3Tag {
            game_name_en: game.to_owned(),
            track_author_en: author.to_owned(),
            creator: creator.to_owned(),
            ..dro_core::Gd3Tag::default()
        });
    }
    PickedFile {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/pack/{name}"))),
        bytes: dro_core::io::write_song(&song).unwrap(),
    }
}

fn rip_folder(name: &str, files: Vec<PickedFile>) -> PickedFolder {
    PickedFolder {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/{name}"))),
        files,
    }
}

/// A two-track "Cool Game" folder.
fn cool_game_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
        ],
    )
}

/// Queues a folder and runs a frame so `poll_folder` installs it.
fn open_folder(harness: &mut Harness<'static, DroApp>, handles: &Handles, folder: PickedFolder) {
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(folder));
    harness.run();
}

/// The rip view scrolls as one page, so the track-row and screenshot buttons sit
/// well below a 720px viewport; a tall harness keeps them clickable.
fn tall_rip_harness() -> (Harness<'static, DroApp>, Handles) {
    build_sized(None, false, false, egui::vec2(1000.0, 1700.0))
}

#[test]
fn opening_a_folder_switches_to_the_rip_tab_and_prefills() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    assert_eq!(harness.state().active_tab, AppTab::Rip);
    {
        let state = harness.state();
        let meta = &state.rip.as_ref().expect("a rip is open").meta;
        assert_eq!(meta.game_name, "Cool Game");
        assert_eq!(meta.creator, "Ripper");
        assert_eq!(meta.music_authors, "Ada, Bob");
        // The fake reports a fixed "today", so the history line is deterministic.
        assert_eq!(meta.history, "1.00 2026-07-16 Ripper: Initial release.");
    }
    // The tab strip is now shown ("Editor" is unique to it).
    assert!(
        harness.query_by_label("Editor").is_some(),
        "tab strip appears"
    );
    assert!(harness.state().status.contains("Cool Game"));
}

#[test]
fn clicking_the_editor_tab_returns_to_the_editor() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    assert_eq!(harness.state().active_tab, AppTab::Rip);

    harness.get_by_label("Editor").click();
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(harness.state().rip.is_some(), "the rip project is retained");
    // The editor's empty-state placeholder is back.
    assert!(harness.query_by_label_contains("Open a DRO").is_some());
}

#[test]
fn editing_a_field_marks_the_rip_dirty() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    assert!(!harness.state().rip.as_ref().unwrap().dirty);

    // Type into the first form field (Game name); any edit sets the dirty flag.
    let field = harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("a metadata field");
    field.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap()
        .type_text("!");
    harness.run();

    assert!(harness.state().rip.as_ref().unwrap().dirty);
}

#[test]
fn a_scanned_track_caches_its_table_entry() {
    // uiwidget-3: the table entry is computed once at scan, not per row per
    // frame, and matches a fresh computation.
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    let state = harness.state();
    let track = &state.rip.as_ref().unwrap().tracks[0];
    let cached = track
        .entry
        .as_ref()
        .expect("a parsed track caches its entry");
    let fresh = dro_core::rip::TrackEntry::from_song(track.song().unwrap(), &track.file_name);
    assert_eq!(*cached, fresh);
}

#[test]
fn save_package_files_writes_the_txt_and_m3u() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.get_by_label("Save Package Files").click();
    harness.run();

    let files = handles.files.borrow();
    assert_eq!(
        files.save_requests.len(),
        2,
        "the description and the playlist"
    );
    let mut names = Vec::new();
    for request in &files.save_requests {
        match request {
            SaveRequest::InPlace { path, bytes } => {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.ends_with(".txt") {
                    let text = String::from_utf8(bytes.clone()).unwrap();
                    assert!(text.contains("Game name:           Cool Game"));
                    assert!(text.contains("\r\n"), "CRLF line endings");
                }
                names.push(name);
            }
            other => panic!("expected an in-place save, got {other:?}"),
        }
    }
    assert_eq!(names, ["Cool Game.txt", "Cool Game.m3u"]);
}

#[test]
fn a_failed_package_doc_save_keeps_the_rip_dirty() {
    // uishell-7: if a package-doc save fails, the dirty flag must be kept, not
    // cleared when the batch's last doc lands, so the edits aren't lost.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().rip.as_mut().unwrap().dirty = true;

    harness.state_mut().save_rip_docs();
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Failed("disk full".to_owned()));
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "Cool Game.m3u".to_owned(),
            path: None,
        });
    harness.run();

    assert!(
        harness.state().rip.as_ref().unwrap().dirty,
        "a failed package-doc save keeps the rip dirty so edits aren't lost"
    );
}

#[test]
fn saving_without_a_game_name_shows_an_alert() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness
        .state_mut()
        .rip
        .as_mut()
        .unwrap()
        .meta
        .game_name
        .clear();

    harness.get_by_label("Save Package Files").click();
    harness.run();

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing was saved"
    );
    assert!(!harness.state().alerts.is_empty(), "an alert explains why");
}

#[test]
fn editor_keys_are_ignored_on_the_rip_tab() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let full_len = song.len();
    harness.state_mut().editor.selection.select_only(0);
    open_folder(&mut harness, &handles, cool_game_folder());
    assert_eq!(harness.state().active_tab, AppTab::Rip);

    // Delete would remove the selected editor row on the editor tab; here it
    // must do nothing, since the editor is hidden.
    harness.key_press(Key::Delete);
    harness.run();
    assert_eq!(
        harness.state().editor.len(),
        full_len,
        "the hidden song is untouched"
    );
}

/// A one-track folder, so the per-row ▶/Edit buttons are unambiguous.
fn single_track_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper")],
    )
}

#[test]
fn switching_to_the_rip_tab_stops_editor_playback() {
    // Regression (M2): the editor's audio must not keep playing under the rip
    // view. Leaving the editor tab unloads it.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().select_tab(AppTab::Editor);
    harness.state_mut().do_play();
    assert!(handles.audio.borrow().playing);

    harness.state_mut().select_tab(AppTab::Rip);
    assert!(
        !handles.audio.borrow().playing,
        "editor audio stops when the rip tab takes over"
    );
    assert!(harness.state().audio_revision.is_none());
}

#[test]
fn entering_the_rip_tab_closes_song_bound_dialogs() {
    // Regression (ux-13): Goto and the song-bound modeless dialogs are
    // editor-only (the menu disables them on the rip tab), so entering the rip
    // tab must close any that are open.
    use crate::dialogs::{FindRegDialog, GotoDialog};
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().select_tab(AppTab::Editor);

    harness.state_mut().dialogs.goto = Some(GotoDialog::new());
    harness.state_mut().dialogs.find_reg = Some(FindRegDialog::new(&song));

    harness.state_mut().select_tab(AppTab::Rip);
    assert!(
        harness.state().dialogs.goto.is_none(),
        "Goto closes on the rip tab"
    );
    assert!(
        harness.state().dialogs.find_reg.is_none(),
        "song-bound dialogs close on the rip tab"
    );
}

#[test]
fn previewing_a_track_plays_it_and_stop_halts_it() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    // U+25B6 play.
    harness.get_by_label("\u{25B6}").click();
    harness.run_steps(3); // playback requests repaints; `run` would spin.
    {
        let audio = handles.audio.borrow();
        assert!(audio.load_count >= 1, "the track is loaded into the output");
        assert_eq!(audio.play_calls, 1);
        assert!(audio.playing);
    }
    assert_eq!(harness.state().rip.as_ref().unwrap().preview, Some(0));

    // The button now shows U+25A0 stop.
    harness.get_by_label("\u{25A0}").click();
    harness.run_steps(3);
    assert!(!handles.audio.borrow().playing);
    assert_eq!(harness.state().rip.as_ref().unwrap().preview, None);
}

#[test]
fn previewing_a_track_uses_its_own_panning_not_the_editor_songs() {
    // Editor song is dual-OPL2, so its panning is the fixed hard-L/R chip image;
    // the rip track is a mono OPL2 song, whose panning is Original. Previewing the
    // track must use the track's own panning -- leaking the editor's hard-L/R
    // image onto a mono track plays it hard left (the reported bug).
    let (mut harness, handles) = build_sized(
        Some(picked(&dual_tone_song())),
        false,
        false,
        egui::vec2(1000.0, 1700.0),
    );

    // Play the editor song so its hard-L/R panning is the last one sent.
    harness.get_by_label("Play").click();
    harness.run_steps(3);
    let mut dual_image = [0x00u8; 18];
    dual_image[9..].fill(0xFF);
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&dro_synth::Panning::Custom(dual_image)),
        "the dual-OPL2 editor song sent the hard-L/R image"
    );

    // Open a rip folder and preview its OPL2 track.
    open_folder(&mut harness, &handles, single_track_folder());
    harness.get_by_label("\u{25B6}").click();
    harness.run_steps(3);

    let audio = handles.audio.borrow();
    assert_eq!(
        audio.pannings.last(),
        Some(&dro_synth::Panning::Original),
        "preview uses the mono track's own Original panning, not the editor's hard-L/R"
    );
    assert_eq!(
        audio.mutings.last(),
        Some(&dro_synth::Muting::all()),
        "preview clears channel mutes"
    );
}

#[test]
fn a_failed_preview_load_does_not_wedge_the_editors_audio() {
    // Regression (H3): a failed preview `load` still tears down the editor's
    // stream, so the editor's audio revision must be invalidated up front --
    // otherwise `ensure_audio` short-circuits on the next editor Play and calls
    // `play()` on an empty output (the "No song is loaded" wedge).
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let editor_name = harness.state().editor.song().unwrap().name.clone();
    open_folder(&mut harness, &handles, single_track_folder());

    // Make the editor's audio current, as if it had just played.
    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    assert!(handles.audio.borrow().playing);

    // Preview a rip track, but force its load to fail.
    handles.audio.borrow_mut().fail_next_load = true;
    harness.state_mut().active_tab = AppTab::Rip;
    harness.state_mut().preview_track(0);
    assert!(
        harness.state().audio_revision.is_none(),
        "a failed preview load invalidates the editor's audio revision"
    );
    assert!(harness.state().rip.as_ref().unwrap().preview.is_none());

    // The editor reloads and plays its own song instead of wedging.
    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    let audio = handles.audio.borrow();
    assert!(audio.playing, "the editor reloaded and plays, not wedged");
    assert_eq!(
        audio.loaded.as_ref().unwrap().name,
        editor_name,
        "the editor's own song is what reloaded"
    );
}

#[test]
fn a_failed_preview_play_reloads_the_editor_song_not_the_rip_track() {
    // Regression (H3): when preview `load` succeeds but `play` fails, the
    // half-started preview must be unloaded and the revision reset, so the next
    // editor Play reloads the *editor's* song rather than resuming the rip track
    // the service still had loaded.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let editor_name = harness.state().editor.song().unwrap().name.clone();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();

    handles.audio.borrow_mut().fail_next_play = true;
    harness.state_mut().active_tab = AppTab::Rip;
    harness.state_mut().preview_track(0);
    assert_eq!(
        harness.state().rip.as_ref().unwrap().preview,
        None,
        "the half-started preview is dropped"
    );
    assert!(!handles.audio.borrow().playing);

    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    let audio = handles.audio.borrow();
    assert!(audio.playing);
    assert_eq!(
        audio.loaded.as_ref().unwrap().name,
        editor_name,
        "the editor's own song reloaded, not the rip track"
    );
}

#[test]
fn loading_a_song_switches_to_the_editor_tab_and_stops_preview() {
    // Regression (M7): File>Open (or a drop) while the rip tab is active must
    // surface the editor tab and stop any preview, not load invisibly behind
    // the rip view with a stranded play button.
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Rip);

    harness.state_mut().preview_track(0);
    assert_eq!(harness.state().rip.as_ref().unwrap().preview, Some(0));

    // Deliver a song the way menu Open / drag-and-drop would.
    harness.state_mut().load_file(picked(&tone_song()));

    assert_eq!(
        harness.state().active_tab,
        AppTab::Editor,
        "the tab flips to the editor"
    );
    assert_eq!(
        harness.state().rip.as_ref().unwrap().preview,
        None,
        "the preview is stopped"
    );
    assert!(harness.state().editor.has_song());
}

#[test]
fn an_in_place_refresh_keeps_a_playing_preview_by_name() {
    // Regression (ux-18): a same-folder rescan (e.g. after a screenshot optimise
    // redelivers the folder) must not cut a running preview -- it re-matches the
    // preview by file name, even when the rescan reorders the track list.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Preview the second track (02 Boss.vgm).
    harness.state_mut().preview_track(1);
    assert_eq!(harness.state().rip.as_ref().unwrap().preview, Some(1));
    assert!(handles.audio.borrow().playing);

    // Redeliver the same folder with the files reversed, as a real rescan can.
    let reversed = rip_folder(
        "Cool Game",
        vec![
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
        ],
    );
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(reversed));
    harness.run_steps(3);

    let state = harness.state();
    let rip = state.rip.as_ref().unwrap();
    assert_eq!(rip.tracks[0].file_name, "02 Boss.vgm");
    assert_eq!(
        rip.preview,
        Some(0),
        "the preview follows 02 Boss.vgm to its new index"
    );
    assert!(
        handles.audio.borrow().playing,
        "the preview keeps playing across the in-place refresh"
    );
}

#[test]
fn opening_a_track_loads_it_into_the_editor() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Rip);

    // A row double-click emits RipTrackOpen; kittest cannot double-click, so
    // drive the handler directly (the row-sense wiring is trivial UI code).
    harness.state_mut().open_track_in_editor(0);
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(
        harness.state().editor.has_song(),
        "the track loaded into the editor"
    );
    assert!(harness.state().rip.is_some(), "the rip project is retained");
}

#[test]
fn open_button_loads_the_track_into_the_editor() {
    // A tall harness so the track row (and its Open button) is on-screen and
    // hit-testable, as the quick-edit test needs too.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Rip);

    // The per-row Open button is the discoverable path to the same handler the
    // double-click drives (wd-9).
    harness.get_by_label("Open").click();
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(
        harness.state().editor.has_song(),
        "the Open button loaded the track"
    );
    assert!(harness.state().rip.is_some(), "the rip project is retained");
}

#[test]
fn reordering_renumbers_files_and_is_undoable_and_redoable() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Feed Ok outcomes for every rename the batch issues, and the reordered
    // folder the follow-up rescan installs.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(rip_folder(
            "Cool Game",
            vec![
                tagged_vgm("01 Boss.vgm", "Cool Game", "Bob", "Ripper"),
                tagged_vgm("02 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            ],
        )));
    }

    // Move 01 Intro down a slot; both tracks renumber.
    harness.state_mut().move_rip_track(0, 1);
    harness.run_steps(16);

    {
        let files = handles.files.borrow();
        assert_eq!(files.rename_requests.len(), 4, "a temp-then-final batch");
        let finals: Vec<&String> = files
            .rename_requests
            .iter()
            .map(|(_, to)| to)
            .filter(|to| !to.starts_with(".drotrim"))
            .collect();
        assert!(finals.iter().any(|to| *to == "01 Boss.vgm"));
        assert!(finals.iter().any(|to| *to == "02 Intro.vgz"));
    }
    assert_eq!(harness.state().rip_undo.len(), 1, "the reorder is undoable");
    assert_eq!(
        harness.state().rip.as_ref().unwrap().tracks[0].file_name,
        "01 Boss.vgm",
        "the rescan installed the new order"
    );

    // Undo: the inverse batch restores the original order.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(cool_game_folder()));
    }
    harness.state_mut().undo_rip_edit();
    harness.run_steps(16);
    assert!(
        harness.state().rip_undo.is_empty(),
        "undo cleared the undo stack"
    );
    assert_eq!(harness.state().rip_redo.len(), 1, "and left a redo");
    assert_eq!(
        harness.state().rip.as_ref().unwrap().tracks[0].file_name,
        "01 Intro.vgz",
        "the original order is back"
    );
}

#[test]
fn quick_edit_opens_a_dialog_and_saves_a_rewrite() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.get_by_label("Tags").click();
    harness.run();
    assert!(
        harness.state().dialogs.track_edit.is_some(),
        "the quick-edit dialog opens"
    );

    // Save without changing the name: an in-place rewrite, no rename.
    harness.get_by_label("Save").click();
    harness.run();

    let files = handles.files.borrow();
    assert_eq!(
        files.save_requests.len(),
        1,
        "the track is rewritten in place"
    );
    match &files.save_requests[0] {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("01 Intro.vgz"));
            assert!(
                dro_core::io::read_song("01 Intro.vgz", bytes).is_ok(),
                "the rewritten bytes are a valid VGZ"
            );
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
    assert!(
        files.rename_requests.is_empty(),
        "an unchanged name is not renamed"
    );
}

#[test]
fn quick_edit_after_a_reorder_targets_the_track_by_name() {
    // Regression (H1): a rescan can reorder the name-sorted list while the
    // quick-edit dialog is open, so the submit re-resolves the track by its
    // original file name -- never a since-stale index.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Reorder: 02 Boss.vgm first, 01 Intro.vgz now at index 1.
    let reversed = rip_folder(
        "Cool Game",
        vec![
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
        ],
    );
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(reversed));
    harness.run_steps(3);
    assert_eq!(
        harness.state().rip.as_ref().unwrap().tracks[1].file_name,
        "01 Intro.vgz",
        "01 Intro is now at index 1"
    );

    // A quick edit that opened on 01 Intro.vgz renames it; it must touch 01
    // Intro's file, not whatever now sits at the old index 0 (02 Boss.vgm).
    harness.state_mut().quick_edit_submitted(
        "01 Intro.vgz".to_owned(),
        "01 Intro Redux.vgz".to_owned(),
        dro_core::Gd3Tag::default(),
    );

    let files = handles.files.borrow();
    let (from, to) = files
        .rename_requests
        .last()
        .expect("a rename was requested");
    assert!(
        from.to_string_lossy().ends_with("01 Intro.vgz"),
        "renamed 01 Intro's file, got {from:?}"
    );
    assert_eq!(to, "01 Intro Redux.vgz");
}

#[test]
fn a_rescan_closes_the_open_quick_edit_dialog() {
    // Regression (H1, defensive): the quick-edit dialog is bound to one track,
    // so a rescan that can reorder or drop tracks must close it.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().open_track_quick_edit(0);
    assert!(harness.state().dialogs.track_edit.is_some());

    // Redeliver the folder (a same-folder rescan).
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(cool_game_folder()));
    harness.run_steps(3);
    assert!(
        harness.state().dialogs.track_edit.is_none(),
        "the rescan closed the quick-edit dialog"
    );
}

#[test]
fn quick_edit_rename_rewrites_only_after_the_rename_lands() {
    // M1/ux-9: a name change must rename first, then rewrite the target-format
    // bytes to the NEW path -- so a failed rename can't leave the old file
    // holding bytes its extension no longer matches.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.state_mut().quick_edit_submitted(
        "01 Intro.vgz".to_owned(),
        "01 Intro.vgm".to_owned(),
        dro_core::Gd3Tag::default(),
    );
    {
        let files = handles.files.borrow();
        assert_eq!(files.rename_requests.len(), 1);
        assert_eq!(files.rename_requests[0].1, "01 Intro.vgm");
        assert!(
            files.save_requests.is_empty(),
            "no byte rewrite before the rename lands"
        );
    }

    // The rename succeeds -> now the bytes are written, to the new path.
    handles.files.borrow_mut().rename_outcomes.push_back(Ok(()));
    harness.run();
    let files = handles.files.borrow();
    match files
        .save_requests
        .last()
        .expect("a rewrite after the rename")
    {
        SaveRequest::InPlace { path, .. } => assert!(
            path.to_string_lossy().ends_with("01 Intro.vgm"),
            "rewrote the renamed file, got {path:?}"
        ),
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

const PNG_FIXTURE: &[u8] = include_bytes!("../../../tests/screenshot.png");

/// A folder that passes every export validation (named, numbered, with a png).
/// The png is a real (decodable) image so the inline preview renders.
fn complete_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            PickedFile {
                name: "Cool Game.png".to_owned(),
                path: Some(PathBuf::from("C:/Cool Game/Cool Game.png")),
                bytes: PNG_FIXTURE.to_vec(),
            },
        ],
    )
}

#[test]
fn a_chip_preset_fills_system_os_and_hardware() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    {
        // Blank the fields so the preset's effect is unambiguous.
        let rip = harness.state_mut().rip.as_mut().unwrap();
        rip.meta.system.clear();
        rip.meta.os.clear();
        rip.meta.music_hardware.clear();
        rip.dirty = false;
    }

    harness.get_by_label("OPL-3").click();
    harness.run();

    let state = harness.state();
    let rip = state.rip.as_ref().unwrap();
    assert_eq!(rip.meta.system, "IBM PC/AT");
    assert_eq!(rip.meta.os, "DOS");
    assert_eq!(rip.meta.music_hardware, "Sound Blaster Pro 2 (YMF262)");
    assert!(rip.dirty, "a preset counts as an edit");
}

#[test]
fn optimize_saves_a_smaller_screenshot_in_place() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Optimize").click();
    harness.run();
    {
        let rip = handles.rip.borrow();
        assert_eq!(rip.optimize_requests.len(), 1);
        assert_eq!(rip.optimize_requests[0].0, "Cool Game.png");
    }

    // The service returns smaller bytes: they are saved over the original.
    handles
        .rip
        .borrow_mut()
        .optimized_outcomes
        .push_back(Ok(OptimizedImage {
            name: "Cool Game.png".to_owned(),
            original_len: PNG_FIXTURE.len(),
            bytes: b"\x89PNG smaller".to_vec(),
        }));
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("Cool Game.png"));
            assert_eq!(bytes, b"\x89PNG smaller");
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

#[test]
fn an_already_optimal_screenshot_is_not_rewritten() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Optimize").click();
    harness.run();
    handles
        .rip
        .borrow_mut()
        .optimized_outcomes
        .push_back(Ok(OptimizedImage {
            name: "Cool Game.png".to_owned(),
            original_len: PNG_FIXTURE.len(),
            bytes: PNG_FIXTURE.to_vec(), // no smaller
        }));
    harness.run();

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing to save"
    );
    assert!(harness.state().status.contains("already optimal"));
}

#[test]
fn exporting_submits_a_job_and_saves_the_returned_zip() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();

    {
        let rip = handles.rip.borrow();
        assert_eq!(rip.submitted.len(), 1, "a build job was submitted");
        let job = &rip.submitted[0];
        assert_eq!(job.zip_name, "Cool Game.zip");
        assert!(job.gzip_vgms);
        let names: Vec<&str> = job
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        for expected in [
            "01 Intro.vgz",
            "Cool Game.png",
            "Cool Game.txt",
            "Cool Game.m3u",
        ] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }

    // The service returns the finished zip; the app saves it via a dialog.
    handles
        .rip
        .borrow_mut()
        .outcomes
        .push_back(RipJobOutcome::Done {
            zip_name: "Cool Game.zip".to_owned(),
            bytes: b"PK\x03\x04".to_vec(),
            log: vec!["Cool Game.png: 100 -> 80 bytes".to_owned()],
        });
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::Dialog { suggested_name, .. } => assert_eq!(suggested_name, "Cool Game.zip"),
        other => panic!("expected a save dialog, got {other:?}"),
    }
}

#[test]
fn exporting_without_a_screenshot_prompts_first() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder()); // no .png

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();
    assert!(
        handles.rip.borrow().submitted.is_empty(),
        "no job until confirmed"
    );
    assert!(
        !harness.state().alerts.is_empty(),
        "a warning prompt is shown"
    );

    harness.get_by_label("OK").click();
    harness.run();
    assert_eq!(
        handles.rip.borrow().submitted.len(),
        1,
        "confirming submits the job"
    );
}

#[test]
fn a_failed_export_shows_an_alert() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();
    handles
        .rip
        .borrow_mut()
        .outcomes
        .push_back(RipJobOutcome::Failed("disk full".to_owned()));
    harness.run();

    assert!(
        harness
            .state()
            .alerts
            .iter()
            .any(|alert| alert.title == "Rip export failed"),
        "the failure is surfaced as an alert"
    );
}

#[test]
fn snapshot_rip_view() {
    // Tall, so the form, track list and inline screenshot are all captured.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.run();
    settled_snapshot(&mut harness, "rip_view");
}

#[test]
fn snapshot_rip_view_scrolled() {
    // A short viewport, so the page overflows and the outer scrollbar appears --
    // framed with the sunken well bevel, flush to the panel edge (wd-13).
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 560.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.run();
    settled_snapshot(&mut harness, "rip_view_scrolled");
}

#[test]
fn snapshot_track_edit_dialog() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    open_folder(&mut harness, &handles, single_track_folder());
    harness.get_by_label("Tags").click();
    harness.run();
    settled_snapshot(&mut harness, "track_edit_dialog");
}

// -- loop points (lp-4) ------------------------------------------------------

/// Drives one action through the app the way the frame loop would.
fn act(harness: &mut Harness<'static, DroApp>, action: Action) {
    let ctx = harness.ctx.clone();
    harness.state_mut().handle_action(&ctx, action);
}

#[test]
fn marking_a_loop_pushes_the_region_only_while_looping_is_on() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();

    // Markers move, but with looping off nothing but `None` reaches the engine:
    // Play still means "play the song".
    act(&mut harness, Action::SetLoopStart(1));
    act(&mut harness, Action::SetLoopEnd(3));
    assert_eq!(
        (
            harness.state().editor.markers.start(),
            harness.state().editor.markers.end()
        ),
        (1, 3)
    );
    assert!(
        handles.audio.borrow().loops.iter().all(Option::is_none),
        "looping is off, so no region should be armed"
    );

    act(&mut harness, Action::ToggleLoopPlayback);
    let armed = handles.audio.borrow().loops.last().copied().flatten();
    let armed = armed.expect("toggling looping on arms the marked region");
    assert_eq!((armed.start, armed.end), (1, 3));

    // Turning it back off disarms rather than leaving a stale region behind.
    act(&mut harness, Action::ToggleLoopPlayback);
    assert!(handles.audio.borrow().loops.last().unwrap().is_none());

    // And a reset marks the whole song again.
    act(&mut harness, Action::ClearLoopMarkers);
    assert!(harness.state().editor.markers.is_full(len));
}

#[test]
fn changing_the_repeat_count_re_arms_the_region() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::ToggleLoopPlayback);
    act(&mut harness, Action::SetLoopCount(LoopCount::Times(3)));

    let armed = handles
        .audio
        .borrow()
        .loops
        .last()
        .copied()
        .flatten()
        .expect("a region is armed");
    assert_eq!(armed.count, LoopCount::Times(3));
}

#[test]
fn deleting_instructions_slides_the_loop_markers() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::SetLoopStart(2));
    act(&mut harness, Action::SetLoopEnd(4));

    harness.state_mut().editor.selection.select_only(0);
    act(&mut harness, Action::DeleteSelection);

    let markers = harness.state().editor.markers;
    assert_eq!(
        (markers.start(), markers.end()),
        (1, 3),
        "both markers slide past the deleted row"
    );
}

#[test]
fn applying_a_loop_to_a_dro_explains_itself_instead_of_failing_quietly() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::ApplyLoopToMetadata);
    harness.run_steps(2);
    assert!(
        harness
            .query_by_label_contains("Convert the song")
            .is_some(),
        "a DRO has nowhere to store a loop, and should say so"
    );
}

#[test]
fn applying_a_loop_writes_the_vgm_metadata() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    let len = harness.state().editor.len();

    act(&mut harness, Action::SetLoopStart(1));
    act(&mut harness, Action::SetLoopEnd(len - 1));
    assert!(
        harness.state().editor.loop_markers_are_unapplied(),
        "the markers differ from the stored loop until applied"
    );

    act(&mut harness, Action::ApplyLoopToMetadata);
    let meta = harness
        .state()
        .editor
        .song()
        .unwrap()
        .vgm_meta()
        .unwrap()
        .clone();
    assert_eq!(meta.loop_point, Some(1));
    assert_eq!(meta.loop_end, Some(len - 1));
    assert!(!harness.state().editor.loop_markers_are_unapplied());
    // The end stops short of the tail, so the status says what that means.
    assert!(
        harness.state().status.contains("trimmed"),
        "status was {:?}",
        harness.state().status
    );

    // An end at the song's end is stored as "to the end", not a fixed index --
    // so a later trim widens the loop with the song instead of stranding it.
    act(&mut harness, Action::SetLoopEnd(len));
    act(&mut harness, Action::ApplyLoopToMetadata);
    let meta = harness.state().editor.song().unwrap().vgm_meta().unwrap();
    assert_eq!(meta.loop_end, None);
}

#[test]
fn play_seam_forces_looping_on_and_seeks_before_the_loop_end() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    act(&mut harness, Action::SetLoopEnd(song.len()));
    act(&mut harness, Action::PlaySeam);

    assert!(
        harness.state().loop_enabled,
        "auditioning a seam with looping off would never reach it"
    );
    let log = handles.audio.borrow();
    let seek = log
        .seeks_ms
        .last()
        .copied()
        .expect("it seeks before playing");
    let end_ms = song.total_delay_ms();
    assert!(
        seek < end_ms,
        "seek {seek} should precede the loop end {end_ms}"
    );
    assert!(log.play_calls > 0);
    assert!(
        log.loops.last().unwrap().is_some(),
        "the region is armed before playback starts"
    );
}

#[test]
fn the_transport_row_drives_the_loop_controls() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    // The count starts "without end" and steps down into the finite range.
    assert!(harness.query_by_label("\u{221E}").is_some());
    harness.get_by_label("\u{2212}").click();
    harness.run();
    assert_eq!(harness.state().loop_count, LoopCount::Times(9));
    harness.get_by_label("+").click();
    harness.run();
    assert_eq!(
        harness.state().loop_count,
        LoopCount::Infinite,
        "stepping back up returns to 'without end'"
    );

    // The Loop toggle arms the region.
    harness.get_by_label("Loop").click();
    harness.run();
    assert!(harness.state().loop_enabled);
    assert!(handles.audio.borrow().loops.last().unwrap().is_some());
}

#[test]
fn the_loop_overlay_appears_only_once_there_is_something_to_show() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert!(
        harness.state().waveform.loop_overlay.is_none(),
        "an unmarked song with looping off shows no brackets"
    );

    // Switching looping on shows the region even though it is still the whole song.
    act(&mut harness, Action::ToggleLoopPlayback);
    harness.run_steps(2);
    let overlay = harness
        .state()
        .waveform
        .loop_overlay
        .expect("brackets show");
    assert!(overlay.active);

    // Marking a region shows them with looping off too.
    act(&mut harness, Action::ToggleLoopPlayback);
    act(&mut harness, Action::SetLoopStart(1));
    harness.run_steps(2);
    let overlay = harness
        .state()
        .waveform
        .loop_overlay
        .expect("brackets show");
    assert!(!overlay.active, "marked, but not repeating");
}

#[test]
fn the_overlay_flags_an_unapplied_region_until_it_is_written() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    act(&mut harness, Action::SetLoopStart(1));
    harness.run_steps(2);
    assert!(
        harness
            .state()
            .waveform
            .loop_overlay
            .expect("brackets show")
            .unapplied,
        "the region differs from the song's stored loop"
    );

    act(&mut harness, Action::ApplyLoopToMetadata);
    harness.run_steps(2);
    assert!(
        !harness
            .state()
            .waveform
            .loop_overlay
            .expect("brackets stay")
            .unapplied,
        "applying clears the cue"
    );
}

#[test]
fn snapshot_loop_overlay() {
    // The visual guard for the loop region: brackets with inward flags, the wash
    // over the region, the lit Loop toggle and the repeat count. The song is a
    // VGM with nothing stored, so the flags are hollow -- the unapplied cue.
    let (mut harness, _handles) = build(Some(picked(&paced_song())), true, true);
    harness.state_mut().editor.convert_to_vgm().unwrap();
    // Instruction 9 opens the first burst and every fourth one after it starts
    // the next 100 ms, so 13..25 is the region from 100 ms to 400 ms of 600.
    act(&mut harness, Action::SetLoopStart(13));
    act(&mut harness, Action::SetLoopEnd(25));
    act(&mut harness, Action::ToggleLoopPlayback);
    act(&mut harness, Action::SetLoopCount(LoopCount::Times(4)));
    harness.run();
    settled_snapshot(&mut harness, "loop_overlay");
}

#[test]
fn an_applied_loop_is_guarded_by_the_discard_prompt() {
    // The metadata half of H2: a loop region is deliberate work, and before this
    // an Open would have thrown it away without a word.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    act(&mut harness, Action::SetLoopStart(1));
    act(&mut harness, Action::ApplyLoopToMetadata);
    assert!(
        harness.state().editor.is_dirty(),
        "applying a loop leaves unsaved changes"
    );

    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.run();
    assert!(
        harness.state().pending_load.is_some(),
        "the open is held behind a confirm rather than clobbering the loop"
    );
    assert!(harness.query_by_label("OK").is_some());
}
