//! What the built-in optimiser saves, per chip configuration -- sizes only, no
//! renders, so a corpus-wide sweep is cheap.
//!
//! The number to watch across rule changes: suspending value dedup on the
//! write-paced chips (the owner's inaudible-under-every-core rule, 2026-09)
//! traded compression for correctness, and this is where the trade is priced.
//!
//!   $env:VGMSTUDIO_VGMRIPS_CORPUS = 'F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17'
//!   cargo test -p vgms-app --release --test optimizer_size_sweep -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;

mod common;

#[derive(Default)]
struct Tally {
    files: usize,
    shrank: usize,
    before: u64,
    after: u64,
}

#[test]
#[ignore = "measurement, needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn how_much_the_builtin_saves_per_chip() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);

    let all = common::collect_songs(&root);
    let stride = (all.len() / limit.max(1)).max(1);

    let mut per_chip: BTreeMap<String, Tally> = BTreeMap::new();
    let mut checked = 0usize;
    for path in all.iter().step_by(stride) {
        if checked >= limit {
            break;
        }
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let Ok(mut file) = vgms_core::vgm::file::read("x.vgm", &raw) else {
            continue;
        };
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            continue;
        };
        checked += 1;
        let entry = per_chip.entry(file.chip_list()).or_default();
        entry.files += 1;
        entry.before += plain.len() as u64;
        let shrank = file.optimize().is_some();
        let after = vgms_core::vgm::file::write(&file).map_or(plain.len(), |out| out.len());
        entry.after += after as u64;
        entry.shrank += usize::from(shrank);
    }

    println!("\n-- built-in size sweep ({checked} files) --");
    let (mut total_before, mut total_after, mut total_shrank) = (0u64, 0u64, 0usize);
    for (chips, tally) in &per_chip {
        total_before += tally.before;
        total_after += tally.after;
        total_shrank += tally.shrank;
        let saved = tally.before.saturating_sub(tally.after);
        #[allow(clippy::cast_precision_loss)]
        let pct = 100.0 * saved as f64 / tally.before.max(1) as f64;
        println!(
            "  {chips}: {}/{} shrank, {saved} bytes saved ({pct:.1}%)",
            tally.shrank, tally.files
        );
    }
    let saved = total_before.saturating_sub(total_after);
    #[allow(clippy::cast_precision_loss)]
    let pct = 100.0 * saved as f64 / total_before.max(1) as f64;
    println!("  TOTAL: {total_shrank}/{checked} shrank, {saved} bytes saved ({pct:.1}%)");
}
