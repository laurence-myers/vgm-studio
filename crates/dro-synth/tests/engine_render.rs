//! End-to-end checks: the real `PlayerEngine` driving the real `NukedOpl3` over
//! the real DRO fixture.
//!
//! The unit tests in `engine.rs` prove the stepping, muting and seek logic against
//! a mock chip; `golden_opl.rs` pins the chip against a hash. This ties the two
//! together: the pull engine, rendering the fixture through the actual emulator,
//! must produce exactly what a straightforward reference render loop does -- so
//! its delay accounting and mid-buffer pausing are provably correct on real data.

mod common;

use common::{Op, render_buffered};
use dro_core::io::read_song;
use dro_core::{DroInstruction, Song};
use dro_synth::{
    NATIVE_SAMPLE_RATE, NukedOpl3, OplChip, PlayerEngine, render_wav, render_waveform,
};

const DRO_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");

fn fixture_song() -> Song {
    read_song("lsl3_score_up_dro2.dro", DRO_FIXTURE).expect("the fixture reads")
}

/// Renders a song to the end through the engine, collecting the PCM.
fn engine_pcm(song: &Song, sample_rate: u32, chunk_frames: usize) -> Vec<i16> {
    let mut engine = PlayerEngine::new(song, sample_rate);
    let mut out = vec![0i16; chunk_frames * 2];
    let mut pcm = Vec::new();
    loop {
        let frames = engine.render(&mut out);
        pcm.extend_from_slice(&out[..frames * 2]);
        if frames < chunk_frames {
            break;
        }
    }
    pcm
}

/// The fixture's instructions as the reference loop's ops, so both paths render
/// exactly the same register stream.
fn fixture_ops(song: &Song) -> Vec<Op> {
    song.data()
        .iter()
        .map(|instruction| match instruction {
            DroInstruction::DelayMs { ms, .. } => Op::Delay(ms),
            DroInstruction::Register { reg, value, bank } => Op::Write(
                bank.expect("v2 register writes carry a bank")
                    .register_offset()
                    | u16::from(reg),
                value,
            ),
            DroInstruction::BankSwitch(_) | DroInstruction::DelaySamples { .. } => {
                unreachable!("the DRO v2 fixture has neither bank switches nor sample delays")
            }
        })
        .collect()
}

#[test]
fn the_engine_matches_the_reference_render_loop() {
    let song = fixture_song();
    let ops = fixture_ops(&song);

    // The reference chip is reset to match the engine, which resets on build.
    // Buffered writes: the engine uses the write buffer for live playback, so
    // the reference must too, or their key-edge timing (and thus PCM) diverges.
    let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
    chip.reset(NATIVE_SAMPLE_RATE);
    let reference = render_buffered(&mut chip, NATIVE_SAMPLE_RATE, &ops, 512);

    let engine = engine_pcm(&song, NATIVE_SAMPLE_RATE, 512);
    assert_eq!(
        engine.len(),
        reference.len(),
        "engine rendered a different number of frames"
    );
    assert_eq!(
        engine, reference,
        "the pull engine's PCM diverges from the reference render loop"
    );
}

#[test]
fn engine_output_is_chunk_invariant_on_real_data() {
    let song = fixture_song();
    let reference = engine_pcm(&song, NATIVE_SAMPLE_RATE, 4096);
    for chunk in [1usize, 3, 128, 1000] {
        assert_eq!(
            engine_pcm(&song, NATIVE_SAMPLE_RATE, chunk),
            reference,
            "output changed at chunk size {chunk}"
        );
    }
}

#[test]
fn wav_export_of_the_fixture_is_the_right_length_and_audible() {
    let song = fixture_song();
    let bytes = render_wav(&song, 48_000, 16).unwrap();
    let reader = hound::WavReader::new(std::io::Cursor::new(&bytes)).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 48_000);
    // The fixture header is 2683 ms; at 48 kHz that is 2683 * 48 stereo frames.
    assert_eq!(reader.len(), 2683 * 48 * 2);

    let samples: Vec<i16> = reader.into_samples::<i16>().map(Result::unwrap).collect();
    assert!(
        samples.iter().any(|&s| s != 0),
        "the fixture rendered silent"
    );
}

#[test]
fn waveform_of_the_fixture_is_shaped() {
    let song = fixture_song();
    let buckets = render_waveform(&song, 200, 48_000);
    assert_eq!(buckets.len(), 200);
    let peak = buckets.iter().map(|b| b.max).max().unwrap();
    let trough = buckets.iter().map(|b| b.min).min().unwrap();
    assert!(peak > 1000, "waveform peak {peak} too quiet");
    assert!(trough < -1000, "waveform trough {trough} too quiet");
}
