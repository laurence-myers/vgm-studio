//! The raw DRO byte array, and the index translation each version needs.
//!
//! Both versions store the instruction stream exactly as it appears in the file,
//! so writing is a memcpy and round-trips are byte-exact. Instructions are
//! decoded on access (see [`DroInstruction`]); nothing is materialised.

use core::ops::Range;

use crate::error::{Error, Result};
use crate::song::instruction::{Bank, DelayKind, DroInstruction};
use crate::util::condense_ranges;

/// A restored instruction: its logical index, and its raw bytes.
pub type InsertEntry = (usize, Box<[u8]>);

/// The DRO v1 delay opcodes, which double as its register-escape opcodes.
mod v1_opcode {
    pub(super) const SHORT_DELAY: u8 = 0x00;
    pub(super) const LONG_DELAY: u8 = 0x01;
    pub(super) const BANK_LOW: u8 = 0x02;
    pub(super) const BANK_HIGH: u8 = 0x03;
    /// Escape: the next byte is a register number that would otherwise collide
    /// with one of the opcodes above.
    pub(super) const ESCAPE: u8 = 0x04;
}

/// The DRO instruction stream, in whichever version's encoding it was read.
///
/// The Python original modelled this as an abstract base class with two
/// subclasses. A closed enum is a better fit: there are exactly two encodings,
/// and dispatch on the table-paint path becomes a jump rather than a vtable call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DroData {
    V1(DroDataV1),
    V2(DroDataV2),
}

impl DroData {
    /// The number of instructions.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::V1(data) => data.len(),
            Self::V2(data) => data.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Decodes the instruction at `index`, or `None` if out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<DroInstruction> {
        match self {
            Self::V1(data) => data.get(index),
            Self::V2(data) => data.get(index),
        }
    }

    /// Iterates the decoded instructions. Allocates nothing.
    pub fn iter(&self) -> impl Iterator<Item = DroInstruction> + '_ {
        (0..self.len()).map(move |index| {
            self.get(index)
                .expect("index map and raw bytes agree by construction")
        })
    }

    /// The whole instruction stream, exactly as it sits in the file.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        match self {
            Self::V1(data) => &data.data,
            Self::V2(data) => &data.data,
        }
    }

    #[must_use]
    pub fn raw_len(&self) -> usize {
        self.raw().len()
    }

    /// The raw bytes of one instruction. This is what undo captures.
    #[must_use]
    pub fn raw_instruction(&self, index: usize) -> Option<&[u8]> {
        match self {
            Self::V1(data) => data.raw_instruction(index),
            Self::V2(data) => data.raw_instruction(index),
        }
    }

    /// Removes the instructions at `indices` in a single compaction pass.
    ///
    /// `indices` need not be sorted or unique. Where the Python did one `del` per
    /// contiguous range -- `O(k*n)` for `k` ranges -- this is `O(n)` regardless of
    /// how fragmented the selection is.
    pub fn delete_many(&mut self, indices: &[usize]) {
        match self {
            Self::V1(data) => data.delete_many(indices),
            Self::V2(data) => data.delete_many(indices),
        }
    }

    /// Re-inserts previously deleted instructions at their original indices.
    ///
    /// `entries` must be sorted ascending by index, as [`Self::delete_many`]
    /// captured them. This is the exact inverse of a `delete_many` with the same
    /// indices.
    pub fn insert_many(&mut self, entries: &[InsertEntry]) {
        match self {
            Self::V1(data) => data.insert_many(entries),
            Self::V2(data) => data.insert_many(entries),
        }
    }
}

// ---------------------------------------------------------------------------
// DRO v1
// ---------------------------------------------------------------------------

/// DRO v1: variable-length instructions, so a logical index needs a lookup table.
///
/// The Python kept a `list[int]` of boxed integers; a `Vec<u32>` is roughly ten
/// times smaller and cache-friendly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroDataV1 {
    data: Vec<u8>,
    index_map: Vec<u32>,
}

impl DroDataV1 {
    /// Wraps a v1 instruction stream, building the logical-to-byte index map.
    ///
    /// # Errors
    /// If the stream ends in the middle of an instruction. The Python original
    /// silently produced an index-map entry pointing at the truncated tail, and
    /// then raised `IndexError` the first time anything read it.
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let index_map = Self::build_index_map(&data)?;
        Ok(Self { data, index_map })
    }

    fn build_index_map(data: &[u8]) -> Result<Vec<u32>> {
        let mut index_map = Vec::new();
        let mut offset = 0usize;
        while offset < data.len() {
            let size = Self::instruction_size(data[offset]);
            if offset + size > data.len() {
                return Err(Error::file(format!(
                    "DRO v1 data ends mid-instruction: opcode {:#04X} at byte {offset} needs \
                     {size} bytes but only {} remain",
                    data[offset],
                    data.len() - offset,
                )));
            }
            index_map.push(
                u32::try_from(offset)
                    .map_err(|_| Error::file("DRO v1 data is larger than 4 GiB".to_owned()))?,
            );
            offset += size;
        }
        Ok(index_map)
    }

    const fn instruction_size(opcode: u8) -> usize {
        match opcode {
            v1_opcode::SHORT_DELAY => 2,
            v1_opcode::LONG_DELAY => 3,
            v1_opcode::BANK_LOW | v1_opcode::BANK_HIGH => 1,
            v1_opcode::ESCAPE => 3,
            _ => 2,
        }
    }

    fn len(&self) -> usize {
        self.index_map.len()
    }

    /// The byte offset of instruction `index`. `index == len()` yields the end.
    fn byte_offset(&self, index: usize) -> usize {
        match self.index_map.get(index) {
            Some(&offset) => offset as usize,
            None if index == self.len() => self.data.len(),
            None => panic!(
                "instruction index {index} out of range (len {})",
                self.len()
            ),
        }
    }

    fn get(&self, index: usize) -> Option<DroInstruction> {
        let start = *self.index_map.get(index)? as usize;
        let opcode = self.data[start];
        let byte = |offset: usize| self.data[start + offset];
        Some(match opcode {
            v1_opcode::SHORT_DELAY => DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms: u32::from(byte(1)) + 1,
            },
            v1_opcode::LONG_DELAY => DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: (u32::from(byte(1)) | (u32::from(byte(2)) << 8)) + 1,
            },
            v1_opcode::BANK_LOW => DroInstruction::BankSwitch(Bank::Low),
            v1_opcode::BANK_HIGH => DroInstruction::BankSwitch(Bank::High),
            v1_opcode::ESCAPE => DroInstruction::Register {
                reg: byte(1),
                value: byte(2),
                bank: None,
            },
            reg => DroInstruction::Register {
                reg,
                value: byte(1),
                bank: None,
            },
        })
    }

    fn raw_instruction(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len() {
            return None;
        }
        Some(&self.data[self.byte_offset(index)..self.byte_offset(index + 1)])
    }

    fn delete_many(&mut self, indices: &[usize]) {
        let Some(byte_ranges) = byte_ranges_to_delete(indices, self.len(), |i| self.byte_offset(i))
        else {
            return;
        };
        splice_out(&mut self.data, &byte_ranges);
        self.index_map = Self::build_index_map(&self.data)
            .expect("deleting whole instructions cannot truncate one");
    }

    fn insert_many(&mut self, entries: &[InsertEntry]) {
        if entries.is_empty() {
            return;
        }
        let spliced = splice_in(&self.data, entries, |i| self.byte_offset(i));
        self.data = spliced;
        self.index_map = Self::build_index_map(&self.data)
            .expect("re-inserting whole instructions cannot truncate one");
    }
}

// ---------------------------------------------------------------------------
// DRO v2
// ---------------------------------------------------------------------------

/// DRO v2: every instruction is exactly two bytes, so the index map is a shift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroDataV2 {
    data: Vec<u8>,
    codemap: Vec<u8>,
    short_delay_code: u8,
    long_delay_code: u8,
}

impl DroDataV2 {
    /// Wraps a v2 instruction stream.
    ///
    /// # Errors
    /// If `data` has an odd length, if `codemap` is longer than 128 entries, or
    /// if any register code indexes past the end of the codemap. The Python
    /// original raised `IndexError` from deep inside `_interpret_data` on that
    /// last case, at paint time rather than at load time.
    pub fn new(
        data: Vec<u8>,
        codemap: Vec<u8>,
        short_delay_code: u8,
        long_delay_code: u8,
    ) -> Result<Self> {
        if data.len() % 2 != 0 {
            return Err(Error::file(format!(
                "DRO v2 data must be register/value pairs, found {} bytes",
                data.len()
            )));
        }
        if codemap.len() > 128 {
            return Err(Error::file(format!(
                "DRO v2 file has too many entries in the codemap. Maximum 128, found {}. \
                 Is the file corrupt?",
                codemap.len()
            )));
        }
        for pair in data.chunks_exact(2) {
            let code = pair[0];
            if code == short_delay_code || code == long_delay_code {
                continue;
            }
            let slot = usize::from(code & 0x7F);
            if slot >= codemap.len() {
                return Err(Error::file(format!(
                    "DRO v2 register code {code:#04X} indexes codemap slot {slot}, but the \
                     codemap has only {} entries. Is the file corrupt?",
                    codemap.len()
                )));
            }
        }
        Ok(Self {
            data,
            codemap,
            short_delay_code,
            long_delay_code,
        })
    }

    /// The register number each 7-bit code stands for.
    #[must_use]
    pub fn codemap(&self) -> &[u8] {
        &self.codemap
    }

    #[must_use]
    pub const fn short_delay_code(&self) -> u8 {
        self.short_delay_code
    }

    #[must_use]
    pub const fn long_delay_code(&self) -> u8 {
        self.long_delay_code
    }

    fn len(&self) -> usize {
        self.data.len() / 2
    }

    const fn byte_offset(index: usize) -> usize {
        index * 2
    }

    fn get(&self, index: usize) -> Option<DroInstruction> {
        if index >= self.len() {
            return None;
        }
        let start = Self::byte_offset(index);
        let code = self.data[start];
        let value = self.data[start + 1];
        Some(if code == self.short_delay_code {
            DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms: u32::from(value) + 1,
            }
        } else if code == self.long_delay_code {
            DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: (u32::from(value) + 1) << 8,
            }
        } else {
            DroInstruction::Register {
                // `new` proved every code indexes a real codemap slot.
                reg: self.codemap[usize::from(code & 0x7F)],
                value,
                bank: Some(Bank::from_bit(code >> 7)),
            }
        })
    }

    fn raw_instruction(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len() {
            return None;
        }
        let start = Self::byte_offset(index);
        Some(&self.data[start..start + 2])
    }

    fn delete_many(&mut self, indices: &[usize]) {
        let Some(byte_ranges) = byte_ranges_to_delete(indices, self.len(), Self::byte_offset)
        else {
            return;
        };
        splice_out(&mut self.data, &byte_ranges);
    }

    fn insert_many(&mut self, entries: &[InsertEntry]) {
        if entries.is_empty() {
            return;
        }
        let spliced = splice_in(&self.data, entries, Self::byte_offset);
        self.data = spliced;
    }
}

// ---------------------------------------------------------------------------
// Shared splice machinery
// ---------------------------------------------------------------------------

/// Turns an arbitrary selection into the byte ranges to remove.
///
/// Sorts and de-duplicates defensively: the Python `DRODataV1.insert_multiple`
/// silently produced garbage when handed an unsorted list, and the only reason
/// that never fired was that the wx list control happened to yield indices in
/// ascending order.
fn byte_ranges_to_delete(
    indices: &[usize],
    len: usize,
    byte_offset: impl Fn(usize) -> usize,
) -> Option<Vec<Range<usize>>> {
    let mut sorted: Vec<usize> = indices.iter().copied().filter(|&i| i < len).collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return None;
    }
    Some(
        condense_ranges(&sorted)
            .into_iter()
            .map(|range| byte_offset(*range.start())..byte_offset(*range.end() + 1))
            .collect(),
    )
}

/// Removes `byte_ranges` (ascending, disjoint) from `data` in one forward pass.
fn splice_out(data: &mut Vec<u8>, byte_ranges: &[Range<usize>]) {
    let Some(first) = byte_ranges.first() else {
        return;
    };
    let mut write = first.start;
    for (i, range) in byte_ranges.iter().enumerate() {
        let next_start = byte_ranges.get(i + 1).map_or(data.len(), |next| next.start);
        let survivors = range.end..next_start;
        let count = survivors.len();
        data.copy_within(survivors, write);
        write += count;
    }
    data.truncate(write);
}

/// Rebuilds `data` with `entries` re-inserted at their logical indices, in one
/// forward pass and one allocation.
fn splice_in(
    data: &[u8],
    entries: &[InsertEntry],
    byte_offset: impl Fn(usize) -> usize,
) -> Vec<u8> {
    debug_assert!(
        entries.windows(2).all(|w| w[0].0 < w[1].0),
        "insert entries must be sorted ascending by index and de-duplicated"
    );

    let extra: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    let mut out = Vec::with_capacity(data.len() + extra);

    // `emitted` counts instructions already written to `out`, so the next entry
    // needs `target - emitted` of the surviving instructions copied before it.
    let mut emitted = 0usize;
    let mut src_logical = 0usize;
    let mut src_byte = 0usize;

    for (target, bytes) in entries {
        let need = target
            .checked_sub(emitted)
            .expect("insert entries must be sorted ascending");
        let end_byte = byte_offset(src_logical + need);
        out.extend_from_slice(&data[src_byte..end_byte]);
        src_logical += need;
        src_byte = end_byte;
        emitted += need;

        out.extend_from_slice(bytes);
        emitted += 1;
    }
    out.extend_from_slice(&data[src_byte..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::fixtures::{dro_data_v1 as v1_fixture, dro_data_v2 as v2_fixture};

    #[test]
    fn v2_fixture_bytes_match_python() {
        let data = v2_fixture();
        assert_eq!(data.data.len(), 28);
        assert_eq!(
            &data.data[..14],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0xFE, 0xB0, 0xFF, 0xC0]
        );
        assert_eq!(&data.data[..14], &data.data[14..]);
    }

    #[test]
    fn v2_len_and_translate_index() {
        let data = v2_fixture();
        assert_eq!(data.len(), 14);
        assert_eq!(data.data.len(), 28);
        assert_eq!(DroDataV2::byte_offset(0), 0);
        assert_eq!(DroDataV2::byte_offset(1), 2);
        assert_eq!(DroDataV2::byte_offset(5), 10);
    }

    #[test]
    fn v2_decode() {
        let data = v2_fixture();
        let codemap = data.codemap().to_vec();

        assert_eq!(
            data.get(0),
            Some(DroInstruction::Register {
                reg: codemap[0],
                value: 0x01,
                bank: Some(Bank::Low)
            })
        );
        assert_eq!(
            data.get(1),
            Some(DroInstruction::Register {
                reg: codemap[2],
                value: 0x03,
                bank: Some(Bank::Low)
            })
        );
        assert_eq!(
            data.get(2),
            Some(DroInstruction::Register {
                reg: codemap[4],
                value: 0x05,
                bank: Some(Bank::Low)
            })
        );
        assert_eq!(
            data.get(5),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms: 0xB0 + 1
            })
        );
        assert_eq!(
            data.get(6),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: (0xC0 + 1) << 8
            })
        );
        assert_eq!(data.get(14), None);
    }

    #[test]
    fn v2_iter_yields_every_instruction() {
        let data = DroData::V2(v2_fixture());
        let instructions: Vec<_> = data.iter().collect();
        assert_eq!(instructions.len(), 14);
        assert_eq!(instructions[0], data.get(0).unwrap());
        assert_eq!(instructions[13], data.get(13).unwrap());
    }

    #[test]
    fn v2_rejects_odd_length() {
        assert!(DroDataV2::new(vec![1, 2, 3], vec![0x10], 0xFE, 0xFF).is_err());
    }

    #[test]
    fn v2_rejects_codes_past_the_codemap() {
        // Code 0x05 needs codemap slot 5, but the codemap has two entries.
        let err = DroDataV2::new(vec![0x05, 0x00], vec![0x10, 0x20], 0xFE, 0xFF).unwrap_err();
        assert!(matches!(err, Error::File(_)));
    }

    #[test]
    fn v2_rejects_oversized_codemap() {
        assert!(DroDataV2::new(vec![], vec![0; 129], 0xFE, 0xFF).is_err());
    }

    #[test]
    fn v2_delay_codes_win_over_register_codes() {
        // 0xFF has bit 7 set, but it is the long-delay code, so it is never a
        // high-bank register write.
        let data = DroDataV2::new(vec![0xFF, 0xC0], vec![0x10], 0xFE, 0xFF).unwrap();
        assert_eq!(
            data.get(0),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: 0xC100
            })
        );
    }

    #[test]
    fn v2_high_bank_register() {
        let data = DroDataV2::new(vec![0x82, 0x7F], vec![0x10, 0x20, 0x30], 0xFE, 0xFF).unwrap();
        assert_eq!(
            data.get(0),
            Some(DroInstruction::Register {
                reg: 0x30,
                value: 0x7F,
                bank: Some(Bank::High)
            })
        );
    }

    #[test]
    fn v2_raw_instruction() {
        let data = v2_fixture();
        assert_eq!(data.raw_instruction(0), Some(&[0x00, 0x01][..]));
        assert_eq!(data.raw_instruction(5), Some(&[0xFE, 0xB0][..]));
        assert_eq!(data.raw_instruction(13), Some(&[0xFF, 0xC0][..]));
        assert_eq!(data.raw_instruction(14), None);
    }

    #[test]
    fn v1_index_map_and_decode() {
        let data = v1_fixture();
        assert_eq!(data.len(), 7);
        assert_eq!(data.index_map, vec![0, 2, 4, 7, 8, 9, 12]);
        assert_eq!(data.data.len(), 14);

        assert_eq!(
            data.get(0),
            Some(DroInstruction::Register {
                reg: 0x20,
                value: 0x01,
                bank: None
            })
        );
        assert_eq!(
            data.get(1),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Short,
                ms: 177
            })
        );
        assert_eq!(
            data.get(2),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: 0x1234 + 1
            })
        );
        assert_eq!(data.get(3), Some(DroInstruction::BankSwitch(Bank::Low)));
        assert_eq!(data.get(4), Some(DroInstruction::BankSwitch(Bank::High)));
        assert_eq!(
            data.get(5),
            Some(DroInstruction::Register {
                reg: 0x01,
                value: 0xFF,
                bank: None
            })
        );
        assert_eq!(
            data.get(6),
            Some(DroInstruction::Register {
                reg: 0xBD,
                value: 0x20,
                bank: None
            })
        );
        assert_eq!(data.get(7), None);
    }

    #[test]
    fn v1_long_delay_is_little_endian() {
        let data = DroDataV1::new(vec![0x01, 0xFF, 0x00]).unwrap();
        assert_eq!(
            data.get(0),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: 0x00FF + 1
            })
        );
        let data = DroDataV1::new(vec![0x01, 0x00, 0xFF]).unwrap();
        assert_eq!(
            data.get(0),
            Some(DroInstruction::DelayMs {
                kind: DelayKind::Long,
                ms: 0xFF00 + 1
            })
        );
    }

    #[test]
    fn v1_rejects_a_truncated_tail() {
        // A long delay needs three bytes.
        assert!(DroDataV1::new(vec![0x20, 0x01, 0x01, 0x34]).is_err());
        // A register write needs two.
        assert!(DroDataV1::new(vec![0x20]).is_err());
        // Bank switches are one byte, so this is fine.
        assert!(DroDataV1::new(vec![0x02]).is_ok());
    }

    #[test]
    fn v1_raw_instruction_spans_the_whole_instruction() {
        let data = v1_fixture();
        assert_eq!(data.raw_instruction(0), Some(&[0x20, 0x01][..]));
        assert_eq!(data.raw_instruction(2), Some(&[0x01, 0x34, 0x12][..]));
        assert_eq!(data.raw_instruction(3), Some(&[0x02][..]));
        assert_eq!(data.raw_instruction(5), Some(&[0x04, 0x01, 0xFF][..]));
        assert_eq!(data.raw_instruction(6), Some(&[0xBD, 0x20][..]));
        assert_eq!(data.raw_instruction(7), None);
    }

    // -- deletion ---------------------------------------------------------

    /// The reference implementation: delete one index at a time, back to front.
    /// Whatever the fast path does, it must agree with this.
    fn naive_delete(data: &DroData, indices: &[usize]) -> Vec<u8> {
        let mut sorted: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < data.len())
            .collect();
        sorted.sort_unstable();
        sorted.dedup();

        let mut raw = data.raw().to_vec();
        for &index in sorted.iter().rev() {
            let span = match data {
                DroData::V1(v1) => v1.byte_offset(index)..v1.byte_offset(index + 1),
                DroData::V2(_) => DroDataV2::byte_offset(index)..DroDataV2::byte_offset(index + 1),
            };
            raw.drain(span);
        }
        raw
    }

    #[test]
    fn v2_delete_single() {
        let mut data = DroData::V2(v2_fixture());
        data.delete_many(&[0]);
        assert_eq!(data.len(), 13);
        assert_eq!(data.raw_len(), 26);
        assert_eq!(&data.raw()[..6], &[0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
    }

    #[test]
    fn v2_delete_contiguous_range() {
        let mut data = DroData::V2(v2_fixture());
        data.delete_many(&[0]);
        // Python's `del dro_data[1:2]` removed logical indices 1 AND 2.
        data.delete_many(&[1, 2]);
        assert_eq!(data.len(), 11);
        assert_eq!(data.raw_len(), 22);
        assert_eq!(&data.raw()[..6], &[0x02, 0x03, 0x08, 0x09, 0xFE, 0xB0]);
    }

    #[test]
    fn delete_many_matches_the_naive_reference() {
        let selections: &[&[usize]] = &[
            &[],
            &[0],
            &[13],
            &[0, 13],
            &[1, 6, 3, 4], // the unsorted, fragmented selection from the Python test
            &[4, 4, 4],    // duplicates
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13], // delete-all
            &[2, 3, 4, 8, 9], // two adjacent runs
            &[0, 2, 4, 6, 8, 10, 12], // maximally fragmented
            &[99],         // entirely out of range
            &[5, 99],      // partially out of range
        ];
        for selection in selections {
            let v2 = DroData::V2(v2_fixture());
            let expected = naive_delete(&v2, selection);
            let mut actual = v2.clone();
            actual.delete_many(selection);
            assert_eq!(
                actual.raw(),
                expected.as_slice(),
                "v2 selection {selection:?}"
            );

            // Out-of-range indices are ignored on both sides, so the same
            // selections exercise the shorter v1 fixture too.
            let v1 = DroData::V1(v1_fixture());
            let expected = naive_delete(&v1, selection);
            let mut actual = v1.clone();
            actual.delete_many(selection);
            assert_eq!(
                actual.raw(),
                expected.as_slice(),
                "v1 selection {selection:?}"
            );
        }
    }

    #[test]
    fn v1_delete_rebuilds_the_index_map() {
        let mut data = DroData::V1(v1_fixture());
        data.delete_many(&[1, 2]); // the short and long delays
        let DroData::V1(v1) = &data else {
            unreachable!()
        };
        assert_eq!(v1.index_map, vec![0, 2, 3, 4, 7]);
        assert_eq!(v1.len(), 5);
        assert_eq!(data.get(1), Some(DroInstruction::BankSwitch(Bank::Low)));
        assert_eq!(
            data.get(4),
            Some(DroInstruction::Register {
                reg: 0xBD,
                value: 0x20,
                bank: None
            })
        );
    }

    #[test]
    fn delete_all_leaves_nothing() {
        for mut data in [DroData::V2(v2_fixture()), DroData::V1(v1_fixture())] {
            let all: Vec<usize> = (0..data.len()).collect();
            data.delete_many(&all);
            assert_eq!(data.len(), 0);
            assert_eq!(data.raw_len(), 0);
            assert!(data.is_empty());
        }
    }

    // -- insertion (undo) --------------------------------------------------

    fn capture(data: &DroData, indices: &[usize]) -> Vec<InsertEntry> {
        let mut sorted = indices.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        sorted
            .into_iter()
            .map(|i| {
                (
                    i,
                    data.raw_instruction(i).unwrap().to_vec().into_boxed_slice(),
                )
            })
            .collect()
    }

    #[test]
    fn delete_then_insert_round_trips() {
        let selections: &[&[usize]] = &[
            &[0],
            &[13],
            &[1, 6, 3, 4],
            &[2, 3, 4, 8, 9],
            &[0, 2, 4, 6, 8, 10, 12],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13],
        ];
        for selection in selections {
            let original = DroData::V2(v2_fixture());
            let entries = capture(&original, selection);
            let mut data = original.clone();
            data.delete_many(selection);
            data.insert_many(&entries);
            assert_eq!(data, original, "v2 selection {selection:?}");

            let original = DroData::V1(v1_fixture());
            let selection: Vec<usize> = selection.iter().copied().filter(|&i| i < 7).collect();
            let entries = capture(&original, &selection);
            let mut data = original.clone();
            data.delete_many(&selection);
            data.insert_many(&entries);
            assert_eq!(data, original, "v1 selection {selection:?}");
        }
    }

    #[test]
    fn insert_many_is_a_no_op_for_no_entries() {
        let mut data = DroData::V2(v2_fixture());
        let before = data.clone();
        data.insert_many(&[]);
        assert_eq!(data, before);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::song::fixtures::{dro_data_v1 as v1_fixture, dro_data_v2 as v2_fixture};
    use proptest::prelude::*;

    fn arbitrary_selection(len: usize) -> impl Strategy<Value = Vec<usize>> {
        proptest::collection::vec(0..len, 0..=len)
    }

    proptest! {
        /// Delete then undo must restore the original bytes exactly, for any
        /// selection -- fragmented, duplicated, or in any order.
        #[test]
        fn v2_delete_then_insert_restores_the_original(selection in arbitrary_selection(14)) {
            let original = DroData::V2(v2_fixture());
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

        #[test]
        fn v1_delete_then_insert_restores_the_original(selection in arbitrary_selection(7)) {
            let original = DroData::V1(v1_fixture());
            let mut sorted = selection.clone();
            sorted.sort_unstable();
            sorted.dedup();

            let entries: Vec<InsertEntry> = sorted
                .iter()
                .map(|&i| (i, original.raw_instruction(i).unwrap().to_vec().into_boxed_slice()))
                .collect();

            let mut data = original.clone();
            data.delete_many(&selection);
            prop_assert_eq!(data.len(), 7 - sorted.len());
            data.insert_many(&entries);
            prop_assert_eq!(&data, &original);
        }

        /// The single-pass compaction must be byte-identical to deleting one
        /// index at a time, back to front.
        #[test]
        fn v1_single_pass_delete_matches_a_naive_reference(selection in arbitrary_selection(7)) {
            let original = DroData::V1(v1_fixture());

            let mut sorted: Vec<usize> = selection.clone();
            sorted.sort_unstable();
            sorted.dedup();
            let mut expected = original.raw().to_vec();
            let DroData::V1(v1) = &original else { unreachable!() };
            for &index in sorted.iter().rev() {
                expected.drain(v1.byte_offset(index)..v1.byte_offset(index + 1));
            }

            let mut actual = original.clone();
            actual.delete_many(&selection);
            prop_assert_eq!(actual.raw(), expected.as_slice());
        }
    }
}
