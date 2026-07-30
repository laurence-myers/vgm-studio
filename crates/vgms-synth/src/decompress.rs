//! Unpacking the `0x40`–`0x7E` compressed data blocks.
//!
//! A sample bank can be stored compressed, in one of two schemes the spec
//! defines. Neither is general-purpose compression: both exploit the fact that
//! a bank of, say, 12-bit samples wastes four bits of every 16, or that
//! successive samples differ by little.
//!
//! **Bit packing** stores each value in fewer bits than it occupies unpacked,
//! and says how to get back: add a constant, shift left into place, or look the
//! packed value up in a table.
//!
//! **DPCM** stores a starting value and then a run of *deltas*, each a table
//! index. Each output value is the last one plus the delta it points at.
//!
//! Both can share a table, which arrives in its own `0x7F` block ahead of the
//! data. A file may send several; the last one wins, which is what lets a bank
//! change schemes mid-file.
//!
//! Written from the spec, not ported. The correctness net is round-tripping
//! against a packer written here (a compressed block that decompresses to what
//! was packed) plus the shapes real files use.

use vgms_core::error::{Error, Result};

/// How a compressed block is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Each value stored in fewer bits, recovered by add, shift or table.
    BitPacking,
    /// A start value and a run of table-indexed deltas.
    Dpcm,
}

/// What a bit-packed block does to get each value back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    /// The packed value plus a constant.
    Copy,
    /// The packed value shifted up into its width, plus a constant.
    ShiftLeft,
    /// The packed value as an index into the shared table.
    Table,
}

impl Recovery {
    const fn of(sub_type: u8) -> Self {
        match sub_type {
            1 => Self::ShiftLeft,
            2 => Self::Table,
            _ => Self::Copy,
        }
    }
}

/// A `0x7F` block: the values a table-lookup or DPCM block indexes into.
#[derive(Debug, Clone, Default)]
pub struct DecompressionTable {
    /// Bits per value, which decides how many bytes each occupies.
    bits: u8,
    values: Vec<u32>,
}

impl DecompressionTable {
    /// Reads a `0x7F` block's payload.
    ///
    /// # Errors
    /// If the payload is shorter than its own header, or shorter than the value
    /// count it declares.
    pub fn parse(payload: &[u8]) -> Result<Self> {
        // `{u8 type, u8 sub_type, u8 bits_decompressed, u8 bits_compressed,
        //   u16 count, values...}`
        const HEADER: usize = 6;
        if payload.len() < HEADER {
            return Err(Error::file(
                "a decompression table block is too short for its header".to_owned(),
            ));
        }
        let bits = payload[2];
        let count = usize::from(u16::from_le_bytes([payload[4], payload[5]]));
        let width = value_bytes(bits);
        let needed = count * width;
        let body = &payload[HEADER..];
        if body.len() < needed {
            return Err(Error::file(format!(
                "a decompression table declares {count} values but carries {} bytes",
                body.len()
            )));
        }
        let values = body[..needed]
            .chunks_exact(width)
            .map(read_le)
            .collect::<Vec<u32>>();
        Ok(Self { bits, values })
    }

    /// The `index`th value, or `None` past the end.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<u32> {
        self.values.get(index).copied()
    }

    /// Bits per value.
    #[must_use]
    pub const fn bits(&self) -> u8 {
        self.bits
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Bytes each value of `bits` bits occupies, unpacked. The spec rounds up.
const fn value_bytes(bits: u8) -> usize {
    (bits as usize).div_ceil(8)
}

/// Reads a little-endian value from one to four bytes.
fn read_le(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .rev()
        .fold(0u32, |value, &byte| (value << 8) | u32::from(byte))
}

/// Reads values of a fixed bit width, most significant bit first.
struct BitReader<'a> {
    bytes: &'a [u8],
    /// The next bit to read, counted from the start of `bytes`.
    at: usize,
}

impl<'a> BitReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next `bits` bits, or `None` once there are not that many left.
    fn read(&mut self, bits: u8) -> Option<u32> {
        if bits == 0 || usize::from(bits) > 32 {
            return None;
        }
        if self.at + usize::from(bits) > self.bytes.len() * 8 {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..bits {
            let byte = self.bytes[self.at / 8];
            let bit = (byte >> (7 - (self.at % 8))) & 1;
            value = (value << 1) | u32::from(bit);
            self.at += 1;
        }
        Some(value)
    }
}

/// Unpacks a compressed block's payload.
///
/// `payload` is everything after the `0x67 0x66 tt ssssssss` header: the
/// ten-byte compression sub-header, then the packed data. `table` is the last
/// `0x7F` block seen, which a table-lookup or DPCM block needs and the other
/// bit-packing modes ignore.
///
/// # Errors
/// If the payload is shorter than its sub-header, or names a compression scheme
/// the spec does not define. A block whose packed data runs out early
/// decompresses to what it did carry rather than failing: a truncated bank is a
/// quieter fault than a refused file, and the sample count is what the caller
/// has to trust anyway.
pub fn decompress(payload: &[u8], table: Option<&DecompressionTable>) -> Result<Vec<u8>> {
    // `{u8 type, u32 uncompressed_size, u8 bits_decompressed,
    //   u8 bits_compressed, u8 sub_type, u16 add_or_start}`
    const HEADER: usize = 10;
    if payload.len() < HEADER {
        return Err(Error::file(
            "a compressed data block is too short for its header".to_owned(),
        ));
    }
    let scheme = match payload[0] {
        0x00 => Compression::BitPacking,
        0x01 => Compression::Dpcm,
        other => {
            return Err(Error::file(format!(
                "compression type {other:#04X} is not one the spec defines"
            )));
        }
    };
    let uncompressed_size = read_le(&payload[1..5]) as usize;
    let bits_out = payload[5];
    let bits_in = payload[6];
    let sub_type = payload[7];
    let operand = u32::from(u16::from_le_bytes([payload[8], payload[9]]));
    let data = &payload[HEADER..];

    let width = value_bytes(bits_out);
    if width == 0 || bits_in == 0 {
        return Err(Error::file(
            "a compressed data block declares a zero-bit value".to_owned(),
        ));
    }
    let count = uncompressed_size / width;
    let mask = if bits_out >= 32 {
        u32::MAX
    } else {
        (1u32 << bits_out) - 1
    };

    let mut reader = BitReader::new(data);
    let mut out = Vec::with_capacity(uncompressed_size);
    let mut running = operand;

    for _ in 0..count {
        let Some(packed) = reader.read(bits_in) else {
            break;
        };
        let value = match scheme {
            Compression::BitPacking => match Recovery::of(sub_type) {
                Recovery::Copy => packed.wrapping_add(operand),
                Recovery::ShiftLeft => {
                    (packed << bits_out.saturating_sub(bits_in)).wrapping_add(operand)
                }
                Recovery::Table => table
                    .and_then(|table| table.get(packed as usize))
                    .unwrap_or(0),
            },
            Compression::Dpcm => {
                let delta = table
                    .and_then(|table| table.get(packed as usize))
                    .unwrap_or(0);
                running = running.wrapping_add(delta) & mask;
                running
            }
        } & mask;
        out.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `0x7F` table block's payload holding `values` at `bits` bits each.
    fn table_block(bits: u8, values: &[u32]) -> Vec<u8> {
        let mut payload = vec![0x00, 0x00, bits, bits];
        payload.extend_from_slice(&(values.len() as u16).to_le_bytes());
        let width = value_bytes(bits);
        for value in values {
            payload.extend_from_slice(&value.to_le_bytes()[..width]);
        }
        payload
    }

    /// A compressed block's payload: the sub-header, then `values` packed at
    /// `bits_in` bits each, most significant bit first.
    fn packed(
        scheme: u8,
        uncompressed_size: u32,
        bits_out: u8,
        bits_in: u8,
        sub_type: u8,
        operand: u16,
        values: &[u32],
    ) -> Vec<u8> {
        let mut payload = vec![scheme];
        payload.extend_from_slice(&uncompressed_size.to_le_bytes());
        payload.extend_from_slice(&[bits_out, bits_in, sub_type]);
        payload.extend_from_slice(&operand.to_le_bytes());

        let mut bits = Vec::new();
        for value in values {
            for shift in (0..bits_in).rev() {
                bits.push(((value >> shift) & 1) as u8);
            }
        }
        for chunk in bits.chunks(8) {
            let mut byte = 0u8;
            for (index, bit) in chunk.iter().enumerate() {
                byte |= bit << (7 - index);
            }
            payload.push(byte);
        }
        payload
    }

    #[test]
    fn bit_packing_copies_and_adds() {
        // Four 4-bit values unpacking to 8-bit ones, plus 0x10.
        let payload = packed(0x00, 4, 8, 4, 0, 0x10, &[0, 1, 2, 15]);
        let out = decompress(&payload, None).unwrap();
        assert_eq!(out, vec![0x10, 0x11, 0x12, 0x1F]);
    }

    #[test]
    fn bit_packing_shifts_into_place() {
        // 4-bit values becoming 8-bit ones: shifted up by 4.
        let payload = packed(0x00, 3, 8, 4, 1, 0, &[1, 2, 15]);
        let out = decompress(&payload, None).unwrap();
        assert_eq!(out, vec![0x10, 0x20, 0xF0]);
    }

    #[test]
    fn bit_packing_looks_values_up() {
        let table = DecompressionTable::parse(&table_block(8, &[0xAA, 0xBB, 0xCC])).unwrap();
        let payload = packed(0x00, 3, 8, 2, 2, 0, &[2, 0, 1]);
        let out = decompress(&payload, Some(&table)).unwrap();
        assert_eq!(out, vec![0xCC, 0xAA, 0xBB]);
    }

    #[test]
    fn dpcm_accumulates_its_deltas() {
        // Deltas of +1, +2 and -1 (as an 8-bit wrap), from a start of 0x40.
        let table = DecompressionTable::parse(&table_block(8, &[1, 2, 0xFF])).unwrap();
        let payload = packed(0x01, 4, 8, 2, 0, 0x40, &[0, 0, 1, 2]);
        let out = decompress(&payload, Some(&table)).unwrap();
        assert_eq!(out, vec![0x41, 0x42, 0x44, 0x43]);
    }

    #[test]
    fn a_sixteen_bit_bank_unpacks_two_bytes_a_value() {
        // 12-bit values in a 16-bit bank -- the case bit packing exists for.
        let payload = packed(0x00, 6, 16, 12, 1, 0, &[0x001, 0xFFF, 0x800]);
        let out = decompress(&payload, None).unwrap();
        assert_eq!(out, vec![0x10, 0x00, 0xF0, 0xFF, 0x00, 0x80]);
    }

    #[test]
    fn a_truncated_block_yields_what_it_carried() {
        // Declares four values, carries one.
        let mut payload = packed(0x00, 4, 8, 8, 0, 0, &[0x77]);
        payload.truncate(11);
        let out = decompress(&payload, None).unwrap();
        assert_eq!(out, vec![0x77], "one value, not a refusal");
    }

    #[test]
    fn an_unknown_scheme_is_refused() {
        let payload = packed(0x09, 1, 8, 8, 0, 0, &[1]);
        let error = decompress(&payload, None).unwrap_err();
        assert!(error.to_string().contains("compression type"), "{error}");
    }

    #[test]
    fn a_table_reports_what_it_holds_and_refuses_what_it_does_not() {
        let table = DecompressionTable::parse(&table_block(16, &[0x1234, 0x5678])).unwrap();
        assert_eq!(table.bits(), 16);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(0), Some(0x1234));
        assert_eq!(table.get(1), Some(0x5678));
        assert_eq!(table.get(2), None);

        // A table claiming more values than it carries.
        let mut short = table_block(16, &[1, 2, 3]);
        short.truncate(8);
        assert!(DecompressionTable::parse(&short).is_err());
        assert!(DecompressionTable::parse(&[0, 0]).is_err());
    }

    #[test]
    fn bits_are_read_most_significant_first() {
        // 0b1010_0110 as two 4-bit values is 0b1010 then 0b0110.
        let mut reader = BitReader::new(&[0b1010_0110]);
        assert_eq!(reader.read(4), Some(0b1010));
        assert_eq!(reader.read(4), Some(0b0110));
        assert_eq!(reader.read(4), None, "and then there are none left");
    }
}
