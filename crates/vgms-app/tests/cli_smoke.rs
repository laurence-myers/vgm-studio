//! End-to-end smoke tests for the `vgmstudio` subcommands.
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
const VGMSTUDIO: &str = env!("CARGO_BIN_EXE_vgmstudio");

/// A unique temp directory, created fresh (the `services::file` tests' pattern).
fn temp_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("vgmstudio-cli-test-{tag}"));
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
            // Two sustained FM notes, each a modulator + carrier with a fast
            // attack (0x60=0xF0) and the sustain-hold bit (EGT, 0x20 bit 5), so
            // the notes actually sound -- the channel split keeps only channels
            // that come out above silence, so a fixture with no envelope would
            // (correctly) split to nothing.
            // Channel 0: modulator (slot 0), then carrier (slot 3).
            0x20, 0x21, 0x40, 0x00, 0x60, 0xF0, 0x80, 0x00, //
            0x23, 0x21, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x00, //
            0xA0, 0x98, 0xB0, 0x31, // freq + key on
            // Channel 1: modulator (slot 1), then carrier (slot 4).
            0x21, 0x21, 0x41, 0x00, 0x61, 0xF0, 0x81, 0x00, //
            0x24, 0x21, 0x44, 0x00, 0x64, 0xF0, 0x84, 0x00, //
            0xA1, 0x98, 0xB1, 0x31, //
            0x00, 0x63, // 100 ms
        ])
        .unwrap(),
        100,
        OplType::Opl2,
    );
    write_song(&song).unwrap()
}

fn run(args: &[&str]) -> Output {
    Command::new(VGMSTUDIO)
        .args(args)
        .output()
        .expect("running vgmstudio")
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
    assert!(output.status.success(), "`vgmstudio help` failed");
    let text = stdout_of(&output);
    for subcommand in ["play", "render", "split", "optimize", "retrowave-probe"] {
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
    // silently do nothing. The file exists, so the *only* reason to fail is that
    // `convert` is not a subcommand -- not that the argument names a missing file.
    let dir = temp_dir("unknown-subcommand");
    let input = dir.join("song.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&["convert", input.to_str().unwrap()]);
    assert!(
        !output.status.success(),
        "an unknown subcommand should fail even with a valid file"
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
    // The two sounding FM channels, named per the chip's roster
    // (`{stem}.{slug}.{index}-{short}.wav`); the other seven never keyed, so the
    // audibility filter drops them.
    assert_eq!(wavs.len(), 2, "{wavs:?}");
    assert!(
        wavs.contains(&"small.ym3812.00-01.wav".to_owned()),
        "{wavs:?}"
    );
    assert!(
        wavs.contains(&"small.ym3812.01-02.wav".to_owned()),
        "{wavs:?}"
    );
    assert!(stdout_of(&output).contains("Done -- 2 file(s)."));
}

#[test]
fn split_song_writes_vgm_files_for_a_dro_input() {
    let dir = temp_dir("split-song");
    let input = dir.join("small.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&["split", "--song", input.to_str().unwrap()]);
    assert!(output.status.success(), "split --song failed: {output:?}");

    // A DRO splits through the generic splitter now (ou-4): its OPL stream
    // projects to a YM3812 VGM, so `--song` rewrites the stream into one VGM per
    // channel of that chip (a rewrite has no render to judge silence by, so every
    // channel of a written chip gets a stem).
    let vgms: Vec<String> = file_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with("small.ym3812."))
        .collect();
    assert_eq!(vgms.len(), 14, "{vgms:?}");
    assert!(
        vgms.contains(&"small.ym3812.00-01.vgm".to_owned()),
        "{vgms:?}"
    );
    assert!(
        std::fs::read(dir.join("small.ym3812.00-01.vgm"))
            .unwrap()
            .starts_with(b"Vgm "),
        "the stem is not a VGM file"
    );
}

#[test]
fn split_song_writes_vgm_files_for_a_vgm_input() {
    let dir = temp_dir("split-vgm");
    let input = dir.join("small.vgm");
    // The same music as a VGM. A VGM split captures each channel as a VGM, which
    // is what `--song` means: the input's own format.
    let dro = vgms_core::io::read_song("small.dro", &small_song_bytes()).unwrap();
    let vgm = vgms_core::convert::dro_to_vgm(&dro).unwrap();
    std::fs::write(&input, vgms_core::vgm::file::write(&vgm).unwrap()).unwrap();

    let output = run(&["split", "--song", input.to_str().unwrap()]);
    assert!(output.status.success(), "split --song failed: {output:?}");

    // One VGM stem per channel of the written YM3812; the input `small.vgm` is
    // filtered out by the roster-named prefix.
    let vgms: Vec<String> = file_names(&dir)
        .into_iter()
        .filter(|name| name.starts_with("small.ym3812."))
        .collect();
    assert_eq!(vgms.len(), 14, "{vgms:?}");
    assert!(
        std::fs::read(dir.join(&vgms[0]))
            .unwrap()
            .starts_with(b"Vgm "),
        "{} is not a VGM file",
        vgms[0]
    );
}

/// Splitting a VGM must keep its own chip clock, not re-synthesise the canonical
/// one: an OPL VGM reaches the CLI as its OPL projection, so it must be split from
/// the file's own bytes (ou-4b review fix), the way the GUI splits it from its
/// cached file.
#[test]
fn split_song_keeps_a_vgms_own_chip_clock() {
    use vgms_core::vgm::ChipKind;

    let dir = temp_dir("split-clock");
    let input = dir.join("odd.vgm");
    // An OPL2 VGM whose YM3812 clock is deliberately non-standard.
    let dro = vgms_core::io::read_song("odd.dro", &small_song_bytes()).unwrap();
    let vgm = vgms_core::convert::dro_to_vgm(&dro).unwrap();
    let mut bytes = vgms_core::vgm::file::write(&vgm).unwrap();
    const ODD_CLOCK: u32 = 4_000_000; // not the canonical YM3812 3_579_545
    let offset = ChipKind::Ym3812.clock_offset();
    bytes[offset..offset + 4].copy_from_slice(&ODD_CLOCK.to_le_bytes());
    std::fs::write(&input, &bytes).unwrap();

    let output = run(&["split", "--song", input.to_str().unwrap()]);
    assert!(output.status.success(), "split --song failed: {output:?}");

    // A stem must declare the file's odd clock; re-projection would canonicalise it.
    let stem = dir.join("odd.ym3812.00-01.vgm");
    let file = vgms_core::vgm::file::read("odd.ym3812.00-01.vgm", &std::fs::read(&stem).unwrap())
        .expect("the stem reads");
    let clock = file
        .header
        .chips()
        .iter()
        .find(|chip| chip.kind == ChipKind::Ym3812)
        .expect("the stem has a YM3812")
        .clock
        & 0x3FFF_FFFF; // mask the dual/variant flag bits
    assert_eq!(
        clock, ODD_CLOCK,
        "the split stem re-synthesised the clock instead of keeping the file's"
    );
}

#[test]
fn a_file_argument_is_not_mistaken_for_a_subcommand() {
    // `vgmstudio <file> <subcommand>` is a mistake, and must be reported as one
    // rather than half-parsed. The file exists, so the failure is the argument
    // order, not a missing file.
    let dir = temp_dir("file-then-subcommand");
    let input = dir.join("small.dro");
    std::fs::write(&input, small_song_bytes()).unwrap();

    let output = run(&[input.to_str().unwrap(), "render"]);
    assert!(!output.status.success());
}
