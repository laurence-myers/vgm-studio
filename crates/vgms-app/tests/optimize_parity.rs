//! The audio gate: an optimised file must render to the same samples.
//!
//! Everything else about the optimisers can be checked cheaply -- the file
//! still parses, the delay total is unchanged, the run terminates
//! (`vgms-vgmtools`' own corpus test). None of that can tell whether a write
//! that was dropped mattered. Only rendering both files and comparing samples
//! can, and that is what this does, through the real cores the app ships.
//!
//! It is the gate for both halves of the pass: `vgm_cmp` deciding a write is
//! redundant, and `vgm_sro` deciding a stretch of sample ROM is never reached.
//! A wrong answer in either changes the render, which is exactly what a
//! sample-by-sample comparison sees and a byte count does not.
//!
//! Ignored by default:
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//!     cargo test -p vgms-app --release \
//!     --test optimize_parity -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vgms_synth::vgm_engine::VgmEngine;
use vgms_vgmtools::{Options, optimize_vgm};

mod common;

/// The rate both sides render at. Parity is a comparison against ourselves, so
/// the value only has to be the same on both sides.
const OUTPUT_RATE: u32 = 44_100;

/// How much of each file to render. Long enough to reach past the driver's
/// set-up writes and into the music, short enough to sweep a corpus.
const FRAMES: usize = 44_100 * 8;

/// Renders up to [`FRAMES`] frames of `file` into interleaved stereo.
fn render(file: &Arc<vgms_core::vgm::VgmFile>) -> Vec<i16> {
    let mut engine = VgmEngine::new(Arc::clone(file), OUTPUT_RATE);
    let mut out = vec![0i16; FRAMES * 2];
    let mut done = 0usize;
    while done < FRAMES {
        let rendered = engine.render(&mut out[done * 2..]);
        if rendered == 0 {
            break;
        }
        done += rendered;
    }
    out.truncate(done * 2);
    out
}

/// Where the two renders first disagree, if they do.
fn first_difference(before: &[i16], after: &[i16]) -> Option<usize> {
    if before.len() != after.len() {
        return Some(before.len().min(after.len()));
    }
    before
        .iter()
        .zip(after)
        .position(|(left, right)| left != right)
}

/// Reads `path` and hands back the plain bytes the optimisers take.
fn plain_bytes(path: &Path) -> Option<(Arc<vgms_core::vgm::VgmFile>, Vec<u8>)> {
    let raw = std::fs::read(path).ok()?;
    let file = vgms_core::vgm::file::read("corpus.vgm", &raw).ok()?;
    let plain = vgms_core::vgm::file::write(&file).ok()?;
    Some((Arc::new(file), plain))
}

#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS"]
fn an_optimised_file_renders_to_the_same_samples() {
    // Loud, not a silent skip: run explicitly (`--ignored`), so an unset corpus
    // is a setup mistake to report, not a reason to pass green.
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(120);
    // Stage switches, so a failure can be attributed without editing the test:
    // re-run with one stage off and see whether the difference goes with it.
    let options = Options {
        sample_roms: std::env::var_os("VGMSTUDIO_NO_ROM_TRIM").is_none(),
        dac_runs: std::env::var_os("VGMSTUDIO_NO_DAC_CLEAN").is_none(),
        // Force the tools so this measures them, not the built-in routing.
        optimizer: vgms_core::config::OptimizerChoice::Tools,
    };
    println!(
        "stages: vgm_cmp always; vgm_sro {}; optdac {}",
        if options.sample_roms { "on" } else { "OFF" },
        if options.dac_runs { "on" } else { "OFF" }
    );

    vgms_app::install_cores();

    let paths = common::collect_songs_capped(&root, limit);
    assert!(!paths.is_empty(), "no VGM files under {}", root.display());

    let mut compared = 0usize;
    let mut identical = 0usize;
    let mut skipped_unchanged = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for path in paths {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let Some((original, plain)) = plain_bytes(&path) else {
            continue;
        };

        let result = optimize_vgm(&plain, options);
        if !result.changed() {
            // Nothing was dropped, so there is nothing to be wrong about.
            skipped_unchanged += 1;
            continue;
        }
        let Ok(optimised) = vgms_core::vgm::file::read("corpus.vgm", &result.bytes) else {
            mismatches.push(format!("{name}: the optimised file no longer reads"));
            continue;
        };

        let before = render(&original);
        let after = render(&Arc::new(optimised));
        compared += 1;

        match first_difference(&before, &after) {
            None => identical += 1,
            Some(at) => {
                let stages: Vec<&str> = result
                    .stages
                    .iter()
                    .filter(|stage| {
                        matches!(stage.outcome, vgms_vgmtools::StageOutcome::Shrank { .. })
                    })
                    .map(|stage| stage.name)
                    .collect();
                // The chips matter more than the file: a tool that gets one
                // chip's addressing wrong will get it wrong everywhere, and
                // that is what a hold-back has to be keyed on.
                mismatches.push(format!(
                    "{name}: [{}] renders differ at sample {at} of {} (stages that changed it: {})",
                    original.chip_list(),
                    before.len(),
                    stages.join(", ")
                ));
            }
        }
    }

    println!(
        "rendered {compared} optimised file(s): {identical} identical, \
         {} differing ({skipped_unchanged} unchanged, not compared)",
        mismatches.len()
    );
    assert!(
        mismatches.is_empty(),
        "{} file(s) changed what they play:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

/// Which chips the sample-ROM trim can be trusted with.
///
/// `vgm_sro` replays a file's register writes through cut-down decoders and keeps
/// only the ROM bytes some decoder says are reachable. A decoder that misreads a
/// chip's addressing throws away samples that do get played, and the file still
/// parses, keeps its timing, and sounds wrong -- only a render catches it.
///
/// So the trim is allowed per chip, and this decides which: it runs `vgm_sro`
/// **alone**, renders both sides, and prints a table of chip against
/// identical/differing. A chip in the "differing" column has no business in
/// `vgms_vgmtools`' allowlist. (Upstream calls SegaPCM support "not entirely
/// reliable" and warns YM2610 ADPCM "may need a patch for certain games".)
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS"]
fn which_chips_the_sample_rom_trim_is_safe_for() {
    // Loud, not a silent skip: run explicitly (`--ignored`), so an unset corpus
    // is a setup mistake to report, not a reason to pass green.
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400);

    vgms_app::install_cores();

    let paths = common::collect_songs_capped(&root, limit);

    // chip list -> (identical, differing)
    let mut tally: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();

    for path in paths {
        let Some((original, plain)) = plain_bytes(&path) else {
            continue;
        };
        let vgms_vgmtools::ToolOutcome::Smaller(bytes) = vgms_vgmtools::trim_sample_roms(&plain)
        else {
            continue;
        };
        let Ok(trimmed) = vgms_core::vgm::file::read("corpus.vgm", &bytes) else {
            tally.entry(original.chip_list()).or_default().1 += 1;
            continue;
        };

        let before = render(&original);
        let after = render(&Arc::new(trimmed));
        let entry = tally.entry(original.chip_list()).or_default();
        if first_difference(&before, &after).is_none() {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    println!("\nvgm_sro, alone, by chip:");
    println!("{:<40} {:>9} {:>9}", "chips", "identical", "DIFFER");
    for (chips, (identical, differing)) in &tally {
        println!("{chips:<40} {identical:>9} {differing:>9}");
    }
    if tally.is_empty() {
        println!("  (no file in this corpus was changed by vgm_sro)");
    }
    // Reports evidence rather than enforcing a rule: the allowlist in
    // `vgms_vgmtools::pipeline` is what this run is for.
}

/// The deferred SAA1099 verdict.
///
/// `vgm_cmp.c:537` is missing a `break`, so `case 0xBD` falls through into
/// `case 0x51` and SAA1099 writes are judged by the YM2413's rules. Because that
/// is a fallthrough rather than a considered rule, `vgms_vgmtools` holds those
/// files back. This does not lift the hold-back; it measures what lifting it
/// would cost, by running `vgm_cmp` on an SAA1099 file directly and rendering
/// both sides. Run it over a corpus of SAA1099 rips before changing anything.
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS pointed at SAA1099 rips"]
fn what_holding_the_saa1099_back_is_buying() {
    // Loud, not a silent skip: run explicitly (`--ignored`), so an unset corpus
    // is a setup mistake to report, not a reason to pass green.
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test"),
    );
    vgms_app::install_cores();

    let paths = common::collect_songs_capped(&root, 400);

    let mut seen = 0usize;
    let mut identical = 0usize;
    let mut differing: Vec<String> = Vec::new();

    for path in paths {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let Some((original, plain)) = plain_bytes(&path) else {
            continue;
        };
        if !original
            .header
            .chips()
            .iter()
            .any(|chip| chip.kind == vgms_core::ChipKind::Saa1099)
        {
            continue;
        }
        seen += 1;

        // Straight to the tool, around the hold-back.
        let vgms_vgmtools::ToolOutcome::Smaller(bytes) = vgms_vgmtools::optimize_writes(&plain)
        else {
            continue;
        };
        let Ok(optimised) = vgms_core::vgm::file::read("saa.vgm", &bytes) else {
            differing.push(format!("{name}: the optimised file no longer reads"));
            continue;
        };

        let before = render(&original);
        let after = render(&Arc::new(optimised));
        match first_difference(&before, &after) {
            None => identical += 1,
            Some(at) => differing.push(format!("{name}: differs at sample {at}")),
        }
    }

    println!(
        "SAA1099 rips seen: {seen}; vgm_cmp shrank and rendered identically: {identical}; \
         rendered differently: {}",
        differing.len()
    );
    for line in differing.iter().take(20) {
        println!("  {line}");
    }
    if seen == 0 {
        println!("no SAA1099 rips in this corpus -- the hold-back stays until there are");
    }
    // Deliberately no assertion: this reports evidence for a decision rather
    // than enforcing one. A run where every file renders identically is the
    // argument for lifting `SAA1099_HELD_BACK`.
}
