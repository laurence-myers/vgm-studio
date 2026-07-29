//! Offline waveform generation.
//!
//! It is a tight loop over [`PlayerEngine::render`] with integer bucket
//! boundaries and a true min/max per bucket. [`WaveformBucketer`] can be fed PCM
//! incrementally and yields completed buckets, so a background task can stream
//! partial updates; [`render_waveform`] is the batch convenience over it.

use std::borrow::Borrow;

use dro_core::util::VGM_SAMPLE_RATE;
use dro_core::{DroInstruction, Song};

use std::sync::Arc;

use dro_core::VgmFile;

use crate::engine::{FrameClock, PlayerEngine};
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

/// The number of progressive snapshots [`render_waveform_progressive`] aims to
/// emit across a whole render, chosen so the fill looks smooth without flooding
/// the UI. Independent of song length -- a longer song simply renders more
/// buckets between updates.
const PROGRESSIVE_UPDATES: usize = 32;

/// Renders `song` and buckets it into `num_buckets` min/max slices for drawing.
#[must_use]
pub fn render_waveform(song: &Song, num_buckets: usize, sample_rate: u32) -> Vec<WaveformBucket> {
    render_waveform_cancellable(song, num_buckets, sample_rate, || true)
        .expect("a render that is never cancelled always completes")
}

/// As [`render_waveform`], but calling `keep_going` between render chunks so a
/// background task can abandon a stale render mid-song (the GUI resubmits the
/// render on every edit). Returns `None` iff `keep_going` returned `false`.
pub fn render_waveform_cancellable(
    song: &Song,
    num_buckets: usize,
    sample_rate: u32,
    mut keep_going: impl FnMut() -> bool,
) -> Option<Vec<WaveformBucket>> {
    // Reuse the progressive loop but keep only the last (final) snapshot.
    let mut last = None;
    let completed = render_waveform_progressive(
        song,
        num_buckets,
        sample_rate,
        &mut keep_going,
        &mut |buckets| last = Some(buckets),
    );
    completed.then_some(last).flatten()
}

/// Renders `song`, calling `on_update` with a `num_buckets`-long snapshot
/// periodically as the waveform fills in left-to-right, and once more with the
/// completed buckets. This is what drives the GUI's progressive waveform:
/// [`WaveformBucketer::snapshot`] emitted every
/// [`PROGRESSIVE_UPDATES`]th of the way through.
///
/// `keep_going` is polled between render chunks; returning `false` abandons the
/// render (a stale render superseded by an edit) with no final `on_update`.
///
/// Returns `true` if the render completed, `false` if it was cancelled. Snapshot
/// pacing is by bucket progress, not wall-clock, so this stays wasm-clean and
/// deterministic.
pub fn render_waveform_progressive(
    song: &Song,
    num_buckets: usize,
    sample_rate: u32,
    keep_going: &mut dyn FnMut() -> bool,
    on_update: &mut dyn FnMut(Vec<WaveformBucket>),
) -> bool {
    if num_buckets == 0 {
        on_update(Vec::new());
        return true;
    }
    let total = total_output_frames(song, sample_rate);
    let mut bucketer = WaveformBucketer::new(total, num_buckets);

    let stride = (num_buckets / PROGRESSIVE_UPDATES).max(1);
    let mut next_update = stride;

    let mut engine = PlayerEngine::new(song, sample_rate);
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

/// Renders a VGM for any chips into `num_buckets` buckets, the same shape as
/// [`render_waveform_progressive`].
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

/// The number of output frames [`PlayerEngine`] will render for the whole song,
/// used to size the buckets. Mirrors the engine's own frame accounting: the
/// delays, through the same [`FrameClock`]. Register writes cost no frames.
fn total_output_frames<B: Borrow<Song>>(song: B, sample_rate: u32) -> u64 {
    let song = song.borrow();
    let delay_unit = if song.data().delays_in_samples() {
        VGM_SAMPLE_RATE
    } else {
        1000
    };
    let mut clock = FrameClock::new(sample_rate, delay_unit);
    let mut frames = 0u64;
    for instruction in song.data().iter() {
        match instruction {
            DroInstruction::DelayMs { ms, .. } => frames += clock.frames_for(ms),
            DroInstruction::DelaySamples { samples, .. } => frames += clock.frames_for(samples),
            DroInstruction::Register { .. } | DroInstruction::BankSwitch(_) => {}
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV1, OplType};

    fn tone_song() -> Song {
        Song::dro_v1(
            "tone.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x7F, // modulator (fast release)
                0x23, 0x01, 0x43, 0x00, 0x63, 0xF0, 0x83, 0x7F, // carrier (fast release)
                0xA0, 0x98, 0xB0, 0x31, // frequency, key on
                0x00, 0xC7, // 200 ms of tone
                0xB0, 0x11, // key off
                0x00, 0x63, // 100 ms of silence
            ])
            .unwrap(),
            300,
            OplType::Opl2,
        )
    }

    #[test]
    fn produces_exactly_the_requested_number_of_buckets() {
        let song = tone_song();
        for num in [1usize, 10, 300, 1000] {
            assert_eq!(render_waveform(&song, num, 48_000).len(), num);
        }
    }

    #[test]
    fn zero_buckets_is_empty() {
        assert!(render_waveform(&tone_song(), 0, 48_000).is_empty());
    }

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
    #[test]
    fn the_tone_shows_amplitude_and_the_tail_is_silent() {
        // 300 ms song, 30 buckets = 10 ms each. The first ~20 buckets are the
        // keyed-on tone; the last ~10 are silence.
        let buckets = render_waveform(&tone_song(), 30, 48_000);
        // Across the tone region the wave swings well to both sides of zero.
        let peak = buckets[..20].iter().map(|b| b.max).max().unwrap();
        let trough = buckets[..20].iter().map(|b| b.min).min().unwrap();
        assert!(peak > 1000, "tone peak {peak} too quiet");
        assert!(trough < -1000, "tone trough {trough} too quiet");
        // The tail (100 ms after key-off) has released to near silence -- far
        // quieter than the tone. A real note's release rings, so this is relative,
        // not exactly zero.
        let last = *buckets.last().unwrap();
        let tail_amp = i32::from(last.max).max(-i32::from(last.min));
        assert!(
            tail_amp * 4 < i32::from(peak),
            "tail {tail_amp} vs peak {peak}"
        );
    }

    #[test]
    fn bucketer_matches_a_direct_min_max_scan() {
        // Hand-fed PCM: two buckets over four frames.
        let pcm: Vec<i16> = vec![
            10, -5, // frame 0
            -20, 3, // frame 1
            7, 7, // frame 2
            -1, 100, // frame 3
        ];
        let mut bucketer = WaveformBucketer::new(4, 2);
        bucketer.push(&pcm);
        let buckets = bucketer.finish();
        assert_eq!(buckets[0], WaveformBucket { min: -20, max: 10 });
        assert_eq!(buckets[1], WaveformBucket { min: -1, max: 100 });
    }

    #[test]
    fn a_cancelled_render_returns_none_and_an_uncancelled_one_matches_the_batch() {
        let song = tone_song();
        assert_eq!(
            render_waveform_cancellable(&song, 30, 48_000, || false),
            None
        );
        assert_eq!(
            render_waveform_cancellable(&song, 30, 48_000, || true).unwrap(),
            render_waveform(&song, 30, 48_000)
        );
    }

    #[test]
    fn progressive_updates_fill_left_to_right_and_end_at_the_batch() {
        let song = tone_song();
        let batch = render_waveform(&song, 64, 48_000);

        let mut updates: Vec<Vec<WaveformBucket>> = Vec::new();
        let completed =
            render_waveform_progressive(&song, 64, 48_000, &mut || true, &mut |buckets| {
                updates.push(buckets)
            });

        assert!(completed);
        // At least one partial before the final, and every snapshot is the full
        // width so the panel can paint it directly.
        assert!(
            updates.len() >= 2,
            "expected progressive partials + a final"
        );
        assert!(updates.iter().all(|u| u.len() == 64));

        // The last snapshot is exactly the batch render.
        assert_eq!(*updates.last().unwrap(), batch);

        // A finalised (non-silent) bucket never changes in a later snapshot:
        // the fill only ever grows, left to right.
        for pair in updates.windows(2) {
            for (i, bucket) in pair[0].iter().enumerate() {
                if *bucket != WaveformBucket::default() {
                    assert_eq!(*bucket, pair[1][i], "bucket {i} changed after finalising");
                }
            }
        }
    }

    #[test]
    fn a_cancelled_progressive_render_makes_no_final_update() {
        let song = tone_song();
        let mut updates = 0;
        let completed =
            render_waveform_progressive(&song, 64, 48_000, &mut || false, &mut |_| updates += 1);
        assert!(!completed);
        assert_eq!(
            updates, 0,
            "a render cancelled before the first chunk emits nothing"
        );
    }

    #[test]
    fn zero_buckets_still_emits_one_empty_final() {
        let mut updates: Vec<Vec<WaveformBucket>> = Vec::new();
        let completed =
            render_waveform_progressive(&tone_song(), 0, 48_000, &mut || true, &mut |buckets| {
                updates.push(buckets)
            });
        assert!(completed);
        assert_eq!(updates, vec![Vec::new()]);
    }

    /// A VGM declaring `chips` with `stream` as its body.
    fn vgm_file(chips: &[(dro_core::ChipKind, u32)], stream: &[u8]) -> Arc<VgmFile> {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x171);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        for (kind, clock) in chips {
            put_u32(&mut bytes, kind.clock_offset(), *clock);
        }
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(dro_core::vgm::file::read("test.vgm", &bytes).expect("a walkable VGM"))
    }

    #[test]
    fn a_vgm_waveform_has_a_bucket_each_and_shows_the_sound() {
        crate::testing::install_registry_with_stub();
        // A Master System tone for a second, then silence for a second.
        let file = vgm_file(
            &[(dro_core::ChipKind::Sn76489, 3_579_545)],
            &[
                0x50, 0x8E, 0x50, 0x0F, 0x50, 0x90, // a tone at full volume
                0x61, 0x44, 0xAC, // a second of it
                0x50, 0x9F, // and off
                0x61, 0x44, 0xAC, // a second of silence
                0x66,
            ],
        );
        let buckets = render_vgm_waveform(file, 8, 44_100, crate::resample::ResampleMode::Sinc);
        assert_eq!(buckets.len(), 8);
        // The first half sounds, the second does not.
        assert!(
            buckets[..4]
                .iter()
                .all(|bucket| bucket.max > 0 || bucket.min < 0)
        );
        assert!(
            buckets[6..]
                .iter()
                .all(|bucket| bucket.max == 0 && bucket.min == 0)
        );
    }

    #[test]
    fn a_vgm_waveform_with_no_core_is_flat_rather_than_absent() {
        crate::testing::install_registry_with_stub();
        let file = vgm_file(
            &[(dro_core::ChipKind::Ym2612, 7_670_454)],
            &[0x52, 0x28, 0xF0, 0x61, 0x44, 0xAC, 0x66],
        );
        let buckets = render_vgm_waveform(file, 4, 44_100, crate::resample::ResampleMode::Sinc);
        assert_eq!(buckets.len(), 4);
        assert!(
            buckets
                .iter()
                .all(|bucket| bucket.max == 0 && bucket.min == 0)
        );
    }

    #[test]
    fn an_abandoned_vgm_waveform_render_says_so() {
        crate::testing::install_registry_with_stub();
        let file = vgm_file(
            &[(dro_core::ChipKind::Sn76489, 3_579_545)],
            &[0x50, 0x9F, 0x61, 0x44, 0xAC, 0x66],
        );
        let mut calls = 0;
        let completed = render_vgm_waveform_progressive(
            file,
            8,
            44_100,
            crate::resample::ResampleMode::Sinc,
            &mut || {
                calls += 1;
                calls <= 1
            },
            &mut |_| {},
        );
        assert!(!completed);
    }

    #[test]
    fn total_output_frames_matches_the_engine() {
        // The bucket-sizing total must equal what the engine actually renders.
        let song = tone_song();
        let expected = u64::from(song.ms_length) * 48; // 48 kHz
        assert_eq!(total_output_frames(&song, 48_000), expected);
    }
}
