//! The VGM command stream, its GD3 tag, and the header fields we model.
//!
//! Ported from `vgm/vgm_data.py`.

use crate::error::{Error, Result};
use crate::song::instruction::{Bank, DelayKind, DroInstruction};
use crate::song::splice::{InsertEntry, byte_ranges_to_delete, splice_in, splice_out};

/// The VGM commands this app understands. Anything else is a hard error, as in
/// the Python -- a trimmer must not silently drop data it cannot re-encode.
pub mod command {
    /// `0x5A aa dd` -- YM3812 (OPL2), write `dd` to register `aa`.
    pub const YM3812: u8 = 0x5A;
    /// `0x5E aa dd` -- YMF262 (OPL3) port 0.
    pub const YMF262_PORT_0: u8 = 0x5E;
    /// `0x5F aa dd` -- YMF262 (OPL3) port 1.
    pub const YMF262_PORT_1: u8 = 0x5F;
    /// `0xAA aa dd` -- YM3812, second chip (dual OPL2).
    pub const YM3812_CHIP_2: u8 = 0xAA;
    /// `0x61 nn nn` -- wait `nn nn` samples, 0..=65535.
    pub const WAIT: u8 = 0x61;
    /// `0x62` -- wait 735 samples (a 60th of a second).
    pub const WAIT_60TH: u8 = 0x62;
    /// `0x63` -- wait 882 samples (a 50th of a second).
    pub const WAIT_50TH: u8 = 0x63;
    /// `0x66` -- end of sound data. Not stored in the stream.
    pub const END: u8 = 0x66;
    /// `0x70..=0x7F` -- wait `n + 1` samples, 1..=16.
    pub const SHORT_WAIT_BASE: u8 = 0x70;
    pub const SHORT_WAIT_LAST: u8 = 0x7F;

    /// Samples waited by `0x62`.
    pub const SAMPLES_60TH: u32 = 735;
    /// Samples waited by `0x63`.
    pub const SAMPLES_50TH: u32 = 882;
}

/// The VGM command stream, minus its `0x66` end marker.
///
/// Commands are variable length, so a logical index needs a lookup table -- the
/// same shape as [`DroDataV1`](crate::song::DroDataV1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmData {
    data: Vec<u8>,
    offsets: Vec<u32>,
}

impl VgmData {
    /// Wraps a command stream, indexing it.
    ///
    /// # Errors
    /// If a command is unrecognised or its operands run past the end.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let offsets = Self::build_offsets(&data)?;
        Ok(Self { data, offsets })
    }

    fn build_offsets(data: &[u8]) -> Result<Vec<u32>> {
        let mut offsets = Vec::new();
        let mut offset = 0usize;
        while offset < data.len() {
            let size = Self::command_size(data[offset])?;
            if offset + size > data.len() {
                return Err(Error::file(format!(
                    "VGM data ends mid-command: {:#04X} at byte {offset} needs {size} bytes but \
                     only {} remain",
                    data[offset],
                    data.len() - offset,
                )));
            }
            offsets.push(
                u32::try_from(offset)
                    .map_err(|_| Error::file("VGM data is larger than 4 GiB".to_owned()))?,
            );
            offset += size;
        }
        Ok(offsets)
    }

    /// Reads a raw file stream up to its `0x66` end marker, indexing it in the same
    /// pass. This is the file read path: [`io::read`](crate::vgm::io) hands over the
    /// bytes from the data offset onward and gets back the stored stream (the end
    /// marker dropped) already indexed -- one walk, where copying the commands and
    /// then re-deriving their offsets used to be two.
    ///
    /// # Errors
    /// If a command is unrecognised or its operands run past the end of the stream.
    pub(crate) fn read_from_stream(bytes: &[u8]) -> Result<Self> {
        let mut offsets = Vec::new();
        let mut offset = 0usize;
        let end = loop {
            if offset >= bytes.len() {
                log::warn!("VGM data has no 0x66 end-of-data marker");
                break bytes.len();
            }
            if bytes[offset] == command::END {
                break offset;
            }
            let size = Self::command_size(bytes[offset])?;
            if offset + size > bytes.len() {
                return Err(Error::file(format!(
                    "VGM data ends mid-command: {:#04X} at byte {offset}",
                    bytes[offset]
                )));
            }
            offsets.push(
                u32::try_from(offset)
                    .map_err(|_| Error::file("VGM data is larger than 4 GiB".to_owned()))?,
            );
            offset += size;
        };
        Ok(Self {
            data: bytes[..end].to_vec(),
            offsets,
        })
    }

    /// The total length of the command at `opcode`, operands included.
    ///
    /// # Errors
    /// If `opcode` is not a command this app understands.
    pub fn command_size(opcode: u8) -> Result<usize> {
        use command::*;
        Ok(match opcode {
            YM3812 | YMF262_PORT_0 | YMF262_PORT_1 | YM3812_CHIP_2 | WAIT => 3,
            WAIT_60TH | WAIT_50TH => 1,
            SHORT_WAIT_BASE..=SHORT_WAIT_LAST => 1,
            _ => {
                return Err(Error::file(format!(
                    "Unsupported VGM command: {opcode:#04x}"
                )));
            }
        })
    }

    /// The command stream, exactly as it sits in the file (no `0x66`).
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// The byte offset of command `index`. `index == len()` yields the end.
    pub(crate) fn byte_offset(&self, index: usize) -> usize {
        match self.offsets.get(index) {
            Some(&offset) => offset as usize,
            None if index == self.len() => self.data.len(),
            None => panic!("command index {index} out of range (len {})", self.len()),
        }
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<DroInstruction> {
        use command::*;
        let start = *self.offsets.get(index)? as usize;
        let opcode = self.data[start];
        let byte = |offset: usize| self.data[start + offset];

        let register = |bank: Bank| DroInstruction::Register {
            reg: byte(1),
            value: byte(2),
            bank: Some(bank),
        };
        let wait = |kind: DelayKind, samples: u32| DroInstruction::DelaySamples { kind, samples };

        Some(match opcode {
            YM3812 | YMF262_PORT_0 => register(Bank::Low),
            YMF262_PORT_1 | YM3812_CHIP_2 => register(Bank::High),
            WAIT => wait(
                DelayKind::Long,
                u32::from(byte(1)) | (u32::from(byte(2)) << 8),
            ),
            WAIT_60TH => wait(DelayKind::Short, SAMPLES_60TH),
            WAIT_50TH => wait(DelayKind::Short, SAMPLES_50TH),
            short @ SHORT_WAIT_BASE..=SHORT_WAIT_LAST => {
                wait(DelayKind::Short, u32::from(short & 0x0F) + 1)
            }
            _ => unreachable!("`new` rejected every other opcode"),
        })
    }

    #[must_use]
    pub fn raw_instruction(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len() {
            return None;
        }
        Some(&self.data[self.byte_offset(index)..self.byte_offset(index + 1)])
    }

    /// The index of the command that *starts* at `byte_offset`.
    ///
    /// `None` if the offset is past the end or falls inside a command. VGM's loop
    /// pointer is a byte offset, and this is how it is resolved to something that
    /// can survive an edit.
    #[must_use]
    pub fn index_at_byte_offset(&self, byte_offset: usize) -> Option<usize> {
        let byte_offset = u32::try_from(byte_offset).ok()?;
        self.offsets.binary_search(&byte_offset).ok()
    }

    pub(crate) fn delete_many(&mut self, indices: &[usize]) {
        let Some(byte_ranges) = byte_ranges_to_delete(indices, self.len(), |i| self.byte_offset(i))
        else {
            return;
        };
        splice_out(&mut self.data, &byte_ranges);
        self.offsets =
            Self::build_offsets(&self.data).expect("deleting whole commands cannot corrupt them");
    }

    pub(crate) fn insert_many(&mut self, entries: &[InsertEntry]) {
        if entries.is_empty() {
            return;
        }
        let spliced = splice_in(&self.data, entries, |i| self.byte_offset(i));
        self.data = spliced;
        self.offsets = Self::build_offsets(&self.data)
            .expect("re-inserting whole commands cannot corrupt them");
    }
}

/// A GD3 tag: eleven strings, in this order.
///
/// Stored as UTF-16LE with two-byte null terminators. Rust strings are UTF-8, so
/// they are transcoded on the way in and out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gd3Tag {
    pub track_name_en: String,
    pub track_name_native: String,
    pub game_name_en: String,
    pub game_name_native: String,
    pub system_name_en: String,
    pub system_name_native: String,
    pub track_author_en: String,
    pub track_author_native: String,
    pub release_date: String,
    pub creator: String,
    pub notes: String,
}

/// The eleven GD3 fields must be written in exactly this order.
pub const GD3_FIELD_COUNT: usize = 11;

impl Gd3Tag {
    /// The fields, in file order.
    #[must_use]
    pub fn fields(&self) -> [&str; GD3_FIELD_COUNT] {
        [
            &self.track_name_en,
            &self.track_name_native,
            &self.game_name_en,
            &self.game_name_native,
            &self.system_name_en,
            &self.system_name_native,
            &self.track_author_en,
            &self.track_author_native,
            &self.release_date,
            &self.creator,
            &self.notes,
        ]
    }

    /// Builds a tag from the eleven fields, in file order.
    #[must_use]
    pub fn from_fields(fields: [String; GD3_FIELD_COUNT]) -> Self {
        let [
            track_name_en,
            track_name_native,
            game_name_en,
            game_name_native,
            system_name_en,
            system_name_native,
            track_author_en,
            track_author_native,
            release_date,
            creator,
            notes,
        ] = fields;
        Self {
            track_name_en,
            track_name_native,
            game_name_en,
            game_name_native,
            system_name_en,
            system_name_native,
            track_author_en,
            track_author_native,
            release_date,
            creator,
            notes,
        }
    }
}

/// VGM header fields that DRO songs have no equivalent for.
///
/// `header` holds the file's own header bytes verbatim. Writing copies them and
/// patches only the fields that can have changed, so a read-then-write of an
/// unedited file reproduces it exactly -- including the chip clocks, the `rate`
/// field, and any v1.70 extra-header offset we do not otherwise model. The Python
/// discarded all of that, emitting a fresh 0x100-byte header every time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmMeta {
    /// The command the loop restarts at, or `None` if the file does not loop.
    ///
    /// The file stores this as a *byte* offset, which trimming invalidates: delete
    /// a command before the loop and every later byte shifts. Holding an
    /// instruction index instead lets edits move it, and the writer converts back.
    ///
    /// The matching `loop # samples` field is not stored at all -- it is
    /// [`Song::loop_num_samples`](crate::Song::loop_num_samples), derived from the
    /// command stream and [`Self::loop_end`], so trimming inside the loop cannot
    /// leave it stale.
    pub loop_point: Option<usize>,
    /// Where the loop stops, as an **exclusive** instruction index, or `None` for
    /// the end of the song.
    ///
    /// VGM has no loop-end field. The header carries `loop # samples`, which the
    /// spec defines as the wait total from the loop point to the end of the file,
    /// and that is exactly what a `None` here writes. Holding an end index lets
    /// the editor express a loop that stops short of the tail, and the writer
    /// emits that region's length in the same field -- so it survives a save and
    /// a reload.
    ///
    /// Be aware that other players restart at the end-of-data command regardless
    /// of the declared length, so a `Some(end)` short of the song's end is
    /// honoured here but not elsewhere; trimming the tail is what makes it
    /// universal.
    ///
    /// Only meaningful alongside a [`Self::loop_point`], and always strictly
    /// greater than it.
    pub loop_end: Option<usize>,
    pub loop_base: u8,
    pub loop_modifier: u8,
    pub volume_modifier: u8,
    pub tag: Option<Gd3Tag>,
    pub(crate) header: Vec<u8>,
}

impl VgmMeta {
    /// A header for a song that has no loop, no tag, and default modifiers.
    #[must_use]
    pub fn new(header: Vec<u8>) -> Self {
        Self {
            loop_point: None,
            loop_end: None,
            loop_base: 0,
            loop_modifier: 0,
            volume_modifier: 0,
            tag: None,
            header,
        }
    }

    /// The file's header bytes, from the magic up to the start of the command stream.
    #[must_use]
    pub fn header(&self) -> &[u8] {
        &self.header
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `test_vgm_data.py::create_vgm_data`: five OPL2 writes, a long wait, a
    /// one-sample short wait -- twice over.
    pub(crate) fn vgm_fixture() -> VgmData {
        let mut data = Vec::new();
        for i in 0..5u8 {
            data.extend_from_slice(&[command::YM3812, i * 2, i * 2 + 1]);
        }
        data.extend_from_slice(&[command::WAIT, 0xB0, 0x00]);
        data.push(command::SHORT_WAIT_BASE);
        data.extend_from_within(..);
        VgmData::new(data).unwrap()
    }

    #[test]
    fn len_and_raw_len() {
        let data = vgm_fixture();
        assert_eq!(data.len(), 14);
        assert_eq!(data.raw().len(), 10 * 3 + 4 * 2);
    }

    #[test]
    fn byte_offsets() {
        let data = vgm_fixture();
        assert_eq!(data.byte_offset(0), 0);
        assert_eq!(data.byte_offset(1), 3);
        assert_eq!(data.byte_offset(5), 5 * 3);
        assert_eq!(data.byte_offset(14), data.raw().len());
    }

    #[test]
    fn decode() {
        let data = vgm_fixture();
        let reg = |reg, value| DroInstruction::Register {
            reg,
            value,
            bank: Some(Bank::Low),
        };
        assert_eq!(data.get(0), Some(reg(0x00, 0x01)));
        assert_eq!(data.get(1), Some(reg(0x02, 0x03)));
        assert_eq!(data.get(2), Some(reg(0x04, 0x05)));
        assert_eq!(
            data.get(5),
            Some(DroInstruction::DelaySamples {
                kind: DelayKind::Long,
                samples: 0xB0
            })
        );
        assert_eq!(
            data.get(6),
            Some(DroInstruction::DelaySamples {
                kind: DelayKind::Short,
                samples: 1
            })
        );
        assert_eq!(data.get(14), None);
    }

    #[test]
    fn decode_every_command() {
        let data = VgmData::new(vec![
            command::YM3812,
            0x20,
            0x01,
            command::YMF262_PORT_0,
            0x21,
            0x02,
            command::YMF262_PORT_1,
            0x22,
            0x03,
            command::YM3812_CHIP_2,
            0x23,
            0x04,
            command::WAIT,
            0x34,
            0x12,
            command::WAIT_60TH,
            command::WAIT_50TH,
            0x70,
            0x7F,
        ])
        .unwrap();

        let low = |reg, value| DroInstruction::Register {
            reg,
            value,
            bank: Some(Bank::Low),
        };
        let high = |reg, value| DroInstruction::Register {
            reg,
            value,
            bank: Some(Bank::High),
        };
        let long = |samples| DroInstruction::DelaySamples {
            kind: DelayKind::Long,
            samples,
        };
        let short = |samples| DroInstruction::DelaySamples {
            kind: DelayKind::Short,
            samples,
        };

        assert_eq!(data.get(0), Some(low(0x20, 0x01)));
        assert_eq!(
            data.get(1),
            Some(low(0x21, 0x02)),
            "YMF262 port 0 is the low bank"
        );
        assert_eq!(
            data.get(2),
            Some(high(0x22, 0x03)),
            "YMF262 port 1 is the high bank"
        );
        assert_eq!(
            data.get(3),
            Some(high(0x23, 0x04)),
            "the second YM3812 is the high bank"
        );
        assert_eq!(data.get(4), Some(long(0x1234)), "little-endian wait");
        assert_eq!(data.get(5), Some(short(735)));
        assert_eq!(data.get(6), Some(short(882)));
        assert_eq!(data.get(7), Some(short(1)), "0x70 waits one sample");
        assert_eq!(data.get(8), Some(short(16)), "0x7F waits sixteen samples");
        assert_eq!(data.len(), 9);
    }

    #[test]
    fn rejects_an_unknown_command() {
        let error = VgmData::new(vec![0x50, 0x00]).unwrap_err().to_string();
        assert!(error.contains("Unsupported VGM command: 0x50"), "{error}");
        // The end marker never appears in the stream -- the reader stops at it.
        assert!(VgmData::new(vec![command::END]).is_err());
    }

    #[test]
    fn rejects_a_truncated_command() {
        assert!(VgmData::new(vec![command::WAIT, 0x34]).is_err());
        assert!(VgmData::new(vec![command::YM3812]).is_err());
    }

    #[test]
    fn raw_instruction() {
        let data = vgm_fixture();
        assert_eq!(data.raw_instruction(0), Some(&[0x5A, 0x00, 0x01][..]));
        assert_eq!(data.raw_instruction(5), Some(&[0x61, 0xB0, 0x00][..]));
        assert_eq!(data.raw_instruction(6), Some(&[0x70][..]));
        assert_eq!(data.raw_instruction(14), None);
    }

    /// Python's `test_del`, restated: `del dro_data[1:2]` removed logical 1 *and* 2.
    #[test]
    fn delete_matches_the_python_expectations() {
        let mut data = vgm_fixture();
        data.delete_many(&[0]);
        assert_eq!(data.len(), 13);
        assert_eq!(data.raw().len(), (5 * 3 + 4) * 2 - 3);
        assert_eq!(
            &data.raw()[..9],
            [0x5A, 0x02, 0x03, 0x5A, 0x04, 0x05, 0x5A, 0x06, 0x07]
        );

        data.delete_many(&[1, 2]);
        assert_eq!(data.len(), 11);
        assert_eq!(data.raw().len(), (5 * 3 + 4) * 2 - 3 * 3);
        assert_eq!(
            &data.raw()[..9],
            [0x5A, 0x02, 0x03, 0x5A, 0x08, 0x09, 0x61, 0xB0, 0x00]
        );
    }

    /// The Python's `VGMData.insert_multiple` is a copy of `DRODataV1`'s, complete
    /// with the unsorted-input defect. The shared splice sorts at the boundary.
    #[test]
    fn delete_then_insert_round_trips_for_a_fragmented_selection() {
        let original = vgm_fixture();
        for selection in [
            vec![0],
            vec![13],
            vec![1, 6, 3, 4],
            vec![0, 2, 4, 6, 8, 10, 12],
            (0..14).collect::<Vec<_>>(),
        ] {
            let entries: Vec<InsertEntry> = {
                let mut sorted = selection.clone();
                sorted.sort_unstable();
                sorted.dedup();
                sorted
                    .iter()
                    .map(|&i| {
                        (
                            i,
                            original
                                .raw_instruction(i)
                                .unwrap()
                                .to_vec()
                                .into_boxed_slice(),
                        )
                    })
                    .collect()
            };
            let mut data = original.clone();
            data.delete_many(&selection);
            data.insert_many(&entries);
            assert_eq!(data, original, "selection {selection:?}");
        }
    }

    #[test]
    fn delete_all_leaves_nothing() {
        let mut data = vgm_fixture();
        data.delete_many(&(0..14).collect::<Vec<_>>());
        assert!(data.is_empty());
        assert_eq!(data.raw().len(), 0);
    }

    #[test]
    fn out_of_range_and_duplicate_indices_are_ignored() {
        let mut data = vgm_fixture();
        data.delete_many(&[99, 0, 0, 1000]);
        assert_eq!(data.len(), 13);
    }

    #[test]
    fn gd3_fields_round_trip_through_from_fields() {
        let fields: [String; GD3_FIELD_COUNT] = core::array::from_fn(|i| format!("field {i}"));
        let tag = Gd3Tag::from_fields(fields.clone());
        let borrowed: Vec<&str> = fields.iter().map(String::as_str).collect();
        assert_eq!(tag.fields().to_vec(), borrowed);
        assert_eq!(tag.track_name_en, "field 0");
        assert_eq!(tag.notes, "field 10");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::tests::vgm_fixture;
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// VGM commands are variable length, like DRO v1's instructions, so the
        /// splice has to get the offsets right. Delete then undo, for any selection.
        #[test]
        fn delete_then_insert_restores_the_original(
            selection in proptest::collection::vec(0..14usize, 0..=14)
        ) {
            let original = vgm_fixture();
            let mut sorted = selection.clone();
            sorted.sort_unstable();
            sorted.dedup();

            let entries: Vec<InsertEntry> = sorted
                .iter()
                .map(|&i| (i, original.raw_instruction(i).unwrap().to_vec().into_boxed_slice()))
                .collect();

            let mut data = original.clone();
            data.delete_many(&selection);
            prop_assert_eq!(data.len(), 14 - sorted.len());
            data.insert_many(&entries);
            prop_assert_eq!(&data, &original);
        }

        /// The single-pass compaction must be byte-identical to deleting one
        /// command at a time, back to front.
        #[test]
        fn single_pass_delete_matches_a_naive_reference(
            selection in proptest::collection::vec(0..14usize, 0..=14)
        ) {
            let original = vgm_fixture();
            let mut sorted = selection.clone();
            sorted.sort_unstable();
            sorted.dedup();

            let mut expected = original.raw().to_vec();
            for &index in sorted.iter().rev() {
                expected.drain(original.byte_offset(index)..original.byte_offset(index + 1));
            }

            let mut actual = original.clone();
            actual.delete_many(&selection);
            prop_assert_eq!(actual.raw(), expected.as_slice());
        }
    }
}
