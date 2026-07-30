//! Render-parity for the VGM optimiser (`vgms_core::optimize`).
//!
//! Stripping a redundant OPL write and merging the delays must not change a
//! rendered sample. The chip is bit-exact integer emulation and the frame clock
//! carries its remainder exactly, so a byte-for-byte match proves the
//! optimisation inaudible.
//!
//! # Immediate writes, not the buffered playback path
//!
//! These renders apply every register write *immediately*, not through nuked's
//! write buffer (which `render_wav` and live playback use). The buffer spaces
//! queued writes a couple of samples apart, so removing a redundant write shifts
//! the following writes ~2 samples (~40 us) -- inaudible but byte-visible, and a
//! property of the emulator's scheduler, not the optimisation. Immediate writes
//! isolate the latched-state audio the optimiser preserves, giving a byte-exact
//! oracle. Rendering is at the OPL3 native rate, so no resampler is in the path.
// This file drives an OPL core; a `--no-default-features` build has none by
// design (the only core available is LGPL). See `licenses/README.md`.
#![cfg(feature = "nuked-opl")]

use vgms_core::io::read_song;
use vgms_core::optimize::optimize;
use vgms_core::util::VGM_SAMPLE_RATE;
use vgms_core::vgm::io::synthesise_header;
use vgms_core::{Bank, DroInstruction, OplType, Song, VgmData, VgmMeta};
use vgms_synth::{FrameClock, NATIVE_SAMPLE_RATE, NukedOpl3, OplChip};

const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

// -- fixture construction ---------------------------------------------------

/// An OPL2 (YM3812) register write.
fn write(reg: u8, value: u8) -> [u8; 3] {
    [0x5A, reg, value]
}

/// A `0x61` wait of `samples` samples.
fn wait(samples: u16) -> [u8; 3] {
    let [lo, hi] = samples.to_le_bytes();
    [0x61, lo, hi]
}

fn vgm_song(bytes: Vec<u8>, loop_point: Option<usize>, loop_end: Option<usize>) -> Song {
    let mut song = Song::vgm(
        "t.vgm".to_owned(),
        0x151,
        VgmData::new(bytes).unwrap(),
        OplType::Opl2,
        VgmMeta::new(synthesise_header()),
    );
    if loop_point.is_some() {
        let meta = song.vgm_meta_mut().unwrap();
        meta.loop_point = loop_point;
        meta.loop_end = loop_end;
    }
    song
}

fn optimised(song: &Song) -> Song {
    let outcome = optimize(song).expect("the fixture has something to optimise");
    let mut copy = song.clone();
    outcome.install(&mut copy);
    copy
}

/// Renders the given instruction indices, in order, through the chip with
/// immediate register writes. See the module docs for why the write buffer is
/// bypassed.
fn render_indices(song: &Song, indices: &[usize]) -> Vec<i16> {
    let mut chip = NukedOpl3::new(NATIVE_SAMPLE_RATE);
    let mut clock = FrameClock::new(NATIVE_SAMPLE_RATE, VGM_SAMPLE_RATE);
    let mut out = Vec::new();
    let mut scratch = vec![0i16; 8192];
    let mut bank = Bank::Low;
    for &index in indices {
        match song.instruction(index).unwrap() {
            DroInstruction::Register {
                reg,
                value,
                bank: written,
            } => {
                if let Some(written) = written {
                    bank = written;
                }
                chip.write_reg(bank.register_offset() | u16::from(reg), value);
            }
            DroInstruction::DelaySamples { samples, .. } => {
                let mut frames = clock.frames_for(samples);
                while frames > 0 {
                    let n = frames.min((scratch.len() / 2) as u64) as usize;
                    chip.generate_samples(&mut scratch[..n * 2]);
                    out.extend_from_slice(&scratch[..n * 2]);
                    frames -= n as u64;
                }
            }
            DroInstruction::BankSwitch(_) | DroInstruction::DelayMs { .. } => {}
        }
    }
    out
}

/// A one-shot render of the whole song.
fn render(song: &Song) -> Vec<i16> {
    render_indices(song, &(0..song.len()).collect::<Vec<_>>())
}

/// Renders the song with its loop `[start, end)` played `iterations` times, then
/// the tail -- by unrolling the loop into one continuous immediate render, which
/// carries chip and clock state across the seam exactly as looped playback does
/// (the engine does not reset the chip at the seam).
fn render_looped(song: &Song, start: usize, end: usize, iterations: u32) -> Vec<i16> {
    let mut indices: Vec<usize> = (0..end).collect(); // prefix + first iteration
    for _ in 1..iterations {
        indices.extend(start..end);
    }
    indices.extend(end..song.len()); // the tail, played once after the loop
    render_indices(song, &indices)
}

/// A melodic OPL2 channel set up and keyed, with a handful of redundant writes a
/// DOSBox capture typically leaves behind: an unchanged operator level, a key-on
/// rewritten with no edge, and repeated rhythm-mode writes.
fn redundant_fixture() -> Vec<u8> {
    [
        write(0x20, 0x01),
        write(0x40, 0x10),
        write(0x60, 0xF0),
        write(0x80, 0x77),
        write(0x23, 0x01),
        write(0x43, 0x00),
        write(0x63, 0xF0),
        write(0x83, 0x77),
        write(0xC0, 0x01),
        write(0xA0, 0x98),
        write(0xB0, 0x31), // key on
        wait(2205),        // 50 ms
        write(0x40, 0x10), // redundant: same operator level
        write(0xB0, 0x31), // redundant: key already on, no edge
        wait(2205),
        write(0xB0, 0x11), // key off (a real change)
        wait(1102),
        write(0xBD, 0x20), // rhythm mode enable
        write(0xBD, 0x20), // redundant
        write(0xBD, 0x30), // bass drum on (a real change)
        wait(2205),
        write(0xBD, 0x20), // bass drum off
        wait(2205),
    ]
    .concat()
}

// -- linear render parity ---------------------------------------------------

#[test]
fn optimising_a_bloated_real_capture_matches_the_clean_render() {
    // The committed capture is already optimal -- a good property in itself.
    let clean = read_song("lsl3.vgm", VGM_FIXTURE).unwrap();
    assert!(
        optimize(&clean).is_none(),
        "the committed capture should already be optimal"
    );

    // Duplicate every register write: 238 inaudible rewrites the optimiser must
    // strip. Real capture data, made redundant in a way real captures often are.
    let mut bloated_bytes = Vec::new();
    for index in 0..clean.len() {
        let raw = clean.data().raw_instruction(index).unwrap();
        bloated_bytes.extend_from_slice(raw);
        if matches!(
            clean.instruction(index),
            Some(DroInstruction::Register { .. })
        ) {
            bloated_bytes.extend_from_slice(raw); // the redundant repeat
        }
    }
    let bloated = vgm_song(bloated_bytes, None, None);
    assert!(bloated.len() > clean.len(), "the bloat added no writes");

    // The optimiser strips the redundancy straight back to the clean stream, byte
    // for byte -- so it renders identically to the committed capture.
    let opt = optimised(&bloated);
    assert_eq!(
        opt.data().raw(),
        clean.data().raw(),
        "the optimised bloat is not the clean capture"
    );
    assert_eq!(render(&opt), render(&clean));
}

#[test]
fn optimising_a_percussion_fixture_does_not_change_the_render() {
    let song = vgm_song(redundant_fixture(), None, None);
    let opt = optimised(&song);
    // It genuinely stripped writes (not a vacuous comparison).
    assert!(opt.len() < song.len(), "nothing was stripped");
    assert_eq!(
        render(&song),
        render(&opt),
        "stripping same-value writes changed the render"
    );
}

// -- looped render parity (the seam) ----------------------------------------

/// The loop-safety trap: a register is set to a value before the loop, re-set to
/// that same value at the loop point, then changed inside the loop body. A naive
/// optimiser would drop the loop-point write as "same as before the loop", and the
/// second iteration would start from the wrong value. The reset at the loop point
/// keeps it, so the loop is stable across the seam.
///
/// Returns `(bytes, loop_point, loop_end)` as instruction indices.
fn loop_trap_fixture() -> (Vec<u8>, usize, usize) {
    let mut commands: Vec<[u8; 3]> = vec![
        write(0x20, 0x01),
        write(0x40, 0x3F), // the load-bearing operator level
        write(0x60, 0xF0),
        write(0x80, 0x77),
        write(0x23, 0x01),
        write(0x43, 0x00),
        write(0x63, 0xF0),
        write(0x83, 0x77),
        write(0xC0, 0x01),
        write(0xA0, 0x98),
    ];
    let loop_point = commands.len();
    commands.extend_from_slice(&[
        write(0x40, 0x3F), // same value: kept by the loop-point reset
        write(0xB0, 0x31), // key on
        wait(2205),
        write(0x40, 0x10), // change the level inside the loop
        write(0xB0, 0x31), // redundant key-on (dropped)
        wait(2205),
        write(0xB0, 0x11), // key off
        wait(1102),
    ]);
    let loop_end = commands.len();
    (commands.concat(), loop_point, loop_end)
}

#[test]
fn optimising_does_not_change_a_looped_render_across_the_seam() {
    let (bytes, loop_point, loop_end) = loop_trap_fixture();
    let song = vgm_song(bytes, Some(loop_point), Some(loop_end));

    let opt = optimised(&song);
    // The optimiser kept the loop-point write, so the loop markers still bound the
    // same music; the optimised markers moved only by the writes stripped before
    // them. A loop that runs to the end carries `loop_end == None`.
    let opt_meta = opt.vgm_meta().unwrap();
    let opt_start = opt_meta.loop_point.unwrap();
    let opt_end = opt_meta.loop_end.unwrap_or(opt.len());

    let original = render_looped(&song, loop_point, loop_end, 3);
    let optimised_render = render_looped(&opt, opt_start, opt_end, 3);
    assert_eq!(
        original, optimised_render,
        "the optimised loop diverged from the original across the seam"
    );
}

/// Proves the trap has teeth: an over-eager strip that *drops* the loop-point
/// write (what a version without the loop-point reset would do) audibly diverges,
/// so the parity test above is meaningful rather than vacuous.
#[test]
fn dropping_the_loop_point_write_would_be_audible() {
    let (bytes, loop_point, loop_end) = loop_trap_fixture();
    let song = vgm_song(bytes, Some(loop_point), Some(loop_end));

    // Hand-build the "naively over-stripped" stream: the same commands with the
    // loop-point operator-level write removed, and the markers slid by one.
    let mut broken = Vec::new();
    for index in 0..song.len() {
        if index != loop_point {
            broken.extend_from_slice(song.data().raw_instruction(index).unwrap());
        }
    }
    let broken_song = vgm_song(broken, Some(loop_point), Some(loop_end - 1));

    let original = render_looped(&song, loop_point, loop_end, 3);
    let broken = render_looped(&broken_song, loop_point, loop_end - 1, 3);
    assert_ne!(
        original, broken,
        "the loop-point write must be audible across the seam, or the parity test proves nothing"
    );
}
