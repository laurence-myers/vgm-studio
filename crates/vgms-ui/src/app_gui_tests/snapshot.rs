//! snapshot tests (split out of app_gui_tests.rs, st-6).

use super::*;

/// The Settings dialog's output section: one row per chip this app can play,
/// and a count of the ones it cannot.
#[test]
fn snapshot_settings_output_per_chip() {
    // The rows come from the core registry, and vgms-ui alone knows only
    // vgms-synth's built-ins, so install the test cores to guard the hardware
    // picker row.
    crate::widgets::chip_output::install_test_cores();
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    let config = harness.state().config.clone();
    // A DRO is loaded, so the Output tab's "This song" section shows the OPL
    // row hoisted above the rest of the roster.
    harness.state_mut().dialogs.settings = Some(
        crate::dialogs::SettingsDialog::new(&config, Vec::new()).with_song(
            crate::dialogs::SongContext {
                name: "tone.dro".to_owned(),
                chips: vec![vgms_core::vgm::ChipKind::Ymf262],
            },
        ),
    );
    settled_snapshot(&mut harness, "settings_output_per_chip");
}

#[test]
fn snapshot_empty_app() {
    let (mut harness, _handles) = build(None, false, true);
    settled_snapshot(&mut harness, "empty_app");
}

#[test]
fn snapshot_loaded_song() {
    // Inline tasks so the waveform is actually rendered for the snapshot.
    let (mut harness, _handles) = build(Some(picked(&tone_song())), true, true);
    harness.state_mut().editor.selection.select_only(0);
    harness.run();
    settled_snapshot(&mut harness, "loaded_tone_song");
}

#[test]
fn snapshot_dro_info_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();
    settled_snapshot(&mut harness, "dro_info_dialog");
}

#[test]
fn snapshot_gd3_tag_dialog() {
    // The Game Name is longer than the field is wide: it must wrap at the
    // dialog's edge and push the box taller, not scroll out of sight.
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    let mut fields: [String; vgms_core::vgm::data::GD3_FIELD_COUNT] =
        core::array::from_fn(|_| String::new());
    fields[0] = "Boss Battle".to_owned();
    fields[2] = "A Game With A Truly Preposterous Subtitle: The Revenge".to_owned();
    fields[6] = "Composed by somebody with an unusually long credit line".to_owned();
    harness.state_mut().dialogs.gd3_tag = Some(crate::dialogs::Gd3TagDialog::new(Some(
        &vgms_core::Gd3Tag::from_fields(fields),
    )));
    harness.run();
    settled_snapshot(&mut harness, "gd3_tag_dialog");
}

/// A VGM declaring the one chip the GUI-test registry can actually play and
/// mute: the SN76489 (the tone-stub core registers under its id with
/// `channel_mute: true`). What the *clicked* channel toggles are tested on.
fn sn76489_vgm_file() -> PickedFile {
    generic_vgm_file("01 Tone.vgm", &[(vgms_core::ChipKind::Sn76489, 3_579_545)])
}

/// A Mega Drive pair -- the shape the chip Mute/Solo controls exist for.
fn mega_drive_vgm_file() -> PickedFile {
    generic_vgm_file(
        "01 Zone.vgm",
        &[
            (vgms_core::ChipKind::Sn76489, 3_579_545),
            (vgms_core::ChipKind::Ym2612, 7_670_454),
        ],
    )
}

/// A generic (non-OPL) VGM declaring `chips`, with a small walkable body.
fn generic_vgm_file(name: &str, chips: &[(vgms_core::ChipKind, u32)]) -> PickedFile {
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x161);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    for (kind, clock) in chips {
        put_u32(&mut bytes, kind.clock_offset(), *clock);
    }
    put_u32(&mut bytes, 0x18, 10_735);
    bytes.extend_from_slice(&[
        0x50, 0x8E, // SN76489 write
        0x61, 0x10, 0x27, // wait 10000
        0x62, // wait 735
        0x66, // end
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    PickedFile {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/rips/Tone/{name}"))),
        bytes,
    }
}

/// The same file with a stream that will not walk: `0x00` is an opcode the
/// spec gives no length, so there is no way past it and no rows to show.
fn unwalkable_vgm_file() -> PickedFile {
    PickedFile {
        name: "04 Broken.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/Athena/04 Broken.vgm")),
        // Its stream cannot be summed, so the header's own totals are all there
        // is -- which is exactly what the dialog reports.
        bytes: other_chip_vgm_bytes(&[0x00, 0x01, 0x02, 0x66], 44_100 * 95, 44_100 * 60),
    }
}

/// The editor for other chips: rows named by chip, and no transport or waveform above
/// them, because there is no OPL stream to drive either.
#[test]
fn snapshot_other_chip_editor() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, true);
    harness.state_mut().editor.selection.select_only(1);
    harness.run();
    settled_snapshot(&mut harness, "other_chip_editor");
}

/// The dialog is now the answer for one case only: a file whose commands
/// cannot be walked, and so has nothing to put in the table.
#[test]
fn snapshot_unwalkable_vgm_dialog() {
    let (mut harness, _handles) = build(Some(unwalkable_vgm_file()), false, true);
    settled_snapshot(&mut harness, "unwalkable_vgm_dialog");
}

/// Opening a VGM for other chips is not a failure: it opens for trimming, with
/// no error alert and no half-loaded song.
#[test]
fn opening_a_vgm_for_other_chips_opens_it_for_trimming() {
    let (harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    let app = harness.state();

    assert!(app.editor.vgm().is_some(), "it opened");
    assert!(app.editor.dro_song().is_none(), "but not as a song");
    assert!(app.dialogs.unwalkable_vgm.is_none(), "no dialog was needed");
    assert!(app.alerts.is_empty(), "and no error: {:?}", app.alerts);
    assert_eq!(app.editor.len(), 4, "four commands, the end marker aside");
    assert!(
        !app.editor.capabilities().playable,
        "there is no OPL stream to play"
    );
    assert!(
        app.status.contains("playback is not supported"),
        "{}",
        app.status
    );
}

/// *Clicking* a generic chip's channel toggle pushes the mask to the audio
/// output -- the path the pointer takes, as distinct from the number-key
/// action below. This is the click the "muting does nothing" report describes,
/// so it is pinned end to end at the UI layer.
#[test]
fn clicking_a_chip_channel_toggle_pushes_the_mask() {
    let (mut harness, handles) = build(Some(sn76489_vgm_file()), false, false);
    harness.run();

    // The SN76489 panel's first channel, by its toggle label.
    harness.get_by_label("T1").click();
    harness.run();

    let audio = handles.audio.borrow();
    let last = audio.chip_mutings.last().expect("a chip muting was pushed");
    assert_eq!(
        last.mask_for(vgms_core::ChipKind::Sn76489, 0),
        0b1,
        "clicking T1 must mute Tone 1"
    );
}

/// A pan-capable chip shows its knobs whether or not Custom is latched -- a
/// control you can see is how Custom is found -- but they only move the output
/// once it is.
#[test]
fn a_generic_chips_pan_knobs_are_shown_before_custom_and_go_live_with_it() {
    let (mut harness, handles) = build(Some(sn76489_vgm_file()), false, false);
    harness.run();

    // Shown under Original, and inert: a drag pushes no panning at all.
    let knob = harness.get_by_label("Tone 1").rect().center();
    drag_by(&mut harness, knob, egui::vec2(-200.0, 0.0));
    assert!(
        handles.audio.borrow().chip_pannings.is_empty(),
        "an Original-mode knob must not move the output"
    );

    harness.get_by_label("Custom").click();
    harness.run();
    let knob = harness.get_by_label("Tone 1").rect().center();
    drag_by(&mut harness, knob, egui::vec2(-200.0, 0.0));

    let audio = handles.audio.borrow();
    let last = audio
        .chip_pannings
        .last()
        .expect("a chip panning was pushed");
    let pans = last
        .pans_for(vgms_core::ChipKind::Sn76489, 0)
        .expect("the chip's pans");
    assert_eq!(
        pans[0],
        vgms_synth::chip_mix::PAN_LEFT,
        "Tone 1 dragged hard left"
    );
    assert_eq!(
        pans[1],
        vgms_synth::chip_mix::PAN_CENTER,
        "Tone 2 stays centred"
    );
}

/// The Spread knob and Reset button work on a generic chip exactly as they do
/// on OPL: spread engages Custom and leans the voices apart, Reset puts the
/// chip's own image back.
#[test]
fn spread_and_reset_pan_a_generic_chip_like_the_opl_panel() {
    let (mut harness, handles) = build(Some(sn76489_vgm_file()), false, false);
    harness.run();

    let spread = harness.get_by_label("Spread").rect().center();
    drag_by(&mut harness, spread, egui::vec2(200.0, 0.0));
    {
        let audio = handles.audio.borrow();
        let last = audio
            .chip_pannings
            .last()
            .expect("a chip panning was pushed");
        let pans = last
            .pans_for(vgms_core::ChipKind::Sn76489, 0)
            .expect("the spread engaged Custom");
        assert!(
            pans[0] < vgms_synth::chip_mix::PAN_CENTER,
            "Tone 1 leans left"
        );
        assert!(
            pans[1] > vgms_synth::chip_mix::PAN_CENTER,
            "Tone 2 leans right"
        );
    }

    harness.get_by_label("Reset").click();
    harness.run();
    let audio = handles.audio.borrow();
    let last = audio.chip_pannings.last().expect("the reset was pushed");
    assert!(
        last.is_neutral(),
        "Reset returns the chip to its own image, {last:?}"
    );
}

/// A chip's lamp masks that whole chip (left-click) and solos it (right-click,
/// additive -- every un-soloed chip silenced) -- the isolation workflow: solo
/// the SN76489 of a Mega Drive rip and only the PSG is left sounding. Works
/// whatever the cores can do, because a whole-chip mask is honoured by the
/// engine itself. The lamp replaces the old Mute/Solo pads, one per chip.
#[test]
fn chip_mute_and_solo_reach_the_audio_as_whole_chip_masks() {
    use vgms_core::ChipKind;

    let (mut harness, handles) = build(Some(mega_drive_vgm_file()), false, false);
    harness.run();

    // Right-click the SN76489 lamp to solo it.
    harness.get_by_label("SN76489 lamp").click_secondary();
    harness.run();
    {
        let audio = handles.audio.borrow();
        let last = audio.chip_mutings.last().expect("a chip muting was pushed");
        assert_eq!(
            last.mask_for(ChipKind::Sn76489, 0),
            0,
            "the soloed chip plays"
        );
        assert_eq!(
            last.mask_for(ChipKind::Ym2612, 0),
            0x7F,
            "every un-soloed chip is fully masked"
        );
    }

    // Right-click again: the solo lifts and everything comes back.
    harness.get_by_label("SN76489 lamp").click_secondary();
    harness.run();
    {
        let audio = handles.audio.borrow();
        let last = audio.chip_mutings.last().expect("a chip muting was pushed");
        assert_eq!(last.mask_for(ChipKind::Sn76489, 0), 0);
        assert_eq!(last.mask_for(ChipKind::Ym2612, 0), 0);
    }

    // Left-click the lamp to mute that chip alone.
    harness.get_by_label("SN76489 lamp").click();
    harness.run();
    let audio = handles.audio.borrow();
    let last = audio.chip_mutings.last().expect("a chip muting was pushed");
    assert_eq!(
        last.mask_for(ChipKind::Sn76489, 0),
        0xF,
        "the muted chip is fully masked"
    );
    assert_eq!(
        last.mask_for(ChipKind::Ym2612, 0),
        0,
        "the other is untouched"
    );
}

/// Dragging a chip's trim knob down attenuates that chip and pushes the trim to
/// the audio output; the other chip stays at the reference balance.
#[test]
fn dragging_a_chip_trim_knob_pushes_the_attenuation() {
    use vgms_core::ChipKind;

    let (mut harness, handles) = build(Some(mega_drive_vgm_file()), false, false);
    harness.run();

    // Drag the SN76489's level knob down (left lowers, as on the pan knobs).
    let knob = harness.get_by_label("SN76489 level").rect().center();
    drag_by(&mut harness, knob, egui::vec2(-30.0, 0.0));

    let audio = handles.audio.borrow();
    let last = audio.chip_trims.last().expect("a chip trim was pushed");
    assert!(
        last.percent_for(ChipKind::Sn76489, 0) < 100,
        "the drag attenuated the chip: {last:?}"
    );
    assert_eq!(
        last.percent_for(ChipKind::Ym2612, 0),
        100,
        "the other chip stays at the reference balance"
    );
}

/// Right-clicking a chip's trim knob returns it to 100%.
#[test]
fn right_clicking_a_chip_trim_knob_resets_it_to_unity() {
    use vgms_core::ChipKind;

    let (mut harness, handles) = build(Some(sn76489_vgm_file()), false, false);
    harness.run();

    let knob = harness.get_by_label("SN76489 level").rect().center();
    drag_by(&mut harness, knob, egui::vec2(-40.0, 0.0));
    assert!(
        handles
            .audio
            .borrow()
            .chip_trims
            .last()
            .expect("a chip trim was pushed")
            .percent_for(ChipKind::Sn76489, 0)
            < 100,
        "the drag attenuated it first"
    );

    harness.get_by_label("SN76489 level").click_secondary();
    harness.run();
    let audio = handles.audio.borrow();
    let last = audio.chip_trims.last().expect("the reset was pushed");
    assert_eq!(
        last.percent_for(ChipKind::Sn76489, 0),
        100,
        "right-click resets the trim to 100%"
    );
}

/// A six-chip arcade set is too wide for the deck, so the chip strip wraps to a
/// second row rather than scrolling. Pinned by the last chip's cell sitting
/// below the first's.
#[test]
fn the_chip_strip_wraps_to_a_second_row() {
    use vgms_core::ChipKind;

    let file = generic_vgm_file(
        "06 Arcade.vgm",
        &[
            (ChipKind::Sn76489, 3_579_545),
            (ChipKind::Ym2612, 7_670_454),
            (ChipKind::Ym2151, 3_579_545),
            (ChipKind::Ym2203, 3_000_000),
            (ChipKind::Ym2608, 8_000_000),
            (ChipKind::Ay8910, 1_789_772),
        ],
    );
    // Narrow, so six cells cannot fit on one row.
    let (mut harness, _handles) = build_sized(Some(file), false, false, egui::vec2(520.0, 720.0));
    harness.run();

    // The lamp labels are unique per chip (the bare chip name also appears in the
    // editor), and each lamp sits in its chip's cell, so their rows are the cells'.
    let first = harness.get_by_label("SN76489 lamp").rect();
    let last = harness.get_by_label("AY8910 lamp").rect();
    assert!(
        last.top() > first.bottom(),
        "the sixth chip wrapped below the first: first {first:?}, last {last:?}"
    );
}

/// The single OPL/DRO device gets the mixer controls too: its lamp mutes the
/// whole device (folded into the OPL muting, so it works on hardware and the
/// emulator alike).
#[test]
fn the_opl_lamp_mutes_the_whole_device() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.run();
    assert_eq!(
        harness.state().channels.muting(),
        vgms_synth::Muting::all(),
        "playing at first"
    );

    harness.get_by_label("YM3812 lamp").click();
    harness.run();
    // The whole OPL2 device is silenced: every low-bank melodic channel muted
    // and the low bank's drums gated. (The high bank belongs to no chip on an
    // OPL2 device, so its bits are immaterial -- this checks the meaningful
    // low-bank state rather than the exact `silent()` byte representation.)
    let muted = harness.state().channels.muting();
    assert_eq!(
        muted.channels_raw() & 0x1FF,
        0,
        "every low-bank melodic channel muted"
    );
    assert_eq!(
        muted.percussion_raw()[0],
        0xE0,
        "the low-bank drums are silenced"
    );

    harness.get_by_label("YM3812 lamp").click();
    harness.run();
    assert_eq!(
        harness.state().channels.muting(),
        vgms_synth::Muting::all(),
        "clicking again brings it back"
    );
}

/// The OPL device's trim knob attenuates it, keyed to the chip its projection
/// plays through (an OPL2 tone -> the YM3812 voice), and reaches the audio.
#[test]
fn dragging_the_opl_trim_knob_pushes_the_attenuation() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.run();

    let knob = harness.get_by_label("YM3812 level").rect().center();
    drag_by(&mut harness, knob, egui::vec2(-30.0, 0.0));

    let audio = handles.audio.borrow();
    let last = audio.chip_trims.last().expect("a chip trim was pushed");
    assert!(
        last.percent_for(vgms_core::ChipKind::Ym3812, 0) < 100,
        "the OPL trim attenuates its projected chip: {last:?}"
    );
}

/// The number keys mute the *selected chip's* channels, not the OPL panel's:
/// pressing 1 on a Mega Drive-era rip mutes that chip's first channel, and the
/// mask reaches the audio output as a chip muting.
#[test]
fn a_number_key_mutes_the_selected_chip_on_a_non_opl_vgm() {
    let (mut harness, handles) = build(Some(other_chip_vgm_file()), false, false);
    harness.run();

    harness.key_press(Key::Num1);
    harness.run();

    let audio = handles.audio.borrow();
    let last = audio.chip_mutings.last().expect("a chip muting was pushed");
    // The file declares one chip (its SSG is a linked child, not a second
    // declaration), so channel 1 is its FM 1 -- bit 0.
    assert_eq!(
        last.mask_for(vgms_core::ChipKind::Ym2610, 0),
        0b1,
        "the selected chip's first channel is muted"
    );
}

/// Delay navigation and Find Register on a non-OPL VGM: ArrowRight steps through
/// the delays and the dialog finds a write to a named chip register.
#[test]
fn delay_navigation_and_find_register_work_on_a_non_opl_vgm() {
    use crate::action::FindQuery;
    use vgms_core::vgm::VgmFindTarget;

    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    harness.run();
    assert!(harness.state().editor.dro_song().is_none(), "held as a VGM");

    // ArrowRight steps to the first delay (row 1), then the second (row 3).
    act(&mut harness, Action::Playback(PlaybackAction::NextDelay));
    assert_eq!(harness.state().editor.selection.first(), Some(1));
    act(&mut harness, Action::Playback(PlaybackAction::NextDelay));
    assert_eq!(harness.state().editor.selection.first(), Some(3));

    // Find Register opens the chip-picker dialog for a VGM (not "wrong file").
    act(&mut harness, Action::Edit(EditAction::OpenFindRegister));
    assert!(harness.state().dialogs.find_reg.is_some());

    // Searching for the AY8910's mixer write lands on row 2.
    harness.state_mut().editor.selection.select_only(0);
    act(
        &mut harness,
        Action::Edit(EditAction::FindRegister {
            query: FindQuery::Vgm(VgmFindTarget::Write {
                kind: vgms_core::ChipKind::Ay8910,
                instance: Some(0),
                addr: Some(0x07),
            }),
            backwards: false,
        }),
    );
    assert_eq!(harness.state().editor.selection.first(), Some(2));
    assert!(
        harness.state().status.contains("found at position 0002"),
        "{}",
        harness.state().status
    );
}

/// The Find Register dialog for a multichip VGM: a chip picker, its registers,
/// and a free hex box -- not the OPL token list.
#[test]
fn snapshot_find_register_vgm_dialog() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, true);
    act(&mut harness, Action::Edit(EditAction::OpenFindRegister));
    settled_snapshot(&mut harness, "find_register_vgm_dialog");
}

/// The Find Register dialog for a DRO: the same chip/register picker a VGM
/// gets, offering the document's one OPL chip and its registers by name rather
/// than the old bare token dropdown.
#[test]
fn snapshot_find_register_dro_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    act(&mut harness, Action::Edit(EditAction::OpenFindRegister));
    settled_snapshot(&mut harness, "find_register_dro_dialog");
}

/// A VGM for chips this app has no core for can be cropped to a marked region
/// and undone. Driven through the menu actions, so their gates are exercised too.
#[test]
fn a_non_opl_document_can_be_cropped_and_undone() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    assert!(!harness.state().editor.capabilities().playable);
    let before = harness.state().editor.save_bytes().unwrap();
    let rows = harness.state().editor.len();

    // Mark the second half and crop to it.
    act(&mut harness, Action::Loop(LoopAction::SetStart(2)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(rows)));
    act(&mut harness, Action::Loop(LoopAction::CropToMarkers));

    let app = harness.state();
    assert!(
        app.status.contains("that restore the chip state"),
        "the discarded head configured something: {}",
        app.status
    );
    assert!(app.editor.is_dirty());
    assert_eq!(app.editor.undo_description(), Some("Crop to Marked Region"));
    // The YM2610 write from the discarded head is back at the top.
    assert!(
        app.editor
            .row_cells_for_test(0)
            .description
            .contains("YM2610"),
        "the restore leads"
    );

    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(harness.state().editor.len(), rows);
    assert_eq!(
        harness.state().editor.save_bytes().unwrap(),
        before,
        "undo restores the file byte for byte"
    );
}

/// And the region delete, which bridges the seam rather than prefixing it.
#[test]
fn a_non_opl_document_can_have_a_region_deleted() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    let before = harness.state().editor.save_bytes().unwrap();

    act(&mut harness, Action::Loop(LoopAction::SetStart(0)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(2)));
    act(&mut harness, Action::Loop(LoopAction::DeleteMarkedRegion));
    assert!(
        harness.state().status.contains("Deleted 2 instruction(s)"),
        "{}",
        harness.state().status
    );
    assert!(
        harness.state().status.contains("across the seam"),
        "the removed span had configured something: {}",
        harness.state().status
    );

    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(harness.state().editor.save_bytes().unwrap(), before);
}

/// Selecting rows, deleting them and saving the result on a document held as a
/// VGM: none of it is an OPL idea, so it works without a `DroSong`.
#[test]
fn a_non_opl_document_can_be_edited_and_saved() {
    let (mut harness, handles) = build(Some(other_chip_vgm_file()), false, false);
    let rows = harness.state().editor.len();

    harness.state_mut().editor.selection.select_only(1);
    act(&mut harness, Action::Edit(EditAction::DeleteSelection));
    assert_eq!(harness.state().editor.len(), rows - 1, "the row is gone");

    act(&mut harness, Action::File(FileAction::Save));
    let saved = handles.files.borrow().save_requests.len();
    assert_eq!(saved, 1, "the save reached the file service");

    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(harness.state().editor.len(), rows);
}

/// The position readout's length follows an edit on a non-OPL VGM. after_edit
/// used to refresh the length only from the OPL song, so a crop or delete on a
/// VGM whose chips are not OPL left the "N / TOTAL ms" total stale.
#[test]
fn editing_a_non_opl_vgm_keeps_the_position_length_current() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    assert!(harness.state().editor.dro_song().is_none(), "held as a VGM");

    let before = harness
        .state()
        .editor
        .timeline()
        .expect("a length")
        .total_ms();
    assert_eq!(
        harness.state().position.length_ms(),
        before,
        "the readout starts at the full length"
    );

    // Delete the 10000-sample wait (row 1); the rest is a 735-sample wait.
    harness.state_mut().editor.selection.select_only(1);
    act(&mut harness, Action::Edit(EditAction::DeleteSelection));

    let after = harness
        .state()
        .editor
        .timeline()
        .expect("a length")
        .total_ms();
    assert!(after < before, "the delete shortened the stream");
    assert_eq!(
        harness.state().position.length_ms(),
        after,
        "and the readout followed it, not the stale full length"
    );
}

/// Save As on a VGM whose chips are not OPL offers the file's own name. It used
/// to read the OPL projection's name, which a non-OPL VGM does not have -- so
/// this was a panic, not a dialog.
#[test]
fn save_as_offers_the_documents_own_name_for_a_non_opl_vgm() {
    let (mut harness, handles) = build(Some(other_chip_vgm_file()), false, false);

    act(&mut harness, Action::File(FileAction::SaveAs));

    let files = handles.files.borrow();
    let Some(SaveRequest::Dialog { suggested_name, .. }) = files.save_requests.last() else {
        panic!("expected a save dialog, got {:?}", files.save_requests)
    };
    assert_eq!(suggested_name, "03 Psycho Soldier.vgm");
}

/// A plain Save with no path -- every save on the web target, where a picked
/// file has no path -- falls through to the dialog for a non-OPL VGM too, and by
/// the same route this used to panic.
#[test]
fn saving_a_pathless_non_opl_vgm_falls_through_to_the_dialog() {
    let mut picked = other_chip_vgm_file();
    picked.path = None;
    let (mut harness, handles) = build(Some(picked), false, false);

    act(&mut harness, Action::File(FileAction::Save));

    let files = handles.files.borrow();
    let Some(SaveRequest::Dialog { suggested_name, .. }) = files.save_requests.last() else {
        panic!("expected a save dialog, got {:?}", files.save_requests)
    };
    assert_eq!(suggested_name, "03 Psycho Soldier.vgm");
}

/// A Neo Geo capture of three songs, parted by two seconds of silence each.
fn other_chip_capture_file() -> PickedFile {
    // One song: a YM2610 write, a beat, an AY8910 write, a beat. The beats are
    // a quarter-second, well inside the threshold, so they are music not silence.
    let song: &[u8] = &[
        0x58, 0x28, 0xF0, // YM2610 port 0
        0x61, 0x11, 0x2B, // wait a quarter-second
        0xA0, 0x07, 0x38, // AY8910
        0x61, 0x11, 0x2B, // wait a quarter-second
    ];
    // The gap between them: two seconds, well past the default 0.75 s threshold.
    // One 0x61 waits at most 65535 samples, so two of them.
    let gap: &[u8] = &[0x61, 0x44, 0xAC, 0x61, 0x44, 0xAC];

    let mut stream = Vec::new();
    for index in 0..3 {
        if index > 0 {
            stream.extend_from_slice(gap);
        }
        stream.extend_from_slice(song);
    }
    stream.push(0x66);
    let total = 3 * 2 * 11_025 + 2 * 2 * 44_100;

    PickedFile {
        name: "capture.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/Athena/capture.vgm")),
        bytes: other_chip_vgm_bytes(&stream, total, 0),
    }
}

/// Splitting a capture at its silences, for chips this app has no core for.
/// Where a capture falls silent is not an OPL question -- the splitter was the
/// last of the chip-agnostic tools still asking for an OPL stream.
#[test]
fn a_non_opl_capture_can_be_split_into_its_songs() {
    let (mut harness, handles) = build(Some(other_chip_capture_file()), true, false);
    let dir = PathBuf::from("C:/out");
    handles
        .files
        .borrow_mut()
        .output_folders
        .push_back(Some(dir.clone()));

    open_split_songs_dialog(&mut harness);
    assert!(harness.state().dialogs.split_songs.is_some());
    assert!(
        harness.query_by_label_contains("3 song(s) found").is_some(),
        "the two silences part three songs"
    );

    harness.get_by_label_contains("Export").click();
    harness.run();

    let files = handles.files.borrow();
    let written: Vec<(PathBuf, Vec<u8>)> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, bytes } => Some((path.clone(), bytes.clone())),
            SaveRequest::Dialog { .. } => None,
        })
        .collect();
    let names: Vec<PathBuf> = written.iter().map(|(path, _)| path.clone()).collect();
    assert_eq!(
        names,
        ["01 capture.vgm", "02 capture.vgm", "03 capture.vgm"]
            .iter()
            .map(|name| dir.join(name))
            .collect::<Vec<_>>(),
        "three numbered songs written into the chosen folder"
    );

    // Each piece is a real VGM declaring the capture's chips.
    let pieces: Vec<vgms_core::VgmFile> = written
        .iter()
        .map(|(_, bytes)| vgms_core::vgm::file::read("piece.vgm", bytes).expect("a readable VGM"))
        .collect();
    for piece in &pieces {
        assert_eq!(piece.chip_list(), "YM2610");
    }

    // The first song is its own three commands: it starts where the capture
    // does, so there is no state to put in front of it.
    assert_eq!(pieces[0].len(), 3);
    // The two cut from the middle open on the state the capture had reached --
    // one write per chip, ahead of the same three commands.
    for piece in &pieces[1..] {
        assert_eq!(piece.len(), 5, "two restores ahead of the song's own three");
        let stream = piece.stream().expect("the piece walks");
        assert!(stream.describe(0).contains("YM2610"));
        assert!(stream.describe(1).contains("AY8910"));
    }
}

/// Tagging and loop metadata for a document there is no core for. A GD3 tag is
/// a GD3 tag whatever the chips, and so is a loop pointer.
#[test]
fn a_non_opl_document_can_be_tagged_and_have_its_loop_edited() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);

    act(&mut harness, Action::Edit(EditAction::OpenEditTag));
    assert!(
        harness.state().dialogs.gd3_tag.is_some(),
        "the tag dialog opens: {}",
        harness.state().status
    );
    harness.state_mut().editor.set_gd3_tag(vgms_core::Gd3Tag {
        track_name_en: "Psycho Soldier (Arcade)".to_owned(),
        ..vgms_core::Gd3Tag::default()
    });
    assert_eq!(
        harness
            .state()
            .editor
            .vgm()
            .unwrap()
            .tag
            .as_ref()
            .unwrap()
            .track_name_en,
        "Psycho Soldier (Arcade)"
    );
    assert!(harness.state().editor.is_dirty(), "and it wants saving");

    act(&mut harness, Action::Edit(EditAction::OpenVgmMetadata));
    assert!(
        harness.state().dialogs.vgm_metadata.is_some(),
        "the metadata dialog opens: {}",
        harness.state().status
    );
    // The fixture loops at row 1; move it to row 2 and turn the volume up.
    assert!(
        !harness
            .state_mut()
            .editor
            .set_vgm_metadata(Some(2), None, 1, 0, 0x20)
    );
    let file = harness.state().editor.vgm().unwrap();
    assert_eq!(file.loop_index(), Some(2));
    assert_eq!(file.header.volume_modifier(), 0x20);
    assert_eq!(file.header.loop_base(), 1);
    // And the markers followed the stored loop.
    assert_eq!(harness.state().editor.markers.start(), 2);
}

/// An OPL VGM is held as its own bytes, so opening it and saving it back
/// returns the file, not a re-spelling of it -- a file whose header says
/// something unusual (a longer header, a stale sample total, an unusual clock)
/// survives the round trip unchanged. Correcting a header is a deliberate,
/// by-name action.
#[test]
fn opening_an_opl_vgm_and_saving_it_returns_the_same_bytes() {
    // A real OPL VGM, with its declared length falsified so a canonicalising
    // writer would visibly "fix" it.
    let mut bytes = looping_vgm_bytes();
    bytes[0x18..0x1C].copy_from_slice(&999_999u32.to_le_bytes());

    let file = PickedFile {
        name: "looping.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/looping.vgm")),
        bytes: bytes.clone(),
    };
    let (mut harness, _handles) = build(Some(file), false, false);

    // It opened as an OPL VGM -- transport, waveform, the lot -- held as its own
    // file (no projected DroSong behind it any more).
    assert!(
        harness
            .state()
            .editor
            .vgm()
            .is_some_and(|file| file.is_opl())
    );
    assert!(harness.state().editor.dro_song().is_none());
    assert!(harness.state().editor.capabilities().playable);

    assert_eq!(
        harness.state().editor.save_bytes().unwrap(),
        bytes,
        "a save that follows no edit returns the file byte for byte"
    );

    // And the disagreement is still there to be reported, not quietly gone.
    act(&mut harness, Action::Edit(EditAction::AuditHeader));
    assert!(
        !harness.state().alerts.is_empty(),
        "the falsified length is still reported: {}",
        harness.state().status
    );
}

/// Render to WAV follows what the app can actually play, not whether the file
/// is OPL. A Master System rip has a core now, so it is offered and it works.
#[test]
fn a_vgm_this_app_has_a_core_for_can_be_rendered_to_a_wav() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);
    assert!(
        harness.state().editor.dro_song().is_none(),
        "held as a VGM, with no OPL projection"
    );
    assert!(
        harness.state().editor.capabilities().renderable,
        "but there is a core for its chip"
    );

    // The File menu offers both the render and the channel split now: a VGM
    // this app can play splits per chip channel to WAV.
    harness.get_by_label("File").click();
    harness.run();
    assert!(harness.query_by_label_contains("Render to WAV").is_some());
    assert!(
        harness.query_by_label_contains("Split Channels").is_some(),
        "a playable VGM can be split per channel"
    );
    harness.key_press(Key::Escape);
    harness.run();

    act(
        &mut harness,
        Action::File(FileAction::RenderWavSubmitted {
            use_toggles: false,
            use_panning: false,
            boost: 1.0,
            core_choices: Default::default(),
        }),
    );
    harness.run();

    // The render follows the app's configured output rate, not the file's.
    let rate = harness.state().config.audio.frequency as usize;
    let files = handles.files.borrow();
    let Some(SaveRequest::Dialog {
        suggested_name,
        bytes,
    }) = files.save_requests.last()
    else {
        panic!("expected a save dialog, got {:?}", files.save_requests)
    };
    assert_eq!(suggested_name, "01 Bios.vgm.wav");
    assert!(bytes.starts_with(b"RIFF"), "not a WAV");

    // One second of 16-bit stereo, and audible: the header is 44 bytes, and a
    // square wave at full volume is nowhere near silence.
    assert_eq!(bytes.len(), 44 + rate * 4, "a second of stereo");
    let peak = bytes[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]).abs())
        .max()
        .unwrap_or(0);
    assert!(peak > 1000, "and it is audible: peak {peak}");
}

/// The generic render carries a per-chip mix, not just a boost. With the mix
/// opt-ins on but nothing muted or panned, the app must still build a
/// `RenderWavMix::Vgm`: `run_task` pairs source and mix by arm, so an OPL mix on
/// a VGM source would emit no WAV at all. A neutral mix renders byte-identically
/// to the faithful export. (Whether a *muted* channel is actually silenced
/// depends on the core's mute support -- that is pm-3's concern, not rs-0's.)
#[test]
fn a_generic_render_with_neutral_mix_options_stays_faithful() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);

    act(
        &mut harness,
        Action::File(FileAction::RenderWavSubmitted {
            use_toggles: false,
            use_panning: false,
            boost: 1.0,
            core_choices: Default::default(),
        }),
    );
    harness.run();
    let faithful = {
        let files = handles.files.borrow();
        let Some(SaveRequest::Dialog { bytes, .. }) = files.save_requests.last() else {
            panic!("expected a faithful render, got {:?}", files.save_requests)
        };
        bytes.clone()
    };

    act(
        &mut harness,
        Action::File(FileAction::RenderWavSubmitted {
            use_toggles: true,
            use_panning: true,
            boost: 1.0,
            core_choices: Default::default(),
        }),
    );
    harness.run();
    let with_options = {
        let files = handles.files.borrow();
        let Some(SaveRequest::Dialog { bytes, .. }) = files.save_requests.last() else {
            panic!("the generic render dropped the per-chip mix arm (no WAV saved)")
        };
        bytes.clone()
    };

    assert_eq!(
        with_options, faithful,
        "a neutral per-chip mix renders byte-identically to the faithful export"
    );
}

/// The Render to WAV *dialog* opens for a non-OPL VGM, and offers the full mix.
/// It was once gated on require_song(); and even after it opened, the channel
/// toggle/pan rows were hidden for a generic VGM because that render dropped
/// them. rs-0 carries a per-chip mix through the generic render, so every option
/// is offered for any renderable document.
#[test]
fn render_to_wav_dialog_opens_for_a_non_opl_vgm() {
    let (mut harness, _handles) = build(Some(sms_vgm_file()), false, false);
    assert!(harness.state().editor.dro_song().is_none(), "held as a VGM");

    act(&mut harness, Action::File(FileAction::OpenRenderWav));
    assert!(
        harness.state().dialogs.render_wav.is_some(),
        "the dialog opened instead of refusing"
    );

    harness.run();
    // The channel toggle/pan mix now applies to a generic VGM too, so both rows
    // are offered alongside Boost.
    assert!(
        harness.query_by_label_contains("Channel toggles").is_some(),
        "the channel-toggle option is offered for a generic VGM"
    );
    assert!(
        harness.query_by_label_contains("Channel panning").is_some(),
        "the channel-panning option is offered for a generic VGM"
    );
    assert!(
        harness.query_by_label_contains("Boost").is_some(),
        "Boost is still offered"
    );
}

/// The File menu offers Render to WAV with nothing loaded, as it always has --
/// the click is gated, not the menu item. can_render used to drop to the bare
/// `renderable` capability, which is false for an empty editor.
#[test]
fn an_empty_editor_still_offers_render_to_wav() {
    let (mut harness, _handles) = empty_harness();
    assert!(!harness.state().editor.has_document(), "nothing open");

    harness.get_by_label("File").click();
    harness.run();
    assert!(
        harness.query_by_label_contains("Render to WAV").is_some(),
        "the empty-editor File menu still lists Render to WAV"
    );
}

/// A file this app can actually play gets the transport, the waveform and the
/// position readout -- the panels that were absent while OPL was the only thing
/// it could make a sound with.
#[test]
fn a_vgm_this_app_can_play_gets_its_transport_back() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);
    assert!(
        harness.state().editor.dro_song().is_none(),
        "no OPL projection..."
    );
    assert!(
        harness.state().editor.capabilities().playable,
        "...but something would be heard"
    );

    // The waveform render was submitted for it, through the generic engine.
    assert!(
        handles
            .tasks
            .borrow()
            .submitted
            .iter()
            .any(|(kind, _)| *kind == TaskKind::RenderWaveform),
        "a waveform is rendered for it like any other song"
    );

    // And playing it loads it into the audio output.
    act(&mut harness, Action::Playback(PlaybackAction::Play));
    let audio = handles.audio.borrow();
    assert_eq!(
        audio.loaded.as_ref().map(vgms_synth::AudioSource::name),
        Some("01 Bios.vgm"),
        "it reached the output: {}",
        harness.state().status
    );
    assert!(audio.playing);
}

/// A VGM this app can play maps a waveform click to its own timeline --
/// selecting the row, moving the readout -- with no OPL projection in sight.
#[test]
fn a_non_opl_vgm_waveform_click_seeks() {
    let (mut harness, _handles) = build(Some(sms_vgm_file()), true, false);
    harness.run();
    assert!(
        harness.state().editor.dro_song().is_none(),
        "held as a VGM, no projection"
    );
    assert!(
        harness.state().editor.timeline().is_some(),
        "but it still has a timeline to map clicks against"
    );

    act(
        &mut harness,
        Action::Playback(PlaybackAction::WaveformClicked { index: 1, ms: 500 }),
    );
    assert_eq!(harness.state().editor.selection.first(), Some(1));
    assert_eq!(harness.state().position.position_ms(), 500);
}

/// A playable non-OPL VGM draws its waveform like any other song -- the panel,
/// the wave, the transport, all present.
#[test]
fn snapshot_playable_vgm_waveform() {
    let (mut harness, _handles) = build(Some(sms_vgm_file()), true, false);
    settled_snapshot(&mut harness, "playable_vgm_waveform");
}

/// Hardware output is an OPL3, so a document that is not OPL never reaches it --
/// and the controls that only work on samples passing through this program stay
/// live for one, whatever the output setting says.
#[test]
fn the_hardware_output_setting_does_not_grey_a_non_opl_documents_controls() {
    let (mut harness, _handles) = build(Some(sms_vgm_file()), false, false);
    harness
        .state_mut()
        .config
        .audio
        .set_output_backend(vgms_core::config::OutputBackend::RetroWave);
    harness.run();

    assert!(
        !harness.state().config.audio.renders_samples(),
        "the setting says the board mixes its own sound..."
    );
    assert!(
        harness.state().output_renders_samples_for_test(),
        "...but this file never goes to the board, so the meter is live"
    );

    // An OPL song does go to the board, so for that one the setting stands.
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, false);
    harness
        .state_mut()
        .config
        .audio
        .set_output_backend(vgms_core::config::OutputBackend::RetroWave);
    harness.run();
    assert!(!harness.state().output_renders_samples_for_test());
}

/// Marking a loop and turning looping on reaches the output for a non-OPL file
/// too -- the region is a pair of rows, which is not an OPL idea.
#[test]
fn a_non_opl_document_can_loop_a_marked_region() {
    let (mut harness, handles) = build(Some(sms_vgm_file()), true, false);
    let rows = harness.state().editor.len();

    act(&mut harness, Action::Loop(LoopAction::SetStart(1)));
    act(&mut harness, Action::Loop(LoopAction::SetEnd(rows)));
    act(&mut harness, Action::Loop(LoopAction::TogglePlayback));

    let config = handles
        .audio
        .borrow()
        .loops
        .last()
        .copied()
        .flatten()
        .expect("a loop reached the output");
    assert_eq!(config.start, 1);
    assert_eq!(config.end, rows);

    // And auditioning the seam plays rather than refusing.
    act(&mut harness, Action::Playback(PlaybackAction::PlaySeam));
    assert!(
        handles.audio.borrow().playing,
        "the seam plays: {}",
        harness.state().status
    );
}

/// A VGM for chips there is no core for renders silence, so the render is not
/// offered at all -- an empty WAV is a worse answer than an absent menu item.
#[test]
fn a_vgm_with_no_core_is_not_offered_a_wav_render() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    assert!(!harness.state().editor.capabilities().renderable);

    harness.get_by_label("File").click();
    harness.run();
    assert!(harness.query_by_label_contains("Render to WAV").is_none());
}

/// A header that disagrees with its stream is reported and offered, never
/// silently corrected -- and the correction only lands once confirmed.
#[test]
fn a_disagreeing_header_is_offered_for_fixing_rather_than_fixed() {
    // The Neo Geo fixture, with its declared length falsified.
    let mut bytes = other_chip_vgm_file().bytes;
    bytes[0x18..0x1C].copy_from_slice(&999_999u32.to_le_bytes());
    let file = PickedFile {
        name: "03 Wrong.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/Athena/03 Wrong.vgm")),
        bytes,
    };
    let (mut harness, _handles) = build(Some(file), false, false);

    // Nothing has been touched by merely opening it.
    assert_eq!(
        harness.state().editor.vgm().unwrap().header.total_samples(),
        999_999
    );
    assert!(!harness.state().editor.is_dirty());

    act(&mut harness, Action::Edit(EditAction::AuditHeader));
    let alert = harness.state().alerts.front().expect("it offers a fix");
    assert_eq!(alert.title, "Fix Header");
    assert!(alert.message.contains("999999"), "{}", alert.message);
    assert!(alert.message.contains("10735"), "{}", alert.message);
    assert!(alert.confirm.is_some(), "and asks before doing it");
    assert_eq!(
        harness.state().editor.vgm().unwrap().header.total_samples(),
        999_999,
        "still untouched while the question is open"
    );

    act(&mut harness, Action::Edit(EditAction::ConfirmFixHeader));
    let app = harness.state();
    assert_eq!(app.editor.vgm().unwrap().header.total_samples(), 10_735);
    assert!(app.editor.is_dirty(), "and there is something to save");
    assert!(app.status.contains("Corrected 1"), "{}", app.status);
}

/// A Mega Drive rip with a repeated write shrinks, undoably.
#[test]
fn a_non_opl_document_can_be_optimized() {
    use vgms_core::ChipKind;
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x161);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
    bytes.extend_from_slice(&[
        0x52, 0x22, 0x08, // LFO
        0x62, //
        0x52, 0x22, 0x08, // the same value -- droppable
        0x62, 0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);

    let file = PickedFile {
        name: "01 MD.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/MD/01 MD.vgm")),
        bytes,
    };
    let (mut harness, _handles) = build(Some(file), false, false);
    let before = harness.state().editor.save_bytes().unwrap();
    assert_eq!(harness.state().editor.len(), 4);

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));
    let app = harness.state();
    assert_eq!(app.editor.len(), 3, "the repeat is gone");
    assert!(app.status.contains("Optimized"), "{}", app.status);

    harness.state_mut().editor.undo();
    assert_eq!(harness.state().editor.save_bytes().unwrap(), before);
}

/// A chip `vgms_core` has no optimise rules for, which the editor optimises
/// anyway via `vgm_cmp`.
///
/// The built-in `latch_rule` covers the OPL family and the YM2413, and nothing
/// else for now (the YM2612 is deferred to part 3a); the YMZ280B here needs
/// `vgm_cmp`, whose table covers about thirty chips. This is the action
/// reaching it -- the fallback the built-in leans on for uncovered chips.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_chip_the_built_in_pass_cannot_touch_is_optimized_in_the_editor() {
    use vgms_core::ChipKind;
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x161);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Ymz280b.clock_offset(), 16_934_400);
    bytes.extend_from_slice(&[
        0x5D, 0x01, 0x40, //
        0x62, //
        0x5D, 0x01, 0x40, // the same value again -- droppable, but only by vgm_cmp
        0x62, 0x66,
    ]);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);

    // The built-in pass must find nothing here, or the test proves nothing.
    let mut built_in = vgms_core::vgm::file::read("01 Arcade.vgm", &bytes).unwrap();
    assert!(
        built_in.optimize().is_none(),
        "vgms_core should have no rules for the YMZ280B"
    );

    let file = PickedFile {
        name: "01 Arcade.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/Arcade/01 Arcade.vgm")),
        bytes,
    };
    let (mut harness, _handles) = build(Some(file), false, false);
    let before = harness.state().editor.save_bytes().unwrap();
    assert_eq!(harness.state().editor.len(), 4);

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));
    let app = harness.state();
    assert_eq!(app.editor.len(), 3, "the repeat is gone");
    assert!(app.status.contains("Optimized"), "{}", app.status);

    harness.state_mut().editor.undo();
    assert_eq!(harness.state().editor.save_bytes().unwrap(), before);
}

/// A VGM built for chips this app has no core for: an intro write, then a body
/// the capture ran through twice, each pass a second long.
fn non_opl_looping_vgm() -> PickedFile {
    use vgms_core::ChipKind;
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    // 0xAC44 samples is one second at the VGM rate, so the candidate's times
    // are legible in the dialog's table.
    let body: &[u8] = &[0x52, 0x28, 0xF0, 0x61, 0x44, 0xAC, 0x50, 0x9F, 0x62];
    // The intro's own half-second keeps the three time columns distinct.
    let mut stream = vec![0x52, 0x22, 0x08, 0x61, 0xA2, 0x56];
    stream.extend_from_slice(body);
    stream.extend_from_slice(body);
    stream.push(0x66);

    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x161);
    put_u32(&mut bytes, 0x34, 0x100 - 0x34);
    put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
    put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
    bytes.extend_from_slice(&stream);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);

    PickedFile {
        name: "01 Looper.vgm".to_owned(),
        path: Some(PathBuf::from("C:/rips/MD/01 Looper.vgm")),
        bytes,
    }
}

/// The loop search reaches a chip this app has no core for -- a repeated block
/// is a repeated block -- and applying what it finds writes the loop into the
/// file. The whole path, dialog included: a non-OPL document has no `DroSong` to
/// take a candidate's time from, so the times come from its own waits.
#[test]
fn a_non_opl_document_can_have_its_loop_found_and_applied() {
    // Inline tasks so the background search runs synchronously on submit.
    let (mut harness, _handles) = build(Some(non_opl_looping_vgm()), true, false);
    assert!(
        harness.state().editor.dro_song().is_none(),
        "held as a VGM, with no OPL projection"
    );

    act(&mut harness, Action::Loop(LoopAction::OpenSearch));
    assert!(harness.state().dialogs.find_loop.is_some());
    // Two writes is the body length; the search finds the one repeat.
    act(
        &mut harness,
        Action::Loop(LoopAction::Search {
            min_len_commands: 2,
        }),
    );
    harness.run();
    assert_eq!(
        harness
            .state()
            .dialogs
            .find_loop
            .as_ref()
            .unwrap()
            .candidate_count(),
        1
    );

    // The found loop renders as a row, timed from the stream's own waits: it
    // starts when the intro's half-second is up and repeats a second later.
    for time in ["0:00.5", "0:01.5"] {
        assert!(
            harness.query_by_label_contains(time).is_some(),
            "the candidate's times should be listed in the table, missing {time}"
        );
    }

    // The top candidate is pre-selected, so Apply writes it straight into the
    // VGM's loop metadata.
    harness.get_by_label("Apply").click();
    harness.run();
    let app = harness.state();
    let file = app.editor.vgm().unwrap();
    assert_eq!(
        file.loop_index(),
        Some(2),
        "loop point at the body's first write"
    );
    assert_eq!(
        file.loop_end_index(),
        Some(6),
        "loop end where the repeat begins"
    );
    assert!(app.editor.is_dirty(), "and there is something to save");
}

/// An honest header says so rather than opening a box about nothing.
#[test]
fn an_honest_header_reports_that_it_agrees() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    act(&mut harness, Action::Edit(EditAction::AuditHeader));
    assert!(harness.state().alerts.is_empty());
    assert!(
        harness.state().status.contains("agrees with the stream"),
        "{}",
        harness.state().status
    );
}

/// A file whose commands cannot be walked has no rows, so the dialog stays the
/// better answer for it.
#[test]
fn a_vgm_whose_commands_do_not_walk_still_gets_the_dialog() {
    let (harness, _handles) = build(Some(unwalkable_vgm_file()), false, false);
    let app = harness.state();
    assert!(app.dialogs.unwalkable_vgm.is_some());
    assert!(app.editor.vgm().is_none(), "nothing was loaded");
    assert!(
        app.alerts.is_empty(),
        "still not an error: {:?}",
        app.alerts
    );
}

/// The whole point of the step: rows can be selected, deleted and undone in a
/// file the editor cannot decode a single command of into OPL terms.
#[test]
fn a_document_for_other_chips_can_be_trimmed_and_undone() {
    let (mut harness, _handles) = build(Some(other_chip_vgm_file()), false, false);
    let before = harness.state().editor.save_bytes().unwrap();

    harness.state_mut().editor.selection.select_only(1); // the 10000 wait
    assert!(harness.state_mut().editor.delete_selection());
    let app = harness.state();
    assert_eq!(app.editor.len(), 3);
    assert!(app.editor.is_dirty());
    assert!(app.editor.can_undo());
    assert_eq!(app.editor.undo_description(), Some("Delete Command(s)"));

    // The header followed the edit, rather than going stale.
    let file = app.editor.vgm().unwrap();
    assert_eq!(file.header.total_samples(), 10_735 - 10_000);
    assert_eq!(file.header.loop_samples(), Some(735), "the loop shrank too");

    harness.state_mut().editor.undo();
    assert_eq!(harness.state().editor.len(), 4);
    assert_eq!(
        harness.state().editor.save_bytes().unwrap(),
        before,
        "undo restores the file byte for byte"
    );
}

/// A file that is not a song at all still gets the plain error -- the friendly
/// dialog is for readable VGMs only.
#[test]
fn opening_junk_still_raises_the_load_error() {
    let junk = PickedFile {
        name: "notes.vgm".to_owned(),
        path: None,
        bytes: b"this is not a vgm".to_vec(),
    };
    let (harness, _handles) = build(Some(junk), false, false);
    let app = harness.state();
    assert!(app.dialogs.unwalkable_vgm.is_none());
    assert_eq!(app.alerts.len(), 1);
    assert_eq!(app.alerts[0].title, "Failed to load file");
}

#[test]
fn snapshot_auto_trim_alert() {
    let (mut harness, _handles) = build(Some(picked(&bogus_leading_delay_song())), false, true);
    settled_snapshot(&mut harness, "auto_trim_alert");
}

#[test]
fn snapshot_render_wav_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    open_render_wav_dialog(&mut harness);
    settled_snapshot(&mut harness, "render_wav_dialog");
}

#[test]
fn snapshot_split_dialog() {
    let (mut harness, _handles) = build(Some(picked(&tone_song())), false, true);
    open_split_dialog(&mut harness);
    settled_snapshot(&mut harness, "split_dialog");
}

/// The Split dialog for a generic VGM whose chips a write-gate covers now offers
/// the WAV/DroSong format radio -- but not the OPL-only percussion option.
#[test]
fn snapshot_split_dialog_vgm() {
    let (mut harness, _handles) = build(Some(mega_drive_vgm_file()), false, true);
    open_split_dialog(&mut harness);
    settled_snapshot(&mut harness, "split_dialog_vgm");
}

#[test]
fn snapshot_split_songs_dialog() {
    let (mut harness, _handles) = build(Some(picked_vgm(&multi_song_capture())), false, true);
    open_split_songs_dialog(&mut harness);
    settled_snapshot(&mut harness, "split_songs_dialog");
}

#[test]
fn snapshot_pan_strip_custom() {
    // A dual-OPL2 song is two YM3812 tabs; the selected one's pan row engages
    // Custom with a spread of pans so the knobs render at distinct angles in the
    // app's controls panel.
    let (mut harness, _handles) = build(Some(picked(&dual_tone_song())), false, true);
    let mut pans = [0x80u8; 18];
    for (slot, pan) in pans.iter_mut().enumerate() {
        *pan = [0x00, 0x40, 0x80, 0xC0, 0xFF][slot % 5];
    }
    harness.state_mut().channels.set_showcase_pans(pans);
    settled_snapshot(&mut harness, "pan_strip_custom");
}

#[test]
fn opening_a_file_over_unsaved_changes_prompts_first() {
    // Opening a file while the editor has unsaved edits holds it behind a
    // discard-changes confirm instead of clobbering the song.
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.selection.select_only(0);
    harness.state_mut().editor.delete_selection();
    assert!(harness.state().editor.is_dirty());

    let other_name = dual_tone_song().name.clone();
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.run();
    assert!(
        harness.state().editor.is_dirty(),
        "the dirty song is untouched while the prompt is up"
    );
    assert!(harness.state().pending_load.is_some());
    assert!(harness.query_by_label("OK").is_some(), "a confirm is shown");

    // Confirming loads the pending file, replacing the editor song.
    harness.get_by_label("OK").click();
    harness.run();
    assert!(harness.state().pending_load.is_none());
    assert_eq!(harness.state().editor.dro_song().unwrap().name, other_name);
    assert!(!harness.state().editor.is_dirty(), "freshly loaded = clean");
}

#[test]
fn opening_a_file_with_no_unsaved_changes_loads_immediately() {
    // The guard must not prompt when there is nothing to lose.
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(!harness.state().editor.is_dirty());
    let other_name = dual_tone_song().name.clone();
    handles
        .files
        .borrow_mut()
        .picked
        .push_back(Ok(picked(&dual_tone_song())));
    harness.run();
    assert!(
        harness.state().pending_load.is_none(),
        "loaded directly, nothing stashed"
    );
    assert_eq!(harness.state().editor.dro_song().unwrap().name, other_name);
}

#[test]
fn exiting_with_unsaved_changes_prompts_then_sets_quitting_on_confirm() {
    // File > Exit raises the discard-changes confirm rather than quitting;
    // confirming sets the quitting flag (and sends a Close to the viewport).
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.selection.select_only(0);
    harness.state_mut().editor.delete_selection();
    assert!(harness.state().editor.is_dirty());

    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Exit").click();
    harness.run();
    assert!(
        harness
            .query_by_label_contains("Discard unsaved changes")
            .is_some(),
        "the discard-changes confirm is shown"
    );
    assert!(!harness.state().quitting, "the app has not quit yet");

    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        harness.state().quitting,
        "confirming sets the quitting flag"
    );
}

#[test]
fn a_stray_tab_does_not_disable_the_keyboard() {
    // Tab must not move focus onto a chrome button, where a keyboard-input gate
    // would swallow every shortcut and Space would "click" the focused button.
    // Tab is consumed; Space still plays.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let len = harness.state().editor.len();
    harness.state_mut().editor.selection.select_only(0);

    harness.key_press(Key::Tab);
    harness.run();
    harness.key_press(Key::Space);
    harness.run_steps(3);

    assert!(
        handles.audio.borrow().play_calls >= 1,
        "Space reached the editor and toggled playback"
    );
    assert_eq!(
        harness.state().editor.len(),
        len,
        "Tab+Space did not delete a row"
    );
}
