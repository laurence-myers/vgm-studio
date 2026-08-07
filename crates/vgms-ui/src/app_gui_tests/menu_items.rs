//! menu_items tests (split out of app_gui_tests.rs, st-6).

use super::*;

/// Opens the Edit menu and reports which of the format-specific items are on it.
fn edit_menu_items(harness: &mut Harness<'static, VgmStudioApp>) -> Vec<&'static str> {
    harness.get_by_label("Edit").click();
    harness.run();
    // The loop items live in the Loop submenu now, which is a menu of its own;
    // this probes the Edit menu's own format-specific items.
    let present: Vec<&'static str> = [
        "DRO Info...",
        "Edit Tag",
        "Edit VGM Metadata",
        "Optimize VGM",
    ]
    .into_iter()
    // `_contains`, not an exact match: an item carrying a shortcut hint folds
    // that hint into its accessible label ("DRO Info...", "Ctrl+I").
    .filter(|label| harness.query_by_label_contains(label).is_some())
    .collect();
    // Close the menu again so the next open starts clean.
    harness.key_press(Key::Escape);
    harness.run();
    present
}

/// Opens File > Convert and reports which conversions it offers. Empty when the
/// Convert submenu is not shown at all (a VGM, or no song).
fn convert_menu_items(harness: &mut Harness<'static, VgmStudioApp>) -> Vec<&'static str> {
    harness.get_by_label("File").click();
    harness.run();
    // The submenu header renders as "Convert âµ" (with a submenu arrow); its
    // children ("Convert to ...") render only once it is expanded, so until then
    // "Convert" matches the header alone.
    let present = if harness.query_by_label_contains("Convert").is_some() {
        harness.get_by_label_contains("Convert").click();
        harness.run();
        ["Convert to VGM", "Convert to DRO v1"]
            .into_iter()
            .filter(|label| harness.query_by_label_contains(label).is_some())
            .collect()
    } else {
        Vec::new()
    };
    harness.key_press(Key::Escape);
    harness.run();
    present
}

#[test]
fn a_dro_shows_only_the_dro_menu_items() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert_eq!(
        edit_menu_items(&mut harness),
        ["DRO Info..."],
        "a DRO has no tag, no VGM header and nowhere to store a loop"
    );
}

#[test]
fn only_a_dro_can_be_converted() {
    // A DRO offers the Convert submenu; a v1 can go to VGM, a v2 also to v1.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    assert_eq!(
        convert_menu_items(&mut harness),
        ["Convert to VGM"],
        "a v1 has nowhere further down to go"
    );

    let (mut harness, _handles) = harness_with_song(&dro_song_v2());
    assert_eq!(
        convert_menu_items(&mut harness),
        ["Convert to VGM", "Convert to DRO v1"],
    );

    // A VGM has no format this app can convert it to: no submenu at all.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    assert!(convert_menu_items(&mut harness).is_empty());
}

#[test]
fn converting_to_dro_v1_renames_the_song_and_clears_its_path() {
    let (mut harness, _handles) = harness_with_song(&dro_song_v2());
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label_contains("Convert").click();
    harness.run();
    harness.get_by_label_contains("Convert to DRO v1").click();
    harness.run();

    let state = harness.state();
    let song = state.editor.dro_song().expect("still loaded");
    assert_eq!(song.file_version, vgms_core::song::DRO_FILE_V1);
    // The `_1` output name, so a Save As cannot overwrite the v2 source...
    assert_eq!(song.name, "test_1.dro");
    // ...and neither can a plain Save, which now has nowhere to write.
    assert!(state.editor.path.is_none());
    assert_eq!(state.status, "Successfully converted to DRO v1");

    // Converting again offers only the VGM direction: v1 is as far down as it goes.
    assert_eq!(convert_menu_items(&mut harness), ["Convert to VGM"]);
}

#[test]
fn a_vgm_shows_only_the_vgm_menu_items() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.state_mut().editor.convert_to_vgm().unwrap();
    harness.run();
    assert_eq!(
        edit_menu_items(&mut harness),
        ["Edit Tag", "Edit VGM Metadata", "Optimize VGM"],
        "a VGM has no DRO header to inspect and is already converted"
    );
}

#[test]
fn with_no_song_no_format_specific_items_show() {
    let (mut harness, _handles) = empty_harness();
    assert!(edit_menu_items(&mut harness).is_empty());
    assert!(convert_menu_items(&mut harness).is_empty());
}
