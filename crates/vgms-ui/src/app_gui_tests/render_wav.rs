//! render_wav tests (split out of app_gui_tests.rs, st-6).

use super::*;

#[test]
fn the_file_menu_opens_the_render_dialog() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    open_render_wav_dialog(&mut harness);
    assert!(harness.state().dialogs.render_wav.is_some());
}

#[test]
fn rendering_with_no_options_writes_to_the_chosen_path() {
    // Inline tasks so the render completes within the same run.
    let (mut harness, handles) = build(Some(picked(&tone_song())), true, false);
    let dest = PathBuf::from("C:/out/tone.dro.wav");
    handles
        .files
        .borrow_mut()
        .save_paths
        .push_back(Some(dest.clone()));

    open_render_wav_dialog(&mut harness);
    harness.get_by_label("Render").click();
    harness.run();

    // The dialog closed; the save dialog was asked *before* the render under the
    // CLI's own name (song.dro -> song.dro.wav), and the bytes were written
    // straight to the chosen path -- no post-render dialog.
    assert!(harness.state().dialogs.render_wav.is_none());
    let files = handles.files.borrow();
    assert_eq!(files.pick_save_path_calls, 1);
    assert_eq!(
        files.save_path_suggestions.last().map(String::as_str),
        Some("tone.dro.wav")
    );
    let Some(SaveRequest::InPlace { path, bytes }) = files.save_requests.last() else {
        panic!("expected an in-place save, got {:?}", files.save_requests)
    };
    assert_eq!(path, &dest);
    assert!(bytes.starts_with(b"RIFF"), "not a WAV");
}

#[test]
fn a_dismissed_save_dialog_renders_nothing() {
    let (mut harness, handles) = build(Some(picked(&tone_song())), true, false);
    // A dismissed picker (the default when no path is queued).
    handles.files.borrow_mut().save_paths.push_back(None);

    open_render_wav_dialog(&mut harness);
    harness.get_by_label("Render").click();
    harness.run();

    assert_eq!(harness.state().status, "The save was cancelled.");
    assert!(
        !handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::RenderWav),
        "a cancelled dialog wastes no render"
    );
    assert!(handles.files.borrow().save_requests.is_empty());
}

#[test]
fn a_saved_render_is_reported_in_the_status_bar() {
    let (mut harness, handles) = build(Some(picked(&tone_song())), true, false);
    let dest = PathBuf::from("C:/songs/tone.dro.wav");
    handles
        .files
        .borrow_mut()
        .save_paths
        .push_back(Some(dest.clone()));

    open_render_wav_dialog(&mut harness);
    harness.get_by_label("Render").click();
    harness.run();
    assert_eq!(
        handles.files.borrow().save_requests.len(),
        1,
        "the render should have been written in place"
    );

    // The in-place save's outcome is delivered on the next poll.
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "tone.dro.wav".to_owned(),
            path: Some(dest.clone()),
        });
    harness.run();

    assert_eq!(
        harness.state().status,
        format!("Rendered {}.", dest.display())
    );
}

#[test]
fn the_render_options_reach_the_task() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    handles
        .files
        .borrow_mut()
        .save_paths
        .push_back(Some(PathBuf::from("C:/out/tone.dro.wav")));
    // Mute a channel and pan another, so "apply" has something to apply.
    harness.state_mut().channels.toggle_selected_channel(1);
    open_render_wav_dialog(&mut harness);
    harness.get_by_label("All of the above").click();
    harness.run();
    harness.get_by_label("Render").click();
    harness.run();

    // Noop tasks record the submission without running it; the render is only
    // submitted once the destination has been chosen.
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
    harness.get_by_label_contains("Open Song").click();
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
    // A failure only ever arrives while a render is in flight, so set that state
    // up (the destination was chosen before the render ran).
    harness.state_mut().render_flow = Some(super::super::RenderWavFlow::Rendering {
        path: PathBuf::from("C:/out/tone.dro.wav"),
    });
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
