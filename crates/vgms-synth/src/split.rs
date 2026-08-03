//! Splitting a song into one file per channel.
//!
//! Pure: [`split`] returns named outputs (WAV bytes, or a captured song); the
//! caller writes them -- `vgmstudio split` to disk, the GUI to a chosen folder.
//! Each channel is rendered (or captured) with all other channels muted, using
//! the register-usage analysis to skip channels the song never touches.

use std::collections::BTreeSet;
use std::sync::Arc;

use vgms_core::config::AudioConfig;
use vgms_core::vgm::{ChipKind, VgmCommand, VgmFile, channels_of};
use vgms_core::{Bank, Error, OplType, RegisterUsage, Result, Song};

use crate::resample::ResampleMode;
use crate::{
    ChipMuting, CoreChoices, Muting, RenderMix, VgmRenderMix, capture,
    render_vgm_wav_mixed_cancellable, render_wav_cancellable,
};

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
#[derive(Debug, Clone)]
pub struct SplitOptions {
    pub format: SplitFormat,
    pub isolate_percussion: bool,
    pub audio: AudioConfig,
    /// The per-render core choices to build each channel's engine with (slot
    /// slug -> core short-name), seeded from Settings but never persisted. The
    /// split functions do not read it; the caller applies it with
    /// [`with_render_choices`](crate::with_render_choices) around the split, so
    /// an empty map renders exactly as the configured cores would.
    pub core_choices: CoreChoices,
}

/// The five percussion voices of register `0xBD`, low bit first, with the drum
/// names the channel splitter gives their files.
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

/// How to split a multichip VGM.
#[derive(Debug, Clone)]
pub struct VgmSplitOptions {
    pub audio: AudioConfig,
    pub resampling: ResampleMode,
    /// As on [`SplitOptions`]: the per-render core choices, seeded from Settings
    /// and never persisted. Applied by the caller with
    /// [`with_render_choices`](crate::with_render_choices), so an empty map
    /// renders exactly as the configured cores would.
    pub core_choices: CoreChoices,
}

/// A peak at or below this fraction of full scale is treated as silence: a
/// channel that was configured but never sounded renders (near-)zero, and its
/// file is not worth writing. `1/1000` separates "never keyed" from "playing"
/// with room to spare.
const SILENCE_DIVISOR: i32 = 1000;

/// Splits a multichip VGM into one WAV per channel it actually sounds.
///
/// The chip-agnostic counterpart of [`split_cancellable`]: where that one reads
/// OPL register usage to know which channels a song touches, this cannot -- the
/// analysis is OPL-shaped -- so it *renders* each channel soloed and keeps only
/// the ones that come out above silence. A whole chip instance the stream never
/// writes is skipped without rendering (the pre-filter), and `on_skip` names
/// every channel dropped so the CLI can report it.
///
/// WAV only: a per-channel song output would need per-chip write gating, which
/// is out of scope; the OPL split keeps both formats.
///
/// `Ok(None)` iff `keep_going` asked it to stop part-way.
///
/// # Errors
/// If a channel cannot be rendered to WAV.
pub fn split_vgm_cancellable(
    file: &Arc<VgmFile>,
    options: &VgmSplitOptions,
    on_skip: &mut dyn FnMut(&str),
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<SplitOutput>>> {
    // Which chip instances the stream ever writes: a chip it never touches is
    // silent on every channel, so it is skipped without a render each.
    let written = written_instances(file);
    let full = if options.audio.bit_depth == 8 {
        127
    } else {
        32_767
    };
    let mut outputs = Vec::new();

    for chip in file.header.chips() {
        let instances = if chip.dual { 2 } else { 1 };
        let channels = channels_of(chip.kind, chip.variant);
        for instance in 0..instances {
            let ever_written = written.contains(&(chip.kind, instance));
            for (index, channel) in channels.iter().enumerate() {
                let name = channel_file_name(&file.name, chip.kind, instance, index, channel.short);
                if !ever_written {
                    on_skip(&name);
                    continue;
                }
                if !keep_going() {
                    return Ok(None);
                }
                // Solo this channel: every other instance fully muted, and
                // every other channel of this instance too.
                let mut muting = ChipMuting::new();
                for other in file.header.chips() {
                    let others = if other.dual { 2 } else { 1 };
                    for other_instance in 0..others {
                        muting.set(other.kind, other_instance, u32::MAX);
                    }
                }
                muting.set(chip.kind, instance, !(1u32 << index));

                let mix = VgmRenderMix {
                    muting,
                    ..VgmRenderMix::default()
                };
                let progress_name = name.clone();
                let rendered = render_vgm_wav_mixed_cancellable(
                    Arc::clone(file),
                    options.audio.frequency,
                    options.audio.bit_depth,
                    &mix,
                    options.resampling,
                    &mut |frames| on_progress(&progress_name, frames),
                    keep_going,
                )
                .map_err(|e| Error::file(format!("Rendering {name} to WAV failed: {e}")))?;
                let Some(bytes) = rendered else {
                    return Ok(None);
                };
                // A channel that never sounded renders (near-)silence; drop it.
                if wav_peak(&bytes) <= full / SILENCE_DIVISOR {
                    on_skip(&name);
                    continue;
                }
                outputs.push(SplitOutput {
                    name,
                    data: SplitData::Wav(bytes),
                });
            }
        }
    }
    Ok(Some(outputs))
}

/// The chip instances a stream writes at least once, so the split can skip a
/// chip it never touches without rendering every one of its channels.
fn written_instances(file: &VgmFile) -> BTreeSet<(ChipKind, u8)> {
    let mut written = BTreeSet::new();
    let Some(stream) = file.stream() else {
        return written;
    };
    for index in 0..stream.len() {
        if let Some(VgmCommand::Write { target, .. }) = stream.get(index) {
            written.insert((target.kind, target.instance));
        }
    }
    written
}

/// The file name for one channel: `<song>.<chip-slug>[#2].<NN>-<short>.wav`.
fn channel_file_name(
    song: &str,
    kind: ChipKind,
    instance: u8,
    index: usize,
    short: &str,
) -> String {
    let stem = song.strip_suffix(".vgm").unwrap_or(song);
    let inst = if instance == 0 {
        String::new()
    } else {
        format!("#{}", instance + 1)
    };
    format!("{stem}.{}{inst}.{index:02}-{short}.wav", kind.slug())
}

/// The largest absolute PCM sample in a rendered WAV, for the silence check.
fn wav_peak(bytes: &[u8]) -> i32 {
    let Ok(reader) = hound::WavReader::new(std::io::Cursor::new(bytes)) else {
        return 0;
    };
    reader
        .into_samples::<i32>()
        .filter_map(std::result::Result::ok)
        .map(i32::abs)
        .max()
        .unwrap_or(0)
}

/// The channels to consider, in the channel splitter's order: melodic `0xB0..=0xB8` (bank
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
mod vgm_split_tests {
    use super::*;

    /// A one-second SN76489 VGM with only Tone 1 turned on.
    fn tone1_vgm() -> Arc<VgmFile> {
        fn put(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put(&mut bytes, 0x08, 0x151);
        put(&mut bytes, 0x34, 0x100 - 0x34);
        put(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        // Latch Tone 1 volume to full (0x90 = channel 0, volume 0), then a
        // one-second delay so the render has something to measure.
        bytes.extend_from_slice(&[0x50, 0x90, 0x61, 0x44, 0xAC, 0x66]);
        let eof = bytes.len();
        put(&mut bytes, 0x04, (eof - 4) as u32);
        Arc::new(vgms_core::vgm::file::read("song.vgm", &bytes).unwrap())
    }

    fn options() -> VgmSplitOptions {
        VgmSplitOptions {
            audio: AudioConfig::default(),
            resampling: ResampleMode::default(),
            core_choices: CoreChoices::new(),
        }
    }

    /// Only the channel that sounds gets a file; the silent ones are skipped
    /// and named so the CLI can report them.
    #[test]
    fn a_split_keeps_the_sounding_channel_and_skips_the_rest() {
        crate::testing::install_registry_with_stub();
        let file = tone1_vgm();
        let mut skipped = Vec::new();
        let outputs = split_vgm_cancellable(
            &file,
            &options(),
            &mut |name| skipped.push(name.to_owned()),
            &mut |_, _| {},
            &mut || true,
        )
        .unwrap()
        .expect("not cancelled");

        // The SN76489 has four channels; only Tone 1 was on.
        assert_eq!(outputs.len(), 1, "one sounding channel");
        assert!(
            outputs[0].name.contains("00-T1"),
            "named for Tone 1: {}",
            outputs[0].name
        );
        assert_eq!(skipped.len(), 3, "the other three are skipped: {skipped:?}");
        assert!(matches!(outputs[0].data, SplitData::Wav(_)));
    }

    /// Cancelling part-way emits nothing, like the OPL split.
    #[test]
    fn a_cancelled_split_emits_nothing() {
        crate::testing::install_registry_with_stub();
        let file = tone1_vgm();
        let result =
            split_vgm_cancellable(&file, &options(), &mut |_| {}, &mut |_, _| {}, &mut || {
                false
            })
            .unwrap();
        assert!(result.is_none(), "a cancelled split completes with None");
    }

    /// The file names follow the chip slug and the channel's short label.
    #[test]
    fn channel_names_carry_the_chip_and_channel() {
        assert_eq!(
            channel_file_name("bios.vgm", ChipKind::Sn76489, 0, 3, "N"),
            "bios.sn76489.03-N.wav"
        );
        assert_eq!(
            channel_file_name("x.vgm", ChipKind::Ym2612, 1, 6, "DA"),
            "x.ym2612#2.06-DA.wav"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vgms_core::io::{read_song, write_song};
    use vgms_core::{DroDataV1, Instruction, OplType};

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
            core_choices: CoreChoices::new(),
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
            matches!(i, Instruction::Register { reg: r, value: v, .. } if r == reg && v == value)
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
        let song = vgms_core::convert::dro_to_vgm(&small_song()).unwrap();
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
        let mut song = vgms_core::convert::dro_to_vgm(&small_song()).unwrap();
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
