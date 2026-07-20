//! Offline WAV rendering (Python's `WavRenderer`).
//!
//! The Python renderer was one of several push sinks fed by the real-time
//! playback pipeline. Here it is a plain loop over [`PlayerEngine::render`],
//! writing into an in-memory `hound` WAV. The same bytes result on native and web
//! -- the caller writes them to disk or offers them as a download.

use std::borrow::Borrow;
use std::io::Cursor;

use dro_core::Song;
use hound::{SampleFormat, WavSpec, WavWriter};

use crate::engine::{Muting, Panning, PlayerEngine};
use crate::limiter::BoostLimiter;

/// How a render is mixed: which voices are audible, where they sit in the stereo
/// image, and how hard the signal is driven.
///
/// [`Default`] is the faithful render every `drotrim render` produces -- nothing
/// muted, the song's own stereo image, and no boost. The GUI's Render to WAV
/// dialog turns each of the three on individually.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderMix {
    pub muting: Muting,
    pub panning: Panning,
    /// Multiplies the signal through the playback peak limiter. `1.0` is
    /// bit-transparent.
    pub boost: f32,
}

impl Default for RenderMix {
    fn default() -> Self {
        Self {
            muting: Muting::all(),
            panning: Panning::Original,
            boost: 1.0,
        }
    }
}

/// Renders `song` to a stereo WAV file held in memory.
///
/// `bit_depth` must be `8` or `16` (as [`dro_core::config::AudioConfig`]
/// guarantees). The chip always renders 16-bit internally; an 8-bit request is
/// down-converted at write time, since -- unlike PyOPL -- the Rust core has no
/// 8-bit mode.
///
/// # Errors
/// If the `hound` writer fails. Writing to an in-memory `Cursor` does not fail in
/// practice, so this is effectively infallible.
pub fn render_wav(song: &Song, sample_rate: u32, bit_depth: u16) -> Result<Vec<u8>, hound::Error> {
    render_wav_impl(
        song,
        RenderMix::default(),
        sample_rate,
        bit_depth,
        &mut |_| {},
    )
}

/// Renders `song` with muting, panning and boost all applied -- the GUI's Render
/// to WAV, whose dialog offers the three independently.
///
/// [`RenderMix::default()`] renders exactly what [`render_wav`] does.
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_mixed<B: Borrow<Song>>(
    song: B,
    mix: RenderMix,
    sample_rate: u32,
    bit_depth: u16,
) -> Result<Vec<u8>, hound::Error> {
    render_wav_impl(song, mix, sample_rate, bit_depth, &mut |_| {})
}

/// As [`render_wav`], but with channel/percussion muting applied -- what
/// `dro_split` uses to render one isolated voice. Generic over the song
/// container so the audio thread can pass an `Arc<Song>` without cloning.
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_muted<B: Borrow<Song>>(
    song: B,
    muting: Muting,
    sample_rate: u32,
    bit_depth: u16,
) -> Result<Vec<u8>, hound::Error> {
    let mix = RenderMix {
        muting,
        ..RenderMix::default()
    };
    render_wav_impl(song, mix, sample_rate, bit_depth, &mut |_| {})
}

/// As [`render_wav_muted`], reporting the running rendered-frame count to
/// `on_progress` after each chunk so `dro_split` can show live progress per
/// channel on a long render. The rendered bytes are identical to
/// [`render_wav_muted`].
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_muted_with_progress<B: Borrow<Song>>(
    song: B,
    muting: Muting,
    sample_rate: u32,
    bit_depth: u16,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, hound::Error> {
    let mix = RenderMix {
        muting,
        ..RenderMix::default()
    };
    render_wav_impl(song, mix, sample_rate, bit_depth, on_progress)
}

/// As [`render_wav`], but multiplies the signal by `boost` through the same peak
/// limiter used for live playback, so a boosted render matches boosted playback
/// and still cannot clip. `boost == 1.0` is bit-transparent -- identical to
/// [`render_wav`].
///
/// This is the one render path deliberately *not* faithful to the un-boosted
/// signal; it is opt-in through `dro_player --render --boost`. The `drotrim.ini`
/// / GUI boost never reaches a render -- only an explicit CLI value does.
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_boosted(
    song: &Song,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
) -> Result<Vec<u8>, hound::Error> {
    let mix = RenderMix {
        boost,
        ..RenderMix::default()
    };
    render_wav_impl(song, mix, sample_rate, bit_depth, &mut |_| {})
}

/// As [`render_wav_boosted`], reporting the running rendered-frame count to
/// `on_progress` after each chunk so a CLI can show live progress on a long
/// render. The rendered bytes are identical to [`render_wav_boosted`].
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_boosted_with_progress(
    song: &Song,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, hound::Error> {
    let mix = RenderMix {
        boost,
        ..RenderMix::default()
    };
    render_wav_impl(song, mix, sample_rate, bit_depth, on_progress)
}

/// The shared render loop behind the public `render_wav*` entry points.
fn render_wav_impl<B: Borrow<Song>>(
    song: B,
    mix: RenderMix,
    sample_rate: u32,
    bit_depth: u16,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, hound::Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: bit_depth,
        sample_format: SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)?;

    let mut engine = PlayerEngine::new(song, sample_rate);
    engine.set_muting(mix.muting);
    // Only when it differs from the engine's own starting state: `set_panning`
    // is a chip write, and `Original` would replay the whole `0xC0` shadow for
    // nothing -- keeping the faithful render byte-for-byte what it always was.
    if mix.panning != Panning::Original {
        engine.set_panning(mix.panning);
    }
    let mut limiter = BoostLimiter::new(sample_rate, mix.boost);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        let frames = engine.render(&mut buffer);
        // Boost and limit exactly as the live audio callback does, so a boosted
        // render matches boosted playback. Bit-transparent when boost is 1.0, so
        // the faithful `render_wav` / `render_wav_muted` paths are unchanged.
        limiter.process(&mut buffer[..frames * 2]);
        for &sample in &buffer[..frames * 2] {
            if bit_depth == 8 {
                // WAV 8-bit is written through hound's i8 sample; the top byte of
                // the 16-bit render is the natural down-conversion.
                writer.write_sample((sample >> 8) as i8)?;
            } else {
                writer.write_sample(sample)?;
            }
        }
        on_progress(engine.position().frames_rendered);
        if frames < buffer.len() / 2 {
            break;
        }
    }

    writer.finalize()?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV1, OplType};

    fn small_song() -> Song {
        Song::dro_v1(
            "small.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, 0x40, 0x10, 0x60, 0xF0, 0x80, 0x77, // operator setup
                0xA0, 0x98, 0xB0, 0x31, // key on
                0x00, 0x63, // 100 ms delay
                0xB0, 0x11, // key off
                0x00, 0x31, // 50 ms delay
            ])
            .unwrap(),
            150,
            OplType::Opl2,
        )
    }

    fn read_back(bytes: &[u8]) -> (WavSpec, Vec<i32>) {
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let spec = reader.spec();
        let samples = reader
            .into_samples::<i32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (spec, samples)
    }

    #[test]
    fn renders_a_16_bit_stereo_wav_of_the_right_length() {
        let song = small_song();
        let bytes = render_wav(&song, 48_000, 16).unwrap();
        let (spec, samples) = read_back(&bytes);

        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        // 150 ms at 48 kHz is 7200 frames, two samples each.
        assert_eq!(samples.len(), 150 * 48 * 2);
    }

    #[test]
    fn progress_is_reported_without_changing_the_render() {
        let song = small_song();
        let plain = render_wav_boosted(&song, 48_000, 16, 1.0).unwrap();
        let mut frames = Vec::new();
        let tracked = render_wav_boosted_with_progress(&song, 48_000, 16, 1.0, &mut |rendered| {
            frames.push(rendered);
        })
        .unwrap();
        assert_eq!(
            tracked, plain,
            "progress reporting must not change the bytes"
        );
        assert!(!frames.is_empty(), "progress was reported");
        assert!(
            frames.windows(2).all(|pair| pair[0] <= pair[1]),
            "the reported frame count only grows"
        );
        assert!(*frames.last().unwrap() > 0);
    }

    #[test]
    fn muted_progress_is_reported_without_changing_the_render() {
        let song = small_song();
        let plain = render_wav_muted(&song, Muting::all(), 48_000, 16).unwrap();
        let mut frames = Vec::new();
        let tracked =
            render_wav_muted_with_progress(&song, Muting::all(), 48_000, 16, &mut |rendered| {
                frames.push(rendered)
            })
            .unwrap();
        assert_eq!(
            tracked, plain,
            "progress reporting must not change the bytes"
        );
        assert!(!frames.is_empty(), "progress was reported");
        assert!(
            frames.windows(2).all(|pair| pair[0] <= pair[1]),
            "the reported frame count only grows"
        );
        assert!(*frames.last().unwrap() > 0);
    }

    #[test]
    fn the_render_is_not_silent() {
        let song = small_song();
        let bytes = render_wav(&song, 48_000, 16).unwrap();
        let (_, samples) = read_back(&bytes);
        assert!(
            samples.iter().any(|&s| s != 0),
            "keyed-on note made no sound"
        );
    }

    #[test]
    fn eight_bit_export_round_trips_through_hound() {
        let song = small_song();
        let bytes = render_wav(&song, 48_000, 8).unwrap();
        let (spec, samples) = read_back(&bytes);
        assert_eq!(spec.bits_per_sample, 8);
        assert_eq!(samples.len(), 150 * 48 * 2);
        assert!(samples.iter().any(|&s| s != 0));
    }

    #[test]
    fn unity_boost_render_is_byte_identical_to_the_plain_render() {
        // The limiter bypasses at boost 1.0, so an opt-in boosted render with no
        // actual boost is the same faithful render as `render_wav`.
        let song = small_song();
        let plain = render_wav(&song, 48_000, 16).unwrap();
        let unity = render_wav_boosted(&song, 48_000, 16, 1.0).unwrap();
        assert_eq!(plain, unity);
    }

    #[test]
    fn the_default_mix_renders_exactly_what_render_wav_does() {
        // Everything audible, the song's own image, no boost -- so the dialog's
        // "none of the options" is the same faithful render the CLI produces.
        let song = small_song();
        let plain = render_wav(&song, 48_000, 16).unwrap();
        let mixed = render_wav_mixed(&song, RenderMix::default(), 48_000, 16).unwrap();
        assert_eq!(plain, mixed);
    }

    #[test]
    fn each_mix_option_alone_matches_its_single_purpose_render() {
        let song = small_song();
        let mut muting = Muting::silent();
        muting.allow_channel(dro_core::Bank::Low, 0xB0);

        assert_eq!(
            render_wav_mixed(
                &song,
                RenderMix {
                    muting,
                    ..RenderMix::default()
                },
                48_000,
                16
            )
            .unwrap(),
            render_wav_muted(&song, muting, 48_000, 16).unwrap(),
        );
        assert_eq!(
            render_wav_mixed(
                &song,
                RenderMix {
                    boost: 4.0,
                    ..RenderMix::default()
                },
                48_000,
                16
            )
            .unwrap(),
            render_wav_boosted(&song, 48_000, 16, 4.0).unwrap(),
        );
    }

    #[test]
    fn panning_a_render_hard_left_moves_the_energy() {
        let song = small_song();
        let hard_left = render_wav_mixed(
            &song,
            RenderMix {
                panning: Panning::Custom([0x00; 18]),
                ..RenderMix::default()
            },
            48_000,
            16,
        )
        .unwrap();
        let (_, samples) = read_back(&hard_left);

        let energy = |channel: usize| {
            samples
                .iter()
                .skip(channel)
                .step_by(2)
                .map(|&v| i64::from(v.abs()))
                .sum::<i64>()
        };
        assert!(energy(0) > 0, "the left channel should carry the song");
        assert!(
            energy(0) > energy(1) * 4,
            "hard left should leave little on the right: {} vs {}",
            energy(0),
            energy(1)
        );
    }

    #[test]
    fn a_boosted_render_is_louder_but_never_clips() {
        let song = small_song();
        let plain = render_wav(&song, 48_000, 16).unwrap();
        let boosted = render_wav_boosted(&song, 48_000, 16, 4.0).unwrap();
        let (_, plain_s) = read_back(&plain);
        let (_, boosted_s) = read_back(&boosted);
        assert_eq!(plain_s.len(), boosted_s.len());

        // Boosting the quiet portions raises the overall level...
        let energy = |s: &[i32]| s.iter().map(|&v| i64::from(v.abs())).sum::<i64>();
        assert!(
            energy(&boosted_s) > energy(&plain_s),
            "boost should raise the overall level"
        );
        // ...while the limiter keeps every sample inside full scale.
        assert!(
            boosted_s.iter().all(|&s| s.abs() <= 32_767),
            "the limiter must prevent clipping"
        );
    }
}
