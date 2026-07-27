// SPDX-License-Identifier: GPL-2.0-or-later
//! Comparing our render of a file against a reference player's.
//!
//! `CORES-PLAN` §6.2 has always said a core is unverified until someone listens
//! to it against VGMPlay. This is what makes most of that mechanical. Every
//! real bug the cores programme shipped was *measurable* -- flat pitch, a
//! silent half, a missing voice, a standing offset, a wrong balance -- so ears
//! are needed for the residual, not for everything.
//!
//! The full reasoning, the two regimes and the step list are in
//! `docs/vgm-multichip-2026-07/PARITY-PLAN.md`. In short:
//!
//! - **Shared-core chips** (YM2612, YM2151, YM2413, OPL) run the same Nuked
//!   upstream in the reference as they do here, so a mismatch is a *driver*
//!   bug -- write pacing, routing, a variant flag -- and the bar is high.
//! - **Clean-room chips** face a different implementation, so the comparison is
//!   statistical and the bar is a frozen band rather than a near-identity.
//! - **OPL is the control group.** Our OPL core is proven bit-identical to the
//!   C the reference runs, so an end-to-end OPL comparison measures only this
//!   pipeline. Until it scores near 1.0, the harness is what is broken.

pub mod metrics;
pub mod reference;

use dro_core::vgm::ChipKind;

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
pub const DUMP_ENV: &str = "DROTRIM_PARITY_DUMP";

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

/// Which regime a chip's comparison falls into.
///
/// Not a detail: it decides what a number *means*. A correlation of 0.95 is a
/// bug on the left and unremarkable on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// The reference runs the same upstream core we do, so the two should agree
    /// closely and a gap is a driver fault.
    SharedCore,
    /// The reference runs a different implementation, so differences are
    /// expected and only *systematic* ones are faults.
    CleanRoom,
}

/// What a chip has to score to pass.
///
/// **Frozen 2026-07-28 as regression floors, not certificates.** pt-5's
/// calibration ran, the outliers were investigated (every one of the first
/// scorecard's disasters turned out to be a real, fixable bug -- see
/// `parity/SCORECARD.md` for the chronicle), and each bar now sits just below
/// its chip's observed n=12 median. A bar below the ideal is an *open
/// investigation with a tripwire under it*: the reason is on the entry, the
/// investigation is in SCORECARD.md, and a regression from the observed level
/// fails loudly. Raising a bar when its chip improves is expected; lowering
/// one requires the same standard of evidence that set it.
pub struct Threshold {
    pub chip: ChipKind,
    pub regime: Regime,
    pub min_correlation: f64,
    pub max_cents: f64,
    pub max_dropout: f64,
    /// The envelope's mean relative error, where a bar has been measured.
    ///
    /// The metric that arbitrates chips whose waveform phase is
    /// implementation-defined: VGMPlay's two HuC6280 cores score -0.19
    /// against *each other* on correlation while agreeing to 0.96 on
    /// envelope, so for that class of chip correlation certifies nothing and
    /// this does.
    pub max_envelope: Option<f64>,
    /// Set when a chip is knowingly incomplete or its bar sits below the
    /// ideal, and why. Printed, never silently skipped.
    pub known_gap: Option<&'static str>,
}

/// The per-chip bar. One table, so a change is one diff.
pub const THRESHOLDS: &[Threshold] = &[
    // Shared-core, at the ideal: our upstream on both sides, and it passes.
    shared(ChipKind::Ymf262),
    shared(ChipKind::Ym3812),
    shared(ChipKind::Ym2151),
    // Shared-core, below the ideal: the gap is a driver question, tracked in
    // SCORECARD.md, with the floor under the observed score.
    Threshold {
        chip: ChipKind::Ym2612,
        regime: Regime::SharedCore,
        min_correlation: 0.88,
        max_cents: 2.0,
        max_dropout: 0.01,
        max_envelope: None,
        known_gap: Some(
            "0.904 observed (n=12) against a shared core; a driver-level              difference under investigation -- the ideal here is 0.99",
        ),
    },
    Threshold {
        chip: ChipKind::Ym2413,
        regime: Regime::SharedCore,
        min_correlation: 0.95,
        max_cents: 2.0,
        max_dropout: 0.01,
        max_envelope: None,
        known_gap: Some(
            "0.977 observed (n=12) against a shared core; short of the 0.99              ideal, unexplained by the LFO or the resampler",
        ),
    },
    // Clean-room: floors under the observed band. The family lands near 0.6
    // against a different implementation; the two far below that carry their
    // own investigations.
    clean(
        ChipKind::Sn76489,
        0.55,
        "HF/noise-band disagreement under investigation; tones match to 1%",
    ),
    clean(
        ChipKind::Ay8910,
        0.55,
        "with the SN76489 in the ~0.6 clean-room band",
    ),
    clean(
        ChipKind::Ym2203,
        0.60,
        "a different FM core on the reference side",
    ),
    clean(
        ChipKind::Ym2608,
        0.55,
        "ADPCM-A rhythm and ADPCM-B are not modelled",
    ),
    clean(
        ChipKind::Ym2610,
        0.55,
        "ADPCM-A rhythm and ADPCM-B are not modelled",
    ),
    clean(
        ChipKind::Okim6258,
        0.50,
        "0.557 observed (n=9); clean-room codec differences",
    ),
    clean(
        ChipKind::Okim6295,
        0.60,
        "0.676 observed after the bank and divider fixes",
    ),
    clean(
        ChipKind::GameBoyDmg,
        0.25,
        "0.295 observed -- far below where a clean-room square-wave chip          should land; under investigation",
    ),
    clean(
        ChipKind::NesApu,
        0.30,
        "0.334 observed -- far below where a clean-room square-wave chip          should land; under investigation",
    ),
    // Correlation certifies nothing here: wave phase is implementation-defined
    // and VGMPlay's own two HuC6280 cores score -0.19 against each other. The
    // envelope is the arbitrating metric; its bar is set once the scorecard
    // has printed a measured value.
    Threshold {
        chip: ChipKind::HuC6280,
        regime: Regime::CleanRoom,
        min_correlation: 0.0,
        max_cents: 5.0,
        max_dropout: 0.05,
        max_envelope: None,
        known_gap: Some(
            "whole-file correlation is meaningless for this chip: channel wave              phase is implementation-defined, and the reference's two HuC6280              cores score -0.19 against each other. Envelope agreement is 0.97,              the same as between the references themselves",
        ),
    },
];

/// A shared-core chip's bar: near-identity, because a gap is a driver fault.
const fn shared(chip: ChipKind) -> Threshold {
    Threshold {
        chip,
        regime: Regime::SharedCore,
        min_correlation: 0.99,
        max_cents: 2.0,
        max_dropout: 0.01,
        max_envelope: None,
        known_gap: None,
    }
}

/// A clean-room chip's bar: a floor under the observed band, with the reason.
const fn clean(chip: ChipKind, min_correlation: f64, why: &'static str) -> Threshold {
    Threshold {
        chip,
        regime: Regime::CleanRoom,
        min_correlation,
        max_cents: 5.0,
        max_dropout: 0.05,
        max_envelope: None,
        known_gap: Some(why),
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
            // A shared-core chip runs *our* upstream on both sides, so its
            // bar is near-identity -- or the entry says, on itself, why it is
            // temporarily not. A silent discount is what this test forbids: a
            // regression floor below the ideal must carry its reason, so the
            // table cannot quietly become a list of tolerated bugs.
            if entry.regime == Regime::SharedCore && entry.min_correlation < 0.99 {
                assert!(
                    entry.known_gap.is_some(),
                    "{:?} shares a core and sits below the 0.99 ideal with no                      stated reason",
                    entry.chip
                );
            }
            // The same rule for a clean-room floor below the family's band.
            if entry.regime == Regime::CleanRoom && entry.min_correlation < 0.55 {
                assert!(
                    entry.known_gap.is_some(),
                    "{:?} sits below the clean-room band with no stated reason",
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
