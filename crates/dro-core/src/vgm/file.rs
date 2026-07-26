//! A VGM file: one model, whatever chips it declares.
//!
//! There is no second kind of VGM. A Mega Drive rip and an AdLib rip are the
//! same thing here -- a header, a command stream and a tag -- and every
//! chip-agnostic feature (tags, durations, trimming, export) works on both.
//!
//! What an OPL file has is *more*, not different: [`VgmFile::opl`] hands out a
//! projection of the same stream as OPL instructions, and that is what the
//! register analyser, find-register and the synth read. `None` from it does
//! not mean the file is broken or foreign; it means the OPL extras do not
//! apply. See [`vgm::projection`](crate::vgm::projection).
//!
//! # Byte-exact retagging
//!
//! Reading a file and writing it back reproduces it exactly: the header is
//! verbatim, the body is verbatim (padding between the end-of-data marker and
//! the tag included), and only the EOF and GD3 offsets are patched -- to the
//! values they already held. The single exception is a file that stores its
//! GD3 *before* its data, which cannot round-trip because the rewritten tag
//! goes at the end; see [`write`].

use std::io::Read;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::chip_state::{self, ChipState};
use crate::error::{Error, Result};
use crate::song::{OplType, Song, slide_index_past_deletion};
use crate::vgm::data::{Gd3Tag, VgmMeta};
use crate::vgm::header::{LEGACY_DATA_START, VgmHeader, offset};
use crate::vgm::io::{is_gzipped, parse_gd3_tag, write_gd3_tag};
use crate::vgm::projection::{OplProjection, opl_type_of};
use crate::vgm::stream::{END_OF_DATA, VgmStream};

/// The header block a GD3 tag carries before its strings: magic, version, length.
const GD3_PREAMBLE: usize = 12;
/// A GD3 stored before the data can only be relocated if it sits past every
/// pointer field the relocation has to patch.
const LAST_POINTER_FIELD_END: usize = offset::EXTRA_HEADER + 4;

/// A VGM file's command stream.
///
/// Normally [`Commands`](VgmBody::Commands): walked, indexed and describable.
/// [`Opaque`](VgmBody::Opaque) is the fallback for a stream that will not walk
/// -- a command with no defined length, or one running off the end. Such a file
/// keeps its tags, which is the whole reason the fallback exists; it just
/// cannot be edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VgmBody {
    Commands(VgmStream),
    Opaque(Vec<u8>),
}

impl VgmBody {
    /// The raw bytes, whatever the representation.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        match self {
            Self::Commands(stream) => stream.raw(),
            Self::Opaque(bytes) => bytes,
        }
    }

    /// The parsed stream, if the body walked.
    #[must_use]
    pub const fn stream(&self) -> Option<&VgmStream> {
        match self {
            Self::Commands(stream) => Some(stream),
            Self::Opaque(_) => None,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.raw().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw().is_empty()
    }
}

/// What a region edit had to do, for the status bar to report.
///
/// Neither field is an error. `restored` says how many commands were re-emitted
/// to put the chips where the music expects them; `unmodelled` counts the
/// commands in the discarded span whose state this app cannot replay -- a PCM
/// RAM write, a reserved opcode -- which is a caveat worth showing, not a
/// reason to refuse the edit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RegionReport {
    pub restored: usize,
    pub unmodelled: usize,
}

/// The bytes of rows `from..to`.
fn span(stream: &VgmStream, from: usize, to: usize) -> &[u8] {
    let start = stream.byte_offset(from).unwrap_or(0);
    let end = stream.byte_offset(to).unwrap_or(start);
    &stream.raw()[start..end]
}

/// A VGM file for any chip, with its tags editable and its music left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmFile {
    /// The file's own name, as it was opened. Renaming is the caller's to do.
    pub name: String,
    pub header: VgmHeader,
    pub body: VgmBody,
    pub tag: Option<Gd3Tag>,
    /// The OPL this file presents as, if it presents as one at all.
    ///
    /// Cached because answering needs a walk of the whole stream (every
    /// command must *be* an OPL instruction, not merely the header say so --
    /// see [`opl_type_of`]), and the answer is asked for far more often than
    /// the stream changes. Recomputed by every edit that touches the stream.
    opl: Option<OplType>,
}

impl VgmFile {
    /// The song's length in samples, from the header.
    #[must_use]
    pub const fn total_samples(&self) -> u32 {
        self.header.total_samples()
    }

    /// The song's length in milliseconds, from the header.
    #[must_use]
    pub fn total_ms(&self) -> u32 {
        crate::util::smp_to_ms(self.header.total_samples(), crate::util::VGM_SAMPLE_RATE)
    }

    /// The loop's length in samples, or `None` if the file does not loop.
    #[must_use]
    pub const fn loop_samples(&self) -> Option<u32> {
        self.header.loop_samples()
    }

    /// The chips this file declares, e.g. `"SN76489, YM2612"`.
    #[must_use]
    pub fn chip_list(&self) -> String {
        self.header.chip_list()
    }

    /// Whether the OPL editor could open this file instead.
    ///
    /// True does not promise the editor *will* succeed -- the command stream
    /// still has to decode -- only that the chips are ones it knows.
    #[must_use]
    pub fn is_opl_only(&self) -> bool {
        self.header.is_opl_only()
    }

    /// This file's commands seen as OPL instructions, when it is an OPL file.
    ///
    /// The gate for every OPL-only feature: the register analyser, the
    /// synth, find-register, the OPL optimiser. `None` is not a failure --
    /// it means "this VGM is not an OPL one", and everything chip-agnostic
    /// still applies.
    #[must_use]
    pub fn opl(&self) -> Option<OplProjection<'_>> {
        Some(OplProjection::new(self.stream()?, self.opl?))
    }

    /// Whether the OPL-only features apply to this file.
    #[must_use]
    pub const fn is_opl(&self) -> bool {
        self.opl.is_some()
    }

    /// Decides whether a body is an OPL one: the header must name a single OPL,
    /// and every command must actually be an instruction for it.
    fn derive_opl(header: &VgmHeader, body: &VgmBody) -> Option<OplType> {
        let opl = opl_type_of(header)?;
        body.stream()
            .is_some_and(crate::vgm::projection::is_wholly_opl)
            .then_some(opl)
    }

    /// A [`Song`] snapshot of this file, for the paths that consume one.
    ///
    /// `None` unless the file is an OPL one. The snapshot carries the same
    /// metadata the OPL reader would have produced, so the synth, the
    /// waveform render and the analyser see exactly what they always have.
    #[must_use]
    pub fn to_song(&self) -> Option<Song> {
        let opl = self.opl()?;
        Some(opl.to_song(self.name.clone(), self.header.version(), self.vgm_meta()))
    }

    /// The metadata a [`Song`] snapshot carries: the loop as instruction
    /// indices, the modifiers, the tag, and the header bytes verbatim.
    #[must_use]
    pub fn vgm_meta(&self) -> VgmMeta {
        VgmMeta {
            loop_point: self.loop_index(),
            loop_end: self.loop_end_index(),
            loop_base: self.header.loop_base(),
            loop_modifier: self.header.loop_modifier(),
            volume_modifier: self.header.volume_modifier(),
            tag: self.tag.clone(),
            header: self.header.raw().to_vec(),
        }
    }

    /// Where the loop stops, as an exclusive row index, or `None` for "runs to
    /// the end".
    ///
    /// The header states the loop's length in samples, which usually means the
    /// end of the file; a shorter value landing on a command boundary is how a
    /// loop that stops early is expressed, and is materialised so that saving
    /// does not silently widen it. See [`VgmMeta::loop_end`].
    #[must_use]
    pub fn loop_end_index(&self) -> Option<usize> {
        let stream = self.stream()?;
        let start = self.loop_index()?;
        let declared = u64::from(self.header.loop_samples()?);
        let to_end = stream.samples_from(start);
        if declared >= to_end {
            // Equal is the ordinary "loops to the end"; longer is a stale
            // header the stream disagrees with. Neither bounds a region.
            return None;
        }
        // The first row at exactly `declared` samples past the loop point.
        // Zero-wait rows share a timestamp, so this lands on the first of them.
        let mut elapsed = 0u64;
        for index in start..stream.len() {
            if elapsed == declared && index > start {
                return Some(index);
            }
            elapsed += u64::from(stream.wait_samples(index));
        }
        None
    }

    /// The parsed command stream, or `None` if it would not walk.
    #[must_use]
    pub const fn stream(&self) -> Option<&VgmStream> {
        self.body.stream()
    }

    /// How many commands the stream holds, or 0 if it would not walk.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stream().map_or(0, VgmStream::len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The command the loop restarts at, as an index into the stream.
    ///
    /// `None` when the file does not loop, its stream did not walk, or its loop
    /// pointer lands somewhere that is not a command boundary -- a corrupt
    /// pointer, which is not worth carrying forward.
    #[must_use]
    pub fn loop_index(&self) -> Option<usize> {
        let stream = self.stream()?;
        let absolute = self.header.loop_offset()?;
        let in_stream = absolute.checked_sub(self.header.data_start())?;
        stream.index_at_byte_offset(in_stream)
    }

    /// Removes the commands at `indices`, and brings the header back into step:
    /// the sample total, and the loop's offset and length.
    ///
    /// Doing the repatch *here*, rather than in [`write`], is what keeps an
    /// untouched file byte-exact: the writer never recomputes anything, so a
    /// file that is only retagged cannot have a disagreeing header quietly
    /// "corrected" underneath it. The header's own bytes stay the one truth.
    ///
    /// Returns whether anything was removed.
    pub fn delete_commands(&mut self, indices: &[usize]) -> bool {
        let Some(stream) = self.body.stream() else {
            return false;
        };
        let before = stream.len();
        // Where the loop was, and where the deletion leaves it.
        let loop_index = self.loop_index();
        let mut sorted: Vec<usize> = indices.iter().copied().filter(|&i| i < before).collect();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.is_empty() {
            return false;
        }

        let VgmBody::Commands(stream) = &mut self.body else {
            unreachable!("checked above");
        };
        stream.delete_many(&sorted);
        let surviving = stream.len();
        let total = stream.total_samples();

        let moved =
            loop_index.and_then(|index| slide_index_past_deletion(index, &sorted, surviving));
        self.repatch_header(moved, total);
        self.refresh_opl();
        true
    }

    /// Keeps only rows `start..end`, prefixed by the chip state the discarded
    /// head had established.
    ///
    /// The music from `start` on was written against chips that had already
    /// been configured, and those writes are in the part being thrown away --
    /// so they are folded into a state and re-emitted first: data blocks, then
    /// each latched register's last write in the order it happened. Notes that
    /// were sounding at `start` re-attack, which is what `vgm_trim` does too.
    ///
    /// Returns what could not be modelled (see [`RegionReport`]), or `None` if
    /// the region is empty, out of range, or the stream did not walk.
    pub fn crop_to_region(&mut self, start: usize, end: usize) -> Option<RegionReport> {
        let stream = self.stream()?;
        let end = end.min(stream.len());
        if start >= end {
            return None;
        }
        let head = ChipState::fold(stream, start);
        let report = RegionReport {
            unmodelled: chip_state::unmodelled_commands(stream, start).len(),
            restored: head.restore_indices().len(),
        };

        let mut bytes = ChipState::bytes_for(stream, &head.restore_indices());
        let prelude_len = bytes.len();
        bytes.extend_from_slice(span(stream, start, end));
        bytes.push(END_OF_DATA);

        // A loop inside the kept region moves with it; one outside it is gone.
        let loop_at = self.loop_index().filter(|&at| (start..end).contains(&at));
        let new_loop = loop_at.map(|at| {
            prelude_len + stream.byte_offset(at).unwrap_or(0)
                - stream.byte_offset(start).unwrap_or(0)
        });
        self.rebuild(bytes, new_loop);
        Some(report)
    }

    /// Removes rows `start..end`, bridging the seam with what that span
    /// actually changed.
    ///
    /// Not every write in the removed span is re-emitted -- only the cells
    /// whose value differs across it, plus any data block it loaded (banks are
    /// cumulative, so a later seek still indexes them). What follows the seam
    /// therefore meets the chips in the state it expects.
    ///
    /// Returns what could not be modelled, or `None` if the region is empty,
    /// out of range, or the stream did not walk.
    pub fn delete_region(&mut self, start: usize, end: usize) -> Option<RegionReport> {
        let stream = self.stream()?;
        let end = end.min(stream.len());
        if start >= end {
            return None;
        }
        let before = ChipState::fold(stream, start);
        let after = ChipState::fold(stream, end);
        let patch = after.changes_from(&before);
        let report = RegionReport {
            unmodelled: chip_state::unmodelled_commands(stream, end).len()
                - chip_state::unmodelled_commands(stream, start).len(),
            restored: patch.len(),
        };

        let mut bytes = span(stream, 0, start).to_vec();
        bytes.extend(ChipState::bytes_for(stream, &patch));
        let tail_at = bytes.len();
        bytes.extend_from_slice(span(stream, end, stream.len()));
        bytes.push(END_OF_DATA);

        // A loop before the cut keeps its offset; one after it slides to the
        // new tail; one *inside* it has gone with the region it pointed into.
        let new_loop = self.loop_index().and_then(|at| {
            if at < start {
                stream.byte_offset(at)
            } else if at >= end {
                Some(tail_at + stream.byte_offset(at)? - stream.byte_offset(end)?)
            } else {
                None
            }
        });
        self.rebuild(bytes, new_loop);
        Some(report)
    }

    /// Installs rebuilt stream bytes, leaving the header alone.
    ///
    /// For the structural half of a header fix, which repatches the derived
    /// fields itself afterwards. Everything else should use [`Self::rebuild`],
    /// which keeps the header in step for you.
    pub(crate) fn replace_stream(&mut self, bytes: Vec<u8>) {
        if let Ok(stream) = VgmStream::parse(bytes, self.header.version()) {
            self.body = VgmBody::Commands(stream);
            self.refresh_opl();
        }
    }

    /// Installs a rebuilt stream and brings the header back into step.
    ///
    /// `loop_at` is a byte offset into the *new* stream, or `None` for a file
    /// that no longer loops.
    fn rebuild(&mut self, bytes: Vec<u8>, loop_at: Option<usize>) {
        let version = self.header.version();
        let stream = match VgmStream::parse(bytes, version) {
            Ok(stream) => stream,
            Err(error) => {
                // Unreachable: every byte came from a stream that already
                // walked, spliced only at command boundaries.
                log::error!("rebuilt VGM stream does not walk ({error}); leaving it alone");
                return;
            }
        };
        let total = stream.total_samples();
        let loop_index = loop_at.and_then(|at| stream.index_at_byte_offset(at));
        self.body = VgmBody::Commands(stream);
        self.repatch_header(loop_index, total);
        self.refresh_opl();
    }

    /// Drops the register writes that change nothing, for any chip this app
    /// has rules for.
    ///
    /// The `vgm_cmp` pass, generalised. Chips without rules keep every write
    /// (see [`chip_state::has_latch_rules`]), so running this over an
    /// unfamiliar file is safe rather than merely likely to be -- worst case it
    /// does nothing.
    ///
    /// Returns how many commands went, or `None` if nothing did.
    pub fn optimize(&mut self) -> Option<usize> {
        let stream = self.stream()?;
        let redundant = chip_state::redundant_indices(stream, self.loop_index());
        if redundant.is_empty() {
            return None;
        }
        let removed = redundant.len();
        self.delete_commands(&redundant).then_some(removed)
    }

    /// The chips in this file that no redundancy rule covers, by name.
    ///
    /// What the export log names when it leaves a file alone: "YM2612 is not
    /// optimised yet" is a better answer than silence, and a much better one
    /// than a smaller file that plays wrong.
    #[must_use]
    pub fn unoptimised_chips(&self) -> Vec<&'static str> {
        self.header
            .chips()
            .iter()
            .filter(|chip| !chip_state::has_latch_rules(chip.kind))
            .map(|chip| chip.kind.name())
            .collect()
    }

    /// Puts commands back where they were, for undo.
    ///
    /// The header is *not* repatched here: its exact previous values are
    /// restored by the undo command, which captured them, rather than
    /// recomputed -- a loop point that was itself deleted slid onto its
    /// successor, and no arithmetic can slide it back.
    pub fn insert_commands(&mut self, entries: &[(usize, Box<[u8]>)]) {
        if let VgmBody::Commands(stream) = &mut self.body {
            stream.insert_many(entries);
        }
        self.refresh_opl();
    }

    /// Re-answers "is this an OPL file?" after the stream changed.
    ///
    /// It can genuinely change: deleting the one command that was not an OPL
    /// instruction makes the rest of the file an OPL one.
    fn refresh_opl(&mut self) {
        self.opl = Self::derive_opl(&self.header, &self.body);
    }

    /// Rewrites the header's derived fields from the stream as it now stands.
    fn repatch_header(&mut self, loop_index: Option<usize>, total_samples: u64) {
        let data_start = self.header.data_start();
        let (absolute, loop_samples) = match (loop_index, self.body.stream()) {
            (Some(index), Some(stream)) => {
                let at = stream.byte_offset(index).map(|offset| data_start + offset);
                let after = stream.samples_from(index);
                (at, u32::try_from(after).unwrap_or(u32::MAX))
            }
            _ => (None, 0),
        };
        self.header.set_loop(absolute, loop_samples);
        self.header
            .set_total_samples(u32::try_from(total_samples).unwrap_or(u32::MAX));
    }
}

/// Reads any VGM or VGZ file, whatever chips it declares.
///
/// # Errors
/// If the gzip stream is corrupt, the magic is wrong, the version predates
/// 1.00, the data offset points outside the file, or a declared GD3 tag is
/// malformed.
pub fn read(name: &str, bytes: &[u8]) -> Result<VgmFile> {
    if is_gzipped(bytes) {
        let mut decoded = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut decoded)
            .map_err(|error| Error::file(format!("Could not decompress the VGZ file: {error}")))?;
        read_uncompressed(name, &decoded)
    } else {
        read_uncompressed(name, bytes)
    }
}

/// Serialises a file back to VGM bytes.
///
/// The header and body go out verbatim and only the EOF and GD3 offsets are
/// patched, so an unedited file reproduces itself byte for byte.
///
/// A file whose GD3 sits *before* its data is the one shape that cannot: the
/// rewritten tag always goes at the end, so the old bytes are cut out of the
/// header and the data, loop and extra-header pointers slide back by what was
/// removed. The result is a smaller, conventionally ordered file with the same
/// music and the same tag.
///
/// # Errors
/// If the header is too short to hold the fields being patched, or an embedded
/// GD3 declares a length that runs past it.
pub fn write(file: &VgmFile) -> Result<Vec<u8>> {
    let mut header = file.header.raw().to_vec();
    if header.len() < LEGACY_DATA_START {
        return Err(Error::file(format!(
            "VGM header is {} bytes; the smallest legal header is {LEGACY_DATA_START:#X}",
            header.len()
        )));
    }
    if let Some(at) = file.header.gd3_offset().filter(|&at| at < header.len()) {
        relocate_embedded_gd3(&mut header, at)?;
    }

    let gd3 = file.tag.as_ref().map(write_gd3_tag);
    let mut out = header;
    out.extend_from_slice(file.body.raw());

    let gd3_start = out.len();
    if let Some(gd3) = &gd3 {
        out.extend_from_slice(gd3);
    }

    let eof = out.len();
    put_u32(&mut out, offset::EOF, (eof - offset::EOF) as u32);
    put_u32(
        &mut out,
        offset::GD3,
        if gd3.is_some() {
            (gd3_start - offset::GD3) as u32
        } else {
            0
        },
    );
    Ok(out)
}

/// Serialises a file to gzipped VGZ bytes.
///
/// # Errors
/// As [`write`], plus any compression failure.
pub fn write_gzipped(file: &VgmFile) -> Result<Vec<u8>> {
    use std::io::Write;

    let plain = write(file)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&plain)
        .and_then(|()| encoder.finish())
        .map_err(|error| Error::file(format!("Could not compress the VGZ file: {error}")))
}

// ---------------------------------------------------------------------------

fn read_uncompressed(name: &str, bytes: &[u8]) -> Result<VgmFile> {
    let header = VgmHeader::parse(bytes)?;
    let data_start = header.data_start();

    // The EOF field is the file's own idea of where it ends. Trust it only as
    // far as the bytes actually go: a truncated download should still open for
    // its tags, and a file with junk appended should not swallow the junk.
    let declared_eof = match u32_at(bytes, offset::EOF) {
        0 => bytes.len(),
        relative => offset::EOF + relative as usize,
    };
    if declared_eof > bytes.len() {
        log::warn!(
            "VGM header claims the file ends at {declared_eof:#X}, past its actual {:#X} bytes",
            bytes.len()
        );
    }
    let file_end = declared_eof.min(bytes.len());

    // A tag before the data is either a deliberate but unusual layout or a
    // stale pointer; either way it is not part of the command stream.
    let tag_at = header.gd3_offset();
    let mut body_end = file_end.max(data_start);
    if let Some(at) = tag_at
        && at >= data_start
        && at < body_end
    {
        body_end = at;
    }
    let body = bytes
        .get(data_start..body_end)
        .ok_or_else(|| {
            Error::file(format!(
                "VGM data runs from {data_start:#X} to {body_end:#X}, outside the {} byte file",
                bytes.len()
            ))
        })?
        .to_vec();
    // A stream that will not walk is kept whole rather than refused: the file's
    // tags are still perfectly good, and they are what this type is for.
    let body = match VgmStream::parse(body, header.version()) {
        Ok(stream) => {
            let from_stream = stream.total_samples();
            let declared = u64::from(header.total_samples());
            if from_stream != declared {
                log::warn!(
                    "VGM header claims {declared} samples, but its waits sum to {from_stream}"
                );
            }
            VgmBody::Commands(stream)
        }
        Err(error) => {
            log::warn!("{name}: keeping the VGM data unparsed ({error})");
            // `parse` consumed the vector, so rebuild the span it was given.
            VgmBody::Opaque(bytes[data_start..body_end].to_vec())
        }
    };

    let tag = match tag_at {
        Some(at) if at < data_start && at < LAST_POINTER_FIELD_END => {
            // The tag would overlap the header's own fields, so the pointer is
            // corrupt rather than unusual. Dropping it loses nothing that could
            // be trusted, and keeps the file openable.
            log::warn!(
                "VGM GD3 pointer at {at:#X} lands inside the header's own fields; ignoring the tag"
            );
            None
        }
        Some(at) => Some(parse_gd3_tag(bytes, at)?),
        None => None,
    };

    let opl = VgmFile::derive_opl(&header, &body);
    Ok(VgmFile {
        name: name.to_owned(),
        header,
        body,
        tag,
        opl,
    })
}

/// Cuts a GD3 stored inside the header out, sliding every pointer past it.
///
/// The tag is rewritten at the end of the file, so its old bytes are dead
/// weight. Each pointer field sits before the tag (the caller has checked
/// that), so only the *targets* move: a pointer whose target was past the tag
/// loses exactly the bytes that were removed.
fn relocate_embedded_gd3(header: &mut Vec<u8>, at: usize) -> Result<()> {
    if at < LAST_POINTER_FIELD_END {
        // Unreachable via `read`, which drops such a pointer, but a hand-built
        // file could still carry one.
        return Err(Error::file(format!(
            "VGM GD3 pointer at {at:#X} lands inside the header's own fields"
        )));
    }
    let length = u32_at(header, at + 8) as usize;
    let end = at
        .checked_add(GD3_PREAMBLE + length)
        .filter(|&end| end <= header.len())
        .ok_or_else(|| {
            Error::file(format!(
                "VGM GD3 at {at:#X} declares {length} bytes, which runs past the header's {:#X}",
                header.len()
            ))
        })?;

    header.drain(at..end);
    let removed = (end - at) as u32;
    for field in [
        offset::DATA_OFFSET,
        offset::LOOP_OFFSET,
        offset::EXTRA_HEADER,
    ] {
        slide_pointer(header, field, at, removed);
    }
    Ok(())
}

/// Subtracts `removed` from a relative pointer whose target sat past `cut`.
///
/// A zero pointer means "absent" for every field this is used on, and stays
/// zero.
fn slide_pointer(header: &mut [u8], field: usize, cut: usize, removed: u32) {
    if field + 4 > header.len() {
        return;
    }
    let relative = u32_at(header, field);
    if relative == 0 {
        return;
    }
    if field + relative as usize > cut {
        put_u32(header, field, relative - removed);
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    match bytes.get(at..at + 4) {
        Some(slice) => u32::from_le_bytes(slice.try_into().expect("a four byte slice")),
        None => 0,
    }
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgm::header::ChipKind;
    use crate::vgm::io::GD3_MAGIC;

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");

    fn tag() -> Gd3Tag {
        Gd3Tag {
            track_name_en: "Green Hill Zone".to_owned(),
            game_name_en: "Sonic the Hedgehog".to_owned(),
            system_name_en: "Sega Mega Drive".to_owned(),
            track_author_en: "Masato Nakamura".to_owned(),
            release_date: "1991-07-26".to_owned(),
            ..Gd3Tag::default()
        }
    }

    /// A synthetic Mega Drive file: a YM2612 and an SN76489, a body of bytes
    /// this app cannot decode, and optionally a tag at the end.
    fn mega_drive(with_tag: bool) -> Vec<u8> {
        build(0x161, 0x100, MEGA_DRIVE_BODY, with_tag)
    }

    /// A YM2612 DAC write, a PSG write, a wait, and the end marker -- none of
    /// which the OPL command table can size.
    const MEGA_DRIVE_BODY: &[u8] = &[
        0x52, 0x28, 0xF0, // YM2612 port 0
        0x50, 0x9F, // SN76489
        0x61, 0x10, 0x27, // wait 10000
        0x80, // DAC write + wait 0
        0x66, // end of data
    ];

    fn build(version: u32, header_size: usize, body: &[u8], with_tag: bool) -> Vec<u8> {
        let mut header = vec![0u8; header_size];
        header[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut header, offset::VERSION, version);
        put_u32(
            &mut header,
            offset::DATA_OFFSET,
            (header_size - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut header, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut header, ChipKind::Sn76489.clock_offset(), 3_579_545);
        put_u32(&mut header, offset::TOTAL_SAMPLES, 10_000);

        let mut out = header;
        out.extend_from_slice(body);
        if with_tag {
            let gd3_at = out.len();
            put_u32(&mut out, offset::GD3, (gd3_at - offset::GD3) as u32);
            out.extend_from_slice(&write_gd3_tag(&tag()));
        }
        let eof = out.len();
        put_u32(&mut out, offset::EOF, (eof - offset::EOF) as u32);
        out
    }

    #[test]
    fn reads_a_file_whose_chips_the_editor_cannot_open() {
        let file = read("sonic.vgm", &mega_drive(true)).unwrap();
        assert_eq!(file.name, "sonic.vgm");
        assert_eq!(file.chip_list(), "SN76489, YM2612");
        assert!(!file.is_opl_only());
        assert_eq!(file.total_samples(), 10_000);
        assert_eq!(file.total_ms(), 227);
        assert_eq!(file.tag.as_ref(), Some(&tag()));
        assert_eq!(file.body.raw(), MEGA_DRIVE_BODY);
    }

    /// The point of the opaque body: commands the OPL reader rejects outright
    /// pass through untouched.
    #[test]
    fn the_body_survives_commands_the_opl_reader_cannot_size() {
        assert!(
            crate::vgm::io::read("sonic.vgm", &mega_drive(true)).is_err(),
            "the OPL reader is expected to refuse this file"
        );
        let file = read("sonic.vgm", &mega_drive(true)).unwrap();
        assert_eq!(file.body.len(), MEGA_DRIVE_BODY.len());
    }

    #[test]
    fn an_unedited_file_round_trips_byte_for_byte() {
        for with_tag in [false, true] {
            let original = mega_drive(with_tag);
            let file = read("sonic.vgm", &original).unwrap();
            assert_eq!(write(&file).unwrap(), original, "with_tag {with_tag}");
        }
    }

    /// The real OPL2 capture goes through the foreign reader too -- it is a
    /// VGM like any other, and pack mode reaches for this path when the editor
    /// declines a file.
    #[test]
    fn the_opl2_fixture_round_trips_through_the_foreign_path() {
        let file = read("lsl3.vgm", VGM_FIXTURE).unwrap();
        assert!(file.is_opl_only());
        assert_eq!(file.chip_list(), "YM3812");
        assert_eq!(file.total_samples(), 118_320);
        assert_eq!(write(&file).unwrap(), VGM_FIXTURE);
    }

    #[test]
    fn retagging_rewrites_only_the_tag() {
        let original = mega_drive(true);
        let mut file = read("sonic.vgm", &original).unwrap();
        file.tag.as_mut().unwrap().notes = "Ripped by nobody".to_owned();

        let written = write(&file).unwrap();
        let reread = read("sonic.vgm", &written).unwrap();
        assert_eq!(reread.tag.unwrap().notes, "Ripped by nobody");
        assert_eq!(reread.body, file.body, "the music is untouched");
        // Past the EOF and GD3 pointers, which a longer tag legitimately moves,
        // every header byte is the one the file arrived with.
        let after_pointers = offset::GD3 + 4;
        assert_eq!(
            &written[after_pointers..file.header.data_start()],
            &original[after_pointers..file.header.data_start()],
            "and so is the rest of the header"
        );
    }

    #[test]
    fn adding_a_tag_to_an_untagged_file() {
        let mut file = read("sonic.vgm", &mega_drive(false)).unwrap();
        assert!(file.tag.is_none());
        file.tag = Some(tag());

        let written = write(&file).unwrap();
        let reread = read("sonic.vgm", &written).unwrap();
        assert_eq!(reread.tag.as_ref(), Some(&tag()));
        assert_eq!(reread.body, file.body);

        let gd3_at = reread.header.gd3_offset().unwrap();
        assert_eq!(&written[gd3_at..gd3_at + 4], GD3_MAGIC);
    }

    #[test]
    fn removing_a_tag_zeroes_its_offset() {
        let mut file = read("sonic.vgm", &mega_drive(true)).unwrap();
        file.tag = None;
        let written = write(&file).unwrap();
        assert_eq!(u32_at(&written, offset::GD3), 0);
        assert_eq!(written, mega_drive(false));
    }

    #[test]
    fn the_eof_field_matches_the_bytes_written() {
        let file = read("sonic.vgm", &mega_drive(true)).unwrap();
        let written = write(&file).unwrap();
        assert_eq!(
            u32_at(&written, offset::EOF) as usize + offset::EOF,
            written.len()
        );
    }

    #[test]
    fn a_vgz_reads_and_writes() {
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&mega_drive(true)).unwrap();
        let compressed = encoder.finish().unwrap();

        let file = read("sonic.vgz", &compressed).unwrap();
        assert_eq!(file.chip_list(), "SN76489, YM2612");

        let written = write_gzipped(&file).unwrap();
        assert!(is_gzipped(&written));
        let mut plain = Vec::new();
        GzDecoder::new(&written[..])
            .read_to_end(&mut plain)
            .unwrap();
        assert_eq!(plain, mega_drive(true));
    }

    /// Padding between the end-of-data marker and the tag lives in the body, so
    /// it survives rather than being silently dropped.
    #[test]
    fn padding_before_the_tag_is_kept_with_the_body() {
        let mut body = MEGA_DRIVE_BODY.to_vec();
        body.extend_from_slice(&[0u8; 16]);
        let original = build(0x161, 0x100, &body, true);

        let file = read("padded.vgm", &original).unwrap();
        assert_eq!(file.body.len(), body.len());
        assert_eq!(write(&file).unwrap(), original);
    }

    #[test]
    fn junk_after_the_declared_end_is_not_swallowed() {
        let mut bytes = mega_drive(false);
        let honest = bytes.len();
        bytes.extend_from_slice(b"trailing junk");

        let file = read("junk.vgm", &bytes).unwrap();
        assert_eq!(file.body.len(), honest - file.header.data_start());
        assert_eq!(write(&file).unwrap(), mega_drive(false));
    }

    #[test]
    fn a_truncated_file_still_opens_for_its_tags() {
        let mut bytes = mega_drive(false);
        bytes.truncate(bytes.len() - 4);
        let file = read("short.vgm", &bytes).unwrap();
        assert_eq!(file.chip_list(), "SN76489, YM2612");
        assert_eq!(
            file.body.raw(),
            &MEGA_DRIVE_BODY[..MEGA_DRIVE_BODY.len() - 4]
        );
    }

    #[test]
    fn a_minimal_header_with_data_at_0x60_opens() {
        // The shape the OPL reader rejects outright, and one of the two reader
        // TODOs this step closes for foreign files.
        let mut header = vec![0u8; 0x60];
        header[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut header, offset::VERSION, 0x151);
        put_u32(&mut header, offset::DATA_OFFSET, (0x60 - 0x34) as u32);
        put_u32(&mut header, ChipKind::Ym3812.clock_offset(), 3_579_545);
        let mut bytes = header;
        bytes.extend_from_slice(&[0x5A, 0x20, 0x01, 0x66]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let file = read("minimal.vgm", &bytes).unwrap();
        assert_eq!(file.header.data_start(), 0x60);
        assert_eq!(file.chip_list(), "YM3812");
        assert_eq!(write(&file).unwrap(), bytes);
    }

    // -- deleting commands --------------------------------------------------

    /// A file whose stream is easy to reason about: a write, a 10000-sample
    /// wait, a write, a 20000-sample wait, a write. Loop at index 2.
    fn trimmable() -> Vec<u8> {
        const STREAM: &[u8] = &[
            0x52, 0x28, 0xF0, // 0: YM2612 write
            0x61, 0x10, 0x27, // 1: wait 10000
            0x50, 0x9F, // 2: SN76489 write   <- the loop
            0x61, 0x20, 0x4E, // 3: wait 20000
            0x52, 0x28, 0x00, // 4: YM2612 write
            0x66, // end
        ];
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 30_000);
        // The loop starts at the third command, six bytes into the stream.
        put_u32(
            &mut bytes,
            offset::LOOP_OFFSET,
            (0x100 + 6 - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 20_000);
        bytes.extend_from_slice(STREAM);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);
        bytes
    }

    #[test]
    fn the_trimmable_fixture_reads_as_expected() {
        let file = read("t.vgm", &trimmable()).unwrap();
        assert_eq!(file.len(), 5);
        assert_eq!(file.loop_index(), Some(2));
        assert_eq!(file.stream().unwrap().total_samples(), 30_000);
        assert_eq!(write(&file).unwrap(), trimmable());
    }

    /// Deleting a command before the loop drags the loop's byte offset back
    /// with it, and shortens nothing.
    #[test]
    fn deleting_before_the_loop_moves_its_offset_but_not_its_length() {
        let mut file = read("t.vgm", &trimmable()).unwrap();
        file.delete_commands(&[0]); // the leading three-byte write

        assert_eq!(file.len(), 4);
        assert_eq!(file.loop_index(), Some(1));
        assert_eq!(
            file.header.loop_samples(),
            Some(20_000),
            "the loop is intact"
        );
        assert_eq!(file.header.total_samples(), 30_000, "no time was removed");
        assert_eq!(
            file.header.loop_offset(),
            Some(0x100 + 3),
            "three bytes earlier"
        );

        // And it survives the write: re-reading finds the same loop.
        let reread = read("t.vgm", &write(&file).unwrap()).unwrap();
        assert_eq!(reread.loop_index(), Some(1));
        assert_eq!(reread.header.loop_samples(), Some(20_000));
    }

    #[test]
    fn deleting_a_wait_inside_the_loop_shortens_the_song_and_the_loop() {
        let mut file = read("t.vgm", &trimmable()).unwrap();
        file.delete_commands(&[3]); // the 20000-sample wait, after the loop

        assert_eq!(file.loop_index(), Some(2), "unmoved");
        assert_eq!(file.header.total_samples(), 10_000);
        assert_eq!(file.header.loop_samples(), Some(0));
        assert_eq!(read("t.vgm", &write(&file).unwrap()).unwrap().len(), 4);
    }

    #[test]
    fn deleting_a_wait_before_the_loop_shortens_only_the_song() {
        let mut file = read("t.vgm", &trimmable()).unwrap();
        file.delete_commands(&[1]); // the 10000-sample wait
        assert_eq!(file.header.total_samples(), 20_000);
        assert_eq!(file.header.loop_samples(), Some(20_000));
        assert_eq!(file.loop_index(), Some(1));
    }

    #[test]
    fn deleting_from_the_loop_onward_clears_the_loop() {
        let mut file = read("t.vgm", &trimmable()).unwrap();
        file.delete_commands(&[2, 3, 4]);
        assert_eq!(file.loop_index(), None);
        assert_eq!(file.header.loop_offset(), None);
        assert_eq!(file.header.loop_samples(), None);
        assert_eq!(file.header.total_samples(), 10_000);
    }

    #[test]
    fn deleting_nothing_changes_nothing() {
        let mut file = read("t.vgm", &trimmable()).unwrap();
        assert!(!file.delete_commands(&[]));
        assert!(!file.delete_commands(&[99]), "out of range is ignored");
        assert_eq!(write(&file).unwrap(), trimmable(), "byte for byte");
    }

    /// Deleting a data block takes its whole payload -- it is one command, and
    /// the bytes after it must not be misread as commands of their own.
    #[test]
    fn deleting_a_data_block_removes_its_payload_too() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 8, 0, 0, 0]);
        bytes.extend_from_slice(&[0xAB; 8]);
        bytes.extend_from_slice(&[0x62, 0x66]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read("b.vgm", &bytes).unwrap();
        assert_eq!(file.len(), 2);
        file.delete_commands(&[0]);
        assert_eq!(file.len(), 1);
        assert_eq!(
            file.body.raw(),
            &[0x62, 0x66],
            "the block and all fifteen of its bytes are gone"
        );
    }

    #[test]
    fn a_delete_is_undoable_back_to_the_original_bytes() {
        use crate::undo::{DeleteCommands, UndoController};

        let original = trimmable();
        let mut file = read("t.vgm", &original).unwrap();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteCommands::new([0, 3])), &mut file);
        assert_eq!(file.len(), 3);
        assert_eq!(file.header.total_samples(), 10_000);

        undo.undo(&mut file);
        assert_eq!(file.len(), 5);
        assert_eq!(file.loop_index(), Some(2));
        assert_eq!(
            write(&file).unwrap(),
            original,
            "undo restores the file exactly, header included"
        );

        undo.redo(&mut file);
        assert_eq!(file.len(), 3);
        assert_eq!(file.header.total_samples(), 10_000);
    }

    /// The loop point itself being deleted loses information -- it slides onto
    /// its successor -- so undo must restore the captured header, not try to
    /// slide it back.
    #[test]
    fn undo_restores_a_loop_point_that_was_itself_deleted() {
        use crate::undo::{DeleteCommands, UndoController};

        let original = trimmable();
        let mut file = read("t.vgm", &original).unwrap();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteCommands::new([2])), &mut file);
        assert_eq!(file.loop_index(), Some(2), "slid onto the next command");

        undo.undo(&mut file);
        assert_eq!(write(&file).unwrap(), original);
    }

    // -- crop and delete-region (uv-3) --------------------------------------

    /// A Mega Drive stream with configuration up front and music after, so a
    /// crop has real state to carry across.
    ///
    /// ```text
    /// 0: YM2612 0x22 <- 0x08   LFO on          (configuration)
    /// 1: YM2612 0x27 <- 0x00   channel 3 mode  (configuration)
    /// 2: SN76489 <- 0x9F       PSG volume      (configuration)
    /// 3: wait 10000
    /// 4: YM2612 0x28 <- 0xF0   key on          (music)
    /// 5: wait 20000
    /// 6: YM2612 0x28 <- 0x00   key off
    /// 7: wait 735
    /// ```
    fn configured() -> Vec<u8> {
        const STREAM: &[u8] = &[
            0x52, 0x22, 0x08, //
            0x52, 0x27, 0x00, //
            0x50, 0x9F, //
            0x61, 0x10, 0x27, //
            0x52, 0x28, 0xF0, //
            0x61, 0x20, 0x4E, //
            0x52, 0x28, 0x00, //
            0x62, //
            0x66,
        ];
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut bytes, ChipKind::Sn76489.clock_offset(), 3_579_545);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 30_735);
        bytes.extend_from_slice(STREAM);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);
        bytes
    }

    /// The state a stream leaves the chips in, for comparing before and after.
    fn state_of(file: &VgmFile) -> Vec<(String, u16)> {
        let stream = file.stream().unwrap();
        let mut cells: Vec<(String, u16)> = Vec::new();
        for index in 0..stream.len() {
            if let Some(crate::vgm::VgmCommand::Write { target, addr, data }) = stream.get(index) {
                let key = format!("{} {addr:#06X}", target.label());
                match cells.iter_mut().find(|(name, _)| *name == key) {
                    Some(cell) => cell.1 = data,
                    None => cells.push((key, data)),
                }
            }
        }
        cells.sort();
        cells
    }

    /// The hard requirement: cropping a VGM for chips this app has no core for.
    /// The music that survives must meet the chips configured as they were.
    #[test]
    fn cropping_a_non_opl_vgm_carries_the_configuration_across() {
        let original = read("md.vgm", &configured()).unwrap();
        assert!(!original.is_opl(), "no OPL, and no core for these chips");
        let full_state = state_of(&original);

        let mut cropped = original.clone();
        // Keep the music (rows 4..8), throwing away the configuration.
        let report = cropped.crop_to_region(4, 8).expect("a real region");
        assert_eq!(report.unmodelled, 0);
        assert_eq!(report.restored, 3, "two YM2612 registers and the PSG");

        // Every register the discarded head had set is set again.
        assert_eq!(
            state_of(&cropped),
            full_state,
            "the chips end up where they were"
        );
        // And the kept music is still there, in order.
        let stream = cropped.stream().unwrap();
        assert_eq!(stream.len(), 3 + 4, "the restore, then the four kept rows");
        assert_eq!(stream.describe(3), "YM2612 0x0028 <- 0xF0");
        assert_eq!(stream.describe(4), "wait 20000");
        assert_eq!(
            cropped.header.total_samples(),
            20_735,
            "the header follows what is left"
        );
    }

    /// A restore never invents a write: the bytes are the source's own, so the
    /// cropped file is still a valid VGM that reads back identically.
    #[test]
    fn a_cropped_file_round_trips() {
        let mut file = read("md.vgm", &configured()).unwrap();
        file.crop_to_region(4, 8);
        let written = write(&file).unwrap();
        let reread = read("md.vgm", &written).unwrap();
        assert_eq!(reread.body, file.body);
        assert_eq!(reread.header.total_samples(), 20_735);
        assert_eq!(write(&reread).unwrap(), written);
    }

    /// Cutting the middle out: what follows the seam must meet the chips in the
    /// state the removed span would have left them.
    #[test]
    fn deleting_a_region_bridges_the_seam_with_the_state_it_changed() {
        let original = read("md.vgm", &configured()).unwrap();
        let full_state = state_of(&original);

        let mut edited = original.clone();
        // Remove the configuration and the first note (rows 0..5).
        let report = edited.delete_region(0, 5).expect("a real region");
        // The two YM2612 configuration registers, the PSG, and the key-on the
        // span ended on -- which re-attacks, as it does in `vgm_trim`.
        assert_eq!(report.restored, 4);

        assert_eq!(
            state_of(&edited),
            full_state,
            "the chips still reach the same state"
        );
        assert_eq!(edited.header.total_samples(), 20_735);
    }

    /// Only what *changed* crosses the seam. A span that rewrote a register
    /// with the value it already held contributes nothing.
    #[test]
    fn a_deleted_region_that_changed_nothing_adds_nothing() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        bytes.extend_from_slice(&[
            0x52, 0x28, 0x01, // 0
            0x62, // 1
            0x52, 0x28, 0x01, // 2: the same value again
            0x62, // 3
            0x52, 0x30, 0x71, // 4
            0x66,
        ]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read("x.vgm", &bytes).unwrap();
        file.delete_region(2, 4).unwrap();
        let stream = file.stream().unwrap();
        assert_eq!(stream.len(), 3, "rows 0 and 1, then row 4 -- no patch");
        assert_eq!(stream.describe(2), "YM2612 0x0030 <- 0x71");
    }

    /// A data block loaded before the crop point comes back: the banks are
    /// cumulative, so the music after the cut still indexes it.
    #[test]
    fn cropping_keeps_a_data_block_the_discarded_head_loaded() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 4, 0, 0, 0, 1, 2, 3, 4]); // 0
        bytes.extend_from_slice(&[0x62]); // 1
        bytes.extend_from_slice(&[0x80]); // 2: a DAC write, which reads the bank
        bytes.extend_from_slice(&[0x62]); // 3
        bytes.push(0x66);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read("dac.vgm", &bytes).unwrap();
        file.crop_to_region(2, 4).unwrap();
        let stream = file.stream().unwrap();
        assert_eq!(
            stream.describe(0),
            "data block 0x00 (uncompressed stream), 4 bytes",
            "the block is re-emitted first"
        );
        assert_eq!(stream.len(), 3, "the block, the DAC write, the wait");
    }

    #[test]
    fn a_loop_inside_a_cropped_region_moves_with_it() {
        let mut bytes = configured();
        // Loop at row 6 (the key-off), inside the region kept below.
        let source = read("md.vgm", &bytes).unwrap();
        let at = source.header.data_start() + source.stream().unwrap().byte_offset(6).unwrap();
        put_u32(
            &mut bytes,
            offset::LOOP_OFFSET,
            (at - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 735);

        let mut file = read("md.vgm", &bytes).unwrap();
        assert_eq!(file.loop_index(), Some(6));
        file.crop_to_region(4, 8).unwrap();
        // Three restored rows, then old 4,5,6 -> the loop lands on row 5.
        assert_eq!(file.loop_index(), Some(5));
        assert_eq!(file.header.loop_samples(), Some(735));
    }

    #[test]
    fn a_loop_outside_a_cropped_region_is_dropped() {
        let mut bytes = configured();
        let source = read("md.vgm", &bytes).unwrap();
        let at = source.header.data_start() + source.stream().unwrap().byte_offset(0).unwrap();
        put_u32(
            &mut bytes,
            offset::LOOP_OFFSET,
            (at - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 30_735);

        let mut file = read("md.vgm", &bytes).unwrap();
        file.crop_to_region(4, 8).unwrap();
        assert_eq!(file.loop_index(), None, "it pointed into what was cut");
        assert_eq!(file.header.loop_offset(), None);
    }

    #[test]
    fn a_loop_after_a_deleted_region_slides_to_the_new_seam() {
        let mut bytes = configured();
        let source = read("md.vgm", &bytes).unwrap();
        let at = source.header.data_start() + source.stream().unwrap().byte_offset(6).unwrap();
        put_u32(
            &mut bytes,
            offset::LOOP_OFFSET,
            (at - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 735);

        let mut file = read("md.vgm", &bytes).unwrap();
        file.delete_region(3, 5).unwrap(); // the first wait and the key-on
        assert_eq!(
            file.stream().unwrap().describe(file.loop_index().unwrap()),
            "YM2612 0x0028 <- 0x00",
            "the loop still points at the key-off it pointed at"
        );
    }

    #[test]
    fn an_empty_or_backwards_region_does_nothing() {
        let mut file = read("md.vgm", &configured()).unwrap();
        assert!(file.crop_to_region(4, 4).is_none());
        assert!(file.crop_to_region(5, 2).is_none());
        assert!(file.delete_region(4, 4).is_none());
        assert_eq!(write(&file).unwrap(), configured(), "byte for byte");
    }

    /// A crop past a command whose state cannot be replayed says so, rather
    /// than refusing the edit or pretending it restored everything.
    #[test]
    fn a_crop_past_an_unmodelled_command_reports_it() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        bytes.extend_from_slice(&[0x68, 0x66, 0x01, 1, 2, 3, 4, 5, 6, 7, 8, 9]); // 0
        bytes.extend_from_slice(&[0x52, 0x28, 0x01]); // 1
        bytes.extend_from_slice(&[0x62, 0x62]); // 2, 3
        bytes.push(0x66);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read("ram.vgm", &bytes).unwrap();
        let report = file.crop_to_region(2, 4).unwrap();
        assert_eq!(report.unmodelled, 1, "the PCM RAM write cannot be replayed");
        assert_eq!(report.restored, 1, "the register write can");
    }

    /// The same path serves an OPL VGM -- there is one crop, not an OPL one and
    /// another. The projection survives it, so the file is still an OPL file.
    #[test]
    fn cropping_an_opl_vgm_keeps_it_an_opl_vgm() {
        let original = read("lsl3.vgm", VGM_FIXTURE).unwrap();
        assert!(original.is_opl());
        let full_state = state_of(&original);
        let rows = original.len();

        let mut file = original.clone();
        file.crop_to_region(10, rows).unwrap();
        assert!(file.is_opl(), "still OPL after the crop");
        assert!(file.to_song().is_some(), "and still materialises a song");
        assert_eq!(
            state_of(&file),
            full_state,
            "the registers the head had set are set again"
        );
        // The head is gone, so the song is shorter by exactly its waits -- even
        // though the restore put a comparable number of *rows* back.
        let head_samples = original.stream().unwrap().total_samples()
            - original.stream().unwrap().samples_from(10);
        assert_eq!(
            u64::from(file.header.total_samples()),
            u64::from(original.header.total_samples()) - head_samples
        );
    }

    // -- a tag stored before the data ---------------------------------------

    /// Builds the awkward shape: header fields, then the GD3, then the data.
    fn tag_before_data() -> Vec<u8> {
        let gd3 = write_gd3_tag(&tag());
        let fields = 0x100;
        let data_at = fields + gd3.len();

        let mut out = vec![0u8; fields];
        out[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut out, offset::VERSION, 0x161);
        put_u32(
            &mut out,
            offset::DATA_OFFSET,
            (data_at - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut out, offset::GD3, (fields - offset::GD3) as u32);
        put_u32(&mut out, ChipKind::Ym2612.clock_offset(), 7_670_454);
        // A loop pointing at the wait, three bytes into the data.
        put_u32(
            &mut out,
            offset::LOOP_OFFSET,
            (data_at + 5 - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut out, offset::LOOP_NUM_SAMPLES, 10_000);

        out.extend_from_slice(&gd3);
        out.extend_from_slice(MEGA_DRIVE_BODY);
        let eof = out.len();
        put_u32(&mut out, offset::EOF, (eof - offset::EOF) as u32);
        out
    }

    #[test]
    fn a_tag_before_the_data_is_read() {
        let file = read("odd.vgm", &tag_before_data()).unwrap();
        assert_eq!(file.tag.as_ref(), Some(&tag()));
        assert_eq!(
            file.body.raw(),
            MEGA_DRIVE_BODY,
            "the tag is not in the body"
        );
    }

    /// Writing moves the tag to the end and slides everything that pointed
    /// past it, so the file becomes conventional without losing its loop.
    #[test]
    fn writing_relocates_a_tag_stored_before_the_data() {
        let original = tag_before_data();
        let file = read("odd.vgm", &original).unwrap();
        let written = write(&file).unwrap();

        let reread = read("odd.vgm", &written).unwrap();
        assert_eq!(reread.tag.as_ref(), Some(&tag()));
        assert_eq!(reread.body.raw(), MEGA_DRIVE_BODY);
        assert_eq!(
            reread.header.data_start(),
            0x100,
            "the tag's bytes are gone"
        );
        assert_eq!(reread.header.chip_list(), "YM2612");

        // The loop still points at the same command, five bytes into the data.
        assert_eq!(reread.header.loop_offset(), Some(0x100 + 5));
        assert_eq!(reread.header.loop_samples(), Some(10_000));
        assert!(
            reread.header.gd3_offset().unwrap() > reread.header.data_start(),
            "the tag is at the end now"
        );
        assert_eq!(written.len(), original.len(), "nothing gained or lost");

        // And once relocated, it round-trips like any other file.
        assert_eq!(write(&reread).unwrap(), written);
    }

    /// A GD3 pointer landing among the header's own fields is corrupt, not
    /// unusual: the file still opens, without a tag.
    #[test]
    fn a_gd3_pointer_inside_the_header_fields_is_ignored() {
        let mut bytes = mega_drive(false);
        put_u32(&mut bytes, offset::GD3, (0x40 - offset::GD3) as u32);
        let file = read("bad.vgm", &bytes).unwrap();
        assert!(file.tag.is_none());
        assert_eq!(file.chip_list(), "SN76489, YM2612");
    }

    // -- rejections ---------------------------------------------------------

    #[test]
    fn rejects_a_bad_magic() {
        let mut bytes = mega_drive(false);
        bytes[0] = b'X';
        assert!(read("bad.vgm", &bytes).is_err());
    }

    #[test]
    fn rejects_a_malformed_tag() {
        let mut bytes = mega_drive(true);
        let gd3_at = read("sonic.vgm", &bytes)
            .unwrap()
            .header
            .gd3_offset()
            .unwrap();
        bytes[gd3_at] = b'X';
        assert!(read("sonic.vgm", &bytes).is_err());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::vgm::header::{CHIP_COUNT, ChipKind};
    use proptest::prelude::*;

    /// Assembles a plausible VGM from parts: any version, any header size, any
    /// set of chips, any body.
    fn synthetic(
        version: u32,
        header_size: usize,
        clocks: Vec<(usize, u32)>,
        body: Vec<u8>,
        tag: Option<Gd3Tag>,
    ) -> Vec<u8> {
        let mut header = vec![0u8; header_size];
        header[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut header, offset::VERSION, version);
        put_u32(
            &mut header,
            offset::DATA_OFFSET,
            (header_size - offset::DATA_OFFSET) as u32,
        );
        for (chip, clock) in clocks {
            let at = ChipKind::all()
                .nth(chip)
                .expect("a chip index")
                .clock_offset();
            if at + 4 <= header_size {
                put_u32(&mut header, at, clock);
            }
        }

        let mut out = header;
        out.extend_from_slice(&body);
        if let Some(tag) = &tag {
            let gd3_at = out.len();
            put_u32(&mut out, offset::GD3, (gd3_at - offset::GD3) as u32);
            out.extend_from_slice(&write_gd3_tag(tag));
        }
        let eof = out.len();
        put_u32(&mut out, offset::EOF, (eof - offset::EOF) as u32);
        out
    }

    proptest! {
        /// The load-bearing property: whatever a file declares, reading it and
        /// writing it back reproduces it exactly. Anything else would make
        /// retagging a pack destructive.
        #[test]
        fn any_synthetic_file_round_trips_byte_for_byte(
            version in prop::sample::select(vec![0x100u32, 0x101, 0x110, 0x150, 0x151, 0x160, 0x161, 0x170, 0x171, 0x172]),
            header_size in prop::sample::select(vec![0x40usize, 0x60, 0x80, 0xC0, 0x100]),
            clocks in prop::collection::vec((0..CHIP_COUNT, 1u32..100_000_000), 0..6),
            body in prop::collection::vec(any::<u8>(), 0..64),
            has_tag in any::<bool>(),
        ) {
            let tag = has_tag.then(|| Gd3Tag {
                track_name_en: "t".to_owned(),
                ..Gd3Tag::default()
            });
            let bytes = synthetic(version, header_size, clocks, body, tag);
            let file = read("p.vgm", &bytes)?;
            prop_assert_eq!(write(&file)?, bytes);
        }

        /// Every chip a file declares must come back out, with its clock intact
        /// and its flag bits read apart from it.
        #[test]
        fn declared_chips_survive_the_read(
            clocks in prop::collection::vec((0..CHIP_COUNT, 1u32..0x3FFF_FFFF), 0..8),
        ) {
            let bytes = synthetic(0x172, 0x100, clocks.clone(), vec![0x66], None);
            let file = read("p.vgm", &bytes)?;

            // Two entries for the same chip write the same field, so the later
            // one is what the file ends up declaring.
            let expected: Vec<(usize, u32)> = clocks
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect();

            let found: Vec<(usize, u32)> = file
                .header
                .chips()
                .iter()
                .map(|chip| (chip.kind as usize, chip.clock))
                .collect();
            prop_assert_eq!(found, expected);
        }
    }
}
