// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bringing a chip's output from its own rate to the output rate, without
//! folding half its spectrum back into the audible band.
//!
//! # What this is for
//!
//! Sound chips run at whatever their clock divides down to -- 41667 Hz for a
//! YM2203, 49716 for an OPL3, 55930 for a YM2151, 223721 for an SN76489 -- and
//! the output is 44100 or 48000. Something has to bridge that, and for the
//! faster chips it is bridging *downwards* by a factor of five.
//!
//! [`VgmEngine`](crate::vgm_engine::VgmEngine) used to bridge it by pulling one
//! source frame at a time and interpolating linearly between the two straddling
//! each output frame. That is a resampler in the sense that it produces the
//! right number of samples, and it is correct at a ratio near 1:1. At 5:1 it is
//! not a resampler at all: linear interpolation attenuates high frequencies
//! only gently, so a square wave's harmonics above the output Nyquist survive
//! the trip and reappear as inharmonic tones somewhere else entirely.
//!
//! The reference-parity harness measured what that costs. Against VGMPlay, an
//! SN76489 rip scored **0.5848** at 44100 and **0.9958** when both sides
//! rendered at the chip's own 223721 Hz; a YM2612 rip 0.9538 against 0.9949.
//! See `docs/vgm-multichip-2026-07/RESAMPLER-PLAN.md`.
//!
//! It is not the explanation for *everything* the scorecard flagged, and an
//! earlier draft of this comment said it was, on the strength of a ratio table
//! built from clock rates rather than from what the cores report. Several cores
//! decimate internally -- the NES APU averages 32 CPU cycles a sample, the
//! HuC6280 64 -- so their ratios are near 1:1, and the HuC6280 scores 0.016 at
//! the same ratio where the YM2151 scores 0.994. Those chips are broken in some
//! other way.
//!
//! # What "accurate" is taken to mean
//!
//! **The output is what an ideal band-limited capture of the chip's output pin
//! would record at the output rate.** Band-limit to the lower of the two
//! Nyquists, then sample. Everything below ~20 kHz passes untouched; nothing
//! above it may reappear.
//!
//! That is a definition a test can hold, which is the point of choosing it.
//! Deliberately *not* included: the analogue output stage of any particular
//! console -- its RC rolloff, its amplifier, its speaker. Those differ by
//! hardware revision, the reference player does not model them either, and
//! folding them in here would make "accurate" unmeasurable. They belong to a
//! tone feature, not to correctness.
//!
//! # The method
//!
//! A polyphase windowed-sinc: for each output frame, a Kaiser-windowed sinc
//! centred on the exact fractional source position, stretched by the ratio when
//! decimating so its cutoff lands below the *output* Nyquist rather than the
//! source's. One code path covers decimation, upsampling, and the exact
//! identity when the two rates match.
//!
//! Three details that are load-bearing rather than decorative:
//!
//! 1. **The taps are normalised by their own sum at each phase.** A fixed
//!    normalisation is right on average and wrong at every individual phase, and
//!    the error moves at the beat frequency between the two rates -- an
//!    amplitude wobble that would be blamed on a core.
//! 2. **The phase accumulator is 32-bit fractional.** The old engine used 16,
//!    which is a step error of up to 1.5e-5 -- invisible across two taps and
//!    not across a thousand.
//! 3. **Source frames are pulled one at a time**, exactly as before, so the
//!    output cannot depend on the caller's chunk size. That property is
//!    structural here rather than tested into place.

use std::sync::OnceLock;

/// Zero crossings of the sinc kept either side of centre.
///
/// Sixteen puts the transition band comfortably inside the guard between 20 kHz
/// and the 22.05 kHz Nyquist while keeping the tap count affordable at the
/// worst ratio this app sees -- the SN76489's and AY8910's 5.07:1, which is 183
/// taps and has measured between 8x and 15x realtime depending on what else the
/// machine was doing.
const ZERO_CROSSINGS: usize = 16;

/// Kaiser window parameter. Beta 10 gives about -90 dB of stopband rejection,
/// which is below the noise floor of a 16-bit render.
const KAISER_BETA: f64 = 10.0;

/// Kernel samples stored per zero-crossing interval.
///
/// The kernel is read at a fractional index and interpolated linearly between
/// entries; at 256 points per lobe that interpolation error is around -100 dB,
/// so it cannot be what limits the stopband.
const SAMPLES_PER_LOBE: usize = 256;

/// Half-kernel length, in table entries.
const HALF_LEN: usize = ZERO_CROSSINGS * SAMPLES_PER_LOBE;

/// Where the passband ends, as a fraction of the lower of the two rates.
///
/// 0.45 leaves a transition band from 19.8 kHz to 22.05 kHz at a 44100 output:
/// wide enough for the window to reach full rejection by Nyquist, narrow enough
/// that nothing audible is touched.
const CUTOFF: f64 = 0.45;

/// Fractional bits in the phase accumulator.
const FRAC_BITS: u32 = 32;
const FRAC_ONE: u64 = 1 << FRAC_BITS;

/// The windowed-sinc half-kernel, `h[i]` at `i / SAMPLES_PER_LOBE` zero
/// crossings from centre. Built once; every voice and every ratio shares it,
/// because the ratio is applied when indexing rather than when building.
fn kernel() -> &'static [f32; HALF_LEN + 2] {
    static KERNEL: OnceLock<[f32; HALF_LEN + 2]> = OnceLock::new();
    KERNEL.get_or_init(|| {
        let mut table = [0.0f32; HALF_LEN + 2];
        let denominator = bessel_i0(KAISER_BETA);
        for (index, entry) in table.iter_mut().enumerate().take(HALF_LEN + 1) {
            let lobes = index as f64 / SAMPLES_PER_LOBE as f64;
            let sinc = if index == 0 {
                1.0
            } else {
                let x = std::f64::consts::PI * lobes;
                x.sin() / x
            };
            // Kaiser, over the half-window: 1 at centre, 0 at the last lobe.
            let ratio = lobes / ZERO_CROSSINGS as f64;
            let window = if ratio >= 1.0 {
                0.0
            } else {
                bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / denominator
            };
            *entry = (sinc * window) as f32;
        }
        // One extra zero past the end so the interpolating read never needs a
        // bounds check for its second point.
        table
    })
}

/// Modified Bessel function of the first kind, order zero.
///
/// The series converges quickly for the arguments a Kaiser window uses; it is
/// here rather than in a dependency because it is eight lines and this crate
/// takes no dependency it can write itself.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= (half / f64::from(k)) * (half / f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Reads the half-kernel at `lobes` zero crossings from centre, interpolating.
///
/// The hot loop in [`Resampler::next_frame`] does this inline and walks the
/// table rather than indexing it, which is worth about four times the speed;
/// this stays as the readable statement of what that loop computes, and is
/// what the kernel's own test reads.
#[cfg(test)]
fn tap(lobes: f64) -> f64 {
    let position = lobes.abs() * SAMPLES_PER_LOBE as f64;
    let index = position as usize;
    if index >= HALF_LEN {
        return 0.0;
    }
    let table = kernel();
    let fraction = position - index as f64;
    let a = f64::from(table[index]);
    let b = f64::from(table[index + 1]);
    a + (b - a) * fraction
}

/// One chip's stereo output, resampled to the engine's rate.
///
/// Pulls source frames from a caller-supplied closure one at a time, so the
/// output is identical however the caller chunks its requests.
#[derive(Debug)]
pub struct Resampler {
    /// Source frames per output frame, in `FRAC_BITS` fixed point.
    step: u64,
    /// Fractional position of the next output frame within the ring, measured
    /// from the oldest frame the kernel still spans.
    phase: u64,
    /// Past source frames, newest last. Length is fixed at construction.
    history: Vec<[i32; 2]>,
    /// Where the next pushed frame goes.
    write: usize,
    /// How many taps the stretched kernel spans on each side.
    half_taps: usize,
    /// Sinc lobes per source frame: 1.0 when upsampling, `output / native` when
    /// decimating, which is what stretches the kernel to the lower Nyquist.
    lobes_per_frame: f64,
    /// True when the rates match exactly and this is a passthrough.
    identity: bool,
}

impl Resampler {
    /// A resampler from `native` Hz to `output` Hz.
    ///
    /// Equal rates give an exact passthrough -- not a kernel that happens to be
    /// nearly one, an actual identity, so a chip already running at the output
    /// rate is bit-for-bit untouched.
    #[must_use]
    pub fn new(native: u32, output: u32) -> Self {
        let native = native.max(1);
        let output = output.max(1);
        let ratio = f64::from(native) / f64::from(output);
        // Decimating stretches the kernel by the ratio so its cutoff follows
        // the *output* Nyquist; upsampling leaves it at the source's.
        let stretch = ratio.max(1.0);
        // Lobes per source frame. The `2 * CUTOFF` is why this is not simply
        // `1 / stretch`: a cutoff below half the rate stretches the sinc
        // further still, so its lobes are wider than one frame apart.
        let lobes_per_frame = (CUTOFF * 2.0) / stretch;
        // Enough frames to reach the window's edge. Getting this wrong
        // truncates the kernel *before* the window has closed, which is a
        // rectangular cut on a non-zero value and leaks accordingly: at
        // `ceil(ZERO_CROSSINGS * stretch)` -- the obvious-looking form, which
        // silently assumes a cutoff of exactly half the rate -- the kernel
        // stopped at 14.5 of its 16 lobes and the stopband came out at -49 dB
        // instead of -90.
        let half_taps = ((ZERO_CROSSINGS as f64 / lobes_per_frame).ceil() as usize).max(1);
        Self {
            step: (f64::from(native) / f64::from(output) * FRAC_ONE as f64) as u64,
            // Half the kernel's span, not all of it. Priming with the whole
            // span would centre the first output frame `half_taps` source
            // frames in -- the output would run 0.4 ms *ahead* of the command
            // stream, and the first 0.4 ms of the chip would be swallowed as
            // filter history and never heard. Priming with half centres output
            // frame zero on source frame zero: the taps reaching back before
            // the start read the zero-filled history, which is what silence
            // before a recording begins actually is.
            phase: FRAC_ONE * half_taps as u64,
            history: vec![[0; 2]; (2 * half_taps + 2).next_power_of_two()],
            write: 0,
            half_taps,
            lobes_per_frame,
            identity: native == output,
        }
    }

    /// The next output frame, pulling from `source` as needed.
    ///
    /// `source` renders exactly one frame at the chip's native rate per call.
    pub fn next_frame(&mut self, mut source: impl FnMut() -> [i32; 2]) -> [i32; 2] {
        if self.identity {
            return source();
        }
        while self.phase >= FRAC_ONE {
            self.history[self.write] = source();
            self.write = (self.write + 1) % self.history.len();
            self.phase -= FRAC_ONE;
        }

        // `phase` is the fractional distance past the newest-but-`half_taps`
        // frame -- the kernel's centre.
        let centre = self.phase as f64 / FRAC_ONE as f64;
        let (mut left, mut right, mut weight) = (0.0f64, 0.0f64, 0.0f64);
        let span = 2 * self.half_taps;

        // **The loop is written to be walked, not computed.** The distance from
        // the kernel's centre falls by exactly `lobes_per_frame` per tap, so
        // the table position is an accumulator rather than a multiply; the ring
        // is a power of two, so the wrap is a mask rather than a modulo; and
        // the kernel is read inline rather than through `tap`'s bounds check.
        //
        // This is not premature. The first version cost 7 ns a tap, which at
        // the NES APU's 1445 taps is **1.1x realtime** -- one voice eating a
        // whole core, and stuttering the moment anything else wanted one. The
        // guard test caught it; the estimate in the plan (a few percent of a
        // core) was simply wrong.
        let table = kernel();
        let mask = self.history.len() - 1;
        let step_per_tap = self.lobes_per_frame * SAMPLES_PER_LOBE as f64;
        let mut position = (centre + self.half_taps as f64) * step_per_tap;
        let mut index = (self.write + self.history.len() - 1 - span) & mask;
        for _ in 0..=span {
            let at = position.abs();
            let whole = at as usize;
            if whole < HALF_LEN {
                let fraction = at - whole as f64;
                let a = f64::from(table[whole]);
                let h = a + (f64::from(table[whole + 1]) - a) * fraction;
                let frame = self.history[index];
                left += h * f64::from(frame[0]);
                right += h * f64::from(frame[1]);
                weight += h;
            }
            position -= step_per_tap;
            index = (index + 1) & mask;
        }
        self.phase += self.step;

        // Normalised by the taps actually used, at this phase. A constant
        // normalisation would be right on average and wrong at every individual
        // phase, wobbling the amplitude at the beat between the two rates.
        if weight.abs() < 1e-12 {
            return [0, 0];
        }
        [clamp(left / weight), clamp(right / weight)]
    }

    /// Drops the history and returns to the starting phase.
    ///
    /// Called wherever the chip itself is reset, so a rewound or seeked engine
    /// is the same machine it was rather than one carrying a tail of the
    /// passage it just left.
    pub fn reset(&mut self) {
        self.history.fill([0; 2]);
        self.write = 0;
        self.phase = FRAC_ONE * self.half_taps as u64;
    }

    /// How many source frames each output frame spans. What the realtime-cost
    /// guard reads, and the number to look at when a ratio seems expensive.
    #[must_use]
    pub fn taps(&self) -> usize {
        if self.identity {
            1
        } else {
            2 * self.half_taps + 1
        }
    }
}

/// Rounds to the nearest integer sample, then clamps.
///
/// **Rounding, not truncation.** `as i32` truncates toward zero, which biases
/// every sample by up to one LSB *towards* zero -- a half-LSB DC offset on
/// average and a slight loss of level, present on every frame of every chip.
/// It showed up here as a constant 1000 coming back as 999: the taps summed to
/// one correctly and the arithmetic landed on 999.9999. This project has
/// already been bitten twice by rounding that goes the wrong way at a fixed
/// point, so it is stated explicitly rather than left to a cast.
fn clamp(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: u32 = 44_100;
    /// The SN76489's rate at the usual 3.58 MHz clock: the 5:1 case the parity
    /// harness caught.
    const SN_NATIVE: u32 = 223_721;

    /// Renders `frames` output frames from a source of `hz` at `native`.
    fn resampled_sine(native: u32, output: u32, hz: f64, frames: usize) -> Vec<f64> {
        let mut resampler = Resampler::new(native, output);
        let mut n = 0u64;
        (0..frames)
            .map(|_| {
                let frame = resampler.next_frame(|| {
                    let t = n as f64 / f64::from(native);
                    n += 1;
                    let value = (20_000.0 * (2.0 * std::f64::consts::PI * hz * t).sin()) as i32;
                    [value, value]
                });
                f64::from(frame[0])
            })
            .collect()
    }

    /// Amplitude of `hz` in `samples`, by Goertzel over a Hann window.
    ///
    /// One filter per probed frequency and about a dozen lines, which is why
    /// there is no FFT dependency here: every test below asks about a specific
    /// tone, not about a spectrum.
    ///
    /// **The window is the whole difficulty.** Without one, a strong tone leaks
    /// into distant bins as roughly `1 / (pi * bins away)` -- for a 1 kHz
    /// fundamental probed at 900 Hz over 42100 samples, that is -49.5 dB, and
    /// it does not care in the least what the resampler did. The first version
    /// of these tests measured exactly -49.8 dB for both the windowed sinc and
    /// the linear interpolation it replaced, and the identical figure was the
    /// clue: the two resamplers were not being compared at all, the analyser's
    /// own skirt was. Hann falls away as the cube of the distance instead,
    /// which puts the floor below -120 dB and out of the way.
    fn amplitude_at(samples: &[f64], hz: f64, rate: u32) -> f64 {
        let n = samples.len();
        if n == 0 {
            return 0.0;
        }
        let omega = 2.0 * std::f64::consts::PI * hz / f64::from(rate);
        let coefficient = 2.0 * omega.cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for (index, &sample) in samples.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * index as f64 / n as f64;
            let windowed = sample * 0.5 * (1.0 - phase.cos());
            let s0 = windowed + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let real = s1 - s2 * omega.cos();
        let imaginary = s2 * omega.sin();
        // Hann's coherent gain is 0.5, so the amplitude is twice what the
        // windowed transform reports.
        4.0 * (real * real + imaginary * imaginary).sqrt() / n as f64
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.max(1e-30).log10()
    }

    /// Where a 1 kHz square's harmonics fold to when 223721 Hz is decimated to
    /// 44100 -- 45 kHz to 900, 43 kHz to 1100, 47 kHz to 2900, and so on.
    ///
    /// Every one is 100 Hz off a true harmonic, so energy here can only have
    /// arrived by folding. Derived from the ratio (`h mod 44100`, reflected
    /// about Nyquist) rather than picked for looking arbitrary, which is the
    /// mistake the first draft of these tests made.
    const ALIAS_PROBES: [f64; 6] = [900.0, 1_100.0, 2_900.0, 3_100.0, 4_900.0, 5_100.0];

    /// A square wave that is already band-limited to `native`'s Nyquist: the
    /// sum of its odd harmonics, and nothing above what the source rate can
    /// represent.
    ///
    /// **The naive form will not do here.** Generating a square by comparing a
    /// phase against a half -- which is what the chips themselves effectively
    /// do -- puts its edges on sample boundaries, and the resulting jitter is
    /// broadband noise sitting around -50 dB. Measuring a resampler's stopband
    /// with that as input measures the input: the first version of these tests
    /// reported -49 dB for a filter whose sine-tone rejection is better than
    /// -80, and the missing 30 dB was never the filter's.
    fn band_limited_square(hz: f64, native: u32, n: u64) -> i32 {
        let t = n as f64 / f64::from(native);
        let nyquist = f64::from(native) / 2.0;
        let mut sum = 0.0;
        let mut harmonic = 1.0;
        while harmonic * hz < nyquist {
            sum += (2.0 * std::f64::consts::PI * harmonic * hz * t).sin() / harmonic;
            harmonic += 2.0;
        }
        // 4/pi normalises the series to a unit square.
        (sum * 20_000.0 * 4.0 / std::f64::consts::PI / 1.273_239) as i32
    }

    /// Everything the ear can hear must arrive at the level it left.
    #[test]
    fn the_passband_is_flat() {
        for hz in [1_000.0, 5_000.0, 10_000.0, 15_000.0] {
            let out = resampled_sine(SN_NATIVE, OUTPUT, hz, 44_100);
            // Skip the kernel's fill-in at the very start.
            let measured = amplitude_at(&out[2_000..], hz, OUTPUT);
            let error = db(measured / 20_000.0);
            assert!(
                error.abs() < 0.1,
                "{hz} Hz arrived {error:+.3} dB off, which is audible shaping"
            );
        }
    }

    /// **The fault this module exists to fix.** A tone above the output Nyquist
    /// is inaudible at the source and must not become audible on the way down.
    /// 30 kHz decimated to 44100 folds to 14.1 kHz, right in the middle of where
    /// it would be heard.
    #[test]
    fn a_tone_above_nyquist_does_not_fold_back() {
        let out = resampled_sine(SN_NATIVE, OUTPUT, 30_000.0, 44_100);
        let alias = amplitude_at(&out[2_000..], 14_100.0, OUTPUT);
        let level = db(alias / 20_000.0);
        assert!(
            level < -80.0,
            "the alias came through at {level:.1} dB, which is plainly audible"
        );
    }

    /// The same statement on the signal the chips actually make. A square wave
    /// is nothing but harmonics, most of them above the output Nyquist, and
    /// every one of them has an unrelated frequency to fold back to.
    ///
    /// Linear interpolation -- what the engine did before this module -- scores
    /// around -20 dB here, so this test is a demonstration and not merely a
    /// guard.
    #[test]
    fn a_square_wave_decimates_without_inharmonic_debris() {
        let hz = 1_000.0;
        let mut resampler = Resampler::new(SN_NATIVE, OUTPUT);
        let mut n = 0u64;
        let out: Vec<f64> = (0..44_100)
            .map(|_| {
                let frame = resampler.next_frame(|| {
                    let value = band_limited_square(hz, SN_NATIVE, n);
                    n += 1;
                    [value, value]
                });
                f64::from(frame[0])
            })
            .collect();
        let body = &out[2_000..];
        let fundamental = amplitude_at(body, hz, OUTPUT);

        // Odd multiples of 1 kHz belong. The probes below are where this
        // signal's aliases *actually land*, worked out rather than guessed:
        // the 45 kHz harmonic folds to 900 Hz, the 43 kHz to 1100, the 47 kHz
        // to 2900, and so on -- every one an offset of 100 Hz from a real
        // harmonic, and each arriving around -33 dB before any filtering.
        //
        // The first draft of this test probed 1450, 3720, 7350, 12130 and
        // 18620 Hz, which are not harmonics *and* not aliases: it read the
        // noise floor and passed against the very resampler it was written to
        // condemn. Probes for an aliasing test have to be derived from the
        // ratio, not chosen for looking untidy.
        for probe in ALIAS_PROBES {
            let level = db(amplitude_at(body, probe, OUTPUT) / fundamental);
            assert!(
                level < -80.0,
                "{probe} Hz is an alias, not a harmonic of {hz} Hz, yet it is                  at {level:.1} dB"
            );
        }
    }

    /// What the change actually bought, measured against what was here before.
    ///
    /// The engine used to interpolate linearly between the two source frames
    /// straddling each output frame. This renders the same square wave both
    /// ways and compares the worst inharmonic tone, so the claim in this
    /// module's header is a number in the suite rather than a story in a
    /// comment -- and so that anyone tempted to put the cheap version back has
    /// to argue with the figure.
    #[test]
    fn the_windowed_sinc_beats_the_linear_interpolation_it_replaced() {
        const HZ: f64 = 1_000.0;

        let square = |n: u64| band_limited_square(HZ, SN_NATIVE, n);

        // The old engine's resampler, reproduced exactly: 16-bit phase, one
        // source frame pulled at a time, linear interpolation between them.
        let lerped: Vec<f64> = {
            let step = (u64::from(SN_NATIVE) << 16) / u64::from(OUTPUT);
            let (mut position, mut prev, mut next, mut n) = (1u64 << 17, 0i32, 0i32, 0u64);
            (0..44_100)
                .map(|_| {
                    while position >= 1 << 16 {
                        prev = next;
                        next = square(n);
                        n += 1;
                        position -= 1 << 16;
                    }
                    let t = position as f64 / f64::from(1u32 << 16);
                    let value = f64::from(prev) + (f64::from(next) - f64::from(prev)) * t;
                    position += step;
                    value
                })
                .collect()
        };

        let mut resampler = Resampler::new(SN_NATIVE, OUTPUT);
        let mut n = 0u64;
        let filtered: Vec<f64> = (0..44_100)
            .map(|_| {
                f64::from(
                    resampler.next_frame(|| {
                        let value = square(n);
                        n += 1;
                        [value, value]
                    })[0],
                )
            })
            .collect();

        let worst = |samples: &[f64]| {
            let body = &samples[2_000..];
            let fundamental = amplitude_at(body, HZ, OUTPUT);
            ALIAS_PROBES
                .iter()
                .map(|&hz| db(amplitude_at(body, hz, OUTPUT) / fundamental))
                .fold(f64::NEG_INFINITY, f64::max)
        };

        let (old, new) = (worst(&lerped), worst(&filtered));
        println!("worst inharmonic tone: lerp {old:.1} dB, windowed sinc {new:.1} dB");
        assert!(
            old > -45.0,
            "the old resampler was supposed to be the problem, yet its worst \
             alias is only {old:.1} dB -- this test is no longer measuring what \
             it claims"
        );
        // Measured: -32.7 dB becomes -117.6 dB, about 85 dB of improvement,
        // which puts the folded energy well below the noise floor of the
        // 16-bit render it ends up in. The bar is the design target rather than
        // the measurement, so a change that halved the rejection would still
        // fail here even though it would still look like an improvement.
        assert!(
            new < -90.0,
            "the filter should bury the aliasing, not merely improve on it: \
             {old:.1} dB became {new:.1} dB"
        );
    }

    /// **Every chip must be delayed by the same amount**, or a file's chips
    /// drift apart from each other: a Mega Drive rip runs a YM2612 at 53 kHz
    /// beside an SN76489 at 224 kHz, and if the filter delayed those by
    /// different amounts the two would sit milliseconds apart in the mix.
    ///
    /// Measured from the step response rather than read off the formula, so it
    /// is the behaviour that is pinned and not the arithmetic.
    #[test]
    fn every_chip_is_delayed_by_the_same_amount() {
        /// How long after a step goes in it comes half-way out, in seconds.
        ///
        /// Silence first, or there is nothing to time: the history is primed
        /// by pulling source frames on the first call, so a source that is
        /// already high has its step arrive before output frame zero.
        fn delay_seconds(native: u32) -> f64 {
            let mut resampler = Resampler::new(native, OUTPUT);
            // The step goes in once the kernel is full of silence.
            let step_at = 2 * u64::from(native) / 100;
            let mut pulled = 0u64;
            let out: Vec<f64> = (0..8_000)
                .map(|_| {
                    f64::from(
                        resampler.next_frame(|| {
                            let value = if pulled >= step_at { 10_000 } else { 0 };
                            pulled += 1;
                            [value, value]
                        })[0],
                    )
                })
                .collect();
            let crossing = out
                .iter()
                .position(|&sample| sample >= 5_000.0)
                .expect("the step must arrive") as f64
                / f64::from(OUTPUT);
            crossing - step_at as f64 / f64::from(native)
        }

        let rates = [49_716, 53_267, 55_930, SN_NATIVE, 1_789_773];
        let measured: Vec<f64> = rates.iter().map(|&rate| delay_seconds(rate)).collect();
        let spread = measured.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - measured.iter().cloned().fold(f64::INFINITY, f64::min);
        // One output frame of slack: the measurement is quantised to that.
        assert!(
            spread <= 1.5 / f64::from(OUTPUT),
            "the chips are delayed by different amounts, spread {:.3} ms: {:?}",
            spread * 1000.0,
            measured.iter().map(|d| d * 1000.0).collect::<Vec<_>>()
        );

        // And they are aligned with the command stream, not merely with each
        // other: a step written at a given moment comes out at that moment.
        // This is what priming the history with *half* the kernel's span buys.
        // With the whole span -- the obvious way to fill a filter before using
        // it -- the output ran 0.4 ms ahead of the music and the chip's first
        // 0.4 ms was swallowed as history and never heard.
        for (&rate, &seconds) in rates.iter().zip(&measured) {
            assert!(
                seconds.abs() < 2.0 / f64::from(OUTPUT),
                "{rate} Hz sits {:.3} ms away from where the command stream put it",
                seconds * 1000.0
            );
        }
    }

    /// A constant in must be the same constant out, at every ratio, or the
    /// resampler has a DC error that would show up as a click at every seam.
    #[test]
    fn direct_current_passes_exactly() {
        for (native, output) in [
            (SN_NATIVE, OUTPUT),
            (1_789_773, OUTPUT),
            (41_667, OUTPUT),
            (49_716, OUTPUT),
            (8_000, OUTPUT),
        ] {
            let mut resampler = Resampler::new(native, output);
            let out: Vec<i32> = (0..2_000)
                .map(|_| resampler.next_frame(|| [1_000, -1_000])[0])
                .collect();
            // Past the fill-in, every frame is the input exactly.
            for &sample in &out[1_000..] {
                assert_eq!(
                    sample, 1_000,
                    "{native} -> {output} shifted DC, so its taps do not sum to one"
                );
            }
        }
    }

    /// Equal rates are a passthrough, not a kernel that rounds to one.
    #[test]
    fn matching_rates_are_bit_identical() {
        let mut resampler = Resampler::new(OUTPUT, OUTPUT);
        let mut n = 0i32;
        for _ in 0..1_000 {
            let expected = [n * 7 - 500, -n * 3];
            let frame = resampler.next_frame(|| {
                let value = expected;
                n += 1;
                value
            });
            assert_eq!(frame, expected);
        }
        assert_eq!(resampler.taps(), 1);
    }

    /// Upsampling is the same code path and must not image either: a source at
    /// 8 kHz has nothing above 4 kHz, and the output must not invent any.
    #[test]
    fn upsampling_adds_no_images() {
        let out = resampled_sine(8_000, OUTPUT, 1_000.0, 22_050);
        let measured = amplitude_at(&out[2_000..], 1_000.0, OUTPUT);
        assert!(
            db(measured / 20_000.0).abs() < 0.2,
            "the tone itself was shaped: {:.3} dB",
            db(measured / 20_000.0)
        );
        // 7 kHz is the mirror of 1 kHz about the source Nyquist: the classic
        // image, and the thing a zero-order hold would leave behind.
        let image = db(amplitude_at(&out[2_000..], 7_000.0, OUTPUT) / measured);
        assert!(image < -80.0, "an image survived at {image:.1} dB");
    }

    /// The output may not depend on how the caller chunks its pulls -- the
    /// audio worklet asks for 128 frames and the WAV renderer for 4096, and
    /// they have to agree.
    #[test]
    fn the_chunk_size_is_invisible() {
        fn render(chunk: usize, total: usize) -> Vec<i32> {
            let mut resampler = Resampler::new(SN_NATIVE, OUTPUT);
            let mut n = 0u64;
            let mut out = Vec::with_capacity(total);
            while out.len() < total {
                for _ in 0..chunk.min(total - out.len()) {
                    let frame = resampler.next_frame(|| {
                        let value = ((n % 977) as i32) * 31 - 15_000;
                        n += 1;
                        [value, value / 2]
                    });
                    out.push(frame[0]);
                }
            }
            out
        }
        assert_eq!(render(128, 8_000), render(4_096, 8_000));
        assert_eq!(render(1, 8_000), render(4_096, 8_000));
    }

    /// A reset must leave the same machine construction did, or a seek would
    /// carry a tail of the passage it left.
    #[test]
    fn a_reset_returns_to_the_starting_state() {
        let mut resampler = Resampler::new(SN_NATIVE, OUTPUT);
        let mut n = 0u64;
        let source = |n: &mut u64| {
            let value = ((*n % 331) as i32) * 61 - 10_000;
            *n += 1;
            [value, value]
        };
        let first: Vec<i32> = (0..500)
            .map(|_| resampler.next_frame(|| source(&mut n))[0])
            .collect();

        resampler.reset();
        n = 0;
        let second: Vec<i32> = (0..500)
            .map(|_| resampler.next_frame(|| source(&mut n))[0])
            .collect();
        assert_eq!(first, second);
    }

    /// The kernel has to be a windowed sinc and not, say, a table of zeros --
    /// asserted directly, because every test above would also pass a resampler
    /// that returned its input unchanged at ratio 1 and silence elsewhere.
    #[test]
    fn the_kernel_is_a_windowed_sinc() {
        assert!((tap(0.0) - 1.0).abs() < 1e-9, "unity at the centre");
        for lobe in 1..ZERO_CROSSINGS {
            assert!(
                tap(lobe as f64).abs() < 1e-3,
                "the sinc must cross zero at every whole lobe, not at {lobe}"
            );
        }
        assert!(tap(0.5) > 0.5, "the first lobe carries most of the weight");
        assert_eq!(tap(ZERO_CROSSINGS as f64), 0.0, "the window closes");
        assert_eq!(tap(100.0), 0.0, "and stays closed past the end");

        // Bessel I0 against known values, since the window is only as good as
        // it is: I0(0) = 1, I0(1) = 1.2660658, I0(10) = 2815.7166.
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-12);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008).abs() < 1e-9);
        assert!((bessel_i0(10.0) - 2_815.716_628_466_254).abs() < 1e-6);
    }

    /// The worst ratio must stay comfortably faster than realtime, because the
    /// audio thread has no way to complain: a resampler that fell behind would
    /// surface as stuttering, which is a bug report rather than a red test.
    ///
    /// **The worst ratio in this app is 5.07:1, not 40:1.** The NES APU's core
    /// averages 32 CPU cycles into each output sample, so it presents 55.9 kHz
    /// rather than the 1.79 MHz its clock might suggest, and the HuC6280 and
    /// Game Boy divide theirs too. What is actually left is the SN76489 and
    /// AY8910 at 223721 Hz -- 183 taps, not 1445.
    ///
    /// Measuring the hypothetical 40:1 first was still worth it: it read 1.1x
    /// realtime and forced the inner loop to be written properly. But the bar
    /// belongs on the ratio that exists.
    ///
    /// Deliberately loose (five times realtime) because this guards against a
    /// change of *order*, not a benchmark: a machine under load should not fail
    /// it, and doubling the kernel should.
    ///
    /// **Release only.** A debug build runs this at 0.5x realtime and a release
    /// build at 14.6x, so in debug the number says nothing about what ships and
    /// the assertion would fail every ordinary `cargo test`. Ignored rather
    /// than compiled out, so a debug run still lists it and says why.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "timing is meaningless in a debug build; run with --release"
    )]
    fn the_worst_ratio_runs_faster_than_realtime() {
        let mut resampler = Resampler::new(SN_NATIVE, OUTPUT);
        let mut n = 0u64;
        let started = std::time::Instant::now();
        let frames = OUTPUT as usize;
        for _ in 0..frames {
            std::hint::black_box(resampler.next_frame(|| {
                let value = ((n % 4_099) as i32) - 2_048;
                n += 1;
                [value, value]
            }));
        }
        let elapsed = started.elapsed().as_secs_f64();
        let realtime_factor = 1.0 / elapsed.max(1e-9);
        println!(
            "worst real ratio (223721 -> 44100): {} taps, {realtime_factor:.1}x realtime",
            resampler.taps()
        );
        assert!(
            realtime_factor > 5.0,
            "one second of the worst ratio took {elapsed:.3} s -- {realtime_factor:.1}x \
             realtime, which is too close to stuttering"
        );
    }

    /// The worst ratio this app meets is the NES APU's 40:1, and it must stay
    /// affordable: the tap count is what decides that, so it is pinned.
    #[test]
    fn the_tap_count_stays_bounded() {
        assert_eq!(Resampler::new(OUTPUT, OUTPUT).taps(), 1);
        // Enough frames to span all ZERO_CROSSINGS lobes of the *stretched*
        // sinc -- `ZC * native / (2 * CUTOFF * output)` each side.
        assert_eq!(Resampler::new(49_716, OUTPUT).taps(), 2 * 21 + 1);
        assert_eq!(Resampler::new(SN_NATIVE, OUTPUT).taps(), 2 * 91 + 1);
        // No core presents a ratio like this today -- the NES APU decimates
        // internally to 55.9 kHz -- but a future one could, and the cost grows
        // linearly with it, so the bound is kept as a warning shot.
        let extreme = Resampler::new(1_789_773, OUTPUT).taps();
        assert!(
            extreme <= 1_500,
            "a 40:1 ratio needs {extreme} taps, which is past what was budgeted"
        );
    }
}
