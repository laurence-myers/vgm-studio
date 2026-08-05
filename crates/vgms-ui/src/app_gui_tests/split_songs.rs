//! split_songs tests (split out of app_gui_tests.rs, st-6).

use super::*;

#[test]
fn split_songs_is_offered_for_vgm_and_dro() {
    // A VGM capture opens the dialog with its detected songs.
    let (mut harness, _handles) = harness_with_vgm(&multi_song_capture());
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
    let (mut harness, handles) = build(Some(picked_vgm(&multi_song_capture())), true, false);
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
fn the_song_split_offers_to_open_the_folder_as_a_pack_project() {
    let (mut harness, handles) = build(Some(picked_vgm(&multi_song_capture())), true, false);
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

    // The completion alert offers the pack handoff; accepting opens the folder.
    assert!(
        harness
            .query_by_label_contains("Open the folder as a pack project")
            .is_some()
    );
    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        handles.files.borrow().opened_folder_paths.contains(&dir),
        "accepting the offer opens the folder as a pack project"
    );
}

#[test]
fn dismissing_the_folder_picker_cancels_the_song_split() {
    let (mut harness, handles) = build(Some(picked_vgm(&multi_song_capture())), true, false);
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
    let (mut harness, handles) = build(Some(picked_vgm(&redundant_vgm_file())), false, false);
    open_split_songs_dialog(&mut harness);

    harness.get_by_label_contains("Preview").click();
    harness.run_steps(3); // playback requests repaints; `run` would spin.

    let audio = handles.audio.borrow();
    assert!(audio.play_calls >= 1, "preview should start playback");
    assert!(
        audio.seeks_ms.contains(&0),
        "preview should seek to the segment's start time (ou-2: playback seeks by \
         time, not row index)"
    );
}
