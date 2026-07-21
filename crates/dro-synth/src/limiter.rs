//! A boost-and-limit stage for *live playback only*.
//!
//! DRO captures vary a lot in loudness. [`BoostLimiter`] multiplies the signal
//! by a user-chosen factor and then rides a peak limiter so the boosted signal
//! can never clip or wrap. It works on the engine's interleaved-stereo i16
//! frames, in `f32` internally, and is bit-transparent at unity boost.
//!
//! It lives here in `dro-synth` -- the wasm-clean, no-I/O DSP crate -- rather
//! than in `dro-audio-native`, so the future web `AudioWorkletProcessor` (Step
//! 9) can reuse it. The WAV render and the waveform display deliberately do
//! *not* run through it: those must stay faithful to the un-boosted signal.

/// Full scale. Deliberately 32_767, not 32_768: at unity boost the limiter is
/// idle and the samples pass through untouched, and a limited peak lands at
/// exactly `+32_767` / `-32_767`, never `i16::MIN`'s unmatched `-32_768`.
const THRESHOLD: f32 = 32_767.0;
/// One-pole release time constant: how quickly the gain climbs back to unity
/// after a loud passage has pulled it down.
const RELEASE_SECONDS: f32 = 0.15;
/// Once the smoothed gain is within this of unity, unity boost re-enables the
/// bit-transparent bypass.
const SETTLED: f32 = 0.999;

/// Boosts and peak-limits interleaved-stereo i16 frames in place.
///
/// The attack is instant (the very frame that would overshoot is already capped)
/// and the release is a one-pole recovery toward unity, so a transient ducks the
/// gain and it slides smoothly back up. The gain is stereo-linked -- both
/// channels share one gain -- so the stereo image is preserved.
#[derive(Debug, Clone)]
pub struct BoostLimiter {
    boost: f32,
    /// The smoothed gain currently applied, in `(0, 1]`.
    gain: f32,
    /// Per-frame decay of the distance between the gain and unity.
    release_decay: f32,
}

impl BoostLimiter {
    /// Builds a limiter for a stream running at `sample_rate`, applying `boost`.
    ///
    /// `sample_rate` must be the stream's *actual* negotiated rate, so the
    /// release time is correct regardless of what the device chose.
    #[must_use]
    pub fn new(sample_rate: u32, boost: f32) -> Self {
        Self {
            boost: boost.max(0.0),
            gain: 1.0,
            release_decay: (-1.0 / (RELEASE_SECONDS * sample_rate as f32)).exp(),
        }
    }

    /// Changes the boost live, carrying the current gain envelope over so the
    /// change cannot click.
    pub fn set_boost(&mut self, boost: f32) {
        self.boost = boost.max(0.0);
    }

    /// Boosts and limits interleaved stereo `samples` in place, returning whether
    /// the limiter **engaged** -- whether any frame's boosted peak overshot full
    /// scale and had to be pulled down.
    ///
    /// The playback UI uses that flag to cap the boost at the level where
    /// clipping starts: once a loud passage drives the signal into the limiter,
    /// raising the boost further only squashes harder. A bypassed unity pass and
    /// an attenuating (`boost < 1.0`) pass never engage.
    ///
    /// A trailing odd sample (there should never be one -- the engine renders
    /// whole stereo frames) is left untouched.
    pub fn process(&mut self, samples: &mut [i16]) -> bool {
        if self.boost == 1.0 && self.gain >= SETTLED {
            // Unity boost with a settled gain is a no-op; pass the samples
            // through bit-for-bit rather than round-trip them through f32.
            self.gain = 1.0;
            return false;
        }
        let mut engaged = false;
        for frame in samples.chunks_exact_mut(2) {
            let l = f32::from(frame[0]) * self.boost;
            let r = f32::from(frame[1]) * self.boost;
            let peak = l.abs().max(r.abs());
            let target = if peak > THRESHOLD {
                // The boosted signal would clip: the limiter is doing work.
                engaged = true;
                THRESHOLD / peak
            } else {
                1.0
            };
            // Instant attack: the `min` snaps the gain straight down to `target`
            // on the frame that overshoots. Otherwise the one-pole release lets
            // the gain climb back toward unity.
            self.gain = target.min(1.0 - (1.0 - self.gain) * self.release_decay);
            // `as` saturates float-to-int, so a value rounded a hair past full
            // scale clamps to `i16::MAX` instead of wrapping to a huge negative.
            frame[0] = (l * self.gain).round() as i16;
            frame[1] = (r * self.gain).round() as i16;
        }
        engaged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    #[test]
    fn unity_boost_is_bit_transparent() {
        let mut limiter = BoostLimiter::new(RATE, 1.0);
        let mut samples = [0, 100, -100, 12_345, i16::MAX, i16::MIN, 32_767, -32_768];
        let original = samples;
        limiter.process(&mut samples);
        // Every sample -- including i16::MIN, which a 32_768-scaled limiter would
        // mangle -- survives untouched.
        assert_eq!(samples, original);
    }

    #[test]
    fn process_reports_whether_it_engaged() {
        // Unity boost, quiet signal: bypassed, so it never engages.
        assert!(!BoostLimiter::new(RATE, 1.0).process(&mut [100, -100, 0, 0]));

        // A big boost drives a loud signal past full scale: the limiter clamps
        // and says so.
        assert!(
            BoostLimiter::new(RATE, 8.0).process(&mut [20_000, -20_000]),
            "an overshoot must report engagement"
        );

        // A boost that stays under full scale does no clamping.
        assert!(!BoostLimiter::new(RATE, 2.0).process(&mut [1_000, -1_000]));

        // Attenuation can never push anything past full scale, so it never
        // engages -- even on the loudest possible input.
        assert!(!BoostLimiter::new(RATE, 0.5).process(&mut [i16::MAX, i16::MIN]));
    }

    #[test]
    fn quiet_signal_scales_exactly() {
        let mut limiter = BoostLimiter::new(RATE, 2.0);
        let mut samples = [100, -200, 300, -400];
        limiter.process(&mut samples);
        // Nothing is near full scale, so the limiter never engages: a clean 2x.
        assert_eq!(samples, [200, -400, 600, -800]);
    }

    #[test]
    fn full_scale_boost_never_wraps() {
        let mut limiter = BoostLimiter::new(RATE, 4.0);
        let mut samples: [i16; 6] = [20_000, -25_000, 32_767, -32_768, 16_000, -16_000];
        let signs: Vec<i16> = samples.iter().map(|s| s.signum()).collect();
        limiter.process(&mut samples);
        for (&sample, sign) in samples.iter().zip(signs) {
            // A naive `sample * 4` would wrap and flip signs; the limiter keeps
            // magnitude within full scale and the sign intact.
            assert!(i32::from(sample).abs() <= 32_767, "wrapped: {sample}");
            assert_eq!(sample.signum(), sign, "sign flipped: {sample}");
        }
    }

    #[test]
    fn attack_is_instant() {
        let mut limiter = BoostLimiter::new(RATE, 4.0);
        // A loud frame from the very first sample: a release-only limiter would
        // let this first frame through and wrap. Instant attack caps it now.
        let mut samples = [30_000, -30_000];
        limiter.process(&mut samples);
        assert!(i32::from(samples[0]).abs() <= 32_767);
        assert!(i32::from(samples[1]).abs() <= 32_767);
    }

    #[test]
    fn release_recovers_to_exact_scaling() {
        let boost = 2.0;
        let mut limiter = BoostLimiter::new(RATE, boost);
        // One full-scale frame ducks the gain to about 0.5.
        limiter.process(&mut [32_767, -32_768]);
        // Feed ~1 s of a quiet signal; the one-pole climbs back to unity and the
        // quiet signal is once again scaled by exactly `boost`.
        let quiet: i16 = 100;
        let mut buffer = vec![quiet; 2 * RATE as usize];
        limiter.process(&mut buffer);
        assert!(
            buffer[0] < quiet * 2,
            "gain should still be ducked at the start"
        );
        assert_eq!(
            *buffer.last().unwrap(),
            quiet * 2,
            "gain should have recovered"
        );
    }

    #[test]
    fn gain_is_stereo_linked() {
        let mut limiter = BoostLimiter::new(RATE, 4.0);
        // Left is loud enough to force limiting; right is quiet.
        let mut samples = [30_000, 1_000];
        limiter.process(&mut samples);
        // The quiet right channel is reduced by the *left* channel's gain, not
        // its own -- otherwise the stereo image would shift under limiting.
        let gain = THRESHOLD / (30_000.0 * 4.0);
        let expected_right = (1_000.0 * 4.0 * gain).round() as i16;
        assert_eq!(samples[1], expected_right);
        assert!(
            samples[1] < 4_000,
            "linked gain should pull the quiet channel down"
        );
    }

    #[test]
    fn returns_to_bypass_after_reset_to_unity() {
        let mut limiter = BoostLimiter::new(RATE, 4.0);
        // Duck the gain with a loud burst...
        limiter.process(&mut [32_000, -32_000]);
        // ...then drop to unity boost and let the gain settle over ~2 s.
        limiter.set_boost(1.0);
        let mut settle = vec![0; 2 * RATE as usize * 2];
        limiter.process(&mut settle);
        // A fresh loud signal now passes through bit-for-bit: bypass is back on.
        let mut samples = [32_767, -32_768, 12_345, -6_789];
        let original = samples;
        limiter.process(&mut samples);
        assert_eq!(samples, original);
    }
}
