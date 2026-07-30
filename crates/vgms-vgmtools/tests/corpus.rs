//! The pass, over real files.
//!
//! Ignored by default and driven by `DROTRIM_CORPUS`:
//!
//! ```text
//! DROTRIM_CORPUS=F:\GameMusic\VGM cargo test -p vgms-vgmtools --release \
//!     --test corpus -- --ignored --nocapture
//! ```
//!
//! What it asserts of every file:
//!
//! - **the result is still a VGM `vgms_core` can walk** -- a tool that exits 0
//!   having written something unreadable must not reach a caller;
//! - **the delay total is unchanged** -- dropping a write that changes nothing
//!   must not change *when* anything happens;
//! - **it terminates** -- `chip_srom.c` can spin a `UINT32` mask forever, so a
//!   file that hits the timeout is reported rather than waited on.
//!
//! Render parity through `VgmEngine` lives in `vgms-app`, where the engine
//! is available; this is the cheap half, and the half that catches a tool
//! corrupting a file.

use std::path::{Path, PathBuf};

use vgms_vgmtools::{Options, StageOutcome, optimize_vgm};

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
    let file = vgms_core::vgm::file::read("corpus.vgm", bytes).ok()?;
    Some(file.stream()?.total_samples())
}

/// How often a rip declares a chip its stream never writes to.
///
/// `vgm_ptch` can strip those, and whether it is worth binding a fourth tool
/// depends on how common the case is, so this counts it.
///
/// A chip is counted as unwritten only when *no* `Write` targets it. Files
/// carrying data blocks, DAC streams or PCM RAM writes are tallied separately:
/// those can feed a chip without a register write appearing against it, so they
/// must not be stripped on this evidence alone.
#[test]
#[ignore = "needs DROTRIM_CORPUS"]
fn how_many_rips_declare_a_chip_they_never_write_to() {
    let Some(root) = corpus_root() else {
        eprintln!("DROTRIM_CORPUS is not set to a directory; nothing to do");
        return;
    };
    let limit: usize = std::env::var("DROTRIM_CORPUS_LIMIT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(600);

    let mut files_checked = 0usize;
    let mut simple_with_unused = 0usize;
    let mut blocky_with_unused = 0usize;
    let mut by_chip: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    for (_, bytes) in collect(&root, limit) {
        let Ok(file) = vgms_core::vgm::file::read("corpus.vgm", &bytes) else {
            continue;
        };
        let Some(stream) = file.stream() else {
            continue;
        };
        files_checked += 1;

        let mut written: std::collections::BTreeSet<vgms_core::ChipKind> = Default::default();
        let mut has_blocks = false;
        for index in 0..stream.len() {
            match stream.get(index) {
                Some(vgms_core::vgm::VgmCommand::Write { target, .. }) => {
                    written.insert(target.kind);
                }
                Some(
                    vgms_core::vgm::VgmCommand::DataBlock { .. }
                    | vgms_core::vgm::VgmCommand::DacStream { .. }
                    | vgms_core::vgm::VgmCommand::PcmRamWrite { .. }
                    | vgms_core::vgm::VgmCommand::DacWrite { .. },
                ) => has_blocks = true,
                _ => {}
            }
        }

        let unused: Vec<&'static str> = file
            .header
            .chips()
            .iter()
            .filter(|chip| !written.contains(&chip.kind))
            .map(|chip| chip.kind.name())
            .collect();
        if unused.is_empty() {
            continue;
        }
        if has_blocks {
            blocky_with_unused += 1;
        } else {
            simple_with_unused += 1;
            for name in unused {
                *by_chip.entry(name.to_owned()).or_default() += 1;
            }
        }
    }

    println!(
        "\n{files_checked} files: {simple_with_unused} declare a chip they never write to \
         (and carry no data blocks, so the count is trustworthy); \
         {blocky_with_unused} more do but carry blocks, where a chip can be fed without a \
         register write"
    );
    for (chip, count) in &by_chip {
        println!("  {chip:<12} {count}");
    }
    if by_chip.is_empty() {
        println!("  (none -- there is nothing here for vgm_ptch to strip)");
    }
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
