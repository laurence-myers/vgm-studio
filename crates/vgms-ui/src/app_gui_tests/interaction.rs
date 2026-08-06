//! interaction tests (split out of app_gui_tests.rs, st-6).

use super::*;

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
fn e2e_hook_dispatches_actions_and_reports_state() {
    // The web e2e hook (`window.__vgms_e2e`) is a thin JS wrapper over exactly
    // these two methods; pin their contract natively so a browser is not needed
    // to catch a regression in enqueue -> drain -> handle -> snapshot.
    let song = tone_song();
    let (mut harness, _handles) = harness_with_song(&song);

    // The snapshot reflects the loaded document.
    let snap = harness.state().e2e_snapshot();
    assert!(snap.has_document);
    assert_eq!(snap.document_name.as_deref(), Some(song.name.as_str()));
    assert_eq!(snap.row_count, song.len());
    assert_eq!(snap.active_tab, "editor");
    assert!(snap.pack.is_none());

    // An enqueued action runs through the ordinary handler on the next frame.
    harness
        .state_mut()
        .e2e_enqueue_action(Action::Ui(UiAction::Status("e2e-probe".to_owned())));
    harness.run();
    assert_eq!(harness.state().e2e_snapshot().status, "e2e-probe");

    // An action with real effect: closing a clean document (no discard prompt).
    harness
        .state_mut()
        .e2e_enqueue_action(Action::File(FileAction::Close));
    harness.run();
    assert!(!harness.state().e2e_snapshot().has_document);
}

#[test]
fn opening_a_zip_makes_a_memory_pack_that_edits_dirty() {
    // The whole wt-8 flow, natively: a .zip picked as a file becomes an in-memory
    // pack (through the fake's real ArchiveBackend), and a reorder renumbers the
    // archive and marks it dirty -- the same path the Firefox e2e proves.
    const ZIP: &[u8] = include_bytes!("../../../../tests/e2e-pack.zip");
    let (mut harness, handles) = empty_harness();
    handles.files.borrow_mut().picked.push_back(Ok(PickedFile {
        name: "e2e-pack.zip".to_owned(),
        path: None,
        bytes: ZIP.to_vec(),
    }));
    harness.run();

    let pack = harness
        .state()
        .e2e_snapshot()
        .pack
        .expect("the .zip opened as a pack");
    assert_eq!(pack.track_names, ["01 Alpha.vgm", "02 Beta.vgm"]);
    assert!(!pack.dirty, "a freshly opened pack is clean");

    // Reordering renumbers the in-memory archive and marks the pack dirty. The
    // file-op run spans several frames (a rename per poll), so step a fixed
    // number rather than `run()`, which settles before the batch finishes.
    act(
        &mut harness,
        Action::Pack(PackAction::MoveTrack { index: 0, delta: 1 }),
    );
    harness.run_steps(16);
    let pack = harness.state().e2e_snapshot().pack.expect("still a pack");
    assert_eq!(pack.track_names, ["01 Beta.vgm", "02 Alpha.vgm"]);
    assert!(pack.dirty, "a memory pack is dirty after an edit");
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
    // The Pos. column is hex, so Goto parses hex (and an optional 0x).
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

/// A raw OS file drop never passes through egui's interaction layer, so a
/// `Modal` cannot block it. While any dialog is open a drop must be refused, or
/// the song would swap under the dialog and a later Apply would target the old
/// song's rows in the new one (sw-5).
#[test]
fn a_file_drop_is_refused_while_a_dialog_is_open() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    let before = harness.state().editor.len();

    // Open a dialog, then deliver a raw OS drop of a different song.
    harness.state_mut().dialogs.goto = Some(crate::dialogs::GotoDialog::new());
    harness.input_mut().dropped_files.push(egui::DroppedFile {
        name: "other.vgm".to_owned(),
        path: Some(PathBuf::from("C:/songs/other.vgm")),
        ..Default::default()
    });
    harness.run();

    assert_eq!(
        harness.state().editor.len(),
        before,
        "the song must not change under an open dialog"
    );
    assert_eq!(
        harness.state().status,
        crate::strings::APP_STATUS_DROP_DIALOG_OPEN
    );
    assert!(
        handles.files.borrow().opened_paths.is_empty(),
        "the drop never reached the file service"
    );
}

#[test]
fn edit_menu_opens_goto_dialog_and_it_jumps_the_selection() {
    let (mut harness, _handles) = harness_with_song(&tone_song());

    harness.get_by_label("Edit").click();
    harness.run();
    harness.get_by_label_contains("Find").click(); // the Find submenu
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
            vgms_synth::Muting::all(),
            "muting a channel is not the all-audible state"
        );
    }

    harness.key_press(Key::Num3);
    harness.run();
    let audio = handles.audio.borrow();
    assert_eq!(audio.mutings.len(), 2);
    assert_eq!(
        *audio.mutings.last().unwrap(),
        vgms_synth::Muting::all(),
        "toggling the same channel back restores everything"
    );
}

#[test]
fn a_held_modifier_suppresses_plain_editor_keys() {
    // A plain editor key must not fire with Command/Alt held.
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
    // Shift+1..9 reach channels 9..17. Muting channel 0 (plain 1) then Shift+1
    // must NOT restore all-audible -- if Shift+1 hit channel 0 again it would.
    // So two distinct channels end up muted.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.key_press(Key::Num1);
    harness.run();
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Num1);
    harness.run();
    assert_ne!(
        *handles.audio.borrow().mutings.last().unwrap(),
        vgms_synth::Muting::all(),
        "Shift+1 targets channel 9, so channels 0 and 9 are both muted"
    );
}

#[test]
fn enter_dismisses_an_info_alert() {
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
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness
        .state_mut()
        .alerts
        .push_back(crate::alert::Alert::confirm(
            "Discard unsaved changes?",
            "Quit anyway?",
            Action::File(FileAction::ConfirmExit),
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
