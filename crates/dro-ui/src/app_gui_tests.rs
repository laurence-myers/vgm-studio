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
use crate::action::AppTab;
use crate::platform::{PickedFile, PickedFolder, RipJobOutcome, SaveOutcome, SaveRequest};
use crate::tasks::TaskKind;
use crate::test_song::{bogus_leading_delay_song, tone_song};
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
fn previewing_a_track_plays_it_and_stop_halts_it() {
    let (mut harness, handles) = empty_harness();
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
fn quick_edit_opens_a_dialog_and_saves_a_rewrite() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.get_by_label("Edit\u{2026}").click();
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

/// A folder that passes every export validation (named, numbered, with a png).
fn complete_folder() -> PickedFolder {
    rip_folder(
        "Cool Game",
        vec![
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            PickedFile {
                name: "Cool Game.png".to_owned(),
                path: Some(PathBuf::from("C:/Cool Game/Cool Game.png")),
                bytes: b"\x89PNG\r\n\x1a\n fake".to_vec(),
            },
        ],
    )
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
    let (mut harness, handles) = build(None, false, true);
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.run();
    harness.snapshot("rip_view");
}

#[test]
fn snapshot_track_edit_dialog() {
    let (mut harness, handles) = build(None, false, true);
    open_folder(&mut harness, &handles, single_track_folder());
    harness.get_by_label("Edit\u{2026}").click();
    harness.run();
    harness.snapshot("track_edit_dialog");
}
