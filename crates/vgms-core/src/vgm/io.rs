//! The VGM container and its gzipped form, VGZ.
//!
//! # Byte-exact round trips
//!
//! The header is kept verbatim and only the fields that can have changed are
//! patched, so an unedited round trip reproduces the file exactly -- including
//! chip clocks, `rate`, and any v1.70 extra header.

use crate::error::{Error, Result};
use crate::io::ByteReader;
use crate::song::OplType;
use crate::vgm::data::{GD3_FIELD_COUNT, Gd3Tag};
use crate::vgm::header::offset;

/// `Vgm `.
pub const MAGIC: &[u8; 4] = b"Vgm ";
/// `Gd3 `.
pub const GD3_MAGIC: &[u8; 4] = b"Gd3 ";

/// v1.51 is the first version with the OPL chip clock fields this app needs.
pub const MINIMUM_SUPPORTED_VERSION: u32 = 0x0000_0151;
/// The version [`DroSong::to_vgm`](crate::convert::dro_to_vgm) emits.
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

    // -- header synthesis --------------------------------------------------

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
}
