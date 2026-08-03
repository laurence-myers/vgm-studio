//! Investigation (not a gate): can the in-house `VgmFile::optimize()` replace the
//! external `vgm_cmp`, and does `vgm_cmp` corrupt audio -- especially on a file
//! that has already been optimised?
//!
//! For every corpus file it renders the original and four variants through the
//! real engine and compares samples:
//!   B  = built-in optimize()                 (should be audio-identical)
//!   C  = vgm_cmp on the original             (first pass)
//!   CB = vgm_cmp on the built-in output      (THE HYPOTHESIS: already-optimised)
//!   CC = vgm_cmp twice                        (idempotency)
//! and tallies, per chip, where each variant changes what the file plays, plus
//! the size each achieves. Prints a report; run with --nocapture.
//!
//!   $env:VGMSTUDIO_VGMRIPS_CORPUS = 'F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17'
//!   $env:VGMSTUDIO_CORPUS_LIMIT = '400'
//!   cargo test -p vgms-app --release --test optimizer_investigation -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use vgms_core::vgm::VgmFile;
use vgms_synth::vgm_engine::VgmEngine;

mod common;

const OUTPUT_RATE: u32 = 44_100;
/// Enough to reach past the driver set-up and into the music; short enough to
/// sweep a corpus with five renders per file.
const FRAMES: usize = 44_100 * 4;

fn render_file(file: &VgmFile) -> Vec<i16> {
    let mut engine = VgmEngine::new(Arc::new(file.clone()), OUTPUT_RATE);
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

fn render_bytes(name: &str, bytes: &[u8]) -> Option<Vec<i16>> {
    let file = vgms_core::vgm::file::read(name, bytes).ok()?;
    Some(render_file(&file))
}

/// `(first differing sample index, peak absolute difference)` or `None` when the
/// two renders are sample-identical. A tiny peak with an early index is the
/// inaudible write-scheduler phase shift; a large sustained peak is corruption.
fn difference(a: &[i16], b: &[i16]) -> Option<(usize, i32)> {
    let mut first = None;
    let mut peak = 0i32;
    let n = a.len().min(b.len());
    for i in 0..n {
        let d = (i32::from(a[i]) - i32::from(b[i])).abs();
        if d != 0 && first.is_none() {
            first = Some(i);
        }
        peak = peak.max(d);
    }
    if a.len() != b.len() && first.is_none() {
        first = Some(n);
    }
    first.map(|i| (i, peak))
}

/// vgm_cmp over `bytes`, returning the (possibly unchanged) result bytes and
/// whether it actually shrank the file.
fn vgm_cmp(bytes: &[u8]) -> (Vec<u8>, bool) {
    match vgms_vgmtools::optimize_writes(bytes) {
        vgms_vgmtools::ToolOutcome::Smaller(out) => (out, true),
        vgms_vgmtools::ToolOutcome::Unchanged => (bytes.to_vec(), false),
        vgms_vgmtools::ToolOutcome::Failed(_) => (bytes.to_vec(), false),
    }
}

/// built-in optimize over a file's bytes, returning the result bytes and whether
/// it shrank.
fn built_in(bytes: &[u8]) -> (Vec<u8>, bool) {
    let Ok(mut file) = vgms_core::vgm::file::read("x.vgm", bytes) else {
        return (bytes.to_vec(), false);
    };
    let shrank = file.optimize().is_some();
    match vgms_core::vgm::file::write(&file) {
        Ok(out) => (out, shrank),
        Err(_) => (bytes.to_vec(), false),
    }
}

#[derive(Default)]
struct Tally {
    /// Files where this variant changed what the file plays.
    changed: Vec<String>,
    /// Bytes before and after, over files this variant shrank.
    before: u64,
    after: u64,
    shrank: usize,
}

impl Tally {
    fn note_change(&mut self, name: &str, chips: &str, first: usize, peak: i32) {
        if self.changed.len() < 25 {
            self.changed
                .push(format!("{name} [{chips}] @ sample {first}, peak {peak}"));
        }
    }
}

/// THE GATE (s1-1): the built-in optimiser must never change what a file plays,
/// on any chip. Renders every corpus file unoptimised and built-in-optimised and
/// requires them byte-identical -- valid because `VgmEngine` applies writes
/// immediately at wait-boundaries, so a difference is a real state change, not a
/// phase artifact. Fails, naming the chip, if the built-in drops a write that
/// matters. This is what decides the built-in's safe coverage (D-opt-1/2).
#[test]
#[ignore = "gate, needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn the_builtin_optimizer_never_changes_audio() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    // Optional: only test files whose chip list contains this substring, e.g.
    // "YM2612" or "YM3812". Without it, a strided spread across the whole corpus
    // is taken, so an alphabetically-clustered corpus still yields diverse chips
    // (the first N sorted files are often one chip).
    let chip_filter = std::env::var("VGMSTUDIO_CHIP_FILTER").ok();

    vgms_app::install_cores();
    let all = common::collect_songs(&root);
    assert!(!all.is_empty(), "no files under {}", root.display());
    // With a chip filter we scan everything for matches; otherwise we stride so
    // the tested set spans the corpus rather than its first alphabetical slice.
    let stride = if chip_filter.is_some() {
        1
    } else {
        (all.len() / limit.max(1)).max(1)
    };
    let candidates: Vec<&PathBuf> = all.iter().step_by(stride).collect();

    // chip list -> (files checked, files whose audio changed)
    let mut per_chip: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    // Files the built-in changed, but whose chip core renders non-deterministically
    // (some libvgm cores power on to unreset state), so a render diff cannot be
    // blamed on the optimiser -- reported, not failed.
    let mut nondeterministic = 0usize;

    for path in candidates {
        if checked >= limit {
            break;
        }
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(file) = vgms_core::vgm::file::read(&name, &raw) else {
            continue;
        };
        let chips = file.chip_list();
        if let Some(want) = &chip_filter
            && !chips.contains(want.as_str())
        {
            continue;
        }
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            continue;
        };
        checked += 1;
        let entry = per_chip.entry(chips.clone()).or_default();
        entry.0 += 1;

        let (optimized, _) = built_in(&plain);
        // The built-in changed nothing: the bytes are the plain round-trip, so no
        // audio change is possible. (This is where a chip with no rule lands --
        // it drops no writes -- so non-deterministic cores never reach the render
        // comparison below.)
        if optimized == plain {
            continue;
        }
        // The built-in changed the file. Establish the determinism baseline first:
        // render the original twice. A core that renders differently each time
        // cannot be judged by a render diff, so skip it rather than blame the
        // optimiser for the core's own noise.
        let original = render_file(&file);
        if difference(&original, &render_file(&file)).is_some() {
            nondeterministic += 1;
            continue;
        }
        // Deterministic: any difference now is the built-in's doing.
        if let Some(rendered) = render_bytes(&name, &optimized)
            && let Some((first, peak)) = difference(&original, &rendered)
        {
            entry.1 += 1;
            if failures.len() < 40 {
                failures.push(format!("{name} [{chips}] @ sample {first}, peak {peak}"));
            }
        }
    }

    println!("\n-- built-in parity by chip ({checked} files) --");
    let mut bad_chips = 0usize;
    for (chip, (n, changed)) in &per_chip {
        let mark = if *changed > 0 {
            " <-- CHANGES AUDIO"
        } else {
            ""
        };
        if *changed > 0 {
            bad_chips += 1;
        }
        println!("  {chip}: {n} checked, {changed} changed{mark}");
    }
    println!(
        "skipped {nondeterministic} file(s) the built-in changed but whose chip core \
         renders non-deterministically (cannot be judged by a render diff)"
    );
    if !failures.is_empty() {
        println!("\nexamples:");
        for line in &failures {
            println!("  {line}");
        }
    }

    assert!(
        failures.is_empty(),
        "the built-in optimiser changed the audio of {} file(s) across {bad_chips} chip \
         configuration(s) -- it must be made self-safe (disable the offending chip's rule) \
         before those chips are optimised by it rather than the tools",
        failures.len()
    );
}

#[test]
#[ignore = "investigation, needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly with --nocapture"]
fn vgm_cmp_vs_builtin_over_the_corpus() {
    let root = PathBuf::from(
        std::env::var_os("VGMSTUDIO_VGMRIPS_CORPUS")
            .expect("VGMSTUDIO_VGMRIPS_CORPUS must name the corpus directory"),
    );
    let limit: usize = std::env::var("VGMSTUDIO_CORPUS_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);

    vgms_app::install_cores();
    let paths = common::collect_songs_capped(&root, limit);
    assert!(!paths.is_empty(), "no files under {}", root.display());

    let (mut scanned, mut rendered) = (0usize, 0usize);
    // The audio-change tallies, each vs the ORIGINAL render.
    let mut b = Tally::default(); // built-in changed audio (should stay empty)
    let mut c = Tally::default(); // vgm_cmp first pass changed audio
    let mut cb = Tally::default(); // vgm_cmp on built-in output changed audio  <-- hypothesis
    let mut cc_not_idempotent = 0usize; // render(vgm_cmp twice) != render(vgm_cmp once)
    let mut cc_examples: Vec<String> = Vec::new();
    // How often vgm_cmp changes a file the built-in had already reduced.
    let mut cmp_shrinks_builtin_output = 0usize;
    // Size reached by chip family, so a coverage gap is visible.
    let mut per_chip: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new(); // chip -> (orig, builtin, cmp)

    for path in &paths {
        scanned += 1;
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(file) = vgms_core::vgm::file::read(&name, &raw) else {
            continue;
        };
        // Canonical uncompressed bytes: what both optimisers take.
        let Ok(plain) = vgms_core::vgm::file::write(&file) else {
            continue;
        };
        let chips = file.chip_list();
        let orig_render = render_file(&file);
        rendered += 1;

        // B: built-in.
        let (b_bytes, b_shrank) = built_in(&plain);
        // C: vgm_cmp on the original.
        let (c_bytes, c_shrank) = vgm_cmp(&plain);
        // CB: vgm_cmp on the built-in output (the already-optimised case).
        let (cb_bytes, cb_shrank) = vgm_cmp(&b_bytes);
        // CC: vgm_cmp twice.
        let (cc_bytes, _) = vgm_cmp(&c_bytes);

        if cb_shrank {
            cmp_shrinks_builtin_output += 1;
        }

        let entry = per_chip.entry(chips.clone()).or_default();
        entry.0 += plain.len() as u64;
        entry.1 += b_bytes.len() as u64;
        entry.2 += c_bytes.len() as u64;
        if b_shrank {
            b.before += plain.len() as u64;
            b.after += b_bytes.len() as u64;
            b.shrank += 1;
        }
        if c_shrank {
            c.before += plain.len() as u64;
            c.after += c_bytes.len() as u64;
            c.shrank += 1;
        }

        // Render each variant and compare to the original.
        if let Some(r) = render_bytes(&name, &b_bytes)
            && let Some((first, peak)) = difference(&orig_render, &r)
        {
            b.note_change(&name, &chips, first, peak);
        }
        let c_render = render_bytes(&name, &c_bytes);
        if let Some(r) = &c_render
            && let Some((first, peak)) = difference(&orig_render, r)
        {
            c.note_change(&name, &chips, first, peak);
        }
        if let Some(r) = render_bytes(&name, &cb_bytes)
            && let Some((first, peak)) = difference(&orig_render, &r)
        {
            cb.note_change(&name, &chips, first, peak);
        }
        // Idempotency: the twice-run must render the same as the once-run.
        if let (Some(once), Some(twice)) = (&c_render, render_bytes(&name, &cc_bytes))
            && difference(once, &twice).is_some()
        {
            cc_not_idempotent += 1;
            if cc_examples.len() < 25 {
                cc_examples.push(format!("{name} [{chips}]"));
            }
        }
    }

    let pct = |before: u64, after: u64| {
        if before == 0 {
            0.0
        } else {
            100.0 * (1.0 - after as f64 / before as f64)
        }
    };

    println!(
        "\n================ vgm_cmp vs built-in ({} files) ================",
        paths.len()
    );
    println!("scanned {scanned}, rendered {rendered}");
    println!(
        "\n-- size (over files each shrank) --\n\
         built-in: shrank {} files, {} -> {} ({:.1}%)\n\
         vgm_cmp:  shrank {} files, {} -> {} ({:.1}%)",
        b.shrank,
        b.before,
        b.after,
        pct(b.before, b.after),
        c.shrank,
        c.before,
        c.after,
        pct(c.before, c.after),
    );

    println!("\n-- AUDIO CHANGES vs the original render --");
    report("BUILT-IN changed audio (expected: none)", &b.changed);
    report("vgm_cmp (first pass) changed audio", &c.changed);
    report(
        "vgm_cmp on an ALREADY-OPTIMISED (built-in) file changed audio  <-- hypothesis",
        &cb.changed,
    );
    println!("\nvgm_cmp NOT idempotent (twice != once): {cc_not_idempotent} file(s)");
    for line in cc_examples.iter().take(25) {
        println!("    {line}");
    }
    println!(
        "vgm_cmp still shrank the built-in's output on {cmp_shrinks_builtin_output} file(s) \
         (writes the built-in kept that vgm_cmp drops)"
    );

    println!("\n-- size by chip (orig -> built-in / vgm_cmp) --");
    for (chip, (o, bi, cm)) in &per_chip {
        println!(
            "  {chip}: {o} -> builtin {bi} ({:.1}%) / vgm_cmp {cm} ({:.1}%)",
            pct(*o, *bi),
            pct(*o, *cm),
        );
    }

    // This test never fails -- it reports. The verdict is read from the output.
    println!("\n(investigation complete)\n");
}

/// Is a render deterministic, and does the built-in change it? Distinguishes a
/// real optimise corruption from render non-determinism (a chip core with
/// unreset/rng state would make the gate flag a file the optimiser never
/// touched). Set VGMSTUDIO_INSPECT_FILE.
#[test]
#[ignore = "diagnostic; needs VGMSTUDIO_INSPECT_FILE"]
fn render_determinism() {
    let path = PathBuf::from(
        std::env::var_os("VGMSTUDIO_INSPECT_FILE").expect("set VGMSTUDIO_INSPECT_FILE"),
    );
    vgms_app::install_cores();
    let raw = std::fs::read(&path).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &raw).unwrap();

    let a = render_file(&file);
    let b = render_file(&file);
    println!("\n== {name} [{}] ==", file.chip_list());
    println!("render twice, same engine build: {:?}", difference(&a, &b));

    let plain = vgms_core::vgm::file::write(&file).unwrap();
    let (opt, shrank) = built_in(&plain);
    println!(
        "built-in optimize shrank: {shrank} ({} -> {} bytes)",
        plain.len(),
        opt.len()
    );
    if let Some(r) = render_bytes(&name, &opt) {
        println!("original vs built-in-optimized: {:?}", difference(&a, &r));
    }
    // And is `plain` (the uncompressed round-trip, no optimise) itself faithful?
    if let Some(r) = render_bytes(&name, &plain) {
        println!(
            "original vs plain round-trip (no optimize): {:?}",
            difference(&a, &r)
        );
    }
}

fn report(title: &str, changed: &[String]) {
    println!("  {title}: {} file(s)", changed.len());
    for line in changed.iter().take(25) {
        println!("      {line}");
    }
}

/// Names the exact registers the built-in optimiser drops on one file, so a
/// render difference can be attributed to a specific chip register class.
///
///   $env:VGMSTUDIO_INSPECT_FILE = 'F:/.../Growing Up Town.vgz'
///   cargo test -p vgms-app --release --test optimizer_investigation \
///       dump_dropped_writes -- --ignored --nocapture
#[test]
#[ignore = "diagnostic; needs VGMSTUDIO_INSPECT_FILE"]
fn dump_dropped_writes() {
    use vgms_core::vgm::stream::VgmCommand;

    let path = PathBuf::from(
        std::env::var_os("VGMSTUDIO_INSPECT_FILE").expect("set VGMSTUDIO_INSPECT_FILE"),
    );
    let raw = std::fs::read(&path).unwrap();
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &raw).unwrap();
    let stream = file.stream().expect("walks");

    let dropped = vgms_core::chip_state::redundant_indices(stream, file.loop_index());
    println!("\n== {name} [{}] ==", file.chip_list());
    println!(
        "stream commands: {}, built-in drops: {}",
        stream.len(),
        dropped.len()
    );

    // Histogram of dropped writes by (chip, port, register-high-nibble-ish).
    let mut by_reg: BTreeMap<String, usize> = BTreeMap::new();
    for &i in &dropped {
        if let Some(VgmCommand::Write { target, addr, data }) = stream.get(i) {
            let key = format!(
                "{:?} port{} reg {:#04X} = {:#04X}",
                target.kind, target.port, addr, data
            );
            *by_reg.entry(key).or_default() += 1;
        }
    }
    let mut rows: Vec<(&String, &usize)> = by_reg.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    println!("dropped writes by register (most-dropped first):");
    for (key, count) in rows.iter().take(40) {
        println!("  {count:6}  {key}");
    }
}
