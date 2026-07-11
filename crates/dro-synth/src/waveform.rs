//! Offline waveform generation (Python's `WaveformRenderer`).
//!
//! The Python renderer played the whole song through the real-time pipeline into
//! an output sink that, per rendered buffer, unpacked every sample in Python, did
//! two float divisions each, and **re-enqueued the entire growing points list**
//! (`self.queue.put(self.points)`) -- quadratic in queue traffic. It also
//! hard-coded mono and tracked only a peak of the *positive* samples.
//!
//! Here it is a tight loop over [`PlayerEngine::render`] with integer bucket
//! boundaries and a true min/max per bucket. [`WaveformBucketer`] can be fed PCM
//! incrementally and yields completed buckets, so a background task can stream
//! partial updates; [`render_waveform`] is the batch convenience over it.

use std::borrow::Borrow;

use dro_core::util::VGM_SAMPLE_RATE;
use dro_core::{DroInstruction, Song};

use crate::engine::{FrameClock, PlayerEngine};

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

    /// Folds a chunk of interleaved stereo PCM into the buckets. Extra samples
    /// beyond `total_frames` are ignored.
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
}

/// Renders `song` and buckets it into `num_buckets` min/max slices for drawing.
///
/// `chip_write_delay` is microseconds per register write, matching playback.
#[must_use]
pub fn render_waveform(
    song: &Song,
    num_buckets: usize,
    sample_rate: u32,
    chip_write_delay: f64,
) -> Vec<WaveformBucket> {
    if num_buckets == 0 {
        return Vec::new();
    }
    let total = total_output_frames(song, sample_rate, chip_write_delay);
    let mut bucketer = WaveformBucketer::new(total, num_buckets);

    let mut engine = PlayerEngine::new(song, sample_rate, chip_write_delay);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        let frames = engine.render(&mut buffer);
        bucketer.push(&buffer[..frames * 2]);
        if frames < buffer.len() / 2 {
            break;
        }
    }
    bucketer.finish()
}

/// The number of output frames [`PlayerEngine`] will render for the whole song,
/// used to size the buckets. Mirrors the engine's own frame accounting: delays
/// through the [`FrameClock`], plus one chip-write-delay per register write.
fn total_output_frames<B: Borrow<Song>>(song: B, sample_rate: u32, chip_write_delay: f64) -> u64 {
    let song = song.borrow();
    let delay_unit = if song.data().delays_in_samples() {
        VGM_SAMPLE_RATE
    } else {
        1000
    };
    let mut clock = FrameClock::new(sample_rate, delay_unit);
    let mut frames = 0u64;
    let mut writes = 0u64;
    for instruction in song.data().iter() {
        match instruction {
            DroInstruction::DelayMs { ms, .. } => frames += clock.frames_for(ms),
            DroInstruction::DelaySamples { samples, .. } => frames += clock.frames_for(samples),
            DroInstruction::Register { .. } => writes += 1,
            DroInstruction::BankSwitch(_) => {}
        }
    }
    if chip_write_delay > 0.0 {
        // The engine carries a fractional remainder here; a whole-frame estimate
        // is close enough to size buckets by.
        let extra = writes as f64 * chip_write_delay * f64::from(sample_rate) / 1_000_000.0;
        frames += extra as u64;
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
            assert_eq!(render_waveform(&song, num, 48_000, 0.0).len(), num);
        }
    }

    #[test]
    fn zero_buckets_is_empty() {
        assert!(render_waveform(&tone_song(), 0, 48_000, 0.0).is_empty());
    }

    #[test]
    fn the_tone_shows_amplitude_and_the_tail_is_silent() {
        // 300 ms song, 30 buckets = 10 ms each. The first ~20 buckets are the
        // keyed-on tone; the last ~10 are silence.
        let buckets = render_waveform(&tone_song(), 30, 48_000, 0.0);
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
    fn total_output_frames_matches_the_engine() {
        // The bucket-sizing total must equal what the engine actually renders.
        let song = tone_song();
        let expected = u64::from(song.ms_length) * 48; // 48 kHz
        assert_eq!(total_output_frames(&song, 48_000, 0.0), expected);
    }
}
