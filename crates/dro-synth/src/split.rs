//! Splitting a song into one file per channel.
//!
//! Pure: [`split`] returns named outputs (WAV bytes, or a captured song); the
//! caller writes them -- `drotrim split` to disk, the GUI to a chosen folder.
//! Each channel is rendered (or captured) with all other channels muted, using
//! the register-usage analysis to skip channels the song never touches.

use dro_core::config::AudioConfig;
use dro_core::{Bank, Error, OplType, RegisterUsage, Result, Song};

use crate::{Muting, RenderMix, capture, render_wav_cancellable};

/// Output format for a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFormat {
    /// One WAV per channel (the default).
    Wav,
    /// One song file per channel, in the same format as the input: a DRO for a
    /// DRO, a VGM for a VGM.
    Song,
}

/// The contents of one split output.
#[derive(Debug)]
pub enum SplitData {
    Wav(Vec<u8>),
    Song(Song),
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
/// names `dro_split` gives their files.
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
/// If a channel cannot be rendered, or cannot be captured -- a DRO capture needing
/// more distinct registers than its codemap can hold.
pub fn split(
    song: &Song,
    options: &SplitOptions,
    on_skip: &mut dyn FnMut(u16),
    on_progress: &mut dyn FnMut(&str, u64),
) -> Result<Vec<SplitOutput>> {
    Ok(
        split_cancellable(song, options, on_skip, on_progress, &mut || true)?
            .expect("a split that is never cancelled always completes"),
    )
}

/// As [`split`], but calling `keep_going` as it renders so a background split can
/// be abandoned part-way. `Ok(None)` iff `keep_going` returned `false`.
///
/// # Errors
/// See [`split`].
pub fn split_cancellable(
    song: &Song,
    options: &SplitOptions,
    on_skip: &mut dyn FnMut(u16),
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<SplitOutput>>> {
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
            if split_percussion(
                song,
                options,
                &usage,
                bank,
                bank_num,
                &mut outputs,
                on_progress,
                keep_going,
            )?
            .is_none()
            {
                return Ok(None);
            }
        } else {
            let mut muting = Muting::silent();
            if (channel & 0xFF) == 0xBD {
                muting.set_percussion(bank, 0xFF); // all drums on this bank
            } else {
                muting.allow_channel(bank, (channel & 0xFF) as u8);
            }
            let base = format!("{}.{}.{:02}", song.name, bank_num, channel_num);
            let Some(output) = render_one(song, muting, options, base, on_progress, keep_going)?
            else {
                return Ok(None);
            };
            outputs.push(output);
        }
    }
    Ok(Some(outputs))
}

/// Isolates each used drum of the percussion channel on `bank` to its own file.
///
/// This isolates drums per bank correctly.
///
/// `Ok(None)` if `keep_going` asked it to stop part-way.
#[allow(clippy::too_many_arguments)]
fn split_percussion(
    song: &Song,
    options: &SplitOptions,
    usage: &RegisterUsage,
    bank: Bank,
    bank_num: u16,
    outputs: &mut Vec<SplitOutput>,
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<()>> {
    for (mask, name) in DRUMS {
        let key = (u16::from(bank.index()) << 8) | u16::from(mask);
        if !usage.percussion_used(key) {
            continue;
        }
        let mut muting = Muting::silent();
        muting.set_percussion(bank, 0xE0 | mask); // keep control bits, one drum
        let base = format!("{}.{}.14.{}", song.name, bank_num, name);
        let Some(output) = render_one(song, muting, options, base, on_progress, keep_going)? else {
            return Ok(None);
        };
        outputs.push(output);
    }
    Ok(Some(()))
}

/// Renders one muted view of `song` into the configured format. A WAV render
/// reports progress as `(base, frames_rendered)`; a capture is not
/// frame-progressive, so it reports nothing.
fn render_one(
    song: &Song,
    muting: Muting,
    options: &SplitOptions,
    base: String,
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<SplitOutput>> {
    Ok(match options.format {
        SplitFormat::Wav => {
            let mix = RenderMix {
                muting,
                ..RenderMix::default()
            };
            let rendered = render_wav_cancellable(
                song,
                mix,
                options.audio.frequency,
                options.audio.bit_depth,
                &mut |frames| on_progress(&base, frames),
                keep_going,
            )
            .map_err(|e| Error::file(format!("Rendering {base} to WAV failed: {e}")))?;
            rendered.map(|bytes| SplitOutput {
                name: format!("{base}.wav"),
                data: SplitData::Wav(bytes),
            })
        }
        // A capture writes no audio, so it finishes fast enough that stopping
        // between channels (which the caller does) is soon enough.
        SplitFormat::Song if !keep_going() => None,
        SplitFormat::Song => {
            // A capture keeps the input's format, so the name must too.
            let name = format!("{base}.out.{}", if song.is_vgm() { "vgm" } else { "dro" });
            let captured = capture(song, muting, name.clone())?;
            Some(SplitOutput {
                name,
                data: SplitData::Song(captured),
            })
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
    use dro_core::{DroDataV1, DroInstruction, OplType};

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
            &options(SplitFormat::Song, false),
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
    fn each_song_split_parses_and_keeps_the_length() {
        let song = small_song();
        let outputs = split(
            &song,
            &options(SplitFormat::Song, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        for output in &outputs {
            assert!(output.name.ends_with(".out.dro"));
            let SplitData::Song(dro) = &output.data else {
                panic!("song split produced a WAV")
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
                panic!("WAV split produced a song file")
            };
            assert!(bytes.starts_with(b"RIFF"), "not a WAV: {}", output.name);
        }
    }

    // -- what actually lands in each file ----------------------------------

    /// Writes `output` out and reads it back, so the assertions below are made
    /// against a real file's bytes rather than the in-memory song that produced
    /// them -- without ever touching the disk.
    fn round_trip(output: &SplitOutput) -> Song {
        let SplitData::Song(song) = &output.data else {
            panic!("{} is not a song file", output.name)
        };
        let bytes = write_song(song).unwrap();
        read_song(&output.name, &bytes).unwrap()
    }

    fn find<'a>(outputs: &'a [SplitOutput], fragment: &str) -> &'a SplitOutput {
        outputs
            .iter()
            .find(|o| o.name.contains(fragment))
            .unwrap_or_else(|| {
                let names: Vec<&str> = outputs.iter().map(|o| o.name.as_str()).collect();
                panic!("no output matching {fragment:?} in {names:?}")
            })
    }

    /// Whether `song` writes `value` to `reg`, on either bank.
    fn writes(song: &Song, reg: u8, value: u8) -> bool {
        song.data().iter().any(|i| {
            matches!(i, DroInstruction::Register { reg: r, value: v, .. } if r == reg && v == value)
        })
    }

    /// The heart of a split: each file must carry its own channel's key-on and
    /// nobody else's.
    #[test]
    fn each_dro_channel_file_keeps_only_its_own_key_on() {
        let song = small_song();
        let outputs = split(
            &song,
            &options(SplitFormat::Song, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();

        let channel_0 = round_trip(find(&outputs, ".0.01."));
        assert!(writes(&channel_0, 0xB0, 0x31), "channel 0 kept its key-on");
        assert!(!writes(&channel_0, 0xB1, 0x31), "channel 1 leaked in");

        let channel_1 = round_trip(find(&outputs, ".0.02."));
        assert!(writes(&channel_1, 0xB1, 0x31), "channel 1 kept its key-on");
        assert!(!writes(&channel_1, 0xB0, 0x31), "channel 0 leaked in");

        // Neither melodic file plays the drums: 0xBD is masked to its control
        // bits (0x20 keeps percussion mode, the five drum bits are cleared).
        for file in [&channel_0, &channel_1] {
            assert!(!writes(file, 0xBD, 0x31), "the drums leaked in");
            assert!(writes(file, 0xBD, 0x20), "0xBD should survive masked");
        }
        // The percussion file is the mirror image.
        let drums = round_trip(find(&outputs, ".0.14."));
        assert!(writes(&drums, 0xBD, 0x31), "the drum file kept its drums");
        assert!(!writes(&drums, 0xB0, 0x31), "a melodic channel leaked in");

        // Every file still runs for exactly as long as the original.
        for output in &outputs {
            assert_eq!(round_trip(output).total_delay_ms(), song.total_delay_ms());
        }
    }

    /// The same split over the same music as a VGM: same channel separation,
    /// same timing, VGM files out.
    #[test]
    fn each_vgm_channel_file_keeps_only_its_own_key_on() {
        let song = dro_core::convert::dro_to_vgm(&small_song()).unwrap();
        let outputs = split(
            &song,
            &options(SplitFormat::Song, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(outputs.len(), 3);

        for output in &outputs {
            assert!(output.name.ends_with(".out.vgm"), "{}", output.name);
            let SplitData::Song(vgm) = &output.data else {
                panic!("song split produced a WAV")
            };
            assert!(write_song(vgm).unwrap().starts_with(b"Vgm "));
            assert_eq!(
                round_trip(output).total_delay_samples(),
                song.total_delay_samples()
            );
        }

        let channel_0 = round_trip(find(&outputs, ".0.01."));
        assert!(writes(&channel_0, 0xB0, 0x31), "channel 0 kept its key-on");
        assert!(!writes(&channel_0, 0xB1, 0x31), "channel 1 leaked in");

        let channel_1 = round_trip(find(&outputs, ".0.02."));
        assert!(writes(&channel_1, 0xB1, 0x31), "channel 1 kept its key-on");
        assert!(!writes(&channel_1, 0xB0, 0x31), "channel 0 leaked in");
    }

    /// A VGM's loop survives the split, still pointing at the same music.
    #[test]
    fn a_vgm_split_keeps_the_loop() {
        let mut song = dro_core::convert::dro_to_vgm(&small_song()).unwrap();
        let loop_point = song.len() - 1; // the trailing delay
        song.vgm_meta_mut().unwrap().loop_point = Some(loop_point);

        let outputs = split(
            &song,
            &options(SplitFormat::Song, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();

        for output in &outputs {
            let split_song = round_trip(output);
            let meta = split_song.vgm_meta().unwrap();
            assert!(meta.loop_point.is_some(), "{} lost its loop", output.name);
            // The loop covers the same music: everything from that delay on.
            assert_eq!(
                split_song.loop_num_samples(),
                song.loop_num_samples(),
                "{} looped a different span",
                output.name
            );
        }
    }

    #[test]
    fn isolating_percussion_names_each_used_drum() {
        // The song's 0xBD = 0x31 sets BD (0x10) and HH (0x01).
        let outputs = split(
            &small_song(),
            &options(SplitFormat::Song, true),
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
    fn a_cancelled_split_produces_nothing() {
        let cancelled = split_cancellable(
            &small_song(),
            &options(SplitFormat::Wav, false),
            &mut |_| {},
            &mut |_, _| {},
            &mut || false,
        )
        .unwrap();
        assert!(cancelled.is_none(), "a cancelled split has no outputs");
    }

    /// Cancelling between channels stops the split rather than returning a
    /// partial set the caller might write out as if it were complete.
    #[test]
    fn cancelling_part_way_abandons_the_whole_split() {
        let mut channels = 0;
        let cancelled = split_cancellable(
            &small_song(),
            &options(SplitFormat::Song, false),
            &mut |_| {},
            &mut |_, _| {},
            &mut || {
                channels += 1;
                channels <= 1
            },
        )
        .unwrap();
        assert!(cancelled.is_none());
    }

    #[test]
    fn an_uncancelled_split_matches_the_plain_one() {
        let song = small_song();
        let plain = split(
            &song,
            &options(SplitFormat::Wav, false),
            &mut |_| {},
            &mut |_, _| {},
        )
        .unwrap();
        let same = split_cancellable(
            &song,
            &options(SplitFormat::Wav, false),
            &mut |_| {},
            &mut |_, _| {},
            &mut || true,
        )
        .unwrap()
        .expect("not cancelled");

        let names = |outputs: &[SplitOutput]| -> Vec<String> {
            outputs.iter().map(|o| o.name.clone()).collect()
        };
        assert_eq!(names(&plain), names(&same));
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
