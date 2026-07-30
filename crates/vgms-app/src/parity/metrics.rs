//! How two renders of the same file are compared.
//!
//! Hand-rolled and self-tested, because these numbers decide whether a core is
//! accepted: a metric that is subtly wrong passes a broken core, which is worse
//! than having no metric at all. Every function here has a test that feeds it a
//! signal whose answer is known by construction -- a sine detuned by exactly
//! ten cents, a gain of exactly two, a channel dropped outright.
//!
//! # Why these metrics and not others
//!
//! Sample-wise L2 is deliberately absent. Two correct players resample
//! differently, so their transients smear differently, and L2 punishes that
//! about as hard as it punishes a genuinely wrong note. Everything here is
//! either scale-invariant, alignment-tolerant, or measured in a unit where the
//! audible threshold is known (cents).
//!
//! Each metric maps to a bug class this project has actually shipped:
//!
//! | Metric | Caught, or would have caught |
//! |---|---|
//! | [`cents_error`] | The AY's counter reloading a tick late -- every note flat by ~27 cents |
//! | [`envelope`] comparison | The OKIM6295's untriggerable fourth voice; the OPN family's absent ADPCM |
//! | [`silence_disagreement`] | The YM2203 rendering nothing at all |
//! | [`dc_offset`] | The DC blockers' fixed point on the negative half |
//! | [`fit_gain`] / [`fit_balance`] | Every `OUTPUT_GAIN` that is currently a guess |

/// One decoded render: interleaved is unhelpful for comparison, so channels are
/// split.
#[derive(Debug, Clone)]
pub struct Render {
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub sample_rate: u32,
}

impl Render {
    /// Splits interleaved stereo samples, scaling to `-1.0..=1.0`.
    #[must_use]
    pub fn from_interleaved_i16(samples: &[i16], sample_rate: u32) -> Self {
        let scale = f64::from(i16::MAX);
        let mut left = Vec::with_capacity(samples.len() / 2);
        let mut right = Vec::with_capacity(samples.len() / 2);
        for frame in samples.chunks_exact(2) {
            left.push(f64::from(frame[0]) / scale);
            right.push(f64::from(frame[1]) / scale);
        }
        Self {
            left,
            right,
            sample_rate,
        }
    }

    /// Frames.
    #[must_use]
    pub fn len(&self) -> usize {
        self.left.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    /// The two channels, so a caller can run every metric per side.
    #[must_use]
    pub fn channels(&self) -> [&[f64]; 2] {
        [&self.left, &self.right]
    }

    /// Both channels truncated to `frames`.
    #[must_use]
    pub fn truncated(&self, frames: usize) -> Self {
        let frames = frames.min(self.len());
        Self {
            left: self.left[..frames].to_vec(),
            right: self.right[..frames].to_vec(),
            sample_rate: self.sample_rate,
        }
    }
}

/// The mean sample value: a standing offset, which is its own bug class.
///
/// Measured *before* any high-pass, and reported separately, because filtering
/// it away is exactly how a DC-blocker fault becomes invisible.
#[must_use]
pub fn dc_offset(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

/// A one-pole high-pass, applied to both sides before comparison.
///
/// DC policy legitimately differs between players -- ours blocks it in some
/// cores and not others -- so comparing it would report a difference that is
/// not a fault. [`dc_offset`] is what reports the fault.
#[must_use]
pub fn high_pass(samples: &[f64], sample_rate: u32, cutoff_hz: f64) -> Vec<f64> {
    if samples.is_empty() || sample_rate == 0 {
        return samples.to_vec();
    }
    // The standard one-pole difference equation; `alpha` near 1 is a gentle
    // slope, which is all that is wanted below 20 Hz.
    let dt = 1.0 / f64::from(sample_rate);
    let rc = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz);
    let alpha = rc / (rc + dt);
    let mut out = Vec::with_capacity(samples.len());
    let mut previous_in = samples[0];
    let mut previous_out = 0.0;
    for &sample in samples {
        previous_out = alpha * (previous_out + sample - previous_in);
        previous_in = sample;
        out.push(previous_out);
    }
    out
}

/// Root mean square: the loudness of a window.
#[must_use]
pub fn rms(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

/// The scalar `a` minimising `‖a·x − y‖²`.
///
/// For a shared-core chip this *is* the gain correction, and worth reporting
/// on its own rather than only dividing it out. Returns `None` when `x` is
/// silent, since any gain fits silence equally badly.
#[must_use]
pub fn fit_gain(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len().min(y.len());
    if n == 0 {
        return None;
    }
    let (mut xy, mut xx) = (0.0, 0.0);
    for index in 0..n {
        xy += x[index] * y[index];
        xx += x[index] * x[index];
    }
    (xx > 1e-12).then(|| xy / xx)
}

/// Pearson correlation, scale-invariant by construction.
///
/// `None` when either side is constant -- correlation with silence is
/// undefined, not zero, and reporting zero would look like a total mismatch.
#[must_use]
pub fn correlation(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len().min(y.len());
    if n < 2 {
        return None;
    }
    let mean_x = x[..n].iter().sum::<f64>() / n as f64;
    let mean_y = y[..n].iter().sum::<f64>() / n as f64;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for index in 0..n {
        let (dx, dy) = (x[index] - mean_x, y[index] - mean_y);
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    (sxx > 1e-12 && syy > 1e-12).then(|| sxy / (sxx * syy).sqrt())
}

/// The best correlation within ±`max_lag` frames, and the lag that achieved it.
///
/// Two players can start a stream a buffer apart without either being wrong, so
/// a zero-lag comparison would report a phase difference as a content
/// difference. The search is bounded: a *large* lag is a real fault (a missing
/// lead-in, a wrong loop point), so the bound is what keeps this from hiding
/// one.
#[must_use]
pub fn best_correlation(x: &[f64], y: &[f64], max_lag: usize) -> Option<(f64, isize)> {
    let mut best: Option<(f64, isize)> = None;
    for lag in -(max_lag as isize)..=(max_lag as isize) {
        let (a, b) = if lag >= 0 {
            let offset = lag as usize;
            if offset >= x.len() {
                continue;
            }
            (&x[offset..], y)
        } else {
            let offset = (-lag) as usize;
            if offset >= y.len() {
                continue;
            }
            (x, &y[offset..])
        };
        let Some(score) = correlation(a, b) else {
            continue;
        };
        if best.is_none_or(|(current, _)| score > current) {
            best = Some((score, lag));
        }
    }
    best
}

/// How far the best alignment moves between the start of the file and the end,
/// in frames, together with the correlation each end achieves.
///
/// **This separates the two ways a whole-file correlation goes soft.** A
/// *resampler* difference costs correlation evenly and leaves the alignment
/// where it was. A *rate* difference -- one side running the chip at a clock
/// the other does not -- slides the two apart as the file plays, so every
/// window is individually well-aligned while the file as a whole is not.
/// Telling those apart by ear is essentially impossible; the two numbers here
/// make it a glance.
///
/// `None` when either end is too short or too quiet to align.
#[must_use]
pub fn lag_drift(x: &[f64], y: &[f64], max_lag: usize) -> Option<(isize, f64, isize, f64)> {
    let common = x.len().min(y.len());
    // A quarter each end, so the two samples are as far apart as the file
    // allows while each stays long enough to align confidently.
    let window = common / 4;
    if window < max_lag * 4 {
        return None;
    }
    let head = best_correlation(&x[..window], &y[..window], max_lag)?;
    let tail_from = common - window;
    let tail = best_correlation(&x[tail_from..common], &y[tail_from..common], max_lag)?;
    Some((head.1, head.0, tail.1, tail.0))
}

/// Per-window RMS, the shape of a render over time.
///
/// This is what catches a missing voice or an absent percussion section: the
/// waveform can correlate poorly for benign reasons while the *envelope* still
/// agrees, and it can also agree in pitch while plainly missing a part.
#[must_use]
pub fn envelope(samples: &[f64], window: usize) -> Vec<f64> {
    if window == 0 {
        return Vec::new();
    }
    samples.chunks(window).map(rms).collect()
}

/// How much of the reference's loudness is missing from ours, and vice versa.
///
/// Returns `(mean relative error, ours quieter, ours louder)` over windows
/// where the reference is audible. The two directions are separated because
/// they mean different things: quieter is a dropped voice, louder is usually
/// a gain or a phantom.
#[must_use]
pub fn envelope_error(ours: &[f64], reference: &[f64], floor: f64) -> (f64, usize, usize) {
    let n = ours.len().min(reference.len());
    let (mut total, mut counted, mut quieter, mut louder) = (0.0, 0usize, 0usize, 0usize);
    for index in 0..n {
        let (a, b) = (ours[index], reference[index]);
        if b < floor {
            continue;
        }
        counted += 1;
        total += ((a - b) / b).abs();
        // A tenth is well inside resampler and dither differences; beyond that
        // is a part, not a nuance.
        if a < b * 0.9 {
            quieter += 1;
        } else if a > b * 1.1 {
            louder += 1;
        }
    }
    let mean = if counted > 0 {
        total / counted as f64
    } else {
        0.0
    };
    (mean, quieter, louder)
}

/// Windows where one side is silent and the other is not, as a rate.
///
/// Returns `(phantom, dropout)`: the fraction of windows where we sound and the
/// reference does not, and the converse. A YM2203 that renders nothing scores
/// 1.0 dropout, which no amount of correlation tolerance can explain away.
#[must_use]
pub fn silence_disagreement(ours: &[f64], reference: &[f64], floor: f64) -> (f64, f64) {
    let n = ours.len().min(reference.len());
    if n == 0 {
        return (0.0, 0.0);
    }
    let (mut phantom, mut dropout) = (0usize, 0usize);
    for index in 0..n {
        let (a, b) = (ours[index] >= floor, reference[index] >= floor);
        match (a, b) {
            (true, false) => phantom += 1,
            (false, true) => dropout += 1,
            _ => {}
        }
    }
    (phantom as f64 / n as f64, dropout as f64 / n as f64)
}

/// The dominant period of a window, in frames, by autocorrelation.
///
/// `None` for a window with no clear periodicity -- noise, silence, a
/// transient. Callers measure pitch only on windows where *both* sides find
/// one, so an unpitched passage is skipped rather than scored as a mismatch.
#[must_use]
pub fn dominant_period(samples: &[f64], min_period: usize, max_period: usize) -> Option<f64> {
    let n = samples.len();
    if n < max_period * 2 || min_period < 1 || min_period >= max_period {
        return None;
    }
    let mean = samples.iter().sum::<f64>() / n as f64;
    let centred: Vec<f64> = samples.iter().map(|s| s - mean).collect();
    let energy: f64 = centred.iter().map(|s| s * s).sum();
    if energy < 1e-9 {
        return None;
    }

    // Normalised per lag by the terms actually summed. Without that, a longer
    // period sums fewer products and scores lower for arithmetic reasons rather
    // than musical ones -- which biases the answer in a way that is invisible
    // until it is not.
    let score_at = |period: usize| -> f64 {
        let terms = n.saturating_sub(period);
        if terms == 0 {
            return 0.0;
        }
        let mut sum = 0.0;
        for index in 0..terms {
            sum += centred[index] * centred[index + period];
        }
        sum / (energy * terms as f64 / n as f64)
    };

    let mut scores: Vec<(usize, f64)> = (min_period..=max_period)
        .map(|period| (period, score_at(period)))
        .collect();
    let peak = scores
        .iter()
        .map(|(_, score)| *score)
        .fold(f64::MIN, f64::max);
    // A weak peak means the window was not pitched -- noise, silence, a
    // transient. Skipped rather than scored, since a pitch comparison of two
    // unpitched windows is noise on both sides.
    if peak < 0.3 {
        return None;
    }
    // **The octave fix.** A periodic signal correlates just as well at twice
    // and three times its period, so taking the maximum picks an arbitrary
    // multiple -- and an octave error reads as a 1200-cent bug that is not
    // there. The shortest period scoring near the peak is the fundamental.
    scores.retain(|(_, score)| *score >= peak * 0.9);
    let period = scores
        .first()
        .map(|(period, _)| *period)
        .expect("the peak itself always qualifies");

    // Parabolic interpolation around the integer peak, because a whole-frame
    // period is far too coarse for cents: at 44100 Hz a 100-frame period is
    // 441 Hz and 101 frames is 437 Hz, seventeen cents apart.
    if period > min_period && period < max_period {
        let (before, here, after) = (score_at(period - 1), score_at(period), score_at(period + 1));
        let denominator = 2.0 * (2.0f64.mul_add(here, -before) - after);
        if denominator.abs() > 1e-12 {
            let shift = (after - before) / denominator;
            if shift.abs() < 1.0 {
                return Some(period as f64 + shift);
            }
        }
    }
    Some(period as f64)
}

/// The interval between two periods, in cents. Positive means `ours` is sharp.
///
/// Cents because that is the unit the audible threshold is known in: about five
/// is the limit of most listeners' discrimination on a sustained tone, and the
/// AY bug was twenty-seven.
#[must_use]
pub fn cents_error(our_period: f64, reference_period: f64) -> Option<f64> {
    (our_period > 0.0 && reference_period > 0.0)
        .then(|| 1200.0 * (reference_period / our_period).log2())
}

/// `samples` read back at `ratio` times the rate, linearly interpolated.
///
/// A ratio above one reads faster and so sounds sharper. Only ever used over a
/// window small enough that linear interpolation's own error stays far below
/// the pitch differences being measured.
#[must_use]
fn resampled(samples: &[f64], ratio: f64) -> Vec<f64> {
    if samples.is_empty() || ratio <= 0.0 {
        return Vec::new();
    }
    let out_len = ((samples.len() as f64) / ratio).floor() as usize;
    (0..out_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let at = position.floor() as usize;
            let fraction = position - at as f64;
            let a = samples[at.min(samples.len() - 1)];
            let b = samples[(at + 1).min(samples.len() - 1)];
            a + (b - a) * fraction
        })
        .collect()
}

/// How far `ours` is detuned from `reference`, in cents. Positive is sharp.
///
/// **Measured as a ratio between the two renders rather than as the difference
/// of two absolute pitches**, for two reasons. Estimating each side's period
/// independently is only as fine as one frame -- at a 25-frame period that is
/// nearly seventy cents, far coarser than the five this harness wants to
/// assert at. And real VGM output is polyphonic, where "the" pitch is not well
/// defined but "these two renders differ by a constant factor" is exactly the
/// AY-class fault.
///
/// Resamples `ours` at candidate ratios and keeps the one that correlates best,
/// so the resolution is the search step rather than the frame. `None` when the
/// two do not correlate well enough for an answer to mean anything.
#[must_use]
pub fn detune_cents(ours: &[f64], reference: &[f64], search_cents: f64) -> Option<f64> {
    let n = ours.len().min(reference.len());
    if n < 512 {
        return None;
    }
    // Gated on the *best* correlation the search achieves, not on the
    // unshifted one. A genuinely detuned pair correlates badly at unison --
    // that is what being detuned means, and over a window long enough to
    // measure ten cents the phase has drifted a whole cycle -- so a baseline
    // gate would reject precisely the case this function exists for.
    let mut best: Option<(f64, f64)> = None;
    let mut cents = -search_cents;
    while cents <= search_cents {
        // A sharp render has to be read back slower to match, hence the sign.
        let candidate = resampled(&ours[..n], 2f64.powf(cents / 1200.0));
        let common = candidate.len().min(n);
        if common >= 512
            && let Some(score) = correlation(&candidate[..common], &reference[..common])
            && best.is_none_or(|(current, _)| score > current)
        {
            best = Some((score, cents));
        }
        cents += 0.5;
    }
    // The winning ratio is the correction, so the error is its negation. A
    // best that is still poor means the two renders do not share content at
    // all, and any ratio would fit them equally badly.
    best.filter(|(score, _)| *score > 0.5)
        .map(|(_, cents)| -cents)
}

/// The `(a, b)` minimising `‖a·first + b·second − mix‖²`.
///
/// The balance fit: render our engine twice with one core withheld each time,
/// and the ratio `a / b` is how far our chip balance is from the reference's.
/// That is what turns an `OUTPUT_GAIN` from a guess into a measurement.
///
/// `None` when the two sources are collinear -- which they are if one is
/// silent, and then no balance is determined.
#[must_use]
pub fn fit_balance(first: &[f64], second: &[f64], mix: &[f64]) -> Option<(f64, f64)> {
    let n = first.len().min(second.len()).min(mix.len());
    if n == 0 {
        return None;
    }
    let (mut aa, mut ab, mut bb, mut ar, mut br) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for index in 0..n {
        let (a, b, r) = (first[index], second[index], mix[index]);
        aa += a * a;
        ab += a * b;
        bb += b * b;
        ar += a * r;
        br += b * r;
    }
    // Normal equations for two sources: [aa ab; ab bb] [x; y] = [ar; br].
    let determinant = aa * bb - ab * ab;
    if determinant.abs() < 1e-12 {
        return None;
    }
    Some((
        (ar * bb - br * ab) / determinant,
        (br * aa - ar * ab) / determinant,
    ))
}

/// What is left over after a fit, relative to the mix's own energy.
///
/// A balance fit with a large residual is not a balance at all -- it means the
/// two renders do not add up to the reference's, so the *cores* differ and the
/// ratio is meaningless. Reported alongside every fit for that reason.
#[must_use]
pub fn residual(first: &[f64], second: &[f64], mix: &[f64], gains: (f64, f64)) -> f64 {
    let n = first.len().min(second.len()).min(mix.len());
    if n == 0 {
        return 0.0;
    }
    let (mut error, mut energy) = (0.0, 0.0);
    for index in 0..n {
        let predicted = gains.0 * first[index] + gains.1 * second[index];
        error += (predicted - mix[index]).powi(2);
        energy += mix[index] * mix[index];
    }
    if energy < 1e-12 {
        return 0.0;
    }
    (error / energy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    /// A sine of `hz`, `seconds` long, at `amplitude`.
    fn sine(hz: f64, seconds: f64, amplitude: f64) -> Vec<f64> {
        let n = (f64::from(RATE) * seconds) as usize;
        (0..n)
            .map(|index| {
                let t = index as f64 / f64::from(RATE);
                amplitude * (2.0 * std::f64::consts::PI * hz * t).sin()
            })
            .collect()
    }

    /// The distinction the control group turns on: a *rate* difference slides
    /// the two renders apart as they play, while an even, phase-invariant
    /// difference leaves them where they were. A whole-file correlation cannot
    /// tell those apart; this is what does.
    #[test]
    fn drift_is_distinguished_from_an_even_difference() {
        // Aperiodic, or every lag would look as good as every other -- but
        // *band-limited*, because linear interpolation mangles white noise
        // beyond recognition and the test would then measure that instead.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let raw: Vec<f64> = (0..RATE as usize * 4 + 64)
            .map(|_| {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                ((seed >> 33) as f64 / f64::from(u32::MAX >> 1)) - 1.0
            })
            .collect();
        let source: Vec<f64> = raw
            .windows(64)
            .map(|window| window.iter().sum::<f64>() / 64.0)
            .collect();

        // One side running 200 ppm fast -- the scale of a real clock
        // disagreement, not a gross one. Each window stays alignable; where
        // they align moves.
        let fast = resampled(&source, 1.0002);
        let (head, head_score, tail, _) =
            lag_drift(&source, &fast, 200).expect("both are long enough");
        assert!(
            head_score > 0.85,
            "the head still aligns well: {head_score}"
        );
        assert!(
            (tail - head).abs() > 8,
            "a rate difference must show as movement, got {head} then {tail}"
        );

        // The same signal, attenuated: a difference in level, not in time.
        let quiet: Vec<f64> = source.iter().map(|s| s * 0.5).collect();
        let (head, _, tail, _) = lag_drift(&source, &quiet, 200).expect("long enough");
        assert_eq!((head, tail), (0, 0), "nothing has moved");
    }

    #[test]
    fn a_gain_fit_recovers_the_gain_it_was_given() {
        let x = sine(440.0, 0.2, 0.5);
        let y: Vec<f64> = x.iter().map(|s| s * 2.5).collect();
        let fitted = fit_gain(&x, &y).expect("a non-silent source");
        assert!((fitted - 2.5).abs() < 1e-9, "fitted {fitted}");

        // Silence determines no gain, rather than an arbitrary one.
        assert!(fit_gain(&vec![0.0; 100], &y).is_none());
    }

    #[test]
    fn correlation_is_blind_to_scale_and_offset() {
        let x = sine(440.0, 0.2, 0.5);
        let scaled: Vec<f64> = x.iter().map(|s| s * 7.0 + 0.3).collect();
        let score = correlation(&x, &scaled).expect("both vary");
        assert!((score - 1.0).abs() < 1e-9, "score {score}");

        // And an inverted copy is a perfect *negative* -- polarity is a fault
        // this must not hide.
        let inverted: Vec<f64> = x.iter().map(|s| -s).collect();
        let score = correlation(&x, &inverted).expect("both vary");
        assert!((score + 1.0).abs() < 1e-9, "score {score}");
    }

    /// Two players can start a buffer apart without either being wrong; a large
    /// lag is a real fault, so the search is bounded.
    ///
    /// Measured on deterministic noise rather than a tone. A periodic signal
    /// correlates just as well at any whole number of periods, so "the" lag is
    /// not defined for it -- and because correlation is scale-invariant, even a
    /// decaying tone matches a much later slice of itself. Noise has one
    /// alignment and no other.
    #[test]
    fn the_lag_search_finds_a_shift_and_reports_it() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let x: Vec<f64> = (0..8192)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 40) as f64 / 8388608.0) - 1.0
            })
            .collect();

        let shifted: Vec<f64> = x[64..].to_vec();
        let (score, lag) = best_correlation(&x, &shifted, 128).expect("a match");
        assert!(score > 0.999, "score {score}");
        assert_eq!(lag, 64, "the shift must be reported, not just absorbed");

        // Beyond the bound it cannot be recovered, which is the point: a large
        // offset is a real fault, not an alignment nuance to absorb.
        let far: Vec<f64> = x[4000..].to_vec();
        let (score, _) = best_correlation(&x, &far, 16).expect("a comparison");
        assert!(
            score < 0.5,
            "a large shift must not score as a match: {score}"
        );
    }

    /// **The metric that would have caught the AY bug.** Ten cents is a
    /// deliberate choice: it is above the ~5 cent threshold this harness
    /// asserts at and below what most listeners reliably hear, so a metric that
    /// can measure it can measure the thing ears miss.
    #[test]
    fn detuning_is_measured_in_cents_to_within_a_cent() {
        for applied in [10.0, -10.0, 3.0, 0.0] {
            let reference = sine(440.0, 0.4, 0.5);
            let ours = sine(440.0 * 2f64.powf(applied / 1200.0), 0.4, 0.5);
            let measured = detune_cents(&ours, &reference, 40.0).expect("a pitched pair");
            assert!(
                (measured - applied).abs() < 1.0,
                "measured {measured} cents where {applied} were applied"
            );
        }
    }

    /// Two signals with nothing in common have no detuning to report -- an
    /// answer would be noise, and a median of noise is still noise.
    #[test]
    fn unrelated_signals_report_no_detuning() {
        assert!(detune_cents(&sine(440.0, 0.4, 0.5), &vec![0.0; 17_640], 40.0).is_none());
        assert!(detune_cents(&vec![0.0; 512], &vec![0.0; 512], 40.0).is_none());
    }

    /// The AY's own bug, reconstructed: a period one tick too long. At period
    /// 64 that is about 27 cents -- comfortably measurable, and exactly the
    /// sort of thing a listener shrugs at.
    #[test]
    fn the_period_off_by_one_bug_is_visible() {
        let correct = 1_789_773.0 / (16.0 * 64.0);
        let flat = 1_789_773.0 / (16.0 * 65.0);
        let measured = detune_cents(&sine(flat, 0.4, 0.5), &sine(correct, 0.4, 0.5), 60.0)
            .expect("a pitched pair");
        assert!(
            (-30.0..-24.0).contains(&measured),
            "an off-by-one period measured {measured} cents"
        );
    }

    #[test]
    fn an_unpitched_window_reports_no_period() {
        // A constant is not pitched, and neither is silence.
        assert!(dominant_period(&vec![0.5; 4000], 20, 800).is_none());
        assert!(dominant_period(&vec![0.0; 4000], 20, 800).is_none());
    }

    /// **The metric that would have caught the missing OKI voice.** A part
    /// dropped from the mix shows as a systematic quieter-than-reference
    /// envelope, even when what remains correlates well.
    #[test]
    fn a_dropped_part_shows_as_a_quieter_envelope() {
        let full: Vec<f64> = sine(440.0, 0.5, 0.4)
            .iter()
            .zip(sine(660.0, 0.5, 0.4).iter())
            .map(|(a, b)| a + b)
            .collect();
        let missing_one = sine(440.0, 0.5, 0.4);

        let window = RATE as usize / 20; // 50 ms
        let (error, quieter, louder) = envelope_error(
            &envelope(&missing_one, window),
            &envelope(&full, window),
            1e-4,
        );
        assert!(
            error > 0.1,
            "a missing part should not be a nuance: {error}"
        );
        assert!(quieter > 0, "and it should read as quieter");
        assert_eq!(louder, 0);

        // The same signal against itself is not an error at all.
        let (error, quieter, louder) =
            envelope_error(&envelope(&full, window), &envelope(&full, window), 1e-4);
        assert!(error < 1e-9 && quieter == 0 && louder == 0, "{error}");
    }

    /// **The metric that would have caught the silent YM2203.** No correlation
    /// tolerance explains a render that is simply not there.
    #[test]
    fn a_silent_render_scores_a_total_dropout() {
        let window = RATE as usize / 20;
        let reference = envelope(&sine(440.0, 0.5, 0.4), window);
        let ours = envelope(&vec![0.0; RATE as usize / 2], window);

        let (phantom, dropout) = silence_disagreement(&ours, &reference, 1e-3);
        assert!(dropout > 0.99, "dropout {dropout}");
        assert_eq!(phantom, 0.0);

        // And the converse direction is reported separately, because a phantom
        // is a different fault from a dropout.
        let (phantom, dropout) = silence_disagreement(&reference, &ours, 1e-3);
        assert!(phantom > 0.99, "phantom {phantom}");
        assert_eq!(dropout, 0.0);
    }

    /// **The metric that would have caught the DC-blocker fixed point.** It is
    /// measured before the high-pass, because filtering is how that fault
    /// becomes invisible.
    #[test]
    fn a_standing_offset_is_measured_before_it_is_filtered() {
        let offset: Vec<f64> = sine(440.0, 0.3, 0.2).iter().map(|s| s + 0.05).collect();
        assert!((dc_offset(&offset) - 0.05).abs() < 1e-3);

        let filtered = high_pass(&offset, RATE, 20.0);
        assert!(
            dc_offset(&filtered).abs() < 1e-3,
            "the high-pass should remove it -- which is why the measurement \
             comes first"
        );
        // And the high-pass must leave the audible content alone.
        let score = correlation(
            &filtered[RATE as usize / 10..],
            &offset[RATE as usize / 10..],
        )
        .expect("both vary");
        assert!(score > 0.99, "the high-pass mangled the signal: {score}");
    }

    /// **The balance fit**: two known sources at known gains must be recovered.
    #[test]
    fn the_balance_fit_recovers_two_gains() {
        let fm = sine(220.0, 0.4, 0.5);
        let psg = sine(1030.0, 0.4, 0.5);
        let mix: Vec<f64> = fm
            .iter()
            .zip(psg.iter())
            .map(|(a, b)| 1.5 * a + 0.4 * b)
            .collect();

        let (a, b) = fit_balance(&fm, &psg, &mix).expect("independent sources");
        assert!((a - 1.5).abs() < 1e-6, "fm gain {a}");
        assert!((b - 0.4).abs() < 1e-6, "psg gain {b}");
        assert!(residual(&fm, &psg, &mix, (a, b)) < 1e-6);
    }

    /// A fit whose residual is large is not a balance -- the sources do not add
    /// up to the mix, so the cores themselves differ and the ratio means
    /// nothing. Reported for exactly that reason.
    #[test]
    fn a_fit_against_an_unrelated_mix_reports_a_large_residual() {
        let fm = sine(220.0, 0.4, 0.5);
        let psg = sine(1030.0, 0.4, 0.5);
        let unrelated = sine(517.0, 0.4, 0.5);

        let gains = fit_balance(&fm, &psg, &unrelated).expect("independent sources");
        let left_over = residual(&fm, &psg, &unrelated, gains);
        assert!(left_over > 0.5, "residual {left_over} should be large");
    }

    /// Collinear sources determine no balance, rather than an arbitrary one.
    #[test]
    fn collinear_sources_have_no_balance() {
        let one = sine(440.0, 0.2, 0.5);
        let same: Vec<f64> = one.iter().map(|s| s * 2.0).collect();
        assert!(fit_balance(&one, &same, &one).is_none());
        assert!(fit_balance(&one, &vec![0.0; one.len()], &one).is_none());
    }

    #[test]
    fn a_render_splits_its_channels_and_scales_them() {
        let interleaved = [i16::MAX, i16::MIN, 0, i16::MAX];
        let render = Render::from_interleaved_i16(&interleaved, RATE);
        assert_eq!(render.len(), 2);
        assert!((render.left[0] - 1.0).abs() < 1e-4);
        assert!((render.right[0] + 1.0).abs() < 1e-3);
        assert_eq!(render.left[1], 0.0);
        assert_eq!(render.truncated(1).len(), 1);
    }
}
