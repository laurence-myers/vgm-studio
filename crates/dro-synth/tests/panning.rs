//! Real-chip panning: drive `PlayerEngine` with the actual `NukedOpl3` core and
//! assert the `stereo-ext` panpots really steer the output, and that a
//! `Custom` -> `Original` round-trip returns to bit-identical disengaged audio
//! (the same guarantee the `golden_opl` hash pins at the chip level).

use dro_core::{DroDataV1, OplType, Song};
use dro_synth::{NATIVE_SAMPLE_RATE, Panning, PlayerEngine};

/// A sustained OPL2 tone: instruments, key-on, then a long delay and no key-off,
/// so every rendered frame is audible and a pan asymmetry is unambiguous.
fn sustained_tone() -> Song {
    Song::dro_v1(
        "tone.dro".to_owned(),
        DroDataV1::new(vec![
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator
            0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier
            0xA0, 0x98, 0xB0, 0x31, // frequency low, key on (octave 4)
            0x01, 0xE7, 0x03, // long delay ~1000 ms of sustained tone
        ])
        .unwrap(),
        1000,
        OplType::Opl2,
    )
}

fn render(engine: &mut PlayerEngine<&Song>, frames: usize) -> Vec<i16> {
    let mut out = vec![0i16; frames * 2];
    engine.render(&mut out);
    out
}

fn left_channel_has_signal(pcm: &[i16]) -> bool {
    pcm.iter().step_by(2).any(|&s| s != 0)
}

fn right_channel_has_signal(pcm: &[i16]) -> bool {
    pcm.iter().skip(1).step_by(2).any(|&s| s != 0)
}

#[test]
fn hard_left_silences_the_right_channel() {
    let song = sustained_tone();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    engine.set_panning(Panning::Custom([0x00; 18])); // 0x00 = hard left
    let pcm = render(&mut engine, 8192);

    assert!(
        !right_channel_has_signal(&pcm),
        "hard left must silence the right channel (panpot(0) == 0)"
    );
    assert!(
        left_channel_has_signal(&pcm),
        "hard left must keep the left channel audible"
    );
}

#[test]
fn hard_right_silences_the_left_channel() {
    let song = sustained_tone();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    engine.set_panning(Panning::Custom([0xFF; 18])); // 0xFF = hard right
    let pcm = render(&mut engine, 8192);

    assert!(
        !left_channel_has_signal(&pcm),
        "hard right must silence the left channel"
    );
    assert!(
        right_channel_has_signal(&pcm),
        "hard right must keep the right channel audible"
    );
}

#[test]
fn center_pan_feeds_both_channels() {
    let song = sustained_tone();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    engine.set_panning(Panning::Custom([0x80; 18])); // 0x80 = centre
    let pcm = render(&mut engine, 8192);

    assert!(
        left_channel_has_signal(&pcm),
        "centre feeds the left channel"
    );
    assert!(
        right_channel_has_signal(&pcm),
        "centre feeds the right channel"
    );
}

#[test]
fn center_custom_pan_matches_the_disengaged_level() {
    // With the balance pan law, a centred Custom pan holds both speakers at unity,
    // so engaging it must not change the level of any channel -- the render is
    // bit-identical to the disengaged output. (Regression guard: the upstream
    // constant-power law dropped the centre ~3 dB.)
    let song = sustained_tone();
    let mut original = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    let expected = render(&mut original, 8192);

    let mut custom = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    custom.set_panning(Panning::Custom([0x80; 18])); // centre
    let actual = render(&mut custom, 8192);

    assert_eq!(
        actual, expected,
        "a centred Custom pan must not change the level"
    );
}

#[test]
fn custom_then_original_round_trip_is_bit_identical() {
    let song = sustained_tone();

    // A never-touched engine is the reference.
    let mut reference = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    let expected = render(&mut reference, 8192);

    // Engage Custom, render some panned audio, then return to Original and rewind:
    // the disengaged render must match the reference sample for sample.
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE, 0.0);
    engine.set_panning(Panning::Custom([0x12; 18]));
    render(&mut engine, 4096);
    engine.set_panning(Panning::Original);
    engine.rewind();
    let actual = render(&mut engine, 8192);

    assert_eq!(
        actual, expected,
        "Original after a Custom round-trip must match a never-panned engine"
    );
}
