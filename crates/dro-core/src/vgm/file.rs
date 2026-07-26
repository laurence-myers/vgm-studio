//! A VGM file of any kind, held for its metadata.
//!
//! [`Song`](crate::Song) is the OPL editing model, and stays that way: it
//! decodes every command into a register write or a delay, which only an OPL
//! stream can promise. A file for a chip this app cannot decode still has a
//! header, a GD3 tag, a duration and a loop, and all of that is editable
//! without understanding a single command -- so it gets its own type, whose
//! body is a span of bytes carried from read to write untouched.
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

use crate::error::{Error, Result};
use crate::vgm::data::Gd3Tag;
use crate::vgm::header::{LEGACY_DATA_START, VgmHeader, offset};
use crate::vgm::io::{is_gzipped, parse_gd3_tag, write_gd3_tag};
use crate::vgm::stream::VgmStream;

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

/// A VGM file for any chip, with its tags editable and its music left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmFile {
    /// The file's own name, as it was opened. Renaming is the caller's to do.
    pub name: String,
    pub header: VgmHeader,
    pub body: VgmBody,
    pub tag: Option<Gd3Tag>,
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

    Ok(VgmFile {
        name: name.to_owned(),
        header,
        body,
        tag,
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
