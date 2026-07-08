//! Stage 1 of the OPL3 spike, graduated into permanent CI.
//!
//! Drives the emulator with a register script decoded from the real DRO fixture,
//! hashes the resulting PCM, and compares it against a golden constant. Three
//! things are pinned by this:
//!
//! 1. **The emulator.** Nuked-OPL3 is integer fixed-point, so the hash is stable
//!    across targets. This exact hash was reproduced by a `wasm32-unknown-unknown`
//!    build running under Node, which is what makes native/web audio parity a
//!    tested property rather than a hope.
//! 2. **`dro-core`'s v2 decoder.** The script is decoded through `DroDataV2`, so a
//!    regression in codemap handling, bank extraction or delay arithmetic moves
//!    the hash.
//! 3. **Chunk invariance.** An `AudioWorklet` pulls 128 frames per quantum; an
//!    offline render pulls thousands. They must agree.
//!
//! If the hash changes, find out *why* before updating it. `tests/c_parity.rs`
//! (`--features c-parity`) answers "is the emulator itself still right?".

mod common;

use common::{FIXTURE_MS, TAIL_MS, render, script};
use dro_synth::{NATIVE_SAMPLE_RATE, NukedOpl3};
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
