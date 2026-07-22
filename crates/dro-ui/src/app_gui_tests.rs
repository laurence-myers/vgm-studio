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
use crate::test_song::{
    bogus_leading_delay_song, dro_song_v2, dual_tone_song, looping_vgm, multi_song_capture,
    multi_song_capture_dro, paced_song, redundant_vgm_song, tone_song,
};
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
fn typing_in_the_volume_field_does_not_toggle_a_channel() {
    // Regression: the channel shortcuts (1-9) must not fire while the volume field
    // holds keyboard focus, or typing a number there would also mute a channel.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    // Stand in for the field holding focus, as it reports each frame it is edited.
    harness.state_mut().volume_field_editing = true;
    let before = harness.state().channels.muting();

    harness.key_press(Key::Num3);
    harness.run_steps(1);

    assert_eq!(
        harness.state().channels.muting(),
        before,
        "a number typed into the focused volume field must not toggle channel 3"
    );
}

#[test]
fn boost_up_arrow_steps_up_the_live_volume_without_persisting_when_unlocked() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.get_by_label("\u{25B2}").click(); // ▲ louder
    harness.run();

    // From unity the up arrow makes a coarse ~1.0 step to about 2x (snapped to the
    // ladder), not a fine nudge.
    let expected = dro_core::volume_step_up(1.0);
    assert!(
        (expected - 2.0).abs() < 0.06,
        "one click steps ~1.0 up from unity: {expected}"
    );
    assert_eq!(
        handles.audio.borrow().boosts.last().copied(),
        Some(expected),
        "the up arrow sets the live volume"
    );
    assert_eq!(
        harness.state().config.audio.boost,
        expected,
        "and the lever reflects it"
    );
    // Unlocked (the default), a volume change is per-song and is not written to
    // drotrim.ini -- so opening another song can start it from its own modifier.
    assert!(
        handles.saved_configs.borrow().is_empty(),
        "an unlocked volume change does not persist"
    );
}

#[test]
fn a_locked_volume_change_is_persisted() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Locking the volume makes changes persist and carry across songs.
    act(&mut harness, Action::SetLockBoost(true));
    let before = handles.saved_configs.borrow().len();

    harness.get_by_label("\u{25B2}").click(); // ▲ louder
    harness.run();

    let saved = handles.saved_configs.borrow();
    assert!(
        saved.len() > before,
        "a locked volume change is written to drotrim.ini"
    );
    let last = saved.last().expect("a save");
    assert_eq!(last.audio.boost, dro_core::volume_step_up(1.0));
    assert!(last.audio.lock_boost, "and the lock state is saved with it");
}

/// The VGM fixture with its header volume modifier set to `modifier`.
fn vgm_with_modifier(modifier: u8) -> Song {
    let mut song = dro_core::io::read_song("m.vgm", VGM_FIXTURE).unwrap();
    song.vgm_meta_mut().unwrap().volume_modifier = modifier;
    song
}

#[test]
fn opening_a_song_sets_the_volume_from_its_header_modifier_when_unlocked() {
    // The header asks for a 2x volume (modifier 0x20); unlocked, opening it sets
    // the playback volume to match, so the boost never carries over stale.
    let song = vgm_with_modifier(0x20);
    let (harness, _handles) = build(Some(picked(&song)), false, false);
    assert!(
        (harness.state().config.audio.boost - 2.0).abs() < 1e-4,
        "the volume follows the header modifier: {}",
        harness.state().config.audio.boost
    );
}

#[test]
fn a_locked_volume_ignores_the_songs_modifier_on_open() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    // Lock the volume at 4x.
    act(&mut harness, Action::SetLockBoost(true));
    act(
        &mut harness,
        Action::SetBoost {
            value: 4.0,
            persist: true,
        },
    );
    // Opening a song whose modifier asks for 2x must not disturb the locked 4x.
    harness
        .state_mut()
        .load_file(picked(&vgm_with_modifier(0x20)));
    harness.run();
    assert_eq!(
        harness.state().config.audio.boost,
        4.0,
        "locked: the volume is kept, not reset to the song's 2x modifier"
    );
}

#[test]
fn unlocking_snaps_the_volume_to_the_current_songs_modifier() {
    // Locked at 4x over a song whose modifier asks for 2x.
    let (mut harness, _handles) = build(Some(picked(&vgm_with_modifier(0x20))), false, false);
    act(&mut harness, Action::SetLockBoost(true));
    act(
        &mut harness,
        Action::SetBoost {
            value: 4.0,
            persist: true,
        },
    );
    assert_eq!(harness.state().config.audio.boost, 4.0);
    // Unlocking hands control back to the song: the volume snaps to its 2x now.
    act(&mut harness, Action::SetLockBoost(false));
    assert!(
        (harness.state().config.audio.boost - 2.0).abs() < 1e-4,
        "unlocking snaps to the modifier: {}",
        harness.state().config.audio.boost
    );
}

#[test]
fn the_volume_lever_cannot_rise_past_the_clipping_ceiling() {
    // Once the limiter has engaged, the app pins a ceiling at the current volume
    // and the up arrow stops raising it -- the clipping guard.
    let (mut harness, handles) = harness_with_song(&tone_song());
    handles.audio.borrow_mut().min_engaged_boost = Some(1.0);
    harness.run(); // the backend reports 1.0x as the lowest clipping level

    let before = harness.state().config.audio.boost;
    harness.get_by_label("\u{25B2}").click(); // ▲ louder -- but capped
    harness.run();

    assert_eq!(
        harness.state().config.audio.boost,
        before,
        "the up arrow is blocked once the limiter has engaged"
    );

    // Lowering is still allowed, and drops off the ceiling.
    harness.get_by_label("\u{25BC}").click(); // ▼ quieter
    harness.run();
    assert!(
        harness.state().config.audio.boost < before,
        "the down arrow still works at the ceiling"
    );
}

#[test]
fn the_volume_ceiling_allows_returning_to_the_trigger_level_but_not_beyond() {
    // The limiter fires at 2.00x, pinning the ceiling there. The lever must then
    // move freely below 2.00x and back up to it, but never past it.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().config.audio.boost = 2.0;
    handles.audio.borrow_mut().min_engaged_boost = Some(2.0);
    harness.run(); // the backend reports 2.00x as the lowest clipping level

    // At the ceiling, the up arrow is blocked.
    harness.get_by_label("\u{25B2}").click();
    harness.run();
    assert_eq!(
        harness.state().config.audio.boost,
        2.0,
        "cannot rise past the 2.00x trigger level"
    );

    // Step down one position -- now below the ceiling.
    harness.get_by_label("\u{25BC}").click();
    harness.run();
    let lowered = harness.state().config.audio.boost;
    assert!(
        lowered < 2.0,
        "the down arrow drops below the ceiling: {lowered}"
    );

    // The up arrow works again from there, climbing back to exactly 2.00x...
    harness.get_by_label("\u{25B2}").click();
    harness.run();
    assert_eq!(
        harness.state().config.audio.boost,
        2.0,
        "can climb back up to the trigger level"
    );

    // ...but no further: still capped at 2.00x.
    harness.get_by_label("\u{25B2}").click();
    harness.run();
    assert_eq!(
        harness.state().config.audio.boost,
        2.0,
        "still capped at the trigger level"
    );
}

#[test]
fn the_volume_ceiling_ratchets_down_to_the_lowest_clipping_level() {
    // The limiter first bites at 10x, so the cap starts there.
    let (mut harness, handles) = harness_with_song(&tone_song());
    handles.audio.borrow_mut().min_engaged_boost = Some(10.0);
    harness.run();
    assert_eq!(
        harness.state().boost_ceiling,
        Some(10.0),
        "the cap starts at the first level that clipped"
    );

    // Dropping to 9x still clips, so the backend reports the lower minimum and the
    // cap follows it down -- unlike the old sticky boolean, which kept 10x and let
    // the user climb back to it.
    handles.audio.borrow_mut().min_engaged_boost = Some(9.0);
    harness.run();
    assert_eq!(
        harness.state().boost_ceiling,
        Some(9.0),
        "the cap ratchets down to the lowest level that clips"
    );
}

#[test]
fn match_volume_measures_the_peak_and_sets_the_volume() {
    // An inline task service runs the scan synchronously, so the whole chain --
    // button -> VolumeScan task -> measure_peak -> set volume -> persist -- runs
    // for real on the song.
    let song = tone_song();
    // build(initial, inline_tasks, wgpu): inline runs the scan synchronously.
    let (mut harness, handles) = build(Some(picked(&song)), true, false);

    harness.get_by_label("Match").click();
    // The inline scan finishes on submit, but its Peak lands in `pending` and is
    // delivered by a later frame's poll. `run` would go idle before then (the
    // synchronous fake never looks busy), so force frames: one processes the
    // click and submits, the next polls the Peak and applies it.
    for _ in 0..4 {
        harness.step();
    }

    assert!(
        handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::VolumeScan),
        "clicking Match submits a volume scan"
    );

    // The status line proves the scan landed and was applied.
    assert!(
        harness.state().status.contains("dBFS"),
        "the status reports the peak: {:?} boost={}",
        harness.state().status,
        harness.state().config.audio.boost
    );

    // Recompute the scan's peak here to pin the exact ladder volume the app must
    // have chosen and persisted.
    let rate = harness.state().config.audio.frequency;
    let peak = dro_synth::measure_peak(&song, rate);
    let expected = dro_core::volume_modifier_factor(dro_core::nearest_volume_modifier(
        dro_core::boost_for_peak(peak.max_level),
    ));
    assert_eq!(
        harness.state().config.audio.boost,
        expected,
        "the volume is matched to the measured peak, on the ladder; status={:?}",
        harness.state().status
    );
    // Match Volume is a per-song action: unlocked, it sets the live volume but
    // does not write to drotrim.ini.
    assert!(
        handles.saved_configs.borrow().is_empty(),
        "an unlocked Match does not persist"
    );
}

#[test]
fn opening_a_song_cancels_a_running_volume_scan() {
    // A scan started for song A must not land on song B: its stale peak would
    // overwrite B's modifier-derived volume. Loading cancels the scan.
    let (mut harness, handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::MatchVolume);
    harness.state_mut().load_file(picked(&tone_song()));
    harness.run();
    assert!(
        handles
            .tasks
            .borrow()
            .cancelled
            .contains(&TaskKind::VolumeScan),
        "loading a song cancels the in-flight volume scan"
    );
}

#[test]
fn match_volume_without_a_song_submits_no_scan() {
    let (mut harness, handles) = empty_harness();
    // The lever (and its Match button) render even with no song loaded; clicking
    // Match then asks for a song rather than scanning nothing.
    harness.get_by_label("Match").click();
    harness.run();
    assert!(
        !handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::VolumeScan),
        "no scan is submitted without a song"
    );
}

#[test]
fn measuring_the_modifier_routes_the_peak_to_the_open_dialog() {
    let song = tone_song();
    // Inline tasks run the scan; convert to VGM so there is a modifier to fill.
    let (mut harness, _handles) = build(Some(picked(&song)), true, false);
    harness.state_mut().editor.convert_to_vgm().unwrap();
    let vgm = harness.state().editor.song().unwrap().clone();
    harness.state_mut().dialogs.vgm_metadata =
        Some(crate::dialogs::VgmMetadataDialog::new(&vgm).unwrap());

    // Trigger the Measure scan; the inline scan stores its Peak, then a poll frame
    // routes it to the open dialog (the same delivery shape as Match Volume).
    act(&mut harness, Action::MeasureVolumeModifier);
    for _ in 0..4 {
        harness.step();
    }

    // Only the FillModifier branch sets this status, and only with an open dialog,
    // so it proves the scan measured and reached the dialog.
    assert!(
        harness.state().status.contains("volume modifier"),
        "status was {:?}",
        harness.state().status
    );
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

// -- render to WAV -----------------------------------------------------------

/// Opens File > Render to WAV..., which needs the menu to be walked.
fn open_render_wav_dialog(harness: &mut Harness<'static, DroApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Render to WAV").click();
    harness.run();
}

#[test]
fn the_file_menu_opens_the_render_dialog() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    open_render_wav_dialog(&mut harness);
    assert!(harness.state().dialogs.render_wav.is_some());
}

#[test]
fn rendering_with_no_options_offers_the_wav_to_save() {
    // Inline tasks so the render completes within the same run.
    let (mut harness, handles) = build(Some(picked(&tone_song())), true, false);
    open_render_wav_dialog(&mut harness);
    harness.get_by_label("Render").click();
    harness.run();

    // The dialog closed, the task ran, and its bytes went to a save dialog under
    // the CLI's own name: song.dro -> song.dro.wav.
    assert!(harness.state().dialogs.render_wav.is_none());
    let files = handles.files.borrow();
    let Some(SaveRequest::Dialog {
        suggested_name,
        bytes,
    }) = files.save_requests.last()
    else {
        panic!("expected a save dialog, got {:?}", files.save_requests)
    };
    assert_eq!(suggested_name, "tone.dro.wav");
    assert!(bytes.starts_with(b"RIFF"), "not a WAV");
}

#[test]
fn a_saved_render_is_reported_in_the_status_bar() {
    let (mut harness, handles) = build(Some(picked(&tone_song())), true, false);
    let expected_path = PathBuf::from("C:/songs/tone.dro.wav");

    open_render_wav_dialog(&mut harness);
    harness.get_by_label("Render").click();
    harness.run();
    assert_eq!(
        handles.files.borrow().save_requests.len(),
        1,
        "the render should have reached the save dialog"
    );

    // Queue the outcome only now: the fake hands back whatever is queued on the
    // next poll, which would otherwise be consumed before the save was made.
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "tone.dro.wav".to_owned(),
            path: Some(expected_path.clone()),
        });
    harness.run();

    assert_eq!(
        harness.state().status,
        format!("Rendered {}.", expected_path.display())
    );
}

#[test]
fn the_render_options_reach_the_task() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Mute a channel and pan another, so "apply" has something to apply.
    harness.state_mut().channels.toggle_channel(1);
    open_render_wav_dialog(&mut harness);
    harness.get_by_label("All of the above").click();
    harness.run();
    harness.get_by_label("Render").click();
    harness.run();

    // Noop tasks record the submission without running it.
    let tasks = handles.tasks.borrow();
    assert_eq!(
        tasks.submitted.last().map(|(kind, _)| *kind),
        Some(TaskKind::RenderWav)
    );
}

#[test]
fn a_second_render_is_refused_while_one_is_running() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Open the dialog before the service claims to be busy: a busy service keeps
    // requesting repaints, so `run` would never settle (as for playback).
    open_render_wav_dialog(&mut harness);
    handles.tasks.borrow_mut().busy = vec![TaskKind::RenderWav];

    harness.get_by_label("Render").click();
    harness.run_steps(3);

    assert_eq!(harness.state().status, "Already rendering a WAV.");
    assert!(
        !handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::RenderWav),
        "no second render should be queued"
    );
}

#[test]
fn loading_a_song_cancels_a_render_of_the_previous_one() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Open").click();
    harness.run();

    assert!(
        handles
            .tasks
            .borrow()
            .cancelled
            .contains(&TaskKind::RenderWav),
        "the previous song's render should be cancelled"
    );
}

#[test]
fn a_failed_render_alerts_instead_of_saving() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Deliver a failure as though the task had produced one.
    harness
        .state_mut()
        .handle_wav_result(Err("no disk".to_owned()));
    harness.run();

    assert_eq!(harness.state().status, "The WAV render failed.");
    assert!(!harness.state().alerts.is_empty(), "an alert is shown");
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing should be saved"
    );
}

// -- split channels ----------------------------------------------------------

/// Opens File > Split Channels...
fn open_split_dialog(harness: &mut Harness<'static, DroApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Split Channels").click();
    harness.run();
}

/// The names a WAV split of `dual_tone_song` writes, in order.
fn split_names(song: &Song) -> Vec<String> {
    dro_synth::split(
        song,
        &dro_synth::SplitOptions {
            format: dro_synth::SplitFormat::Wav,
            isolate_percussion: false,
            audio: dro_core::config::AudioConfig::default(),
        },
        &mut |_| {},
        &mut |_, _| {},
    )
    .unwrap()
    .into_iter()
    .map(|output| output.name)
    .collect()
}

#[test]
fn splitting_writes_one_file_per_channel_into_the_chosen_folder() {
    let song = dual_tone_song();
    let (mut harness, handles) = build(Some(picked(&song)), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_dialog(&mut harness);
    harness.get_by_label("Split").click();
    harness.run();

    assert_eq!(handles.files.borrow().pick_output_folder_calls, 1);
    let expected = split_names(&song);
    assert!(!expected.is_empty(), "the fixture uses some channels");

    let files = handles.files.borrow();
    let written: Vec<PathBuf> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => Some(path.clone()),
            SaveRequest::Dialog { .. } => None,
        })
        .collect();
    assert_eq!(
        written,
        expected
            .iter()
            .map(|name| dir.join(name))
            .collect::<Vec<_>>(),
        "every channel should be written into the chosen folder"
    );
}

#[test]
fn the_last_written_split_file_reports_the_total() {
    let song = dual_tone_song();
    let (mut harness, handles) = build(Some(picked(&song)), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_dialog(&mut harness);
    harness.get_by_label("Split").click();
    harness.run();

    // One outcome is polled per frame, so feed the batch a frame at a time.
    let count = split_names(&song).len();
    for _ in 0..count {
        handles
            .files
            .borrow_mut()
            .save_outcomes
            .push_back(SaveOutcome::Saved {
                name: "split.wav".to_owned(),
                path: None,
            });
        harness.run();
    }

    assert_eq!(
        harness.state().status,
        format!("Wrote {count} file(s) to {}.", dir.display())
    );
    assert!(
        harness.state().split_flow.is_none(),
        "the flow should be finished"
    );
}

#[test]
fn dismissing_the_folder_picker_cancels_the_split() {
    let (mut harness, handles) = build(Some(picked(&dual_tone_song())), true, false);
    // A dismissed picker.
    handles.files.borrow_mut().output_folders.push_back(None);

    open_split_dialog(&mut harness);
    harness.get_by_label("Split").click();
    harness.run();

    assert_eq!(harness.state().status, "Split cancelled.");
    assert!(harness.state().split_flow.is_none());
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing should be written"
    );
}

#[test]
fn a_failed_split_file_is_reported_once_for_the_batch() {
    let song = dual_tone_song();
    let (mut harness, handles) = build(Some(picked(&song)), true, false);
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(PathBuf::from("C:/out")));

    open_split_dialog(&mut harness);
    harness.get_by_label("Split").click();
    harness.run();

    let count = split_names(&song).len();
    for index in 0..count {
        let outcome = if index == 0 {
            SaveOutcome::Failed("disk full".to_owned())
        } else {
            SaveOutcome::Saved {
                name: "split.wav".to_owned(),
                path: None,
            }
        };
        handles.files.borrow_mut().save_outcomes.push_back(outcome);
        harness.run();
    }

    assert_eq!(
        harness.state().status,
        "Some split files could not be written."
    );
    assert!(
        harness.state().alerts.is_empty(),
        "one status line, not an alert per file"
    );
}

#[test]
fn loading_a_song_abandons_a_split_of_the_previous_one() {
    // Noop tasks, so the split stays mid-render for the duration of the test.
    let (mut harness, handles) = harness_with_song(&dual_tone_song());
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(PathBuf::from("C:/out")));

    open_split_dialog(&mut harness);
    harness.get_by_label("Split").click();
    harness.run();
    assert!(harness.state().split_flow.is_some(), "a split is in flight");

    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&tone_song())));
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Open").click();
    harness.run();

    assert!(harness.state().split_flow.is_none());
    assert!(handles.tasks.borrow().cancelled.contains(&TaskKind::Split));
}

#[test]
fn a_vgm_can_be_split_too() {
    // The song-data split captures a VGM as VGMs, so both formats are offered
    // whatever the song is.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();

    open_split_dialog(&mut harness);
    assert!(harness.state().dialogs.split.is_some());
    assert!(harness.query_by_label_contains("Song data").is_some());
}

// -- split songs -------------------------------------------------------------

/// Opens File > Split Songs...
fn open_split_songs_dialog(harness: &mut Harness<'static, DroApp>) {
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Split Songs").click();
    harness.run();
}

#[test]
fn split_songs_is_offered_for_vgm_and_dro() {
    // A VGM capture opens the dialog with its detected songs.
    let (mut harness, _handles) = harness_with_song(&multi_song_capture());
    open_split_songs_dialog(&mut harness);
    assert!(harness.state().dialogs.split_songs.is_some());
    assert!(harness.query_by_label_contains("song(s) found").is_some());

    // A DRO capture works too (pieces are written as DROs).
    let (mut harness, _handles) = harness_with_song(&multi_song_capture_dro());
    open_split_songs_dialog(&mut harness);
    assert!(harness.state().dialogs.split_songs.is_some());
    assert!(harness.query_by_label_contains("song(s) found").is_some());
}

#[test]
fn exporting_a_dro_capture_writes_numbered_dro_files() {
    let (mut harness, handles) = build(Some(picked(&multi_song_capture_dro())), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_songs_dialog(&mut harness);
    harness.get_by_label_contains("Export").click();
    harness.run();

    let files = handles.files.borrow();
    let written: Vec<PathBuf> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => Some(path.clone()),
            SaveRequest::Dialog { .. } => None,
        })
        .collect();
    assert_eq!(
        written,
        ["01 capture.dro", "02 capture.dro", "03 capture.dro"]
            .iter()
            .map(|name| dir.join(name))
            .collect::<Vec<_>>(),
        "three numbered DRO songs written into the chosen folder"
    );
}

#[test]
fn exporting_songs_writes_a_numbered_file_per_song() {
    let (mut harness, handles) = build(Some(picked(&multi_song_capture())), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_songs_dialog(&mut harness);
    harness.get_by_label_contains("Export").click();
    harness.run();

    assert_eq!(handles.files.borrow().pick_output_folder_calls, 1);
    let files = handles.files.borrow();
    let written: Vec<PathBuf> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => Some(path.clone()),
            SaveRequest::Dialog { .. } => None,
        })
        .collect();
    assert_eq!(
        written,
        ["01 capture.vgm", "02 capture.vgm", "03 capture.vgm"]
            .iter()
            .map(|name| dir.join(name))
            .collect::<Vec<_>>(),
        "three numbered songs written into the chosen folder"
    );
}

#[test]
fn the_song_split_offers_to_open_the_folder_as_a_rip_project() {
    let (mut harness, handles) = build(Some(picked(&multi_song_capture())), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_songs_dialog(&mut harness);
    harness.get_by_label_contains("Export").click();
    harness.run();

    // Feed a Saved outcome per written file; the offer appears once the last lands.
    for _ in 0..3 {
        handles
            .files
            .borrow_mut()
            .save_outcomes
            .push_back(SaveOutcome::Saved {
                name: "song.vgm".to_owned(),
                path: None,
            });
        harness.run();
    }
    assert_eq!(
        harness.state().status,
        format!("Wrote 3 song(s) to {}.", dir.display())
    );

    // The completion alert offers the rip handoff; accepting opens the folder.
    assert!(
        harness
            .query_by_label_contains("Open the folder as a rip project")
            .is_some()
    );
    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        handles.files.borrow().opened_folder_paths.contains(&dir),
        "accepting the offer opens the folder as a rip project"
    );
}

#[test]
fn dismissing_the_folder_picker_cancels_the_song_split() {
    let (mut harness, handles) = build(Some(picked(&multi_song_capture())), true, false);
    handles.files.borrow_mut().output_folders.push_back(None);

    open_split_songs_dialog(&mut harness);
    harness.get_by_label_contains("Export").click();
    harness.run();

    assert_eq!(harness.state().status, "Split cancelled.");
    assert!(harness.state().split_flow.is_none());
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing should be written"
    );
}

#[test]
fn previewing_a_song_seeks_to_its_start_and_plays() {
    // A single-song VGM: one segment, so its lone Preview button is unambiguous.
    let (mut harness, handles) = build(Some(picked(&redundant_vgm_song())), false, false);
    open_split_songs_dialog(&mut harness);

    harness.get_by_label_contains("Preview").click();
    harness.run_steps(3); // playback requests repaints; `run` would spin.

    let audio = handles.audio.borrow();
    assert!(audio.play_calls >= 1, "preview should start playback");
    assert!(
        audio.seeks_pos.contains(&0),
        "preview should seek to the song's first instruction"
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
fn snapshot_render_wav_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    open_render_wav_dialog(&mut harness);
    settled_snapshot(&mut harness, "render_wav_dialog");
}

#[test]
fn snapshot_split_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    open_split_dialog(&mut harness);
    settled_snapshot(&mut harness, "split_dialog");
}

#[test]
fn snapshot_split_songs_dialog() {
    let (mut harness, _handles) = build(Some(picked(&multi_song_capture())), false, true);
    open_split_songs_dialog(&mut harness);
    settled_snapshot(&mut harness, "split_songs_dialog");
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
fn scanning_rip_volumes_fills_the_peak_map() {
    // Inline tasks run the whole-pack scan; a two-VGM folder.
    let (mut harness, handles) = build(None, true, false);
    open_folder(&mut harness, &handles, cool_game_folder());

    act(&mut harness, Action::RipScanVolumes);
    // The inline scan stores its RipPeaks on submit; a poll frame delivers them.
    for _ in 0..4 {
        harness.step();
    }

    let peaks = &harness.state().rip.as_ref().expect("a rip is open").peaks;
    assert_eq!(peaks.len(), 2, "both tracks measured: {peaks:?}");
    assert!(peaks.contains_key("01 Intro.vgz"));
    assert!(peaks.contains_key("02 Boss.vgm"));
}

#[test]
fn a_rip_preview_starts_at_the_tracks_modifier_volume() {
    let (mut harness, handles) = tall_rip_harness();
    // A one-track pack whose track's header modifier asks for 2x (0x20).
    let track = PickedFile {
        name: "01 Loud.vgm".to_owned(),
        path: Some(PathBuf::from("C:/pack/01 Loud.vgm")),
        bytes: dro_core::io::write_song(&vgm_with_modifier(0x20)).unwrap(),
    };
    open_folder(&mut harness, &handles, rip_folder("Loud Pack", vec![track]));

    act(&mut harness, Action::RipTrackPreview(0));

    assert_eq!(
        handles.audio.borrow().loaded_boost,
        Some(2.0),
        "the preview loads at the track's 2x header modifier"
    );
    // ...and the editor's stored volume is left untouched by the preview.
    assert_eq!(
        harness.state().config.audio.boost,
        1.0,
        "previewing does not disturb the editor volume"
    );
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

/// An overlay that writes one field (by GD3 index) to a given value.
fn overlay_writing(index: usize, value: &str) -> crate::rip::BulkTagOverlay {
    let mut overlay = crate::rip::BulkTagOverlay::default();
    overlay.apply[index] = true;
    overlay.values[index] = value.to_owned();
    overlay
}

/// Reads back a written VGM/VGZ and returns its GD3 tag.
fn tag_of(name: &str, bytes: &[u8]) -> dro_core::Gd3Tag {
    dro_core::io::read_song(name, bytes)
        .unwrap()
        .vgm_meta()
        .unwrap()
        .tag
        .clone()
        .unwrap_or_default()
}

/// Drives a rip run to completion: feed one save outcome per write, plus the
/// rescan folder, then step the frame loop.
fn settle_rip_run(harness: &mut Harness<'static, DroApp>, handles: &Handles, writes: usize) {
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..writes {
            files.save_outcomes.push_back(SaveOutcome::Saved {
                name: "written".to_owned(),
                path: None,
            });
        }
        files.picked_folders.push_back(Ok(cool_game_folder()));
    }
    harness.run_steps(writes + 4);
}

const GD3_TRACK_AUTHOR_EN: usize = 6;
const GD3_GAME_NAME_EN: usize = 2;

#[test]
fn bulk_tag_rewrites_every_selected_track_with_the_checked_field() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Push a new composer onto both tracks; every other field is left alone.
    harness.state_mut().bulk_tag_submitted(
        vec!["01 Intro.vgz".to_owned(), "02 Boss.vgm".to_owned()],
        overlay_writing(GD3_TRACK_AUTHOR_EN, "New Composer"),
    );
    settle_rip_run(&mut harness, &handles, 2);

    let files = handles.files.borrow();
    let writes: Vec<(&PathBuf, &Vec<u8>)> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, bytes } => Some((path, bytes)),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 2, "both selected tracks are rewritten");

    for (path, bytes) in writes {
        let name = path.file_name().unwrap().to_string_lossy();
        let tag = tag_of(&name, bytes);
        assert_eq!(
            tag.track_author_en, "New Composer",
            "{name}: author written"
        );
        // Untouched fields keep each track's existing values.
        assert_eq!(tag.game_name_en, "Cool Game", "{name}: game name kept");
        assert_eq!(tag.creator, "Ripper", "{name}: creator kept");
    }
    // The whole bulk edit is one undoable step.
    assert_eq!(
        harness.state().rip_undo.len(),
        1,
        "one transaction, one undo"
    );
}

#[test]
fn bulk_tag_can_target_a_subset_of_tracks() {
    // The composer differs across the pack: only 02 Boss gets the new author.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().bulk_tag_submitted(
        vec!["02 Boss.vgm".to_owned()],
        overlay_writing(GD3_TRACK_AUTHOR_EN, "Only Bob"),
    );
    settle_rip_run(&mut harness, &handles, 1);

    let files = handles.files.borrow();
    let writes: Vec<&PathBuf> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 1, "only the one selected track is rewritten");
    assert!(
        writes[0].to_string_lossy().ends_with("02 Boss.vgm"),
        "the subset targeted 02 Boss, got {:?}",
        writes[0]
    );
}

#[test]
fn bulk_tag_skips_tracks_whose_tag_would_not_change() {
    // Writing the game name every track already has changes nothing, so no file
    // is rewritten and the run never starts.
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().bulk_tag_submitted(
        vec!["01 Intro.vgz".to_owned(), "02 Boss.vgm".to_owned()],
        overlay_writing(GD3_GAME_NAME_EN, "Cool Game"),
    );

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "an all-no-op bulk edit writes nothing"
    );
    assert!(
        harness.state().status.contains("nothing changed"),
        "it says so; status was {:?}",
        harness.state().status
    );
    assert!(harness.state().rip_undo.is_empty(), "nothing to undo");
}

#[test]
fn bulk_tag_button_opens_a_dialog() {
    let (mut harness, handles) = tall_rip_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.get_by_label_contains("Bulk Tag").click();
    harness.run();
    assert!(
        harness.state().dialogs.bulk_tag.is_some(),
        "the Bulk Tag button opens the dialog"
    );
}

const PNG_FIXTURE: &[u8] = include_bytes!("../../../tests/screenshot.png");

/// A folder that passes every export validation (named, numbered, with a png).
/// The png is a real (decodable) image so the inline preview renders.
/// A VGM fixture re-serialised under `name` carrying `tag`.
fn vgm_with_tag(name: &str, tag: dro_core::Gd3Tag) -> PickedFile {
    let mut song = dro_core::io::read_song(name, VGM_FIXTURE).unwrap();
    if let Some(meta) = song.vgm_meta_mut() {
        meta.tag = Some(tag);
    }
    PickedFile {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/pack/{name}"))),
        bytes: dro_core::io::write_song(&song).unwrap(),
    }
}

/// A single VGM with every submission-required GD3 field filled, so the
/// readiness checks pass clean. The track name matches the file name, and the
/// game/system/date/ripper agree with the pack meta the app prefills.
fn complete_vgm(name: &str) -> PickedFile {
    vgm_with_tag(
        name,
        dro_core::Gd3Tag {
            track_name_en: dro_core::rip::title_from_filename(name).to_owned(),
            game_name_en: "Cool Game".to_owned(),
            system_name_en: "IBM PC/AT".to_owned(),
            track_author_en: "Ada".to_owned(),
            release_date: "1994".to_owned(),
            creator: "Ripper".to_owned(),
            ..dro_core::Gd3Tag::default()
        },
    )
}

/// A pack with a spread of readiness problems across the checklist categories,
/// for the submission-checklist tests and snapshot. Track 1 is missing its
/// System and carries a slash-separated date (which the app also prefills into
/// the pack meta); track 2's game name disagrees with the pack, its file name
/// drifts from its Track Name, and it has no composer. There is no screenshot.
fn dirty_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![
            vgm_with_tag(
                "01 Intro.vgz",
                dro_core::Gd3Tag {
                    track_name_en: "Intro".to_owned(),
                    game_name_en: "Cool Game".to_owned(),
                    track_author_en: "Ada".to_owned(),
                    release_date: "1994/03".to_owned(),
                    creator: "Ripper".to_owned(),
                    ..dro_core::Gd3Tag::default()
                },
            ),
            vgm_with_tag(
                "02 Boss.vgz",
                dro_core::Gd3Tag {
                    track_name_en: "Boss Theme".to_owned(),
                    game_name_en: "Different Game".to_owned(),
                    system_name_en: "IBM PC/AT".to_owned(),
                    release_date: "1994/03".to_owned(),
                    creator: "Ripper".to_owned(),
                    ..dro_core::Gd3Tag::default()
                },
            ),
        ],
    )
}

/// A submission-ready "Cool Game" pack: one fully tagged track and a screenshot,
/// so [`RipState::validations`] finds nothing to warn about and an export goes
/// straight through without the "export anyway?" confirm.
fn complete_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![
            complete_vgm("01 Intro.vgz"),
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
        assert!(job.optimize_vgms, "optimise-on-export defaults on");
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

/// A tall interaction harness with the dirty pack open, so the whole submission
/// checklist is on-screen and its lines are real click targets.
fn dirty_checklist_harness() -> (Harness<'static, DroApp>, Handles) {
    let (mut harness, handles) = build_sized(None, false, false, egui::vec2(1280.0, 1400.0));
    open_folder(&mut harness, &handles, dirty_folder());
    harness.run();
    (harness, handles)
}

#[test]
fn the_submission_checklist_lists_the_readiness_problems() {
    let (harness, _handles) = dirty_checklist_harness();
    // A line from each of several categories: track tags, consistency, files.
    let _ = harness.get_by_label_contains("01 Intro: missing System");
    let _ = harness.get_by_label_contains("differs from the pack's");
    let _ = harness.get_by_label_contains("There is no screenshot");
}

#[test]
fn clicking_a_checklist_track_item_opens_that_tracks_quick_edit() {
    let (mut harness, _handles) = dirty_checklist_harness();
    // The consistency line belongs to track 2 ("02 Boss.vgz").
    harness
        .get_by_label_contains("differs from the pack's")
        .click();
    harness.run();
    let state = harness.state();
    let dialog = state
        .dialogs
        .track_edit
        .as_ref()
        .expect("the quick-edit dialog opened");
    assert_eq!(dialog.original_name(), "02 Boss.vgz");
}

#[test]
fn clicking_a_meta_checklist_item_focuses_its_form_field() {
    let (mut harness, _handles) = dirty_checklist_harness();
    // The bad pack date is a Package-info line targeting the release-date field.
    harness
        .get_by_label_contains("should be a hyphen-separated date")
        .click();
    harness.run();
    // The form consumed the focus request (so it does not re-fire next frame)...
    assert!(
        harness.state().rip.as_ref().unwrap().focus_field.is_none(),
        "the form takes focus_field the frame after the click"
    );
    // ...and a form field now holds keyboard focus.
    assert!(
        harness.ctx.memory(|memory| memory.focused()).is_some(),
        "a field took focus"
    );
}

#[test]
fn converting_dates_to_hyphens_fixes_the_pack_in_one_undoable_step() {
    let (mut harness, handles) = dirty_checklist_harness();
    // The date the app prefilled from the first track is slash-separated.
    assert_eq!(
        harness.state().rip.as_ref().unwrap().meta.release_date,
        "1994/03"
    );
    // The fix-assist button is offered while a slash date remains.
    let _ = harness.get_by_label("Convert dates to hyphens");

    // Feed Ok save outcomes for the two track writes, then the rescan folder the
    // batch installs -- now carrying hyphenated GD3 dates.
    {
        let mut files = handles.files.borrow_mut();
        files.save_outcomes.push_back(SaveOutcome::Saved {
            name: "01 Intro.vgz".to_owned(),
            path: None,
        });
        files.save_outcomes.push_back(SaveOutcome::Saved {
            name: "02 Boss.vgz".to_owned(),
            path: None,
        });
        files.picked_folders.push_back(Ok(rip_folder(
            "Cool Game",
            vec![
                vgm_with_tag(
                    "01 Intro.vgz",
                    dro_core::Gd3Tag {
                        track_name_en: "Intro".to_owned(),
                        game_name_en: "Cool Game".to_owned(),
                        track_author_en: "Ada".to_owned(),
                        release_date: "1994-03".to_owned(),
                        creator: "Ripper".to_owned(),
                        ..dro_core::Gd3Tag::default()
                    },
                ),
                vgm_with_tag(
                    "02 Boss.vgz",
                    dro_core::Gd3Tag {
                        track_name_en: "Boss Theme".to_owned(),
                        game_name_en: "Different Game".to_owned(),
                        system_name_en: "IBM PC/AT".to_owned(),
                        release_date: "1994-03".to_owned(),
                        creator: "Ripper".to_owned(),
                        ..dro_core::Gd3Tag::default()
                    },
                ),
            ],
        )));
    }

    // Dispatch the fix-assist directly (as the button does), so the batch is
    // built from the current slash dates before any frame's folder poll runs --
    // the same pattern the reorder test uses.
    act(&mut harness, Action::RipConvertDatesToHyphens);
    harness.run_steps(16);

    let state = harness.state();
    let rip = state.rip.as_ref().unwrap();
    // The pack date converted immediately (a form edit)...
    assert_eq!(rip.meta.release_date, "1994-03");
    // ...and the rescan installed both tracks with hyphenated GD3 dates.
    for track in &rip.tracks {
        let tag = track
            .song()
            .unwrap()
            .vgm_meta()
            .unwrap()
            .tag
            .as_ref()
            .unwrap();
        assert_eq!(tag.release_date, "1994-03", "{} converted", track.file_name);
    }
    // The track rewrites landed as one undoable batch, and no slash date is left.
    assert_eq!(state.rip_undo.len(), 1, "one undoable batch");
    assert!(
        !rip.has_convertible_dates(),
        "the fix-assist has nothing left"
    );
}

#[test]
fn snapshot_rip_checklist_dirty() {
    // Wider than the other rip snapshots so the crowded toolbar fits and the
    // checklist's glyphs and category headings are all in frame.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1280.0, 1200.0));
    open_folder(&mut harness, &handles, dirty_folder());
    harness.run();
    settled_snapshot(&mut harness, "rip_checklist_dirty");
}

#[test]
fn snapshot_rip_checklist_clean() {
    // A submission-ready pack: the checklist collapses to ticks (the single
    // non-looping track leaves just the optional Loops note).
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1280.0, 1000.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.run();
    settled_snapshot(&mut harness, "rip_checklist_clean");
}

#[test]
fn snapshot_track_edit_dialog() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    open_folder(&mut harness, &handles, single_track_folder());
    harness.get_by_label("Tags").click();
    harness.run();
    settled_snapshot(&mut harness, "track_edit_dialog");
}

#[test]
fn snapshot_bulk_tag_dialog() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.get_by_label_contains("Bulk Tag").click();
    harness.run();
    settled_snapshot(&mut harness, "bulk_tag_dialog");
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

#[test]
fn shift_brackets_the_loop_with_the_two_mouse_buttons() {
    use super::waveform_action;

    // Shift+left marks the start, Shift+right the end -- one gesture apart.
    assert_eq!(
        waveform_action(7, 500, false, true),
        Some(Action::SetLoopStart(7))
    );
    assert_eq!(
        waveform_action(7, 500, true, true),
        Some(Action::SetLoopEnd(7))
    );
    // Unmodified, the left button still seeks...
    assert_eq!(
        waveform_action(7, 500, false, false),
        Some(Action::WaveformClicked { index: 7, ms: 500 })
    );
    // ...and the right button does nothing at all.
    assert_eq!(waveform_action(7, 500, true, false), None);
}

// -- find loop (lf-3) --------------------------------------------------------

#[test]
fn find_loop_is_offered_for_both_dro_and_vgm() {
    // A DRO has nowhere to store a loop, but marking and auditioning one still
    // work, so the search is offered -- only the dialog's Apply is VGM-gated.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.get_by_label("Edit").click();
    harness.run();
    assert!(
        harness.query_by_label_contains("Find Loop").is_some(),
        "Find Loop should be on the Edit menu for a DRO"
    );

    let (mut harness, _handles) = harness_with_song(&looping_vgm());
    harness.get_by_label("Edit").click();
    harness.run();
    assert!(
        harness.query_by_label_contains("Find Loop").is_some(),
        "Find Loop should be on the Edit menu for a VGM"
    );
}

#[test]
fn searching_finds_a_loop_and_applying_writes_it() {
    // Inline tasks so the background search runs synchronously on submit.
    let (mut harness, _handles) = build(Some(picked(&looping_vgm())), true, false);
    act(&mut harness, Action::OpenFindLoop);
    assert!(harness.state().dialogs.find_loop.is_some());

    // Four commands is the body length; the search finds the one repeat.
    act(
        &mut harness,
        Action::FindLoopSearch {
            min_len_commands: 4,
        },
    );
    harness.run(); // a poll frame delivers the streamed candidates

    // The found loop renders as a row: it starts at 0.5 s and ends at 1.0 s.
    assert!(
        harness.query_by_label_contains("0:01.0").is_some(),
        "the found loop's end time should be listed in the table"
    );

    // The top candidate is pre-selected, so Apply writes it straight into the
    // VGM's loop metadata.
    harness.get_by_label("Apply").click();
    harness.run();
    let song = harness.state().editor.song().unwrap();
    let meta = song.vgm_meta().unwrap();
    assert_eq!(
        meta.loop_point,
        Some(3),
        "loop point at the body's first write"
    );
    assert_eq!(meta.loop_end, Some(9), "loop end where the repeat begins");
}

#[test]
fn cancelling_a_search_stops_it() {
    let (mut harness, handles) = build(Some(picked(&looping_vgm())), false, false);
    act(&mut harness, Action::OpenFindLoop);
    act(&mut harness, Action::CancelLoopSearch);
    assert!(
        handles
            .tasks
            .borrow()
            .cancelled
            .contains(&TaskKind::LoopSearch),
        "Cancel should cancel the loop-search task"
    );
}

#[test]
fn snapshot_find_loop_dialog() {
    // Inline tasks render a real result; wgpu renders the pixels.
    let (mut harness, _handles) = build(Some(picked(&looping_vgm())), true, true);
    act(&mut harness, Action::OpenFindLoop);
    act(
        &mut harness,
        Action::FindLoopSearch {
            min_len_commands: 4,
        },
    );
    harness.run();
    settled_snapshot(&mut harness, "find_loop_dialog");
}

// -- format-specific menu items ----------------------------------------------

/// Opens the Edit menu and reports which of the format-specific items are on it.
fn edit_menu_items(harness: &mut Harness<'static, DroApp>) -> Vec<&'static str> {
    harness.get_by_label("Edit").click();
    harness.run();
    let present: Vec<&'static str> = [
        "DRO Info...",
        "Edit Tag",
        "Edit VGM Metadata",
        "Optimize VGM",
        "Apply Loop to Metadata",
    ]
    .into_iter()
    // `_contains`, not an exact match: an item carrying a shortcut hint folds
    // that hint into its accessible label ("DRO Info...", "Ctrl+I").
    .filter(|label| harness.query_by_label_contains(label).is_some())
    .collect();
    // Close the menu again so the next open starts clean.
    harness.key_press(Key::Escape);
    harness.run();
    present
}

/// Opens File > Convert and reports which conversions it offers. Empty when the
/// Convert submenu is not shown at all (a VGM, or no song).
fn convert_menu_items(harness: &mut Harness<'static, DroApp>) -> Vec<&'static str> {
    harness.get_by_label("File").click();
    harness.run();
    // The submenu header renders as "Convert ⏵" (with a submenu arrow); its
    // children ("Convert to ...") render only once it is expanded, so until then
    // "Convert" matches the header alone.
    let present = if harness.query_by_label_contains("Convert").is_some() {
        harness.get_by_label_contains("Convert").click();
        harness.run();
        ["Convert to VGM", "Convert to DRO v1"]
            .into_iter()
            .filter(|label| harness.query_by_label_contains(label).is_some())
            .collect()
    } else {
        Vec::new()
    };
    harness.key_press(Key::Escape);
    harness.run();
    present
}

#[test]
fn a_dro_shows_only_the_dro_menu_items() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert_eq!(
        edit_menu_items(&mut harness),
        ["DRO Info..."],
        "a DRO has no tag, no VGM header and nowhere to store a loop"
    );
}

#[test]
fn only_a_dro_can_be_converted() {
    // A DRO offers the Convert submenu; a v1 can go to VGM, a v2 also to v1.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert_eq!(
        convert_menu_items(&mut harness),
        ["Convert to VGM"],
        "a v1 has nowhere further down to go"
    );

    let (mut harness, _handles) = harness_with_song(&dro_song_v2());
    assert_eq!(
        convert_menu_items(&mut harness),
        ["Convert to VGM", "Convert to DRO v1"],
    );

    // A VGM has no format this app can convert it to: no submenu at all.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    assert!(convert_menu_items(&mut harness).is_empty());
}

#[test]
fn converting_to_dro_v1_renames_the_song_and_clears_its_path() {
    let (mut harness, _handles) = harness_with_song(&dro_song_v2());
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Convert").click();
    harness.run();
    harness.get_by_label_contains("Convert to DRO v1").click();
    harness.run();

    let state = harness.state();
    let song = state.editor.song().expect("still loaded");
    assert_eq!(song.file_version, dro_core::song::DRO_FILE_V1);
    // The `_1` output name, so a Save As cannot overwrite the v2 source...
    assert_eq!(song.name, "test_1.dro");
    // ...and neither can a plain Save, which now has nowhere to write.
    assert!(state.editor.path.is_none());
    assert_eq!(state.status, "Successfully converted to DRO v1");

    // Converting again offers only the VGM direction: v1 is as far down as it goes.
    assert_eq!(convert_menu_items(&mut harness), ["Convert to VGM"]);
}

#[test]
fn a_vgm_shows_only_the_vgm_menu_items() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    assert_eq!(
        edit_menu_items(&mut harness),
        [
            "Edit Tag",
            "Edit VGM Metadata",
            "Optimize VGM",
            "Apply Loop to Metadata"
        ],
        "a VGM has no DRO header to inspect and is already converted"
    );
}

#[test]
fn with_no_song_no_format_specific_items_show() {
    let (mut harness, _handles) = empty_harness();
    assert!(edit_menu_items(&mut harness).is_empty());
    assert!(convert_menu_items(&mut harness).is_empty());
}

// -- Optimize VGM (cmp-3) ----------------------------------------------------

#[test]
fn optimizing_a_vgm_strips_writes_and_reports_the_saving() {
    let (mut harness, _handles) = harness_with_song(&redundant_vgm_song());
    let before = harness.state().editor.len();

    act(&mut harness, Action::OptimizeVgm);

    let state = harness.state();
    assert!(
        state.editor.len() < before,
        "optimising should remove commands"
    );
    // The two redundant writes go, and the delays they separated merge into one.
    assert_eq!(state.editor.len(), before - 3);
    assert!(
        state.status.contains("Optimized") && state.status.contains("saved"),
        "status should report the saving, got {:?}",
        state.status
    );
}

#[test]
fn optimize_undo_then_redo_restores_the_exact_bytes() {
    let (mut harness, _handles) = harness_with_song(&redundant_vgm_song());
    let original = harness.state().editor.song().unwrap().data().raw().to_vec();

    act(&mut harness, Action::OptimizeVgm);
    let optimised = harness.state().editor.song().unwrap().data().raw().to_vec();
    assert_ne!(optimised, original, "optimising should change the stream");

    act(&mut harness, Action::Undo);
    assert_eq!(
        harness.state().editor.song().unwrap().data().raw(),
        original.as_slice(),
        "undo must restore the original bytes exactly"
    );

    act(&mut harness, Action::Redo);
    assert_eq!(
        harness.state().editor.song().unwrap().data().raw(),
        optimised.as_slice(),
        "redo must re-apply the optimisation exactly"
    );
}

#[test]
fn optimizing_a_dro_is_refused() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let before = harness.state().editor.len();

    act(&mut harness, Action::OptimizeVgm);

    let state = harness.state();
    assert_eq!(state.editor.len(), before, "a DRO must be left untouched");
    assert!(state.status.contains("Only VGMs"), "got {:?}", state.status);
}

#[test]
fn optimizing_an_already_optimal_vgm_reports_nothing() {
    // A freshly converted VGM has no redundant writes.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    let before = harness.state().editor.len();

    act(&mut harness, Action::OptimizeVgm);

    let state = harness.state();
    assert_eq!(state.editor.len(), before, "nothing should change");
    assert!(
        state.status.contains("Nothing to optimize"),
        "got {:?}",
        state.status
    );
}

#[test]
fn optimize_re_derives_the_loop_markers_from_the_remapped_loop() {
    // Loop point at the key-off write (index 7); stripping the two redundant
    // writes before it slides and re-indexes it.
    let mut song = redundant_vgm_song();
    song.vgm_meta_mut().unwrap().loop_point = Some(7);
    let (mut harness, _handles) = harness_with_song(&song);
    assert_eq!(
        harness.state().editor.markers.start(),
        7,
        "loaded loop point"
    );

    act(&mut harness, Action::OptimizeVgm);

    let state = harness.state();
    let remapped = state
        .editor
        .song()
        .unwrap()
        .vgm_meta()
        .unwrap()
        .loop_point
        .expect("the loop survives optimisation");
    // The markers were re-derived from the song's remapped loop, so they agree.
    assert_eq!(state.editor.markers.start(), remapped);
    assert!(
        remapped < 7,
        "the loop point slid left past the stripped writes, got {remapped}"
    );
}

#[test]
fn the_dro_info_shortcut_is_refused_for_a_vgm() {
    // The menu hides the item for a VGM, so Ctrl+I must agree rather than open
    // a dialog the menu says does not apply.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();

    assert!(harness.state().dialogs.dro_info.is_none());
    assert!(
        harness.state().status.contains("VGM Metadata"),
        "it points at the dialog that does apply; status was {:?}",
        harness.state().status
    );
}

#[test]
fn dro_info_offers_editing_when_the_setting_is_on() {
    let (mut harness, _handles) = harness_with_song(&tone_song());

    // "Edit" is also the menu bar's Edit menu, which is always on screen, so
    // the dialog's button is the *second* node with that label.
    let edit_nodes =
        |harness: &mut Harness<'static, DroApp>| harness.get_all_by_label("Edit").count();

    // Off by default: the dialog is view-only, so only the menu answers.
    act(&mut harness, Action::OpenDroInfo);
    harness.run();
    assert!(harness.state().dialogs.dro_info.is_some());
    assert_eq!(
        edit_nodes(&mut harness),
        1,
        "view-only until the setting is on"
    );
    harness.get_by_label("Close").click();
    harness.run();

    harness.state_mut().config.ui.dro_info_edit_enabled = true;
    act(&mut harness, Action::OpenDroInfo);
    harness.run();
    assert_eq!(
        edit_nodes(&mut harness),
        2,
        "the setting is on, so the dialog should offer Edit"
    );

    // And Edit actually unlocks the fields: the button becomes Save.
    harness
        .get_all_by_label("Edit")
        .nth(1)
        .expect("the dialog's Edit button")
        .click();
    harness.run();
    assert!(harness.query_by_label("Save").is_some(), "now in edit mode");
    assert_eq!(edit_nodes(&mut harness), 1, "the button became Save");
}

#[test]
fn clicking_a_settings_caption_toggles_its_checkbox() {
    // The caption used to be inert text, so clicking the words did nothing and
    // only the small box itself worked -- which reads as a setting that does
    // not work at all.
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(!harness.state().config.ui.dro_info_edit_enabled);

    let config = harness.state().config;
    harness.state_mut().dialogs.settings = Some(crate::dialogs::SettingsDialog::new(&config));
    harness.run();
    harness.get_by_label("Allow editing in DRO Info").click();
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();

    assert!(
        harness.state().config.ui.dro_info_edit_enabled,
        "clicking the caption should have toggled the setting"
    );
    assert_eq!(
        handles
            .saved_configs
            .borrow()
            .last()
            .map(|c| c.ui.dro_info_edit_enabled),
        Some(true),
        "and it should have been persisted"
    );

    // The DRO Info dialog now offers editing -- the user-visible payoff.
    act(&mut harness, Action::OpenDroInfo);
    harness.run();
    assert_eq!(
        harness.get_all_by_label("Edit").count(),
        2,
        "the menu's Edit, plus the dialog's now-available Edit button"
    );
}
