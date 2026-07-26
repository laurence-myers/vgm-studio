//! Corpus validation for the OPL projection (uv-1), run on demand.
//!
//! The net under the whole unification. Every OPL feature is being moved off
//! the old closed-table reader and onto the one VGM model, and the promise is
//! that nothing about an OPL song changes. This checks that promise against
//! real files rather than synthetic ones: for every VGM in the local corpus
//! that the OPL reader accepts, the projected song must agree row for row,
//! total for total, and byte for byte on the way out.
//!
//! Ignored by default: it needs the local corpus, whose root is passed via the
//! `DROTRIM_CORPUS` environment variable. Run it with:
//!
//! ```powershell
//! $env:DROTRIM_CORPUS = 'F:\GameMusic\VGM'
//! cargo test -p dro-trimmer --release --test projection_corpus -- --ignored --nocapture
//! ```
//!
//! It also reports the files the *old* reader rejected but the VGM model opens
//! -- the packs this whole feature exists for -- broken down by why.

use std::path::{Path, PathBuf};

fn collect_songs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_songs(&path, out);
        } else if matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("vgm") | Some("vgz")
        ) {
            out.push(path);
        }
    }
}

#[test]
#[ignore = "needs the local corpus via DROTRIM_CORPUS"]
fn the_projection_matches_the_opl_reader_across_the_corpus() {
    let Ok(root) = std::env::var("DROTRIM_CORPUS") else {
        eprintln!("DROTRIM_CORPUS not set; skipping corpus validation");
        return;
    };
    let mut songs = Vec::new();
    collect_songs(Path::new(&root), &mut songs);
    songs.sort();
    assert!(!songs.is_empty(), "no .vgm/.vgz files under {root}");

    let mut scanned = 0usize;
    let mut opl = 0usize;
    let mut agreed = 0usize;
    let mut newly_openable = 0usize;
    let mut unreadable_by_both = 0usize;
    let mut with_blocks = 0usize;
    let mut chips_seen: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &songs {
        scanned += 1;
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let by_opl_reader = dro_core::vgm::io::read(&name, &bytes);
        let by_model = dro_core::vgm::file::read(&name, &bytes);

        match (by_opl_reader, by_model) {
            (Ok(expected), Ok(file)) => {
                opl += 1;
                let Some(projected) = file.to_song() else {
                    failures.push(format!(
                        "{name}: the OPL reader accepted it, the model did not"
                    ));
                    continue;
                };
                let checked = compare(&expected, &projected)
                    .and_then(|()| compare_optimised(&name, &bytes, &expected));
                if let Err(why) = checked {
                    failures.push(format!("{name}: {why}"));
                } else {
                    agreed += 1;
                }
                if file
                    .stream()
                    .is_some_and(|stream| !dro_core::vgm::projection::is_wholly_opl(stream))
                {
                    with_blocks += 1;
                }
            }
            (Err(_), Ok(file)) => {
                // The files this feature exists for: openable now, not before.
                newly_openable += 1;
                *chips_seen.entry(file.chip_list()).or_default() += 1;
            }
            (Err(_), Err(_)) => unreadable_by_both += 1,
            (Ok(_), Err(why)) => {
                failures.push(format!("{name}: the model rejected an OPL file: {why}"));
            }
        }
    }

    eprintln!("--- projection corpus ---");
    eprintln!("scanned:            {scanned}");
    eprintln!("OPL (both readers): {opl}, agreed: {agreed}");
    eprintln!("  of those, carrying non-OPL commands: {with_blocks}");
    eprintln!("newly openable:     {newly_openable}");
    for (chips, count) in &chips_seen {
        eprintln!("    {count:>5}  {chips}");
    }
    eprintln!("unreadable by both: {unreadable_by_both}");

    assert!(
        failures.is_empty(),
        "{} file(s) disagreed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every way the two paths could differ, in the order that localises a fault
/// fastest: shape first, then rows, then derived totals, then the bytes.
fn compare(expected: &dro_core::Song, projected: &dro_core::Song) -> Result<(), String> {
    if projected.opl_type != expected.opl_type {
        return Err(format!(
            "chip {:?} != {:?}",
            projected.opl_type, expected.opl_type
        ));
    }
    if projected.len() != expected.len() {
        return Err(format!("{} rows != {}", projected.len(), expected.len()));
    }
    for index in 0..expected.len() {
        if projected.instruction(index) != expected.instruction(index) {
            return Err(format!(
                "row {index}: {:?} != {:?}",
                projected.instruction(index),
                expected.instruction(index)
            ));
        }
    }
    if projected.total_delay_samples() != expected.total_delay_samples() {
        return Err(format!(
            "{} samples != {}",
            projected.total_delay_samples(),
            expected.total_delay_samples()
        ));
    }
    if projected.vgm_meta() != expected.vgm_meta() {
        return Err("metadata differs".to_owned());
    }
    let written = dro_core::vgm::io::write(projected).map_err(|e| e.to_string())?;
    let reference = dro_core::vgm::io::write(expected).map_err(|e| e.to_string())?;
    if written != reference {
        return Err(format!(
            "written bytes differ ({} vs {})",
            written.len(),
            reference.len()
        ));
    }
    Ok(())
}

/// The chip-agnostic optimiser against the OPL one it will replace, on the same
/// file: both halves -- the redundant-write strip and the byte-minimal delay
/// merge -- must produce the same bytes, or every optimised file in a pack
/// would change how it is spelled.
fn compare_optimised(name: &str, bytes: &[u8], expected: &dro_core::Song) -> Result<(), String> {
    let mut file = dro_core::vgm::file::read(name, bytes).map_err(|e| e.to_string())?;
    file.optimize();
    // The *stream*, not the whole file: the two writers legitimately differ in
    // the header (the OPL one re-derives the chip clocks from the song's type,
    // which is the canonicalisation `vgm::file::write` deliberately avoids), so
    // comparing files would be comparing writers rather than optimisers.
    let ours = file.body.raw();
    let ours = &ours[..ours.len().saturating_sub(1)]; // drop the end marker

    let theirs = match dro_core::optimize::optimize(expected) {
        Some(outcome) => outcome.data.raw().to_vec(),
        // Nothing to strip or merge: the stream must come back untouched.
        None => expected.data().raw().to_vec(),
    };
    if ours != theirs.as_slice() {
        let at = ours
            .iter()
            .zip(&theirs)
            .position(|(a, b)| a != b)
            .map_or_else(|| "the length".to_owned(), |at| format!("byte {at}"));
        return Err(format!(
            "optimised streams differ at {at} ({} vs {} bytes)",
            ours.len(),
            theirs.len()
        ));
    }

    // The loop must land on the same command in both.
    let theirs_loop = dro_core::optimize::optimize(expected).map_or_else(
        || expected.vgm_meta().and_then(|m| m.loop_point),
        |o| o.loop_point,
    );
    if file.loop_index() != theirs_loop {
        return Err(format!(
            "optimised loop differs ({:?} vs {theirs_loop:?})",
            file.loop_index()
        ));
    }
    Ok(())
}
