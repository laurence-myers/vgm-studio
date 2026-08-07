//! split_channels tests (split out of app_gui_tests.rs, st-6).

use super::*;

/// The names a WAV split of `dual_tone_song` writes, in order -- computed the way
/// the app does now (ou-4): the OPL document projects to a VGM and splits through
/// the generic splitter, so the stems carry the chip's roster names.
fn split_names(song: &DroSong) -> Vec<String> {
    let file = std::sync::Arc::new(
        vgms_core::convert::opl_song_to_vgm_file(song).expect("the OPL song projects"),
    );
    vgms_synth::split_vgm_cancellable(
        &file,
        &vgms_synth::VgmSplitOptions {
            format: vgms_synth::SplitFormat::Wav,
            audio: vgms_core::config::AudioConfig::default(),
            resampling: vgms_synth::resample::ResampleMode::default(),
            panning: vgms_synth::ChipPanning::new(),
            boost: 1.0,
            skip_muted: None,
            core_choices: Default::default(),
        },
        &mut |_| {},
        &mut |_, _| {},
        &mut || true,
    )
    .unwrap()
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

/// A playable non-OPL VGM splits per chip channel: only the channels that
/// sound are written, named for the chip and channel.
#[test]
fn splitting_a_non_opl_vgm_writes_a_wav_per_sounding_channel() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);
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
    let files = handles.files.borrow();
    let written: Vec<String> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => {
                Some(path.file_name()?.to_string_lossy().into_owned())
            }
            SaveRequest::Dialog { .. } => None,
        })
        .collect();
    // Only Tone 1 sounds in the fixture, so exactly one file, named for the
    // SN76489 and that channel.
    assert_eq!(written.len(), 1, "one sounding channel: {written:?}");
    assert!(
        written[0].contains("sn76489") && written[0].contains("T1"),
        "named for the chip and channel: {}",
        written[0]
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
    harness.get_by_label_contains("Open Song").click();
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
