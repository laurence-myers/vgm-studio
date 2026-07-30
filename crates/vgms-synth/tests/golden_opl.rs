//! Renders a register script decoded from the real DRO fixture, hashes the PCM,
//! and compares against a golden constant. This pins three things: the emulator
//! (Nuked-OPL3 is integer fixed-point, so the hash is stable across targets, and
//! was reproduced by a wasm32 build under Node); `vgms-core`'s v2 decoder (the
//! script decodes through `DroDataV2`); and chunk invariance (128-frame quanta
//! versus a thousands-frame offline render must agree).
//!
//! If the hash changes, find out *why* before updating it. `tests/c_parity.rs`
//! (`--features c-parity`) answers "is the emulator itself still right?".
// This file drives an OPL core; a `--no-default-features` build has none by
// design (the only core available is LGPL). See `licenses/README.md`.
#![cfg(feature = "nuked-opl")]

mod common;

use common::{FIXTURE_MS, TAIL_MS, render, script};
use vgms_synth::{NATIVE_SAMPLE_RATE, NukedOpl3};
use sha2::{Digest, Sha256};

/// SHA-256 of the little-endian PCM produced by rendering `script()` at 49716 Hz.
const GOLDEN_SHA256: &str = "718cb933816049b9021c014bb8df6bd010f320e043325100411c6018c1965f82";

fn sha256(pcm: &[i16]) -> String {
    let mut hasher = Sha256::new();
    for sample in pcm {
        hasher.update(sample.to_le_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn render_script(chunk_frames: usize) -> Vec<i16> {
    let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
    render(&mut chip, NATIVE_SAMPLE_RATE, &script(), chunk_frames)
}

#[test]
fn pcm_matches_the_golden_hash() {
    let pcm = render_script(128);
    let expected_frames = u64::from(FIXTURE_MS + TAIL_MS) * u64::from(NATIVE_SAMPLE_RATE) / 1000;
    assert_eq!(pcm.len() as u64 / 2, expected_frames);
    assert_eq!(sha256(&pcm), GOLDEN_SHA256);
}

#[test]
fn output_is_independent_of_the_pull_size() {
    let reference = render_script(4096);
    for chunk in [1, 2, 3, 127, 128, 1000] {
        let pcm = render_script(chunk);
        assert_eq!(
            pcm.len(),
            reference.len(),
            "frame count differs at chunk={chunk}"
        );
        assert_eq!(pcm, reference, "PCM differs at chunk={chunk}");
    }
}

#[test]
fn the_fixture_renders_audible_sound() {
    let pcm = render_script(512);
    assert!(
        pcm.iter().any(|&sample| sample != 0),
        "rendered PCM is silent"
    );
    let peak = pcm
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap();
    assert!(peak > 1000, "peak amplitude {peak} is suspiciously low");
}
