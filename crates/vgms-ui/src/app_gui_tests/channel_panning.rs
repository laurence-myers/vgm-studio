//! channel_panning tests (split out of app_gui_tests.rs, st-6).

use super::*;

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
        Some(&vgms_synth::Panning::Custom([0x80; 18])),
        "engaging Custom pushes the centred pans"
    );

    harness.get_by_label("Custom").click();
    harness.run();
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&vgms_synth::Panning::Original),
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
    let center = harness.get_by_label("FM 1").rect().center();
    harness.drag_at(center);
    harness.run();
    harness.hover_at(center - egui::vec2(200.0, 0.0));
    harness.run();
    harness.drop_at(center - egui::vec2(200.0, 0.0));
    harness.run();

    let audio = handles.audio.borrow();
    match audio.pannings.last().expect("a panning was pushed") {
        vgms_synth::Panning::Custom(pans) => {
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
    // does: the vertical axis feeds the same relative mapping.
    let center = harness.get_by_label("FM 1").rect().center();
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
        vgms_synth::Panning::Custom(pans) => {
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

    harness.get_by_label("FM 1").click_secondary();
    harness.run();

    assert_eq!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Custom([0x80; 18]),
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
        Some(&vgms_synth::Panning::Custom(image)),
        "dual-OPL2 Original plays chip 1 left, chip 2 right"
    );
    assert_eq!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Custom(image),
        "the panel reports the fixed image while still in Original mode"
    );
}

#[test]
fn spread_knob_spreads_the_pans_and_engages_custom() {
    let (mut harness, handles) = harness_with_song(&tone_song());

    // Drag the Spread knob to the right: a positive spread leans even channels
    // left, odd channels right, and engages Custom so the knobs go live.
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
        vgms_synth::Panning::Custom(pans) => {
            assert!(pans[0] < 0x80, "channel 1 leans left");
            assert!(pans[1] > 0x80, "channel 2 leans right");
            assert_ne!(pans[0], pans[2], "channels get slightly different values");
        }
        other => panic!("expected Custom panning, got {other:?}"),
    }
    // The spread engaged Custom, so the knobs are now live.
    assert!(matches!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Custom(_)
    ));
}

#[test]
fn all_button_unmutes_but_leaves_panning() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Custom mode with off-centre pans, plus a muted channel.
    harness.state_mut().channels.set_showcase_pans([0x10; 18]);
    harness.key_press(Key::Num3);
    harness.run();
    let panning_before = harness.state().channels.panning();
    assert!(
        matches!(panning_before, vgms_synth::Panning::Custom(_)),
        "the showcase pans engaged Custom"
    );

    harness.get_by_label("All").click();
    harness.run();

    assert_eq!(
        handles.audio.borrow().mutings.last(),
        Some(&vgms_synth::Muting::all()),
        "All unmutes everything"
    );
    // The custom pan image is left untouched -- All is a muting control.
    assert_eq!(
        harness.state().channels.panning(),
        panning_before,
        "All leaves the pans alone"
    );
}

#[test]
fn reset_button_restores_original_panning() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().channels.set_showcase_pans([0x10; 18]);
    assert!(matches!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Custom(_)
    ));

    harness.get_by_label("Reset").click();
    harness.run();

    // A plain OPL2 song's default is Original (mono).
    assert_eq!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Original,
        "Reset returns panning to the song type's default"
    );
}

#[test]
fn loading_a_song_resets_pan_mode_to_original() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().channels.set_showcase_pans([0x00; 18]);
    assert!(matches!(
        harness.state().channels.panning(),
        vgms_synth::Panning::Custom(_)
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
        vgms_synth::Panning::Original,
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

    harness.get_by_label("\u{25B2}").click(); // â–² louder
    harness.run();

    // From unity the up arrow makes a coarse ~1.0 step to about 2x (snapped to the
    // ladder), not a fine nudge.
    let expected = vgms_core::volume_step_up(1.0);
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
    // vgmstudio.ini -- so opening another song can start it from its own modifier.
    assert!(
        handles.saved_configs.borrow().is_empty(),
        "an unlocked volume change does not persist"
    );
}

#[test]
fn a_locked_volume_change_is_persisted() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    // Locking the volume makes changes persist and carry across songs.
    act(&mut harness, Action::Mixer(MixerAction::SetLockBoost(true)));
    let before = handles.saved_configs.borrow().len();

    harness.get_by_label("\u{25B2}").click(); // â–² louder
    harness.run();

    let saved = handles.saved_configs.borrow();
    assert!(
        saved.len() > before,
        "a locked volume change is written to vgmstudio.ini"
    );
    let last = saved.last().expect("a save");
    assert_eq!(last.audio.boost, vgms_core::volume_step_up(1.0));
    assert!(last.audio.lock_boost, "and the lock state is saved with it");
}

#[test]
fn opening_a_song_sets_the_volume_from_its_header_modifier_when_unlocked() {
    // The header asks for a 2x volume (modifier 0x20); unlocked, opening it sets
    // the playback volume to match, so the boost never carries over stale.
    let file = vgm_with_modifier(0x20);
    let (harness, _handles) = build(Some(picked_vgm(&file)), false, false);
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
    act(&mut harness, Action::Mixer(MixerAction::SetLockBoost(true)));
    act(
        &mut harness,
        Action::Mixer(MixerAction::SetBoost {
            value: 4.0,
            persist: true,
        }),
    );
    // Opening a song whose modifier asks for 2x must not disturb the locked 4x.
    harness
        .state_mut()
        .load_file(picked_vgm(&vgm_with_modifier(0x20)));
    harness.run();
    assert_eq!(
        harness.state().config.audio.boost,
        4.0,
        "locked: the volume is kept, not reset to the song's 2x modifier"
    );
}

/// A VGM whose chips are not OPL opens at the volume its header modifier asks
/// for, just like an OPL one. The load boost used to read the OPL projection --
/// which a non-OPL VGM does not have -- so it always opened such a file at unity.
#[test]
fn a_non_opl_vgms_header_modifier_sets_the_load_volume() {
    let mut picked = sms_vgm_file();
    picked.bytes[0x7C] = 0x20; // header modifier: 2x
    let (mut harness, _handles) = build(Some(picked), false, false);
    assert!(harness.state().editor.song().is_none(), "held as a VGM");

    let expected = vgms_core::volume_modifier_factor(0x20);
    assert!((expected - 2.0).abs() < 1e-4, "sanity: 0x20 is 2x");
    assert!(
        (harness.state().config.audio.boost - expected).abs() < 1e-4,
        "the volume follows the non-OPL VGM's modifier: {}",
        harness.state().config.audio.boost
    );

    // Locked, opening a second non-OPL file keeps the volume, ignoring its 4x.
    act(&mut harness, Action::Mixer(MixerAction::SetLockBoost(true)));
    let mut other = sms_vgm_file();
    other.bytes[0x7C] = 0x40; // would ask for 4x
    harness.state_mut().load_file(other);
    harness.run();
    assert!(
        (harness.state().config.audio.boost - expected).abs() < 1e-4,
        "locked: kept at 2x, not reset to the second file's 4x: {}",
        harness.state().config.audio.boost
    );
}

#[test]
fn unlocking_snaps_the_volume_to_the_current_songs_modifier() {
    // Locked at 4x over a song whose modifier asks for 2x.
    let (mut harness, _handles) = build(Some(picked_vgm(&vgm_with_modifier(0x20))), false, false);
    act(&mut harness, Action::Mixer(MixerAction::SetLockBoost(true)));
    act(
        &mut harness,
        Action::Mixer(MixerAction::SetBoost {
            value: 4.0,
            persist: true,
        }),
    );
    assert_eq!(harness.state().config.audio.boost, 4.0);
    // Unlocking hands control back to the song: the volume snaps to its 2x now.
    act(
        &mut harness,
        Action::Mixer(MixerAction::SetLockBoost(false)),
    );
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
    harness.get_by_label("\u{25B2}").click(); // â–² louder -- but capped
    harness.run();

    assert_eq!(
        harness.state().config.audio.boost,
        before,
        "the up arrow is blocked once the limiter has engaged"
    );

    // Lowering is still allowed, and drops off the ceiling.
    harness.get_by_label("\u{25BC}").click(); // â–¼ quieter
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
    // cap follows it down to the lowest level that clips.
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
    let peak = vgms_synth::measure_peak(&song, rate);
    let expected = vgms_core::volume_modifier_factor(vgms_core::nearest_volume_modifier(
        vgms_core::boost_for_peak(peak.max_level),
    ));
    assert_eq!(
        harness.state().config.audio.boost,
        expected,
        "the volume is matched to the measured peak, on the ladder; status={:?}",
        harness.state().status
    );
    // Match Volume is a per-song action: unlocked, it sets the live volume but
    // does not write to vgmstudio.ini.
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
    act(&mut harness, Action::Mixer(MixerAction::MatchVolume));
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

/// Match measures a VGM whose chips are not OPL and sets the volume from its
/// peak, just as it does for an OPL song. It used to refuse with "This needs an
/// OPL song" because the scan only measured the OPL projection.
#[test]
fn match_volume_measures_a_non_opl_vgm() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);
    assert!(harness.state().editor.song().is_none(), "held as a VGM");

    act(&mut harness, Action::Mixer(MixerAction::MatchVolume));
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
        "a scan is submitted for the non-OPL VGM"
    );

    // The exact ladder volume the app must have chosen, recomputed through the
    // same generic engine at the same rate and resample mode.
    let rate = harness.state().config.audio.frequency;
    let resample =
        vgms_synth::resample::ResampleMode::from_slug(&harness.state().config.audio.resampling)
            .unwrap_or_default();
    let file = std::sync::Arc::new(harness.state().editor.vgm().unwrap().clone());
    let peak = vgms_synth::measure_vgm_peak(file, rate, resample);
    assert!(peak.max_level > 0, "the SN76489 tone is audible");
    assert_eq!(
        harness.state().config.audio.boost,
        vgms_core::matched_volume(peak.max_level),
        "the volume is matched to the measured peak; status={:?}",
        harness.state().status
    );
}

/// Measure fills the VGM metadata dialog's volume-modifier suggestion for a
/// non-OPL VGM. The dialog opens for any VGM, so its Measure button was dead on
/// a non-OPL file until the scan learned to measure one.
#[test]
fn measuring_the_modifier_fills_for_a_non_opl_vgm() {
    let (mut harness, _handles) = build(Some(sms_vgm_file()), true, false);
    assert!(harness.state().editor.song().is_none(), "held as a VGM");

    act(&mut harness, Action::Edit(EditAction::OpenVgmMetadata));
    assert!(
        harness.state().dialogs.vgm_metadata.is_some(),
        "dialog opens"
    );
    act(
        &mut harness,
        Action::Mixer(MixerAction::MeasureVolumeModifier),
    );
    for _ in 0..4 {
        harness.step();
    }

    assert!(
        harness.state().status.contains("volume modifier"),
        "the measure reached the dialog: {:?}",
        harness.state().status
    );
}

/// A VGM for chips there is no core for is refused rather than measured: it
/// would render silence, and a silent measurement suggests a nonsense +36 dB.
#[test]
fn match_volume_on_a_coreless_vgm_reports_nothing_to_play() {
    let (mut harness, handles) = build(Some(other_chip_vgm_file()), true, false);
    assert!(
        !harness.state().editor.capabilities().renderable,
        "no core for its chips"
    );

    act(&mut harness, Action::Mixer(MixerAction::MatchVolume));
    harness.run();

    assert!(
        !handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::VolumeScan),
        "no scan is submitted for a coreless document"
    );
    assert_eq!(
        harness.state().status,
        crate::strings::APP_STATUS_NOTHING_TO_PLAY
    );
}

#[test]
fn measuring_the_modifier_routes_the_peak_to_the_open_dialog() {
    let song = tone_song();
    // Inline tasks run the scan; convert to VGM so there is a modifier to fill.
    let (mut harness, _handles) = build(Some(picked(&song)), true, false);
    harness.state_mut().editor.convert_to_vgm().unwrap();
    let vgm = harness.state().editor.vgm().unwrap().clone();
    harness.state_mut().dialogs.vgm_metadata =
        Some(crate::dialogs::VgmMetadataDialog::for_vgm(&vgm).unwrap());

    // Trigger the Measure scan; the inline scan stores its Peak, then a poll frame
    // routes it to the open dialog (the same delivery shape as Match Volume).
    act(
        &mut harness,
        Action::Mixer(MixerAction::MeasureVolumeModifier),
    );
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
    // The Settings dialog snapshots the config at open and doesn't expose the
    // boost, so a boost changed via the transport meanwhile must not be reverted
    // on Save.
    let (mut harness, handles) = harness_with_song(&tone_song());
    let config = harness.state().config.clone();
    harness.state_mut().dialogs.settings =
        Some(crate::dialogs::SettingsDialog::new(&config, Vec::new()));
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

/// Open Settings and pick Wine from the Theme dropdown, through the real combo
/// -- the pick is the thing that has to fire the preview.
fn previewing_wine() -> (Harness<'static, VgmStudioApp>, Handles) {
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert_eq!(harness.state().config.ui.theme, ThemeChoice::Petrol);
    let config = harness.state().config.clone();
    harness.state_mut().dialogs.settings =
        Some(crate::dialogs::SettingsDialog::new(&config, Vec::new()));
    harness.run();

    // Theme lives on the Interface tab now; open it before reaching the combo.
    harness.get_by_label("Interface").click();
    harness.run();

    // A closed combo reports its selection as its value, not its label.
    harness.get_by_value("Petrol").click();
    harness.run();
    // The popup's own items don't take a synthetic pointer click here (the
    // press lands on the panel under the popup's layer), so pick through
    // accesskit, which kittest offers for exactly this.
    harness.get_by_label("Wine").click_accesskit();
    harness.run();
    // An accesskit pick doesn't dismiss the popup the way a pointer one does,
    // and a popup left open eats the next click. Dismiss it on the inert row
    // caption beside the dropdown.
    harness.get_by_label("Theme").click();
    harness.run();
    assert_eq!(
        harness.query_all_by_label("Navy").count(),
        0,
        "the theme popup is dismissed"
    );
    (harness, handles)
}

#[test]
fn a_picked_theme_previews_on_the_whole_window() {
    // A colour scheme cannot be judged from a dropdown's label, so the pick
    // repaints everything straight away -- without committing anything.
    let (harness, handles) = previewing_wine();

    assert_eq!(harness.state().shown_skin().0, ThemeChoice::Wine);
    assert_eq!(
        harness.state().config.ui.theme,
        ThemeChoice::Petrol,
        "the preview must not reach the config"
    );
    assert!(
        handles.saved_configs.borrow().is_empty(),
        "and nothing may be written to the ini -- the volume lever persists \
         `config` from under us, which would leak an unsaved preview"
    );
}

#[test]
fn closing_settings_puts_the_previewed_theme_back() {
    // Trying themes out and then backing out must leave no trace; otherwise
    // Close silently keeps whichever one was highlighted last.
    let (mut harness, handles) = previewing_wine();

    harness.get_by_label("Close").click();
    harness.run();

    assert!(harness.state().dialogs.settings.is_none(), "dialog closed");
    assert_eq!(
        harness.state().shown_skin().0,
        ThemeChoice::Petrol,
        "the theme the dialog opened with is back"
    );
    assert!(harness.state().skin_preview.is_none(), "no stale preview");
    assert!(handles.saved_configs.borrow().is_empty());
}

#[test]
fn saving_settings_keeps_the_previewed_theme() {
    let (mut harness, handles) = previewing_wine();

    harness.get_by_label("Save").click();
    harness.run();

    assert_eq!(harness.state().config.ui.theme, ThemeChoice::Wine);
    assert_eq!(harness.state().shown_skin().0, ThemeChoice::Wine);
    assert!(
        harness.state().skin_preview.is_none(),
        "the preview became the saved skin"
    );
    assert_eq!(
        handles.saved_configs.borrow().last().map(|c| c.ui.theme),
        Some(ThemeChoice::Wine),
    );
}

#[test]
fn settings_do_not_retune_the_position_panel_while_a_stream_is_live() {
    // A frequency change must not retune the panel while a stream plays at the
    // old rate (the readout would mix a new-rate length with old-rate frames).
    // The panel keeps the live rate until the stream reloads.
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
    let mut config = harness.state().config.clone();
    config.audio.frequency = 44_100;
    harness.state_mut().dialogs.settings =
        Some(crate::dialogs::SettingsDialog::new(&config, Vec::new()));
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
