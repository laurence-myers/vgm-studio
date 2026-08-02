//! Our render against a reference player's, chip by chip.
//!
//! The mechanical half of the acceptance bar; the reasoning and step list are in
//! `docs/vgm-multichip-2026-07/PARITY-PLAN.md`, this is the harness.
//!
//! ```text
//! VGMSTUDIO_VGMRIPS_CORPUS=F:/GameMusic/VGM/VGMRips_all_of_them_2025-10-17 \
//! VGMSTUDIO_REF_PLAYER=/path/to/vgmplay \
//! VGMSTUDIO_REF_ARGS="-o" \
//!     cargo test -p vgms-app --release --test reference_parity -- --ignored --nocapture
//! ```
//!
//! **Without a reference player every test here skips**, saying so. The
//! pipeline's own correctness does not depend on one: `parity::metrics` is
//! self-tested, and `the_pipeline_agrees_with_itself` closes the loop with no
//! external binary at all.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vgms_app::corpus::{self, ChipIndex};
use vgms_app::parity::{self, Reference, ReferenceError, Render, Settings};
use vgms_core::vgm::ChipKind;
use vgms_synth::vgm_engine::VgmEngine;

/// The rate both sides render at unless a chip's native rate is asked for.
const RATE: u32 = 44_100;
/// How much of each file to compare. Long enough for an envelope to develop,
/// short enough that a dozen files per chip is minutes rather than an hour.
const SECONDS: usize = 20;
/// Files per chip.
const SAMPLE: usize = 12;

/// Renders `path` the way the app would.
///
/// Which engine matters: an OPL file plays through `PlayerEngine` (which carries
/// the OPL register policy); `VgmEngine` has no OPL core and would render it as
/// silence.
fn render_ours(path: &Path) -> Option<Render> {
    render_ours_at(path, RATE)
}

/// The same, at a caller-chosen rate -- so a chip can be compared at the rate
/// it natively runs at and neither side's resampler enters the measurement.
fn render_ours_at(path: &Path, rate: u32) -> Option<Render> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    if let Some(song) = file.to_song() {
        // The OPL path, exactly as the WAV export uses it.
        let wav = vgms_synth::render_wav(&song, rate, 16).ok()?;
        let (samples, wav_rate) = parity::reference::read_wav(&wav).ok()?;
        let wanted = rate as usize * SECONDS * 2;
        let samples = &samples[..samples.len().min(wanted)];
        return Some(Render::from_interleaved_i16(samples, wav_rate));
    }

    render_with_at(path, rate, build_core)
}

/// Builds `kind`'s core, honouring `VGMSTUDIO_PARITY_CORE`.
///
/// A reused core takes a chip's default only once it beats the frozen row it
/// would replace, so this harness must be able to point at a non-default core.
/// The variable names a provider suffix, not a full id:
///
/// ```text
/// VGMSTUDIO_PARITY_CORE=libvgm  cargo test ... --test reference_parity -- --ignored
/// ```
///
/// `resolve_choice` composes it with each chip's slot (`sn76489` + `libvgm` →
/// `sn76489.libvgm`) and falls back to the default for any chip that provider
/// does not serve. Unset, this is the registry's default per chip.
fn build_core(kind: ChipKind) -> Option<Box<dyn vgms_synth::ChipCore>> {
    let registry = vgms_synth::registry::registry();
    match std::env::var("VGMSTUDIO_PARITY_CORE") {
        Ok(choice) if !choice.is_empty() => registry.resolve_choice(kind, Some(&choice))?.build(),
        _ => registry.build(kind, None),
    }
}

/// Whether `chip` is in `VGMSTUDIO_PARITY_CHIPS`, which defaults to all of them.
///
/// Judging one core swap means re-measuring one chip; naming slugs turns an hour
/// into a minute.
///
/// ```text
/// VGMSTUDIO_PARITY_CHIPS=sn76489,ym2612  cargo test ... -- --ignored
/// ```
fn chip_wanted(chip: ChipKind) -> bool {
    let Ok(list) = std::env::var("VGMSTUDIO_PARITY_CHIPS") else {
        return true;
    };
    if list.trim().is_empty() {
        return true;
    }
    list.split(',')
        .map(str::trim)
        .any(|slug| slug.eq_ignore_ascii_case(chip.slug()))
}

/// Renders `path` at `rate` with a caller-chosen set of cores -- the
/// decomposition the balance fit needs.
fn render_with_at(
    path: &Path,
    rate: u32,
    cores: impl Fn(ChipKind) -> Option<Box<dyn vgms_synth::ChipCore>> + Send + 'static,
) -> Option<Render> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    file.stream()?;

    let mut engine = VgmEngine::with_cores(Arc::new(file), rate, cores);
    // VGMSTUDIO_PARITY_RESAMPLER=linear renders our side with the same aliased
    // conversion VGMPlay uses, making a 44100 comparison like-for-like. Unset,
    // the accurate default stands.
    if let Some(mode) = std::env::var("VGMSTUDIO_PARITY_RESAMPLER")
        .ok()
        .and_then(|slug| vgms_synth::resample::ResampleMode::from_slug(&slug))
    {
        engine.set_resample_mode(mode);
    }
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
/// A single-chip file is what makes a global gain fit legitimate: with two chips
/// in the mix, one scalar cannot describe both.
fn single_chip_files(index: &ChipIndex, root: &Path, chip: ChipKind, want: usize) -> Vec<PathBuf> {
    let all = index.files(chip);
    // Strided rather than from the head: the index is in directory order, so the
    // first dozen entries are one game's soundtrack -- a sample of one driver,
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
        let Ok(file) = vgms_core::vgm::file::read(&name, &bytes) else {
            continue;
        };
        if file.header.chips().len() != 1 {
            continue;
        }
        // The reference applies the header volume modifier and the extra header's
        // per-chip volumes; our engine applies neither, so a file using them would
        // show a gain difference that is real but not the chip's. Filter them out.
        if file.header.volume_modifier() != 0 {
            continue;
        }
        if file
            .header
            .extra()
            .is_some_and(|extra| !extra.volumes.is_empty())
        {
            continue;
        }
        found.push(path);
    }
    found
}

/// A copy of `path` cut to just past the compared window, or `path` itself if
/// already short enough or it will not walk.
///
/// The reference renders whole tracks; we compare twenty seconds. Cutting the
/// input is what makes the saving real -- there is no "render only the first N
/// seconds" switch. Both sides get the *same* cut file, so it cannot bias a
/// comparison; a couple of seconds past the window are kept so nothing near the
/// boundary is measured against a decay one side was cut off from.
fn shortened(path: &Path, work_dir: &Path) -> PathBuf {
    use vgms_core::vgm::stream::VgmCommand;

    let keep_samples = (SECONDS as u32 + 2) * 44_100;
    let Ok(bytes) = std::fs::read(path) else {
        return path.to_owned();
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let Ok(mut file) = vgms_core::vgm::file::read(&name, &bytes) else {
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
    let Ok(out) = vgms_core::vgm::file::write(&file) else {
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
/// Not a constant: a core's native rate derives from the header clock (a YM2151
/// at 3.58 MHz renders at 55930 Hz, at 4 MHz it does not), so it must be asked
/// per file. Comparing at anything else puts two resamplers between the cores and
/// measures those instead.
fn native_rate_of(path: &Path, chip: ChipKind) -> Option<u32> {
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    let clocked = file.header.chips().iter().find(|c| c.kind == chip)?;
    // The core under test, not the default: two cores for one chip need not
    // agree on a native rate, and asking the default while rendering through a
    // challenger puts our resampler back into the measurement (measuring libvgm's
    // SN76489 at the clean-room rate showed -3.5 cents of resampler pitch error).
    let mut core = build_core(chip)?;
    core.reset(clocked.clock, clocked.variant);
    Some(core.native_rate())
}

/// What fraction of an OPL file's operator writes switch vibrato on.
///
/// Our OPL core and the reference's are the same Nuked source and agree to three
/// or four decimals at native rate -- except on files that use vibrato, where
/// correlation falls off (roughly: 0% -> 0.998, 16% -> 0.94, 46% -> 0.59). The
/// chip's vibrato LFO free-runs from reset and the two sides start it at
/// different points, so each partial's instantaneous frequency wobbles on a
/// different schedule; the average pitch is identical (0.0 cents), so nothing is
/// out of tune, the waveforms simply are not the same. No pipeline work closes
/// it, so vibrato is used as a filter for the assertion, not a correction.
fn vibrato_share(path: &Path) -> Option<f64> {
    modulation_share(path, ChipKind::Ymf262)
}

/// The same question for whichever chip has an answer: what share of the writes
/// that *could* switch the chip's LFO on actually do.
///
/// Each chip hides it somewhere different, and only the chips whose rule is
/// written here can be asked -- the rest return `None`, which reads as "this
/// outlier is not explained by an LFO" rather than "there is no LFO". A rule
/// that guessed would be worse than no rule: it would explain away a real bug.
fn modulation_share(path: &Path, chip: ChipKind) -> Option<f64> {
    use vgms_core::vgm::stream::VgmCommand;
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let file = vgms_core::vgm::file::read(&name, &bytes).ok()?;
    let stream = file.stream()?;

    // (registers that carry the switch, the bits that are the switch)
    let (registers, switch): (std::ops::RangeInclusive<u16>, u16) = match chip {
        // The operator's AM/VIB/EG/KSR/MULT byte; bit 6 is vibrato.
        ChipKind::Ymf262 | ChipKind::Ym3812 => (0x20..=0x35, 0x40),
        // Per-channel L/R/AMS/PMS; the low three bits are PMS, and a channel
        // with PMS 0 is untouched by the LFO however the LFO is running.
        ChipKind::Ym2612 => (0xb4..=0xb6, 0x07),
        // Per-channel PMS (bits 6-4) and AMS (bits 1-0).
        ChipKind::Ym2151 => (0x38..=0x3f, 0x73),
        _ => return None,
    };

    let (mut operators, mut vibrato) = (0usize, 0usize);
    for command in (0..stream.len()).filter_map(|index| stream.get(index)) {
        if let VgmCommand::Write { addr, data, .. } = command
            && registers.contains(&(addr & 0xff))
        {
            operators += 1;
            if data & switch != 0 {
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
    std::env::temp_dir().join("vgmstudio-parity")
}

/// Shortening a file must not change what we render from it.
///
/// The claim that the same cut file "cannot bias a comparison" holds on OPL but
/// not everywhere: OKIM6258, OKIM6295 and HuC6280 rendered *nothing* from their
/// cut copies while the reference played them (a property of the cut, not the
/// cores -- all three pass the audibility suite on the originals). So it is a
/// test now, not a comment.
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn shortening_a_file_does_not_change_what_we_render() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = vgms_synth::registry::registry();

    let mut trouble = Vec::new();
    for chip in ChipKind::all().filter(|chip| registry.can_build(*chip)) {
        for original in single_chip_files(&index, &root, chip, 2) {
            let cut = shortened(&original, &work_dir());
            if cut == original {
                continue;
            }
            let (Some(whole), Some(part)) =
                (render_ours_at(&original, RATE), render_ours_at(&cut, RATE))
            else {
                continue;
            };
            // The cut keeps two seconds more than the window, so the compared
            // span is identical -- only the tail beyond it is gone.
            let frames = whole.len().min(part.len());
            let score = parity::compare(
                &whole.truncated(frames),
                &part.truncated(frames),
                Settings::default(),
            );
            println!(
                "{:<14} {:<44} frames {}/{}  corr {:.4}",
                chip.name(),
                short(&original),
                part.len(),
                whole.len(),
                score.worst_correlation()
            );
            if part.is_empty() || score.worst_correlation() < 0.999 {
                trouble.push(format!(
                    "{}: {} renders {} frames cut against {} whole, correlating {:.4}",
                    chip.name(),
                    short(&original),
                    part.len(),
                    whole.len(),
                    score.worst_correlation()
                ));
            }
        }
    }
    assert!(
        trouble.is_empty(),
        "cutting a file changed what we render from it, so every score \
         measured through a cut copy is suspect:\n{}",
        trouble.join("\n")
    );
}

/// The reference must be a fixed point, or every threshold downstream is noise
/// -- and the symptom would look like flaky cores rather than a flaky reference.
///
/// Checked per chip, because determinism is not a property of the player: a
/// YMF262+YMZ280B rip had the reference disagree with itself on 0.9% of samples
/// at full scale -- a PCM chip reading sample memory it never wrote. A player
/// deterministic for FM and a coin toss for PCM would quietly widen the very
/// thresholds meant to catch our bugs.
#[test]
#[ignore = "needs VGMSTUDIO_REF_PLAYER; run explicitly"]
fn the_reference_player_is_deterministic() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = vgms_synth::registry::registry();

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

/// The control group. Our OPL core is bit-identical to the C the reference runs
/// (`c-parity`), so anything less than a near-perfect score here is the *harness*
/// -- resampling, alignment, gain fitting -- not a chip. Nothing else in this
/// file means anything until this passes.
///
/// Asserted on vibrato-free files, where "the same core driven the same way" is
/// actually true (see [`vibrato_share`]); vibrato-using files are still rendered
/// and printed, because a difference that is explained still has to stay visible.
#[test]
#[ignore = "needs VGMSTUDIO_REF_PLAYER; run explicitly"]
fn the_opl_control_group_calibrates_the_pipeline() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));

    let files = single_chip_files(&index, &root, ChipKind::Ymf262, SAMPLE);
    assert!(!files.is_empty(), "no single-chip OPL file in the corpus");

    // Both sides at the chip's own rate. At 44100 this scored 0.84 on a
    // bit-identical core, with alignment sliding several frames across the file
    // -- two resamplers disagreeing. At 49716 neither side converts anything.
    let native = vgms_synth::NATIVE_SAMPLE_RATE;
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

/// There is no test here comparing our 44100 render against the reference's, on
/// purpose. VGMPlay runs a chip well above 44100 and *linearly interpolates*
/// down -- the exact aliasing this branch removed from our engine -- so at 44100
/// the reference is the one aliasing, and agreeing with it would be the bug. A
/// filter-both-sides variant is also unsound: filtering strips the band where
/// two cores agree and correlation falls legitimately. The sound approach is the
/// synthetic-probe tier `PARITY-PLAN` §2 specifies; until then the filter's
/// evidence is `vgms_synth::resample`'s own tests.
///
/// **The scorecard, against the frozen per-chip bar.** The first run against a
/// new reference is expected to *fail* and be read as a table: the thresholds in
/// `parity::THRESHOLDS` are provisional until the outliers have been listened to.
#[test]
#[ignore = "needs VGMSTUDIO_REF_PLAYER; run explicitly"]
fn every_cored_chip_matches_the_reference_within_its_band() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = vgms_synth::registry::registry();

    let mut failures = Vec::new();
    // How many files were actually compared. A run that compared nothing -- the
    // reference player missing, the corpus empty -- must not report PASS by
    // asserting an empty failure list over an empty scorecard.
    let mut total_compared = 0usize;
    for chip in ChipKind::all() {
        if !registry.can_build(chip) || !chip_wanted(chip) {
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

        let (mut correlations, mut cents, mut dropouts, mut gains, mut levels) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut unmodulated: Vec<f64> = Vec::new();
        for original in &files {
            // Both sides render the same cut file, so the saving costs nothing.
            let path = &shortened(original, &work_dir());
            // Every chip at its own rate. Comparing at 44100 measures our
            // resampler, which for a chip running several times that is the
            // loudest thing in the measurement: the SN76489 scores 0.5848 at
            // 44100 and 0.9958 at its own 223721, the YM2612 0.9538 against
            // 0.9949. That is a real engine fault (see SCORECARD.md), not the one
            // this scorecard attributes to a core. Costly, and unavoidably so:
            // the reference renders at that rate too, and `compare`'s cents search
            // is quadratic in it.
            let rate = if std::env::var_os("VGMSTUDIO_PARITY_AT_OUTPUT_RATE").is_some() {
                RATE
            } else {
                native_rate_of(path, chip).unwrap_or(RATE)
            };
            let Some(ours) = render_ours_at(path, rate) else {
                continue;
            };
            // A reference-player error is a hard failure, not a silent skip: the
            // reference is the oracle, so a comparison it could not produce leaves
            // the scorecard measuring less than it reports.
            let bytes = match reference.at_rate(rate).render(path, &work_dir()) {
                Ok(bytes) => bytes,
                Err(error) => {
                    failures.push(format!(
                        "{}: the reference player failed on {}: {error}",
                        chip.name(),
                        short(original)
                    ));
                    continue;
                }
            };
            let (samples, rate) = match parity::reference::read_wav(&bytes) {
                Ok(parsed) => parsed,
                Err(error) => {
                    failures.push(format!(
                        "{}: the reference player's output for {} would not parse: {error}",
                        chip.name(),
                        short(original)
                    ));
                    continue;
                }
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
            levels.push(score.channels[0].rms_ratio);
            // Files that leave the chip's LFO alone are scored separately:
            // that is where "the same core driven the same way" is actually
            // true, and where a shared-core bar can mean near-identity. See
            // `modulation_share`.
            if modulation_share(path, chip) == Some(0.0) {
                unmodulated.push(score.worst_correlation());
            }
            if let Some(measured) = score.worst_cents() {
                cents.push(measured);
            }
        }
        if correlations.is_empty() {
            println!("{:<14} nothing comparable", chip.name());
            continue;
        }
        total_compared += correlations.len();

        // Medians: one dud rip should not decide a chip, and a systematic
        // fault moves every file together anyway.
        let correlation = median(&mut correlations);
        let dropout = median(&mut dropouts);
        let gain = median(&mut gains);
        // Level is what the balance work reads; `gain` conflates level with
        // correlation and only means what it says once correlation is high.
        let level = median(&mut levels);
        let steady = (!unmodulated.is_empty()).then(|| median(&mut unmodulated));
        let detune = (!cents.is_empty()).then(|| median(&mut cents));

        // The sample size travels with the number: a small-sample median read as
        // a chip's score has misled this programme more than once.
        println!(
            "{:<14}  corr {correlation:.4} (n={})  lvl {level:.3}  gain {gain:.3}  drop {dropout:.3}  cents {}{}",
            chip.name(),
            correlations.len(),
            detune.map_or_else(|| "  --".to_owned(), |c| format!("{c:+.1}")),
            bar.known_gap
                .map_or(String::new(), |why| format!("  [known gap: {why}]")),
        );
        if let Some(steady) = steady {
            println!(
                "   {} of {} files leave the LFO off; those score {steady:.4}",
                unmodulated.len(),
                correlations.len()
            );
        }

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

    assert!(
        total_compared > 0,
        "the scorecard compared nothing -- the reference player or corpus is not \
         producing renders, so an empty failure list means nothing"
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The balance fit. Every `OUTPUT_GAIN` in the cores is currently plausible-
/// sounding arithmetic; this measures it instead.
///
/// For a two-chip file, render ours twice with one core withheld each time and
/// solve `a·A + b·B ≈ reference`. The ratio `a / b` is how far our balance sits
/// from the reference's; the residual says whether the fit means anything, since
/// two cores that differ in *content* will not add up however they are scaled.
#[test]
#[ignore = "needs VGMSTUDIO_REF_PLAYER; run explicitly"]
fn the_chip_balance_is_measured_rather_than_guessed() {
    let Some(reference) = reference() else { return };
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
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

        let registry = vgms_synth::registry::registry();
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
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn the_pipeline_agrees_with_itself() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
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

/// How far a core's level may sit from its chip's default before the picker is
/// a volume control. A tenth is about 0.8 dB -- under what a listener calls a
/// level change, and wide enough that two emulators rounding differently do not
/// trip it.
const LEVEL_BAND: f64 = 0.10;

/// How much scatter across files still counts as "one scalar describes the
/// difference". Above this the cores differ in more than level, and
/// [`CoreInfo::level`] is the wrong tool -- see the YM2413 row.
///
/// [`CoreInfo::level`]: vgms_synth::CoreInfo::level
const LEVEL_SPREAD: f64 = 1.25;

/// Every core for one chip renders it at the same loudness.
///
/// Changing the core in Settings is a choice about *accuracy*; if it also moves
/// the fader, then a multi-chip rip's balance depends on which emulators
/// happen to be selected -- picking libvgm for a Mega Drive rip's YM2612 used
/// to drop its FM 6 dB under its PSG. [`CoreInfo::level`] is the correction,
/// and this is the measurement that sizes it and then keeps it sized.
///
/// No reference player: this is a question about our cores agreeing with each
/// other, so the chip's default row is the datum and the reference only enters
/// through the default's own calibration.
///
/// [`CoreInfo::level`]: vgms_synth::CoreInfo::level
#[test]
#[ignore = "needs VGMSTUDIO_VGMRIPS_CORPUS; run explicitly"]
fn every_core_for_a_chip_agrees_on_its_level() {
    let Some(root) = corpus::corpus_root() else {
        eprintln!("{} not set; skipping", corpus::CORPUS_ENV);
        return;
    };
    vgms_app::install_cores();
    let index = ChipIndex::open_or_build(&root, &corpus::cache_path(&root));
    let registry = vgms_synth::registry::registry();

    let mut failures = Vec::new();
    for chip in ChipKind::all() {
        if !chip_wanted(chip) {
            continue;
        }
        // Realtime rows only. The LLE die sims render orders of magnitude
        // slower than realtime, and their calibration rides the Nuked core's
        // own gain rather than a row of its own.
        let ids: Vec<&str> = registry
            .for_chip(chip)
            .filter(|info| info.realtime && info.build().is_some())
            .map(|info| info.id)
            .collect();
        if ids.len() < 2 {
            continue;
        }
        let files = single_chip_files(&index, &root, chip, SAMPLE);
        if files.is_empty() {
            println!("{:<14} no single-chip corpus file", chip.name());
            continue;
        }

        // One row per core: its RMS over the default core's, file by file.
        let mut ratios: Vec<Vec<f64>> = vec![Vec::new(); ids.len()];
        for original in &files {
            let path = &shortened(original, &work_dir());
            // One rate for every core of this chip -- the default's -- rather
            // than each core's own. This measures level, not pitch, and a
            // common rate keeps our resampler's contribution identical on both
            // sides of the ratio instead of leaving it in one of them.
            let rate = native_rate_of(path, chip).unwrap_or(RATE);
            let Some(base) = render_with_core(path, rate, chip, ids[0]) else {
                continue;
            };
            let base = parity::metrics::rms(&base.left);
            if base < 1e-6 {
                continue; // a silent render fixes no ratio
            }
            for (at, id) in ids.iter().enumerate() {
                let Some(render) = render_with_core(path, rate, chip, id) else {
                    continue;
                };
                ratios[at].push(parity::metrics::rms(&render.left) / base);
            }
        }

        for (at, id) in ids.iter().enumerate() {
            if ratios[at].is_empty() {
                println!("{:<14} {id:<28} nothing measured", chip.name());
                continue;
            }
            let n = ratios[at].len();
            let level = median(&mut ratios[at]);
            let (low, high) = (ratios[at][0], ratios[at][n - 1]);
            let spread = if low > 1e-9 { high / low } else { f64::MAX };
            println!(
                "{:<14} {id:<28} lvl {level:.4} [{low:.4}..{high:.4}] (n={n})",
                chip.name()
            );
            if (level - 1.0).abs() <= LEVEL_BAND {
                continue;
            }
            // A row that scatters is reported and not failed: a scalar cannot
            // fix it, so demanding one would only invite a wrong constant.
            if spread > LEVEL_SPREAD {
                println!(
                    "   off by {:.0}% but scattered {spread:.2}x -- not one scalar's worth of \
                     difference; left alone deliberately",
                    (level - 1.0) * 100.0
                );
                continue;
            }
            failures.push(format!(
                "{}: {id} renders at {level:.3} of {} -- set its `level` to {}",
                chip.name(),
                ids[0],
                (f64::from(vgms_synth::LEVEL_UNITY) / level).round() as u32
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Renders `path` at `rate` with `id` driving `chip` and nothing else voiced.
/// The files are single-chip, so that is the whole render.
fn render_with_core(path: &Path, rate: u32, chip: ChipKind, id: &'static str) -> Option<Render> {
    render_with_at(path, rate, move |kind| {
        (kind == chip)
            .then(|| vgms_synth::registry::registry().find(kind, id))
            .flatten()
            .and_then(vgms_synth::CoreInfo::build)
    })
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
