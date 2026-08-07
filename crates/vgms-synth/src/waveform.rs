//! Offline waveform generation.
//!
//! It is a tight loop over [`VgmEngine::render`] with integer bucket boundaries
//! and a true min/max per bucket. [`WaveformBucketer`] can be fed PCM
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
            }

            self.min = self.min.min(frame[0]).min(frame[1]);
            self.max = self.max.max(frame[0]).max(frame[1]);
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
    resampling: crate::resample::ResampleMode,
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

    let mut engine = VgmEngine::new(file, sample_rate);
    // The waveform shows what playback would sound like, so it follows the
    // same resampling choice; the difference is invisible at bucket scale, but
    // drawing from a different render than the one being heard is the kind of
    // quiet inconsistency that eventually confuses a bug report.
    engine.set_resample_mode(resampling);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        if !keep_going() {
            return false;
        }
        let frames = engine.render(&mut buffer);
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
}

/// [`render_vgm_waveform_progressive`] keeping only the finished buckets.
#[must_use]
pub fn render_vgm_waveform(
    file: Arc<VgmFile>,
    num_buckets: usize,
    sample_rate: u32,
    resampling: crate::resample::ResampleMode,
) -> Vec<WaveformBucket> {
    let mut last = Vec::new();
    render_vgm_waveform_progressive(
        file,
        num_buckets,
        sample_rate,
        resampling,
        &mut || true,
        &mut |buckets| {
            last = buckets;
        },
    );
    last
}
