//! pack_mode tests (split out of app_gui_tests.rs, st-6).

use super::*;

/// A VGM fixture re-serialised with a file name and GD3 tag, wrapped as a picked
/// file for a pack folder.
fn tagged_vgm(name: &str, game: &str, author: &str, creator: &str) -> PickedFile {
    let mut file = vgms_core::vgm::file::read(name, VGM_FIXTURE).unwrap();
    file.tag = Some(vgms_core::Gd3Tag {
        game_name_en: game.to_owned(),
        track_author_en: author.to_owned(),
        creator: creator.to_owned(),
        ..vgms_core::Gd3Tag::default()
    });
    PickedFile {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/pack/{name}"))),
        bytes: vgms_core::vgm::file::write(&file).unwrap(),
    }
}

fn pack_folder(name: &str, files: Vec<PickedFile>) -> PickedFolder {
    PickedFolder {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/{name}"))),
        files,
    }
}

/// A two-track "Cool Game" folder.
fn cool_game_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
        ],
    )
}

/// Four tracks, for the reorder tests that need room to walk a track up.
fn four_track_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
            tagged_vgm("03 Cave.vgz", "Cool Game", "Ada", "Ripper"),
            tagged_vgm("04 Ending.vgz", "Cool Game", "Bob", "Ripper"),
        ],
    )
}

/// Queues a folder and runs a frame so `poll_folder` installs it.
fn open_folder(
    harness: &mut Harness<'static, VgmStudioApp>,
    handles: &Handles,
    folder: PickedFolder,
) {
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(folder));
    harness.run();
}

/// The pack view scrolls as one page, so the track-row and screenshot buttons sit
/// well below a 720px viewport; a tall harness keeps them clickable.
fn tall_pack_harness() -> (Harness<'static, VgmStudioApp>, Handles) {
    build_sized(None, false, false, egui::vec2(1000.0, 1700.0))
}

#[test]
fn scanning_pack_volumes_fills_the_peak_map() {
    // Inline tasks run the whole-pack scan; a two-VGM folder.
    let (mut harness, handles) = build(None, true, false);
    open_folder(&mut harness, &handles, cool_game_folder());

    act(&mut harness, Action::Pack(PackAction::ScanVolumes));
    // The inline scan stores its PackPeaks on submit; a poll frame delivers them.
    for _ in 0..4 {
        harness.step();
    }

    let peaks = &harness.state().pack.as_ref().expect("a pack is open").peaks;
    assert_eq!(peaks.len(), 2, "both tracks measured: {peaks:?}");
    assert!(peaks.contains_key("01 Intro.vgz"));
    assert!(peaks.contains_key("02 Boss.vgm"));
}

/// A pack mixing an OPL rip and a non-OPL one (Master System SN76489) scans
/// both. The scan used to filter on the OPL projection, so a non-OPL track kept
/// a "-" in the Peak column forever and the levelling stayed greyed.
#[test]
fn pack_scan_measures_non_opl_tracks_too() {
    let (mut harness, handles) = build(None, true, false);
    let mut sms = sms_vgm_file();
    sms.name = "02 Sms.vgm".to_owned();
    sms.path = Some(PathBuf::from("C:/Mixed/02 Sms.vgm"));
    let folder = pack_folder(
        "Mixed",
        vec![tagged_vgm("01 Opl.vgm", "Mixed", "Ada", "Ripper"), sms],
    );
    open_folder(&mut harness, &handles, folder);

    act(&mut harness, Action::Pack(PackAction::ScanVolumes));
    for _ in 0..4 {
        harness.step();
    }

    let pack = harness.state().pack.as_ref().expect("a pack is open");
    assert_eq!(
        pack.peaks.len(),
        2,
        "both tracks measured: {:?}",
        pack.peaks
    );
    assert!(
        pack.peaks
            .get("01 Opl.vgm")
            .is_some_and(|p| p.max_level > 0),
        "the OPL track was scanned"
    );
    assert!(
        pack.peaks
            .get("02 Sms.vgm")
            .is_some_and(|p| p.max_level > 0),
        "the non-OPL SN76489 track was scanned too"
    );
    assert!(
        pack.suggested_modifier_transaction().is_some(),
        "Apply/Album are no longer greyed out"
    );
}

#[test]
fn a_pack_preview_starts_at_the_tracks_modifier_volume() {
    let (mut harness, handles) = tall_pack_harness();
    // A one-track pack whose track's header modifier asks for 2x (0x20).
    let track = PickedFile {
        name: "01 Loud.vgm".to_owned(),
        path: Some(PathBuf::from("C:/pack/01 Loud.vgm")),
        bytes: vgms_core::vgm::file::write(&vgm_with_modifier(0x20)).unwrap(),
    };
    open_folder(
        &mut harness,
        &handles,
        pack_folder("Loud Pack", vec![track]),
    );

    act(&mut harness, Action::Pack(PackAction::TrackPreview(0)));

    assert_eq!(
        handles.audio.borrow().loaded_boost,
        Some(2.0),
        "the preview loads at the track's 2x header modifier"
    );
    // ...and the editor's stored volume is left untouched by the preview.
    assert_eq!(
        harness.state().config.audio.boost,
        1.0,
        "previewing does not disturb the editor volume"
    );
}

#[test]
fn opening_a_folder_switches_to_the_pack_tab_and_prefills() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    assert_eq!(harness.state().active_tab, AppTab::Pack);
    {
        let state = harness.state();
        let meta = &state.pack.as_ref().expect("a pack is open").meta;
        assert_eq!(meta.game_name, "Cool Game");
        assert_eq!(meta.creator, "Ripper");
        assert_eq!(meta.music_authors, "Ada, Bob");
        // The fake reports a fixed "today", so the history line is deterministic.
        assert_eq!(meta.history, "1.00 2026-07-16 Ripper: Initial release.");
    }
    assert!(harness.state().status.contains("Cool Game"));
}

/// Whether the Pack *tab* is greyed. The menu bar carries a "Pack" menu button
/// with the same label, so pick the node reporting a selected state -- only the
/// tab cells do.
fn pack_tab_is_barred(harness: &Harness<'static, VgmStudioApp>) -> bool {
    harness
        .get_all_by_label("Pack")
        .find(|node| node.accesskit_node().toggled().is_some())
        .expect("the Pack tab cell")
        .accesskit_node()
        .is_disabled()
}

#[test]
fn the_pack_tab_is_barred_until_a_pack_is_open() {
    let (mut harness, handles) = empty_harness();

    // The strip is part of the app's furniture, not something opening a pack
    // conjures up.
    assert!(
        harness.query_by_label("Editor").is_some(),
        "the tab strip is always present"
    );
    assert!(
        pack_tab_is_barred(&harness),
        "with no pack open the Pack view cannot be entered"
    );
    assert_eq!(harness.state().active_tab, AppTab::Editor);

    open_folder(&mut harness, &handles, cool_game_folder());

    assert!(
        !pack_tab_is_barred(&harness),
        "opening a pack frees the tab"
    );
}

#[test]
fn clicking_the_editor_tab_returns_to_the_editor() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);

    harness.get_by_label("Editor").click();
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(
        harness.state().pack.is_some(),
        "the pack project is retained"
    );
    // The editor's empty-state placeholder is back.
    assert!(harness.query_by_label_contains("Open a DRO").is_some());
}

#[test]
fn editing_a_field_marks_the_pack_dirty() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    assert!(!harness.state().pack.as_ref().unwrap().dirty);

    // Type into the first form field (Game name); any edit sets the dirty flag.
    // The metadata fields wrap, so they report as multiline inputs.
    let field = harness
        .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .next()
        .expect("a metadata field");
    field.focus();
    harness.run();
    harness
        .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
        .next()
        .unwrap()
        .type_text("!");
    harness.run();

    assert!(harness.state().pack.as_ref().unwrap().dirty);
}

#[test]
fn a_scanned_track_caches_its_table_entry() {
    // The table entry is computed once at scan, not per row per frame, and
    // matches a fresh computation.
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    let state = harness.state();
    let track = &state.pack.as_ref().unwrap().tracks[0];
    let cached = track
        .entry
        .as_ref()
        .expect("a parsed track caches its entry");
    let fresh = vgms_core::pack::TrackEntry::from_vgm_file(track.vgm().unwrap());
    assert_eq!(*cached, fresh);
}

#[test]
fn save_package_files_writes_the_txt_and_m3u() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.get_by_label("Save Pack").click();
    harness.run();

    let files = handles.files.borrow();
    assert_eq!(
        files.save_requests.len(),
        2,
        "the description and the playlist"
    );
    let mut names = Vec::new();
    for request in &files.save_requests {
        match request {
            SaveRequest::InPlace { path, bytes } => {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.ends_with(".txt") {
                    let text = String::from_utf8(bytes.clone()).unwrap();
                    assert!(text.contains("Game name:           Cool Game"));
                    assert!(text.contains("\r\n"), "CRLF line endings");
                }
                names.push(name);
            }
            other => panic!("expected an in-place save, got {other:?}"),
        }
    }
    assert_eq!(names, ["Cool Game.txt", "Cool Game.m3u"]);
}

#[test]
fn a_failed_package_doc_save_keeps_the_pack_dirty() {
    // If a package-doc save fails, the dirty flag must be kept, not cleared when
    // the batch's last doc lands, so the edits aren't lost.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().pack.as_mut().unwrap().dirty = true;

    harness.state_mut().save_pack_docs();
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Failed("disk full".to_owned()));
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "Cool Game.m3u".to_owned(),
            path: None,
        });
    harness.run();

    assert!(
        harness.state().pack.as_ref().unwrap().dirty,
        "a failed package-doc save keeps the pack dirty so edits aren't lost"
    );
}

#[test]
fn saving_without_a_game_name_shows_an_alert() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness
        .state_mut()
        .pack
        .as_mut()
        .unwrap()
        .meta
        .game_name
        .clear();

    harness.get_by_label("Save Pack").click();
    harness.run();

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing was saved"
    );
    assert!(!harness.state().alerts.is_empty(), "an alert explains why");
}

#[test]
fn editor_keys_are_ignored_on_the_pack_tab() {
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let full_len = song.len();
    harness.state_mut().editor.selection.select_only(0);
    open_folder(&mut harness, &handles, cool_game_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);

    // Delete would remove the selected editor row on the editor tab; here it
    // must do nothing, since the editor is hidden.
    harness.key_press(Key::Delete);
    harness.run();
    assert_eq!(
        harness.state().editor.len(),
        full_len,
        "the hidden song is untouched"
    );
}

/// A one-track folder, so the per-row â–¶/Edit buttons are unambiguous.
fn single_track_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper")],
    )
}

#[test]
fn switching_to_the_pack_tab_stops_editor_playback() {
    // The editor's audio must not keep playing under the pack view. Leaving the
    // editor tab unloads it.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().select_tab(AppTab::Editor);
    harness.state_mut().do_play();
    assert!(handles.audio.borrow().playing);

    harness.state_mut().select_tab(AppTab::Pack);
    assert!(
        !handles.audio.borrow().playing,
        "editor audio stops when the pack tab takes over"
    );
    assert!(harness.state().audio_revision.is_none());
}

#[test]
fn entering_the_pack_tab_closes_song_bound_dialogs() {
    // Goto and the song-bound modeless dialogs are editor-only (the menu
    // disables them on the pack tab), so entering the pack tab must close any
    // that are open.
    use crate::dialogs::{FindRegDialog, GotoDialog};
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().select_tab(AppTab::Editor);

    harness.state_mut().dialogs.goto = Some(GotoDialog::new());
    harness.state_mut().dialogs.find_reg = Some(FindRegDialog::new(&song));

    harness.state_mut().select_tab(AppTab::Pack);
    assert!(
        harness.state().dialogs.goto.is_none(),
        "Goto closes on the pack tab"
    );
    assert!(
        harness.state().dialogs.find_reg.is_none(),
        "song-bound dialogs close on the pack tab"
    );
}

#[test]
fn previewing_a_track_plays_it_and_stop_halts_it() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    // The inline glyph carries the verb as its accessible name.
    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label("Preview").click();
    harness.run_steps(3); // playback requests repaints; `run` would spin.
    {
        let audio = handles.audio.borrow();
        assert!(audio.load_count >= 1, "the track is loaded into the output");
        assert_eq!(audio.play_calls, 1);
        assert!(audio.playing);
    }
    assert_eq!(harness.state().pack.as_ref().unwrap().preview, Some(0));

    // It becomes U+25A0 stop, and says so.
    harness.get_by_label("Stop preview").click();
    harness.run_steps(3);
    assert!(!handles.audio.borrow().playing);
    assert_eq!(harness.state().pack.as_ref().unwrap().preview, None);
}

#[test]
fn previewing_a_track_uses_its_own_panning_not_the_editor_songs() {
    // Editor song is a dual-OPL2 DRO, so its panning is the fixed hard-L/R chip
    // image; the pack track is a mono OPL2 VGM. Previewing the track must use the
    // track's own image, not the editor's -- leaking the editor's hard-L/R onto a
    // mono track plays it hard left (the reported bug). An OPL VGM track previews
    // through the generic VgmEngine path now (Stage K), so isolation shows as the
    // chip mixer reset to neutral rather than an OPL Panning::Original.
    let (mut harness, handles) = build_sized(
        Some(picked(&dual_tone_song())),
        false,
        false,
        egui::vec2(1000.0, 1700.0),
    );

    // Play the editor song so its hard-L/R panning is the last one sent.
    harness.get_by_label("Play").click();
    harness.run_steps(3);
    let mut dual_image = [0x00u8; 18];
    dual_image[9..].fill(0xFF);
    assert_eq!(
        handles.audio.borrow().pannings.last(),
        Some(&vgms_synth::Panning::Custom(dual_image)),
        "the dual-OPL2 editor song sent the hard-L/R image"
    );

    // Open a pack folder and preview its OPL2 track.
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label("Preview").click();
    harness.run_steps(3);

    // The preview resets the chip mixer to neutral, so the track plays its own
    // image (an OPL core reproduces the file's own 0xC0 writes) rather than
    // inheriting the editor's leaked panning/mutes.
    let audio = handles.audio.borrow();
    assert_eq!(
        audio.chip_pannings.last(),
        Some(&vgms_synth::ChipPanning::new()),
        "preview resets to the track's own image, not the editor's hard-L/R"
    );
    assert_eq!(
        audio.chip_mutings.last(),
        Some(&vgms_synth::ChipMuting::new()),
        "preview clears channel mutes"
    );
}

#[test]
fn a_failed_preview_load_does_not_wedge_the_editors_audio() {
    // A failed preview `load` still tears down the editor's stream, so the
    // editor's audio revision must be invalidated up front -- otherwise
    // `ensure_audio` short-circuits on the next editor Play and calls `play()`
    // on an empty output (the "No song is loaded" wedge).
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let editor_name = harness.state().editor.song().unwrap().name.clone();
    open_folder(&mut harness, &handles, single_track_folder());

    // Make the editor's audio current, as if it had just played.
    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    assert!(handles.audio.borrow().playing);

    // Preview a pack track, but force its load to fail.
    handles.audio.borrow_mut().fail_next_load = true;
    harness.state_mut().active_tab = AppTab::Pack;
    harness.state_mut().preview_track(0);
    assert!(
        harness.state().audio_revision.is_none(),
        "a failed preview load invalidates the editor's audio revision"
    );
    assert!(harness.state().pack.as_ref().unwrap().preview.is_none());

    // The editor reloads and plays its own song instead of wedging.
    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    let audio = handles.audio.borrow();
    assert!(audio.playing, "the editor reloaded and plays, not wedged");
    assert_eq!(
        audio.loaded.as_ref().unwrap().name(),
        editor_name,
        "the editor's own song is what reloaded"
    );
}

#[test]
fn a_failed_preview_play_reloads_the_editor_song_not_the_pack_track() {
    // When preview `load` succeeds but `play` fails, the half-started preview
    // must be unloaded and the revision reset, so the next editor Play reloads
    // the *editor's* song rather than resuming the pack track the service still
    // had loaded.
    let song = tone_song();
    let (mut harness, handles) = harness_with_song(&song);
    let editor_name = harness.state().editor.song().unwrap().name.clone();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();

    handles.audio.borrow_mut().fail_next_play = true;
    harness.state_mut().active_tab = AppTab::Pack;
    harness.state_mut().preview_track(0);
    assert_eq!(
        harness.state().pack.as_ref().unwrap().preview,
        None,
        "the half-started preview is dropped"
    );
    assert!(!handles.audio.borrow().playing);

    harness.state_mut().active_tab = AppTab::Editor;
    harness.state_mut().do_play();
    let audio = handles.audio.borrow();
    assert!(audio.playing);
    assert_eq!(
        audio.loaded.as_ref().unwrap().name(),
        editor_name,
        "the editor's own song reloaded, not the pack track"
    );
}

#[test]
fn loading_a_song_switches_to_the_editor_tab_and_stops_preview() {
    // File>Open (or a drop) while the pack tab is active must surface the editor
    // tab and stop any preview, not load invisibly behind the pack view with a
    // stranded play button.
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);

    harness.state_mut().preview_track(0);
    assert_eq!(harness.state().pack.as_ref().unwrap().preview, Some(0));

    // Deliver a song the way menu Open / drag-and-drop would.
    harness.state_mut().load_file(picked(&tone_song()));

    assert_eq!(
        harness.state().active_tab,
        AppTab::Editor,
        "the tab flips to the editor"
    );
    assert_eq!(
        harness.state().pack.as_ref().unwrap().preview,
        None,
        "the preview is stopped"
    );
    assert!(harness.state().editor.has_document());
}

#[test]
fn an_in_place_refresh_keeps_a_playing_preview_by_name() {
    // A same-folder rescan (e.g. after a screenshot optimise redelivers the
    // folder) must not cut a running preview -- it re-matches the preview by
    // file name, even when the rescan reorders the track list.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Preview the second track (02 Boss.vgm).
    harness.state_mut().preview_track(1);
    assert_eq!(harness.state().pack.as_ref().unwrap().preview, Some(1));
    assert!(handles.audio.borrow().playing);

    // Redeliver the same folder with the files reversed, as a real rescan can.
    let reversed = pack_folder(
        "Cool Game",
        vec![
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
        ],
    );
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(reversed));
    harness.run_steps(3);

    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    assert_eq!(pack.tracks[0].file_name, "02 Boss.vgm");
    assert_eq!(
        pack.preview,
        Some(0),
        "the preview follows 02 Boss.vgm to its new index"
    );
    assert!(
        handles.audio.borrow().playing,
        "the preview keeps playing across the in-place refresh"
    );
}

#[test]
fn opening_a_track_loads_it_into_the_editor() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);

    // A row double-click emits PackTrackOpen; kittest cannot double-click, so
    // drive the handler directly (the row-sense wiring is trivial UI code).
    harness.state_mut().open_track_in_editor(0);
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(
        harness.state().editor.has_document(),
        "the track loaded into the editor"
    );
    assert!(
        harness.state().pack.is_some(),
        "the pack project is retained"
    );
}

#[test]
fn open_button_loads_the_track_into_the_editor() {
    // A tall harness so the track row (and its Open button) is on-screen and
    // hit-testable, as the quick-edit test needs too.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);

    // The row menu is the discoverable path to the same handler the
    // double-click drives.
    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label("Track menu").click();
    harness.run();
    harness.get_by_label("Open in editor").click();
    harness.run();

    assert_eq!(harness.state().active_tab, AppTab::Editor);
    assert!(
        harness.state().editor.has_document(),
        "the Open button loaded the track"
    );
    assert!(
        harness.state().pack.is_some(),
        "the pack project is retained"
    );
}

#[test]
fn reordering_renumbers_files_and_is_undoable_and_redoable() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Feed Ok outcomes for every rename the batch issues, and the reordered
    // folder the follow-up rescan installs.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(pack_folder(
            "Cool Game",
            vec![
                tagged_vgm("01 Boss.vgm", "Cool Game", "Bob", "Ripper"),
                tagged_vgm("02 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            ],
        )));
    }

    // Move 01 Intro down a slot; both tracks renumber.
    harness.state_mut().move_pack_track(0, 1);
    harness.run_steps(16);

    {
        let files = handles.files.borrow();
        assert_eq!(files.rename_requests.len(), 4, "a temp-then-final batch");
        let finals: Vec<&String> = files
            .rename_requests
            .iter()
            .map(|(_, to)| to)
            .filter(|to| !to.starts_with(".vgmstudio"))
            .collect();
        assert!(finals.iter().any(|to| *to == "01 Boss.vgm"));
        assert!(finals.iter().any(|to| *to == "02 Intro.vgz"));
    }
    assert_eq!(
        harness.state().pack_undo.len(),
        1,
        "the reorder is undoable"
    );
    assert_eq!(
        harness.state().pack.as_ref().unwrap().tracks[0].file_name,
        "01 Boss.vgm",
        "the rescan installed the new order"
    );

    // Undo: the inverse batch restores the original order.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(cool_game_folder()));
    }
    harness.state_mut().undo_pack_edit();
    harness.run_steps(16);
    assert!(
        harness.state().pack_undo.is_empty(),
        "undo cleared the undo stack"
    );
    assert_eq!(harness.state().pack_redo.len(), 1, "and left a redo");
    assert_eq!(
        harness.state().pack.as_ref().unwrap().tracks[0].file_name,
        "01 Intro.vgz",
        "the original order is back"
    );
}

#[test]
fn dragging_a_track_by_its_grip_moves_it_to_where_it_is_dropped() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder()); // 01 Intro, 02 Boss
    pack_section(&mut harness, PackSection::Tracks);

    // Take hold of track 1's grip and let it go below track 2's midpoint.
    let grips: Vec<egui::Rect> = harness
        .get_all_by_label("\u{2195}")
        .map(|node| node.rect())
        .collect();
    assert_eq!(grips.len(), 2, "one grip per row");
    let (grab, release) = (grips[0].center(), grips[1].center() + egui::vec2(0.0, 6.0));
    harness.drag_at(grab);
    harness.run();
    harness.hover_at(release);
    harness.run();
    harness.drop_at(release);
    harness.run();

    // The batch is issued the moment the row is dropped: a rename per moved
    // file, staged through temp names.
    {
        let files = handles.files.borrow();
        assert!(
            !files.rename_requests.is_empty(),
            "the drop started the reorder batch"
        );
    }

    // Feed the outcomes, and the reordered folder the follow-up rescan installs.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(pack_folder(
            "Cool Game",
            vec![
                tagged_vgm("01 Boss.vgm", "Cool Game", "Bob", "Ripper"),
                tagged_vgm("02 Intro.vgz", "Cool Game", "Ada", "Ripper"),
            ],
        )));
    }
    harness.run_steps(16);

    {
        let files = handles.files.borrow();
        let finals: Vec<&String> = files
            .rename_requests
            .iter()
            .map(|(_, to)| to)
            .filter(|to| !to.starts_with(".vgmstudio"))
            .collect();
        assert!(
            finals.iter().any(|to| *to == "01 Boss.vgm"),
            "the track dropped past moved up, got {finals:?}"
        );
        assert!(finals.iter().any(|to| *to == "02 Intro.vgz"));
    }
    assert_eq!(
        harness.state().pack.as_ref().unwrap().tracks[0].file_name,
        "01 Boss.vgm",
        "the rescan installed the new order"
    );
    assert_eq!(
        harness.state().pack_undo.len(),
        1,
        "and the drag is undoable"
    );
}

#[test]
fn alt_arrow_moves_the_focused_track_and_keeps_it_focused() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder()); // 01 Intro, 02 Boss
    pack_section(&mut harness, PackSection::Tracks);

    // With no focused row there is nothing to move, and the bar says how to get
    // one rather than failing silently.
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    harness.run();
    assert!(
        handles.files.borrow().rename_requests.is_empty(),
        "nothing is focused yet"
    );
    assert!(harness.state().status.contains("Click a track first"));

    // Clicking a row focuses it; Alt+Down then moves it, and the focus travels
    // with it so the keys can be pressed again straight away.
    act(&mut harness, Action::Pack(PackAction::FocusTrack(0)));
    harness.run();
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    harness.run();
    assert!(
        !handles.files.borrow().rename_requests.is_empty(),
        "the focused track moved"
    );
    assert_eq!(
        harness.state().pack.as_ref().unwrap().focused_track,
        Some(1),
        "the focus followed the track, so the key can be pressed again"
    );
}

#[test]
fn a_run_of_keyboard_moves_on_one_track_is_one_undo() {
    // Three presses to lift a track to the top must not be three undos back
    // down: to the person pressing them it was one edit.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, four_track_folder());
    pack_section(&mut harness, PackSection::Tracks);
    act(&mut harness, Action::Pack(PackAction::FocusTrack(3)));

    for _ in 0..3 {
        {
            let mut files = handles.files.borrow_mut();
            for _ in 0..8 {
                files.rename_outcomes.push_back(Ok(()));
            }
        }
        harness.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
        harness.run_steps(16);
    }

    assert_eq!(
        harness.state().pack_undo.len(),
        1,
        "the run folded into one undoable step"
    );
    assert_eq!(
        harness.state().pack.as_ref().unwrap().focused_track,
        Some(0),
        "and the track walked all the way up"
    );

    // A move that does not continue the run starts a new step.
    act(&mut harness, Action::Pack(PackAction::FocusTrack(2)));
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..8 {
            files.rename_outcomes.push_back(Ok(()));
        }
    }
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    harness.run_steps(16);
    assert_eq!(
        harness.state().pack_undo.len(),
        2,
        "a new track, a new step"
    );
}

#[test]
fn a_keyboard_move_asks_to_scroll_the_row_back_into_view() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    pack_section(&mut harness, PackSection::Tracks);

    // Dispatched without a frame in between: the table takes the request as it
    // draws, which is exactly what makes it fire once.
    act(&mut harness, Action::Pack(PackAction::FocusTrack(0)));
    act(
        &mut harness,
        Action::Pack(PackAction::MoveFocusedTrack { delta: 1 }),
    );
    assert_eq!(
        harness.state().pack.as_ref().unwrap().scroll_to_track,
        Some(1)
    );
    harness.run();
    assert_eq!(
        harness.state().pack.as_ref().unwrap().scroll_to_track,
        None,
        "the request is spent once the row has drawn"
    );
}

/// The track table draws the rows that are on screen, not the rows the pack
/// has.
///
/// egui is immediate mode: a `for` loop over the tracks lays out, hit-tests and
/// paints every row of a 150-track pack sixty times a second, and the Tracks
/// view got measurably slower the longer the pack was. Counting the per-row
/// Preview controls is how that stays fixed -- one per drawn row, so the count
/// is the number of rows the table actually built.
#[test]
fn the_track_table_only_builds_the_rows_it_can_show() {
    // A short window, so most of a long pack is off screen.
    let (mut harness, handles) = build_sized(None, false, false, egui::vec2(1000.0, 620.0));
    let tracks: Vec<PickedFile> = (1..=120)
        .map(|n| tagged_vgm(&format!("{n:03} Track.vgz"), "Cool Game", "Ada", "Ripper"))
        .collect();
    open_folder(&mut harness, &handles, pack_folder("Cool Game", tracks));
    pack_section(&mut harness, PackSection::Tracks);
    harness.run();

    let drawn = harness.get_all_by_label("Preview").count();
    assert!(
        drawn > 0,
        "the visible rows still draw their Preview control"
    );
    assert!(
        drawn < 40,
        "{drawn} of 120 rows drew -- the table is laying out the whole pack \
         instead of the visible window"
    );
}

/// ...and a row the keyboard moves is scrolled to even when it was culled.
///
/// The half of the same change that could regress silently: the request is
/// answered outside the table, because a row that is off screen is exactly the
/// row that does not draw and so cannot ask for itself. (It never worked from
/// inside either -- a `scroll_to_me` there is swallowed by the table's own
/// disabled scroll area before the section's can see it.)
#[test]
fn a_keyboard_move_scrolls_a_culled_row_into_view() {
    let (mut harness, handles) = build_sized(None, false, false, egui::vec2(1000.0, 620.0));
    let tracks: Vec<PickedFile> = (1..=120)
        .map(|n| tagged_vgm(&format!("{n:03} Track.vgz"), "Cool Game", "Ada", "Ripper"))
        .collect();
    open_folder(&mut harness, &handles, pack_folder("Cool Game", tracks));
    pack_section(&mut harness, PackSection::Tracks);
    harness.run();

    // Track 100 is far below the fold: its row number is not drawn at all.
    assert!(
        harness.query_by_label("100").is_none(),
        "the fixture must start with track 100 off screen"
    );

    // The request a keyboard move leaves behind, set directly: the move itself
    // renames files and rescans the folder, none of which this is about.
    harness.state_mut().pack.as_mut().unwrap().scroll_to_track = Some(99);
    harness.run();
    assert!(
        harness.query_by_label("100").is_some(),
        "the row is still off screen -- the scroll request never reached the \
         section's scroll area"
    );
}

#[test]
fn moving_the_pointer_hands_the_row_back_to_the_mouse() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    pack_section(&mut harness, PackSection::Tracks);

    act(&mut harness, Action::Pack(PackAction::FocusTrack(1)));
    harness.run();
    assert_eq!(
        harness.state().pack.as_ref().unwrap().focused_track,
        Some(1)
    );

    harness.hover_at(egui::pos2(400.0, 300.0));
    harness.run();
    assert_eq!(
        harness.state().pack.as_ref().unwrap().focused_track,
        None,
        "a moved pointer drops the keyboard's row"
    );
}

#[test]
fn quick_edit_opens_a_dialog_and_saves_a_rewrite() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label("Track menu").click();
    harness.run();
    harness.get_by_label("Quick edit\u{2026}").click();
    harness.run();
    assert!(
        harness.state().dialogs.track_edit.is_some(),
        "the quick-edit dialog opens"
    );

    // Save without changing the name: an in-place rewrite, no rename.
    harness.get_by_label("Save").click();
    harness.run();

    let files = handles.files.borrow();
    assert_eq!(
        files.save_requests.len(),
        1,
        "the track is rewritten in place"
    );
    match &files.save_requests[0] {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("01 Intro.vgz"));
            assert!(
                vgms_core::vgm::file::read("01 Intro.vgz", bytes).is_ok(),
                "the rewritten bytes are a valid VGZ"
            );
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
    assert!(
        files.rename_requests.is_empty(),
        "an unchanged name is not renamed"
    );
}

#[test]
fn quick_edit_after_a_reorder_targets_the_track_by_name() {
    // A rescan can reorder the name-sorted list while the quick-edit dialog is
    // open, so the submit re-resolves the track by its original file name --
    // never a since-stale index.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Reorder: 02 Boss.vgm first, 01 Intro.vgz now at index 1.
    let reversed = pack_folder(
        "Cool Game",
        vec![
            tagged_vgm("02 Boss.vgm", "Cool Game", "Bob", "Ripper"),
            tagged_vgm("01 Intro.vgz", "Cool Game", "Ada", "Ripper"),
        ],
    );
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(reversed));
    harness.run_steps(3);
    assert_eq!(
        harness.state().pack.as_ref().unwrap().tracks[1].file_name,
        "01 Intro.vgz",
        "01 Intro is now at index 1"
    );

    // A quick edit that opened on 01 Intro.vgz renames it; it must touch 01
    // Intro's file, not whatever now sits at the old index 0 (02 Boss.vgm).
    harness.state_mut().quick_edit_submitted(
        "01 Intro.vgz".to_owned(),
        "01 Intro Redux.vgz".to_owned(),
        vgms_core::Gd3Tag::default(),
    );

    let files = handles.files.borrow();
    let (from, to) = files
        .rename_requests
        .last()
        .expect("a rename was requested");
    assert!(
        from.to_string_lossy().ends_with("01 Intro.vgz"),
        "renamed 01 Intro's file, got {from:?}"
    );
    assert_eq!(to, "01 Intro Redux.vgz");
}

#[test]
fn a_rescan_closes_the_open_quick_edit_dialog() {
    // The quick-edit dialog is bound to one track, so a rescan that can reorder
    // or drop tracks must close it.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    harness.state_mut().open_track_quick_edit(0);
    assert!(harness.state().dialogs.track_edit.is_some());

    // Redeliver the folder (a same-folder rescan).
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(cool_game_folder()));
    harness.run_steps(3);
    assert!(
        harness.state().dialogs.track_edit.is_none(),
        "the rescan closed the quick-edit dialog"
    );
}

#[test]
fn quick_edit_rename_rewrites_only_after_the_rename_lands() {
    // A name change must rename first, then rewrite the target-format bytes to
    // the NEW path -- so a failed rename can't leave the old file holding bytes
    // its extension no longer matches.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());

    harness.state_mut().quick_edit_submitted(
        "01 Intro.vgz".to_owned(),
        "01 Intro.vgm".to_owned(),
        vgms_core::Gd3Tag::default(),
    );
    {
        let files = handles.files.borrow();
        assert_eq!(files.rename_requests.len(), 1);
        assert_eq!(files.rename_requests[0].1, "01 Intro.vgm");
        assert!(
            files.save_requests.is_empty(),
            "no byte rewrite before the rename lands"
        );
    }

    // The rename succeeds -> now the bytes are written, to the new path.
    handles.files.borrow_mut().rename_outcomes.push_back(Ok(()));
    harness.run();
    let files = handles.files.borrow();
    match files
        .save_requests
        .last()
        .expect("a rewrite after the rename")
    {
        SaveRequest::InPlace { path, .. } => assert!(
            path.to_string_lossy().ends_with("01 Intro.vgm"),
            "rewrote the renamed file, got {path:?}"
        ),
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

/// An overlay that writes one field (by GD3 index) to a given value.
fn overlay_writing(index: usize, value: &str) -> crate::pack::BulkTagOverlay {
    let mut overlay = crate::pack::BulkTagOverlay::default();
    overlay.apply[index] = true;
    overlay.values[index] = value.to_owned();
    overlay
}

/// Reads back a written VGM/VGZ and returns its GD3 tag.
fn tag_of(name: &str, bytes: &[u8]) -> vgms_core::Gd3Tag {
    vgms_core::vgm::file::read(name, bytes)
        .unwrap()
        .tag
        .clone()
        .unwrap_or_default()
}

/// Drives a pack run to completion: feed one save outcome per write, plus the
/// rescan folder, then step the frame loop.
fn settle_pack_run(harness: &mut Harness<'static, VgmStudioApp>, handles: &Handles, writes: usize) {
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..writes {
            files.save_outcomes.push_back(SaveOutcome::Saved {
                name: "written".to_owned(),
                path: None,
            });
        }
        files.picked_folders.push_back(Ok(cool_game_folder()));
    }
    harness.run_steps(writes + 4);
}

const GD3_TRACK_AUTHOR_EN: usize = 6;

const GD3_GAME_NAME_EN: usize = 2;

#[test]
fn bulk_tag_rewrites_every_selected_track_with_the_checked_field() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    // Push a new composer onto both tracks; every other field is left alone.
    harness.state_mut().bulk_tag_submitted(
        vec!["01 Intro.vgz".to_owned(), "02 Boss.vgm".to_owned()],
        overlay_writing(GD3_TRACK_AUTHOR_EN, "New Composer"),
    );
    settle_pack_run(&mut harness, &handles, 2);

    let files = handles.files.borrow();
    let writes: Vec<(&PathBuf, &Vec<u8>)> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, bytes } => Some((path, bytes)),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 2, "both selected tracks are rewritten");

    for (path, bytes) in writes {
        let name = path.file_name().unwrap().to_string_lossy();
        let tag = tag_of(&name, bytes);
        assert_eq!(
            tag.track_author_en, "New Composer",
            "{name}: author written"
        );
        // Untouched fields keep each track's existing values.
        assert_eq!(tag.game_name_en, "Cool Game", "{name}: game name kept");
        assert_eq!(tag.creator, "Ripper", "{name}: creator kept");
    }
    // The whole bulk edit is one undoable step.
    assert_eq!(
        harness.state().pack_undo.len(),
        1,
        "one transaction, one undo"
    );
}

#[test]
fn bulk_tag_can_target_a_subset_of_tracks() {
    // The composer differs across the pack: only 02 Boss gets the new author.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().bulk_tag_submitted(
        vec!["02 Boss.vgm".to_owned()],
        overlay_writing(GD3_TRACK_AUTHOR_EN, "Only Bob"),
    );
    settle_pack_run(&mut harness, &handles, 1);

    let files = handles.files.borrow();
    let writes: Vec<&PathBuf> = files
        .save_requests
        .iter()
        .filter_map(|request| match request {
            SaveRequest::InPlace { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 1, "only the one selected track is rewritten");
    assert!(
        writes[0].to_string_lossy().ends_with("02 Boss.vgm"),
        "the subset targeted 02 Boss, got {:?}",
        writes[0]
    );
}

#[test]
fn bulk_tag_skips_tracks_whose_tag_would_not_change() {
    // Writing the game name every track already has changes nothing, so no file
    // is rewritten and the run never starts.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    harness.state_mut().bulk_tag_submitted(
        vec!["01 Intro.vgz".to_owned(), "02 Boss.vgm".to_owned()],
        overlay_writing(GD3_GAME_NAME_EN, "Cool Game"),
    );

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "an all-no-op bulk edit writes nothing"
    );
    assert!(
        harness.state().status.contains("nothing changed"),
        "it says so; status was {:?}",
        harness.state().status
    );
    assert!(harness.state().pack_undo.is_empty(), "nothing to undo");
}

#[test]
fn bulk_tag_button_opens_a_dialog() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());

    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label_contains("Bulk Tag").click();
    harness.run();
    assert!(
        harness.state().dialogs.bulk_tag.is_some(),
        "the Bulk Tag button opens the dialog"
    );
}

const PNG_FIXTURE: &[u8] = include_bytes!("../../../../tests/screenshot.png");

/// A folder that passes every export validation (named, numbered, with a png).
/// The png is a real (decodable) image so the inline preview renders.
/// A VGM fixture re-serialised under `name` carrying `tag`.
fn vgm_with_tag(name: &str, tag: vgms_core::Gd3Tag) -> PickedFile {
    let mut file = vgms_core::vgm::file::read(name, VGM_FIXTURE).unwrap();
    file.tag = Some(tag);
    PickedFile {
        name: name.to_owned(),
        path: Some(PathBuf::from(format!("C:/pack/{name}"))),
        bytes: vgms_core::vgm::file::write(&file).unwrap(),
    }
}

/// A single VGM with every submission-required GD3 field filled, so the
/// readiness checks pass clean. The track name matches the file name, and the
/// game/system/date/ripper agree with the pack meta the app prefills.
fn complete_vgm(name: &str) -> PickedFile {
    vgm_with_tag(
        name,
        vgms_core::Gd3Tag {
            track_name_en: vgms_core::pack::naming::title_from_filename(name).to_owned(),
            game_name_en: "Cool Game".to_owned(),
            system_name_en: "IBM PC/AT".to_owned(),
            track_author_en: "Ada".to_owned(),
            release_date: "1994".to_owned(),
            creator: "Ripper".to_owned(),
            ..vgms_core::Gd3Tag::default()
        },
    )
}

/// A pack with a spread of readiness problems across the checklist categories,
/// for the submission-checklist tests and snapshot. Track 1 is missing its
/// System and carries a slash-separated date (which the app also prefills into
/// the pack meta); track 2's game name disagrees with the pack, its file name
/// drifts from its Track Name, and it has no composer. There is no screenshot.
fn dirty_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![
            vgm_with_tag(
                "01 Intro.vgz",
                vgms_core::Gd3Tag {
                    track_name_en: "Intro".to_owned(),
                    game_name_en: "Cool Game".to_owned(),
                    track_author_en: "Ada".to_owned(),
                    release_date: "1994/03".to_owned(),
                    creator: "Ripper".to_owned(),
                    ..vgms_core::Gd3Tag::default()
                },
            ),
            vgm_with_tag(
                "02 Boss.vgz",
                vgms_core::Gd3Tag {
                    track_name_en: "Boss Theme".to_owned(),
                    game_name_en: "Different Game".to_owned(),
                    system_name_en: "IBM PC/AT".to_owned(),
                    release_date: "1994/03".to_owned(),
                    creator: "Ripper".to_owned(),
                    ..vgms_core::Gd3Tag::default()
                },
            ),
        ],
    )
}

/// A submission-ready "Cool Game" pack: one fully tagged track and a screenshot,
/// so [`PackState::validations`] finds nothing to warn about and an export goes
/// straight through without the "export anyway?" confirm.
fn complete_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![
            complete_vgm("01 Intro.vgz"),
            PickedFile {
                name: "Cool Game.png".to_owned(),
                path: Some(PathBuf::from("C:/Cool Game/Cool Game.png")),
                bytes: PNG_FIXTURE.to_vec(),
            },
        ],
    )
}

#[test]
fn a_chip_preset_fills_system_os_and_hardware() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    {
        // Blank the fields so the preset's effect is unambiguous.
        let pack = harness.state_mut().pack.as_mut().unwrap();
        pack.meta.system.clear();
        pack.meta.os.clear();
        pack.meta.music_hardware.clear();
        pack.dirty = false;
    }

    // Presets are a dropdown: open it (a closed combo reports its prompt as its
    // value), then pick through accesskit, as the theme-combo test does.
    harness
        .get_by_value(crate::strings::PACK_PRESET_PROMPT)
        .click();
    harness.run();
    harness.get_by_label("OPL-3").click_accesskit();
    harness.run();

    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    assert_eq!(pack.meta.system, "IBM PC/AT");
    assert_eq!(pack.meta.os, "DOS");
    assert_eq!(pack.meta.music_hardware, "Sound Blaster Pro 2 (YMF262)");
    assert!(pack.dirty, "a preset counts as an edit");
}

/// The Hardware disclosure is an inline triangle now, not a pad button. Clicking
/// it toggles the System / OS / Music hardware fields, both directions.
#[test]
fn the_hardware_disclosure_toggles_the_fields() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    assert!(
        !harness.state().pack.as_ref().unwrap().show_hardware,
        "the fields are folded by default"
    );

    // Folded, the glyph leads the field summary in one clickable label.
    harness.get_by_label_contains("\u{25BA}").click();
    harness.run();
    assert!(
        harness.state().pack.as_ref().unwrap().show_hardware,
        "clicking the inline triangle unfolds the fields"
    );

    // Expanded, the glyph flips and stands alone; clicking it folds again.
    harness.get_by_label("\u{25BC}").click();
    harness.run();
    assert!(!harness.state().pack.as_ref().unwrap().show_hardware);
}

/// A non-OPL console preset fills the fields too, and leaves the OS blank for a
/// cartridge system (the description omits an empty OS line).
#[test]
fn a_console_preset_fills_the_fields_and_leaves_no_os() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    {
        let pack = harness.state_mut().pack.as_mut().unwrap();
        pack.meta.system.clear();
        pack.meta.os = "DOS".to_owned();
        pack.meta.music_hardware.clear();
        pack.dirty = false;
    }

    harness
        .get_by_value(crate::strings::PACK_PRESET_PROMPT)
        .click();
    harness.run();
    harness.get_by_label("Mega Drive").click_accesskit();
    harness.run();

    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    assert_eq!(pack.meta.system, "Sega Mega Drive / Genesis");
    assert_eq!(pack.meta.os, "", "a cartridge console clears the OS");
    assert_eq!(pack.meta.music_hardware, "YM2612, SN76489");
    assert!(pack.dirty);
}

#[test]
fn the_screenshot_inspector_reports_what_the_png_header_says() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    // Dimensions are the fact most likely to be wrong on a submission.
    let _ = harness.get_by_label_contains("320");
    let _ = harness.get_by_label_contains("VGA mode 13h");
    let _ = harness.get_by_label_contains("8-bit palette");

    // With a screenshot present the empty state must be nowhere in sight.
    assert!(
        harness
            .query_by_label_contains("No screenshot in this folder")
            .is_none(),
        "the empty state belongs to a folder with no .png"
    );
}

#[test]
fn a_folder_without_a_png_gets_the_empty_state() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);
    let _ = harness.get_by_label_contains("No screenshot in this folder");
}

#[test]
fn adding_a_screenshot_copies_it_in_under_the_packs_own_name() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder()); // no .png
    pack_section(&mut harness, PackSection::Screenshots);

    // The empty state's pad opens the image picker -- its own channel, so a
    // pending screenshot is never mistaken for a song to open.
    harness.get_by_label_contains("Add Screenshot").click();
    harness.run();
    assert_eq!(handles.files.borrow().pick_image_calls, 1);
    assert_eq!(
        handles.files.borrow().pick_open_calls,
        0,
        "the song picker is a different channel and must stay untouched"
    );

    // The picker delivers a PNG named nothing like the game.
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "dosbox_000.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/dosbox_000.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();

    // Nothing is written yet: the file is named first, and the dialog proposes
    // the pack's own name for it.
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "the picked file is not copied in until its name is settled"
    );
    assert_eq!(
        harness
            .state()
            .dialogs
            .screenshot_rename
            .as_ref()
            .expect("the naming dialog opened")
            .derived_name(),
        "Cool Game.png"
    );

    harness.get_by_label("Add").click();
    harness.run();

    // Recompression happens on the way in, so the write waits for it. The
    // service finds nothing to save here, and the picked bytes go in as they are.
    {
        let pack = handles.pack.borrow();
        assert_eq!(pack.optimize_requests.len(), 1, "recompressed by default");
        assert_eq!(pack.optimize_requests[0].0, "Cool Game.png");
    }
    handles
        .pack
        .borrow_mut()
        .optimized_outcomes
        .push_back(Err("no gain".to_owned()));
    harness.run();

    // It lands in the pack folder as <Game Name>.png, beside the .txt and .m3u.
    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::InPlace { path, bytes } => {
            assert!(
                path.to_string_lossy().ends_with("Cool Game.png"),
                "renamed to the pack's convention, got {}",
                path.display()
            );
            assert_eq!(bytes, PNG_FIXTURE, "the picked bytes are copied verbatim");
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

#[test]
fn an_added_screenshot_is_recompressed_on_the_way_in() {
    // One write, not a write and a rewrite: the smaller bytes are what land, so
    // the file is optimal from the moment it exists and the undo stack stays
    // clear of a recompression nobody asked for.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label_contains("Add Screenshot").click();
    harness.run();
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "dosbox_000.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/dosbox_000.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();
    harness.get_by_label("Add").click();
    harness.run();

    // Nothing is written until the recompression comes back.
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "the write waits for the smaller bytes"
    );
    handles
        .pack
        .borrow_mut()
        .optimized_outcomes
        .push_back(Ok(OptimizedImage {
            name: "Cool Game.png".to_owned(),
            original_len: PNG_FIXTURE.len(),
            bytes: b"\x89PNG smaller".to_vec(),
        }));
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("Cool Game.png"));
            assert_eq!(bytes, b"\x89PNG smaller", "the recompressed bytes landed");
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
    assert_eq!(
        files.save_requests.len(),
        1,
        "written once, not written then rewritten"
    );
}

#[test]
fn closing_the_naming_dialog_leaves_the_picked_screenshot_uncopied() {
    // The dialog holds the bytes, so a cancelled add writes nothing at all.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label_contains("Add Screenshot").click();
    harness.run();
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "dosbox_000.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/dosbox_000.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();

    harness.get_by_label("Close").click();
    harness.run();
    assert!(
        harness.state().dialogs.screenshot_rename.is_none(),
        "Close dismisses the dialog"
    );
    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "and nothing was copied into the folder"
    );
}

#[test]
fn a_second_screenshot_lands_beside_the_first_rather_than_on_it() {
    // A pack may want a title screen per region or graphics mode, so Add is
    // offered once the folder already has one -- and must not overwrite it.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder()); // has "Cool Game.png"
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label_contains("Add Screenshot").click();
    harness.run();
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "dosbox_001.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/dosbox_001.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();
    // The proposed name clears the screenshot already there, and the user can
    // still edit it into "Cool Game (Japan)" before anything is written.
    harness.get_by_label("Add").click();
    harness.run();
    handles
        .pack
        .borrow_mut()
        .optimized_outcomes
        .push_back(Err("no gain".to_owned()));
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::InPlace { path, .. } => assert!(
            path.to_string_lossy().ends_with("Cool Game (2).png"),
            "numbered clear of the screenshot already there, got {}",
            path.display()
        ),
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

#[test]
fn a_picked_file_that_is_not_a_png_is_refused() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    // The picker filters to .png, but a file that only *looks* like one would
    // otherwise ship in the zip and fail review.
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "not-really.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/not-really.png")),
            bytes: b"GIF89a and definitely not a PNG".to_vec(),
        }));
    harness.run();

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing is written when the file is not a PNG"
    );
    assert!(
        harness
            .state()
            .alerts
            .iter()
            .any(|a| a.title == "Not a PNG"),
        "the refusal is surfaced rather than silent"
    );
}

#[test]
fn an_added_screenshot_appears_once_the_folder_rescans() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "shot.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/shot.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();

    // Ack the write, then hand back the folder as it now stands.
    {
        let mut files = handles.files.borrow_mut();
        files.save_outcomes.push_back(SaveOutcome::Saved {
            name: "Cool Game.png".to_owned(),
            path: None,
        });
        let mut folder = single_track_folder();
        folder.files.push(PickedFile {
            name: "Cool Game.png".to_owned(),
            path: Some(PathBuf::from("C:/Cool Game/Cool Game.png")),
            bytes: PNG_FIXTURE.to_vec(),
        });
        files.picked_folders.push_back(Ok(folder));
    }
    harness.run_steps(6);

    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    assert_eq!(pack.images.len(), 1, "the rescan picked the screenshot up");
    assert_eq!(pack.images[0].name, "Cool Game.png");
    assert!(
        pack.images[0].info.is_some_and(|info| info.width == 320),
        "its header was read for the inspector"
    );
}

#[test]
fn replacing_a_screenshot_overwrites_it_and_keeps_its_name() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label_contains("Replace").click();
    harness.run();
    assert_eq!(handles.files.borrow().pick_image_calls, 1);

    // A differently-named PNG: a replace keeps the file it is replacing.
    let mut replacement = PNG_FIXTURE.to_vec();
    replacement.extend_from_slice(b"\x00trailing bytes, so it differs");
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "some other capture.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/some other capture.png")),
            bytes: replacement.clone(),
        }));
    harness.run();

    {
        let files = handles.files.borrow();
        match files.save_requests.last().expect("a save request") {
            SaveRequest::InPlace { path, bytes } => {
                assert!(
                    path.to_string_lossy().ends_with("Cool Game.png"),
                    "the replacement keeps the replaced file's name, got {}",
                    path.display()
                );
                assert_eq!(bytes, &replacement);
            }
            other => panic!("expected an in-place save, got {other:?}"),
        }
    }

    // The write lands, and with the old bytes as its inverse it is undoable.
    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Saved {
            name: "Cool Game.png".to_owned(),
            path: None,
        });
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(complete_folder()));
    harness.run_steps(6);
    assert_eq!(
        harness.state().pack_undo.len(),
        1,
        "a replace is reversible: it holds the bytes it overwrote"
    );
}

/// A "Cool Game" pack whose screenshot still carries its capture name.
fn dosbox_screenshot_folder() -> PickedFolder {
    pack_folder(
        "Cool Game",
        vec![
            complete_vgm("01 Intro.vgz"),
            PickedFile {
                name: "dosbox_000.png".to_owned(),
                path: Some(PathBuf::from("C:/Cool Game/dosbox_000.png")),
                bytes: PNG_FIXTURE.to_vec(),
            },
        ],
    )
}

#[test]
fn renaming_a_screenshot_opens_on_it_and_proposes_the_game_name() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, dosbox_screenshot_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label_contains("Rename").click();
    harness.run();
    let state = harness.state();
    let dialog = state
        .dialogs
        .screenshot_rename
        .as_ref()
        .expect("the rename dialog opened");
    assert_eq!(dialog.original_name(), "dosbox_000.png");
    // ...proposing the pack's own name, which is the whole point of opening it
    // on a file still called what DOSBox called it.
    assert_eq!(dialog.derived_name(), "Cool Game.png");
}

#[test]
fn a_renamed_screenshot_keeps_its_new_name_and_is_undoable() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, dosbox_screenshot_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    // A variant name: the reason the field is editable at all.
    {
        let mut files = handles.files.borrow_mut();
        files.rename_outcomes.push_back(Ok(()));
        files.picked_folders.push_back(Ok(pack_folder(
            "Cool Game",
            vec![
                complete_vgm("01 Intro.vgz"),
                PickedFile {
                    name: "Cool Game (Japan).png".to_owned(),
                    path: Some(PathBuf::from("C:/Cool Game/Cool Game (Japan).png")),
                    bytes: PNG_FIXTURE.to_vec(),
                },
            ],
        )));
    }
    act(
        &mut harness,
        Action::Pack(PackAction::RenameScreenshot {
            original_name: "dosbox_000.png".to_owned(),
            file_name: "Cool Game (Japan).png".to_owned(),
        }),
    );
    harness.run_steps(8);

    {
        let files = handles.files.borrow();
        let (from, to) = files.rename_requests.last().expect("the rename");
        assert!(from.to_string_lossy().ends_with("dosbox_000.png"));
        assert_eq!(to, "Cool Game (Japan).png");
    }
    let state = harness.state();
    assert_eq!(
        state.pack.as_ref().unwrap().images[0].name,
        "Cool Game (Japan).png",
        "the rescan installed the new name"
    );
    assert_eq!(state.pack_undo.len(), 1, "and the rename is undoable");
}

#[test]
fn deleting_a_screenshot_asks_first_then_removes_the_file() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    harness.get_by_label("Delete").click();
    harness.run();
    // Nothing is touched until the prompt is answered.
    assert!(
        handles.files.borrow().delete_requests.is_empty(),
        "the file survives until the prompt is answered"
    );
    assert!(
        harness
            .state()
            .alerts
            .iter()
            .any(|alert| alert.title == "Delete screenshot?"),
        "deleting a file asks first"
    );

    act(
        &mut harness,
        Action::Pack(PackAction::ConfirmDeleteScreenshot(
            "Cool Game.png".to_owned(),
        )),
    );
    harness.run_steps(4);

    let requested = handles.files.borrow().delete_requests.clone();
    assert_eq!(requested.len(), 1, "one file deleted");
    assert!(requested[0].to_string_lossy().ends_with("Cool Game.png"));
}

#[test]
fn a_deleted_screenshot_can_be_undone_back_onto_disk() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Screenshots);

    act(
        &mut harness,
        Action::Pack(PackAction::ConfirmDeleteScreenshot(
            "Cool Game.png".to_owned(),
        )),
    );
    // The rescan after the delete: the folder no longer has the .png.
    handles
        .files
        .borrow_mut()
        .picked_folders
        .push_back(Ok(single_track_folder()));
    harness.run_steps(6);
    assert!(
        harness.state().pack.as_ref().unwrap().images.is_empty(),
        "the screenshot is gone from the pack"
    );
    assert_eq!(harness.state().pack_undo.len(), 1, "and it is undoable");

    // Undo writes the bytes the app still held straight back to the same path.
    act(&mut harness, Action::Edit(EditAction::Undo));
    harness.run_steps(4);
    let files = handles.files.borrow();
    match files.save_requests.last().expect("the restoring write") {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("Cool Game.png"));
            assert_eq!(bytes, PNG_FIXTURE, "restored byte for byte");
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

#[test]
fn optimize_saves_a_smaller_screenshot_in_place() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());

    pack_section(&mut harness, PackSection::Screenshots);
    harness.get_by_label("Recompress").click();
    harness.run();
    {
        let pack = handles.pack.borrow();
        assert_eq!(pack.optimize_requests.len(), 1);
        assert_eq!(pack.optimize_requests[0].0, "Cool Game.png");
    }

    // The service returns smaller bytes: they are saved over the original.
    handles
        .pack
        .borrow_mut()
        .optimized_outcomes
        .push_back(Ok(OptimizedImage {
            name: "Cool Game.png".to_owned(),
            original_len: PNG_FIXTURE.len(),
            bytes: b"\x89PNG smaller".to_vec(),
        }));
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::InPlace { path, bytes } => {
            assert!(path.to_string_lossy().ends_with("Cool Game.png"));
            assert_eq!(bytes, b"\x89PNG smaller");
        }
        other => panic!("expected an in-place save, got {other:?}"),
    }
}

#[test]
fn an_already_optimal_screenshot_is_not_rewritten() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());

    pack_section(&mut harness, PackSection::Screenshots);
    harness.get_by_label("Recompress").click();
    harness.run();
    handles
        .pack
        .borrow_mut()
        .optimized_outcomes
        .push_back(Ok(OptimizedImage {
            name: "Cool Game.png".to_owned(),
            original_len: PNG_FIXTURE.len(),
            bytes: PNG_FIXTURE.to_vec(), // no smaller
        }));
    harness.run();

    assert!(
        handles.files.borrow().save_requests.is_empty(),
        "nothing to save"
    );
    assert!(harness.state().status.contains("already optimal"));
}

#[test]
fn exporting_submits_a_job_and_saves_the_returned_zip() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();

    {
        let pack = handles.pack.borrow();
        assert_eq!(pack.submitted.len(), 1, "a build job was submitted");
        let job = &pack.submitted[0];
        assert_eq!(job.zip_name, "Cool Game.zip");
        assert!(job.gzip_vgms);
        assert!(job.optimize_vgms, "optimize-on-export defaults on");
        let names: Vec<&str> = job
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        for expected in [
            "01 Intro.vgz",
            "Cool Game.png",
            "Cool Game.txt",
            "Cool Game.m3u",
        ] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }

    // The service returns the finished zip; the app saves it via a dialog.
    handles
        .pack
        .borrow_mut()
        .outcomes
        .push_back(PackJobOutcome::Done {
            zip_name: "Cool Game.zip".to_owned(),
            bytes: b"PK\x03\x04".to_vec(),
            log: vec!["Cool Game.png: 100 -> 80 bytes".to_owned()],
        });
    harness.run();

    let files = handles.files.borrow();
    match files.save_requests.last().expect("a save request") {
        SaveRequest::Dialog { suggested_name, .. } => assert_eq!(suggested_name, "Cool Game.zip"),
        other => panic!("expected a save dialog, got {other:?}"),
    }
}

#[test]
fn cancelling_the_export_save_says_so_rather_than_reading_as_done() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, complete_folder());
    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();

    handles
        .pack
        .borrow_mut()
        .outcomes
        .push_back(PackJobOutcome::Done {
            zip_name: "Cool Game.zip".to_owned(),
            bytes: b"PK\x03\x04".to_vec(),
            log: vec!["Gzipped 1 song.".to_owned()],
        });
    harness.run();
    // The build's own line already says the picker is still to come.
    assert!(
        harness.state().status.contains("Choose where"),
        "got {:?}",
        harness.state().status
    );

    handles
        .files
        .borrow_mut()
        .save_outcomes
        .push_back(SaveOutcome::Cancelled);
    harness.run_steps(4);
    let status = harness.state().status.clone();
    assert!(
        status.contains("cancelled") && status.contains("not saved"),
        "a cancelled export must not read as a finished one, got {status:?}"
    );
}

#[test]
fn exporting_without_a_screenshot_prompts_first() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, single_track_folder()); // no .png

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();
    assert!(
        handles.pack.borrow().submitted.is_empty(),
        "no job until confirmed"
    );
    assert!(
        !harness.state().alerts.is_empty(),
        "a warning prompt is shown"
    );

    harness.get_by_label("OK").click();
    harness.run();
    assert_eq!(
        handles.pack.borrow().submitted.len(),
        1,
        "confirming submits the job"
    );
}

#[test]
fn a_failed_export_shows_an_alert() {
    let (mut harness, handles) = empty_harness();
    open_folder(&mut harness, &handles, complete_folder());

    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();
    handles
        .pack
        .borrow_mut()
        .outcomes
        .push_back(PackJobOutcome::Failed("disk full".to_owned()));
    harness.run();

    assert!(
        harness
            .state()
            .alerts
            .iter()
            .any(|alert| alert.title == "Pack export failed"),
        "the failure is surfaced as an alert"
    );
}

#[test]
fn snapshot_pack_view() {
    // The section a pack opens on: name row, section tabs, metadata form.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 900.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.run();
    settled_snapshot(&mut harness, "pack_view");
}

#[test]
fn snapshot_pack_meta_long_value() {
    // A game name longer than its box wraps inside the form and pushes the row
    // taller, as the dialogs' fields do -- it is not scrolled out of sight.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 900.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.state_mut().pack.as_mut().unwrap().meta.game_name =
        "A Game With A Truly Preposterous Subtitle: The Revenge".to_owned();
    harness.run();
    settled_snapshot(&mut harness, "pack_meta_long_value");
}

#[test]
fn snapshot_pack_view_scrolled() {
    // A short viewport, so the section overflows and the outer scrollbar appears --
    // framed with the sunken well bevel, flush to the panel edge.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 460.0));
    open_folder(&mut harness, &handles, complete_folder());
    harness.run();
    settled_snapshot(&mut harness, "pack_view_scrolled");
}

#[test]
fn snapshot_pack_tracks() {
    // The Tracks section: the only one carrying the LEVELS and TAGS tool groups.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 500.0));
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Tracks);
    settled_snapshot(&mut harness, "pack_tracks");
}

#[test]
fn snapshot_pack_screenshots() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 500.0));
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Screenshots);
    settled_snapshot(&mut harness, "pack_screenshots");
}

#[test]
fn snapshot_pack_tracks_scrolled() {
    // Many tracks at the app's default window, so the Tracks section overflows.
    // Its scrollbar handle only shows while nothing is drawn over it -- content
    // that overflows *horizontally* lands on top of the bar and buries the
    // handle, which is what "the scrollbar pill is missing" turned out to be.
    let mut files: Vec<PickedFile> = (1..=12)
        .map(|n| complete_vgm(&format!("{n:02} Track.vgz")))
        .collect();
    files.push(PickedFile {
        name: "Cool Game.png".to_owned(),
        path: Some(PathBuf::from("C:/Cool Game/Cool Game.png")),
        bytes: PNG_FIXTURE.to_vec(),
    });
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(800.0, 600.0));
    open_folder(&mut harness, &handles, pack_folder("Cool Game", files));
    pack_section(&mut harness, PackSection::Tracks);
    settled_snapshot(&mut harness, "pack_tracks_scrolled");
}

#[test]
fn snapshot_pack_screenshots_empty() {
    // A folder with no .png: the section says what a submission wants rather
    // than only reporting that nothing is there.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 400.0));
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);
    settled_snapshot(&mut harness, "pack_screenshots_empty");
}

#[test]
fn snapshot_export_warning_dialog() {
    // The app's own default window, and a pack that trips eight checks: the box
    // has to stay inside the screen with its buttons reachable, which means the
    // list scrolls rather than the box growing.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(800.0, 600.0));
    open_folder(&mut harness, &handles, dirty_folder());
    harness.run();
    harness.get_by_label("Export Zip\u{2026}").click();
    harness.run();
    settled_snapshot(&mut harness, "export_warning_dialog");
}

#[test]
fn snapshot_pack_checklist_narrow() {
    // The app's own default window width. The checklist's longest messages run
    // past 90 characters; an extending line that does not wrap overflows the
    // panel and is drawn over the scrollbar, burying the handle. This guards the
    // wrap.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(800.0, 600.0));
    open_folder(&mut harness, &handles, dirty_folder());
    pack_section(&mut harness, PackSection::Checklist);
    settled_snapshot(&mut harness, "pack_checklist_narrow");
}

/// A tall interaction harness with the dirty pack open, so the whole submission
/// checklist is on-screen and its lines are real click targets.
fn dirty_checklist_harness() -> (Harness<'static, VgmStudioApp>, Handles) {
    let (mut harness, handles) = build_sized(None, false, false, egui::vec2(1280.0, 1400.0));
    open_folder(&mut harness, &handles, dirty_folder());
    harness.run();
    pack_section(&mut harness, PackSection::Checklist);
    (harness, handles)
}

#[test]
fn the_submission_checklist_lists_the_readiness_problems() {
    let (harness, _handles) = dirty_checklist_harness();
    // A line from each of several categories: track tags, consistency, files.
    let _ = harness.get_by_label_contains("01 Intro: missing System");
    let _ = harness.get_by_label_contains("differs from the pack's");
    let _ = harness.get_by_label_contains("There is no screenshot");
}

#[test]
fn clicking_a_checklist_track_item_opens_that_tracks_quick_edit() {
    let (mut harness, _handles) = dirty_checklist_harness();
    // The consistency line belongs to track 2 ("02 Boss.vgz").
    harness
        .get_by_label_contains("differs from the pack's")
        .click();
    harness.run();
    let state = harness.state();
    let dialog = state
        .dialogs
        .track_edit
        .as_ref()
        .expect("the quick-edit dialog opened");
    assert_eq!(dialog.original_name(), "02 Boss.vgz");
}

#[test]
fn clicking_a_meta_checklist_item_focuses_its_form_field() {
    let (mut harness, _handles) = dirty_checklist_harness();
    // The bad pack date is a Package-info line targeting the release-date field.
    harness
        .get_by_label_contains("should be a hyphen-separated date")
        .click();
    harness.run();
    // The form consumed the focus request (so it does not re-fire next frame)...
    assert!(
        harness.state().pack.as_ref().unwrap().focus_field.is_none(),
        "the form takes focus_field the frame after the click"
    );
    // ...and a form field now holds keyboard focus.
    assert!(
        harness.ctx.memory(|memory| memory.focused()).is_some(),
        "a field took focus"
    );
}

#[test]
fn a_checklist_category_folds_its_findings_away() {
    let (mut harness, _handles) = dirty_checklist_harness();
    // One of Package info's findings, to watch disappear and come back.
    let _ = harness.get_by_label_contains("should be a hyphen-separated date");

    // The heading's triangle folds the group; the count stands in for it.
    harness.get_by_label("Hide Package info").click();
    harness.run();
    assert!(
        harness.state().pack.as_ref().unwrap().collapsed[0],
        "the category is folded"
    );
    assert!(
        harness
            .query_by_label_contains("should be a hyphen-separated date")
            .is_none(),
        "its findings are hidden while it is folded"
    );
    let _ = harness.get_by_label_contains("1 item");

    harness.get_by_label("Show Package info").click();
    harness.run();
    let _ = harness.get_by_label_contains("should be a hyphen-separated date");
}

#[test]
fn the_loops_heading_tallies_how_many_tracks_loop() {
    let (harness, _handles) = dirty_checklist_harness();
    // Neither of the dirty pack's two tracks carries a loop point.
    assert_eq!(harness.state().pack.as_ref().unwrap().loop_tally(), (0, 2));
    let _ = harness.get_by_label_contains("0/2 looping");
}

#[test]
fn the_loopless_tracks_are_listed_one_per_line() {
    let (harness, _handles) = dirty_checklist_harness();
    let listed = harness
        .state()
        .pack
        .as_ref()
        .unwrap()
        .readiness_items()
        .into_iter()
        .find(|item| item.message.starts_with("No loop point"))
        .expect("both tracks are loopless");
    // One line of preamble plus one line per track: a comma-joined run was what
    // overflowed the panel and buried the scrollbar handle behind it.
    assert_eq!(
        listed.message.lines().count(),
        3,
        "expected a line each, got {:?}",
        listed.message
    );
    assert!(!listed.message.contains(", "), "{:?}", listed.message);
}

#[test]
fn the_scan_buttons_are_barred_while_their_scan_runs() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, cool_game_folder());
    pack_section(&mut harness, PackSection::Tracks);
    assert!(
        !harness
            .get_by_label("Scan Volumes")
            .accesskit_node()
            .is_disabled(),
        "live when nothing is scanning"
    );

    // A second click would cancel the running scan and start over, which reads
    // as the button doing nothing. Stepped rather than `run`: a task pinned busy
    // requests a repaint every frame, so the harness never goes idle.
    handles
        .tasks
        .borrow_mut()
        .busy
        .push(TaskKind::PackVolumeScan);
    harness.run_steps(2);
    assert!(
        harness
            .get_by_label("Scan Volumes")
            .accesskit_node()
            .is_disabled(),
        "barred while the pack scan runs"
    );

    // The editor's Match button is barred by its own scan, not the pack's.
    act(
        &mut harness,
        Action::Pack(PackAction::SelectTab(AppTab::Editor)),
    );
    harness.run_steps(2);
    assert!(
        !harness.get_by_label("Match").accesskit_node().is_disabled(),
        "a pack scan does not bar the editor's Match"
    );
    handles.tasks.borrow_mut().busy.push(TaskKind::VolumeScan);
    harness.run_steps(2);
    assert!(
        harness.get_by_label("Match").accesskit_node().is_disabled(),
        "barred while the song scan runs"
    );
}

#[test]
fn the_output_deck_counts_the_outstanding_work_and_jumps_to_it() {
    let (mut harness, _handles) = dirty_checklist_harness();
    let summary = harness.state().pack.as_ref().unwrap().readiness_summary().1;
    assert!(
        summary.contains("warning"),
        "the dirty pack's verdict counts warnings, got {summary:?}"
    );
    // The deck's lamp is captioned with exactly that phrase...
    let _ = harness.get_by_label(&summary);
    // ...and its link is the way from the verdict back to the detail.
    harness.get_by_label("view checklist").click();
    harness.run();
    assert_eq!(
        harness.state().pack.as_ref().unwrap().section,
        PackSection::Checklist,
        "the verdict link opens the checklist section"
    );
}

#[test]
fn a_note_leaves_the_deck_reading_as_shippable() {
    // The complete pack's only outstanding item is the optional Loops note, and
    // notes never gate an export -- so the verdict must not read as a problem.
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    let (severity, summary) = harness.state().pack.as_ref().unwrap().readiness_summary();
    assert!(
        matches!(severity, Some(vgms_core::pack::readiness::Severity::Note)),
        "expected the note tier, got {severity:?} ({summary})"
    );
    assert!(
        !summary.contains("blocked"),
        "a note must not claim the export is blocked: {summary:?}"
    );
    let _ = harness.get_by_label(&summary);
}

#[test]
fn the_export_options_latch_on_the_deck() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    assert!(
        harness.state().pack.as_ref().unwrap().gzip_on_export,
        "gzipping to .vgz is the default"
    );

    // The sentence-long checkbox is now a pad that latches.
    harness.get_by_label("VGZ").click();
    harness.run();
    assert!(
        !harness.state().pack.as_ref().unwrap().gzip_on_export,
        "the VGZ pad unlatches the .vgz conversion"
    );
}

#[test]
fn converting_dates_to_hyphens_fixes_the_pack_in_one_undoable_step() {
    let (mut harness, handles) = dirty_checklist_harness();
    // The date the app prefilled from the first track is slash-separated.
    assert_eq!(
        harness.state().pack.as_ref().unwrap().meta.release_date,
        "1994/03"
    );
    // The fix-assist pad is live while a slash date remains. It keeps its place
    // in the TAGS group either way -- greyed rather than gone -- so the tool row
    // does not reflow the moment it is used.
    pack_section(&mut harness, PackSection::Tracks);
    assert!(
        !harness
            .get_by_label("Fix Dates")
            .accesskit_node()
            .is_disabled(),
        "the fix-assist is offered while a slash date remains"
    );

    // Feed Ok save outcomes for the two track writes, then the rescan folder the
    // batch installs -- now carrying hyphenated GD3 dates.
    {
        let mut files = handles.files.borrow_mut();
        files.save_outcomes.push_back(SaveOutcome::Saved {
            name: "01 Intro.vgz".to_owned(),
            path: None,
        });
        files.save_outcomes.push_back(SaveOutcome::Saved {
            name: "02 Boss.vgz".to_owned(),
            path: None,
        });
        files.picked_folders.push_back(Ok(pack_folder(
            "Cool Game",
            vec![
                vgm_with_tag(
                    "01 Intro.vgz",
                    vgms_core::Gd3Tag {
                        track_name_en: "Intro".to_owned(),
                        game_name_en: "Cool Game".to_owned(),
                        track_author_en: "Ada".to_owned(),
                        release_date: "1994-03".to_owned(),
                        creator: "Ripper".to_owned(),
                        ..vgms_core::Gd3Tag::default()
                    },
                ),
                vgm_with_tag(
                    "02 Boss.vgz",
                    vgms_core::Gd3Tag {
                        track_name_en: "Boss Theme".to_owned(),
                        game_name_en: "Different Game".to_owned(),
                        system_name_en: "IBM PC/AT".to_owned(),
                        release_date: "1994-03".to_owned(),
                        creator: "Ripper".to_owned(),
                        ..vgms_core::Gd3Tag::default()
                    },
                ),
            ],
        )));
    }

    // Dispatch the fix-assist directly (as the button does), so the batch is
    // built from the current slash dates before any frame's folder poll runs --
    // the same pattern the reorder test uses.
    act(
        &mut harness,
        Action::Pack(PackAction::ConvertDatesToHyphens),
    );
    harness.run_steps(16);

    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    // The pack date converted immediately (a form edit)...
    assert_eq!(pack.meta.release_date, "1994-03");
    // ...and the rescan installed both tracks with hyphenated GD3 dates.
    for track in &pack.tracks {
        let tag = track.tag().unwrap();
        assert_eq!(tag.release_date, "1994-03", "{} converted", track.file_name);
    }
    // The track rewrites landed as one undoable batch, and no slash date is left.
    assert_eq!(state.pack_undo.len(), 1, "one undoable batch");
    assert!(
        !pack.has_convertible_dates(),
        "the fix-assist has nothing left"
    );
    // With nothing left to convert the pad greys out rather than vanishing.
    assert!(
        harness
            .get_by_label("Fix Dates")
            .accesskit_node()
            .is_disabled(),
        "the spent fix-assist is greyed, not removed"
    );
}

#[test]
fn fixing_names_renames_each_file_from_its_tag_in_one_undoable_step() {
    let named = |file_name: &str, track_name: &str| {
        vgm_with_tag(
            file_name,
            vgms_core::Gd3Tag {
                track_name_en: track_name.to_owned(),
                game_name_en: "Cool Game".to_owned(),
                system_name_en: "IBM PC/AT".to_owned(),
                track_author_en: "Ada".to_owned(),
                release_date: "1994".to_owned(),
                creator: "Ripper".to_owned(),
                ..vgms_core::Gd3Tag::default()
            },
        )
    };
    let (mut harness, handles) = tall_pack_harness();
    open_folder(
        &mut harness,
        &handles,
        pack_folder(
            "Cool Game",
            vec![
                named("01 Intro.vgz", "Intro"), // already correct
                named("02 Boss.vgz", "Doom II: Hell on Earth"),
            ],
        ),
    );
    pack_section(&mut harness, PackSection::Tracks);

    // The drift is on the checklist, and it names the file the fix will write.
    assert!(
        harness
            .state()
            .pack
            .as_ref()
            .unwrap()
            .readiness_items()
            .iter()
            .any(|item| item
                .message
                .contains("expected \"02 Doom II - Hell on Earth.vgz\"")),
        "the check reports the vgm_ren name"
    );
    assert!(
        !harness
            .get_by_label("Fix File Names")
            .accesskit_node()
            .is_disabled(),
        "the fix-assist is offered while a name has drifted"
    );

    // Ok outcomes for the temp-then-final rename pair, and the renamed folder
    // the follow-up rescan installs.
    {
        let mut files = handles.files.borrow_mut();
        for _ in 0..4 {
            files.rename_outcomes.push_back(Ok(()));
        }
        files.picked_folders.push_back(Ok(pack_folder(
            "Cool Game",
            vec![
                named("01 Intro.vgz", "Intro"),
                named("02 Doom II - Hell on Earth.vgz", "Doom II: Hell on Earth"),
            ],
        )));
    }
    // Dispatched directly (as the button does) so the batch is built before any
    // frame's folder poll installs the rescan above -- as the reorder test does.
    act(&mut harness, Action::Pack(PackAction::RenameFromTags));
    harness.run_steps(16);

    {
        let files = handles.files.borrow();
        let finals: Vec<&String> = files
            .rename_requests
            .iter()
            .map(|(_, to)| to)
            .filter(|to| !to.starts_with(".vgmstudio"))
            .collect();
        assert_eq!(
            finals,
            ["02 Doom II - Hell on Earth.vgz"],
            "only the drifted track is renamed, to its vgm_ren name"
        );
    }
    let state = harness.state();
    let pack = state.pack.as_ref().unwrap();
    assert_eq!(state.pack_undo.len(), 1, "one undoable batch");
    assert_eq!(pack.tracks[1].file_name, "02 Doom II - Hell on Earth.vgz");
    assert!(!pack.has_tag_renames(), "the fix-assist has nothing left");
    // With nothing left to rename the pad greys out rather than vanishing.
    assert!(
        harness
            .get_by_label("Fix File Names")
            .accesskit_node()
            .is_disabled(),
        "the spent fix-assist is greyed, not removed"
    );
}

/// Opens one menu on a pack harness and hands the harness back for querying.
/// One menu per test: reopening a second one in the same test does not register.
fn pack_harness_with_menu(menu: &str) -> Harness<'static, VgmStudioApp> {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    assert_eq!(harness.state().active_tab, AppTab::Pack);
    harness.get_by_label(menu).click();
    harness.run();
    harness
}

#[test]
fn on_the_pack_tab_file_carries_the_packs_outputs() {
    // Both openers stay -- that is how you get from one tab to the other -- but
    // the song commands are left out rather than greyed.
    let harness = pack_harness_with_menu("File");
    // An item with a shortcut carries it in its accessible name, so the openers
    // are matched by prefix rather than exactly.
    let _ = harness.get_by_label_contains("Open Song...");
    let _ = harness.get_by_label("Open Pack Folder...");
    let _ = harness.get_by_label("Save Package Files");
    let _ = harness.get_by_label("Close Pack");
    assert!(
        harness.query_by_label_contains("Save As").is_none(),
        "the song's Save As has no song to act on here"
    );
}

#[test]
fn on_the_pack_tab_edit_leaves_out_the_song_commands() {
    let harness = pack_harness_with_menu("Edit");
    let _ = harness.get_by_label_contains("Undo");
    let _ = harness.get_by_label_contains("Redo");
    for absent in ["Goto", "Find Register", "Delete Instruction"] {
        assert!(
            harness.query_by_label_contains(absent).is_none(),
            "{absent} edits a song the pack tab does not show"
        );
    }
}

#[test]
fn on_the_pack_tab_edit_carries_the_track_operations() {
    // The two silkscreen groups from the Tracks section, as submenus -- so the
    // batch operations have a keyboard-reachable home as well as a pad.
    // A submenu button carries its right-arrow in its accessible name.
    let mut harness = pack_harness_with_menu("Edit");
    let _ = harness.get_by_label_contains("Levels");
    let _ = harness.get_by_label_contains("Track Tags");

    harness.get_by_label_contains("Levels").click();
    harness.run();
    let _ = harness.get_by_label("Scan Volumes");
    // Two items, not one plus a latch: a menu cannot show which mode is armed.
    let _ = harness.get_by_label("Apply Album Level");
    let _ = harness.get_by_label("Apply Track Levels");
}

#[test]
fn applying_album_levels_from_the_menu_arms_the_album_latch() {
    let (mut harness, handles) = tall_pack_harness();
    open_folder(&mut harness, &handles, complete_folder());
    // Start from the other mode, so the change is unambiguous.
    harness.state_mut().pack.as_mut().unwrap().album_normalize = false;

    act(
        &mut harness,
        Action::Pack(PackAction::ApplySuggestedModifiers { album: true }),
    );
    harness.run();
    assert!(
        harness.state().pack.as_ref().unwrap().album_normalize,
        "the pad reflects the levelling the menu item asked for"
    );
}

#[test]
fn in_the_editor_file_carries_the_song_commands_and_both_openers() {
    // With a song loaded: the empty state's hint names File > Open Song... too,
    // and would match the menu item's query twice over.
    let (mut harness, _handles) = harness_with_song(&tone_song());
    harness.get_by_label("File").click();
    harness.run();
    let _ = harness.get_by_label_contains("Open Song...");
    let _ = harness.get_by_label("Open Pack Folder...");
    let _ = harness.get_by_label("Open Pack Zip...");
    let _ = harness.get_by_label_contains("Save As...");
    assert!(
        harness.query_by_label("Save Package Files").is_none(),
        "the pack's outputs belong to the pack tab"
    );
}

/// A pack in a `.zip` is openable from the menu, not only by dropping it: the
/// item opens the zip picker rather than the folder one.
#[test]
fn open_pack_zip_opens_the_zip_picker() {
    let (mut harness, handles) = harness_with_song(&tone_song());
    harness.get_by_label("File").click();
    harness.run();
    harness.get_by_label("Open Pack Zip...").click();
    harness.run();

    let files = handles.files.borrow();
    assert_eq!(files.pick_pack_zip_calls, 1, "the zip picker opened");
    assert_eq!(files.pick_folder_calls, 0, "not the folder picker");
}

#[test]
fn snapshot_pack_checklist_dirty() {
    // Wider than the other pack snapshots so the header's tool groups fit and
    // the checklist's glyphs and category headings are all in frame.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1280.0, 1200.0));
    open_folder(&mut harness, &handles, dirty_folder());
    pack_section(&mut harness, PackSection::Checklist);
    settled_snapshot(&mut harness, "pack_checklist_dirty");
}

#[test]
fn snapshot_pack_checklist_clean() {
    // A submission-ready pack: the checklist collapses to ticks (the single
    // non-looping track leaves just the optional Loops note).
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1280.0, 1000.0));
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Checklist);
    settled_snapshot(&mut harness, "pack_checklist_clean");
}

#[test]
fn snapshot_track_edit_dialog() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label("Track menu").click();
    harness.run();
    harness.get_by_label("Quick edit\u{2026}").click();
    harness.run();
    settled_snapshot(&mut harness, "track_edit_dialog");
}

#[test]
fn snapshot_screenshot_rename_dialog() {
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 700.0));
    open_folder(&mut harness, &handles, dosbox_screenshot_folder());
    pack_section(&mut harness, PackSection::Screenshots);
    harness.get_by_label_contains("Rename").click();
    harness.run();
    settled_snapshot(&mut harness, "screenshot_rename_dialog");
}

#[test]
fn snapshot_add_screenshot_dialog() {
    // The same box in its other job: naming a picked file before it is copied
    // in, so "Copying:" names the source rather than a file in the folder.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 700.0));
    open_folder(&mut harness, &handles, single_track_folder());
    pack_section(&mut harness, PackSection::Screenshots);
    harness.get_by_label_contains("Add Screenshot").click();
    harness.run();
    handles
        .files
        .borrow_mut()
        .picked_images
        .push_back(Ok(PickedFile {
            name: "dosbox_000.png".to_owned(),
            path: Some(PathBuf::from("C:/captures/dosbox_000.png")),
            bytes: PNG_FIXTURE.to_vec(),
        }));
    harness.run();
    settled_snapshot(&mut harness, "add_screenshot_dialog");
}

#[test]
fn help_opens_a_dialog_listing_the_shortcuts() {
    let (mut harness, _handles) = harness_with_song(&tone_song());
    act(&mut harness, Action::Ui(UiAction::Help));
    harness.run();
    assert!(harness.state().dialogs.help.is_some(), "the dialog opens");
    // A key from each of the two screens, so the tables really are both there.
    let _ = harness.get_by_label_contains("Alt+Up");
    // Two rows mention the loop start (the key and the mouse tables), which is
    // the point: the same job, listed where each is done.
    assert!(
        harness
            .get_all_by_label_contains("Set the loop start")
            .count()
            >= 2,
        "both the key and the gesture are documented"
    );
}

#[test]
fn snapshot_help_dialog() {
    // Tall: the point of this dialog is that it holds every shortcut at once.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(1000.0, 1500.0));
    let _ = &handles;
    act(&mut harness, Action::Ui(UiAction::Help));
    harness.run();
    settled_snapshot(&mut harness, "help_dialog");
}

#[test]
fn snapshot_bulk_tag_dialog() {
    // The app's own default window: eleven GD3 fields plus a track list is more
    // than 600pt tall, so this is the size at which the box used to run off the
    // bottom of the screen with its Apply button beyond reach.
    let (mut harness, handles) = build_sized(None, false, true, egui::vec2(800.0, 600.0));
    open_folder(&mut harness, &handles, complete_folder());
    pack_section(&mut harness, PackSection::Tracks);
    harness.get_by_label_contains("Bulk Tag").click();
    harness.run();
    settled_snapshot(&mut harness, "bulk_tag_dialog");
}
