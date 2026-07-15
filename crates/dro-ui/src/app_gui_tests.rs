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

use super::DroApp;
use crate::platform::{PickedFile, SaveOutcome, SaveRequest};
use crate::tasks::TaskKind;
use crate::test_song::{bogus_leading_delay_song, tone_song};
use crate::test_support::{
    AudioLog, FakeAudioService, FakeFileService, FileLog, InlineTaskService, MemoryConfigStore,
    NoopTaskService, TaskLog,
};

/// Shared handles onto the fake services, for scripting and inspection.
struct Handles {
    files: Rc<RefCell<FileLog>>,
    audio: Rc<RefCell<AudioLog>>,
    tasks: Rc<RefCell<TaskLog>>,
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
    let files = Rc::new(RefCell::new(FileLog::default()));
    let audio = Rc::new(RefCell::new(AudioLog::default()));
    let tasks = Rc::new(RefCell::new(TaskLog::default()));
    let saved_configs = Rc::new(RefCell::new(Vec::new()));

    let handles = Handles {
        files: files.clone(),
        audio: audio.clone(),
        tasks: tasks.clone(),
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
            Box::new(MemoryConfigStore {
                initial: AppConfig::default(),
                saved: saved_configs,
            }),
            initial,
        )
    };

    // Tall enough for the five stacked panels plus table rows. `max_steps` well
    // above the default 4 gives settling room; playback tests still avoid `run`.
    let builder = Harness::builder()
        .with_size(egui::vec2(1000.0, 720.0))
        .with_max_steps(64);
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
    for header in ["Pos.", "Bank", "Reg.", "Value", "Description"] {
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
        assert_eq!(audio.load_count, 1, "the song is loaded into the output once");
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
        Some((TaskKind::RenderWaveform, Some(std::time::Duration::from_secs(1))))
    );

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    harness.run();

    assert_eq!(harness.state().editor.len(), full_len, "undo restores the row");
    assert!(harness.state().status.starts_with("Undone:"));
}

#[test]
fn edit_menu_opens_goto_dialog_and_it_jumps_the_selection() {
    let (mut harness, _handles) = harness_with_song(&tone_song());

    harness.get_by_label("Edit").click();
    harness.run();
    harness.get_by_label_contains("Goto").click();
    harness.run();

    assert!(harness.state().dialogs.goto.is_some(), "Goto dialog should open");
    assert!(harness.query_by_label("Go to instruction:").is_some());

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
    assert_eq!(harness.state().status, "Gone to position: 5");
}

#[test]
fn dro_info_shortcut_opens_modal_and_blocks_playback_keys() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();

    assert!(harness.state().dialogs.dro_info.is_some());
    assert!(harness.query_by_label("DRO Info").is_some(), "heading should render");

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
    handles.files.borrow_mut().save_outcomes.push_back(SaveOutcome::Saved {
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

#[test]
fn snapshot_empty_app() {
    let (mut harness, _handles) = build(None, false, true);
    harness.snapshot("empty_app");
}

#[test]
fn snapshot_loaded_song() {
    // Inline tasks so the waveform is actually rendered for the snapshot.
    let (mut harness, _handles) = build(Some(picked(&tone_song())), true, true);
    harness.state_mut().editor.selection.select_only(0);
    harness.run();
    harness.snapshot("loaded_tone_song");
}

#[test]
fn snapshot_dro_info_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();
    harness.snapshot("dro_info_dialog");
}

#[test]
fn snapshot_auto_trim_alert() {
    let (mut harness, _handles) = build(Some(picked(&bogus_leading_delay_song())), false, true);
    harness.snapshot("auto_trim_alert");
}
