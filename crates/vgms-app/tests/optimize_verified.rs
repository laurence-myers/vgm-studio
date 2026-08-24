//! The per-track optimise gate, end to end through the real cores.
//!
//! `vgms_ui::optimize::optimize_verified` runs the pipeline and then renders the
//! original and the optimised file through the real engine, keeping the smaller
//! file only when the samples match (D-orw-1/D-orw-4). This exercises that with
//! a controllable [`Tools`] over a tiny SN76489 file the app has a core for, so
//! the gate's two verdicts -- accept an equivalent shrink, reject an audible one
//! -- are both provoked without needing the corpus.
//!
//! The corpus spot-check (`optimize_verified` accepts what `optimize_parity`
//! accepts) is the `#[ignore]` test at the bottom, driven by
//! `VGMSTUDIO_VGMRIPS_CORPUS` like the other corpus suites.

use std::path::PathBuf;

use vgms_core::vgm::ChipKind;
use vgms_core::vgm::stream::{ChipTarget, VgmCommand};
use vgms_synth::VerifyOptions;
use vgms_ui::optimize::{VerifiedOutcome, optimize_verified};
use vgms_vgmtools::{Options, ToolOutcome, Tools};

mod common;

/// A minimal walkable VGM: an SN76489 tone keyed on, held across two waits, with
/// a redundant second key-on so an optimiser has something safe to drop.
///
/// - `0x50 0x84`, `0x50 0x20`: latch channel 0's tone period (some non-zero
///   frequency, so the loud channel actually sounds).
/// - `0x50 0x90`: channel 0 attenuation 0 -- full volume. This is the *live*
///   key-on; drop it and the channel stays silent.
/// - a wait, then a **redundant** `0x50 0x90` (already loud), then a wait.
///   Dropping this one changes nothing.
fn sn76489_tone() -> Vec<u8> {
    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn wait(samples: u16) -> [u8; 3] {
        [0x61, samples as u8, (samples >> 8) as u8]
    }

    const DATA_START: usize = 0x100;
    let mut bytes = vec![0u8; DATA_START];
    bytes[..4].copy_from_slice(b"Vgm ");
    put_u32(&mut bytes, 0x08, 0x171);
    put_u32(&mut bytes, 0x34, (DATA_START - 0x34) as u32);
    put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x50, 0x84]); // ch0 tone period, low
    body.extend_from_slice(&[0x50, 0x20]); // ch0 tone period, high
    body.extend_from_slice(&[0x50, 0x90]); // ch0 attenuation 0 (loud) -- LIVE
    body.extend_from_slice(&wait(10_000));
    body.extend_from_slice(&[0x50, 0x90]); // same again -- REDUNDANT
    body.extend_from_slice(&wait(10_000));
    body.push(0x66); // end of sound data

    bytes.extend_from_slice(&body);
    let eof = bytes.len();
    put_u32(&mut bytes, 0x04, (eof - 4) as u32);
    bytes
}

/// The command indices of channel-0 attenuation-0 writes (`0x50 0x90`), in
/// order. There are two in [`sn76489_tone`]: the live one and the redundant one.
fn loud_writes(bytes: &[u8]) -> Vec<usize> {
    let file = vgms_core::vgm::file::read("tone.vgm", bytes).expect("readable");
    let stream = file.stream().expect("walkable");
    (0..stream.len())
        .filter(|&index| {
            matches!(
                stream.get(index),
                Some(VgmCommand::Write {
                    target: ChipTarget {
                        kind: ChipKind::Sn76489,
                        instance: 0,
                        ..
                    },
                    addr: 0,
                    data: 0x90,
                })
            )
        })
        .collect()
}

/// The file with the `nth` loud write removed, header repatched -- what a real
/// optimiser would emit, so the result is smaller and still reads.
fn drop_loud_write(bytes: &[u8], nth: usize) -> Vec<u8> {
    let mut file = vgms_core::vgm::file::read("tone.vgm", bytes).expect("readable");
    let index = loud_writes(bytes)[nth];
    assert!(file.delete_commands(&[index]), "a command must be removed");
    vgms_core::vgm::file::write(&file).expect("writable")
}

/// A `Tools` whose `vgm_cmp` step drops a chosen loud write; the other two
/// stages do nothing. Which write it drops decides whether the shrink is safe.
struct DropWrite {
    /// The loud write to drop: `1` is the redundant one (safe), `0` the live
    /// one (silences the channel).
    nth: usize,
}

impl Tools for DropWrite {
    fn optimize_writes(&self, vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Smaller(drop_loud_write(vgm, self.nth))
    }
    fn trim_sample_roms(&self, _vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Unchanged
    }
    fn clean_dac_runs(&self, _vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Unchanged
    }
}

/// A `Tools` that never changes anything -- the pass finds nothing to gain.
struct DoNothing;

impl Tools for DoNothing {
    fn optimize_writes(&self, _vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Unchanged
    }
    fn trim_sample_roms(&self, _vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Unchanged
    }
    fn clean_dac_runs(&self, _vgm: &[u8]) -> ToolOutcome {
        ToolOutcome::Unchanged
    }
}

/// Options that force the external tools and skip the ROM/DAC stages, so the
/// mock's `optimize_writes` is the only thing that changes the file.
fn tools_only() -> Options {
    Options {
        sample_roms: false,
        dac_runs: false,
        optimizer: vgms_core::config::OptimizerChoice::Tools,
        ..Default::default()
    }
}

#[test]
fn an_equivalent_shrink_is_accepted() {
    vgms_app::install_cores();
    let bytes = sn76489_tone();
    // Drop the redundant key-on: smaller, and the channel is loud either way.
    let result = optimize_verified(
        &bytes,
        tools_only(),
        &DropWrite { nth: 1 },
        VerifyOptions::default(),
    );
    match &result.outcome {
        VerifiedOutcome::Optimized(optimized) => {
            assert!(
                optimized.len() < bytes.len(),
                "the accepted file is smaller"
            );
            assert_eq!(
                result.accepted_bytes().map(<[u8]>::len),
                Some(optimized.len())
            );
            assert!(result.saved() > 0);
        }
        other => panic!("an equivalent shrink must be accepted, got {other:?}"),
    }
}

#[test]
fn an_audible_change_keeps_the_original() {
    vgms_app::install_cores();
    let bytes = sn76489_tone();
    // Drop the live key-on: smaller, but the channel now never sounds.
    let result = optimize_verified(
        &bytes,
        tools_only(),
        &DropWrite { nth: 0 },
        VerifyOptions::default(),
    );
    assert!(
        matches!(result.outcome, VerifiedOutcome::KeptOriginal(_)),
        "a render-changing shrink must keep the original, got {:?}",
        result.outcome
    );
    assert_eq!(result.accepted_bytes(), None, "nothing is offered to write");
    assert_eq!(result.saved(), 0);
}

#[test]
fn nothing_to_gain_reports_unchanged_without_rendering() {
    // No cores needed: the pass shrinks nothing, so the gate never renders.
    let bytes = sn76489_tone();
    let result = optimize_verified(&bytes, tools_only(), &DoNothing, VerifyOptions::default());
    assert!(
        matches!(result.outcome, VerifiedOutcome::Unchanged),
        "an unchanged pass reports unchanged, got {:?}",
        result.outcome
    );
    assert_eq!(result.accepted_bytes(), None);
}

/// The chips `vgm_sro` / `vgm_cmp` are otherwise held back on -- the ones the
/// speculative path tries and the gate then keeps or rejects (D-orw-8).
fn has_held_back_chip(file: &vgms_core::vgm::VgmFile) -> bool {
    use vgms_core::ChipKind::{K053260, QSound, Saa1099, SegaPcm};
    file.header
        .chips()
        .iter()
        .any(|chip| matches!(chip.kind, QSound | K053260 | SegaPcm | Saa1099))
}

fn declares(file: &vgms_core::vgm::VgmFile, kind: ChipKind) -> bool {
    file.header.chips().iter().any(|chip| chip.kind == kind)
}

/// The Stage 4 measurement (D-orw-8): run the verified path -- which is
/// speculative, so it *attempts* the held-back stages -- over the corpus and
/// tally what the render gate then does. Prints the numbers the plan's addendum
/// wants: how many previously-denied files the gate recovers (s4-1), and how
/// many corruptions it catches (s4-2, chiefly the vgm_cmp first-pass YM2612 bug).
///
/// The one hard invariant: a file with none of the risky stages (no held-back
/// chip, no YM2612) must never be kept back -- the tools are trusted there, so a
/// rejection would be a real regression. Everything else is measurement, not a
/// pass/fail: a kept-back QSound or YM2612 file is the gate working.
///
/// ```text
/// VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
///     cargo test -p vgms-app --release --test optimize_verified \
///     -- --ignored --nocapture the_verified_path
/// ```
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS"]
fn the_verified_path_recovers_holdbacks_and_catches_corruptions() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200);

    vgms_app::install_cores();
    let paths = common::collect_songs_capped(&root, limit);
    assert!(!paths.is_empty(), "no VGM files under {}", root.display());

    let tools = Options {
        optimizer: vgms_core::config::OptimizerChoice::Tools,
        ..Options::default()
    };

    let (mut accepted, mut unchanged) = (0usize, 0usize);
    // s4-1: held-back-chip files the gate recovered (accepted a shrink the
    // non-speculative path denied) vs kept back (the trim really did corrupt).
    let (mut holdback_recovered, mut holdback_kept) = (0usize, 0usize);
    // s4-2: YM2612 files kept back -- the vgm_cmp first-pass corruption, caught.
    let mut ym2612_kept = 0usize;
    // The invariant breach: a "safe" file (no risky stage) kept back.
    let mut safe_kept: Vec<String> = Vec::new();

    for path in &paths {
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let Ok(file) = vgms_core::vgm::file::read("corpus.vgm", &raw) else {
            continue;
        };
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            continue;
        };
        let held_back = has_held_back_chip(&file);
        let has_ym2612 = declares(&file, ChipKind::Ym2612);
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );

        let result = optimize_verified(
            &plain,
            tools,
            &vgms_vgmtools::NativeTools,
            VerifyOptions::default(),
        );
        match result.outcome {
            VerifiedOutcome::Optimized(bytes) => {
                accepted += 1;
                if held_back {
                    // Recovered iff the held-back stage is what shrank it: the
                    // non-speculative pass (hold-backs on) leaves those bytes.
                    let denied = vgms_vgmtools::optimize_vgm(&plain, tools);
                    if bytes.len() < denied.bytes.len() {
                        holdback_recovered += 1;
                    }
                }
            }
            VerifiedOutcome::Unchanged => unchanged += 1,
            VerifiedOutcome::KeptOriginal(_) | VerifiedOutcome::Unverifiable(_) => {
                if held_back {
                    holdback_kept += 1;
                } else if has_ym2612 {
                    ym2612_kept += 1;
                } else {
                    safe_kept.push(name);
                }
            }
        }
    }

    println!("\nStage 4 measurement over {} file(s):", paths.len());
    println!("  accepted (verified shrink): {accepted}");
    println!("  unchanged (nothing to gain): {unchanged}");
    println!("  s4-1 held-back chips: {holdback_recovered} recovered, {holdback_kept} kept back");
    println!("  s4-2 YM2612 corruptions caught: {ym2612_kept}");
    println!("  safe files kept back (must be 0): {}", safe_kept.len());

    assert!(
        safe_kept.is_empty(),
        "the verified path kept back files with no risky stage:\n{}",
        safe_kept.join("\n")
    );
}
