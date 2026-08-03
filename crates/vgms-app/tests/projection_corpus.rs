//! Corpus validation for the OPL projection, run on demand.
//!
//! Every OPL feature is being moved off the old closed-table reader onto the one
//! VGM model, and the promise is that nothing about an OPL song changes. This
//! checks that against real files: for every VGM the OPL reader accepts, the
//! projected song must agree row for row, total for total, and byte for byte on
//! the way out.
//!
//! Needs the local corpus via `VGMSTUDIO_VGMRIPS_CORPUS`:
//!
//! ```powershell
//! $env:VGMSTUDIO_VGMRIPS_CORPUS = 'F:\GameMusic\VGM\VGMRips_all_of_them_2025-10-17'
//! cargo test -p vgms-app --release --test projection_corpus -- --ignored --nocapture
//! ```
//!
//! It also reports the files the old reader rejected but the VGM model opens,
//! broken down by why.

use std::path::Path;

mod common;

#[test]
#[ignore = "needs the local corpus via VGMSTUDIO_VGMRIPS_CORPUS"]
fn the_projection_matches_the_opl_reader_across_the_corpus() {
    // Loud, not a silent skip: run explicitly (`--ignored`), so an unset corpus
    // is a setup mistake to report, not a reason to pass green.
    let root = std::env::var("VGMSTUDIO_VGMRIPS_CORPUS")
        .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory to run this test");
    let songs = common::collect_songs(Path::new(&root));
    assert!(!songs.is_empty(), "no .vgm/.vgz files under {root}");

    let mut scanned = 0usize;
    let mut opl = 0usize;
    let mut agreed = 0usize;
    let mut newly_openable = 0usize;
    let mut unreadable_by_both = 0usize;
    let mut with_blocks = 0usize;
    let mut split_pieces = 0usize;
    let mut chips_seen: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    // Why the OPL reader turned each newly-openable file away, so mg-1 can cite
    // how many files each gate it removes actually opens.
    let mut by_cause: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &songs {
        scanned += 1;
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        let by_opl_reader = vgms_core::vgm::io::read(&name, &bytes);
        let by_model = vgms_core::vgm::file::read(&name, &bytes);

        match (by_opl_reader, by_model) {
            (Ok(expected), Ok(file)) => {
                opl += 1;
                if file.to_song().is_none() {
                    failures.push(format!(
                        "{name}: the OPL reader accepted it, the model did not"
                    ));
                    continue;
                }
                // The reader-parity `compare(expected, projected)` link was
                // dropped: once mg-1 delegates `io::read` to `file::read`,
                // `expected` and the projection are the same code, so it would
                // pass vacuously. The unit goldens in `projection.rs` hold the
                // frozen reader reference; the optimiser and splitter oracles
                // below still bite.
                let checked = compare_optimised(&name, &bytes, &expected)
                    .and_then(|()| compare_split(&file, &expected, &mut split_pieces));
                if let Err(why) = checked {
                    failures.push(format!("{name}: {why}"));
                } else {
                    agreed += 1;
                }
                if file
                    .stream()
                    .is_some_and(|stream| !vgms_core::vgm::projection::is_wholly_opl(stream))
                {
                    with_blocks += 1;
                }
            }
            (Err(why), Ok(file)) => {
                // The files this feature exists for: openable now, not before.
                newly_openable += 1;
                *chips_seen.entry(file.chip_list()).or_default() += 1;
                *by_cause.entry(cause_label(&why.to_string())).or_default() += 1;
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
    eprintln!("split pieces checked: {split_pieces}");
    eprintln!("newly openable:     {newly_openable}");
    eprintln!("  by cause the OPL reader turned them away (first gate tripped):");
    for (cause, count) in &by_cause {
        eprintln!("    {count:>5}  {cause}");
    }
    eprintln!("  by chip:");
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

/// Which gate `io::read` tripped first, from its error message -- the faithful
/// "first cause", since the reader short-circuits: version, then OPL chip, then
/// an unsupported command in the stream, then a bad GD3. Ephemeral scaffolding:
/// it breaks the newly-openable count down for mg-1's evidence, and goes with
/// the `io::read` arm at mg-2.
fn cause_label(why: &str) -> &'static str {
    if why.contains("Unsupported VGM version") {
        "old version (< v1.51)"
    } else if why.contains("No OPL2 or OPL3") {
        "non-OPL chip"
    } else if why.contains("Unsupported VGM command") {
        "unsupported command"
    } else {
        "other (GD3 / parse)"
    }
}

/// Guards the classifier against a wording drift in `io::read`, using the exact
/// strings it produces (this runs even though the corpus sweep is `#[ignore]`).
#[test]
fn cause_label_buckets_by_the_first_gate() {
    assert_eq!(
        cause_label("Unsupported VGM version, v1.51 is the minimum supported version."),
        "old version (< v1.51)"
    );
    assert_eq!(
        cause_label("No OPL2 or OPL3 data detected."),
        "non-OPL chip"
    );
    assert_eq!(
        cause_label("Unsupported VGM command: 0x50"),
        "unsupported command"
    );
    assert_eq!(
        cause_label("Does not appear to be a GD3 tag (invalid header)."),
        "other (GD3 / parse)"
    );
}

/// The chip-agnostic optimiser against the OPL one it will replace, on the same
/// file: both halves -- the redundant-write strip and the byte-minimal delay
/// merge -- must produce the same bytes, or every optimised file in a pack
/// would change how it is spelled.
fn compare_optimised(name: &str, bytes: &[u8], expected: &vgms_core::Song) -> Result<(), String> {
    let mut file = vgms_core::vgm::file::read(name, bytes).map_err(|e| e.to_string())?;
    file.optimize();
    // The *stream*, not the whole file: the two writers legitimately differ in
    // the header (the OPL one re-derives the chip clocks from the song's type,
    // which is the canonicalisation `vgm::file::write` deliberately avoids), so
    // comparing files would be comparing writers rather than optimisers.
    let ours = file.body.raw();
    let ours = &ours[..ours.len().saturating_sub(1)]; // drop the end marker

    let theirs = match vgms_core::optimize::optimize(expected) {
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
    let theirs_loop = vgms_core::optimize::optimize(expected).map_or_else(
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

/// The chip-agnostic splitter against the OPL one, on the same file.
///
/// Detection must agree exactly -- it is a pure function of the stream, and a
/// disagreement means one of the two miscounts a gap. Each piece must then hold
/// the same music: the same OPL register state at its end, and the same length.
///
/// Not the same *bytes*, deliberately. The two state preludes emit the same
/// writes in different orders (the OPL fold walks the register file low bank
/// first, the generic one replays the source's own write order) and the generic
/// one also restores a register explicitly written to zero, which the OPL fold
/// skips as unchanged from a blank chip. Both leave the chip in the same place,
/// which is what this checks.
fn compare_split(
    file: &vgms_core::VgmFile,
    expected: &vgms_core::Song,
    pieces_checked: &mut usize,
) -> Result<(), String> {
    // vgm_sptd's own gap threshold: 0x8000 samples.
    const THRESHOLD: u32 = 0x8000;
    let theirs = vgms_core::split_songs::detect_segments(expected, THRESHOLD);
    let ours = vgms_core::split_songs::detect_segments_in_vgm(file, THRESHOLD);
    if ours != theirs {
        return Err(format!(
            "detected {} segment(s), the OPL splitter {}",
            ours.len(),
            theirs.len()
        ));
    }

    for (index, segment) in theirs.iter().enumerate() {
        let theirs = vgms_core::split_songs::materialise(expected, segment, true, 0);
        let ours = vgms_core::split_songs::materialise_vgm(file, segment, true, 0)
            .ok_or_else(|| format!("segment {index}: the generic splitter produced nothing"))?;
        let ours = ours
            .to_song()
            .ok_or_else(|| format!("segment {index}: the piece is no longer an OPL song"))?;

        if opl_state(&ours) != opl_state(&theirs) {
            return Err(format!(
                "segment {index}: the pieces leave the chip in different states"
            ));
        }
        if ours.total_delay_ms() != theirs.total_delay_ms() {
            return Err(format!(
                "segment {index}: piece lengths differ ({} ms vs {} ms)",
                ours.total_delay_ms(),
                theirs.total_delay_ms()
            ));
        }
        *pieces_checked += 1;
    }
    Ok(())
}

/// The OPL register state a song leaves behind: every register it wrote, at its
/// last value, with registers left at zero dropped (a blank chip reads zero, so
/// writing one explicitly is indistinguishable from never writing it).
fn opl_state(song: &vgms_core::Song) -> std::collections::BTreeMap<(u8, u8), u8> {
    use vgms_core::song::{Bank, Instruction};
    let mut state = std::collections::BTreeMap::new();
    let mut bank = Bank::Low;
    for index in 0..song.len() {
        match song.instruction(index) {
            Some(Instruction::BankSwitch(to)) => bank = to,
            Some(Instruction::Register {
                reg,
                value,
                bank: at,
            }) => {
                state.insert((at.unwrap_or(bank).index(), reg), value);
            }
            _ => {}
        }
    }
    state.retain(|_, value| *value != 0);
    state
}
