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

/// The corpus spot-check: over real files, `optimize_verified` with the real
/// tools must accept every shrink (`optimize_parity` is green on this corpus, so
/// nothing the tools do here changes a render). A file kept back would be a
/// genuine tool regression the parity gate also flags.
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS"]
fn the_verified_path_accepts_what_the_parity_gate_accepts() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(120);

    vgms_app::install_cores();
    let paths = common::collect_songs_capped(&root, limit);
    assert!(!paths.is_empty(), "no VGM files under {}", root.display());

    let mut accepted = 0usize;
    let mut unchanged = 0usize;
    let mut kept_back: Vec<String> = Vec::new();

    for path in paths {
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let Ok(file) = vgms_core::vgm::file::read("corpus.vgm", &raw) else {
            continue;
        };
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            continue;
        };
        let result = optimize_verified(
            &plain,
            Options {
                optimizer: vgms_core::config::OptimizerChoice::Tools,
                ..Options::default()
            },
            &vgms_vgmtools::NativeTools,
            VerifyOptions::default(),
        );
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        match result.outcome {
            VerifiedOutcome::Optimized(_) => accepted += 1,
            VerifiedOutcome::Unchanged => unchanged += 1,
            VerifiedOutcome::KeptOriginal(verdict) => {
                kept_back.push(format!("{name}: {verdict:?}"));
            }
            VerifiedOutcome::Unverifiable(reason) => {
                kept_back.push(format!("{name}: unverifiable ({reason})"));
            }
        }
    }

    println!(
        "verified: {accepted} accepted, {unchanged} unchanged, {} kept back",
        kept_back.len()
    );
    assert!(
        kept_back.is_empty(),
        "the verified path kept files the parity gate accepts:\n{}",
        kept_back.join("\n")
    );
}
