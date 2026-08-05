//! find_loop tests (split out of app_gui_tests.rs, st-6).

use super::*;

#[test]
fn find_loop_is_offered_for_both_dro_and_vgm() {
    // A DRO has nowhere to store a loop, but marking and auditioning one still
    // work, so the search is offered -- only the dialog's Apply is VGM-gated.
    let dro = harness_with_song(&tone_song());
    let vgm = harness_with_vgm(&looping_vgm());
    for (mut harness, _handles, what) in [(dro.0, dro.1, "DRO"), (vgm.0, vgm.1, "VGM")] {
        harness.get_by_label("Edit").click();
        harness.run();
        harness.get_by_label_contains("Find").click(); // the Find submenu
        harness.run();
        assert!(
            harness.query_by_label_contains("Find Loop").is_some(),
            "Find Loop should be under Edit > Find for a {what}"
        );
    }
}

#[test]
fn searching_finds_a_loop_and_applying_writes_it() {
    // Inline tasks so the background search runs synchronously on submit.
    let (mut harness, _handles) = build(Some(picked_vgm(&looping_vgm())), true, false);
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
    let file = harness.state().editor.vgm().unwrap();
    assert_eq!(
        file.loop_index(),
        Some(3),
        "loop point at the body's first write"
    );
    // Not row 9, where the repeat begins: a header stores the loop's length in
    // samples, and rows 8 and 9 fall at the same instant, so what comes back is
    // the first row at that moment. The file cannot express the difference, and
    // the markers snap to what it can hold.
    assert_eq!(
        file.loop_end_index(),
        Some(8),
        "as far as the header can say"
    );
    assert_eq!(
        harness.state().editor.markers.end(),
        8,
        "and the markers agree"
    );
    assert!(
        !harness.state().editor.loop_markers_are_unapplied(),
        "so the loop reads as applied, not as pending"
    );
}

#[test]
fn cancelling_a_search_stops_it() {
    let (mut harness, handles) = build(Some(picked_vgm(&looping_vgm())), false, false);
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
    let (mut harness, _handles) = build(Some(picked_vgm(&looping_vgm())), true, true);
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
