// SPDX-License-Identifier: GPL-2.0-or-later
//! Comparing our render of a file against a reference player's.
//!
//! A core is unverified until someone listens to it against VGMPlay; this makes
//! most of that mechanical. Every real bug the cores programme shipped was
//! *measurable* -- flat pitch, a silent half, a missing voice, a standing
//! offset, a wrong balance -- so ears are needed for the residual, not for
//! everything.
//!
//! The full reasoning is in `docs/vgm-multichip-2026-07/PARITY-PLAN.md`. In
//! short, every chip this harness bars is **shared-core**: it runs the same
//! upstream emulator (pinned via VGMPlay.ini) in the reference as it does here,
//! so a mismatch is a *driver* bug -- write pacing, routing, a variant flag --
//! and the bar is a near-identity floor. **OPL is the control group**: our OPL
//! core is proven bit-identical to the C the reference runs, so an end-to-end
//! OPL comparison measures only this pipeline. Until it scores near 1.0, the
//! harness is what is broken.

pub mod metrics;
pub mod reference;

use vgms_core::vgm::ChipKind;

pub use metrics::Render;
pub use reference::{Reference, ReferenceError};

/// How far either side of unison the detuning search looks.
///
/// Wide enough to find the AY-class fault (about 27 cents) with room around it,
/// narrow enough that the search stays cheap and cannot wander into a
/// neighbouring harmonic.
const SEARCH_CENTS: f64 = 60.0;

/// Comparison settings, shared by every chip so a change applies everywhere.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Below this a window counts as silent, for both the envelope and the
    /// silence-agreement metrics.
    pub silence_floor: f64,
    /// Envelope window, in seconds. 50 ms is short enough to see a percussion
    /// hit and long enough not to measure the resampler.
    pub window_seconds: f64,
    /// How far the lag search may look, in seconds.
    pub max_lag_seconds: f64,
    /// Where the high-pass sits before comparison. DC is measured separately.
    pub high_pass_hz: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            silence_floor: 1e-4,
            window_seconds: 0.05,
            max_lag_seconds: 0.005,
            high_pass_hz: 20.0,
        }
    }
}

/// What one comparison found, for one channel.
#[derive(Debug, Clone, Copy)]
pub struct ChannelScore {
    pub correlation: f64,
    /// The lag, in frames, at which that correlation was found. A large one is
    /// itself a fault -- a missing lead-in, a wrong loop point.
    pub lag: isize,
    /// The scalar that best maps our render onto the reference's. For a
    /// shared-core chip this *is* the gain correction.
    ///
    /// **Only when the correlation is high.** The least-squares fit is
    /// `α = ρ · σ_reference / σ_ours`, so a decorrelated pair reports a small
    /// α whatever its level: the SN76489's first scorecard row read `corr 0.58
    /// gain 0.55`, which looks like our render being nearly twice too loud and
    /// is nothing of the kind -- the levels were within 5%. Read [`rms_ratio`]
    /// for level and this for level *only* once correlation says the two are
    /// the same waveform.
    ///
    /// [`rms_ratio`]: ChannelScore::rms_ratio
    pub gain: f64,
    /// Our RMS over the reference's: level, uncontaminated by correlation.
    ///
    /// This is the number the balance work wants. 1.0 is agreement, above 1.0
    /// means we are louder.
    pub rms_ratio: f64,
    pub envelope_error: f64,
    pub quieter_windows: usize,
    pub louder_windows: usize,
    /// Windows where we sound and the reference does not.
    pub phantom_rate: f64,
    /// Windows where the reference sounds and we do not.
    pub dropout_rate: f64,
    /// Median pitch difference over windows both sides found pitched, in cents.
    /// `None` when nothing sustained enough was found.
    pub cents: Option<f64>,
    /// Measured before the high-pass, because filtering is how a DC fault
    /// becomes invisible.
    pub our_dc: f64,
    pub reference_dc: f64,
}

/// What one comparison found.
#[derive(Debug, Clone)]
pub struct Score {
    pub channels: [ChannelScore; 2],
    pub frames: usize,
}

impl Score {
    /// The worse of the two channels' correlations -- a chip that is right on
    /// one side and wrong on the other is wrong.
    #[must_use]
    pub fn worst_correlation(&self) -> f64 {
        self.channels[0]
            .correlation
            .min(self.channels[1].correlation)
    }

    /// The worst dropout rate across channels.
    #[must_use]
    pub fn worst_dropout(&self) -> f64 {
        self.channels[0]
            .dropout_rate
            .max(self.channels[1].dropout_rate)
    }

    /// The largest absolute pitch error either channel found.
    #[must_use]
    pub fn worst_cents(&self) -> Option<f64> {
        self.channels
            .iter()
            .filter_map(|channel| channel.cents)
            .max_by(|a, b| a.abs().total_cmp(&b.abs()))
    }

    /// One line for the scorecard.
    #[must_use]
    pub fn summary(&self) -> String {
        let cents = self
            .worst_cents()
            .map_or_else(|| "  --  ".to_owned(), |cents| format!("{cents:+6.1}"));
        format!(
            "corr {:.4}  lvl {:.3}  gain {:.3}  env {:.3}  drop {:.3}  cents {cents}  dc {:+.4}/{:+.4}",
            self.worst_correlation(),
            self.channels[0].rms_ratio,
            self.channels[0].gain,
            self.channels[0].envelope_error,
            self.worst_dropout(),
            self.channels[0].our_dc,
            self.channels[0].reference_dc,
        )
    }
}

/// Optional: a directory to write both sides of a flagged comparison into.
///
/// A score is a pointer, not a diagnosis. PARITY-PLAN's whole bet is that the
/// metrics reduce "audition thirteen chips" to "audition the handful the
/// numbers flagged", and that only works if the flagged pair is *to hand*.
pub const DUMP_ENV: &str = "VGMSTUDIO_PARITY_DUMP";

/// Where flagged pairs should be written, if anywhere.
#[must_use]
pub fn dump_dir() -> Option<std::path::PathBuf> {
    std::env::var_os(DUMP_ENV).map(std::path::PathBuf::from)
}

/// Writes both renders as WAVs named after `label`, for listening.
///
/// Returns where they went, or `None` if dumping is off or the write failed --
/// a diagnostic that cannot be produced must never fail the run that produced
/// the finding.
pub fn dump_pair(label: &str, ours: &Render, reference: &Render) -> Option<std::path::PathBuf> {
    let dir = dump_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    // Path separators and the punctuation rip directories are full of would
    // otherwise scatter these across directories that do not exist.
    let stem: String = label
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    for (suffix, render) in [("ours", ours), ("theirs", reference)] {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: render.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let path = dir.join(format!("{stem}.{suffix}.wav"));
        let mut writer = hound::WavWriter::create(&path, spec).ok()?;
        let [left, right] = render.channels();
        for (l, r) in left.iter().zip(right) {
            for sample in [l, r] {
                let clamped = (sample * f64::from(i16::MAX)).clamp(-32768.0, 32767.0);
                writer.write_sample(clamped as i16).ok()?;
            }
        }
        writer.finalize().ok()?;
    }
    Some(dir.join(format!("{stem}.*.wav")))
}

/// Compares our render against a reference's.
///
/// Both are trimmed to their common length first: a reference may fade or stop
/// at a loop point differently, and comparing past the shorter one measures
/// that rather than the chip.
#[must_use]
pub fn compare(ours: &Render, reference: &Render, settings: Settings) -> Score {
    let frames = ours.len().min(reference.len());
    let rate = reference.sample_rate;
    let window = ((f64::from(rate) * settings.window_seconds) as usize).max(1);
    let max_lag = ((f64::from(rate) * settings.max_lag_seconds) as usize).max(1);

    let mut channels = [None, None];
    for (index, (our_channel, ref_channel)) in ours
        .channels()
        .into_iter()
        .zip(reference.channels())
        .enumerate()
    {
        let ours_cut = &our_channel[..frames.min(our_channel.len())];
        let reference_cut = &ref_channel[..frames.min(ref_channel.len())];

        // Before the filter, so the fault it exists to catch stays visible.
        let our_dc = metrics::dc_offset(ours_cut);
        let reference_dc = metrics::dc_offset(reference_cut);

        let ours_hp = metrics::high_pass(ours_cut, rate, settings.high_pass_hz);
        let reference_hp = metrics::high_pass(reference_cut, rate, settings.high_pass_hz);

        let (correlation, lag) =
            metrics::best_correlation(&ours_hp, &reference_hp, max_lag).unwrap_or((0.0, 0));
        let gain = metrics::fit_gain(&ours_hp, &reference_hp).unwrap_or(0.0);
        let reference_rms = metrics::rms(&reference_hp);
        let rms_ratio = if reference_rms > 1e-12 {
            metrics::rms(&ours_hp) / reference_rms
        } else {
            0.0
        };

        let our_envelope = metrics::envelope(&ours_hp, window);
        let reference_envelope = metrics::envelope(&reference_hp, window);
        let (envelope_error, quieter_windows, louder_windows) =
            metrics::envelope_error(&our_envelope, &reference_envelope, settings.silence_floor);
        let (phantom_rate, dropout_rate) = metrics::silence_disagreement(
            &our_envelope,
            &reference_envelope,
            settings.silence_floor,
        );

        // Detuning, measured per window as a *ratio* between the two renders
        // rather than as the difference of two absolute pitches -- see
        // `detune_cents` for why that is the only form fine enough to assert on
        // and the only one that means anything on polyphonic content.
        //
        // The median rather than the mean: one window that lands on a
        // transient should not move the answer, and a systematic detuning --
        // which is what the AY-class fault looks like -- moves every window
        // together anyway.
        let mut errors: Vec<f64> = Vec::new();
        let pitch_window = (rate as usize / 4).max(1024);
        let usable = ours_hp.len().min(reference_hp.len());
        let mut at = 0;
        while at + pitch_window <= usable {
            if let Some(cents) = metrics::detune_cents(
                &ours_hp[at..at + pitch_window],
                &reference_hp[at..at + pitch_window],
                SEARCH_CENTS,
            ) {
                errors.push(cents);
            }
            at += pitch_window;
        }
        errors.sort_by(f64::total_cmp);
        let cents = (!errors.is_empty()).then(|| errors[errors.len() / 2]);

        channels[index] = Some(ChannelScore {
            correlation,
            lag,
            gain,
            rms_ratio,
            envelope_error,
            quieter_windows,
            louder_windows,
            phantom_rate,
            dropout_rate,
            cents,
            our_dc,
            reference_dc,
        });
    }

    Score {
        channels: [
            channels[0].expect("both channels scored"),
            channels[1].expect("both channels scored"),
        ],
        frames,
    }
}

/// What a chip has to score to pass.
///
/// The bars are regression floors, not certificates: each sits just below its
/// chip's observed n=12 median, so a regression fails loudly. A bar below the
/// ideal is an open investigation with a tripwire under it -- the reason is on
/// the entry (`known_gap`) and in `parity/SCORECARD.md`. Raising a bar when its
/// chip improves is expected; lowering one needs the same evidence that set it.
///
/// Every barred chip is shared-core (see the module docs), so a low score is
/// always a driver or binding fault, never "implementations differ".
#[derive(Debug)]
pub struct Threshold {
    pub chip: ChipKind,
    pub min_correlation: f64,
    pub max_cents: f64,
    pub max_dropout: f64,
    /// Set when a chip is knowingly incomplete or its bar sits below the
    /// ideal, and why. Printed, never silently skipped.
    pub known_gap: Option<&'static str>,
}

/// The per-chip bar. One table, so a change is one diff.
pub const THRESHOLDS: &[Threshold] = &[
    // Shared-core, at the ideal: our upstream on both sides, and it passes.
    shared(ChipKind::Ym2151),
    shared(ChipKind::Ay8910),
    // Shared-core, discounted for free-running state: the n=12 medians include
    // rips that engage the vibrato/tremolo LFO or rhythm-mode noise, whose
    // phase free-runs from reset and cannot match across two players (the
    // scorecard's steady-subset line and the control group carry the
    // near-identity proof instead). SCORECARD 2026-08-12.
    Threshold {
        chip: ChipKind::Ymf262,
        min_correlation: 0.97,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.9898 observed (n=12): free-running state in the sample -- \
             vibrato-heavy rips score down to 0.59 while the control group \
             pins steady files at 0.9978. Not a driver fault",
        ),
    },
    Threshold {
        chip: ChipKind::Ym3812,
        min_correlation: 0.95,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.9771 observed (n=12): free-running state in the sample -- \
             vibrato, deep tremolo and rhythm-mode noise (the LFO-off-but-low \
             files were rhythm rips: the noise LFSR free-runs exactly as the \
             LFO does). Not a driver fault",
        ),
    },
    // Shared-core, below the ideal: the gap is a driver question, tracked in
    // SCORECARD.md, with the floor under the observed score.
    Threshold {
        chip: ChipKind::Ym2612,
        min_correlation: 0.88,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.904 observed (n=12) against a shared core; the LLE oracle puts \
             Nuked-OPN2 at 0.985 against the 2612 die, so the gap to VGMPlay \
             lives in the reference player's driver, not our emulation",
        ),
    },
    Threshold {
        chip: ChipKind::Ym2413,
        min_correlation: 0.95,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.977 observed (n=12) against a shared core; short of the 0.99 \
             ideal, unexplained by the LFO or the resampler",
        ),
    },
    // Measured 2026-08-11 once the libvgm cores had become these chips'
    // defaults: YM2203 corr 0.9999, YM2608 0.9983, YM2610 1.0000, OKIM6295
    // 1.0000 (all n=12, native rate) -- shared-lineage at the ideal, so they
    // take the shared bar. The same run caught the YM2203 rendering at half
    // the reference's level (lvl 0.508), fixed on its specs row.
    shared(ChipKind::Ym2203),
    shared(ChipKind::Ym2608),
    shared(ChipKind::Ym2610),
    shared(ChipKind::Okim6295),
    // The 2026-08-12 whole-roster sweep (SCORECARD "the level sweep"): every
    // row here read at or above the shared ideal against the reference, most
    // at exactly 1.0000. The sweep's level corrections live on the specs rows.
    shared(ChipKind::HuC6280),
    shared(ChipKind::X1010),
    shared(ChipKind::K054539),
    shared(ChipKind::Ymf271),
    shared(ChipKind::QSound),
    shared(ChipKind::Vsu),
    shared(ChipKind::C352),
    shared(ChipKind::Rf5c68),
    shared(ChipKind::K053260),
    shared(ChipKind::C140),
    shared(ChipKind::Ymz280b),
    shared(ChipKind::Ymf278b),
    shared(ChipKind::MultiPcm),
    shared(ChipKind::Upd7759),
    // The sweep's polarity-inversion OPEN finding, closed 2026-08-12: the
    // 0.2605 / fit gain -0.963 was our default row running the MAME core
    // against the reference's Gens core -- the two read the sign-magnitude
    // sample bytes with opposite polarity. With the Gens core as the 164's
    // default (specs.rs), corr 0.9994, lvl 1.000, cents +0.5 (n=12).
    shared(ChipKind::Rf5c164),
    // Shared-lineage but below the ideal, each with its observed score and
    // the band that explains the shortfall.
    Threshold {
        chip: ChipKind::WonderSwan,
        min_correlation: 0.95,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some("0.9888 observed (n=12); just under the ideal, unexplained residual"),
    },
    Threshold {
        chip: ChipKind::Okim6258,
        min_correlation: 0.95,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some("0.9766 observed (n=9); the divider/flag-dependent tail"),
    },
    Threshold {
        chip: ChipKind::Saa1099,
        min_correlation: 0.80,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some("0.8471 observed (n=12); the two noise generators' phase"),
    },
    Threshold {
        chip: ChipKind::Y8950,
        min_correlation: 0.78,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some("0.8287 observed (n=12); the ADPCM speech half"),
    },
    Threshold {
        chip: ChipKind::Sn76489,
        min_correlation: 0.30,
        max_cents: 3.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.358 observed (n=12): the noise/HF band decorrelates the pair \
             (the clean-room era's open item, inherited). The level is pinned \
             two ways instead -- lvl 0.247 measured and 4.0 derived",
        ),
    },
    Threshold {
        chip: ChipKind::Es5503,
        min_correlation: 0.98,
        max_cents: 10.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.9944 observed (n=12, lvl 1.005) once the dynamic-rate fix and \
             the x0.25 staging level landed (2026-08-12: the oscillator-enable \
             register moves the chip's output rate, and with the resampler \
             stuck at the reset rate the sweep read corr 0.0022). Residual: a \
             flat offset, median -6.5 cents across the sample -- our rate \
             arithmetic matches upstream exactly, so the suspect is the \
             reference's older core revision (the MAME core's loop-phase \
             handling changed in v2.1); untested",
        ),
    },
    Threshold {
        chip: ChipKind::Ym3526,
        min_correlation: 0.70,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: Some(
            "0.7533 observed (n=12) since the OPL adapter's clock projection \
             (was 0.0312: every sampled rip is a 4 MHz or 3 MHz arcade board \
             the adapter used to play at the standard crystal). The reference \
             offers no Nuked option for the YM3526 -- it always plays MAME \
             fmopl -- so the band is the cross-core one, like its Y8950 \
             sibling's",
        ),
    },
];

/// A shared-core chip's bar: near-identity, because a gap is a driver fault.
const fn shared(chip: ChipKind) -> Threshold {
    Threshold {
        chip,
        min_correlation: 0.99,
        max_cents: 2.0,
        max_dropout: 0.01,
        known_gap: None,
    }
}

/// The bar for `chip`, if this harness has one.
#[must_use]
pub fn threshold_for(chip: ChipKind) -> Option<&'static Threshold> {
    THRESHOLDS.iter().find(|entry| entry.chip == chip)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chip with a core should have a bar, or it is being compared
    /// against nothing. Nuked-CQM is the deliberate exception -- the reference
    /// does not ship it, so it stays a listening-only core.
    #[test]
    fn every_threshold_names_a_distinct_chip_and_a_sane_bar() {
        let mut seen = std::collections::HashSet::new();
        for entry in THRESHOLDS {
            assert!(seen.insert(entry.chip), "{:?} listed twice", entry.chip);
            assert!(
                (0.0..=1.0).contains(&entry.min_correlation),
                "{:?}: correlation bar out of range",
                entry.chip
            );
            assert!(entry.max_cents > 0.0 && entry.max_dropout >= 0.0);
            // Every barred chip runs *our* upstream on both sides, so its bar is
            // near-identity -- or the entry says, on itself, why it is temporarily
            // not. A silent discount is what this test forbids: a regression floor
            // below the ideal must carry its reason, so the table cannot quietly
            // become a list of tolerated bugs.
            if entry.min_correlation < 0.99 {
                assert!(
                    entry.known_gap.is_some(),
                    "{:?} sits below the 0.99 ideal with no stated reason",
                    entry.chip
                );
            }
        }
    }

    /// **A low `gain` does not mean a level difference.** The least-squares fit
    /// is `α = ρ · σ_reference / σ_ours`, so decorrelation drags it down on its
    /// own. The SN76489's first scorecard row read `corr 0.58  gain 0.55` and
    /// was briefly read as our render being nearly twice too loud; the levels
    /// were within 5%. `rms_ratio` is the number that answers the level
    /// question, and this is the case that separates them.
    #[test]
    fn a_level_difference_and_a_decorrelation_are_told_apart() {
        let rate = 44_100;
        fn render(rate: u32, hz: f64, amplitude: f64) -> Render {
            let samples: Vec<i16> = (0..rate)
                .flat_map(|index| {
                    let t = f64::from(index) / f64::from(rate);
                    let value = (amplitude * (2.0 * std::f64::consts::PI * hz * t).sin()) as i16;
                    [value, value]
                })
                .collect();
            Render::from_interleaved_i16(&samples, rate)
        }

        // Same waveform, half the level: both numbers say so, and agree.
        let quiet = compare(
            &render(rate, 440.0, 6_000.0),
            &render(rate, 440.0, 12_000.0),
            Settings::default(),
        );
        assert!(quiet.worst_correlation() > 0.999);
        assert!(
            (quiet.channels[0].gain - 2.0).abs() < 0.01,
            "{}",
            quiet.channels[0].gain
        );
        assert!((quiet.channels[0].rms_ratio - 0.5).abs() < 0.01);

        // Same level, unrelated content: `gain` collapses, `rms_ratio` does
        // not. Reading the first as a level error is the trap.
        let other = compare(
            &render(rate, 440.0, 12_000.0),
            &render(rate, 997.0, 12_000.0),
            Settings::default(),
        );
        assert!(other.worst_correlation().abs() < 0.5);
        assert!(
            other.channels[0].gain.abs() < 0.5,
            "a decorrelated fit is small: {}",
            other.channels[0].gain
        );
        assert!(
            (other.channels[0].rms_ratio - 1.0).abs() < 0.02,
            "but the levels are equal: {}",
            other.channels[0].rms_ratio
        );
    }

    /// Comparing a render with itself must be a perfect score. If this fails,
    /// the pipeline is wrong and no chip result means anything -- the same
    /// argument the OPL control group makes, in a form that needs no reference.
    #[test]
    fn a_render_against_itself_scores_perfectly() {
        let rate = 44_100;
        let samples: Vec<i16> = (0..rate)
            .flat_map(|index| {
                let t = f64::from(index) / f64::from(rate);
                let value = (12_000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
                [value, value]
            })
            .collect();
        let render = Render::from_interleaved_i16(&samples, rate);

        let score = compare(&render, &render, Settings::default());
        assert!(
            score.worst_correlation() > 0.9999,
            "self-comparison scored {}",
            score.worst_correlation()
        );
        assert_eq!(score.channels[0].lag, 0);
        assert!((score.channels[0].gain - 1.0).abs() < 1e-6);
        assert!((score.channels[0].rms_ratio - 1.0).abs() < 1e-6);
        assert_eq!(score.worst_dropout(), 0.0);
        assert!(
            score.worst_cents().is_none_or(|cents| cents.abs() < 0.5),
            "self-comparison found a pitch difference"
        );
    }

    /// And the converse: a render that is plainly wrong must not score well.
    /// A metric that cannot fail is not a metric.
    #[test]
    fn a_silent_render_against_a_loud_one_fails_every_way() {
        let rate = 44_100;
        let loud: Vec<i16> = (0..rate)
            .flat_map(|index| {
                let t = f64::from(index) / f64::from(rate);
                let value = (12_000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
                [value, value]
            })
            .collect();
        let reference = Render::from_interleaved_i16(&loud, rate);
        let ours = Render::from_interleaved_i16(&vec![0i16; loud.len()], rate);

        let score = compare(&ours, &reference, Settings::default());
        assert!(
            score.worst_correlation() < 0.5,
            "silence must not correlate"
        );
        assert!(score.worst_dropout() > 0.9, "and must read as a dropout");
    }
}
