//! End-to-end smoke tests for the `drotrim` subcommands.
//!
//! These run the real executable, so they cover the parts a unit test cannot:
//! that the subcommands are wired to the parser at all, that `help` lists them,
//! and that each writes the files it claims to. The song under test is built
//! here rather than taken from `tests/`, small enough to render in milliseconds.
//!
//! `play` is deliberately absent -- it needs an audio device.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use vgms_core::io::write_song;
use vgms_core::{DroDataV1, OplType, Song};

/// The executable under test, built by cargo before the test runs.
const DROTRIM: &str = env!("CARGO_BIN_EXE_drotrim");

/// A unique temp directory, created fresh (the `services::file` tests' pattern).
fn temp_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("drotrim-cli-test-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A short OPL2 song touching channels 0 and 1 and the percussion register, so a
/// split produces a handful of outputs and a render finishes in milliseconds.
fn small_song_bytes() -> Vec<u8> {
    let song = Song::dro_v1(
        "small.dro".to_owned(),
        DroDataV1::new(vec![
            0x20, 0x01, 0xA0, 0x98, 0xB0, 0x31, // channel 0: operator, freq, key on
            0x21, 0x01, 0xA1, 0x98, 0xB1, 0x31, // channel 1
            0xBD, 0x31, // percussion: mode + BD + HH
            0x00, 0x63, // 100 ms
        ])
        .unwrap(),
        100,
        OplType::Opl2,
    );
    write_song(&song).unwrap()
}

fn run(args: &[&str]) -> Output {
    Command::new(DROTRIM)
        .args(args)
        .output()
        .expect("running drotrim")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn help_lists_every_subcommand() {
    let output = run(&["help"]);
    assert!(output.status.success(), "`drotrim help` failed");
    let text = stdout_of(&output);
    for subcommand in ["play", "render", "split"] {
        assert!(
            text.contains(subcommand),
            "`help` omits {subcommand}:\n{text}"
        );
    }
    // Convert is a GUI action now (Edit > Convert to DRO v1), not a subcommand.
    assert!(
        !text.contains("convert"),
        "`convert` should be gone:\n{text}"
    );
}

#[test]
fn version_is_reported() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert!(stdout_of(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn an_unknown_subcommand_is_rejected() {
    // An unknown subcommand (a typo, or the old `convert`) must error, not
    // silently do nothing.
    let output = run(&["convert", "song.dro"]);
    assert!(
        !output.status.success(),
        "an unknown subcommand should fail"
    );
}

#[test]
fn render_appends_a_wav_next_to_the_input() {
    let dir = temp_dir("render");
    let input = dir.join("small.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&["render", input.to_str().unwrap()]);
    assert!(output.status.success(), "render failed: {output:?}");

    // `song.dro` becomes `song.dro.wav`, not `song.wav`.
    let wav = dir.join("small.dro.wav");
    assert!(wav.is_file(), "{:?}", file_names(&dir));
    assert!(std::fs::read(&wav).unwrap().starts_with(b"RIFF"));
}

#[test]
fn split_writes_one_wav_per_used_channel() {
    let dir = temp_dir("split-wav");
    let input = dir.join("small.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&["split", input.to_str().unwrap()]);
    assert!(output.status.success(), "split failed: {output:?}");

    let wavs: Vec<String> = file_names(&dir)
        .into_iter()
        .filter(|name| name.ends_with(".wav"))
        .collect();
    // Channels 0 and 1 and the percussion register were written; the other seven
    // melodic channels were not.
    assert_eq!(wavs.len(), 3, "{wavs:?}");
    assert!(wavs.iter().any(|name| name.contains(".0.01.")), "{wavs:?}");
    assert!(stdout_of(&output).contains("Done -- 3 file(s)."));
}

#[test]
fn split_song_writes_dro_files_for_a_dro_input() {
    let dir = temp_dir("split-song");
    let input = dir.join("small.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&["split", "--song", input.to_str().unwrap()]);
    assert!(output.status.success(), "split --song failed: {output:?}");

    let dros: Vec<String> = file_names(&dir)
        .into_iter()
        .filter(|name| name.ends_with(".out.dro"))
        .collect();
    assert_eq!(dros.len(), 3, "{dros:?}");
}

#[test]
fn split_song_writes_vgm_files_for_a_vgm_input() {
    let dir = temp_dir("split-vgm");
    let input = dir.join("small.vgm");
    // The same music as a VGM. A VGM split captures each channel as a VGM, which
    // is what `--song` means: the input's own format.
    let dro = vgms_core::io::read_song("small.dro", &small_song_bytes()).unwrap();
    let vgm = vgms_core::convert::dro_to_vgm(&dro).unwrap();
    std::fs::write(&input, write_song(&vgm).unwrap()).unwrap();

    let output = run(&["split", "--song", input.to_str().unwrap()]);
    assert!(output.status.success(), "split --song failed: {output:?}");

    let vgms: Vec<String> = file_names(&dir)
        .into_iter()
        .filter(|name| name.ends_with(".out.vgm"))
        .collect();
    assert_eq!(vgms.len(), 3, "{vgms:?}");
    assert!(
        std::fs::read(dir.join(&vgms[0]))
            .unwrap()
            .starts_with(b"Vgm "),
        "{} is not a VGM file",
        vgms[0]
    );
}

#[test]
fn a_file_argument_is_not_mistaken_for_a_subcommand() {
    // `drotrim <file> <subcommand>` is a mistake, and must be reported as one
    // rather than half-parsed.
    let output = run(&["small.dro", "render"]);
    assert!(!output.status.success());
}
