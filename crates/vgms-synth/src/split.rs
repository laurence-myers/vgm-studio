//! Splitting a VGM into one file per channel.
//!
//! Pure: [`split_vgm_cancellable`] returns named outputs (WAV bytes, or a
//! per-channel VGM); the caller writes them -- `vgmstudio split` to disk, the GUI
//! to a chosen folder. A WAV stem renders each channel soloed and keeps the ones
//! that come out above silence; a song-format stem rewrites the command stream
//! per channel (for chips a [`ChannelGate`] covers). An OPL document reaches here
//! as a VGM: a DRO projects, an OPL VGM splits from its own file (ou-4).

use std::collections::BTreeSet;
use std::sync::Arc;

use vgms_core::config::AudioConfig;
use vgms_core::vgm::{ChipKind, VgmCommand, VgmFile, channels_of};
use vgms_core::{Error, Result};

use crate::channel_gate::ChannelGate;
use crate::registry::{self, CoreRegistry};
use crate::resample::ResampleMode;
use crate::{ChipMuting, ChipPanning, CoreChoices, VgmRenderMix, render_vgm_wav_mixed_cancellable};

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
    /// The song-format output: one channel's command stream rewritten into its
    /// own VGM. Boxed because a whole [`VgmFile`] dwarfs the WAV variant.
    Vgm(Box<VgmFile>),
}

/// One split file: its name and contents.
#[derive(Debug)]
pub struct SplitOutput {
    pub name: String,
    pub data: SplitData,
}

/// How to split a multichip VGM.
#[derive(Debug, Clone)]
pub struct VgmSplitOptions {
    /// [`SplitFormat::Wav`] renders each channel to its own WAV;
    /// [`SplitFormat::Song`] rewrites the command stream into a per-channel VGM
    /// (only for chips a [`ChannelGate`] covers -- see
    /// [`split_vgm_cancellable`]).
    pub format: SplitFormat,
    pub audio: AudioConfig,
    pub resampling: ResampleMode,
    /// The panning applied to each rendered stem (`SplitFormat::Wav` only), so a
    /// stem is placed exactly as its channel sits in the full mix. Neutral by
    /// default -- the split owns the mute mask, not the pan. Ignored by the song
    /// format (a VGM stem carries raw commands; pan is render-time).
    pub panning: ChipPanning,
    /// The boost applied to each rendered stem (`SplitFormat::Wav` only), through
    /// the same limiter a whole-song render uses. `1.0` is bit-transparent.
    /// Ignored by the song format.
    pub boost: f32,
    /// When `Some`, channels muted in this mask are excluded from the output set
    /// (decision 9): the split owns the per-channel *solo* masks, so a "mute" for
    /// a split means "do not emit a stem for it". `None` splits every channel.
    /// Applies to both formats.
    pub skip_muted: Option<ChipMuting>,
    /// The per-render core choices, seeded from Settings and never persisted.
    /// Applied by the caller with
    /// [`with_render_choices`](crate::with_render_choices), so an empty map
    /// renders exactly as the configured cores would.
    pub core_choices: CoreChoices,
}

/// A peak at or below this fraction of full scale is treated as silence: a
/// channel that was configured but never sounded renders (near-)zero, and its
/// file is not worth writing. `1/1000` separates "never keyed" from "playing"
/// with room to spare.
const SILENCE_DIVISOR: i32 = 1000;

/// Splits a VGM into one file per channel, in the chosen format.
///
/// For [`SplitFormat::Wav`] it *renders* each channel soloed and keeps only the
/// ones that come out above silence. Being chip-agnostic, it has no per-channel
/// usage analysis, so it renders *every* channel of a written chip instance and
/// drops the silent renders afterward -- more render work than a chip-specific
/// pre-filter, but a channel split is an offline operation and the output set is
/// the same. For [`SplitFormat::Song`] it *rewrites* the command stream into a
/// per-channel VGM (see [`song_gate`](crate::song_gate)), which needs no render
/// but only works for a chip a [`ChannelGate`] covers.
///
/// A whole chip instance the stream never writes is skipped without work (the
/// pre-filter). A chip that cannot be isolated in the chosen format -- a WAV core
/// that can neither mute natively nor be gated, or a DroSong chip with no gate table
/// -- is skipped per instance with a warning, rather than writing N identical or
/// impossible files. `on_skip` names every channel dropped so the CLI can report
/// it.
///
/// `Ok(None)` iff `keep_going` asked it to stop part-way.
///
/// # Errors
/// If a channel cannot be rendered to WAV, or a song-format channel cannot be
/// filtered.
pub fn split_vgm_cancellable(
    file: &Arc<VgmFile>,
    options: &VgmSplitOptions,
    on_skip: &mut dyn FnMut(&str),
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<SplitOutput>>> {
    // Which chip instances the stream ever writes: a chip it never touches is
    // silent on every channel, so it is skipped without work.
    let written = written_instances(file);
    let full = if options.audio.bit_depth == 8 {
        127
    } else {
        32_767
    };
    let extension = match options.format {
        SplitFormat::Wav => "wav",
        SplitFormat::Song => "vgm",
    };
    let mut outputs = Vec::new();

    for chip in file.header.chips() {
        let instances = if chip.dual { 2 } else { 1 };
        let channels = channels_of(chip.kind, chip.variant);
        // Whether this chip's channels can be isolated in the chosen format.
        // WAV: a core that can neither mute natively nor be write-gated renders
        // the same full mix for every "solo", so N identical files -- the offline
        // choice is what the render below resolves. DroSong: a rewrite needs a gate
        // table; native mute is a render-time trick, no help here.
        let isolable = match options.format {
            SplitFormat::Wav => {
                let choice = registry::render_override(chip.kind)
                    .or_else(|| registry::core_choice(chip.kind));
                !renders_identical_files(registry::registry(), chip.kind, choice.as_deref())
            }
            SplitFormat::Song => ChannelGate::exists(chip.kind),
        };
        for instance in 0..instances {
            let ever_written = written.contains(&(chip.kind, instance));
            if ever_written && !isolable {
                match options.format {
                    SplitFormat::Wav => log::warn!(
                        "channel split: {}'s core cannot isolate channels (no native mute, \
                         no write-gate); skipping rather than writing identical files",
                        chip.kind.name()
                    ),
                    SplitFormat::Song => log::warn!(
                        "channel split: {} has no write-gate table, so it cannot be split to \
                         song data; skipping (its channels can still be split to WAV)",
                        chip.kind.name()
                    ),
                }
            }
            for (index, channel) in channels.iter().enumerate() {
                let name = channel_file_name(
                    &file.name,
                    chip.kind,
                    instance,
                    index,
                    channel.short,
                    extension,
                );
                if !ever_written || !isolable {
                    on_skip(&name);
                    continue;
                }
                // The user muted this channel: exclude it from the output set
                // (decision 9). The split owns the per-channel solo masks, so a
                // live mute means "do not emit a stem", not "silence within one".
                if options
                    .skip_muted
                    .as_ref()
                    .is_some_and(|m| m.mask_for(chip.kind, instance) & (1u32 << index) != 0)
                {
                    on_skip(&name);
                    continue;
                }
                if !keep_going() {
                    return Ok(None);
                }
                match options.format {
                    SplitFormat::Wav => {
                        let Some(bytes) = render_channel_wav(
                            file,
                            chip.kind,
                            instance,
                            index,
                            options,
                            &name,
                            on_progress,
                            keep_going,
                        )?
                        else {
                            return Ok(None);
                        };
                        // A channel that never sounded renders (near-)silence.
                        if wav_peak(&bytes) <= full / SILENCE_DIVISOR {
                            on_skip(&name);
                            continue;
                        }
                        outputs.push(SplitOutput {
                            name,
                            data: SplitData::Wav(bytes),
                        });
                    }
                    SplitFormat::Song => {
                        let vgm = crate::song_gate::solo_channel_to_vgm(
                            file,
                            chip.kind,
                            instance,
                            index,
                            name.clone(),
                        )?;
                        outputs.push(SplitOutput {
                            name,
                            data: SplitData::Vgm(Box::new(vgm)),
                        });
                    }
                }
            }
        }
    }
    Ok(Some(outputs))
}

/// Renders one soloed channel of a multichip VGM to WAV bytes, or `None` if the
/// render was cancelled part-way.
#[allow(clippy::too_many_arguments)]
fn render_channel_wav(
    file: &Arc<VgmFile>,
    kind: ChipKind,
    instance: u8,
    index: usize,
    options: &VgmSplitOptions,
    name: &str,
    on_progress: &mut dyn FnMut(&str, u64),
    keep_going: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<u8>>> {
    // Solo this channel: every other instance fully muted, and every other
    // channel of this instance too.
    let mut muting = ChipMuting::new();
    for other in file.header.chips() {
        let others = if other.dual { 2 } else { 1 };
        for other_instance in 0..others {
            muting.set(other.kind, other_instance, u32::MAX);
        }
    }
    muting.set(kind, instance, !(1u32 << index));

    // Pan and boost apply to the stem exactly as to a whole-song render.
    let mix = VgmRenderMix {
        muting,
        panning: options.panning.clone(),
        boost: options.boost,
    };
    render_vgm_wav_mixed_cancellable(
        Arc::clone(file),
        options.audio.frequency,
        options.audio.bit_depth,
        &mix,
        options.resampling,
        &mut |frames| on_progress(name, frames),
        keep_going,
    )
    .map_err(|e| Error::file(format!("Rendering {name} to WAV failed: {e}")))
}

/// Whether the channel split would write N identical files for `kind`.
///
/// True only when the chip's chosen offline core produces sound
/// ([`can_build`](CoreRegistry::can_build)) but can neither mute natively nor be
/// write-gated -- then every soloed render is the same full mix. A gated chip, a
/// native-mute core, and a chip with no core at all (which renders silence the
/// filter already drops) are all fine, so none of them is flagged.
fn renders_identical_files(registry: &CoreRegistry, kind: ChipKind, choice: Option<&str>) -> bool {
    if ChannelGate::exists(kind) || !registry.can_build(kind) {
        return false;
    }
    !registry
        .resolve_choice(kind, choice)
        .is_some_and(|info| info.channel_mute)
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

/// The file name for one channel: `<song>.<chip-slug>[#2].<NN>-<short>.<ext>`.
fn channel_file_name(
    song: &str,
    kind: ChipKind,
    instance: u8,
    index: usize,
    short: &str,
    extension: &str,
) -> String {
    let stem = song.strip_suffix(".vgm").unwrap_or(song);
    let inst = if instance == 0 {
        String::new()
    } else {
        format!("#{}", instance + 1)
    };
    format!(
        "{stem}.{}{inst}.{index:02}-{short}.{extension}",
        kind.slug()
    )
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
            format: SplitFormat::Wav,
            audio: AudioConfig::default(),
            resampling: ResampleMode::default(),
            panning: ChipPanning::new(),
            boost: 1.0,
            skip_muted: None,
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
            channel_file_name("bios.vgm", ChipKind::Sn76489, 0, 3, "N", "wav"),
            "bios.sn76489.03-N.wav"
        );
        assert_eq!(
            channel_file_name("x.vgm", ChipKind::Ym2612, 1, 6, "DA", "vgm"),
            "x.ym2612#2.06-DA.vgm"
        );
    }

    // -- song-format split (rs-2) --------------------------------------------

    fn put(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Wraps a body into a v1.61 VGM header declaring the given chip clocks, with
    /// the header's total-sample field stamped from the body's own delays.
    fn vgm_with(clocks: &[(usize, u32)], body: &[u8]) -> Arc<VgmFile> {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put(&mut bytes, 0x08, 0x161);
        put(&mut bytes, 0x34, 0x100 - 0x34);
        for &(offset, clock) in clocks {
            put(&mut bytes, offset, clock);
        }
        bytes.extend_from_slice(body);
        let eof = bytes.len();
        put(&mut bytes, 0x04, (eof - 4) as u32);
        // Stamp the header's total-sample field (0x18) to match the stream, so a
        // stem's repatched header can be compared against the source's.
        let total = vgms_core::vgm::file::read("song.vgm", &bytes)
            .unwrap()
            .stream()
            .unwrap()
            .total_samples() as u32;
        put(&mut bytes, 0x18, total);
        Arc::new(vgms_core::vgm::file::read("song.vgm", &bytes).unwrap())
    }

    /// A YM2612 (FM ch0 keyed) plus an SN76489 (tones 1 and 2 loud), one second.
    fn mega_ish_vgm() -> Arc<VgmFile> {
        vgm_with(
            &[
                (ChipKind::Sn76489.clock_offset(), 3_579_545),
                (ChipKind::Ym2612.clock_offset(), 7_670_454),
            ],
            &[
                0x52, 0x28, 0xF0, // YM2612 FM ch0 key-on (all slots)
                0x50, 0x90, // SN tone 1 volume 0 (loud)
                0x50, 0xB0, // SN tone 2 volume 0 (loud)
                0x61, 0x44, 0xAC, // wait 44100
                0x66,
            ],
        )
    }

    fn song_options() -> VgmSplitOptions {
        VgmSplitOptions {
            format: SplitFormat::Song,
            ..options()
        }
    }

    fn run_song_split(file: &Arc<VgmFile>) -> (Vec<SplitOutput>, Vec<String>) {
        let mut skipped = Vec::new();
        let outputs = split_vgm_cancellable(
            file,
            &song_options(),
            &mut |name| skipped.push(name.to_owned()),
            &mut |_, _| {},
            &mut || true,
        )
        .unwrap()
        .expect("not cancelled");
        (outputs, skipped)
    }

    fn as_vgm(output: &SplitOutput) -> &VgmFile {
        match &output.data {
            SplitData::Vgm(file) => file.as_ref(),
            other => panic!("expected a VGM output, got {other:?}"),
        }
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

    /// Every `Write` in a filtered VGM, as `(kind, instance, addr, data)`.
    fn writes(file: &VgmFile) -> Vec<(ChipKind, u8, u16, u16)> {
        let stream = file.stream().expect("a walkable stream");
        (0..stream.len())
            .filter_map(|index| match stream.get(index) {
                Some(VgmCommand::Write { target, addr, data }) => {
                    Some((target.kind, target.instance, addr, data))
                }
                _ => None,
            })
            .collect()
    }

    /// The song format writes one standalone VGM per gate-capable channel --
    /// every one of them, since there is no render to silence-filter.
    #[test]
    fn a_song_split_writes_one_valid_vgm_per_gate_capable_channel() {
        let file = mega_ish_vgm();
        let (outputs, _skipped) = run_song_split(&file);

        // YM2612 (7 channels) + SN76489 (4) = 11 gate-capable channels.
        assert_eq!(outputs.len(), 11, "one VGM per gate-capable channel");
        for output in &outputs {
            assert!(output.name.ends_with(".vgm"), "{}", output.name);
            let vgm = as_vgm(output);
            // The timeline is preserved to the sample, and it re-reads as a VGM.
            assert_eq!(vgm.total_samples(), file.total_samples());
            let bytes = vgms_core::vgm::file::write(vgm).unwrap();
            assert!(bytes.starts_with(b"Vgm "), "{}", output.name);
            let reread = vgms_core::vgm::file::read(&output.name, &bytes).unwrap();
            assert_eq!(reread.total_samples(), file.total_samples());
        }
    }

    /// Each stem keeps its own channel's writes and silences everything else:
    /// other channels of the same chip, and every other chip whole.
    #[test]
    fn a_song_stem_keeps_only_its_own_channel() {
        let file = mega_ish_vgm();
        let (outputs, _) = run_song_split(&file);

        // SN tone 1 (index 0): keeps its loud volume, forces tone 2 silent
        // (0xB0 -> 0xBF), and the YM2612 is gone.
        let sn0 = writes(as_vgm(find(&outputs, "sn76489.00-")));
        assert!(
            sn0.iter()
                .any(|&(k, _, _, d)| k == ChipKind::Sn76489 && d == 0x90),
            "tone 1 stays loud: {sn0:02X?}"
        );
        assert!(
            sn0.iter()
                .any(|&(k, _, _, d)| k == ChipKind::Sn76489 && d == 0xBF),
            "tone 2 is forced silent: {sn0:02X?}"
        );
        assert!(
            !sn0.iter().any(|&(k, ..)| k == ChipKind::Ym2612),
            "the YM2612 is silenced whole: {sn0:02X?}"
        );

        // YM2612 FM1 (index 0): keeps its key-on, and the SN76489 is gone.
        let ym0 = writes(as_vgm(find(&outputs, "ym2612.00-")));
        assert!(
            ym0.iter()
                .any(|&(k, _, a, d)| k == ChipKind::Ym2612 && a == 0x28 && d == 0xF0),
            "the FM key-on survives: {ym0:02X?}"
        );
        assert!(
            !ym0.iter().any(|&(k, ..)| k == ChipKind::Sn76489),
            "the SN76489 is silenced whole: {ym0:02X?}"
        );
    }

    /// A chip with no gate table cannot be split to song data: its channels are
    /// skipped (per-chip), while a gated chip in the same file still splits.
    #[test]
    fn a_song_split_refuses_a_chip_with_no_gate_table() {
        // SN76489 (gated) + YM2413 (no gate table), both written.
        let file = vgm_with(
            &[
                (ChipKind::Sn76489.clock_offset(), 3_579_545),
                (ChipKind::Ym2413.clock_offset(), 3_579_545),
            ],
            &[
                0x51, 0x10, 0x01, // YM2413 write
                0x50, 0x90, // SN tone 1 loud
                0x61, 0x44, 0xAC, 0x66,
            ],
        );
        let (outputs, skipped) = run_song_split(&file);

        assert_eq!(outputs.len(), 4, "only the four SN76489 channels split");
        assert!(
            outputs.iter().all(|o| o.name.contains("sn76489")),
            "{:?}",
            outputs.iter().map(|o| &o.name).collect::<Vec<_>>()
        );
        assert!(
            skipped.iter().any(|n| n.contains("ym2413")),
            "the YM2413 channels are skipped: {skipped:?}"
        );
    }

    /// A YM2612 with a `0x8n` DAC write and a PCM data block. Soloing an FM
    /// channel mutes the DAC, so the `0x8n` becomes an equivalent wait; the data
    /// block is kept. Soloing the DAC keeps the `0x8n`.
    fn ym2612_dac_vgm() -> Arc<VgmFile> {
        vgm_with(
            &[(ChipKind::Ym2612.clock_offset(), 7_670_454)],
            &[
                0x52, 0x2B, 0x80, // DAC enable
                0x52, 0x28, 0xF0, // FM ch0 key-on
                0x67, 0x66, 0x00, 0x02, 0x00, 0x00, 0x00, 0xAA, 0xBB, // PCM data block
                0x85, // DAC write + wait 5
                0x61, 0x44, 0xAC, 0x66,
            ],
        )
    }

    #[test]
    fn a_muted_dac_write_becomes_a_wait_and_keeps_the_timing() {
        let file = ym2612_dac_vgm();
        let (outputs, _) = run_song_split(&file);
        let fm0 = as_vgm(find(&outputs, "ym2612.00-"));

        // The DAC is muted for an FM stem: the 0x8n sample write is gone, but its
        // five-sample wait is not, so the total length is untouched.
        let stream = fm0.stream().unwrap();
        assert!(
            (0..stream.len()).all(|i| !matches!(stream.get(i), Some(VgmCommand::DacWrite { .. }))),
            "no DAC sample write survives an FM stem"
        );
        assert!(
            (0..stream.len()).any(|i| matches!(stream.get(i), Some(VgmCommand::DataBlock { .. }))),
            "the PCM data block is kept"
        );
        assert_eq!(
            fm0.total_samples(),
            file.total_samples(),
            "timing preserved"
        );
    }

    #[test]
    fn soloing_the_dac_keeps_its_sample_writes() {
        let file = ym2612_dac_vgm();
        let (outputs, _) = run_song_split(&file);
        // The DAC is channel 6 of the YM2612 (short label "DA").
        let dac = as_vgm(find(&outputs, "ym2612.06-"));
        let stream = dac.stream().unwrap();
        assert!(
            (0..stream.len()).any(|i| matches!(stream.get(i), Some(VgmCommand::DacWrite { .. }))),
            "the DAC stem keeps the 0x8n sample writes"
        );
        // ...and the FM key-on is gone (that channel is muted here).
        assert!(
            !writes(dac)
                .iter()
                .any(|&(k, _, a, _)| k == ChipKind::Ym2612 && a == 0x28),
            "the muted FM key-on is dropped"
        );
    }

    /// A DAC stream (0x90-0x93) bound to the YM2612 DAC is dropped for a stem
    /// that mutes the DAC, and kept for the DAC's own stem -- because a stream's
    /// samples are synthesised at render time, not written into the stream.
    fn ym2612_stream_vgm() -> Arc<VgmFile> {
        vgm_with(
            &[(ChipKind::Ym2612.clock_offset(), 7_670_454)],
            &[
                0x52, 0x28, 0xF0, // FM ch0 key-on
                0x67, 0x66, 0x00, 0x02, 0x00, 0x00, 0x00, 0xAA, 0xBB, // PCM data block
                0x90, 0x00, 0x02, 0x00, 0x2A, // setup stream 0 -> YM2612 reg 0x2A
                0x91, 0x00, 0x00, 0x01, 0x00, // bind
                0x92, 0x00, 0x11, 0x2B, 0x00, 0x00, // rate (0x92 ss dddddddd, 6 bytes)
                0x93, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // start
                0x61, 0x44, 0xAC, 0x66,
            ],
        )
    }

    fn has_stream_start(file: &VgmFile) -> bool {
        let stream = file.stream().unwrap();
        (0..stream.len()).any(|i| {
            matches!(
                stream.get(i),
                Some(VgmCommand::DacStream { opcode: 0x93, .. })
            )
        })
    }

    #[test]
    fn a_stream_bound_to_a_muted_channel_is_not_started() {
        let file = ym2612_stream_vgm();
        let (outputs, _) = run_song_split(&file);
        // An FM stem mutes the DAC the stream feeds: its start is dropped.
        assert!(
            !has_stream_start(as_vgm(find(&outputs, "ym2612.00-"))),
            "the DAC stream's start is dropped for an FM stem"
        );
        // The DAC's own stem keeps it.
        assert!(
            has_stream_start(as_vgm(find(&outputs, "ym2612.06-"))),
            "the DAC stem keeps its stream start"
        );
    }

    /// A channel the user has live-muted is excluded from the output set
    /// (decision 9): the split does not emit a stem for it.
    #[test]
    fn a_song_split_skips_muted_channels() {
        let file = mega_ish_vgm();
        let mut skip = ChipMuting::new();
        skip.set(ChipKind::Sn76489, 0, 1 << 0); // mute SN tone 1
        let options = VgmSplitOptions {
            skip_muted: Some(skip),
            ..song_options()
        };
        let mut skipped = Vec::new();
        let outputs = split_vgm_cancellable(
            &file,
            &options,
            &mut |name| skipped.push(name.to_owned()),
            &mut |_, _| {},
            &mut || true,
        )
        .unwrap()
        .expect("not cancelled");

        // SN tone 1 is excluded; the other three SN channels and seven YM2612
        // channels remain.
        assert_eq!(outputs.len(), 10, "one fewer than the unmuted 11");
        assert!(
            !outputs.iter().any(|o| o.name.contains("sn76489.00-")),
            "the muted SN tone 1 has no stem"
        );
        assert!(
            skipped.iter().any(|n| n.contains("sn76489.00-")),
            "and it is reported as skipped: {skipped:?}"
        );
    }

    #[test]
    fn a_song_split_carries_the_loop() {
        let mut file = (*mega_ish_vgm()).clone();
        let loop_row = file.len() - 1; // the trailing wait
        file.set_loop_rows(Some(loop_row), None);
        let expected = file.loop_samples();
        assert!(expected.is_some(), "the fixture loops");
        let file = Arc::new(file);

        let (outputs, _) = run_song_split(&file);
        for output in &outputs {
            let vgm = as_vgm(output);
            assert!(vgm.loop_index().is_some(), "{} lost its loop", output.name);
            assert_eq!(
                vgm.loop_samples(),
                expected,
                "{} looped a different span",
                output.name
            );
        }
    }

    /// The guard against writing N identical files: a chip that can be soloed
    /// (gated, or natively muteable) is fine; a buildable chip that can be
    /// neither is flagged; a chip with no core (which renders silence) is not.
    #[test]
    fn the_identical_file_guard_flags_only_the_unmuteable() {
        use crate::registry::CoreTier;
        use crate::{ChipCore, CoreInfo, CoreMaker, CoreRegistry, LEVEL_UNITY};

        #[derive(Debug)]
        struct Dummy;
        impl ChipCore for Dummy {
            fn reset(&mut self, _clock: u32, _variant: bool) {}
            fn native_rate(&self) -> u32 {
                44_100
            }
            fn write(&mut self, _port: u8, _addr: u16, _data: u16) {}
            fn render(&mut self, out: &mut [i32]) {
                out.fill(0);
            }
        }
        fn core(id: &'static str, chip: ChipKind, native_mute: bool) -> CoreInfo {
            CoreInfo {
                id,
                chip,
                label: "dummy",
                authors: "test",
                license: "MIT",
                upstream: "",
                tier: CoreTier::Behavioural,
                exact: true,
                realtime: true,
                channel_pan: false,
                channel_mute: native_mute,
                level: LEVEL_UNITY,
                make: CoreMaker::Generic(|| Box::new(Dummy)),
            }
        }

        let mut reg = CoreRegistry::new();
        reg.register(core("sn76489.nuked", ChipKind::Sn76489, false)); // gated
        reg.register(core("c352.lle", ChipKind::C352, false)); // no gate, no native mute
        reg.register(core("c140.libvgm", ChipKind::C140, true)); // native mute

        assert!(
            !renders_identical_files(&reg, ChipKind::Sn76489, None),
            "the gate covers the SN76489, so it can be soloed"
        );
        assert!(
            renders_identical_files(&reg, ChipKind::C352, None),
            "no gate table and no native mute: every solo is the same full mix"
        );
        assert!(
            !renders_identical_files(&reg, ChipKind::C140, None),
            "a native-mute core solos fine"
        );
        assert!(
            !renders_identical_files(&reg, ChipKind::Ymz280b, None),
            "no core at all renders silence, which the filter drops -- not identical files"
        );
    }
}
