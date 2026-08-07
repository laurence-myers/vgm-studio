// SPDX-License-Identifier: GPL-2.0-or-later
//! Byte codecs for [`TaskRequest`] and [`TaskResult`], so a background task can
//! cross the Web Worker boundary as a transferable `ArrayBuffer`.
//!
//! A `Worker` shares no memory with the page, so a task's inputs have to be
//! serialised. There is no serde here on purpose: the request carries whole
//! documents (`DroSong`, `VgmFile`), which already have exact readers and writers of
//! their own, so they ride as their file bytes and read back by name -- the
//! round-trip `vgms-core` already tests to the byte. Everything else is a handful
//! of scalars, length-prefixed. `AudioConfig` rides its INI text, the same string
//! the config store persists.
//!
//! This module is portable and native-tested: the round-trip proofs run in the
//! ordinary `cargo test`, off the browser, because a codec bug would otherwise
//! only surface as a mangled render in a Worker no test can see.

use std::collections::BTreeMap;

use vgms_core::config::AudioConfig;
use vgms_core::config::OptimizerChoice;
use vgms_core::loopfind::Candidate;
use vgms_core::vgm::ChipKind;
use vgms_core::{DroSong, VgmFile};
use vgms_synth::resample::ResampleMode;
use vgms_synth::{
    AudioSource, ChipMuting, ChipPanning, CoreChoices, Muting, Panning, Peak, RenderMix,
    SplitFormat, VgmRenderMix, VgmSplitOptions, WaveformBucket,
};
use vgms_ui::tasks::{
    LoopSearchSource, RenderWavMix, SplitFiles, SplitSource, SplitTaskSource, TaskResult, WavSource,
};
use vgms_ui::{PackEntry, PackEntryKind, PackJobOutcome, PackJobRequest, TaskRequest};

/// Why a decode failed. Encodes never fail on scalars; a document encode can, if
/// its own writer does.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("codec: buffer underran reading {0}")]
    Eof(&'static str),
    #[error("codec: unknown {0} tag {1}")]
    Tag(&'static str, u8),
    #[error("codec: non-UTF-8 string")]
    Utf8,
    #[error("codec: {0}")]
    Document(String),
}

type Result<T> = std::result::Result<T, CodecError>;

// -- the byte writer ------------------------------------------------------------

/// A little-endian byte sink. Every multi-byte value is LE; every variable-length
/// value (bytes, strings) is prefixed with a `u32` length.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, value: u8) {
        self.out.push(value);
    }
    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }
    fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
    fn i16(&mut self, value: i16) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
    fn usize(&mut self, value: usize) {
        self.u64(value as u64);
    }
    fn f32(&mut self, value: f32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.out.extend_from_slice(value);
    }
    fn str(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
}

// -- the byte reader ------------------------------------------------------------

/// A cursor over the encoded bytes; every read is bounds-checked and surfaces an
/// [`CodecError::Eof`] rather than panicking on a short buffer.
struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(CodecError::Eof(what))?;
        let slice = self.input.get(self.pos..end).ok_or(CodecError::Eof(what))?;
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }
    fn bool(&mut self, what: &'static str) -> Result<bool> {
        Ok(self.u8(what)? != 0)
    }
    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2, what)?.try_into().expect("2 bytes"),
        ))
    }
    fn i16(&mut self, what: &'static str) -> Result<i16> {
        Ok(i16::from_le_bytes(
            self.take(2, what)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4, what)?.try_into().expect("4 bytes"),
        ))
    }
    fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8, what)?.try_into().expect("8 bytes"),
        ))
    }
    fn usize(&mut self, what: &'static str) -> Result<usize> {
        // The encoder wrote a u64; on wasm32 usize is 32-bit, so a value from a
        // 64-bit host could in principle overflow -- but these are lengths and
        // indices of in-memory documents, far below u32::MAX. Saturate rather
        // than fail: a truncated index is still bounded and the task will simply
        // find nothing there.
        Ok(usize::try_from(self.u64(what)?).unwrap_or(usize::MAX))
    }
    fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_le_bytes(
            self.take(4, what)?.try_into().expect("4 bytes"),
        ))
    }
    fn bytes(&mut self, what: &'static str) -> Result<&'a [u8]> {
        let len = self.u32(what)? as usize;
        self.take(len, what)
    }
    fn str(&mut self, what: &'static str) -> Result<String> {
        let bytes = self.bytes(what)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| CodecError::Utf8)
    }
}

// -- documents ------------------------------------------------------------------

/// Encodes a [`DroSong`] as `(name, file-bytes)`: its own writer produces the bytes,
/// and the name's extension picks the reader back. This is the exact byte the app
/// would save, so the round-trip is the one `vgms-core` already pins.
fn write_song(writer: &mut Writer, song: &DroSong) -> Result<()> {
    let bytes =
        vgms_core::io::write_song(song).map_err(|error| CodecError::Document(error.to_string()))?;
    writer.str(&song.name);
    writer.bytes(&bytes);
    Ok(())
}

fn read_song(reader: &mut Reader) -> Result<DroSong> {
    let name = reader.str("song.name")?;
    let bytes = reader.bytes("song.bytes")?;
    vgms_core::io::read_song(&name, bytes).map_err(|error| CodecError::Document(error.to_string()))
}

fn write_vgm(writer: &mut Writer, file: &VgmFile) -> Result<()> {
    let bytes = vgms_core::vgm::file::write(file)
        .map_err(|error| CodecError::Document(error.to_string()))?;
    writer.str(&file.name);
    writer.bytes(&bytes);
    Ok(())
}

fn read_vgm(reader: &mut Reader) -> Result<VgmFile> {
    let name = reader.str("vgm.name")?;
    let bytes = reader.bytes("vgm.bytes")?;
    vgms_core::vgm::file::read(&name, bytes)
        .map_err(|error| CodecError::Document(error.to_string()))
}

// -- small value types ----------------------------------------------------------

/// Serialises an [`AudioConfig`] field for field. Every field is public, so this
/// is a faithful copy -- no INI round-trip needed, and no field silently dropped
/// (a new field is a compile error here until it is handled).
fn write_config(writer: &mut Writer, config: &AudioConfig) {
    writer.u16(config.bit_depth);
    writer.f32(config.boost);
    writer.bool(config.lock_boost);
    writer.u32(config.buffer_size);
    writer.u32(config.frequency);
    writer.u32(config.cores.len() as u32);
    for (slot, name) in &config.cores {
        writer.str(slot);
        writer.str(name);
    }
    writer.str(&config.resampling);
    match &config.retrowave_port {
        Some(port) => {
            writer.u8(1);
            writer.str(port);
        }
        None => writer.u8(0),
    }
}

fn read_config(reader: &mut Reader) -> Result<AudioConfig> {
    let bit_depth = reader.u16("cfg.bit_depth")?;
    let boost = reader.f32("cfg.boost")?;
    let lock_boost = reader.bool("cfg.lock_boost")?;
    let buffer_size = reader.u32("cfg.buffer_size")?;
    let frequency = reader.u32("cfg.frequency")?;
    let core_count = reader.u32("cfg.cores.len")? as usize;
    let mut cores = BTreeMap::new();
    for _ in 0..core_count {
        let slot = reader.str("cfg.core.slot")?;
        let name = reader.str("cfg.core.name")?;
        cores.insert(slot, name);
    }
    let resampling = reader.str("cfg.resampling")?;
    let retrowave_port = match reader.u8("cfg.port.tag")? {
        0 => None,
        1 => Some(reader.str("cfg.port")?),
        other => return Err(CodecError::Tag("cfg.port.tag", other)),
    };
    Ok(AudioConfig {
        bit_depth,
        boost,
        lock_boost,
        buffer_size,
        frequency,
        cores,
        resampling,
        retrowave_port,
    })
}

/// Serialises a per-render [`CoreChoices`] map (slot slug -> core short-name) as
/// `u32 len` then `str slot, str name` per entry -- the exact shape
/// [`write_config`] gives `AudioConfig::cores`, so an unknown slot reads back as
/// an ordinary entry rather than failing.
fn write_core_choices(writer: &mut Writer, choices: &CoreChoices) {
    writer.u32(choices.len() as u32);
    for (slot, name) in choices {
        writer.str(slot);
        writer.str(name);
    }
}

fn read_core_choices(reader: &mut Reader) -> Result<CoreChoices> {
    let count = reader.u32("core-choices.len")? as usize;
    let mut choices = CoreChoices::new();
    for _ in 0..count {
        let slot = reader.str("core-choices.slot")?;
        let name = reader.str("core-choices.name")?;
        choices.insert(slot, name);
    }
    Ok(choices)
}

fn write_resample(writer: &mut Writer, mode: ResampleMode) {
    writer.u8(match mode {
        ResampleMode::Sinc => 0,
        ResampleMode::Linear => 1,
    });
}

fn read_resample(reader: &mut Reader) -> Result<ResampleMode> {
    match reader.u8("resample")? {
        0 => Ok(ResampleMode::Sinc),
        1 => Ok(ResampleMode::Linear),
        other => Err(CodecError::Tag("resample", other)),
    }
}

fn write_muting(writer: &mut Writer, muting: Muting) {
    writer.u32(muting.channels_raw());
    let [low, high] = muting.percussion_raw();
    writer.u8(low);
    writer.u8(high);
}

fn read_muting(reader: &mut Reader) -> Result<Muting> {
    let channels = reader.u32("muting.channels")?;
    let low = reader.u8("muting.perc0")?;
    let high = reader.u8("muting.perc1")?;
    Ok(Muting::from_raw(channels, [low, high]))
}

fn write_panning(writer: &mut Writer, panning: Panning) {
    match panning {
        Panning::Original => writer.u8(0),
        Panning::Custom(pans) => {
            writer.u8(1);
            for pan in pans {
                writer.u8(pan);
            }
        }
    }
}

fn read_panning(reader: &mut Reader) -> Result<Panning> {
    match reader.u8("panning")? {
        0 => Ok(Panning::Original),
        1 => {
            let mut pans = [0u8; 18];
            for pan in &mut pans {
                *pan = reader.u8("panning.pan")?;
            }
            Ok(Panning::Custom(pans))
        }
        other => Err(CodecError::Tag("panning", other)),
    }
}

fn write_mix(writer: &mut Writer, mix: RenderMix) {
    write_muting(writer, mix.muting);
    write_panning(writer, mix.panning);
    writer.f32(mix.boost);
}

fn read_mix(reader: &mut Reader) -> Result<RenderMix> {
    Ok(RenderMix {
        muting: read_muting(reader)?,
        panning: read_panning(reader)?,
        boost: reader.f32("mix.boost")?,
    })
}

/// A chip is identified on the wire by its slug -- the same stable string the
/// config's core map and the worklet ABI use -- so an entry for a chip a future
/// build drops decodes as "skip", not a hard error.
fn write_chip_muting(writer: &mut Writer, muting: &ChipMuting) {
    let entries: Vec<_> = muting.entries().collect();
    writer.u32(entries.len() as u32);
    for (kind, instance, mask) in entries {
        writer.str(kind.slug());
        writer.u8(instance);
        writer.u32(mask);
    }
}

fn read_chip_muting(reader: &mut Reader) -> Result<ChipMuting> {
    let count = reader.u32("chip-muting.len")?;
    let mut muting = ChipMuting::new();
    for _ in 0..count {
        let slug = reader.str("chip-muting.slug")?;
        let instance = reader.u8("chip-muting.instance")?;
        let mask = reader.u32("chip-muting.mask")?;
        if let Some(kind) = ChipKind::from_slug(&slug) {
            muting.set(kind, instance, mask);
        }
    }
    Ok(muting)
}

fn write_chip_panning(writer: &mut Writer, panning: &ChipPanning) {
    let entries: Vec<_> = panning.entries().collect();
    writer.u32(entries.len() as u32);
    for (kind, instance, pans) in entries {
        writer.str(kind.slug());
        writer.u8(instance);
        writer.u32(pans.len() as u32);
        for pan in pans {
            writer.i16(*pan);
        }
    }
}

fn read_chip_panning(reader: &mut Reader) -> Result<ChipPanning> {
    let count = reader.u32("chip-panning.len")?;
    let mut panning = ChipPanning::new();
    for _ in 0..count {
        let slug = reader.str("chip-panning.slug")?;
        let instance = reader.u8("chip-panning.instance")?;
        let pan_count = reader.u32("chip-panning.pans.len")? as usize;
        let mut pans = Vec::with_capacity(pan_count);
        for _ in 0..pan_count {
            pans.push(reader.i16("chip-panning.pan")?);
        }
        // An unknown slug's entry is read past (to keep the stream aligned) but
        // dropped, exactly as `read_chip_muting` does.
        if let Some(kind) = ChipKind::from_slug(&slug) {
            panning.set(kind, instance, pans);
        }
    }
    Ok(panning)
}

fn write_vgm_mix(writer: &mut Writer, mix: &VgmRenderMix) {
    write_chip_muting(writer, &mix.muting);
    write_chip_panning(writer, &mix.panning);
    writer.f32(mix.boost);
}

fn read_vgm_mix(reader: &mut Reader) -> Result<VgmRenderMix> {
    Ok(VgmRenderMix {
        muting: read_chip_muting(reader)?,
        panning: read_chip_panning(reader)?,
        boost: reader.f32("vgm-mix.boost")?,
    })
}

fn write_vgm_split_options(writer: &mut Writer, options: &VgmSplitOptions) {
    writer.u8(match options.format {
        SplitFormat::Wav => 0,
        SplitFormat::Song => 1,
    });
    write_config(writer, &options.audio);
    write_resample(writer, options.resampling);
    write_chip_panning(writer, &options.panning);
    writer.f32(options.boost);
    match &options.skip_muted {
        Some(muting) => {
            writer.bool(true);
            write_chip_muting(writer, muting);
        }
        None => writer.bool(false),
    }
    write_core_choices(writer, &options.core_choices);
}

fn read_vgm_split_options(reader: &mut Reader) -> Result<VgmSplitOptions> {
    let format = match reader.u8("vgm-split.format")? {
        0 => SplitFormat::Wav,
        1 => SplitFormat::Song,
        other => return Err(CodecError::Tag("vgm-split.format", other)),
    };
    Ok(VgmSplitOptions {
        format,
        audio: read_config(reader)?,
        resampling: read_resample(reader)?,
        panning: read_chip_panning(reader)?,
        boost: reader.f32("vgm-split.boost")?,
        skip_muted: if reader.bool("vgm-split.skip_muted")? {
            Some(read_chip_muting(reader)?)
        } else {
            None
        },
        core_choices: read_core_choices(reader)?,
    })
}

// -- source enums (each an OPL/VGM pair) ----------------------------------------

fn write_audio_source(writer: &mut Writer, source: &AudioSource) -> Result<()> {
    match source {
        AudioSource::Dro(song) => {
            writer.u8(0);
            write_song(writer, song)
        }
        AudioSource::Vgm(file) => {
            writer.u8(1);
            write_vgm(writer, file)
        }
    }
}

fn read_audio_source(reader: &mut Reader) -> Result<AudioSource> {
    match reader.u8("audio-source")? {
        0 => Ok(AudioSource::Dro(std::sync::Arc::new(read_song(reader)?))),
        1 => Ok(AudioSource::Vgm(std::sync::Arc::new(read_vgm(reader)?))),
        other => Err(CodecError::Tag("audio-source", other)),
    }
}

// -- requests -------------------------------------------------------------------

/// Encodes a [`TaskRequest`] to bytes. Fails only if a document's own writer does.
pub fn encode_request(request: &TaskRequest) -> Result<Vec<u8>> {
    let mut writer = Writer::default();
    match request {
        TaskRequest::RenderWaveform {
            source,
            num_buckets,
            sample_rate,
            resampling,
        } => {
            writer.u8(0);
            write_audio_source(&mut writer, source)?;
            writer.usize(*num_buckets);
            writer.u32(*sample_rate);
            write_resample(&mut writer, *resampling);
        }
        TaskRequest::RenderWav {
            source,
            mix,
            sample_rate,
            bit_depth,
            resampling,
            core_choices,
        } => {
            writer.u8(1);
            // Source and mix share one arm tag: the OPL arm carries a `RenderMix`,
            // the generic arm a `VgmRenderMix`, so the pair cannot desync on the
            // wire (they are always built together app-side).
            match (source, mix) {
                (WavSource::Dro(song), RenderWavMix::Opl(mix)) => {
                    writer.u8(0);
                    write_song(&mut writer, song)?;
                    write_mix(&mut writer, *mix);
                }
                (WavSource::Vgm(file), RenderWavMix::Vgm(mix)) => {
                    writer.u8(1);
                    write_vgm(&mut writer, file)?;
                    write_vgm_mix(&mut writer, mix);
                }
                _ => {
                    return Err(CodecError::Document(
                        "render mix did not match its source".to_owned(),
                    ));
                }
            }
            writer.u32(*sample_rate);
            writer.u16(*bit_depth);
            write_resample(&mut writer, *resampling);
            write_core_choices(&mut writer, core_choices);
        }
        TaskRequest::Split { source } => {
            writer.u8(2);
            // Every split runs over a VGM now (ou-4): an OPL document is resolved
            // to its file before it reaches the task.
            let SplitTaskSource::Vgm { file, options } = source;
            write_vgm(&mut writer, file)?;
            write_vgm_split_options(&mut writer, options);
        }
        TaskRequest::SplitSongs {
            source,
            threshold_native,
            included,
            trailing_tail,
        } => {
            writer.u8(3);
            write_split_source(&mut writer, source)?;
            writer.u32(*threshold_native);
            writer.u32(included.len() as u32);
            for flag in included {
                writer.bool(*flag);
            }
            writer.u32(*trailing_tail);
        }
        TaskRequest::VolumeScan {
            source,
            sample_rate,
            resampling,
        } => {
            writer.u8(4);
            write_audio_source(&mut writer, source)?;
            writer.u32(*sample_rate);
            write_resample(&mut writer, *resampling);
        }
        TaskRequest::PackVolumeScan {
            tracks,
            sample_rate,
            resampling,
        } => {
            writer.u8(5);
            writer.u32(tracks.len() as u32);
            for (name, source) in tracks {
                writer.str(name);
                write_audio_source(&mut writer, source)?;
            }
            writer.u32(*sample_rate);
            write_resample(&mut writer, *resampling);
        }
        TaskRequest::LoopSearch {
            source,
            min_len_commands,
        } => {
            writer.u8(6);
            match source {
                LoopSearchSource::Dro(song) => {
                    writer.u8(0);
                    write_song(&mut writer, song)?;
                }
                LoopSearchSource::Vgm(file) => {
                    writer.u8(1);
                    write_vgm(&mut writer, file)?;
                }
            }
            writer.usize(*min_len_commands);
        }
    }
    Ok(writer.out)
}

fn write_split_source(writer: &mut Writer, source: &SplitSource) -> Result<()> {
    match source {
        SplitSource::Dro(song) => {
            writer.u8(0);
            write_song(writer, song)
        }
        SplitSource::Vgm(file) => {
            writer.u8(1);
            write_vgm(writer, file)
        }
    }
}

fn read_split_source(reader: &mut Reader) -> Result<SplitSource> {
    match reader.u8("split-source")? {
        0 => Ok(SplitSource::Dro(std::sync::Arc::new(read_song(reader)?))),
        1 => Ok(SplitSource::Vgm(std::sync::Arc::new(read_vgm(reader)?))),
        other => Err(CodecError::Tag("split-source", other)),
    }
}

/// Decodes a [`TaskRequest`] from [`encode_request`]'s bytes.
pub fn decode_request(input: &[u8]) -> Result<TaskRequest> {
    let mut reader = Reader::new(input);
    let request = match reader.u8("request.tag")? {
        0 => TaskRequest::RenderWaveform {
            source: read_audio_source(&mut reader)?,
            num_buckets: reader.usize("num_buckets")?,
            sample_rate: reader.u32("sample_rate")?,
            resampling: read_resample(&mut reader)?,
        },
        1 => {
            let (source, mix) = match reader.u8("wav-source")? {
                0 => {
                    let song = std::sync::Arc::new(read_song(&mut reader)?);
                    let mix = RenderWavMix::Opl(read_mix(&mut reader)?);
                    (WavSource::Dro(song), mix)
                }
                1 => {
                    let file = std::sync::Arc::new(read_vgm(&mut reader)?);
                    let mix = RenderWavMix::Vgm(read_vgm_mix(&mut reader)?);
                    (WavSource::Vgm(file), mix)
                }
                other => return Err(CodecError::Tag("wav-source", other)),
            };
            TaskRequest::RenderWav {
                source,
                mix,
                sample_rate: reader.u32("sample_rate")?,
                bit_depth: reader.u16("bit_depth")?,
                resampling: read_resample(&mut reader)?,
                core_choices: read_core_choices(&mut reader)?,
            }
        }
        2 => {
            let source = SplitTaskSource::Vgm {
                file: std::sync::Arc::new(read_vgm(&mut reader)?),
                options: read_vgm_split_options(&mut reader)?,
            };
            TaskRequest::Split { source }
        }
        3 => {
            let source = read_split_source(&mut reader)?;
            let threshold_native = reader.u32("threshold_native")?;
            let count = reader.u32("included.len")? as usize;
            let mut included = Vec::with_capacity(count);
            for _ in 0..count {
                included.push(reader.bool("included")?);
            }
            TaskRequest::SplitSongs {
                source,
                threshold_native,
                included,
                trailing_tail: reader.u32("trailing_tail")?,
            }
        }
        4 => TaskRequest::VolumeScan {
            source: read_audio_source(&mut reader)?,
            sample_rate: reader.u32("sample_rate")?,
            resampling: read_resample(&mut reader)?,
        },
        5 => {
            let count = reader.u32("tracks.len")? as usize;
            let mut tracks = Vec::with_capacity(count);
            for _ in 0..count {
                let name = reader.str("track.name")?;
                let source = read_audio_source(&mut reader)?;
                tracks.push((name, source));
            }
            TaskRequest::PackVolumeScan {
                tracks,
                sample_rate: reader.u32("sample_rate")?,
                resampling: read_resample(&mut reader)?,
            }
        }
        6 => {
            let source = match reader.u8("loop-source")? {
                0 => LoopSearchSource::Dro(std::sync::Arc::new(read_song(&mut reader)?)),
                1 => LoopSearchSource::Vgm(std::sync::Arc::new(read_vgm(&mut reader)?)),
                other => return Err(CodecError::Tag("loop-source", other)),
            };
            TaskRequest::LoopSearch {
                source,
                min_len_commands: reader.usize("min_len_commands")?,
            }
        }
        other => return Err(CodecError::Tag("request.tag", other)),
    };
    Ok(request)
}

// -- results --------------------------------------------------------------------

fn write_named_files(writer: &mut Writer, files: &[(String, Vec<u8>)]) {
    writer.u32(files.len() as u32);
    for (name, bytes) in files {
        writer.str(name);
        writer.bytes(bytes);
    }
}

fn read_named_files(reader: &mut Reader) -> Result<Vec<(String, Vec<u8>)>> {
    let count = reader.u32("files.len")? as usize;
    let mut files = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.str("file.name")?;
        let bytes = reader.bytes("file.bytes")?.to_vec();
        files.push((name, bytes));
    }
    Ok(files)
}

fn write_peak(writer: &mut Writer, peak: Peak) {
    writer.i16(peak.max_level);
    writer.bool(peak.clipped);
}

fn read_peak(reader: &mut Reader) -> Result<Peak> {
    Ok(Peak {
        max_level: reader.i16("peak.level")?,
        clipped: reader.bool("peak.clipped")?,
    })
}

/// Encodes a [`TaskResult`] to bytes for the Worker to post back.
pub fn encode_result(result: &TaskResult) -> Vec<u8> {
    let mut writer = Writer::default();
    match result {
        TaskResult::Waveform(buckets) => {
            writer.u8(0);
            writer.u32(buckets.len() as u32);
            for bucket in buckets {
                writer.i16(bucket.min);
                writer.i16(bucket.max);
            }
        }
        TaskResult::Wav(outcome) => {
            writer.u8(1);
            match outcome {
                Ok((name, bytes)) => {
                    writer.u8(0);
                    writer.str(name);
                    writer.bytes(bytes);
                }
                Err(message) => {
                    writer.u8(1);
                    writer.str(message);
                }
            }
        }
        TaskResult::Split(outcome) => {
            writer.u8(2);
            write_split_result(&mut writer, outcome);
        }
        TaskResult::SplitSongs(outcome) => {
            writer.u8(3);
            write_split_result(&mut writer, outcome);
        }
        TaskResult::Peak(peak) => {
            writer.u8(4);
            write_peak(&mut writer, *peak);
        }
        TaskResult::PackPeaks(peaks) => {
            writer.u8(5);
            writer.u32(peaks.len() as u32);
            for (name, peak) in peaks {
                writer.str(name);
                write_peak(&mut writer, *peak);
            }
        }
        TaskResult::LoopCandidates(candidates) => {
            writer.u8(6);
            writer.u32(candidates.len() as u32);
            for candidate in candidates {
                writer.usize(candidate.loop_point);
                writer.usize(candidate.loop_end);
                writer.usize(candidate.match_len);
                writer.bool(candidate.ends_at_eof);
                writer.bool(candidate.clean_repeat);
            }
        }
    }
    writer.out
}

fn write_split_result(writer: &mut Writer, outcome: &SplitFiles) {
    match outcome {
        Ok(files) => {
            writer.u8(0);
            write_named_files(writer, files);
        }
        Err(message) => {
            writer.u8(1);
            writer.str(message);
        }
    }
}

fn read_split_result(reader: &mut Reader) -> Result<SplitFiles> {
    match reader.u8("result.ok")? {
        0 => Ok(Ok(read_named_files(reader)?)),
        1 => Ok(Err(reader.str("result.err")?)),
        other => Err(CodecError::Tag("result.ok", other)),
    }
}

/// Decodes a [`TaskResult`] from [`encode_result`]'s bytes.
pub fn decode_result(input: &[u8]) -> Result<TaskResult> {
    let mut reader = Reader::new(input);
    let result = match reader.u8("result.tag")? {
        0 => {
            let count = reader.u32("waveform.len")? as usize;
            let mut buckets = Vec::with_capacity(count);
            for _ in 0..count {
                buckets.push(WaveformBucket {
                    min: reader.i16("bucket.min")?,
                    max: reader.i16("bucket.max")?,
                });
            }
            TaskResult::Waveform(buckets)
        }
        1 => {
            let outcome = match reader.u8("wav.ok")? {
                0 => {
                    let name = reader.str("wav.name")?;
                    let bytes = reader.bytes("wav.bytes")?.to_vec();
                    Ok((name, bytes))
                }
                1 => Err(reader.str("wav.err")?),
                other => return Err(CodecError::Tag("wav.ok", other)),
            };
            TaskResult::Wav(outcome)
        }
        2 => TaskResult::Split(read_split_result(&mut reader)?),
        3 => TaskResult::SplitSongs(read_split_result(&mut reader)?),
        4 => TaskResult::Peak(read_peak(&mut reader)?),
        5 => {
            let count = reader.u32("peaks.len")? as usize;
            let mut peaks = Vec::with_capacity(count);
            for _ in 0..count {
                let name = reader.str("peak.name")?;
                peaks.push((name, read_peak(&mut reader)?));
            }
            TaskResult::PackPeaks(peaks)
        }
        6 => {
            let count = reader.u32("candidates.len")? as usize;
            let mut candidates = Vec::with_capacity(count);
            for _ in 0..count {
                candidates.push(Candidate {
                    loop_point: reader.usize("candidate.loop_point")?,
                    loop_end: reader.usize("candidate.loop_end")?,
                    match_len: reader.usize("candidate.match_len")?,
                    ends_at_eof: reader.bool("candidate.ends_at_eof")?,
                    clean_repeat: reader.bool("candidate.clean_repeat")?,
                });
            }
            TaskResult::LoopCandidates(candidates)
        }
        other => return Err(CodecError::Tag("result.tag", other)),
    };
    Ok(result)
}

// -- pack export jobs -----------------------------------------------------------

/// Encodes a [`PackJobRequest`] for the pack Worker. Every field is scalars,
/// strings, or entry bytes, so unlike a task request this never fails.
#[must_use]
pub fn encode_pack_job(request: &PackJobRequest) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.str(&request.zip_name);
    writer.u32(request.entries.len() as u32);
    for entry in &request.entries {
        writer.str(&entry.name);
        writer.u8(match entry.kind {
            PackEntryKind::Song => 0,
            PackEntryKind::Image => 1,
            PackEntryKind::Doc => 2,
        });
        writer.bytes(&entry.bytes);
    }
    writer.bool(request.gzip_vgms);
    writer.bool(request.optimize_vgms);
    writer.u8(match request.optimizer {
        OptimizerChoice::Auto => 0,
        OptimizerChoice::BuiltInOnly => 1,
        OptimizerChoice::Tools => 2,
    });
    writer.out
}

/// Decodes a [`PackJobRequest`] on the Worker side.
pub fn decode_pack_job(input: &[u8]) -> Result<PackJobRequest> {
    let mut reader = Reader::new(input);
    let zip_name = reader.str("pack.zip_name")?;
    let count = reader.u32("pack.count")? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name = reader.str("pack.entry.name")?;
        let kind = match reader.u8("pack.entry.kind")? {
            0 => PackEntryKind::Song,
            1 => PackEntryKind::Image,
            2 => PackEntryKind::Doc,
            other => return Err(CodecError::Tag("pack entry kind", other)),
        };
        let bytes = reader.bytes("pack.entry.bytes")?.to_vec();
        entries.push(PackEntry { name, bytes, kind });
    }
    let gzip_vgms = reader.bool("pack.gzip")?;
    let optimize_vgms = reader.bool("pack.optimize")?;
    let optimizer = match reader.u8("pack.optimizer")? {
        0 => OptimizerChoice::Auto,
        1 => OptimizerChoice::BuiltInOnly,
        2 => OptimizerChoice::Tools,
        other => return Err(CodecError::Tag("pack optimizer", other)),
    };
    Ok(PackJobRequest {
        zip_name,
        entries,
        gzip_vgms,
        optimize_vgms,
        optimizer,
    })
}

/// Encodes a [`PackJobOutcome`] the Worker posts back.
#[must_use]
pub fn encode_pack_outcome(outcome: &PackJobOutcome) -> Vec<u8> {
    let mut writer = Writer::default();
    match outcome {
        PackJobOutcome::Done {
            zip_name,
            bytes,
            log,
        } => {
            writer.u8(0);
            writer.str(zip_name);
            writer.bytes(bytes);
            writer.u32(log.len() as u32);
            for line in log {
                writer.str(line);
            }
        }
        PackJobOutcome::Failed(message) => {
            writer.u8(1);
            writer.str(message);
        }
    }
    writer.out
}

/// Decodes a [`PackJobOutcome`] on the page side.
pub fn decode_pack_outcome(input: &[u8]) -> Result<PackJobOutcome> {
    let mut reader = Reader::new(input);
    let outcome = match reader.u8("pack outcome tag")? {
        0 => {
            let zip_name = reader.str("pack.done.zip_name")?;
            let bytes = reader.bytes("pack.done.bytes")?.to_vec();
            let count = reader.u32("pack.done.log_count")? as usize;
            let mut log = Vec::with_capacity(count);
            for _ in 0..count {
                log.push(reader.str("pack.done.log")?);
            }
            PackJobOutcome::Done {
                zip_name,
                bytes,
                log,
            }
        }
        1 => PackJobOutcome::Failed(reader.str("pack.failed")?),
        other => return Err(CodecError::Tag("pack outcome", other)),
    };
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use vgms_synth::{SplitFormat, VgmSplitOptions};
    use vgms_ui::tasks::{LoopSearchSource, SplitSource, SplitTaskSource, WavSource};

    use super::*;

    const OPL_VGM: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
    const OPL_DRO: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");

    // The codec's Opl-document arm carries a DRO `DroSong` now (an OPL VGM travels as
    // a `VgmFile` on the Vgm arm); a `DroSong` is always a DRO.
    fn sample_song() -> Arc<DroSong> {
        Arc::new(
            vgms_core::io::read_song("lsl3_score_up_dro2.dro", OPL_DRO).expect("fixture parses"),
        )
    }

    fn sample_vgm() -> Arc<VgmFile> {
        Arc::new(vgms_core::vgm::file::read("lsl3_score_up.vgm", OPL_VGM).expect("fixture parses"))
    }

    /// A non-empty per-render core map, so the render/split codec paths carry
    /// real choices across the wire.
    fn sample_core_choices() -> CoreChoices {
        CoreChoices::from([
            ("opl3".to_owned(), "cqm".to_owned()),
            ("ym2612".to_owned(), "nuked".to_owned()),
        ])
    }

    /// A config with every field pushed off its default, so a dropped or
    /// mis-ordered field in the codec shows up.
    fn sample_config() -> AudioConfig {
        let mut cores = BTreeMap::new();
        cores.insert("ym2612".to_owned(), "nuked".to_owned());
        cores.insert("sn76489".to_owned(), "mame".to_owned());
        AudioConfig {
            bit_depth: 24,
            boost: 2.5,
            lock_boost: true,
            buffer_size: 2048,
            frequency: 49_716,
            cores,
            resampling: "linear".to_owned(),
            retrowave_port: Some("COM7".to_owned()),
        }
    }

    /// A muting/panning that is neither all-on nor original, so both codec paths
    /// carry real data.
    fn sample_mix() -> RenderMix {
        let mut muting = Muting::all();
        muting.mute_channel(vgms_core::Bank::High, 0xB2);
        muting.set_percussion(vgms_core::Bank::Low, 0xE0);
        let mut pans = [0x80u8; 18];
        pans[0] = 0x00;
        pans[17] = 0xFF;
        RenderMix {
            muting,
            panning: Panning::Custom(pans),
            boost: 1.75,
        }
    }

    /// A non-neutral generic mix, so the chip-muting/chip-panning codec paths
    /// carry real per-chip data across two instances -- including the
    /// out-of-roster `u32::MAX` mask the channel split deliberately sets, and a
    /// variable-length pan array.
    fn sample_vgm_mix() -> VgmRenderMix {
        let mut muting = ChipMuting::new();
        muting.set(ChipKind::Ym2612, 0, 0b0000_0010);
        muting.set(ChipKind::Sn76489, 1, u32::MAX);
        let mut panning = ChipPanning::new();
        panning.set(ChipKind::Ym2612, 0, vec![-256, 0, 256]);
        VgmRenderMix {
            muting,
            panning,
            boost: 1.5,
        }
    }

    /// Every request variant round-trips: encoding what a decode produced yields
    /// the identical bytes, so no field is dropped, reordered, or misread.
    #[track_caller]
    fn assert_request_round_trips(request: &TaskRequest) {
        let bytes = encode_request(request).expect("encodes");
        let decoded = decode_request(&bytes).expect("decodes");
        let reencoded = encode_request(&decoded).expect("re-encodes");
        assert_eq!(bytes, reencoded, "request round-trip is not byte-stable");
    }

    #[track_caller]
    fn assert_result_round_trips(result: &TaskResult) {
        let bytes = encode_result(result);
        let decoded = decode_result(&bytes).expect("decodes");
        let reencoded = encode_result(&decoded);
        assert_eq!(bytes, reencoded, "result round-trip is not byte-stable");
    }

    #[test]
    fn every_request_variant_round_trips() {
        let requests = [
            TaskRequest::RenderWaveform {
                source: AudioSource::Dro(sample_song()),
                num_buckets: 4096,
                sample_rate: 48_000,
                resampling: ResampleMode::Sinc,
            },
            TaskRequest::RenderWaveform {
                source: AudioSource::Vgm(sample_vgm()),
                num_buckets: 512,
                sample_rate: 44_100,
                resampling: ResampleMode::Linear,
            },
            // A non-empty core map on this RenderWav case, empty on the next, so
            // both branches of the core-choices codec are exercised.
            TaskRequest::RenderWav {
                source: WavSource::Dro(sample_song()),
                mix: RenderWavMix::Opl(sample_mix()),
                sample_rate: 49_716,
                bit_depth: 24,
                resampling: ResampleMode::Sinc,
                core_choices: sample_core_choices(),
            },
            TaskRequest::RenderWav {
                source: WavSource::Vgm(sample_vgm()),
                mix: RenderWavMix::Vgm(sample_vgm_mix()),
                sample_rate: 44_100,
                bit_depth: 16,
                resampling: ResampleMode::Linear,
                core_choices: CoreChoices::new(),
            },
            TaskRequest::Split {
                source: SplitTaskSource::Vgm {
                    file: sample_vgm(),
                    options: VgmSplitOptions {
                        // A song-format VGM split, so the format byte round-trips
                        // as a non-default value.
                        format: SplitFormat::Song,
                        audio: sample_config(),
                        resampling: ResampleMode::Linear,
                        panning: sample_vgm_mix().panning,
                        boost: 3.0,
                        skip_muted: Some(sample_vgm_mix().muting),
                        core_choices: CoreChoices::new(),
                    },
                },
            },
            TaskRequest::SplitSongs {
                source: SplitSource::Dro(sample_song()),
                threshold_native: 33_075,
                included: vec![true, false, true, true],
                trailing_tail: 4410,
            },
            TaskRequest::VolumeScan {
                source: AudioSource::Dro(sample_song()),
                sample_rate: 48_000,
                resampling: ResampleMode::Sinc,
            },
            // A VGM-source scan, and a pack list with both arms, so the codec's
            // audio-source tag is exercised for VolumeScan too.
            TaskRequest::VolumeScan {
                source: AudioSource::Vgm(sample_vgm()),
                sample_rate: 44_100,
                resampling: ResampleMode::Linear,
            },
            TaskRequest::PackVolumeScan {
                tracks: vec![
                    ("01 first.vgm".to_owned(), AudioSource::Dro(sample_song())),
                    ("02 second.vgm".to_owned(), AudioSource::Vgm(sample_vgm())),
                ],
                sample_rate: 44_100,
                resampling: ResampleMode::Linear,
            },
            TaskRequest::LoopSearch {
                source: LoopSearchSource::Vgm(sample_vgm()),
                min_len_commands: 64,
            },
        ];
        for request in &requests {
            assert_request_round_trips(request);
        }
    }

    #[test]
    fn scalar_request_fields_decode_to_what_was_encoded() {
        // The byte-stable round-trip proves faithfulness structurally; this pins
        // that the scalars are the actual values, not just self-consistent ones.
        let bytes = encode_request(&TaskRequest::VolumeScan {
            source: AudioSource::Dro(sample_song()),
            sample_rate: 12_345,
            resampling: ResampleMode::Sinc,
        })
        .unwrap();
        let TaskRequest::VolumeScan { sample_rate, .. } = decode_request(&bytes).unwrap() else {
            panic!("decoded the wrong variant");
        };
        assert_eq!(sample_rate, 12_345);

        let bytes = encode_request(&TaskRequest::SplitSongs {
            source: SplitSource::Dro(sample_song()),
            threshold_native: 7,
            included: vec![false, true, false],
            trailing_tail: 99,
        })
        .unwrap();
        let TaskRequest::SplitSongs {
            threshold_native,
            included,
            trailing_tail,
            ..
        } = decode_request(&bytes).unwrap()
        else {
            panic!("decoded the wrong variant");
        };
        assert_eq!(threshold_native, 7);
        assert_eq!(included, vec![false, true, false]);
        assert_eq!(trailing_tail, 99);
    }

    #[test]
    fn the_audio_config_survives_the_codec() {
        // Round-trip the config alone through a request that carries one.
        let original = sample_config();
        let bytes = encode_request(&TaskRequest::Split {
            source: SplitTaskSource::Vgm {
                file: sample_vgm(),
                options: VgmSplitOptions {
                    format: SplitFormat::Wav,
                    audio: original.clone(),
                    resampling: ResampleMode::Sinc,
                    panning: ChipPanning::new(),
                    boost: 1.0,
                    skip_muted: None,
                    core_choices: CoreChoices::new(),
                },
            },
        })
        .unwrap();
        let TaskRequest::Split {
            source: SplitTaskSource::Vgm { options, .. },
        } = decode_request(&bytes).unwrap()
        else {
            panic!("decoded the wrong variant");
        };
        assert_eq!(options.audio, original, "every audio-config field survives");
    }

    #[test]
    fn every_result_variant_round_trips() {
        let results = [
            TaskResult::Waveform(vec![
                WaveformBucket {
                    min: -100,
                    max: 200,
                },
                WaveformBucket {
                    min: -32768,
                    max: 32767,
                },
            ]),
            TaskResult::Wav(Ok(("song.dro.wav".to_owned(), vec![1, 2, 3, 4]))),
            TaskResult::Wav(Err("render failed".to_owned())),
            TaskResult::Split(Ok(vec![
                ("00 A.wav".to_owned(), vec![9, 8, 7]),
                ("01 B.wav".to_owned(), vec![]),
            ])),
            TaskResult::Split(Err("split failed".to_owned())),
            TaskResult::SplitSongs(Ok(vec![("01 x.vgm".to_owned(), vec![0x56])])),
            TaskResult::Peak(Peak {
                max_level: 12_345,
                clipped: true,
            }),
            TaskResult::PackPeaks(vec![
                (
                    "01.vgm".to_owned(),
                    Peak {
                        max_level: 1,
                        clipped: false,
                    },
                ),
                (
                    "02.vgm".to_owned(),
                    Peak {
                        max_level: 32_767,
                        clipped: true,
                    },
                ),
            ]),
            TaskResult::LoopCandidates(vec![Candidate {
                loop_point: 10,
                loop_end: 250,
                match_len: 120,
                ends_at_eof: true,
                clean_repeat: false,
            }]),
        ];
        for result in &results {
            assert_result_round_trips(result);
        }
    }

    #[test]
    fn scalar_result_fields_decode_to_what_was_encoded() {
        let bytes = encode_result(&TaskResult::Peak(Peak {
            max_level: -321,
            clipped: true,
        }));
        let TaskResult::Peak(peak) = decode_result(&bytes).unwrap() else {
            panic!("decoded the wrong variant");
        };
        assert_eq!(peak.max_level, -321);
        assert!(peak.clipped);

        let bytes = encode_result(&TaskResult::Waveform(vec![WaveformBucket {
            min: -7,
            max: 11,
        }]));
        let TaskResult::Waveform(buckets) = decode_result(&bytes).unwrap() else {
            panic!("decoded the wrong variant");
        };
        assert_eq!(buckets, vec![WaveformBucket { min: -7, max: 11 }]);
    }

    #[test]
    fn a_truncated_buffer_errors_rather_than_panics() {
        let bytes = encode_request(&TaskRequest::VolumeScan {
            source: AudioSource::Dro(sample_song()),
            sample_rate: 48_000,
            resampling: ResampleMode::Sinc,
        })
        .unwrap();
        // Lopping off the tail must surface an Eof, never an index panic.
        assert!(matches!(
            decode_request(&bytes[..bytes.len() - 3]),
            Err(CodecError::Eof(_))
        ));
        // An empty buffer has not even a tag byte.
        assert!(matches!(decode_request(&[]), Err(CodecError::Eof(_))));
        // An unknown request tag is a Tag error, not a silent misparse.
        assert!(matches!(
            decode_request(&[0xFF]),
            Err(CodecError::Tag("request.tag", 0xFF))
        ));
    }

    #[test]
    fn pack_job_round_trips() {
        let request = PackJobRequest {
            zip_name: "Great Game (1991).zip".to_owned(),
            entries: vec![
                PackEntry {
                    name: "01 Intro.vgm".to_owned(),
                    bytes: OPL_VGM.to_vec(),
                    kind: PackEntryKind::Song,
                },
                PackEntry {
                    name: "Cover.png".to_owned(),
                    bytes: vec![0x89, b'P', b'N', b'G'],
                    kind: PackEntryKind::Image,
                },
                PackEntry {
                    name: "Great Game.txt".to_owned(),
                    bytes: b"notes".to_vec(),
                    kind: PackEntryKind::Doc,
                },
            ],
            gzip_vgms: true,
            optimize_vgms: false,
            // A non-default value, so a dropped field would fail the round trip
            // (Auto is 0, which would survive even an unwritten byte).
            optimizer: OptimizerChoice::BuiltInOnly,
        };
        let decoded = decode_pack_job(&encode_pack_job(&request)).expect("round trips");
        assert_eq!(decoded.zip_name, request.zip_name);
        assert_eq!(decoded.gzip_vgms, request.gzip_vgms);
        assert_eq!(decoded.optimize_vgms, request.optimize_vgms);
        assert_eq!(decoded.optimizer, request.optimizer);
        assert_eq!(decoded.entries.len(), 3);
        assert_eq!(decoded.entries[0].name, "01 Intro.vgm");
        assert!(matches!(decoded.entries[0].kind, PackEntryKind::Song));
        assert_eq!(decoded.entries[0].bytes, OPL_VGM);
        assert!(matches!(decoded.entries[1].kind, PackEntryKind::Image));
        assert!(matches!(decoded.entries[2].kind, PackEntryKind::Doc));
    }

    #[test]
    fn pack_outcome_round_trips() {
        let done = PackJobOutcome::Done {
            zip_name: "Game.zip".to_owned(),
            bytes: vec![b'P', b'K', 3, 4, 9, 9],
            log: vec!["01 Intro.vgm -> 01 Intro.vgz".to_owned(), "done".to_owned()],
        };
        match decode_pack_outcome(&encode_pack_outcome(&done)).expect("done round trips") {
            PackJobOutcome::Done {
                zip_name,
                bytes,
                log,
            } => {
                assert_eq!(zip_name, "Game.zip");
                assert_eq!(bytes, vec![b'P', b'K', 3, 4, 9, 9]);
                assert_eq!(log.len(), 2);
            }
            PackJobOutcome::Failed(message) => panic!("expected Done, got Failed({message})"),
        }

        let failed = PackJobOutcome::Failed("boom".to_owned());
        match decode_pack_outcome(&encode_pack_outcome(&failed)).expect("failed round trips") {
            PackJobOutcome::Failed(message) => assert_eq!(message, "boom"),
            PackJobOutcome::Done { .. } => panic!("expected Failed"),
        }
    }
}
