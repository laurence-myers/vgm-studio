//! The VGM container and its gzipped form, VGZ.
//!
//! # Byte-exact round trips
//!
//! The header is kept verbatim and only the fields that can have changed are
//! patched, so an unedited round trip reproduces the file exactly -- including
//! chip clocks, `rate`, and any v1.70 extra header.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::error::{Error, Result};
use crate::io::ByteReader;
use crate::song::{OplType, Song, SongData};
use crate::vgm::data::{GD3_FIELD_COUNT, Gd3Tag, VgmData, VgmMeta, command};
use crate::vgm::header::{ChipKind, VgmHeader, offset};

/// `Vgm `.
pub const MAGIC: &[u8; 4] = b"Vgm ";
/// `Gd3 `.
pub const GD3_MAGIC: &[u8; 4] = b"Gd3 ";

/// v1.51 is the first version with the OPL chip clock fields this app needs.
pub const MINIMUM_SUPPORTED_VERSION: u32 = 0x0000_0151;
/// The version [`Song::to_vgm`](crate::convert::dro_to_vgm) emits.
pub const CONVERSION_VERSION: u32 = 0x0000_0151;
const GD3_SUPPORTED_VERSION: u32 = 0x0000_0100;
const GD3_ENCODING_UNITS: usize = 2;

/// The chip clocks, and the flag that marks a second chip.
///
/// The field offsets these are written to live in [`header::offset`], the one
/// table both readers share.
mod clock {
    pub(super) const OPL2: u32 = 3_579_545;
    /// The spec says the high bits should be `0x4000_0000`, but `dro2vgm` writes
    /// `0xC000_0000`, and files in the wild follow it.
    pub(super) const DUAL_OPL2: u32 = 3_579_545 | 0xC000_0000;
    pub(super) const OPL3: u32 = 14_318_180;
}

/// A full v1.51 header runs to 0x7F. Files that declare only the fields they
/// use can be shorter; this is the size at which the volume and loop modifiers
/// exist, and the size a synthesised header gets.
const MINIMUM_HEADER_SIZE: usize = 0x80;

/// The end of the YMF262 clock field: the least a header can be and still
/// declare an OPL chip at all, and so the least this writer can patch.
const OPL_CLOCKS_END: usize = offset::YMF262_CLOCK + 4;

/// The header a converted song gets: exactly the v1.51 header size, which is
/// what `dro2vgm` emits and what `tests/lsl3_score_up.vgm` has.
///
/// A wider header would carry zero-padded fields for later VGM versions that
/// nothing writes yet, and that padding is what stops it round-tripping -- so a
/// converted song gets the tight header for now.
const SYNTHESISED_HEADER_SIZE: usize = MINIMUM_HEADER_SIZE;
/// `dro2vgm` writes this in the `rate` field.
const SYNTHESISED_RATE: u32 = 1000;

/// Gzip's magic. VGZ is just a gzipped VGM.
const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];

/// Whether `bytes` is gzipped, and therefore a `.vgz`.
#[must_use]
pub fn is_gzipped(bytes: &[u8]) -> bool {
    bytes.starts_with(&GZIP_MAGIC)
}

/// Parses a VGM or VGZ file.
///
/// # Errors
/// If the gzip stream is corrupt, the magic or version is wrong, no OPL chip is
/// declared, or a command in the data stream is unrecognised.
pub fn read(name: &str, bytes: &[u8]) -> Result<Song> {
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

/// Serialises a song to VGM bytes.
///
/// # Errors
/// If `song` is not a VGM song, or its OPL type has no VGM chip clock.
pub fn write(song: &Song) -> Result<Vec<u8>> {
    let (SongData::Vgm(stream), Some(meta)) = (song.data(), song.vgm_meta()) else {
        return Err(Error::file("Tried to write a DRO song as a VGM"));
    };

    let gd3 = meta.tag.as_ref().map(write_gd3_tag);
    let gd3_size = gd3.as_ref().map_or(0, Vec::len);

    let mut out = meta.header().to_vec();
    // The fixed-offset field writes below would panic on a header too short to
    // hold them. Unreachable with a header that passed the reader's own check --
    // an OPL clock at 0x5C implies at least this much -- but guard it so a
    // malformed one errors rather than panics.
    if out.len() < OPL_CLOCKS_END {
        return Err(Error::file(format!(
            "VGM header is {} bytes; the writer needs at least {OPL_CLOCKS_END:#X}",
            out.len()
        )));
    }
    let data_offset = out.len();
    let data = stream.raw();
    let end_marker_size = 1;
    let eof = data_offset + data.len() + end_marker_size + gd3_size;

    // The loop point lives as an instruction index, so the byte offset the header
    // records is recomputed here rather than carried across edits, and the loop
    // length is derived from what is actually left. Both fields are zero when the
    // song does not loop, as the spec requires.
    let (loop_offset, loop_num_samples) = match meta.loop_point {
        Some(index) => {
            let absolute = data_offset + stream.byte_offset(index);
            (
                (absolute - offset::LOOP_OFFSET) as u32,
                song.loop_num_samples().unwrap_or(0),
            )
        }
        None => (0, 0),
    };

    put_u32(&mut out, offset::EOF, (eof - offset::EOF) as u32);
    put_u32(
        &mut out,
        offset::GD3,
        if gd3_size == 0 {
            0
        } else {
            (eof - gd3_size - offset::GD3) as u32
        },
    );
    put_u32(&mut out, offset::TOTAL_SAMPLES, song.total_delay_samples());
    put_u32(&mut out, offset::LOOP_OFFSET, loop_offset);
    put_u32(&mut out, offset::LOOP_NUM_SAMPLES, loop_num_samples);
    put_chip_clocks(&mut out, song.opl_type)?;
    // The volume and loop modifiers are v1.51/v1.60 additions. A header that
    // stops before them does not have them, and cannot grow them here without
    // moving the data -- their values are zero for such a file anyway.
    if out.len() >= MINIMUM_HEADER_SIZE {
        out[offset::VOLUME_MODIFIER] = meta.volume_modifier;
        out[offset::LOOP_BASE] = meta.loop_base;
        out[offset::LOOP_MODIFIER] = meta.loop_modifier;
    }

    out.extend_from_slice(data);
    out.push(command::END);
    if let Some(gd3) = gd3 {
        out.extend_from_slice(&gd3);
    }
    debug_assert_eq!(out.len(), eof);
    Ok(out)
}

/// Serialises a song to gzipped VGZ bytes.
///
/// # Errors
/// As [`write`], plus any compression failure.
pub fn write_gzipped(song: &Song) -> Result<Vec<u8>> {
    let plain = write(song)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&plain)
        .and_then(|()| encoder.finish())
        .map_err(|error| Error::file(format!("Could not compress the VGZ file: {error}")))
}

/// The header a freshly converted song gets: v1.51, `rate` 1000, data at 0x80.
///
/// Matches what `dro2vgm` emits, which is what `tests/lsl3_score_up.vgm` is.
#[must_use]
pub fn synthesise_header() -> Vec<u8> {
    let mut header = vec![0u8; SYNTHESISED_HEADER_SIZE];
    header[offset::MAGIC..offset::MAGIC + 4].copy_from_slice(MAGIC);
    put_u32(&mut header, offset::VERSION, CONVERSION_VERSION);
    put_u32(&mut header, offset::RATE, SYNTHESISED_RATE);
    put_u32(
        &mut header,
        offset::DATA_OFFSET,
        (SYNTHESISED_HEADER_SIZE - offset::DATA_OFFSET) as u32,
    );
    header
}

// ---------------------------------------------------------------------------

fn read_uncompressed(name: &str, bytes: &[u8]) -> Result<Song> {
    // The header model does the field reading: the header ends at the data, so
    // a minimal rip putting its data at 0x60 has its OPL clock inside a 0x60-byte
    // header and is perfectly readable.
    let parsed = VgmHeader::parse(bytes)?;
    if parsed.version() < MINIMUM_SUPPORTED_VERSION {
        return Err(Error::file(
            "Unsupported VGM version, v1.51 is the minimum supported version.".to_owned(),
        ));
    }

    let opl_type = opl_type_of(&parsed)?;
    let data_offset = parsed.data_start();
    let header = parsed.raw().to_vec();
    let header_total_samples = parsed.total_samples();
    let loop_num_samples = parsed.loop_samples().unwrap_or(0);

    let data = VgmData::read_from_stream(&bytes[data_offset..])?;
    let loop_point = resolve_loop_point(parsed.loop_offset(), data_offset, &data);

    let tag = parsed
        .gd3_offset()
        .map(|offset| parse_gd3_tag(bytes, offset))
        .transpose()?;

    let mut song = Song::vgm(
        name.to_owned(),
        parsed.version(),
        data,
        opl_type,
        VgmMeta {
            loop_point,
            // Resolved below: it needs the assembled song's delay prefix.
            loop_end: None,
            loop_base: parsed.loop_base(),
            loop_modifier: parsed.loop_modifier(),
            volume_modifier: parsed.volume_modifier(),
            tag,
            header,
        },
    );

    if song.total_delay_samples() != header_total_samples {
        log::warn!(
            "VGM header claims {header_total_samples} samples, but the command stream sums to \
             {}; trusting the stream",
            song.total_delay_samples()
        );
    }
    if let Some(loop_point) = loop_point
        && let Some(end) = resolve_loop_end(&song, loop_point, loop_num_samples)
    {
        song.vgm_meta_mut()
            .expect("the loop point came from this song's own VGM metadata")
            .loop_end = Some(end);
    }
    Ok(song)
}

/// Which OPL a header's chip clocks declare.
///
/// The dual-OPL2 marker is the second-chip bit on the YM3812 clock, which
/// `dro2vgm` writes alongside a meaningless bit 31 -- the header model reads the
/// two apart, so only the one that means something is consulted here.
fn opl_type_of(header: &VgmHeader) -> Result<OplType> {
    let chip = |kind| header.chips().iter().find(|chip| chip.kind == kind);
    match (chip(ChipKind::Ym3812), chip(ChipKind::Ymf262)) {
        (Some(ym3812), _) if ym3812.dual => Ok(OplType::DualOpl2),
        (Some(_), _) => Ok(OplType::Opl2),
        (None, Some(_)) => Ok(OplType::Opl3),
        (None, None) => Err(Error::file("No OPL2 or OPL3 data detected.")),
    }
}

/// Turns the header's loop offset into the index of the command it points at.
///
/// No offset means "no loop", per the spec. Anything that lands inside the
/// header, past the stream, or in the middle of a command is a corrupt loop
/// pointer: warn and drop the loop rather than write it back to point somewhere
/// meaningless.
fn resolve_loop_point(
    absolute: Option<usize>,
    data_offset: usize,
    data: &VgmData,
) -> Option<usize> {
    let absolute = absolute?;
    let Some(byte_in_stream) = absolute.checked_sub(data_offset) else {
        log::warn!(
            "VGM loop point at {absolute:#X} is inside the header (data starts at \
             {data_offset:#X}); ignoring the loop"
        );
        return None;
    };
    match data.index_at_byte_offset(byte_in_stream) {
        Some(index) => Some(index),
        None => {
            log::warn!(
                "VGM loop point at {absolute:#X} does not fall on a command boundary; \
                 ignoring the loop"
            );
            None
        }
    }
}

/// Turns the header's `loop # samples` field into an exclusive end index, or
/// `None` for "the loop runs to the end of the song".
///
/// The spec defines the field as the wait total from the loop point to the end of
/// the file, and that is what a `None` writes back. A *shorter* value landing
/// exactly on a command boundary is how this editor records a loop that stops
/// short of the tail ([`VgmMeta::loop_end`]), so it is materialised rather than
/// discarded -- without this, re-saving such a file would silently widen its loop
/// to the whole tail. Anything else (longer than what actually follows the loop
/// point, or falling inside a delay) is a stale or corrupt header: warn, and let
/// the derived to-the-end length stand.
fn resolve_loop_end(song: &Song, loop_point: usize, header_samples: u32) -> Option<usize> {
    let prefix = song.delay_samples_prefix();
    let start = prefix[loop_point];
    let to_end = prefix[song.len()].saturating_sub(start);
    if header_samples == to_end {
        return None;
    }
    if header_samples > to_end {
        log::warn!(
            "VGM header claims a loop of {header_samples} samples, but only {to_end} follow the \
             loop point; trusting the stream"
        );
        return None;
    }

    // Strictly shorter: find the command starting exactly `header_samples` after
    // the loop point. Zero-delay commands share a timestamp, so this lands on the
    // first of them -- the loop then covers everything sounding before that
    // instant, and writing it back produces the same length, so a re-read
    // normalises to this same index.
    let target = start + header_samples;
    let end = loop_point + prefix[loop_point..].partition_point(|&samples| samples < target);
    if end <= loop_point || prefix.get(end) != Some(&target) {
        log::warn!(
            "VGM header's {header_samples}-sample loop does not end on a command boundary; \
             looping to the end of the stream instead"
        );
        return None;
    }
    Some(end)
}

pub(crate) fn parse_gd3_tag(bytes: &[u8], offset: usize) -> Result<Gd3Tag> {
    let mut reader = ByteReader::new(bytes);
    reader.seek(offset)?;

    let magic = reader.take(4)?;
    if magic != GD3_MAGIC {
        return Err(Error::file(format!(
            "Does not appear to be a GD3 tag (invalid header. Expected {}, found {}).",
            String::from_utf8_lossy(GD3_MAGIC),
            String::from_utf8_lossy(magic),
        )));
    }
    if reader.u32_le()? != GD3_SUPPORTED_VERSION {
        return Err(Error::file(
            "Unsupported GD3 version, only v1.00 is supported.".to_owned(),
        ));
    }
    let data_length = reader.u32_le()? as usize;
    let blob = reader.take(data_length)?;
    if blob.len() % GD3_ENCODING_UNITS != 0 {
        return Err(Error::file(
            "GD3 tag length is not a whole number of UTF-16 code units.".to_owned(),
        ));
    }

    // Eleven null-terminated UTF-16LE strings.
    let units: Vec<u16> = blob
        .chunks_exact(GD3_ENCODING_UNITS)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();

    let mut fields: Vec<String> = units
        .split(|&unit| unit == 0)
        .map(String::from_utf16_lossy)
        .collect();
    // Splitting on the terminators leaves one trailing empty string.
    if fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    // Some real encoders append further empty fields (a doubled final terminator,
    // seen on the Doom rips); drop those extras rather than reject an otherwise
    // valid eleven-field tag. A tag that is genuinely short is still an error.
    while fields.len() > GD3_FIELD_COUNT && fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    let count = fields.len();
    let fields: [String; GD3_FIELD_COUNT] = fields.try_into().map_err(|_| {
        Error::file(format!(
            "GD3 tag has {count} fields, expected {GD3_FIELD_COUNT}"
        ))
    })?;
    Ok(Gd3Tag::from_fields(fields))
}

pub(crate) fn write_gd3_tag(tag: &Gd3Tag) -> Vec<u8> {
    let mut blob = Vec::new();
    for field in tag.fields() {
        for unit in field.encode_utf16() {
            blob.extend_from_slice(&unit.to_le_bytes());
        }
        blob.extend_from_slice(&0u16.to_le_bytes()); // null terminator
    }

    let mut out = Vec::with_capacity(12 + blob.len());
    out.extend_from_slice(GD3_MAGIC);
    out.extend_from_slice(&GD3_SUPPORTED_VERSION.to_le_bytes());
    out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
    out.extend_from_slice(&blob);
    out
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Writes the chip clocks that declare which OPL a file targets.
///
/// # Errors
/// If `header` is too short to hold them.
pub(crate) fn put_chip_clocks(header: &mut [u8], opl_type: OplType) -> Result<()> {
    if header.len() < OPL_CLOCKS_END {
        return Err(Error::file(format!(
            "VGM header is {} bytes; the OPL clocks need at least {OPL_CLOCKS_END}",
            header.len()
        )));
    }
    let (ym3812, ymf262) = match opl_type {
        OplType::Opl2 => (clock::OPL2, 0),
        OplType::DualOpl2 => (clock::DUAL_OPL2, 0),
        OplType::Opl3 => (0, clock::OPL3),
    };
    put_u32(header, offset::YM3812_CLOCK, ym3812);
    put_u32(header, offset::YMF262_CLOCK, ymf262);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Bank, DelayKind, Instruction};
    use crate::undo::{DeleteInstructions, UndoController};

    const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn read_the_fixture() {
        let song = read("lsl3_score_up.vgm", VGM_FIXTURE).unwrap();
        assert_eq!(song.name, "lsl3_score_up.vgm");
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.file_version, 0x151);
        assert_eq!(song.total_delay_samples(), 118_320);
        assert_eq!(song.len(), 299);
        assert_eq!(
            &song.data().raw()[..6],
            [0x5A, 0x01, 0x20, 0x5A, 0x20, 0x31]
        );
        assert_eq!(
            song.instruction(0).unwrap(),
            Instruction::Register {
                reg: 0x01,
                value: 0x20,
                bank: Some(Bank::Low)
            }
        );
        assert!(
            song.vgm_meta().unwrap().tag.is_none(),
            "the fixture has no GD3"
        );
    }

    #[test]
    fn the_fixtures_header_is_preserved_verbatim() {
        let song = read("f.vgm", VGM_FIXTURE).unwrap();
        let header = song.vgm_meta().unwrap().header();
        assert_eq!(header.len(), 0x80, "data starts at 0x80");
        assert_eq!(header, &VGM_FIXTURE[..0x80]);
    }

    /// The load-bearing test: an unedited file must round-trip byte for byte.
    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let song = read("f.vgm", VGM_FIXTURE).unwrap();
        let written = write(&song).unwrap();
        assert_eq!(written.len(), VGM_FIXTURE.len());
        assert_eq!(written, VGM_FIXTURE);
    }

    #[test]
    fn ms_length_is_derived_from_the_samples() {
        let song = read("f.vgm", VGM_FIXTURE).unwrap();
        // 118320 samples / 44.1 = 2683.0 ms, the DRO fixture's length.
        assert_eq!(song.ms_length, 2683);
        assert_eq!(song.ms_length, song.total_delay_ms());
    }

    #[test]
    fn vgz_reads_and_writes() {
        let song = read("f.vgz", &gzip(VGM_FIXTURE)).unwrap();
        assert_eq!(song.total_delay_samples(), 118_320);
        assert_eq!(song.len(), 299);

        let written = write_gzipped(&song).unwrap();
        assert!(is_gzipped(&written));
        let reread = read("f.vgz", &written).unwrap();
        assert_eq!(reread.data(), song.data());
        assert_eq!(reread.vgm_meta(), song.vgm_meta());

        // Decompressed, it is still the original file.
        let mut plain = Vec::new();
        GzDecoder::new(&written[..])
            .read_to_end(&mut plain)
            .unwrap();
        assert_eq!(plain, VGM_FIXTURE);
    }

    /// `flate2`'s default gzip header embeds neither the current time nor the
    /// source filename, so the output is identical run to run.
    #[test]
    fn vgz_output_is_deterministic() {
        let song = read("f.vgz", &gzip(VGM_FIXTURE)).unwrap();
        assert_eq!(write_gzipped(&song).unwrap(), write_gzipped(&song).unwrap());
    }

    #[test]
    fn compression_is_detected_from_the_bytes_not_the_name() {
        assert!(!is_gzipped(VGM_FIXTURE));
        assert!(is_gzipped(&gzip(VGM_FIXTURE)));
        let song = read("misnamed.vgm", &gzip(VGM_FIXTURE)).unwrap();
        assert_eq!(song.len(), 299);
    }

    // -- loop points -------------------------------------------------------

    /// Six commands, 16 bytes, 30735 samples. Byte offsets 0, 3, 6, 9, 12, 15.
    ///
    /// index 0: 5A 20 01  register write
    /// index 1: 61 10 27  wait 10000 samples
    /// index 2: 5A 21 02  register write     <- a loop pointing here starts at 20735
    /// index 3: 61 20 4E  wait 20000 samples
    /// index 4: 5A 22 03  register write
    /// index 5: 62        wait 735 samples
    const LOOPING_COMMANDS: &[u8] = &[
        0x5A, 0x20, 0x01, //
        0x61, 0x10, 0x27, //
        0x5A, 0x21, 0x02, //
        0x61, 0x20, 0x4E, //
        0x5A, 0x22, 0x03, //
        0x62,
    ];
    const LOOPING_TOTAL_SAMPLES: u32 = 10_000 + 20_000 + 735;

    /// Builds a VGM whose header loops at `loop_byte` (an offset into the command
    /// stream), declaring `loop_num_samples`.
    fn looping_vgm(loop_byte: usize, loop_num_samples: u32) -> Vec<u8> {
        let mut header = synthesise_header();
        put_chip_clocks(&mut header, OplType::Opl2).unwrap();
        put_u32(&mut header, offset::TOTAL_SAMPLES, LOOPING_TOTAL_SAMPLES);
        let absolute = header.len() + loop_byte;
        put_u32(
            &mut header,
            offset::LOOP_OFFSET,
            (absolute - offset::LOOP_OFFSET) as u32,
        );
        put_u32(&mut header, offset::LOOP_NUM_SAMPLES, loop_num_samples);

        let eof = header.len() + LOOPING_COMMANDS.len() + 1;
        put_u32(&mut header, offset::EOF, (eof - offset::EOF) as u32);

        let mut out = header;
        out.extend_from_slice(LOOPING_COMMANDS);
        out.push(command::END);
        out
    }

    fn header_u32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_loop_point_resolves_to_an_instruction_index() {
        let song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
        assert_eq!(song.loop_num_samples(), Some(20_735));
        assert_eq!(song.total_delay_samples(), LOOPING_TOTAL_SAMPLES);
    }

    #[test]
    fn writing_a_short_header_errors_rather_than_panicking() {
        // A header below the v1.51 minimum must be rejected, not panic on the
        // fixed-offset field writes.
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        song.vgm_meta_mut().unwrap().header.truncate(0x10);
        assert!(write(&song).is_err());
    }

    #[test]
    fn a_looping_file_round_trips_byte_for_byte() {
        let original = looping_vgm(6, 20_735);
        let song = read("t.vgm", &original).unwrap();
        assert_eq!(write(&song).unwrap(), original);
    }

    /// The whole point. Delete a command *before* the loop and the byte offset must
    /// follow it.
    #[test]
    fn deleting_before_the_loop_point_moves_the_offset() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let mut undo = UndoController::new();

        // Instruction 0 is a three-byte register write with no delay.
        undo.execute(Box::new(DeleteInstructions::new([0])), &mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(1));
        assert_eq!(
            song.loop_num_samples(),
            Some(20_735),
            "the loop is unchanged"
        );

        let written = write(&song).unwrap();
        let reread = read("t.vgm", &written).unwrap();
        assert_eq!(reread.vgm_meta().unwrap().loop_point, Some(1));
        assert_eq!(reread.loop_num_samples(), Some(20_735));

        // The loop byte offset shrank by exactly the deleted command's length.
        let before = header_u32(&looping_vgm(6, 20_735), offset::LOOP_OFFSET);
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), before - 3);

        undo.undo(&mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
        assert_eq!(write(&song).unwrap(), looping_vgm(6, 20_735));
    }

    /// Deleting a delay *inside* the loop shortens it, and `loop # samples` is
    /// recomputed to match.
    #[test]
    fn deleting_inside_the_loop_shortens_it() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let mut undo = UndoController::new();

        // Instruction 3 is the 20000-sample wait, after the loop point.
        undo.execute(Box::new(DeleteInstructions::new([3])), &mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2), "unmoved");
        assert_eq!(song.loop_num_samples(), Some(735));
        assert_eq!(song.total_delay_samples(), LOOPING_TOTAL_SAMPLES - 20_000);

        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 735);
        assert_eq!(
            header_u32(&written, offset::TOTAL_SAMPLES),
            LOOPING_TOTAL_SAMPLES - 20_000
        );

        undo.undo(&mut song);
        assert_eq!(write(&song).unwrap(), looping_vgm(6, 20_735));
    }

    /// A delay before the loop point shortens the song but not the loop.
    #[test]
    fn deleting_a_delay_before_the_loop_point_leaves_the_loop_length_alone() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteInstructions::new([1])), &mut song); // the 10000 wait
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(1));
        assert_eq!(song.loop_num_samples(), Some(20_735));
        assert_eq!(song.total_delay_samples(), 20_735);

        undo.undo(&mut song);
        assert_eq!(song.loop_num_samples(), Some(20_735));
        assert_eq!(song.total_delay_samples(), LOOPING_TOTAL_SAMPLES);
    }

    /// Deleting the loop instruction leaves the loop on whatever now occupies its
    /// slot -- the next surviving command.
    #[test]
    fn deleting_the_loop_instruction_slides_it_forward() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteInstructions::new([2])), &mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
        // Index 2 is now the old index 3, the 20000-sample wait.
        assert_eq!(song.loop_num_samples(), Some(20_735));

        undo.undo(&mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
    }

    #[test]
    fn deleting_the_loop_point_and_everything_after_it_removes_the_loop() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteInstructions::new([2, 3, 4, 5])), &mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, None);
        assert_eq!(song.loop_num_samples(), None);

        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), 0);
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 0);

        undo.undo(&mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
        assert_eq!(write(&song).unwrap(), looping_vgm(6, 20_735));
    }

    #[test]
    fn a_loop_point_inside_a_command_is_dropped() {
        // Byte 7 is the middle of the register write at index 2.
        let song = read("t.vgm", &looping_vgm(7, 20_735)).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_point, None);
        assert_eq!(header_u32(&write(&song).unwrap(), offset::LOOP_OFFSET), 0);
    }

    #[test]
    fn a_loop_point_inside_the_header_or_past_the_stream_is_dropped() {
        let mut bytes = looping_vgm(6, 20_735);
        put_u32(&mut bytes, offset::LOOP_OFFSET, 4); // absolute 0x20, inside the header
        assert_eq!(
            read("t.vgm", &bytes)
                .unwrap()
                .vgm_meta()
                .unwrap()
                .loop_point,
            None
        );

        let mut bytes = looping_vgm(6, 20_735);
        put_u32(&mut bytes, offset::LOOP_OFFSET, 0xFFFF); // way past the end
        assert_eq!(
            read("t.vgm", &bytes)
                .unwrap()
                .vgm_meta()
                .unwrap()
                .loop_point,
            None
        );
    }

    /// A zero loop offset means "no loop", so both fields stay zero -- which is why
    /// the OPL2 fixture, which does not loop, still round-trips.
    #[test]
    fn a_zero_loop_offset_means_no_loop() {
        let song = read("f.vgm", VGM_FIXTURE).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_point, None);
        assert_eq!(song.loop_num_samples(), None);
        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), 0);
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 0);
    }

    // -- explicit loop ends -------------------------------------------------
    //
    // The fixture's sample prefix is [0, 0, 10000, 10000, 30000, 30000, 30735],
    // so a loop at index 2 starts at 10000 and runs 20735 samples to the end.
    // A declared 20000 ends at index 4, the only boundary 20000 samples along.

    #[test]
    fn a_shorter_loop_length_resolves_to_an_end_index() {
        let song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        let meta = song.vgm_meta().unwrap();
        assert_eq!(meta.loop_point, Some(2));
        assert_eq!(
            meta.loop_end,
            Some(4),
            "ends before the last register write"
        );
        assert_eq!(song.loop_num_samples(), Some(20_000));
        // The song itself is untouched -- only the loop stops early.
        assert_eq!(song.total_delay_samples(), LOOPING_TOTAL_SAMPLES);
    }

    /// The whole point of materialising it: without a `loop_end` the re-save
    /// would widen the loop back out to the full tail.
    #[test]
    fn a_file_with_an_explicit_loop_end_round_trips_byte_for_byte() {
        let original = looping_vgm(6, 20_000);
        let song = read("t.vgm", &original).unwrap();
        assert_eq!(write(&song).unwrap(), original);
    }

    #[test]
    fn an_explicit_loop_end_survives_a_reload() {
        let song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        let reread = read("t.vgm", &write(&song).unwrap()).unwrap();
        assert_eq!(reread.vgm_meta().unwrap().loop_end, Some(4));
        assert_eq!(reread.loop_num_samples(), Some(20_000));
    }

    /// A length landing mid-delay cannot be an instruction boundary, so it is a
    /// stale header rather than an authored end: loop to the end of the stream.
    #[test]
    fn a_loop_length_that_misses_a_command_boundary_falls_back_to_the_end() {
        // 15000 lands inside the 20000-sample wait at index 3.
        let song = read("t.vgm", &looping_vgm(6, 15_000)).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_end, None);
        assert_eq!(song.loop_num_samples(), Some(20_735));
        // Saving corrects the header to what the stream actually says.
        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 20_735);
    }

    #[test]
    fn a_loop_length_longer_than_the_stream_falls_back_to_the_end() {
        let song = read("t.vgm", &looping_vgm(6, 99_999)).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_end, None);
        assert_eq!(song.loop_num_samples(), Some(20_735));
    }

    /// A loop offset with a zero length is self-contradictory; an empty loop is
    /// never what was meant, so the stream wins.
    #[test]
    fn a_zero_loop_length_falls_back_to_the_end() {
        let song = read("t.vgm", &looping_vgm(6, 0)).unwrap();
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(2));
        assert_eq!(song.vgm_meta().unwrap().loop_end, None);
        assert_eq!(song.loop_num_samples(), Some(20_735));
    }

    #[test]
    fn deleting_before_the_loop_slides_both_markers() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        song.delete_instructions(&[0]);
        let meta = song.vgm_meta().unwrap();
        assert_eq!((meta.loop_point, meta.loop_end), (Some(1), Some(3)));
        assert_eq!(
            song.loop_num_samples(),
            Some(20_000),
            "the region is intact"
        );
    }

    #[test]
    fn deleting_inside_the_loop_shortens_it_but_keeps_the_end() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        // Index 3 is the 20000-sample wait, inside the loop.
        song.delete_instructions(&[3]);
        let meta = song.vgm_meta().unwrap();
        assert_eq!((meta.loop_point, meta.loop_end), (Some(2), Some(3)));
        assert_eq!(song.loop_num_samples(), Some(0));
    }

    /// Deleting the entire region leaves no loop to bound, so the end marker
    /// gives way to the default rather than describing an empty loop.
    #[test]
    fn deleting_the_whole_loop_region_drops_the_end_marker() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        song.delete_instructions(&[2, 3]);
        let meta = song.vgm_meta().unwrap();
        assert_eq!(
            meta.loop_point,
            Some(2),
            "slid onto the surviving successor"
        );
        assert_eq!(meta.loop_end, None, "back to the end of the song");
    }

    #[test]
    fn deleting_from_the_loop_point_onward_clears_both_markers() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        song.delete_instructions(&[2, 3, 4, 5]);
        let meta = song.vgm_meta().unwrap();
        assert_eq!((meta.loop_point, meta.loop_end), (None, None));
        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), 0);
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 0);
    }

    /// The synthetic fixtures above all have their data at 0x80 and delays in
    /// tidy round numbers. This runs the same round trip over the real
    /// `dro2vgm` capture, whose stream is a few hundred commands of ordinary
    /// music, so the boundary search has to find a genuine command edge.
    #[test]
    fn a_real_capture_carries_an_explicit_loop_end_through_a_round_trip() {
        let mut song = read("lsl3.vgm", VGM_FIXTURE).unwrap();
        let len = song.len();
        // A region well inside the song, ending on an instruction that actually
        // has time before it (the prefix must strictly increase across it, or
        // the length would be ambiguous rather than wrong).
        let prefix = song.delay_samples_prefix();
        let start = 1;
        let end = (start + 1..len)
            .find(|&index| prefix[index] > prefix[start])
            .expect("the capture has delays");
        {
            let meta = song.vgm_meta_mut().unwrap();
            meta.loop_point = Some(start);
            meta.loop_end = Some(end);
        }
        let expected = song.loop_num_samples().unwrap();
        assert!(expected > 0, "the region has to last some time to be found");

        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), expected);

        let reread = read("lsl3.vgm", &written).unwrap();
        let meta = reread.vgm_meta().unwrap();
        assert_eq!(meta.loop_point, Some(start));
        assert_eq!(meta.loop_end, Some(end));
        assert_eq!(reread.loop_num_samples(), Some(expected));
        // And it is now stable: writing the re-read song reproduces the bytes.
        assert_eq!(write(&reread).unwrap(), written);
    }

    #[test]
    fn undoing_a_delete_restores_the_loop_end() {
        use crate::UndoableCommand;
        use crate::undo::DeleteInstructions;

        let mut song = read("t.vgm", &looping_vgm(6, 20_000)).unwrap();
        let mut command = DeleteInstructions::new([2, 3]);
        command.apply(&mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_end, None);

        command.revert(&mut song);
        let meta = song.vgm_meta().unwrap();
        assert_eq!((meta.loop_point, meta.loop_end), (Some(2), Some(4)));
        assert_eq!(song.loop_num_samples(), Some(20_000));
    }

    /// Adding a GD3 tag appends after the data, so the loop point must not move.
    #[test]
    fn adding_a_gd3_tag_does_not_move_the_loop_point() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        let before = header_u32(&write(&song).unwrap(), offset::LOOP_OFFSET);

        song.vgm_meta_mut().unwrap().tag = Some(tag());
        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), before);
        assert_eq!(
            read("t.vgm", &written)
                .unwrap()
                .vgm_meta()
                .unwrap()
                .loop_point,
            Some(2)
        );
    }

    #[test]
    fn clearing_the_loop_point_zeroes_both_header_fields() {
        let mut song = read("t.vgm", &looping_vgm(6, 20_735)).unwrap();
        song.vgm_meta_mut().unwrap().loop_point = None;
        let written = write(&song).unwrap();
        assert_eq!(header_u32(&written, offset::LOOP_OFFSET), 0);
        assert_eq!(header_u32(&written, offset::LOOP_NUM_SAMPLES), 0);
    }

    #[test]
    fn trimming_then_writing_updates_the_sample_count_and_eof() {
        let mut song = read("f.vgm", VGM_FIXTURE).unwrap();
        let mut undo = UndoController::new();
        // Instruction 5 is the first wait, in the fixture.
        let first_wait = (0..song.len())
            .find(|&i| song.instruction(i).unwrap().is_delay())
            .unwrap();
        let removed = song.instruction(first_wait).unwrap().delay_samples();
        undo.execute(Box::new(DeleteInstructions::new([first_wait])), &mut song);

        let written = write(&song).unwrap();
        let total = u32::from_le_bytes(written[0x18..0x1C].try_into().unwrap());
        let eof = u32::from_le_bytes(written[0x04..0x08].try_into().unwrap());
        assert_eq!(total, 118_320 - removed);
        assert_eq!(eof as usize + 4, written.len());

        let reread = read("f.vgm", &written).unwrap();
        assert_eq!(reread.len(), 298);
        assert_eq!(reread.total_delay_samples(), 118_320 - removed);
    }

    // -- GD3 ---------------------------------------------------------------

    fn tag() -> Gd3Tag {
        Gd3Tag {
            track_name_en: "Score Up".to_owned(),
            track_name_native: "スコアアップ".to_owned(),
            game_name_en: "Leisure Suit Larry 3".to_owned(),
            game_name_native: String::new(),
            system_name_en: "IBM PC AT".to_owned(),
            system_name_native: String::new(),
            track_author_en: "Sierra".to_owned(),
            track_author_native: String::new(),
            release_date: "1989".to_owned(),
            creator: "VGM Studio".to_owned(),
            notes: "line one\nline two".to_owned(),
        }
    }

    #[test]
    fn gd3_round_trips() {
        let bytes = write_gd3_tag(&tag());
        assert_eq!(&bytes[..4], GD3_MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0x100);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len() - 12,
            "the length field counts only the string blob"
        );
        assert_eq!(parse_gd3_tag(&bytes, 0).unwrap(), tag());
    }

    #[test]
    fn gd3_survives_a_whole_file_round_trip() {
        let mut song = read("f.vgm", VGM_FIXTURE).unwrap();
        song.vgm_meta_mut().unwrap().tag = Some(tag());

        let written = write(&song).unwrap();
        let reread = read("f.vgm", &written).unwrap();
        assert_eq!(reread.vgm_meta().unwrap().tag.as_ref(), Some(&tag()));
        assert_eq!(reread.data(), song.data());

        // The header's GD3 offset must point at the tag, relative to 0x14.
        let gd3_relative = u32::from_le_bytes(written[0x14..0x18].try_into().unwrap()) as usize;
        assert_eq!(
            &written[0x14 + gd3_relative..0x14 + gd3_relative + 4],
            GD3_MAGIC
        );
        // ... and writing the tag back out again is stable.
        assert_eq!(write(&reread).unwrap(), written);
    }

    #[test]
    fn an_empty_gd3_tag_round_trips() {
        let empty = Gd3Tag::default();
        let bytes = write_gd3_tag(&empty);
        assert_eq!(
            bytes.len(),
            12 + GD3_FIELD_COUNT * 2,
            "eleven bare terminators"
        );
        assert_eq!(parse_gd3_tag(&bytes, 0).unwrap(), empty);
    }

    #[test]
    fn gd3_rejects_a_wrong_field_count() {
        let mut bytes = write_gd3_tag(&Gd3Tag::default());
        bytes.truncate(bytes.len() - 2); // drop one terminator
        let length = bytes.len() - 12;
        bytes[8..12].copy_from_slice(&(length as u32).to_le_bytes());
        let error = parse_gd3_tag(&bytes, 0).unwrap_err().to_string();
        assert!(error.contains("expected 11"), "{error}");
    }

    #[test]
    fn gd3_tolerates_extra_trailing_empty_fields() {
        // Some encoders (the Doom rips among them) append a doubled final
        // terminator, leaving a twelfth empty field. It parses as the real eleven.
        let original = tag();
        let mut bytes = write_gd3_tag(&original);
        bytes.extend_from_slice(&0u16.to_le_bytes()); // one extra empty field
        let length = bytes.len() - 12;
        bytes[8..12].copy_from_slice(&(length as u32).to_le_bytes());
        assert_eq!(parse_gd3_tag(&bytes, 0).unwrap(), original);
    }

    #[test]
    fn gd3_rejects_a_bad_magic_or_version() {
        let mut bytes = write_gd3_tag(&tag());
        bytes[0] = b'X';
        assert!(parse_gd3_tag(&bytes, 0).is_err());

        let mut bytes = write_gd3_tag(&tag());
        bytes[4..8].copy_from_slice(&0x200u32.to_le_bytes());
        assert!(parse_gd3_tag(&bytes, 0).is_err());
    }

    // -- rejections --------------------------------------------------------

    /// A rip that declares only the fields it uses puts its data at 0x60, right
    /// after the YMF262 clock -- exactly as valid as a full-length header.
    #[test]
    fn a_minimal_header_with_its_data_at_0x60_opens_and_round_trips() {
        let mut header = vec![0u8; 0x60];
        header[..4].copy_from_slice(MAGIC);
        put_u32(&mut header, offset::VERSION, 0x151);
        put_u32(
            &mut header,
            offset::DATA_OFFSET,
            (0x60 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut header, offset::YMF262_CLOCK, clock::OPL3);
        put_u32(&mut header, offset::TOTAL_SAMPLES, 735);

        let mut bytes = header;
        bytes.extend_from_slice(&[0x5E, 0x20, 0x01, command::WAIT_60TH]);
        bytes.push(command::END);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let song = read("minimal.vgm", &bytes).unwrap();
        assert_eq!(song.opl_type, OplType::Opl3);
        assert_eq!(song.len(), 2);
        assert_eq!(song.total_delay_samples(), 735);
        assert_eq!(
            song.vgm_meta().unwrap().header().len(),
            0x60,
            "the short header is kept as it is, not padded out"
        );
        // And it writes back byte for byte, with no attempt to grow the fields
        // a longer header would have.
        assert_eq!(write(&song).unwrap(), bytes);
    }

    /// A header too short to hold even the OPL clocks cannot declare one, so it
    /// is not an OPL song at all.
    #[test]
    fn a_header_stopping_before_the_opl_clocks_is_not_an_opl_song() {
        let mut bytes = vec![0u8; 0x40];
        bytes[..4].copy_from_slice(MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x151);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x40 - offset::DATA_OFFSET) as u32,
        );
        bytes.push(command::END);
        let error = read("tiny.vgm", &bytes).unwrap_err().to_string();
        assert!(error.contains("No OPL2 or OPL3 data detected"), "{error}");
    }

    #[test]
    fn rejects_a_bad_magic() {
        assert!(read("f.vgm", b"NOPE............").is_err());
    }

    #[test]
    fn rejects_an_old_version() {
        let mut bytes = VGM_FIXTURE.to_vec();
        bytes[0x08..0x0C].copy_from_slice(&0x150u32.to_le_bytes());
        let error = read("f.vgm", &bytes).unwrap_err().to_string();
        assert!(error.contains("v1.51 is the minimum"), "{error}");
    }

    #[test]
    fn rejects_a_file_with_no_opl_chip() {
        let mut bytes = VGM_FIXTURE.to_vec();
        bytes[0x50..0x54].copy_from_slice(&0u32.to_le_bytes());
        let error = read("f.vgm", &bytes).unwrap_err().to_string();
        assert!(error.contains("No OPL2 or OPL3 data detected"), "{error}");
    }

    #[test]
    fn rejects_an_unsupported_command() {
        let mut bytes = VGM_FIXTURE.to_vec();
        bytes[0x80] = 0x50; // a PSG write, which this app cannot re-encode
        let error = read("f.vgm", &bytes).unwrap_err().to_string();
        assert!(error.contains("Unsupported VGM command"), "{error}");
    }

    #[test]
    fn rejects_a_corrupt_gzip_stream() {
        let mut bytes = gzip(VGM_FIXTURE);
        let last = bytes.len() - 20;
        bytes[last] ^= 0xFF;
        assert!(read("f.vgz", &bytes).is_err());
    }

    #[test]
    fn opl_type_selects_the_chip_clocks() {
        for (opl_type, ym3812, ymf262) in [
            (OplType::Opl2, clock::OPL2, 0),
            (OplType::DualOpl2, clock::DUAL_OPL2, 0),
            (OplType::Opl3, 0, clock::OPL3),
        ] {
            let mut header = synthesise_header();
            put_chip_clocks(&mut header, opl_type).unwrap();
            assert_eq!(
                u32::from_le_bytes(header[0x50..0x54].try_into().unwrap()),
                ym3812
            );
            assert_eq!(
                u32::from_le_bytes(header[0x5C..0x60].try_into().unwrap()),
                ymf262
            );
        }
    }

    #[test]
    fn a_synthesised_header_matches_the_fixtures_shape() {
        let header = synthesise_header();
        assert_eq!(header.len(), 0x80);
        assert_eq!(&header[..4], MAGIC);
        assert_eq!(
            u32::from_le_bytes(header[0x08..0x0C].try_into().unwrap()),
            0x151
        );
        assert_eq!(
            u32::from_le_bytes(header[0x24..0x28].try_into().unwrap()),
            1000
        );
        assert_eq!(
            u32::from_le_bytes(header[0x34..0x38].try_into().unwrap()),
            0x4C,
            "data offset, relative to 0x34"
        );
    }

    #[test]
    fn the_short_wait_commands_decode_as_the_spec_says() {
        // Not in the fixture, so build a tiny file around them.
        let mut header = synthesise_header();
        put_chip_clocks(&mut header, OplType::Opl2).unwrap();
        let mut bytes = header;
        bytes.extend_from_slice(&[command::WAIT_60TH, command::WAIT_50TH, 0x70, 0x7F]);
        bytes.push(command::END);

        let song = read("t.vgm", &bytes).unwrap();
        let samples: Vec<u32> = song.data().iter().map(Instruction::delay_samples).collect();
        assert_eq!(samples, [735, 882, 1, 16]);
        assert_eq!(song.total_delay_samples(), 735 + 882 + 1 + 16);
        assert_eq!(
            song.instruction(0).unwrap().delay_kind(),
            Some(DelayKind::Short)
        );
    }
}
