//! Our render against a reference player's, chip by chip.
//!
//! The mechanical half of `CORES-PLAN` §6.2's acceptance bar. The reasoning,
//! the two regimes and the step list are in
//! `docs/vgm-multichip-2026-07/PARITY-PLAN.md`; this is the harness.
//!
//! ```text
//! DROTRIM_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//! DROTRIM_REF_PLAYER=/path/to/vgmplay \
//! DROTRIM_REF_ARGS="-o" \
//!     cargo test -p dro-trimmer --release --test reference_parity -- --ignored --nocapture
//! ```
//!
//! **Without a reference player every test here skips**, saying so, exactly as
//! the corpus tests skip without a corpus. The pipeline's own correctness does
//! not depend on having one: `parity::metrics` is self-tested against signals
//! whose answers are known by construction, and
//! `the_pipeline_agrees_with_itself` below closes the loop end to end with no
//! external binary at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dro_core::vgm::ChipKind;
use dro_synth::vgm_engine::VgmEngine;
use dro_trimmer::corpus::{self, ChipIndex};
use dro_trimmer::parity::{self, Reference, ReferenceError, Render, Settings};

/// The rate both sides render at unless a chip's native rate is asked for.
const RATE: u32 = 44_100;
/// How much of each file to compare. Long enough for an envelope to develop,
/// short enough that a dozen files per chip is minutes rather than an hour.
const SECONDS: usize = 20;
/// Files per chip.
const SAMPLE: usize = 12;

/// Renders `path` the way the app would.
///
/// **Which engine matters.** An OPL file plays through `PlayerEngine`, which
/// carries the OPL register policy; `VgmEngine` has no OPL core at all and
/// would render it as silence -- the same listed-versus-buildable distinction
/// the registry draws. Routing every file through the generic engine made the
/// OPL control group compare silence with silence and score zero, which is
/// exactly the failure the control group exists to expose, arriving one layer
/// earlier than expected.
fn render_ours(path: &Path) -> Option<Render> {
    render_ours_at(path, RATE)
}

/// The same, at a caller-chosen rate -- so a chip can be compared at the rate
/// it natively runs at and neither side's resampler enters the measurement.
fn render_ours_at(path: &Path, rate: u32) -> Option<Render> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = dro_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    if let Some(song) = file.to_song() {
        // The OPL path, exactly as the WAV export uses it.
        let wav = dro_synth::render_wav(&song, rate, 16).ok()?;
        let (samples, wav_rate) = parity::reference::read_wav(&wav).ok()?;
        let wanted = rate as usize * SECONDS * 2;
        let samples = &samples[..samples.len().min(wanted)];
        return Some(Render::from_interleaved_i16(samples, wav_rate));
    }

    render_with_at(path, rate, |kind| {
        dro_synth::registry::registry().build(kind, None)
    })
}

/// Renders `path` at `rate` with a caller-chosen set of cores -- the
/// decomposition the balance fit needs.
fn render_with_at(
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

/// Files declaring exactly `chip` and nothing else.
///
/// A single-chip file is what makes a global gain fit legitimate: with two
/// chips in the mix, one scalar cannot describe both and the number would be an
/// average of two different answers.
fn single_chip_files(index: &ChipIndex, root: &Path, chip: ChipKind, want: usize) -> Vec<PathBuf> {
    let all = index.files(chip);
    // Strided rather than taken from the head, because the index is in
    // directory order: the first dozen entries are one game's soundtrack, and a
    // dozen tracks by one composer on one driver is a sample of that driver,
    // not of the chip. `ChipIndex::sample` strides for the same reason.
    let stride = (all.len() / want.max(1)).max(1);
    // Every offset in turn, so the spread is preferred but nothing is
    // unreachable: most of the corpus fails the filters below, and a sample
    // that gave up after one pass would come back half empty.
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
        // The reference applies the header volume modifier and the extra
        // header's per-chip volumes; our engine applies neither. Until that
        // gap closes, a file that uses them would show a gain difference that
        // is real but is not the chip's -- so they are filtered out rather
        // than explained away in every result.
        if file.header.volume_modifier() != 0 {
            continue;
        }
        found.push(path);
    }
    found
}

/// A copy of `path` cut to just past the compared window, or `path` itself if
/// it is already short enough or will not walk.
///
/// **The reference renders whole tracks; we compare twenty seconds of them.**
/// A three-minute rip at a chip's native rate is nine times the work for the
/// same answer, and across thirteen chips and a dozen files each that was the
/// difference between a scorecard that runs in an hour and one that runs in
/// five. Cutting the input instead of the output is what makes the saving
/// real -- there is no "render only the first N seconds" switch to ask for.
///
/// Both sides are given the *same* cut file, so this cannot bias a comparison:
/// it is the identical input either way. A couple of seconds past the window
/// are kept so nothing near the boundary is measured against a decay that one
/// side was cut off from.
fn shortened(path: &Path, work_dir: &Path) -> PathBuf {
    use dro_core::vgm::stream::VgmCommand;

    let keep_samples = (SECONDS as u32 + 2) * 44_100;
    let Ok(bytes) = std::fs::read(path) else {
        return path.to_owned();
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let Ok(mut file) = dro_core::vgm::file::read(&name, &bytes) else {
        return path.to_owned();
    };
    if file.total_samples() <= keep_samples {
        return path.to_owned();
    }
    let Some(stream) = file.stream() else {
        return path.to_owned();
    };
    // The VGM time base is 44100 regardless of what any chip runs at, so the
    // row to cut at is found by adding up waits, not by any chip's clock.
    let mut elapsed = 0u32;
    let mut cut = stream.len();
    for index in 0..stream.len() {
        match stream.get(index) {
            Some(VgmCommand::Wait(samples)) => elapsed += samples,
            Some(VgmCommand::DacWrite { wait }) => elapsed += wait,
            _ => {}
        }
        if elapsed >= keep_samples {
            cut = index + 1;
            break;
        }
    }
    if file.crop_to_region(0, cut).is_none() {
        return path.to_owned();
    }
    let Ok(out) = dro_core::vgm::file::write(&file) else {
        return path.to_owned();
    };
    let short_dir = work_dir.join("shortened");
    if std::fs::create_dir_all(&short_dir).is_err() {
        return path.to_owned();
    }
    // `.vgm`, not `.vgz`: the writer emits plain bytes, and handing the
    // reference a file whose name promises gzip would fail at its reader.
    let target = short_dir.join(format!(
        "{}.vgm",
        path.file_stem().unwrap_or_default().to_string_lossy()
    ));
    if std::fs::write(&target, out).is_err() {
        return path.to_owned();
    }
    target
}

/// The rate `chip` runs at in `path`, straight from the core.
///
/// **Not a constant, because it is not one.** A core's native rate is derived
/// from the clock in the file's header -- a YM2151 at 3.58 MHz renders at
/// 55930 Hz and the same chip at 4 MHz does not -- so the rate to compare at
/// has to be asked per file. Comparing at anything else puts two resamplers
/// between the cores and measures those instead, which cost the OPL control
/// group fifteen points of correlation before it was fixed.
fn native_rate_of(path: &Path, chip: ChipKind) -> Option<u32> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = dro_core::vgm::file::read(&name, &bytes).ok()?;
    let clocked = file.header.chips().iter().find(|c| c.kind == chip)?;
    let mut core = dro_synth::registry::registry().build(chip, None)?;
    core.reset(clocked.clock, clocked.variant);
    Some(core.native_rate())
}

/// What fraction of an OPL file's operator writes switch vibrato on.
///
/// **This is the control group's explanation of itself.** Our OPL core and the
/// reference's are the same Nuked source, and with both rendering at the
/// chip's native rate they agree to three or four decimal places on level,
/// envelope, average pitch, DC and alignment -- except on files that use
/// vibrato:
///
/// ```text
///   vibrato share of operator writes     correlation
///          0%                              0.998, 0.999
///          6%                              0.978, 0.984
///         16%                              0.935, 0.972
///         46%                              0.590
/// ```
///
/// Zero vibrato is the only reliable predictor -- a 12% file scored 0.998 while
/// a 16% one scored 0.935, because what matters is how much of the *audible*
/// energy is modulated, not how many writes set the bit. That is why this is
/// used as a filter for the assertion and not as a correction to it.
///
/// The spectrum says why. Both renders put the same energy in the same
/// partials, but each partial's *instantaneous* frequency wobbles on a
/// different schedule -- the chip's vibrato LFO free-runs from reset, and the
/// two sides start it at different points relative to the music. The average
/// pitch is identical (0.0 cents), so nothing is out of tune; the waveforms
/// simply are not the same waveform. It is not a resampler, a gain, or a
/// missing voice, and no amount of pipeline work will close it.
fn vibrato_share(path: &Path) -> Option<f64> {
    use dro_core::vgm::stream::VgmCommand;
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = dro_core::vgm::file::read(&name, &bytes).ok()?;
    let stream = file.stream()?;
    let (mut operators, mut vibrato) = (0usize, 0usize);
    for command in (0..stream.len()).filter_map(|index| stream.get(index)) {
        // 0x20..=0x35 in either bank: the operator's AM/VIB/EG/KSR/MULT byte,
        // whose bit 6 is that operator's vibrato switch.
        if let VgmCommand::Write { addr, data, .. } = command
            && matches!(addr & 0xff, 0x20..=0x35)
        {
            operators += 1;
            if data & 0x40 != 0 {
                vibrato += 1;
            }
        }
    }
    (operators > 0).then(|| vibrato as f64 / operators as f64)
}

/// The reference, or a printed reason and `None`.
fn reference() -> Option<Reference> {
    match Reference::from_env() {
        Ok(reference) => {
            println!("reference: {}", reference.describe());
            Some(reference)
        }
        Err(ReferenceError::NotConfigured(why)) => {
            eprintln!(
                "no reference player ({why}); skipping. Set {} to a batch-capable \
                 player -- see docs/vgm-multichip-2026-07/PARITY-PLAN.md §1.",
                parity::reference::PLAYER_ENV
            );
            None
        }
        Err(other) => {
            eprintln!("reference unusable: {other}; skipping");
            None
        }
    }
}

fn work_dir() -> PathBuf {
    std::env::temp_dir().join("drotrim-parity")
}

/// **pt-1's acceptance.** The reference must be a fixed point, or every
/// threshold downstream is noise -- and the symptom would look like flaky cores
/// rather than a flaky reference.
///
/// Checked **per chip**, on the same single-chip files the scorecard compares,
/// because determinism turns out not to be a property of the player: the first
/// run of this test drew a YMF262+YMZ280B rip and the reference disagreed with
/// itself on 0.9% of samples, at full scale -- the signature of a PCM chip
/// reading sample memory it never wrote. A player that is a fixed point for FM
/// and a coin toss for PCM would have quietly widened exactly the thresholds
/// that are supposed to be catching our bugs.
#[test]
#[ignore = "needs DROTRIM_REF_PLAYER; run explicitly"]
fn the_reference_player_is_deterministic() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = dro_synth::registry::registry();

    // The control group leads, then everything the scorecard will compare.
    let mut chips = vec![ChipKind::Ymf262];
    chips.extend(
        ChipKind::all().filter(|chip| registry.can_build(*chip) && *chip != ChipKind::Ymf262),
    );

    let (mut checked, mut flaky) = (0usize, Vec::new());
    for chip in chips {
        let Some(file) = single_chip_files(&index, &root, chip, 1).into_iter().next() else {
            println!("{:<14} no single-chip corpus file", chip.name());
            continue;
        };
        match reference.self_check(&file, &work_dir()) {
            Ok(()) => {
                checked += 1;
                println!("{:<14} identical twice   {}", chip.name(), short(&file));
            }
            Err(ReferenceError::NotDeterministic) => {
                flaky.push(format!("{} ({})", chip.name(), short(&file)));
                println!("{:<14} DIFFERED           {}", chip.name(), short(&file));
            }
            Err(other) => eprintln!("{:<14} unusable: {other}", chip.name()),
        }
    }

    assert!(
        checked > 0,
        "nothing was rendered; the reference is unusable"
    );
    assert!(
        flaky.is_empty(),
        "the reference rendered these differently twice: {}. Every threshold \
         for them would be noise, and the symptom would look like flaky cores.",
        flaky.join(", ")
    );
}

/// **pt-3: the control group.** Our OPL core is proven bit-identical to the C
/// the reference runs (`dro-synth`'s `c-parity` suite), so anything less than a
/// near-perfect score here is the *harness* -- resampling, alignment, gain
/// fitting -- and not a chip. Nothing else in this file means anything until
/// this passes.
///
/// It is asserted on **vibrato-free files**, which is where "the same core
/// driven the same way" is actually true. See [`vibrato_share`] for the
/// evidence that vibrato is a chip-state difference rather than a pipeline one;
/// the vibrato-using files are still rendered, scored and printed, because a
/// difference that is explained still has to stay visible.
#[test]
#[ignore = "needs DROTRIM_REF_PLAYER; run explicitly"]
fn the_opl_control_group_calibrates_the_pipeline() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    let files = single_chip_files(&index, &root, ChipKind::Ymf262, SAMPLE);
    assert!(!files.is_empty(), "no single-chip OPL file in the corpus");

    // **Both sides at the chip's own rate.** At 44100 this comparison scored
    // 0.84 on a core that is bit-identical to the reference's, and the
    // alignment slid several frames between the start of a file and its end --
    // two resamplers disagreeing, which is exactly the pipeline artefact the
    // control group exists to find. At 49716 neither side converts anything.
    let native = dro_synth::NATIVE_SAMPLE_RATE;
    let reference = reference.at_rate(native);

    let (mut worst, mut judged) = (1.0f64, 0usize);
    for original in &files {
        // The same cut file to both sides -- see `shortened`.
        let path = &shortened(original, &work_dir());
        let Some(ours) = render_ours_at(path, native) else {
            continue;
        };
        let vibrato = vibrato_share(path).unwrap_or(0.0);
        let bytes = match reference.render(path, &work_dir()) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("  {}: {error}", path.display());
                continue;
            }
        };
        let (samples, rate) = parity::reference::read_wav(&bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let theirs = Render::from_interleaved_i16(&samples, rate);

        let score = parity::compare(&ours, &theirs, Settings::default());
        println!(
            "  {:<50} {}  vib {:.0}%",
            short(original),
            score.summary(),
            vibrato * 100.0
        );
        if score.worst_correlation() < 0.99
            && let Some(where_to) = parity::dump_pair(&short(original), &ours, &theirs)
        {
            println!(
                "    flagged; both renders written to {}",
                where_to.display()
            );
        }
        // The control group is where a soft correlation has to be *explained*,
        // not merely reported: an even resampler difference and a rate
        // difference look identical in the headline number and have completely
        // different causes.
        if let Some((head, head_score, tail, tail_score)) = parity::metrics::lag_drift(
            ours.channels()[0],
            theirs.channels()[0],
            (native as usize) / 200,
        ) {
            println!(
                "    alignment: head {head:+} ({head_score:.4})  tail {tail:+} ({tail_score:.4})  \
                 drift {:+}",
                tail - head
            );
        }
        // Files that switch vibrato on are reported but not judged: the LFO
        // phase difference is a property of the two chips' state, and holding
        // the pipeline to account for it would mean either a permanently red
        // control group or a bar so loose it stops catching pipeline faults.
        if vibrato == 0.0 {
            judged += 1;
            worst = worst.min(score.worst_correlation());
        }
    }
    assert!(
        judged > 0,
        "every OPL file sampled uses vibrato, so nothing here measures the \
         pipeline. Widen the sample."
    );
    assert!(
        worst >= 0.99,
        "the control group scored {worst} across {judged} vibrato-free files. \
         Our OPL core is bit-identical to the reference's and vibrato is ruled \
         out, so this is the pipeline -- resampling, alignment, the gain fit -- \
         and every other result here is meaningless until it is fixed."
    );
    println!("the pipeline is sound: {judged} vibrato-free files, worst {worst:.4}");
}

/// **pt-4 and pt-5**: the scorecard, against the frozen per-chip bar.
///
/// The first run against a new reference is expected to *fail* and be read as a
/// table: the thresholds in `parity::THRESHOLDS` are provisional until the
/// outliers have been listened to and the observed band written down. That
/// calibration is a deliberate step, not a slip.
#[test]
#[ignore = "needs DROTRIM_REF_PLAYER; run explicitly"]
fn every_cored_chip_matches_the_reference_within_its_band() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = dro_synth::registry::registry();

    let mut failures = Vec::new();
    for chip in ChipKind::all() {
        if !registry.can_build(chip) {
            continue;
        }
        let Some(bar) = parity::threshold_for(chip) else {
            println!("{:<14} no threshold -- not compared", chip.name());
            continue;
        };
        let files = single_chip_files(&index, &root, chip, SAMPLE);
        if files.is_empty() {
            println!("{:<14} no single-chip corpus file", chip.name());
            continue;
        }

        let (mut correlations, mut cents, mut dropouts, mut gains) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for original in &files {
            // Both sides render the same cut file, so the saving costs nothing.
            let path = &shortened(original, &work_dir());
            // **Native rate only where the core is genuinely shared.** There it
            // is the same core at the same rate on both sides and neither
            // resamples. For a clean-room chip "native" is *our* core's rate
            // and means nothing to the reference, which would simply upsample
            // its own core into it -- reintroducing the resampler this is
            // meant to remove, and at up to five times the cost, since some of
            // these cores run above 200 kHz.
            let rate = match bar.regime {
                parity::Regime::SharedCore => native_rate_of(path, chip).unwrap_or(RATE),
                parity::Regime::CleanRoom => RATE,
            };
            let Some(ours) = render_ours_at(path, rate) else {
                continue;
            };
            let Ok(bytes) = reference.at_rate(rate).render(path, &work_dir()) else {
                continue;
            };
            let Ok((samples, rate)) = parity::reference::read_wav(&bytes) else {
                continue;
            };
            let theirs = Render::from_interleaved_i16(&samples, rate);
            let score = parity::compare(&ours, &theirs, Settings::default());
            if score.worst_correlation() < bar.min_correlation {
                // Named after the original: the cut copy lives in a temp
                // directory and its bare stem would not say which rip it came
                // from, which is the one thing a flagged pair has to say.
                parity::dump_pair(&short(original), &ours, &theirs);
            }
            correlations.push(score.worst_correlation());
            dropouts.push(score.worst_dropout());
            gains.push(score.channels[0].gain);
            if let Some(measured) = score.worst_cents() {
                cents.push(measured);
            }
        }
        if correlations.is_empty() {
            println!("{:<14} nothing comparable", chip.name());
            continue;
        }

        // Medians: one dud rip should not decide a chip, and a systematic
        // fault moves every file together anyway.
        let correlation = median(&mut correlations);
        let dropout = median(&mut dropouts);
        let gain = median(&mut gains);
        let detune = (!cents.is_empty()).then(|| median(&mut cents));

        println!(
            "{:<14} {:?}  corr {correlation:.4}  drop {dropout:.3}  gain {gain:.3}  cents {}{}",
            chip.name(),
            bar.regime,
            detune.map_or_else(|| "  --".to_owned(), |c| format!("{c:+.1}")),
            bar.known_gap
                .map_or(String::new(), |why| format!("  [known gap: {why}]")),
        );

        let mut trouble = Vec::new();
        if correlation < bar.min_correlation {
            trouble.push(format!(
                "correlation {correlation:.4} < {:.4}",
                bar.min_correlation
            ));
        }
        if dropout > bar.max_dropout {
            trouble.push(format!("dropout {dropout:.3} > {:.3}", bar.max_dropout));
        }
        if let Some(detune) = detune
            && detune.abs() > bar.max_cents
        {
            trouble.push(format!("detune {detune:+.1} cents > {:.1}", bar.max_cents));
        }
        if !trouble.is_empty() {
            // A known gap is printed and tolerated; anything else is a failure.
            // Never silently skipped -- an expected failure that stops being
            // expected has to be visible.
            match bar.known_gap {
                Some(why) => println!("   tolerated ({why}): {}", trouble.join("; ")),
                None => failures.push(format!("{}: {}", chip.name(), trouble.join("; "))),
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// **pt-6: the balance fit.** Every `OUTPUT_GAIN` in the cores is currently
/// arithmetic that sounds plausible. This measures it instead.
///
/// For a two-chip file, render ours twice with one core withheld each time and
/// solve `a·A + b·B ≈ reference`. The ratio `a / b` is how far our balance sits
/// from the reference's; the residual says whether the fit means anything at
/// all, since two cores that differ in *content* will not add up to the
/// reference's mix however they are scaled.
#[test]
#[ignore = "needs DROTRIM_REF_PLAYER; run explicitly"]
fn the_chip_balance_is_measured_rather_than_guessed() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    // The Mega Drive pair is the first customer: FM against PSG is the balance
    // `opn2.rs` flags as a listening question.
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

    let stride = (both.len() / SAMPLE).max(1);
    let mut ratios = Vec::new();
    for relative in both.iter().step_by(stride).take(SAMPLE) {
        let path = root.join(relative);
        // Two chips means two native rates, so *something* gets resampled
        // whatever rate is chosen -- unlike the single-chip scorecard, where
        // the native rate takes both resamplers out of the measurement
        // entirely. Rendering at the FM chip's rate at least spares the part
        // of the mix carrying most of the energy, and leaves the resampler
        // acting only on the PSG, where it inflates the residual without
        // moving the balance the fit is after.
        let rate = native_rate_of(&path, ChipKind::Ym2612).unwrap_or(RATE);
        let Ok(bytes) = reference.at_rate(rate).render(&path, &work_dir()) else {
            continue;
        };
        let Ok((samples, rate)) = parity::reference::read_wav(&bytes) else {
            continue;
        };
        let theirs = Render::from_interleaved_i16(&samples, rate);

        let registry = dro_synth::registry::registry();
        let Some(fm_only) = render_with_at(&path, rate, move |kind| {
            (kind == ChipKind::Ym2612)
                .then(|| registry.build(kind, None))
                .flatten()
        }) else {
            continue;
        };
        let Some(psg_only) = render_with_at(&path, rate, move |kind| {
            (kind != ChipKind::Ym2612)
                .then(|| registry.build(kind, None))
                .flatten()
        }) else {
            continue;
        };

        let frames = fm_only.len().min(psg_only.len()).min(theirs.len());
        let Some((fm, sn)) = parity::metrics::fit_balance(
            &fm_only.left[..frames],
            &psg_only.left[..frames],
            &theirs.left[..frames],
        ) else {
            continue;
        };
        let left_over = parity::metrics::residual(
            &fm_only.left[..frames],
            &psg_only.left[..frames],
            &theirs.left[..frames],
            (fm, sn),
        );
        println!(
            "  {:<50} fm {fm:.3}  psg {sn:.3}  ratio {:.3}  residual {left_over:.3}",
            short(&path),
            if sn.abs() > 1e-9 { fm / sn } else { f64::NAN },
        );
        // A large residual means the cores differ in content, not in balance,
        // and the ratio would be meaningless.
        if left_over < 0.5 && sn.abs() > 1e-9 {
            ratios.push(fm / sn);
        }
    }

    assert!(
        !ratios.is_empty(),
        "no file produced a usable fit -- every residual was too large, which \
         means the cores differ in content rather than in balance"
    );
    let median_ratio = median(&mut ratios);
    println!(
        "\nFM-to-PSG balance: ours is {median_ratio:.3}x the reference's over {} files.\n\
         A value near 1.0 means the gains are right; otherwise divide opn2.rs's\n\
         OUTPUT_GAIN by it and record the measurement in PROVENANCE.md.",
        ratios.len()
    );
}

/// The end-to-end check that needs **no reference at all**: our own render,
/// through the whole comparison pipeline, against itself.
///
/// It cannot say a core is right. It can say that reading a render, filtering
/// it, aligning it, fitting it and scoring it does not itself introduce a
/// difference -- which is the assumption every other result here rests on, and
/// the one part of the control-group argument that can be made on any machine.
#[test]
#[ignore = "needs DROTRIM_VGMRIPS_CORPUS; run explicitly"]
fn the_pipeline_agrees_with_itself() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    dro_trimmer::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    let mut checked = 0usize;
    for chip in [ChipKind::Ymf262, ChipKind::Ym2612, ChipKind::Sn76489] {
        for path in index.sample(chip, 2) {
            let Some(render) = render_ours(&path) else {
                continue;
            };
            // A render with no sound in it has no correlation to measure --
            // `compare` reports zero for a constant signal, which is right but
            // says nothing about the pipeline. Skipped rather than scored.
            if render.is_empty() || parity::metrics::rms(&render.left) < 1e-6 {
                continue;
            }
            let score = parity::compare(&render, &render, Settings::default());
            assert!(
                score.worst_correlation() > 0.9999,
                "{}: the pipeline disagreed with itself at {}",
                path.display(),
                score.worst_correlation()
            );
            assert_eq!(score.worst_dropout(), 0.0);
            assert_eq!(score.channels[0].lag, 0);
            checked += 1;
        }
    }
    assert!(checked > 0, "nothing was checked");
    println!("the pipeline agreed with itself on {checked} real renders");
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The last two path components, which is enough to recognise a rip.
fn short(path: &Path) -> String {
    let mut parts: Vec<_> = path.components().rev().take(2).collect();
    parts.reverse();
    parts
        .iter()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}
