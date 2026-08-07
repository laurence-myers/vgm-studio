//! Byte-exact regression tests for every way the app renders audio.
//!
//! Renders are compared against WAV files committed under `tests/render/`. The
//! whole path is deterministic (`nuked-opl3` is integer emulation, bit-identical
//! to the C reference; the limiter is plain `f32`), so a byte-for-byte match
//! means every stage -- decoder, frame clock, chip, muting, panpots, limiter, WAV
//! writer -- produced exactly what it did when the fixture was blessed.
//!
//! Re-bless after an intentional change, and check the diff is what you meant:
//!
//! ```PowerShell
//! $env:UPDATE_RENDER_FIXTURES='1'
//! cargo test -p vgms-synth --test render_regression
//! Remove-Item Env:\UPDATE_RENDER_FIXTURES
//! ```
// This file drives an OPL core; a `--no-default-features` build has none by
// design (the only core available is LGPL). See `licenses/README.md`.
#![cfg(feature = "nuked-opl")]

use std::path::PathBuf;

use vgms_core::{Bank, DroDataV1, DroSong, OplType};
use vgms_synth::{Muting, Panning, RenderMix, render_wav, render_wav_mixed};

/// The render every fixture uses: the app's own defaults.
const SAMPLE_RATE: u32 = 48_000;
const BIT_DEPTH: u16 = 16;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/render")
}

/// A 100 ms OPL2 song with two melodic channels at different pitches and two
/// drums.
///
/// The pitches differ so that muting, panning and splitting produce audibly --
/// and bytewise -- distinct results: were both channels identical, a test that
/// muted one could pass while muting the wrong one.
fn regression_song() -> DroSong {
    DroSong::dro_v1(
        "regress.dro".to_owned(),
        DroDataV1::new(vec![
            // Channel 0: operator envelope, then a high note keyed on.
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x77, //
            0xA0, 0x98, 0xB0, 0x31, //
            // Channel 1: the same envelope an octave or so lower.
            0x21, 0x01, 0x41, 0x10, 0x61, 0xF0, 0x81, 0x77, //
            0xA1, 0x40, 0xB1, 0x25, //
            // Percussion mode, bass drum and hi-hat.
            0xBD, 0x31, //
            0x00, 0x63, // 100 ms
        ])
        .unwrap(),
        100,
        OplType::Opl2,
    )
}

/// Compares `actual` against the committed fixture `name`, or writes it when
/// blessing.
///
/// The mismatch report deliberately does not dump the bytes: these are ~19 KB
/// each, and the offset of the first difference is the useful part.
fn assert_matches_fixture(name: &str, actual: &[u8]) {
    let path = fixture_dir().join(name);

    if std::env::var_os("UPDATE_RENDER_FIXTURES").is_some() {
        std::fs::create_dir_all(fixture_dir()).expect("creating tests/render");
        std::fs::write(&path, actual).expect("writing the fixture");
        return;
    }

    let expected = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the fixture {}: {e}\n\
             If this scenario is new, bless it: \
             UPDATE_RENDER_FIXTURES=1 cargo test -p vgms-synth --test render_regression",
            path.display()
        )
    });

    if actual == expected {
        return;
    }
    let first_difference = actual
        .iter()
        .zip(&expected)
        .position(|(a, b)| a != b)
        .map_or_else(
            || "no differing byte -- one is a prefix of the other".to_owned(),
            |at| format!("first differs at byte {at}"),
        );
    panic!(
        "{name} no longer renders as committed: {first_difference} \
         (rendered {} bytes, fixture has {}).\n\
         If the change was intended, re-bless with UPDATE_RENDER_FIXTURES=1.",
        actual.len(),
        expected.len(),
    );
}

#[test]
fn the_full_mix_is_unchanged() {
    let wav = render_wav(&regression_song(), SAMPLE_RATE, BIT_DEPTH).unwrap();
    assert_matches_fixture("full.wav", &wav);
}

#[test]
fn a_muted_channel_is_unchanged() {
    let mut muting = Muting::all();
    muting.mute_channel(Bank::Low, 0xB1); // channel 1 silent, channel 0 and the drums stay
    let wav = render_wav_mixed(
        regression_song(),
        RenderMix {
            muting,
            ..RenderMix::default()
        },
        SAMPLE_RATE,
        BIT_DEPTH,
    )
    .unwrap();
    assert_matches_fixture("muted.wav", &wav);
}

#[test]
fn a_boosted_render_is_unchanged() {
    let wav = render_wav_mixed(
        regression_song(),
        RenderMix {
            boost: 2.0,
            ..RenderMix::default()
        },
        SAMPLE_RATE,
        BIT_DEPTH,
    )
    .unwrap();
    assert_matches_fixture("boosted.wav", &wav);
}

#[test]
fn a_panned_render_is_unchanged() {
    // Channel 0 hard left, channel 1 hard right, the rest centred.
    let mut pans = [0x80u8; 18];
    pans[0] = 0x00;
    pans[1] = 0xFF;
    let wav = render_wav_mixed(
        regression_song(),
        RenderMix {
            panning: Panning::Custom(pans),
            ..RenderMix::default()
        },
        SAMPLE_RATE,
        BIT_DEPTH,
    )
    .unwrap();
    assert_matches_fixture("panned.wav", &wav);
}

/// All three at once -- the GUI's "All of the above".
#[test]
fn a_fully_mixed_render_is_unchanged() {
    let mut muting = Muting::all();
    muting.mute_channel(Bank::Low, 0xB1);
    let mut pans = [0x80u8; 18];
    pans[0] = 0x00;

    let wav = render_wav_mixed(
        regression_song(),
        RenderMix {
            muting,
            panning: Panning::Custom(pans),
            boost: 2.0,
        },
        SAMPLE_RATE,
        BIT_DEPTH,
    )
    .unwrap();
    assert_matches_fixture("combined.wav", &wav);
}

// The OPL-specific channel split (`vgms_synth::split`) was retired in ou-4:
// splitting now runs through the generic `split_vgm_cancellable`, covered by
// `song_split_parity`, `cli_smoke`, and the OPL A/B parity gates. Its exact-byte
// regression (and the `split.0.*` fixtures) went with it -- the render path a
// split stem takes is the whole-song render path, still pinned above.

/// Each scenario must actually differ from the others, or the fixtures above
/// would pass while testing nothing.
#[test]
fn the_scenarios_render_differently_from_one_another() {
    let song = regression_song();
    let mut muting = Muting::all();
    muting.mute_channel(Bank::Low, 0xB1);
    let mut pans = [0x80u8; 18];
    pans[0] = 0x00;

    let full = render_wav(&song, SAMPLE_RATE, BIT_DEPTH).unwrap();
    let variants = [
        (
            "muted",
            RenderMix {
                muting,
                ..RenderMix::default()
            },
        ),
        (
            "boosted",
            RenderMix {
                boost: 2.0,
                ..RenderMix::default()
            },
        ),
        (
            "panned",
            RenderMix {
                panning: Panning::Custom(pans),
                ..RenderMix::default()
            },
        ),
    ];
    for (name, mix) in variants {
        let rendered = render_wav_mixed(&song, mix, SAMPLE_RATE, BIT_DEPTH).unwrap();
        assert_ne!(rendered, full, "{name} rendered the same as the full mix");
    }
}
