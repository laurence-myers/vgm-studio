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

use crate::engine::PlayerEngine;

/// Renders `song` to a stereo WAV file held in memory.
///
/// `bit_depth` must be `8` or `16` (as [`dro_core::config::AudioConfig`]
/// guarantees). The chip always renders 16-bit internally; an 8-bit request is
/// down-converted at write time, since -- unlike PyOPL -- the Rust core has no
/// 8-bit mode. `chip_write_delay` is microseconds per register write.
///
/// # Errors
/// If the `hound` writer fails. Writing to an in-memory `Cursor` does not fail in
/// practice, so this is effectively infallible.
pub fn render_wav(
    song: &Song,
    sample_rate: u32,
    bit_depth: u16,
    chip_write_delay: f64,
) -> Result<Vec<u8>, hound::Error> {
    render_wav_from(song, sample_rate, bit_depth, chip_write_delay)
}

/// As [`render_wav`], but generic over the song container so the audio thread can
/// pass an `Arc<Song>` without cloning.
///
/// # Errors
/// See [`render_wav`].
pub fn render_wav_from<B: Borrow<Song>>(
    song: B,
    sample_rate: u32,
    bit_depth: u16,
    chip_write_delay: f64,
) -> Result<Vec<u8>, hound::Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: bit_depth,
        sample_format: SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)?;

    let mut engine = PlayerEngine::new(song, sample_rate, chip_write_delay);
    let mut buffer = vec![0i16; 4096 * 2];
    loop {
        let frames = engine.render(&mut buffer);
        for &sample in &buffer[..frames * 2] {
            if bit_depth == 8 {
                // WAV 8-bit is written through hound's i8 sample; the top byte of
                // the 16-bit render is the natural down-conversion.
                writer.write_sample((sample >> 8) as i8)?;
            } else {
                writer.write_sample(sample)?;
            }
        }
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
        let bytes = render_wav(&song, 48_000, 16, 0.0).unwrap();
        let (spec, samples) = read_back(&bytes);

        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        // 150 ms at 48 kHz is 7200 frames, two samples each.
        assert_eq!(samples.len(), 150 * 48 * 2);
    }

    #[test]
    fn the_render_is_not_silent() {
        let song = small_song();
        let bytes = render_wav(&song, 48_000, 16, 0.0).unwrap();
        let (_, samples) = read_back(&bytes);
        assert!(
            samples.iter().any(|&s| s != 0),
            "keyed-on note made no sound"
        );
    }

    #[test]
    fn eight_bit_export_round_trips_through_hound() {
        let song = small_song();
        let bytes = render_wav(&song, 48_000, 8, 0.0).unwrap();
        let (spec, samples) = read_back(&bytes);
        assert_eq!(spec.bits_per_sample, 8);
        assert_eq!(samples.len(), 150 * 48 * 2);
        assert!(samples.iter().any(|&s| s != 0));
    }
}
