//! End-to-end smoke tests for the `drotrim` subcommands.
//!
//! These run the real executable, so they cover the parts a unit test cannot:
//! that the subcommands are wired to the parser at all, that `help` lists them,
//! and that each writes the files it claims to. The song under test is built
//! here rather than taken from `tests/`: the committed fixture is 99 seconds
//! long, which is fine to convert but far too slow to render.
//!
//! `play` is deliberately absent -- it needs an audio device.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dro_core::io::write_song;
use dro_core::{DroDataV1, OplType, Song};

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
    for subcommand in ["play", "render", "split", "convert"] {
        assert!(
            text.contains(subcommand),
            "`help` omits {subcommand}:\n{text}"
        );
    }
}

#[test]
fn version_is_reported() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert!(stdout_of(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn convert_writes_the_default_output_and_refuses_to_clobber_it() {
    let dir = temp_dir("convert");
    let input = dir.join("song.dro");
    // The committed v2 fixture: `convert` only reads the stream, so its length
    // costs nothing here.
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/lsl3_score_up_dro2.dro"),
        &input,
    )
    .unwrap();

    let output = run(&["convert", input.to_str().unwrap()]);
    assert!(output.status.success(), "convert failed: {output:?}");
    assert!(dir.join("song_1.dro").is_file(), "{:?}", file_names(&dir));

    // A second run must not overwrite what the first wrote.
    let again = run(&["convert", input.to_str().unwrap()]);
    assert!(!again.status.success(), "the second convert should fail");
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already exists"),
        "unexpected error: {}",
        String::from_utf8_lossy(&again.stderr)
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
fn a_file_argument_is_not_mistaken_for_a_subcommand() {
    // `drotrim <file> <subcommand>` is a mistake, and must be reported as one
    // rather than half-parsed.
    let output = run(&["small.dro", "convert"]);
    assert!(!output.status.success());
}
