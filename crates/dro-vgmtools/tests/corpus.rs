//! The pass, over real files.
//!
//! Ignored by default and driven by `DROTRIM_CORPUS`:
//!
//! ```text
//! DROTRIM_CORPUS=F:\GameMusic\VGM cargo test -p dro-vgmtools --release \
//!     --test corpus -- --ignored --nocapture
//! ```
//!
//! What it asserts of every file, whatever the tools decide to do with it:
//!
//! - **the result is still a VGM `dro_core` can walk** -- a tool that exits 0
//!   having written something unreadable must not reach a caller;
//! - **the delay total is unchanged** -- dropping a write that changes nothing
//!   must not change *when* anything happens, and this is the one property
//!   every stage here shares;
//! - **it terminates** -- `chip_srom.c` can spin a `UINT32` mask forever, so a
//!   file that hits the timeout is reported rather than waited on.
//!
//! This is not the whole of ot-7: render parity through `VgmEngine` lives in
//! `dro-trimmer`, which is where the engine is available. What is here is the
//! cheap half, and it is the half that catches a tool corrupting a file.

use std::path::{Path, PathBuf};

use dro_vgmtools::{Options, StageOutcome, optimize_vgm};

fn corpus_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("DROTRIM_CORPUS")?);
    root.is_dir().then_some(root)
}

/// Every `.vgm`/`.vgz` under `root`, uncompressed, capped at `limit`.
fn collect(root: &Path, limit: usize) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= limit {
                return found;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let is_vgm = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("vgm") || extension.eq_ignore_ascii_case("vgz")
                });
            if !is_vgm {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            // A `.vgm` in the wild is often gzip inside; the tools only take
            // plain bytes, exactly as the app hands them over.
            let bytes = if raw.len() >= 2 && raw[0] == 0x1F && raw[1] == 0x8B {
                let mut out = Vec::new();
                if std::io::copy(&mut flate2::read::GzDecoder::new(raw.as_slice()), &mut out)
                    .is_err()
                {
                    continue;
                }
                out
            } else {
                raw
            };
            found.push((path, bytes));
        }
    }
    found
}

fn total_samples(bytes: &[u8]) -> Option<u64> {
    let file = dro_core::vgm::file::read("corpus.vgm", bytes).ok()?;
    Some(file.stream()?.total_samples())
}

#[test]
#[ignore = "needs DROTRIM_CORPUS"]
fn the_pass_never_corrupts_a_file_or_moves_its_timing() {
    let Some(root) = corpus_root() else {
        eprintln!("DROTRIM_CORPUS is not set to a directory; nothing to do");
        return;
    };
    let limit: usize = std::env::var("DROTRIM_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400);

    let files = collect(&root, limit);
    assert!(!files.is_empty(), "no VGM files under {}", root.display());

    let mut checked = 0usize;
    let mut shrank = 0usize;
    let mut before_total = 0usize;
    let mut after_total = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut timeouts: Vec<String> = Vec::new();
    let mut unreadable = 0usize;

    for (path, original) in files {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );

        // Only files this app can read in the first place are in scope: the
        // pass is judged on what it does, not on what it is given.
        let Some(samples_before) = total_samples(&original) else {
            unreadable += 1;
            continue;
        };

        let result = optimize_vgm(&original, Options::default());
        checked += 1;
        before_total += result.original_len;
        after_total += result.bytes.len();

        for stage in &result.stages {
            if let StageOutcome::Failed(reason) = &stage.outcome {
                if reason.contains("did not finish") {
                    timeouts.push(format!("{name}: {}", stage.name));
                } else {
                    failures.push(format!("{name}: {}: {reason}", stage.name));
                }
            }
        }

        let Some(samples_after) = total_samples(&result.bytes) else {
            failures.push(format!("{name}: the optimised file no longer reads"));
            continue;
        };
        assert_eq!(
            samples_after, samples_before,
            "{name}: the delay total moved ({samples_before} -> {samples_after})"
        );

        if result.changed() {
            shrank += 1;
        }
    }

    let saved = before_total.saturating_sub(after_total);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a percentage for a human to read"
    )]
    let percent = if before_total == 0 {
        0.0
    } else {
        100.0 * saved as f64 / before_total as f64
    };
    println!(
        "checked {checked} files ({unreadable} skipped as unreadable); {shrank} shrank; \
         {before_total} -> {after_total} bytes ({percent:.2}% saved)"
    );

    // A timeout is the infinite loop being contained, which is the design
    // working -- but it should be rare enough to name every instance.
    if !timeouts.is_empty() {
        println!(
            "timed out on {} file(s): {}",
            timeouts.len(),
            timeouts.join(", ")
        );
    }
    assert!(
        failures.is_empty(),
        "{} file(s) failed a stage:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
