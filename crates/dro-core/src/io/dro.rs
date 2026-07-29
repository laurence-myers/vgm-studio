//! The DRO container, versions 1 and 2.
//!
//! Trailing bytes after the declared v1 data are a warning, not an error. The
//! oracle is the format itself, plus `tests/lsl3_score_up_dro2.dro`, which this
//! reads and writes byte-for-byte.

use crate::error::{Error, Result};
use crate::io::ByteReader;
use crate::song::dro_data::{DroDataV1, DroDataV2};
use crate::song::{OplType, Song, SongData};

/// `DBRAWOPL`.
pub const MAGIC: &[u8; 8] = b"DBRAWOPL";

/// The DOSBox developers shipped v1 files with the version pair both ways round.
const VERSION_V1_OLD: (u16, u16) = (1, 0);
const VERSION_V1_NEW: (u16, u16) = (0, 1);
const VERSION_V2: (u16, u16) = (2, 0);

/// v1 headers were once written with a one-byte OPL type and later with a
/// four-byte one. [`read`] detects which; writing always uses four bytes.
const WRITE_CHAR_OPL: bool = false;

/// Parses a DRO file of either version.
///
/// `name` is carried on the returned [`Song`] for the title bar and for
/// `Save`; it is never opened.
///
/// # Errors
/// If the magic or version is wrong, if the file is truncated, or if the data
/// stream is malformed in a way that cannot be recovered from.
pub fn read(name: &str, bytes: &[u8]) -> Result<Song> {
    let mut reader = ByteReader::new(bytes);

    let magic = reader.take(8)?;
    if magic != MAGIC {
        return Err(Error::file(format!(
            "Does not appear to be a DRO file (invalid header. Expected {}, found {}).",
            String::from_utf8_lossy(MAGIC),
            String::from_utf8_lossy(magic),
        )));
    }

    let version = (reader.u16_le()?, reader.u16_le()?);
    match version {
        VERSION_V1_OLD | VERSION_V1_NEW => read_v1(name, reader),
        VERSION_V2 => read_v2(name, reader),
        (major, minor) => Err(Error::file(format!(
            "Unsupported version of the DRO file format. Supported: v1 or v2. Found: \
             ({major}, {minor})"
        ))),
    }
}

/// Serialises a song back to DRO bytes, at whichever version it was read as.
///
/// # Errors
/// If the song is a VGM, or its data does not match its declared version.
pub fn write(song: &Song) -> Result<Vec<u8>> {
    match song.data() {
        SongData::V1(_) => Ok(write_v1(song)),
        SongData::V2(data) => Ok(write_v2(song, data)),
        SongData::Vgm(_) => Err(Error::file("Tried to write a VGM song as a DRO")),
    }
}

// ---------------------------------------------------------------------------
// v1
// ---------------------------------------------------------------------------

fn read_v1(name: &str, mut reader: ByteReader<'_>) -> Result<Song> {
    let ms_length = reader.u32_le()?;
    let byte_length = reader.u32_le()? as usize;

    // Old rips wrote the OPL type as one byte, newer ones as four. If the
    // four-byte read yields something implausibly large, we must have eaten the
    // first bytes of the instruction stream, so back up and read one byte.
    //
    // This cannot mistake a four-byte type for a one-byte one (0, 1 and 2 all fit
    // in a byte). It *can* go the other way, if a one-byte type is followed by
    // three zero bytes.
    let opl_type_offset = reader.offset();
    let opl_type_code = reader.u32_le()?;
    let opl_type_code = if opl_type_code > 0xFF {
        reader.seek(opl_type_offset)?;
        u32::from(reader.u8()?)
    } else {
        opl_type_code
    };

    let opl_type = u8::try_from(opl_type_code)
        .ok()
        .and_then(OplType::from_v1_code)
        .ok_or_else(|| Error::file(format!("Unknown DRO v1 OPL type: {opl_type_code}")))?;

    if reader.remaining() < byte_length {
        return Err(Error::file(format!(
            "DRO v1 header declares {byte_length} bytes of data, but only {} remain",
            reader.remaining()
        )));
    }
    let raw = reader.take(byte_length)?.to_vec();

    // Trailing bytes are not rejected. Rejecting a file over some slop at the end
    // helps nobody.
    let trailing = reader.remaining();
    if trailing > 0 {
        log::warn!(
            "DRO v1 file has {trailing} byte(s) after the {byte_length} bytes its header \
             declares; ignoring them"
        );
    }

    let (data, dropped) = DroDataV1::new_truncating(raw)?;
    if dropped > 0 {
        log::warn!("DRO v1 data ends mid-instruction; dropping the last {dropped} byte(s)");
    }

    Ok(Song::dro_v1(name.to_owned(), data, ms_length, opl_type))
}

fn write_v1(song: &Song) -> Vec<u8> {
    let raw = song.data().raw();
    debug_assert!(
        u32::try_from(raw.len()).is_ok(),
        "DRO data must fit the header's u32 length field"
    );

    // Not `song.ms_length`: recompute it, because V1 and V2 files write this
    // value differently.
    let total_delay = song.total_delay_ms();

    let mut out = Vec::with_capacity(0x18 + raw.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION_V1_NEW.0.to_le_bytes());
    out.extend_from_slice(&VERSION_V1_NEW.1.to_le_bytes());
    out.extend_from_slice(&total_delay.to_le_bytes());
    out.extend_from_slice(&u32::try_from(raw.len()).unwrap_or(u32::MAX).to_le_bytes());
    if WRITE_CHAR_OPL {
        out.push(song.opl_type.v1_code());
    } else {
        out.extend_from_slice(&u32::from(song.opl_type.v1_code()).to_le_bytes());
    }
    out.extend_from_slice(raw);
    out
}

// ---------------------------------------------------------------------------
// v2
// ---------------------------------------------------------------------------

fn read_v2(name: &str, mut reader: ByteReader<'_>) -> Result<Song> {
    let length_pairs = reader.u32_le()?;
    let ms_length = reader.u32_le()?;
    let hardware_type = reader.u8()?;
    let format = reader.u8()?;
    let compression = reader.u8()?;
    let short_delay_code = reader.u8()?;
    let long_delay_code = reader.u8()?;
    let codemap_length = usize::from(reader.u8()?);

    if format != 0 {
        return Err(Error::file(format!(
            "Unsupported DRO v2 format. Only 0 is supported, found format ID {format}"
        )));
    }
    if compression != 0 {
        return Err(Error::file(format!(
            "Unsupported DRO v2 compression. Only 0 is supported, found compression ID \
             {compression}"
        )));
    }
    if codemap_length > 128 {
        return Err(Error::file(format!(
            "DRO v2 file has too many entries in the codemap. Maximum 128, found \
             {codemap_length}. Is the file corrupt?"
        )));
    }

    let opl_type = OplType::from_v2_code(hardware_type)
        .ok_or_else(|| Error::file(format!("Unknown DRO v2 hardware type: {hardware_type}")))?;

    let codemap = reader.take(codemap_length)?.to_vec();

    // A corrupt header can declare more pairs than a `usize` can address -- and on
    // wasm32 a `usize` is only 32 bits, so `length_pairs * 2` would wrap.
    let data_length = usize::try_from(length_pairs)
        .ok()
        .and_then(|pairs| pairs.checked_mul(2))
        .ok_or_else(|| {
            Error::file(format!(
                "DRO v2 header declares {length_pairs} register/value pairs, which cannot be \
                 addressed on this platform"
            ))
        })?;
    let raw = reader.take(data_length)?.to_vec();

    let trailing = reader.remaining();
    if trailing > 0 {
        log::warn!("DRO v2 file has {trailing} byte(s) after the instruction stream; ignoring");
    }

    let data = DroDataV2::new(raw, codemap, short_delay_code, long_delay_code)?;
    Ok(Song::dro_v2(name.to_owned(), data, ms_length, opl_type))
}

fn write_v2(song: &Song, data: &DroDataV2) -> Vec<u8> {
    let raw = data.raw();
    let codemap = data.codemap();
    debug_assert!(
        u32::try_from(raw.len()).is_ok(),
        "DRO data must fit the header's u32 length field"
    );
    debug_assert!(codemap.len() <= 128, "`DroDataV2::new` caps the codemap");

    let mut out = Vec::with_capacity(0x1A + codemap.len() + raw.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION_V2.0.to_le_bytes());
    out.extend_from_slice(&VERSION_V2.1.to_le_bytes());
    // Length in register/value pairs.
    out.extend_from_slice(&u32::try_from(song.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&song.ms_length.to_le_bytes());
    out.push(song.opl_type.v2_code());
    out.push(0); // format
    out.push(0); // compression
    out.push(data.short_delay_code());
    out.push(data.long_delay_code());
    out.push(u8::try_from(codemap.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(codemap);
    out.extend_from_slice(raw);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Bank, DRO_FILE_V1, DRO_FILE_V2, DelayKind, DroInstruction};
    use crate::undo::{DeleteInstructions, UndoController};

    const DRO_V2_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up_dro2.dro");

    /// A hand-built v1 file: register write, short delay, long delay, both bank
    /// switches, an escaped register, another register.
    fn v1_bytes(char_opl_type: bool) -> Vec<u8> {
        let data: &[u8] = &[
            0x20, 0x01, // register 0x20 = 0x01
            0x00, 0xB0, // short delay: 177 ms
            0x01, 0x34, 0x12, // long delay: 0x1234 + 1 = 4661 ms
            0x02, // bank low
            0x03, // bank high
            0x04, 0x01, 0xFF, // escaped register 0x01 = 0xFF
            0xBD, 0x20, // register 0xBD = 0x20
        ];
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&0u16.to_le_bytes()); // VERSION_V1_NEW
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(177u32 + 4661).to_le_bytes()); // ms length
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        if char_opl_type {
            out.push(0); // OPL2, one byte
        } else {
            out.extend_from_slice(&0u32.to_le_bytes()); // OPL2, four bytes
        }
        out.extend_from_slice(data);
        out
    }

    // -- v2 ----------------------------------------------------------------

    #[test]
    fn read_the_v2_fixture() {
        let song = read("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
        assert_eq!(song.ms_length, 2683);
        assert_eq!(song.file_version, DRO_FILE_V2);
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.name, "lsl3_score_up_dro2.dro");
        assert_eq!(song.len(), 299);

        let SongData::V2(data) = song.data() else {
            panic!("expected a v2 song")
        };
        assert_eq!(data.short_delay_code(), 122);
        assert_eq!(data.long_delay_code(), 123);
        assert_eq!(
            &data.codemap()[..10],
            [1, 4, 5, 8, 189, 32, 64, 96, 128, 224]
        );
        assert_eq!(&data.raw()[..10], [0, 32, 5, 49, 10, 2, 15, 2, 20, 98]);
    }

    #[test]
    fn the_v2_fixture_decodes_to_sensible_instructions() {
        let song = read("f.dro", DRO_V2_FIXTURE).unwrap();
        // codemap[0] = 0x01, value 0x20: "Test LSI Register / Waveform Select Enable"
        assert_eq!(
            song.instruction(0).unwrap(),
            DroInstruction::Register {
                reg: 0x01,
                value: 0x20,
                bank: Some(Bank::Low)
            }
        );
        // The header's ms length must equal the summed delays.
        assert_eq!(song.total_delay_ms(), 2683);
        assert_eq!(song.total_delay_ms(), song.ms_length);
    }

    /// The load-bearing test: `dro-core` reproduces the fixture byte for byte.
    #[test]
    fn the_v2_fixture_round_trips_byte_for_byte() {
        let song = read("f.dro", DRO_V2_FIXTURE).unwrap();
        let written = write(&song).unwrap();
        assert_eq!(written.len(), DRO_V2_FIXTURE.len());
        assert_eq!(written, DRO_V2_FIXTURE);
    }

    #[test]
    fn writing_a_trimmed_v2_song_stays_readable() {
        let mut song = read("f.dro", DRO_V2_FIXTURE).unwrap();
        let mut undo = UndoController::new();
        undo.execute(Box::new(DeleteInstructions::new([0, 1, 2])), &mut song);

        let written = write(&song).unwrap();
        let reread = read("f.dro", &written).unwrap();
        assert_eq!(reread.len(), 299 - 3);
        assert_eq!(reread.ms_length, song.ms_length);
        assert_eq!(reread.data(), song.data());
    }

    #[test]
    fn v2_rejects_a_nonzero_format_or_compression() {
        let mut bytes = DRO_V2_FIXTURE.to_vec();
        bytes[0x15] = 1; // format
        assert!(
            read("f.dro", &bytes)
                .unwrap_err()
                .to_string()
                .contains("format ID 1")
        );

        let mut bytes = DRO_V2_FIXTURE.to_vec();
        bytes[0x16] = 1; // compression
        assert!(
            read("f.dro", &bytes)
                .unwrap_err()
                .to_string()
                .contains("compression ID 1")
        );
    }

    #[test]
    fn v2_rejects_an_oversized_codemap() {
        let mut bytes = DRO_V2_FIXTURE.to_vec();
        bytes[0x19] = 129;
        assert!(
            read("f.dro", &bytes)
                .unwrap_err()
                .to_string()
                .contains("Maximum 128")
        );
    }

    #[test]
    fn v2_rejects_an_unknown_hardware_type() {
        let mut bytes = DRO_V2_FIXTURE.to_vec();
        bytes[0x14] = 3;
        assert!(read("f.dro", &bytes).is_err());
    }

    // -- v1 ----------------------------------------------------------------

    #[test]
    fn read_a_v1_file_with_a_four_byte_opl_type() {
        let song = read("test.dro", &v1_bytes(false)).unwrap();
        assert_eq!(song.file_version, DRO_FILE_V1);
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.len(), 7);
        assert_eq!(song.ms_length, 177 + 4661);
        assert_eq!(song.total_delay_ms(), 177 + 4661);
        assert_eq!(
            song.instruction(1).unwrap(),
            DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms: 177
            }
        );
        assert_eq!(
            song.instruction(5).unwrap(),
            DroInstruction::Register {
                reg: 0x01,
                value: 0xFF,
                bank: None
            }
        );
    }

    /// Old rips (adplug's `samurai.dro`) wrote the OPL type as a single byte.
    #[test]
    fn read_a_v1_file_with_a_one_byte_opl_type() {
        let song = read("test.dro", &v1_bytes(true)).unwrap();
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.len(), 7);
        assert_eq!(song.total_delay_ms(), 177 + 4661);
    }

    #[test]
    fn v1_opl_type_uses_the_v1_code_ordering() {
        // v1 orders the types (OPL2, OPL3, DUAL_OPL2), unlike v2.
        for (code, expected) in [
            (0u8, OplType::Opl2),
            (1, OplType::Opl3),
            (2, OplType::DualOpl2),
        ] {
            let mut bytes = v1_bytes(false);
            bytes[0x14] = code;
            assert_eq!(read("t.dro", &bytes).unwrap().opl_type, expected);
        }
    }

    #[test]
    fn v1_round_trips_byte_for_byte() {
        let original = v1_bytes(false);
        let song = read("t.dro", &original).unwrap();
        assert_eq!(write(&song).unwrap(), original);
    }

    /// A one-byte OPL type is upgraded to four on write. The instructions survive
    /// intact.
    #[test]
    fn v1_char_opl_type_is_upgraded_on_write() {
        let song = read("t.dro", &v1_bytes(true)).unwrap();
        let written = write(&song).unwrap();
        assert_eq!(written, v1_bytes(false));
        assert_eq!(written.len(), v1_bytes(true).len() + 3);
    }

    /// The v1 writer takes the length from the data, not from `ms_length`, because
    /// V1 and V2 files write this value differently.
    #[test]
    fn v1_writer_recomputes_the_total_delay_and_ignores_the_header() {
        let mut song = read("t.dro", &v1_bytes(false)).unwrap();
        song.ms_length = 999_999; // a header that lies

        let written = write(&song).unwrap();
        let ms_length = u32::from_le_bytes(written[0x0C..0x10].try_into().unwrap());
        assert_eq!(ms_length, 177 + 4661, "the writer must not trust ms_length");

        // ... and trimming a delay is reflected in the saved header.
        let mut undo = UndoController::new();
        undo.execute(Box::new(DeleteInstructions::new([1])), &mut song); // the 177 ms delay

        let written = write(&song).unwrap();
        let ms_length = u32::from_le_bytes(written[0x0C..0x10].try_into().unwrap());
        let byte_length = u32::from_le_bytes(written[0x10..0x14].try_into().unwrap());
        assert_eq!(ms_length, 4661);
        assert_eq!(byte_length, 12);

        let reread = read("t.dro", &written).unwrap();
        assert_eq!(reread.len(), 6);
        assert_eq!(reread.total_delay_ms(), 4661);
    }

    /// The v2 writer, by contrast, writes `ms_length` verbatim -- which is what
    /// makes the fixture round-trip byte for byte.
    #[test]
    fn v2_writer_preserves_the_header_ms_length() {
        let mut song = read("f.dro", DRO_V2_FIXTURE).unwrap();
        song.ms_length = 12_345;
        let written = write(&song).unwrap();
        assert_eq!(
            u32::from_le_bytes(written[0x10..0x14].try_into().unwrap()),
            12_345
        );
    }

    #[test]
    fn v1_accepts_the_old_version_pair() {
        let mut bytes = v1_bytes(false);
        bytes[0x08..0x0C].copy_from_slice(&[1, 0, 0, 0]); // (1, 0) instead of (0, 1)
        assert_eq!(read("t.dro", &bytes).unwrap().len(), 7);
    }

    /// Trailing bytes are tolerated: warn and carry on.
    #[test]
    fn v1_tolerates_trailing_bytes() {
        let mut bytes = v1_bytes(false);
        bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let song = read("t.dro", &bytes).unwrap();
        assert_eq!(song.len(), 7);
    }

    #[test]
    fn v1_tolerates_a_truncated_final_instruction() {
        let mut bytes = v1_bytes(false);
        // Extend the declared length by one and add half of a register write.
        let declared = u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap());
        bytes[0x10..0x14].copy_from_slice(&(declared + 1).to_le_bytes());
        bytes.push(0x40);
        let song = read("t.dro", &bytes).unwrap();
        assert_eq!(song.len(), 7, "the half instruction is dropped");
    }

    #[test]
    fn v1_rejects_a_short_file() {
        let mut bytes = v1_bytes(false);
        let declared = u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap());
        bytes[0x10..0x14].copy_from_slice(&(declared + 100).to_le_bytes());
        assert!(
            read("t.dro", &bytes)
                .unwrap_err()
                .to_string()
                .contains("only")
        );
    }

    // -- container ---------------------------------------------------------

    #[test]
    fn rejects_a_bad_magic() {
        let error = read("t.dro", b"NOTADROFILE...").unwrap_err().to_string();
        assert!(
            error.contains("Does not appear to be a DRO file"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_unsupported_version() {
        let mut bytes = v1_bytes(false);
        bytes[0x08..0x0C].copy_from_slice(&[3, 0, 0, 0]);
        let error = read("t.dro", &bytes).unwrap_err().to_string();
        assert!(error.contains("Unsupported version"), "{error}");
    }

    #[test]
    fn rejects_an_empty_file() {
        assert!(read("t.dro", &[]).is_err());
        assert!(read("t.dro", MAGIC).is_err());
    }
}
