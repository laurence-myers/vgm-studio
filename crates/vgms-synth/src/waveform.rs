//! Offline waveform generation.
//!
//! It is a tight loop over [`VgmEngine::render`] with integer bucket boundaries
//! and a true min/max per bucket, drawn from the render with its standing DC
//! offset removed (see [`DcBlocker`]). [`WaveformBucketer`] can be fed PCM
//! incrementally and yields completed buckets, so a background task can stream
//! partial updates; [`render_vgm_waveform`] is the batch convenience over it.

use std::sync::Arc;

use vgms_core::VgmFile;
use vgms_core::util::VGM_SAMPLE_RATE;

use crate::vgm_engine::VgmEngine;

/// The vertical extent of one horizontal slice of the waveform: the lowest and
/// highest sample in that slice. A silent slice is `{ min: 0, max: 0 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WaveformBucket {
    pub min: i16,
    pub max: i16,
}

/// Buckets a stream of interleaved stereo PCM into `num_buckets` min/max slices.
///
/// Frames map to buckets by exact integer arithmetic (`frame * num_buckets /
/// total_frames`), so there is no per-sample float division and no drift. Feed
/// PCM with [`Self::push`]; take the finished slices with [`Self::finish`].
#[derive(Debug)]
pub struct WaveformBucketer {
    total_frames: u64,
    num_buckets: usize,
    frame_index: u64,
    current_bucket: usize,
    min: i16,
    max: i16,
    /// Whether the bucket being accumulated has seen no frame yet, so its
    /// `min`/`max` are placeholders rather than an extent.
    empty: bool,
    buckets: Vec<WaveformBucket>,
}

impl WaveformBucketer {
    /// Prepares to bucket a song of `total_frames` frames into `num_buckets`
    /// slices.
    #[must_use]
    pub fn new(total_frames: u64, num_buckets: usize) -> Self {
        Self {
            total_frames: total_frames.max(1),
            num_buckets,
            frame_index: 0,
            current_bucket: 0,
            min: 0,
            max: 0,
            empty: true,
            buckets: Vec::with_capacity(num_buckets),
        }
    }

    /// Folds a chunk of interleaved stereo PCM into the buckets. Frames beyond
    /// `total_frames` fold into the last bucket rather than being dropped.
    pub fn push(&mut self, pcm: &[i16]) {
        for frame in pcm.chunks_exact(2) {
            if self.num_buckets == 0 {
                return;
            }
            let bucket =
                usize::try_from(self.frame_index * self.num_buckets as u64 / self.total_frames)
                    .unwrap_or(self.num_buckets - 1)
                    .min(self.num_buckets - 1);

            if bucket != self.current_bucket {
                self.buckets.push(WaveformBucket {
                    min: self.min,
                    max: self.max,
                });
                // A slice with no frames (more buckets than frames) is silent.
                for _ in self.current_bucket + 1..bucket {
                    self.buckets.push(WaveformBucket::default());
                }
                self.current_bucket = bucket;
                self.min = 0;
                self.max = 0;
                self.empty = true;
            }

            if self.empty {
                // The bucket's first frame seeds its extent. Seeding with zero
                // instead would stretch every slice to the centre line, so a
                // stretch sitting wholly above zero would draw down to it.
                self.min = frame[0].min(frame[1]);
                self.max = frame[0].max(frame[1]);
                self.empty = false;
            } else {
                self.min = self.min.min(frame[0]).min(frame[1]);
                self.max = self.max.max(frame[0]).max(frame[1]);
            }
            self.frame_index += 1;
        }
    }

    /// Finishes bucketing, returning exactly `num_buckets` slices (padding the
    /// tail with silence if the song was shorter than expected).
    #[must_use]
    pub fn finish(mut self) -> Vec<WaveformBucket> {
        if self.num_buckets == 0 {
            return Vec::new();
        }
        self.buckets.push(WaveformBucket {
            min: self.min,
            max: self.max,
        });
        self.buckets
            .resize(self.num_buckets, WaveformBucket::default());
        self.buckets
    }

    /// The number of buckets finalised so far -- one behind the bucket
    /// currently being accumulated. Used to pace progressive updates.
    #[must_use]
    pub fn completed(&self) -> usize {
        self.buckets.len()
    }

    /// A `num_buckets`-long snapshot of progress: the finalised leading buckets,
    /// then silence for the rest. Cheap to call repeatedly; unlike [`Self::finish`]
    /// it borrows, so bucketing continues afterwards.
    #[must_use]
    pub fn snapshot(&self) -> Vec<WaveformBucket> {
        if self.num_buckets == 0 {
            return Vec::new();
        }
        let mut snapshot = self.buckets.clone();
        snapshot.resize(self.num_buckets, WaveformBucket::default());
        snapshot
    }
}

/// Where [`DcBlocker`] cuts off, in Hz: below any musical fundamental, above
/// the drift of a chip's standing offset as its channels key on and off.
const DC_CUTOFF_HZ: f32 = 10.0;

/// A one-pole high-pass (`y = x - x[-1] + r * y[-1]`) that takes the standing
/// DC offset out of a stereo stream, for drawing only.
///
/// Many chips idle off zero or swing one way only: an AY8910 or an NES APU
/// outputs `0..+peak`, emu2413's YM2413 carries a standing offset, and a
/// YM2612's DAC ladder sits off centre. The engine keeps that offset -- the
/// reference player keeps it too, and parity is measured against it -- but
/// nobody hears it: every speaker and coupling capacitor between the chip and
/// an ear removes it. Drawn raw, such a song hugs the top of the well with a
/// flat bottom half. Removing it here makes the picture what is heard, and
/// touches neither playback nor export.
#[derive(Debug)]
struct DcBlocker {
    r: f32,
    previous_in: [f32; 2],
    previous_out: [f32; 2],
}

impl DcBlocker {
    fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            // The first-order form of `exp(-2 pi fc / fs)`: indistinguishable at
            // a cutoff this far below the rate, and built from IEEE-exact
            // operations only -- `exp` comes from the platform's libm, and the
            // snapshot baselines must render alike on every target.
            r: 1.0 - 2.0 * std::f32::consts::PI * DC_CUTOFF_HZ / rate,
            previous_in: [0.0; 2],
            previous_out: [0.0; 2],
        }
    }

    /// Filters interleaved stereo PCM in place.
    fn process(&mut self, pcm: &mut [i16]) {
        for frame in pcm.chunks_exact_mut(2) {
            for (side, sample) in frame.iter_mut().enumerate() {
                let input = f32::from(*sample);
                let output = input - self.previous_in[side] + self.r * self.previous_out[side];
                self.previous_in[side] = input;
                self.previous_out[side] = output;
                // A full-scale swing just after a full-scale step can briefly
                // overshoot the 16-bit range; the picture clips it like the DAC would.
                *sample = output
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX))
                    as i16;
            }
        }
    }
}

/// The number of progressive snapshots [`render_vgm_waveform_progressive`] aims to
/// emit across a whole render, chosen so the fill looks smooth without flooding
/// the UI. Independent of song length -- a longer song simply renders more
/// buckets between updates.
const PROGRESSIVE_UPDATES: usize = 32;

/// Renders a VGM for any chips into `num_buckets` buckets, the same shape as
/// [`render_vgm_waveform_progressive`].
///
/// A waveform is a picture of the audio, and what produced the audio does not
/// change how it is drawn -- so this is the same loop over the other engine.
/// A chip with no core renders silence, so a file this app only half knows draws
/// only the half it can play.
///
/// Returns `true` if the render completed, `false` if `keep_going` abandoned it.
pub fn render_vgm_waveform_progressive(
    file: Arc<VgmFile>,
    num_buckets: usize,
    sample_rate: u32,
    keep_going: &mut dyn FnMut() -> bool,
    on_update: &mut dyn FnMut(Vec<WaveformBucket>),
) -> bool {
    if num_buckets == 0 {
        on_update(Vec::new());
        return true;
    }
    // The stream's own waits, in output frames: the same number the engine will
    // render, derived the same way the corpus test checks it against.
    let total = file.stream().map_or(0, |stream| {
        stream.total_samples() * u64::from(sample_rate) / u64::from(VGM_SAMPLE_RATE)
    });
    let mut bucketer = WaveformBucketer::new(total, num_buckets);

    let stride = (num_buckets / PROGRESSIVE_UPDATES).max(1);
    let mut next_update = stride;

    // The waveform is drawn with the *fastest* core the build has for each chip,
    // not the user's playback pick: a below-realtime LLE core would make this
    // offline render crawl, while the coarse min/max buckets hide any accuracy
    // difference.
    //
    // The same goes for resampling: always linear, whatever playback uses. Sinc
    // spends ~36 taps per *source* frame, so on a fast chip it rivals the
    // emulation itself -- measured over a corpus sample, linear cut the render's
    // CPU by a quarter overall and by 2-4x on the SN76489, AY8910 and YMZ280B.
    // Its box average does shave the peaks of bright content by 10-15%; the
    // buckets are auto-scaled, so that is a slightly different picture of the
    // same song, not a wrong one. The rate still follows playback: a lower one
    // saves little more, and would pile a DAC stream's writes into single frames
    // so the chip plays only the last of each pile.
    let choices =
        crate::registry::fastest_choices(file.header.chips().iter().map(|chip| chip.kind));
    crate::registry::with_render_choices(Some(choices), move || {
        let mut engine = VgmEngine::new(file, sample_rate);
        engine.set_resample_mode(crate::resample::ResampleMode::Linear);
        let mut dc_blocker = DcBlocker::new(sample_rate);
        let mut buffer = vec![0i16; 4096 * 2];
        loop {
            if !keep_going() {
                return false;
            }
            let frames = engine.render(&mut buffer);
            dc_blocker.process(&mut buffer[..frames * 2]);
            bucketer.push(&buffer[..frames * 2]);
            if bucketer.completed() >= next_update {
                on_update(bucketer.snapshot());
                next_update = bucketer.completed() + stride;
            }
            if frames < buffer.len() / 2 {
                break;
            }
        }
        on_update(bucketer.finish());
        true
    })
}

/// [`render_vgm_waveform_progressive`] keeping only the finished buckets.
#[must_use]
pub fn render_vgm_waveform(
    file: Arc<VgmFile>,
    num_buckets: usize,
    sample_rate: u32,
) -> Vec<WaveformBucket> {
    let mut last = Vec::new();
    render_vgm_waveform_progressive(
        file,
        num_buckets,
        sample_rate,
        &mut || true,
        &mut |buckets| {
            last = buckets;
        },
    );
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Interleaved stereo with the same value on both sides.
    fn mono(samples: &[i16]) -> Vec<i16> {
        samples
            .iter()
            .flat_map(|&sample| [sample, sample])
            .collect()
    }

    /// A slice that never crosses zero reports its own extent, not one
    /// stretched to the centre line.
    #[test]
    fn a_bucket_reports_its_true_extent() {
        let mut bucketer = WaveformBucketer::new(4, 2);
        bucketer.push(&mono(&[300, 500, -200, -100]));
        assert_eq!(
            bucketer.finish(),
            vec![
                WaveformBucket { min: 300, max: 500 },
                WaveformBucket {
                    min: -200,
                    max: -100
                },
            ]
        );
    }

    /// A slice no frame lands in -- more buckets than frames -- stays silent.
    #[test]
    fn a_bucket_with_no_frames_is_silent() {
        let mut bucketer = WaveformBucketer::new(2, 4);
        bucketer.push(&mono(&[700, 900]));
        assert_eq!(
            bucketer.finish(),
            vec![
                WaveformBucket { min: 700, max: 700 },
                WaveformBucket::default(),
                WaveformBucket { min: 900, max: 900 },
                WaveformBucket::default(),
            ]
        );
    }

    /// A one-sided square wave -- an AY8910 tone, `0..+peak` -- comes out
    /// centred once the blocker has settled: as far below zero as above.
    #[test]
    fn the_dc_blocker_centres_a_one_sided_wave() {
        const RATE: u32 = 48_000;
        let mut blocker = DcBlocker::new(RATE);
        // 500 Hz, two seconds: far longer than the blocker's ~16 ms settling.
        let mut pcm = mono(
            &(0..RATE as usize * 2)
                .map(|n| if (n / 48) % 2 == 0 { 8000 } else { 0 })
                .collect::<Vec<_>>(),
        );
        blocker.process(&mut pcm);
        let tail = &pcm[pcm.len() / 2..];
        let max = i32::from(*tail.iter().max().unwrap());
        let min = i32::from(*tail.iter().min().unwrap());
        assert!(
            (max + min).abs() < 200,
            "the wave should straddle zero evenly: min {min}, max {max}"
        );
        // The swing itself survives: a 500 Hz tone is far above the cutoff.
        assert!(max - min > 7800, "the tone lost level: {min}..{max}");
    }

    /// A constant offset decays to nothing.
    #[test]
    fn the_dc_blocker_removes_a_constant_offset() {
        let mut blocker = DcBlocker::new(48_000);
        let mut pcm = mono(&[5000; 48_000]);
        blocker.process(&mut pcm);
        assert_eq!(pcm[pcm.len() - 1], 0);
    }
}
