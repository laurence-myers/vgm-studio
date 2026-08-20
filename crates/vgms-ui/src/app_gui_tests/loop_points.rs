//! loop_points tests (split out of app_gui_tests.rs, st-6).

use super::*;

#[test]
fn marking_a_loop_pushes_the_region_only_while_looping_is_on() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();

    // Markers move, but with looping off nothing but `None` reaches the engine:
    // Play still means "play the song".
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(3)));
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

    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    let armed = handles.audio.borrow().loops.last().copied().flatten();
    let armed = armed.expect("toggling looping on arms the marked region");
    assert_eq!((armed.start, armed.end), (1, 3));

    // Turning it back off disarms rather than leaving a stale region behind.
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    assert!(handles.audio.borrow().loops.last().unwrap().is_none());

    // And a reset marks the whole song again.
    act(&mut harness, Action::Loop(LoopAction::ClearMarkers));
    assert!(harness.state().editor.markers.is_full(len));
}

#[test]
fn changing_the_repeat_count_re_arms_the_region() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    act(
        &mut harness,
        Action::Loop(LoopAction::SetCount(LoopCount::Times(3))),
    );

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

/// When a file's own loop base/modifier rescale the user's chosen count, the
/// progress readout must show what actually plays, not the user's pick. Here a
/// loop base of 1 halves a Times(2) request to a single play; the stepper keeps
/// the 2, the readout total (and the armed engine config) are 1.
#[test]
fn the_loop_readout_total_reflects_the_scaled_count_not_the_users_pick() {
    // other_chip_vgm_bytes declares its own loop at command 1; set loop_base 1.
    let mut bytes = other_chip_vgm_bytes(
        &[
            0x58, 0x28, 0xF0, // YM2610 port 0
            0x61, 0x10, 0x27, // wait 10000
            0xA0, 0x07, 0x38, // AY8910
            0x62, // wait 735
            0x66, // end
        ],
        10_735,
        10_735,
    );
    bytes[0x7E] = 1; // loop_base
    let file = vgms_core::vgm::file::read("loop.vgm", &bytes).unwrap();
    let (mut harness, handles) = harness_with_vgm(&file);

    // Mark the file's own loop (command 1 to the end) and ask for two plays.
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    act(
        &mut harness,
        Action::Loop(LoopAction::SetCount(LoopCount::Times(2))),
    );

    // (2 * 0x10 + 8) / 0x10 - 1 = 1: the engine plays the loop once.
    let armed = handles
        .audio
        .borrow()
        .loops
        .last()
        .copied()
        .flatten()
        .expect("a region is armed");
    assert_eq!(armed.count, LoopCount::Times(1), "the engine plays it once");
    // The stepper keeps the user's chosen target...
    assert_eq!(harness.state().loop_count, LoopCount::Times(2));
    // ...while the readout total agrees with what actually plays.
    assert_eq!(harness.state().loop_total, LoopCount::Times(1));
}

#[test]
fn a_waveform_click_scrolls_the_table_to_the_row_it_will_play_from() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    act(
        &mut harness,
        Action::Playback(PlaybackAction::WaveformClicked { index: 5, ms: 120 }),
    );

    let state = harness.state();
    assert_eq!(state.editor.selection.first(), Some(5));
    // Top-aligned, not centred: the click says "play from here", so the rows
    // after it are what the view should be spending itself on.
    assert_eq!(
        state.scroll_to,
        Some(crate::widgets::table::ScrollTo::to_top(5))
    );
}

#[test]
fn an_edit_that_outruns_the_playback_start_snaps_it_back_to_the_top() {
    // Click late in the song to set the playback start, then crop to a region
    // that ends well before it: playing from there would seek past the end.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();
    let length_ms = harness.state().editor.dro_song().unwrap().total_delay_ms();
    act(
        &mut harness,
        Action::Playback(PlaybackAction::WaveformClicked {
            index: len - 1,
            ms: length_ms,
        }),
    );
    assert!(harness.state().position.position_ms() > 0, "start is set");

    act(&mut harness, Action::Loop(LoopAction::SetStart(0)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(3)));
    act(&mut harness, Action::Loop(LoopAction::CropToMarkers));

    let state = harness.state();
    assert!(state.editor.len() < len, "the song was cropped");
    assert_eq!(
        state.position.position_ms(),
        0,
        "a start outside the song comes back to the top"
    );
    assert_eq!(state.waveform.cursor_ms, 0, "and the cursor went with it");
    assert!(
        state
            .editor
            .selection
            .first()
            .is_none_or(|row| row < state.editor.len()),
        "and no row outside the song is left selected"
    );
}

#[test]
fn a_crop_puts_the_playback_start_back_at_the_beginning() {
    // Even when the row it pointed at survives: the stream was rebuilt, so row
    // 400 of the old numbering is a different instruction now, and starting
    // there would be starting somewhere the user never chose.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();
    act(
        &mut harness,
        Action::Playback(PlaybackAction::WaveformClicked { index: 1, ms: 5 }),
    );
    assert_eq!(harness.state().position.position_ms(), 5);

    act(&mut harness, Action::Loop(LoopAction::SetStart(0)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(len - 1)));
    act(&mut harness, Action::Loop(LoopAction::CropToMarkers));

    let state = harness.state();
    assert_eq!(state.position.position_ms(), 0);
    assert_eq!(state.waveform.start_ms, 0, "the marker went with it");
    assert_eq!(state.waveform.cursor_ms, 0);
}

#[test]
fn cropping_to_the_markers_keeps_only_the_region() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();
    act(&mut harness, Action::Loop(LoopAction::SetStart(2)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(len - 1)));

    act(&mut harness, Action::Loop(LoopAction::CropToMarkers));
    let state = harness.state();
    assert!(state.editor.len() < len, "the song was cropped");
    assert!(state.status.starts_with("Cropped to "), "{}", state.status);
    // The stream was rebuilt, so the markers reset and the view goes to the top.
    assert!(state.editor.markers.is_full(state.editor.len()));
    assert_eq!(
        state.editor.undo_description(),
        Some("Crop to Marked Region")
    );

    // And it undoes back to the whole song.
    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(harness.state().editor.len(), len);
}

#[test]
fn deleting_the_marked_region_keeps_everything_else() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(3)));

    act(&mut harness, Action::Loop(LoopAction::DeleteMarkedRegion));
    let state = harness.state();
    assert!(
        state.status.starts_with("Deleted 2 instruction(s)"),
        "{}",
        state.status
    );
    assert_eq!(
        state.editor.undo_description(),
        Some("Delete Marked Region")
    );

    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(harness.state().editor.len(), len);
}

#[test]
fn the_region_edits_need_a_region_to_act_on() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    // Fresh markers cover the whole song, so the menu items are disabled...
    assert!(!harness.state().menu_state().has_marked_region);

    // ...and the actions decline rather than edit anything if they fire anyway.
    let len = harness.state().editor.len();
    for action in [
        Action::Loop(LoopAction::CropToMarkers),
        Action::Loop(LoopAction::DeleteMarkedRegion),
    ] {
        act(&mut harness, action);
        let state = harness.state();
        assert_eq!(state.editor.len(), len);
        assert!(!state.editor.can_undo());
        assert!(
            state.status.starts_with("Mark a loop region first"),
            "{}",
            state.status
        );
    }

    // Marking one enables them.
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    assert!(harness.state().menu_state().has_marked_region);
}

#[test]
fn a_cropped_region_re_arms_looping_over_the_new_stream() {
    // Both edits reset the markers and rebuild the stream, so a live loop must
    // be re-armed over what is actually there now.
    let (mut harness, handles) = harness_with_song(&tone_song());
    let len = harness.state().editor.len();
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    act(&mut harness, Action::Loop(LoopAction::SetStart(2)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(len - 1)));

    act(&mut harness, Action::Loop(LoopAction::CropToMarkers));
    let cropped_len = harness.state().editor.len();
    let armed = handles
        .audio
        .borrow()
        .loops
        .last()
        .copied()
        .flatten()
        .expect("looping is still on, so a region is armed");
    assert_eq!(
        (armed.start, armed.end),
        (0, cropped_len),
        "the whole cropped song, not the pre-crop region"
    );
}

#[test]
fn deleting_instructions_slides_the_loop_markers() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::Loop(LoopAction::SetStart(2)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(4)));

    harness.state_mut().editor.selection.select_only(0);
    act(&mut harness, Action::Edit(EditAction::DeleteSelection));

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
    act(&mut harness, Action::Loop(LoopAction::ApplyToMetadata));
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

    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(len - 1)));
    assert!(
        harness.state().editor.loop_markers_are_unapplied(),
        "the markers differ from the stored loop until applied"
    );

    act(&mut harness, Action::Loop(LoopAction::ApplyToMetadata));
    let file = harness.state().editor.vgm().unwrap().clone();
    assert_eq!(file.loop_index(), Some(1));
    // The stored end is what the header can express: it holds the loop's length
    // in samples, so an end sharing its instant with the rows before it comes
    // back as the first of them. The markers snap to it, which is what keeps
    // the "unapplied" cue from staying lit on a loop that was just applied.
    let stored = file
        .loop_end_index()
        .expect("an end short of the tail is stored");
    assert!((2..len).contains(&stored), "a real region, got {stored}");
    assert_eq!(harness.state().editor.markers.end(), stored);
    assert!(!harness.state().editor.loop_markers_are_unapplied());
    assert!(
        harness.state().status.starts_with("Loop saved:"),
        "status was {:?}",
        harness.state().status
    );

    // An end at the song's end is stored as "to the end", not a fixed index --
    // so a later trim widens the loop with the song instead of stranding it.
    act(&mut harness, Action::Loop(LoopAction::SetEnd(len)));
    act(&mut harness, Action::Loop(LoopAction::ApplyToMetadata));
    let meta = harness.state().editor.vgm().unwrap().vgm_meta();
    assert_eq!(meta.loop_end, None);
}

/// Play Tail auditions the ending of a non-OPL VGM. It used to read the length
/// off the OPL song, which a VGM whose chips are not OPL does not have -- so it
/// panicked. It reads the timeline now, which serves either representation.
#[test]
fn play_tail_seeks_near_the_end_of_a_non_opl_vgm() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), false, false);
    assert!(
        harness.state().editor.dro_song().is_none(),
        "held as a VGM, with no OPL projection"
    );
    // A tail shorter than the song, so the seek lands inside it rather than at 0.
    harness.state_mut().config.ui.tail_length = 200;
    let total = harness
        .state()
        .editor
        .timeline()
        .expect("a length")
        .total_ms();

    act(&mut harness, Action::Playback(PlaybackAction::PlayTail));

    let log = handles.audio.borrow();
    assert_eq!(
        log.seeks_ms.last().copied(),
        Some(total.saturating_sub(200))
    );
    assert!(log.play_calls >= 1, "the tail plays");
}

#[test]
fn play_seam_forces_looping_on_and_seeks_before_the_loop_end() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    act(&mut harness, Action::Loop(LoopAction::SetEnd(song.len())));
    act(&mut harness, Action::Playback(PlaybackAction::PlaySeam));

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
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    harness.run_steps(2);
    let overlay = harness
        .state()
        .waveform
        .loop_overlay
        .expect("brackets show");
    assert!(overlay.active);

    // Marking a region shows them with looping off too.
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
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
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
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

    act(&mut harness, Action::Loop(LoopAction::ApplyToMetadata));
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
    // The v1 waveform-select prime is command 0 of the converted VGM, so the
    // song's own writes shift up one: instruction 10 opens the first burst and
    // every fourth one after it starts the next 100 ms, so 14..26 is the region
    // from 100 ms to 400 ms of 600.
    act(&mut harness, Action::Loop(LoopAction::SetStart(14)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(26)));
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));
    act(
        &mut harness,
        Action::Loop(LoopAction::SetCount(LoopCount::Times(4))),
    );
    harness.run();
    settled_snapshot(&mut harness, "loop_overlay");
}

#[test]
fn an_applied_loop_is_guarded_by_the_discard_prompt() {
    // The metadata half of the discard guard: a loop region is deliberate work,
    // so an Open over it must prompt rather than throw it away.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::ApplyToMetadata));
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
    use super::super::waveform_action;

    // Shift+left marks the start, Shift+right the end -- one gesture apart.
    assert_eq!(
        waveform_action(7, 500, false, true),
        Some(Action::Loop(LoopAction::SetStart(7)))
    );
    assert_eq!(
        waveform_action(7, 500, true, true),
        Some(Action::Loop(LoopAction::SetEnd(7)))
    );
    // Unmodified, the left button still seeks...
    assert_eq!(
        waveform_action(7, 500, false, false),
        Some(Action::Playback(PlaybackAction::WaveformClicked {
            index: 7,
            ms: 500
        }))
    );
    // ...and the right button does nothing at all.
    assert_eq!(waveform_action(7, 500, true, false), None);
}
