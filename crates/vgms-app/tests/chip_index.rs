//! Builds the corpus chip index, and reports what the corpus actually holds.
//!
//! It is the index every later core test draws its files from:
//! `ChipIndex::sample(chip, n)` gives N files naming that chip, spread across
//! systems and rippers rather than taken from one pack.
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//!     cargo test -p vgms-app --release --test chip_index -- --ignored --nocapture
//! ```
//!
//! The first run reads every header in the corpus (minutes) and caches beside
//! the corpus, so later runs are immediate.

use vgms_app::corpus::{self, ChipIndex};
use vgms_core::vgm::ChipKind;

#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn the_corpus_index_says_which_chips_are_worth_a_core_first() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!(
            "{} not set (or not a directory); skipping the chip index",
            corpus::CORPUS_ENV
        );
        return;
    };
    let cache = corpus::cache_path(&root);
    let index = ChipIndex::open_or_build(&root, &cache);
    let (scanned, unreadable) = index.scanned();

    let counts = index.by_frequency();
    assert!(
        !counts.is_empty(),
        "no chips found under {} -- is it the corpus root?",
        root.display()
    );

    let total: usize = counts.iter().map(|(_, count)| count).sum();
    println!("chip index: {} chips over {scanned} files", counts.len());
    if unreadable > 0 {
        println!("  {unreadable} files whose header would not read");
    }
    println!("  {total} (chip, file) pairs -- a multi-chip rip counts once per chip");
    println!();
    for (chip, count) in &counts {
        let share = (*count as f64) * 100.0 / (scanned.max(1) as f64);
        println!("  {:<14} {count:>6}  {share:>5.1}%", chip.name());
    }

    // The chips with no core yet, in the order the corpus argues for. Printed,
    // not asserted: evidence for the step order, which is the user's call.
    let registry = vgms_synth::registry::registry();
    let uncored: Vec<_> = counts
        .iter()
        .filter(|(chip, _)| !registry.has_core(*chip))
        .take(10)
        .collect();
    if !uncored.is_empty() {
        println!();
        println!("the ten uncored chips the corpus most wants:");
        for (chip, count) in uncored {
            println!("  {:<14} {count:>6} files", chip.name());
        }
    }

    // The index is only useful if it can hand a core test real files.
    let commonest = counts[0].0;
    let sample = index.sample(commonest, 5);
    assert!(!sample.is_empty(), "{} yielded no files", commonest.name());
    for path in &sample {
        assert!(path.is_file(), "{} is indexed but missing", path.display());
    }

    // And the cache must be usable, or every later run pays the walk again.
    let reread = ChipIndex::load(&cache, &root).expect("the cache we just wrote reads back");
    assert_eq!(
        reread.files(commonest).len(),
        index.files(commonest).len(),
        "the cache disagrees with the walk that wrote it"
    );
}

/// A cheap sanity check that runs without a corpus: an absent one must skip,
/// not panic, since that is how every machine but the maintainer's sees this.
#[test]
fn a_missing_corpus_is_not_an_error() {
    // Whatever the environment says, asking about a chip in an empty index is
    // an empty answer rather than a panic -- which is what the ignored test
    // above relies on when it returns early.
    let empty = ChipIndex::default();
    assert!(empty.files(ChipKind::Ym2612).is_empty());
    assert!(empty.sample(ChipKind::Ym2612, 5).is_empty());
    assert!(empty.by_frequency().is_empty());
    assert_eq!(empty.scanned(), (0, 0));
}
