//! The LLE oracle diff: the shipping core measured against the die itself.
//!
//! For the chips with a die-level simulation, the acceptance bar is mechanical
//! -- render the same VGM through the fast core and through the LLE core and
//! correlate, with no reference *player* (or its resampler) in the loop. Both
//! renders happen at the chip's native rate through the same engine, so the diff
//! isolates exactly one variable: the emulation.
//!
//! Not CI: an LLE core runs minutes per corpus file, so it is a documented,
//! `--ignored`, corpus-gated run:
//!
//! ```text
//! DROTRIM_VGMRIPS_CORPUS=<corpus root> \
//!   cargo test -p dro-trimmer --release --test oracle_lle -- \
//!   --nocapture --ignored
//! ```
//!
//! The first chip on the bench is deliberately the strongest: the YM2151 already
//! scores 0.9991 against the reference player, so a high correlation here
//! validates the whole LLE harness before it is pointed at the chips where the
//! answer is not already known.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dro_core::vgm::ChipKind;
use dro_synth::vgm_engine::VgmEngine;
use dro_trimmer::corpus::{self, ChipIndex};
use dro_trimmer::parity::{self, Render};

/// Seconds compared. Shorter than the scorecard's 20: the LLE side pays for
/// every second at die speed, and a correlation this controlled stabilises
/// fast.
const SECONDS: usize = 8;

/// Files per chip. Enough to catch a file-shaped fluke without spending an
/// afternoon of die time.
const FILES: usize = 4;

/// Renders `path` at `rate` through the engine with a caller-chosen core.
fn render_at(
    path: &Path,
    rate: u32,
    cores: impl Fn(ChipKind) -> Option<Box<dyn dro_synth::ChipCore>> + Send + 'static,
) -> Option<Render> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = dro_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    let mut engine = VgmEngine::with_cores(Arc::new(file), rate, cores);
    let wanted = rate as usize * SECONDS * 2;
    let mut samples = Vec::with_capacity(wanted);
    let mut buffer = vec![0i16; 4096 * 2];
    while samples.len() < wanted {
        let rendered = engine.render(&mut buffer);
        if rendered == 0 {
            break;
        }
        samples.extend_from_slice(&buffer[..rendered * 2]);
    }
    Some(Render::from_interleaved_i16(&samples, rate))
}

/// The chip's native rate from the first declared clock in the file.
fn native_rate_of(path: &Path, chip: ChipKind, per_sample: u32) -> Option<u32> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = dro_core::vgm::file::read(&name, &bytes).ok()?;
    let clock = file
        .header
        .chips()
        .iter()
        .find(|c| c.kind == chip)
        .map(|c| c.clock)?;
    Some((clock / per_sample).max(1))
}

/// Files declaring exactly `chip` and nothing else, as the scorecard picks
/// them: strided through the index so the sample spans drivers rather than
/// one game's soundtrack.
fn single_chip_files(index: &ChipIndex, root: &Path, chip: ChipKind, want: usize) -> Vec<PathBuf> {
    let all = index.files(chip);
    let stride = (all.len() / want.max(1)).max(1);
    let order = (0..stride).flat_map(|offset| (offset..all.len()).step_by(stride));
    let mut found = Vec::new();
    for index in order {
        if found.len() >= want {
            break;
        }
        let relative = &all[index];
        let path = root.join(relative);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let name = relative.to_string_lossy().to_string();
        let Ok(file) = dro_core::vgm::file::read(&name, &bytes) else {
            continue;
        };
        if file.header.chips().len() != 1 {
            continue;
        }
        found.push(path);
    }
    found
}

/// One row of the bench: the chip, its master clocks per sample, how to build
/// its LLE oracle, and the median correlation it must clear.
type BenchRow = (ChipKind, u32, fn() -> Box<dyn dro_synth::ChipCore>, f64);

/// The bench: every chip with both a shipping core and an LLE oracle.
#[test]
#[ignore = "corpus-gated die-speed run; see the module doc for the command"]
fn the_shipping_cores_match_the_die() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("DROTRIM_VGMRIPS_CORPUS not set; skipping");
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    // (chip, master clocks per sample, LLE core builder, median bar)
    let bench: [BenchRow; 3] = [
        (
            ChipKind::Ym2151,
            64,
            || Box::new(dro_cores_gpl::Ym2151Lle::new()),
            0.90,
        ),
        // 0.9848 observed (n=4), cents 0.0, no dropouts -- die-accurate. With the
        // die agreeing with Nuked-OPN2 at 0.98, the gap to VGMPlay lives in the
        // reference player's driver, not our emulation.
        (
            ChipKind::Ym2612,
            144,
            || Box::new(dro_cores_gpl::Ym2612Lle::new()),
            0.90,
        ),
        // The 2608 row differs in kind: the die HAS the rhythm mask ROM the
        // clean-room core cannot ship, so a low correlation is not a bug. 0.5829
        // observed (n=4). This package decodes not the OPM's floating-point DAC
        // word but the YM3016's 16-bit linear one, LSB first, with SH1 framing the
        // *left* word -- both pinned by probe against the die's shifter. What
        // remains is believed to be the drums our core cannot play, but the
        // per-file spread is wider than a fixed missing section should give, so a
        // harness gap is not yet ruled out. The bar stays a tripwire under the
        // observed median.
        (
            ChipKind::Ym2608,
            144,
            || Box::new(dro_cores_gpl::Ym2608Lle::new()),
            0.50,
        ),
    ];

    let mut failures = Vec::new();
    for (chip, per_sample, make_lle, bar) in bench {
        let files = single_chip_files(&index, &root, chip, FILES);
        if files.is_empty() {
            eprintln!("{chip:?}: no single-chip corpus files; skipping");
            continue;
        }
        let mut scores = Vec::new();
        for path in &files {
            let Some(rate) = native_rate_of(path, chip, per_sample) else {
                continue;
            };
            let Some(fast) = render_at(path, rate, move |kind| {
                dro_synth::registry::registry().build(kind, None)
            }) else {
                continue;
            };
            let Some(die) = render_at(path, rate, move |kind| (kind == chip).then(make_lle)) else {
                continue;
            };
            let score = parity::compare(&fast, &die, parity::Settings::default());
            let corr = score.worst_correlation();
            println!(
                "{chip:?}  {}  corr {corr:.4}  {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                score.summary(),
            );
            scores.push(corr);
        }
        scores.sort_by(f64::total_cmp);
        let Some(&median) = scores.get(scores.len() / 2) else {
            failures.push(format!("{chip:?}: nothing rendered"));
            continue;
        };
        println!("{chip:?}  median corr {median:.4} (n={})", scores.len());
        // The YM2151 bar, and why it is not 0.99: in lockstep Nuked-OPM and the
        // die correlate 1.0000, so the mechanics agree exactly. What remains in a
        // real stream is the noise LFSR's phase and +-1-sample write-burst jitter;
        // 0.9742 observed (n=4). A real emulation gap looks like the OPN family's
        // 0.6, not this.
        if median < bar {
            failures.push(format!("{chip:?}: median {median:.4} against the die"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
