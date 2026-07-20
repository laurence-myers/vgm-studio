//! Splitting a song into one file per channel (Python `dro_split.py`).
//!
//! Pure: [`split`] returns named outputs (WAV bytes, or a captured DRO song); the
//! `dro_split` bin writes them. Each channel is rendered (or captured) with all
//! other channels muted, using the register-usage analysis to skip channels the
//! song never touches.

use anyhow::Result;

use dro_core::config::AudioConfig;
use dro_core::{Bank, OplType, RegisterUsage, Song};
use dro_synth::{Muting, capture, render_wav_muted_with_progress};

/// Output format for a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFormat {
    /// One WAV per channel (the default).
    Wav,
    /// One DRO file per channel.
    Dro,
}

/// The contents of one split output.
#[derive(Debug)]
pub enum SplitData {
    Wav(Vec<u8>),
    Dro(Song),
}

/// One split file: its name and contents.
#[derive(Debug)]
pub struct SplitOutput {
    pub name: String,
    pub data: SplitData,
}

/// How to split.
#[derive(Debug, Clone, Copy)]
pub struct SplitOptions {
    pub format: SplitFormat,
    pub isolate_percussion: bool,
    pub audio: AudioConfig,
}

/// The five percussion voices of register `0xBD`, low bit first, with the drum
/// names `dro_split` gives their files. The order matches the Python's
/// `sorted(percs)` and `PERC_NAME_MAP`.
const DRUMS: [(u8, &str); 5] = [
    (0x01, "HH"),
    (0x02, "CY"),
    (0x04, "TT"),
    (0x08, "SD"),
    (0x10, "BD"),
];

/// Splits `song` into one output per channel it actually uses.
///
/// `on_skip` is called with each channel register (`0xB0..=0xB8`, `0xBD`, and
/// their high-bank `0x1xx` forms) the song never writes, so the CLI can report
/// it. `on_progress` is called during each WAV render with the output's base name
/// and the running rendered-frame count, for a live progress line. Both are no-ops
/// for a headless caller.
///
/// # Errors
/// If a channel cannot be captured -- a VGM input, or more distinct registers
/// than the DRO codemap can hold.
pub fn split(
    song: &Song,
    options: &SplitOptions,
    on_skip: &mut dyn FnMut(u16),
    on_progress: &mut dyn FnMut(&str, u64),
) -> Result<Vec<SplitOutput>> {
    let usage = RegisterUsage::analyze(song, options.isolate_percussion);
    let mut outputs = Vec::new();

    for channel in channels_to_render(song.opl_type) {
        if usage.count(channel) == 0 {
            on_skip(channel); // never written -> nothing to render
            continue;
        }
        let bank = if channel & 0x100 != 0 {
            Bank::High
        } else {
            Bank::Low
        };
        let bank_num = channel >> 8;
        let channel_num = (channel & 0xFF) - 0xAF; // 0xB0 -> 1, 0xB8 -> 9, 0xBD -> 14

        if options.isolate_percussion && (channel & 0xFF) == 0xBD {
            split_percussion(
                song,
                options,
                &usage,
                bank,
                bank_num,
                &mut outputs,
                on_progress,
            )?;
        } else {
            let mut muting = Muting::silent();
            if (channel & 0xFF) == 0xBD {
                muting.set_percussion(bank, 0xFF); // all drums on this bank
            } else {
                muting.allow_channel(bank, (channel & 0xFF) as u8);
            }
            let base = format!("{}.{}.{:02}", song.name, bank_num, channel_num);
            outputs.push(render_one(song, muting, options, base, on_progress)?);
        }
    }
    Ok(outputs)
}

/// Isolates each used drum of the percussion channel on `bank` to its own file.
///
/// Unlike the Python -- whose `p <= 16` filter silently dropped the high bank's
/// drums -- this isolates drums per bank correctly.
fn split_percussion(
    song: &Song,
    options: &SplitOptions,
    usage: &RegisterUsage,
    bank: Bank,
    bank_num: u16,
    outputs: &mut Vec<SplitOutput>,
    on_progress: &mut dyn FnMut(&str, u64),
) -> Result<()> {
    for (mask, name) in DRUMS {
        let key = (u16::from(bank.index()) << 8) | u16::from(mask);
        if !usage.percussion_used(key) {
            continue;
        }
        let mut muting = Muting::silent();
        muting.set_percussion(bank, 0xE0 | mask); // keep control bits, one drum
        let base = format!("{}.{}.14.{}", song.name, bank_num, name);
        outputs.push(render_one(song, muting, options, base, on_progress)?);
    }
    Ok(())
}

/// Renders one muted view of `song` into the configured format. A WAV render
/// reports progress as `(base, frames_rendered)`; a DRO capture is not
/// frame-progressive, so it reports nothing.
fn render_one(
    song: &Song,
    muting: Muting,
    options: &SplitOptions,
    base: String,
    on_progress: &mut dyn FnMut(&str, u64),
) -> Result<SplitOutput> {
    Ok(match options.format {
        SplitFormat::Wav => SplitOutput {
            name: format!("{base}.wav"),
            data: SplitData::Wav(render_wav_muted_with_progress(
                song,
                muting,
                options.audio.frequency,
                options.audio.bit_depth,
                &mut |frames| on_progress(&base, frames),
            )?),
        },
        SplitFormat::Dro => {
            let name = format!("{base}.out.dro");
            let dro = capture(song, muting, name.clone())?;
            SplitOutput {
                name,
                data: SplitData::Dro(dro),
            }
        }
    })
}

/// The channels to consider, in `dro_split`'s order: melodic `0xB0..=0xB8` (bank
/// 0 then bank 1), then percussion `0xBD` (bank 0 then bank 1). OPL2 keeps only
/// the low bank.
fn channels_to_render(opl_type: OplType) -> Vec<u16> {
    let mut channels: Vec<u16> = (0xB0u16..=0xB8).collect();
    channels.extend((0xB0u16..=0xB8).map(|reg| 0x100 | reg));
    channels.push(0xBD);
    channels.push(0x1BD);
    if opl_type == OplType::Opl2 {
        channels.retain(|&channel| channel < 0x100);
    }
    channels
}

#[cfg(test)]
mod tests {
    use super::*;
    use dro_core::io::{read_song, write_song};
    use dro_core::{DroDataV1, OplType};

    /// A small OPL2 song touching channels 0 and 1 and the percussion register,
    /// so a split produces a few outputs without rendering a 99-second fixture.
    fn small_song() -> Song {
        Song::dro_v1(
            "s.dro".to_owned(),
            DroDataV1::new(vec![
                0x20, 0x01, 0xA0, 0x98, 0xB0, 0x31, // channel 0: operator, freq, key on
                0x21, 0x01, 0xA1, 0x98, 0xB1, 0x31, // channel 1
                0xBD, 0x31, // percussion: mode + BD + HH
                0x00, 0x63, // 100 ms
            ])
            .unwrap(),
            100,
            OplType::Opl2,
        )
    }

    fn options(format: SplitFormat, isolate_percussion: bool) -> SplitOptions {
        SplitOptions {
            format,
            isolate_percussion,
            audio: AudioConfig::default(),
        }
    }

    #[test]
    fn splits_only_the_used_channels() {
        let outputs = split(
            &small_song(),
            &options(SplitFormat::Dro, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        // Channels 0 and 1 (0xB0, 0xB1) and percussion (0xBD) were written; the
        // other seven melodic channels were not.
        assert_eq!(outputs.len(), 3);
        assert!(outputs.iter().any(|o| o.name.contains(".0.01.")));
        assert!(outputs.iter().any(|o| o.name.contains(".0.02.")));
        assert!(outputs.iter().any(|o| o.name.contains(".0.14.")));
    }

    #[test]
    fn each_dro_split_parses_and_keeps_the_length() {
        let song = small_song();
        let outputs = split(
            &song,
            &options(SplitFormat::Dro, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        for output in &outputs {
            assert!(output.name.ends_with(".out.dro"));
            let SplitData::Dro(dro) = &output.data else {
                panic!("DRO split produced a WAV")
            };
            let bytes = write_song(dro).unwrap();
            let reread = read_song(&output.name, &bytes).unwrap();
            assert_eq!(reread.total_delay_ms(), song.total_delay_ms());
        }
    }

    #[test]
    fn each_wav_split_is_a_full_length_wav() {
        let outputs = split(
            &small_song(),
            &options(SplitFormat::Wav, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        assert!(!outputs.is_empty());
        for output in &outputs {
            assert!(output.name.ends_with(".wav"));
            let SplitData::Wav(bytes) = &output.data else {
                panic!("WAV split produced a DRO")
            };
            assert!(bytes.starts_with(b"RIFF"), "not a WAV: {}", output.name);
        }
    }

    #[test]
    fn isolating_percussion_names_each_used_drum() {
        // The song's 0xBD = 0x31 sets BD (0x10) and HH (0x01).
        let outputs = split(
            &small_song(),
            &options(SplitFormat::Dro, true),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        let names: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains(".14.BD.")), "{names:?}");
        assert!(names.iter().any(|n| n.contains(".14.HH.")), "{names:?}");
        // SD/CY/TT were not set, so they are not rendered.
        assert!(!names.iter().any(|n| n.contains(".14.SD.")), "{names:?}");
    }

    #[test]
    fn opl2_songs_only_use_the_low_bank() {
        let channels = channels_to_render(OplType::Opl2);
        assert!(channels.iter().all(|&c| c < 0x100));
        assert!(channels.contains(&0xBD));
        let opl3 = channels_to_render(OplType::Opl3);
        assert!(opl3.contains(&0x1B0));
        assert!(opl3.contains(&0x1BD));
    }
}
