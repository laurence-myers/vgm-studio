//! render_wav tests (split out of app_gui_tests.rs, st-6).

use super::*;

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
    harness.state_mut().channels.opl().toggle_channel(1);
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
