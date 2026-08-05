//! End-to-end pack export: scan an in-memory folder into a `PackState`, build the
//! release zip, and reopen it -- exercising vgms-core's description generation,
//! vgms-ui's PackState, and vgms-app's zip/oxipng/gzip assembly together.

use std::io::{Cursor, Read as _};

use vgms_app::build_pack_zip;
use vgms_ui::{PackState, PickedFile, PickedFolder};

const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
const PNG_FIXTURE: &[u8] = include_bytes!("../../../tests/screenshot.png");

/// The VGM fixture re-serialised under `name` with a GD3 game name.
fn song(name: &str, game: &str, author: &str) -> PickedFile {
    let mut file = vgms_core::vgm::file::read(name, VGM_FIXTURE).unwrap();
    file.tag = Some(vgms_core::Gd3Tag {
        game_name_en: game.to_owned(),
        track_author_en: author.to_owned(),
        creator: "Ripper".to_owned(),
        ..vgms_core::Gd3Tag::default()
    });
    PickedFile {
        name: name.to_owned(),
        path: Some(std::path::PathBuf::from(format!("C:/Cool Game/{name}"))),
        bytes: vgms_core::vgm::file::write(&file).unwrap(),
    }
}

#[test]
fn scan_build_and_reopen_a_release_zip() {
    let folder = PickedFolder {
        name: "Cool Game".to_owned(),
        path: Some(std::path::PathBuf::from("C:/Cool Game")),
        files: vec![
            song("01 Intro.vgm", "Cool Game", "Ada"),
            song("02 Boss.vgm", "Cool Game", "Bob"),
            PickedFile {
                name: "Cool Game.png".to_owned(),
                path: None,
                bytes: PNG_FIXTURE.to_vec(),
            },
        ],
    };

    let state = PackState::from_folder(folder, Some((2026, 7, 16)));
    assert_eq!(state.meta.game_name, "Cool Game", "prefilled from GD3");
    assert_eq!(state.meta.music_authors, "Ada, Bob");

    let request = state.export_request();
    let output = build_pack_zip(
        &request.entries,
        request.gzip_vgms,
        request.optimize_vgms,
        request.optimizer,
        &|| false,
    )
    .unwrap()
    .expect("the build was not cancelled");

    // Reopen the archive: flat, .vgm gzipped to .vgz, docs and screenshot present.
    let mut archive = zip::ZipArchive::new(Cursor::new(output.bytes)).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_owned())
        .collect();
    for expected in [
        "01 Intro.vgz",
        "02 Boss.vgz",
        "Cool Game.txt",
        "Cool Game.m3u",
        "Cool Game.png",
    ] {
        assert!(
            names.contains(&expected.to_owned()),
            "missing {expected} in {names:?}"
        );
    }
    assert!(
        names.iter().all(|name| !name.contains('/')),
        "entries are flat"
    );

    // The description inside the zip re-parses to the same metadata.
    let mut txt = String::new();
    archive
        .by_name("Cool Game.txt")
        .unwrap()
        .read_to_string(&mut txt)
        .unwrap();
    let reparsed = vgms_core::pack::parse_description(&txt).unwrap();
    assert_eq!(reparsed.game_name, "Cool Game");
    assert!(txt.contains("01 Intro"), "the track list is present");

    // The playlist names the gzipped files.
    let mut m3u = String::new();
    archive
        .by_name("Cool Game.m3u")
        .unwrap()
        .read_to_string(&mut m3u)
        .unwrap();
    assert_eq!(m3u, "01 Intro.vgz\r\n02 Boss.vgz\r\n");
}
