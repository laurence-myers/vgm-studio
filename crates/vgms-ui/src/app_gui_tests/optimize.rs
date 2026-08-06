//! optimize tests (split out of app_gui_tests.rs, st-6).

use super::*;

#[test]
fn optimizing_a_vgm_strips_writes_and_reports_the_saving() {
    let (mut harness, _handles) = harness_with_vgm(&redundant_vgm_file());
    let before = harness.state().editor.len();

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));

    let state = harness.state();
    assert!(
        state.editor.len() < before,
        "optimizing should remove commands"
    );
    // The two redundant writes go, and the delays they separated merge into one.
    assert_eq!(state.editor.len(), before - 3);
    assert!(
        state.status.contains("Optimized") && state.status.contains("saved"),
        "status should report the saving, got {:?}",
        state.status
    );
}

#[test]
fn optimize_undo_then_redo_restores_the_exact_bytes() {
    let (mut harness, _handles) = harness_with_vgm(&redundant_vgm_file());
    let stream_bytes = |h: &Harness<'static, VgmStudioApp>| {
        h.state()
            .editor
            .vgm()
            .unwrap()
            .stream()
            .unwrap()
            .commands()
            .to_vec()
    };
    let original = stream_bytes(&harness);

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));
    let optimized = stream_bytes(&harness);
    assert_ne!(optimized, original, "optimizing should change the stream");

    act(&mut harness, Action::Edit(EditAction::Undo));
    assert_eq!(
        stream_bytes(&harness),
        original,
        "undo must restore the original bytes exactly"
    );

    act(&mut harness, Action::Edit(EditAction::Redo));
    assert_eq!(
        stream_bytes(&harness),
        optimized,
        "redo must re-apply the optimization exactly"
    );
}

#[test]
fn optimizing_a_dro_is_refused() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    let before = harness.state().editor.len();

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));

    let state = harness.state();
    assert_eq!(state.editor.len(), before, "a DRO must be left untouched");
    assert!(state.status.contains("Only VGMs"), "got {:?}", state.status);
}

#[test]
fn optimizing_an_already_optimal_vgm_reports_nothing() {
    // A freshly converted VGM has no redundant writes.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    let before = harness.state().editor.len();

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));

    let state = harness.state();
    assert_eq!(state.editor.len(), before, "nothing should change");
    assert!(
        state.status.contains("Nothing to optimize"),
        "got {:?}",
        state.status
    );
}

#[test]
fn optimize_re_derives_the_loop_markers_from_the_remapped_loop() {
    // Loop point at the key-off write (index 7); stripping the two redundant
    // writes before it slides and re-indexes it.
    let mut file = redundant_vgm_file();
    file.set_loop_rows(Some(7), None);
    let (mut harness, _handles) = harness_with_vgm(&file);
    assert_eq!(
        harness.state().editor.markers.start(),
        7,
        "loaded loop point"
    );

    act(&mut harness, Action::Edit(EditAction::OptimizeVgm));

    let state = harness.state();
    let remapped = state
        .editor
        .vgm()
        .unwrap()
        .loop_index()
        .expect("the loop survives optimization");
    // The markers were re-derived from the song's remapped loop, so they agree.
    assert_eq!(state.editor.markers.start(), remapped);
    assert!(
        remapped < 7,
        "the loop point slid left past the stripped writes, got {remapped}"
    );
}

#[test]
fn the_dro_info_shortcut_is_refused_for_a_vgm() {
    // The menu hides the item for a VGM, so Ctrl+I must agree rather than open
    // a dialog the menu says does not apply.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::I);
    harness.run();

    assert!(harness.state().dialogs.dro_info.is_none());
    assert!(
        harness.state().status.contains("VGM Metadata"),
        "it points at the dialog that does apply; status was {:?}",
        harness.state().status
    );
}

#[test]
fn dro_info_offers_editing_when_the_setting_is_on() {
    let (mut harness, _handles) = harness_with_song(&tone_song());

    // "Edit" is also the menu bar's Edit menu, which is always on screen, so
    // the dialog's button is the *second* node with that label.
    let edit_nodes =
        |harness: &mut Harness<'static, VgmStudioApp>| harness.get_all_by_label("Edit").count();

    // Off by default: the dialog is view-only, so only the menu answers.
    act(&mut harness, Action::Edit(EditAction::OpenDroInfo));
    harness.run();
    assert!(harness.state().dialogs.dro_info.is_some());
    assert_eq!(
        edit_nodes(&mut harness),
        1,
        "view-only until the setting is on"
    );
    harness.get_by_label("Close").click();
    harness.run();

    harness.state_mut().config.ui.dro_info_edit_enabled = true;
    act(&mut harness, Action::Edit(EditAction::OpenDroInfo));
    harness.run();
    assert_eq!(
        edit_nodes(&mut harness),
        2,
        "the setting is on, so the dialog should offer Edit"
    );

    // And Edit actually unlocks the fields: the button becomes Save.
    harness
        .get_all_by_label("Edit")
        .nth(1)
        .expect("the dialog's Edit button")
        .click();
    harness.run();
    assert!(harness.query_by_label("Save").is_some(), "now in edit mode");
    assert_eq!(edit_nodes(&mut harness), 1, "the button became Save");
}

#[test]
fn clicking_a_settings_caption_toggles_its_checkbox() {
    // Clicking the caption words, not just the small box, must toggle the
    // setting -- an inert caption reads as a setting that does not work.
    let (mut harness, handles) = harness_with_song(&tone_song());
    assert!(!harness.state().config.ui.dro_info_edit_enabled);

    let config = harness.state().config.clone();
    harness.state_mut().dialogs.settings =
        Some(crate::dialogs::SettingsDialog::new(&config, Vec::new()));
    harness.run();
    // The toggle lives on the Interface tab; open it first.
    harness.get_by_label("Interface").click();
    harness.run();
    harness.get_by_label("Allow editing in DRO Info").click();
    harness.run();
    harness.get_by_label("Save").click();
    harness.run();

    assert!(
        harness.state().config.ui.dro_info_edit_enabled,
        "clicking the caption should have toggled the setting"
    );
    assert_eq!(
        handles
            .saved_configs
            .borrow()
            .last()
            .map(|c| c.ui.dro_info_edit_enabled),
        Some(true),
        "and it should have been persisted"
    );

    // The DRO Info dialog now offers editing -- the user-visible payoff.
    act(&mut harness, Action::Edit(EditAction::OpenDroInfo));
    harness.run();
    assert_eq!(
        harness.get_all_by_label("Edit").count(),
        2,
        "the menu's Edit, plus the dialog's now-available Edit button"
    );
}
