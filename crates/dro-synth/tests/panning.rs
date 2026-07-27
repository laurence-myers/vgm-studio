//! Real-chip panning: drive `PlayerEngine` with the actual `NukedOpl3` core and
//! assert the `stereo-ext` panpots really steer the output, and that a
//! `Custom` -> `Original` round-trip returns to bit-identical disengaged audio
//! (the same guarantee the `golden_opl` hash pins at the chip level).
// Every test here drives an OPL core and asserts what it sounds like, so the
// whole file needs one. A `--no-default-features` build of this crate has no
// OPL core by design (the only one available is LGPL) -- see
// `licenses/README.md`.
#![cfg(feature = "nuked-opl")]

use dro_core::{DroDataV1, OplType, Song};
use dro_synth::{Muting, NATIVE_SAMPLE_RATE, Panning, PlayerEngine};

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

/// An OPL3 tone whose own `0xC0` routes channel 0 to the **left** speaker only
/// (bit 4 set, bit 5 clear). With stereo-ext disengaged the song plays hard left;
/// a Custom centre pan must override that, and must keep overriding it after a
/// seek. New OPL3 mode is enabled first (`0x105 = 0x01` on the high bank).
fn opl3_left_tone() -> Song {
    Song::dro_v1(
        "opl3left.dro".to_owned(),
        DroDataV1::new(vec![
            0x03, // bank switch high
            0x05, 0x01, // 0x105 = 0x01 (OPL3 new mode)
            0x02, // bank switch low
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator
            0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier
            0xC0, 0x10, // fb/con 0, LEFT speaker only (bit 4)
            0xA0, 0x98, 0xB0, 0x31, // frequency low, key on
            0x01, 0xE7, 0x03, // long delay ~1000 ms of sustained tone
        ])
        .unwrap(),
        1000,
        OplType::Opl3,
    )
}

/// An OPL3 tone that writes its own pan register (`0xD0`) mid-stream, exactly as
/// a real capture (Monkey Island 2's intro) does. On real hardware `0xD0` is an
/// unused no-op, but the stereo-ext chip reads it as a hard-left pan, so the
/// engine must drop it while Custom is engaged. Runs in newm=0 (OPL2-compat),
/// like the real song.
fn song_writing_a_pan_register() -> Song {
    Song::dro_v1(
        "pan_reg.dro".to_owned(),
        DroDataV1::new(vec![
            0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator
            0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier
            0xD0, 0x00, // the song writes the pan register itself (reads as hard left)
            0xA0, 0x98, 0xB0, 0x31, // frequency low, key on
            0x01, 0xE7, 0x03, // long delay ~1000 ms
        ])
        .unwrap(),
        1000,
        OplType::Opl3,
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
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
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
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
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
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
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
    let mut original = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    let expected = render(&mut original, 8192);

    let mut custom = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    custom.set_panning(Panning::Custom([0x80; 18])); // centre
    let actual = render(&mut custom, 8192);

    assert_eq!(
        actual, expected,
        "a centred Custom pan must not change the level"
    );
}

#[test]
fn seek_keeps_custom_panning_engaged() {
    // Repro for the "click the waveform pans hard left" bug: an OPL3 song whose
    // own C0 is hard left, played with Custom centre. Both channels sound before
    // the seek; after a seek they still must, i.e. the seek's replay must keep
    // stereo-ext engaged rather than fall back to the song's left-only C0 image.
    let song = opl3_left_tone();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    engine.set_panning(Panning::Custom([0x80; 18])); // centre overrides the C0 image

    let before = render(&mut engine, 4096);
    assert!(
        right_channel_has_signal(&before),
        "centre pan: right channel audible before the seek"
    );

    engine.seek_to_pos(14); // back to the sustained delay, past the setup writes
    let after = render(&mut engine, 8192);
    assert!(
        right_channel_has_signal(&after),
        "centre Custom pan must survive a seek (right channel still audible)"
    );
}

#[test]
fn a_songs_own_pan_register_writes_do_not_override_custom() {
    // Regression for the Monkey Island 2 "hard left on play" bug: the song writes
    // 0xD0 = 0x00 itself (the stereo-ext chip reads that as hard left), but with
    // Custom centre engaged the engine must drop that write so both channels stay
    // audible.
    let song = song_writing_a_pan_register();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    engine.set_panning(Panning::Custom([0x80; 18])); // centre
    let pcm = render(&mut engine, 8192);
    assert!(left_channel_has_signal(&pcm), "left channel audible");
    assert!(
        right_channel_has_signal(&pcm),
        "a song's own 0xD0 write must not clobber Custom panning (was hard left)"
    );
}

#[test]
fn first_play_after_enabling_custom_keeps_the_pan() {
    // Reproduce "open song, click Custom, click Play". NativeAudioService::play()
    // sends the seek BEFORE the panning, so the engine seeks while still Original
    // (the song's 0x105 disables stereo-ext during the replay); the following
    // SetPanning(Custom) must re-engage it, so playback is centred, not hard left.
    let song = opl3_left_tone();
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    engine.seek_to_pos(14); // play()'s seek: engine.panning still Original here
    engine.set_muting(Muting::all());
    engine.set_panning(Panning::Custom([0x80; 18]));
    let pcm = render(&mut engine, 8192);
    assert!(
        right_channel_has_signal(&pcm),
        "the first play with Custom must be centred, not hard left"
    );
}

#[test]
fn custom_then_original_round_trip_is_bit_identical() {
    let song = sustained_tone();

    // A never-touched engine is the reference.
    let mut reference = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
    let expected = render(&mut reference, 8192);

    // Engage Custom, render some panned audio, then return to Original and rewind:
    // the disengaged render must match the reference sample for sample.
    let mut engine = PlayerEngine::new(&song, NATIVE_SAMPLE_RATE);
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
