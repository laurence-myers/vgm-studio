//! Offline WAV rendering.
//!
//! A plain loop over an engine's `render`, writing into an in-memory `hound`
//! WAV. The same bytes result on native and web -- the caller writes them to disk
//! or offers them as a download.
//!
//! Two engines, one loop: [`PlayerEngine`] for a DRO or an OPL VGM (with the
//! muting and panning that only mean something there), and
//! [`VgmEngine`](crate::vgm_engine::VgmEngine) for a VGM of any other chips.

use std::borrow::Borrow;
use std::io::Cursor;

use dro_core::Song;
use hound::{SampleFormat, WavSpec, WavWriter};

use crate::engine::{Muting, Panning, PlayerEngine};
use crate::resample::ResampleMode;
use std::sync::Arc;

use dro_core::VgmFile;

use crate::limiter::BoostLimiter;
use crate::vgm_engine::VgmEngine;

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
/// down-converted at write time, since the core has no 8-bit mode.
///
/// # Errors
/// If the `hound` writer fails. Writing to an in-memory `Cursor` does not fail in
/// practice, so this is effectively infallible.
pub fn render_wav(song: &Song, sample_rate: u32, bit_depth: u16) -> Result<Vec<u8>, hound::Error> {
    render_uncancelled(
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
    render_uncancelled(song, mix, sample_rate, bit_depth, &mut |_| {})
}

/// As [`render_wav_mixed`], reporting progress and calling `keep_going` between
/// render chunks so a background export can be abandoned part-way -- when the
/// song it belongs to is replaced, say. `Ok(None)` iff `keep_going` returned
/// `false`.
///
/// This is the entry point with everything exposed; the rest are the convenient
/// shorthands for it.
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_cancellable<B: Borrow<Song>>(
    song: B,
    mix: RenderMix,
    sample_rate: u32,
    bit_depth: u16,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    render_wav_impl(song, mix, sample_rate, bit_depth, on_progress, keep_going)
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
    render_uncancelled(song, mix, sample_rate, bit_depth, &mut |_| {})
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
    render_uncancelled(song, mix, sample_rate, bit_depth, on_progress)
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
    render_uncancelled(song, mix, sample_rate, bit_depth, &mut |_| {})
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
    render_uncancelled(song, mix, sample_rate, bit_depth, on_progress)
}

/// [`render_wav_impl`] for the entry points that cannot be cancelled, which is
/// every one but [`render_wav_cancellable`].
fn render_uncancelled<B: Borrow<Song>>(
    song: B,
    mix: RenderMix,
    sample_rate: u32,
    bit_depth: u16,
    on_progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, hound::Error> {
    Ok(
        render_wav_impl(song, mix, sample_rate, bit_depth, on_progress, &mut || true)?
            .expect("a render that is never cancelled always completes"),
    )
}

/// The shared render loop behind the public `render_wav*` entry points.
///
/// Returns `Ok(None)` when `keep_going` asks it to stop; the callers that cannot
/// be cancelled go through [`render_uncancelled`].
fn render_wav_impl<B: Borrow<Song>>(
    song: B,
    mix: RenderMix,
    sample_rate: u32,
    bit_depth: u16,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: bit_depth,
        sample_format: SampleFormat::Int,
    };
    let mut engine = PlayerEngine::new(song, sample_rate);
    engine.set_muting(mix.muting);
    // Only when it differs from the engine's own starting state: `set_panning`
    // is a chip write, and `Original` would replay the whole `0xC0` shadow for
    // nothing -- keeping the faithful render byte-for-byte what it always was.
    if mix.panning != Panning::Original {
        engine.set_panning(mix.panning);
    }
    write_render(
        spec,
        bit_depth,
        mix.boost,
        &mut |buffer| {
            let frames = engine.render(buffer);
            (frames, engine.position().frames_rendered)
        },
        on_progress,
        keep_going,
    )
}

/// Renders a VGM for whatever chips it declares, through the multi-chip engine.
///
/// The counterpart of [`render_wav_mixed`] for a file the OPL model does not
/// cover. There is no muting or panning: those are OPL ideas, and this engine
/// has no register policy at all. A chip with no core renders silence, so a file
/// this app only half knows comes out half played -- check
/// [`playability`](crate::chip::playability) first if that matters.
///
/// # Errors
/// If the WAV cannot be written -- the same failures as [`render_wav`].
pub fn render_vgm_wav(
    file: Arc<VgmFile>,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
    resampling: ResampleMode,
) -> Result<Vec<u8>, hound::Error> {
    render_vgm_wav_cancellable(
        file,
        sample_rate,
        bit_depth,
        boost,
        resampling,
        &mut |_| {},
        &mut || true,
    )
    .map(|bytes| bytes.unwrap_or_default())
}

/// As [`render_vgm_wav`], reporting progress and stopping when `keep_going`
/// returns `false`. `Ok(None)` iff it did.
///
/// # Errors
/// See [`render_vgm_wav`].
pub fn render_vgm_wav_cancellable(
    file: Arc<VgmFile>,
    sample_rate: u32,
    bit_depth: u16,
    boost: f32,
    resampling: ResampleMode,
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: bit_depth,
        sample_format: SampleFormat::Int,
    };
    let mut engine = VgmEngine::new(file, sample_rate);
    // The render honours the same choice playback does: a user who picked the
    // crunchy conversion exports the sound they hear, not a cleaned-up cousin.
    engine.set_resample_mode(resampling);
    let mut rendered = 0u64;
    write_render(
        spec,
        bit_depth,
        boost,
        &mut |buffer| {
            let frames = engine.render(buffer);
            rendered += frames as u64;
            (frames, rendered)
        },
        on_progress,
        keep_going,
    )
}

/// The write loop both renderers share: pull frames, boost and limit them,
/// encode them, and stop when the source runs out or the caller says so.
///
/// `pull` fills the buffer and reports `(frames written, frames rendered so
/// far)` -- the second being what a progress bar counts, which the two engines
/// track differently.
fn write_render(
    spec: WavSpec,
    bit_depth: u16,
    boost: f32,
    pull: &mut dyn FnMut(&mut [i16]) -> (usize, u64),
    on_progress: &mut dyn FnMut(u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>, hound::Error> {
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)?;
    let mut limiter = BoostLimiter::new(spec.sample_rate, boost);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        // Between chunks, as the waveform render does: often enough that an
        // abandoned export stops promptly, never mid-buffer.
        if !keep_going() {
            return Ok(None);
        }
        let (frames, rendered) = pull(&mut buffer);
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
        on_progress(rendered);
        if frames < buffer.len() / 2 {
            break;
        }
    }

    writer.finalize()?;
    Ok(Some(cursor.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::{DroDataV1, OplType};

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

    /// A Master System rip: a tone at full volume for a second.
    fn sms_vgm() -> Arc<VgmFile> {
        vgm_file(
            &[(dro_core::ChipKind::Sn76489, 3_579_545)],
            &[
                0x50, 0x8E, 0x50, 0x0F, // tone 0, period 254
                0x50, 0x90, // full volume
                0x61, 0x44, 0xAC, // a second
                0x66,
            ],
        )
    }

    #[test]
    fn a_vgm_for_other_chips_renders_a_wav_of_its_own_length() {
        let bytes =
            render_vgm_wav(sms_vgm(), 44_100, 16, 1.0, ResampleMode::Sinc).expect("renders");
        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("a readable WAV");
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 44_100);
        // One second, in stereo samples.
        assert_eq!(reader.len(), 44_100 * 2);
    }

    #[test]
    fn a_chip_this_app_can_play_comes_out_audible() {
        let bytes =
            render_vgm_wav(sms_vgm(), 44_100, 16, 1.0, ResampleMode::Sinc).expect("renders");
        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("a readable WAV");
        let peak = reader
            .into_samples::<i16>()
            .filter_map(Result::ok)
            .map(i16::abs)
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "a square wave, not silence: peak {peak}");
    }

    #[test]
    fn a_chip_this_app_cannot_play_comes_out_silent_rather_than_failing() {
        // A YM2612 rip: readable, playable in the sense that it renders, and
        // silent because there is no core. Better than a refusal.
        let file = vgm_file(
            &[(dro_core::ChipKind::Ym2612, 7_670_454)],
            &[0x52, 0x28, 0xF0, 0x62, 0x66],
        );
        let bytes =
            render_vgm_wav(file, 44_100, 16, 1.0, ResampleMode::Sinc).expect("renders anyway");
        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("a readable WAV");
        assert_eq!(reader.len(), 735 * 2);
        assert!(
            reader
                .into_samples::<i16>()
                .filter_map(Result::ok)
                .all(|sample| sample == 0)
        );
    }

    #[test]
    fn a_cancelled_vgm_render_yields_nothing() {
        let mut calls = 0;
        let outcome = render_vgm_wav_cancellable(
            sms_vgm(),
            44_100,
            16,
            1.0,
            ResampleMode::Sinc,
            &mut |_| {},
            &mut || {
                calls += 1;
                calls <= 1
            },
        )
        .expect("no write error");
        assert!(outcome.is_none(), "an abandoned render produces no file");
    }

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

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
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

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
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
    fn a_cancelled_render_stops_and_returns_nothing() {
        let song = small_song();
        // Refused before the first chunk.
        assert!(
            render_wav_cancellable(
                &song,
                RenderMix::default(),
                48_000,
                16,
                &mut |_| {},
                &mut || false
            )
            .unwrap()
            .is_none()
        );

        // ...and part-way through: the render stops early, so fewer chunks run
        // than the whole song needs.
        let mut chunks = 0;
        let cancelled = render_wav_cancellable(
            &song,
            RenderMix::default(),
            48_000,
            16,
            &mut |_| {},
            &mut || {
                chunks += 1;
                chunks <= 1
            },
        )
        .unwrap();
        assert!(cancelled.is_none());
    }

    #[test]
    fn an_uncancelled_render_is_identical_to_the_plain_one() {
        let song = small_song();
        let plain = render_wav(&song, 48_000, 16).unwrap();
        let cancellable = render_wav_cancellable(
            &song,
            RenderMix::default(),
            48_000,
            16,
            &mut |_| {},
            &mut || true,
        )
        .unwrap();
        assert_eq!(cancellable.as_deref(), Some(plain.as_slice()));
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

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
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

    // Asserts the OPL render is *not* silent, which only an OPL core can
    // make true. `--no-default-features` has none by design.
    #[cfg(feature = "nuked-opl")]
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
