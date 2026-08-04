//! Readers and writers. Bytes in, bytes out -- never paths.
//!
//! Locating and opening files belongs to the platform: the native shell hands us
//! `std::fs::read`'s output, the web shell hands us a `File`'s `ArrayBuffer`.
//! Keeping paths out of here is also what makes round-trip byte-equality tests
//! natural to write.

pub mod dro;

use crate::error::{Error, Result};
use crate::song::{Song, SongData};
use crate::vgm::io as vgm;

/// Reads a song, choosing the format from `name`'s extension.
///
/// A `.vgz` is detected from its gzip magic rather than its name, so a compressed
/// file with a `.vgm` extension opens.
///
/// # Errors
/// If the extension is not one we support, or the bytes do not parse as that format.
pub fn read_song(name: &str, bytes: &[u8]) -> Result<Song> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".dro") {
        dro::read(name, bytes)
    } else if lower.ends_with(".vgm") || lower.ends_with(".vgz") {
        vgm::read(name, bytes)
    } else {
        Err(Error::file(format!(
            "Tried to read an unsupported file format: {name}"
        )))
    }
}

/// Serialises a song in its own format, compressing if `name` ends in `.vgz`.
///
/// # Errors
/// If the song's data and its declared format disagree.
pub fn write_song(song: &Song) -> Result<Vec<u8>> {
    match song.data() {
        SongData::V1(_) | SongData::V2(_) => dro::write(song),
        SongData::Vgm(_) if song.name.to_ascii_lowercase().ends_with(".vgz") => {
            vgm::write_gzipped(song)
        }
        SongData::Vgm(_) => vgm::write(song),
    }
}

/// Reads `u8` / `u16` / `u32` little-endian values with bounds checks.
#[derive(Debug, Clone)]
pub(crate) struct ByteReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(crate) const fn offset(&self) -> usize {
        self.offset
    }

    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub(crate) fn seek(&mut self, offset: usize) -> Result<()> {
        if offset > self.bytes.len() {
            return Err(Error::file(format!(
                "cannot seek to offset {offset}: the file is only {} bytes",
                self.bytes.len()
            )));
        }
        self.offset = offset;
        Ok(())
    }

    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| Error::file("byte count overflowed"))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| Self::truncated(self.offset, count, self.bytes.len()))?;
        self.offset = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16_le(&mut self) -> Result<u16> {
        let bytes: [u8; 2] = self.take(2)?.try_into().expect("take(2) yields 2 bytes");
        Ok(u16::from_le_bytes(bytes))
    }

    pub(crate) fn u32_le(&mut self) -> Result<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("take(4) yields 4 bytes");
        Ok(u32::from_le_bytes(bytes))
    }

    fn truncated(offset: usize, wanted: usize, total: usize) -> Error {
        Error::file(format!(
            "file ends unexpectedly: wanted {wanted} bytes at offset {offset}, but the file is \
             only {total} bytes"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRO_V2_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up_dro2.dro");
    const VGM_FIXTURE: &[u8] = include_bytes!("../../../../tests/lsl3_score_up.vgm");

    #[test]
    fn read_song_dispatches_on_the_extension() {
        assert_eq!(read_song("a.dro", DRO_V2_FIXTURE).unwrap().len(), 299);
        assert_eq!(read_song("a.DRO", DRO_V2_FIXTURE).unwrap().len(), 299);
        assert_eq!(read_song("a.vgm", VGM_FIXTURE).unwrap().len(), 299);
        assert_eq!(read_song("a.VGZ", VGM_FIXTURE).unwrap().len(), 299);

        let error = read_song("a.mid", VGM_FIXTURE).unwrap_err().to_string();
        assert!(error.contains("unsupported file format"), "{error}");
    }

    #[test]
    fn write_song_round_trips_every_format() {
        for (name, bytes) in [("a.dro", DRO_V2_FIXTURE), ("a.vgm", VGM_FIXTURE)] {
            let song = read_song(name, bytes).unwrap();
            assert_eq!(write_song(&song).unwrap(), bytes, "{name}");
        }
    }

    /// The editor's Load then Save is `read_song` then `write_song`: a DRO of
    /// either version comes back byte-for-byte and in its own format. v2 is the
    /// real capture; v1 is that same music as a canonical v1 file (the standard
    /// NEW version pair and four-byte OPL type). This pins the guarantee at the
    /// public boundary the Open/Save actions use, not just the `dro` module.
    #[test]
    fn a_dro_round_trips_byte_for_byte_through_load_and_save() {
        use crate::song::{DRO_FILE_V1, DRO_FILE_V2};

        // v2: the real fixture saves back byte-for-byte, still a v2 file.
        let v2 = read_song("song.dro", DRO_V2_FIXTURE).unwrap();
        assert_eq!(v2.file_version, DRO_FILE_V2);
        assert_eq!(
            write_song(&v2).unwrap(),
            DRO_V2_FIXTURE,
            "a v2 DRO must save byte-for-byte"
        );

        // v1: the same music as a canonical v1 DRO. Save it once to get the
        // canonical bytes, then confirm a fresh Load + Save reproduces them.
        let v1 = crate::convert::dro2_to_dro1(&v2).unwrap();
        let saved = write_song(&v1).unwrap();
        let reloaded = read_song("song.dro", &saved).unwrap();
        assert_eq!(reloaded.file_version, DRO_FILE_V1, "a v1 DRO stays v1");
        assert_eq!(
            write_song(&reloaded).unwrap(),
            saved,
            "a v1 DRO must save byte-for-byte"
        );
    }

    #[test]
    fn write_song_compresses_a_vgz_by_name() {
        let mut song = read_song("a.vgm", VGM_FIXTURE).unwrap();
        assert!(!vgm::is_gzipped(&write_song(&song).unwrap()));

        song.name = "a.vgz".to_owned();
        let compressed = write_song(&song).unwrap();
        assert!(vgm::is_gzipped(&compressed));
        assert_eq!(read_song("a.vgz", &compressed).unwrap().data(), song.data());
    }

    #[test]
    fn byte_reader_bounds_check_everything() {
        let mut reader = ByteReader::new(&[1, 2, 3]);
        assert_eq!(reader.u8().unwrap(), 1);
        assert_eq!(reader.remaining(), 2);
        assert_eq!(reader.u16_le().unwrap(), 0x0302);
        assert_eq!(reader.remaining(), 0);
        assert!(reader.u8().is_err());

        let mut reader = ByteReader::new(&[1, 2, 3]);
        assert!(reader.u32_le().is_err());
        assert_eq!(reader.offset(), 0, "a failed read does not advance");
        assert!(reader.seek(4).is_err());
        assert!(reader.seek(3).is_ok());
        assert!(reader.take(usize::MAX).is_err());
    }
}
