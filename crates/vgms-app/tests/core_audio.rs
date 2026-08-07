//! Do the cores make sound on *real files*, not just on hand-written register
//! scripts? Each core's own tests drive it directly; this proves the other half
//! -- that the engine's routing, the file's commands and the core agree well
//! enough for a corpus rip to come out audible (a core can be perfect and still
//! be handed writes on the wrong port).
//!
//! Files come from the chip index, spread across systems and rippers.
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//!     cargo test -p vgms-app --release --test core_audio -- --ignored --nocapture
//! ```

use std::sync::Arc;

use vgms_app::corpus::{self, ChipIndex};
use vgms_core::vgm::ChipKind;
use vgms_synth::vgm_engine::VgmEngine;

/// Files to draw per chip. Enough that one dud rip does not decide the answer,
/// few enough that the run stays quick.
const SAMPLE: usize = 12;

/// How much of each file to render, in output frames at 44.1 kHz.
///
/// Long enough to get past a lead-in of register setup and silence, short
/// enough that twelve files is seconds rather than minutes.
const FRAMES: usize = 44_100 * 8;

/// Total absolute amplitude below which a render counts as silence.
///
/// Not zero: a YM2612's discrete DAC idles off zero (its ladder effect), so a
/// genuinely silent Mega Drive render still carries a small DC offset. This is
/// far above that and far below any real music.
const SILENCE: i64 = 100_000;

fn energy(samples: &[i16]) -> i64 {
    samples.iter().map(|&s| i64::from(s.abs())).sum()
}

/// Renders the front of `path` and returns its total amplitude.
fn render_head(path: &std::path::Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    let mut engine = VgmEngine::new(Arc::new(file), 44_100);
    let mut total = 0i64;
    let mut buffer = vec![0i16; 4096 * 2];
    let mut frames = 0usize;
    while frames < FRAMES {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        total += energy(&buffer[..rendered * 2]);
        frames += rendered;
    }
    Some(total)
}

/// Every chip with a core must make a sound on real files, not just on the
/// register scripts its own tests write.
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn every_cored_chip_is_audible_on_corpus_files() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!(
            "{} not set (or not a directory); skipping",
            corpus::CORPUS_ENV
        );
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    // Only the chips this build can actually drive through `VgmEngine`. OPL
    // plays through `DroEngine` instead, so it is not this test's business.
    let registry = vgms_synth::registry::registry();
    let cored: Vec<ChipKind> = ChipKind::all()
        .filter(|&chip| registry.can_build(chip))
        .collect();
    assert!(!cored.is_empty(), "no chip has a generic core");

    let mut failures = Vec::new();
    for chip in cored {
        let files = index.sample(chip, SAMPLE);
        if files.is_empty() {
            println!("{:<14} no corpus files", chip.name());
            continue;
        }
        // A rip can legitimately be near-silent at its start, or declare a chip
        // it barely uses, so the bar is "most of them", not "all".
        let mut audible = 0usize;
        let mut checked = 0usize;
        for path in &files {
            let Some(total) = render_head(path) else {
                continue;
            };
            checked += 1;
            if total > SILENCE {
                audible += 1;
            } else {
                println!("  quiet: {}", path.display());
            }
        }
        println!(
            "{:<14} {audible}/{checked} audible of {} indexed",
            chip.name(),
            index.files(chip).len()
        );
        if checked > 0 && audible * 2 <= checked {
            failures.push(format!(
                "{}: only {audible} of {checked} sampled files made a sound",
                chip.name()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A Mega Drive rip has both a PSG and an FM chip. A file where only the PSG
/// sounds would pass the test above while being exactly the regression worth
/// catching.
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn a_mega_drive_rip_plays_its_fm_as_well_as_its_psg() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    // Files naming both chips: that is a Mega Drive rip, whatever folder it is
    // filed under.
    let psg: std::collections::HashSet<_> = index.files(ChipKind::Sn76489).iter().collect();
    let both: Vec<_> = index
        .files(ChipKind::Ym2612)
        .iter()
        .filter(|path| psg.contains(path))
        .collect();
    assert!(
        !both.is_empty(),
        "no file declares both a YM2612 and an SN76489"
    );
    println!("{} files declare both a YM2612 and an SN76489", both.len());

    // Spread across the list rather than the first few, same reasoning as
    // `ChipIndex::sample`.
    let stride = (both.len() / SAMPLE).max(1);
    let mut checked = 0usize;
    let mut fm_audible = 0usize;
    for relative in both.iter().step_by(stride).take(SAMPLE) {
        let path = root.join(relative);
        let Some(with_fm) = render_head(&path) else {
            continue;
        };
        checked += 1;
        // The comparison that makes this test worth having: the same file with
        // the FM core withheld. If the two agree, the FM contributed nothing.
        let Some(psg_only) = render_without_fm(&path) else {
            continue;
        };
        if with_fm > psg_only + SILENCE {
            fm_audible += 1;
        } else {
            println!("  no FM: {} ({with_fm} vs {psg_only})", path.display());
        }
    }
    assert!(checked > 0, "none of the sampled files could be read");
    assert!(
        fm_audible * 2 > checked,
        "only {fm_audible} of {checked} Mega Drive rips gained anything from the FM core"
    );
    println!("{fm_audible}/{checked} rips are louder with the FM core than without");
}

/// The same render with no YM2612 core, so the FM's contribution can be
/// measured rather than assumed.
fn render_without_fm(path: &std::path::Path) -> Option<i64> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    let registry = vgms_synth::registry::registry();
    let mut engine = VgmEngine::with_cores(Arc::new(file), 44_100, move |kind| {
        (kind != ChipKind::Ym2612).then(|| registry.build(kind, None))?
    });
    let mut total = 0i64;
    let mut buffer = vec![0i16; 4096 * 2];
    let mut frames = 0usize;
    while frames < FRAMES {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        total += energy(&buffer[..rendered * 2]);
        frames += rendered;
    }
    Some(total)
}
