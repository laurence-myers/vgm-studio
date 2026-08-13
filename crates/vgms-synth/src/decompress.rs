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
enum Compression {
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

/// Reads values of a fixed bit width, the reference's way (`READ_BITS` in
/// `player/dblk_compr.c`): the stream is consumed most significant bit first,
/// in chunks of up to eight bits (eight at a time while more than eight
/// remain), and the chunks assemble **low chunk first** -- the first chunk
/// lands in bits 0-7, the next in bits 8-15, and so on.
///
/// For widths of eight bits or fewer there is one chunk and the two orders
/// coincide, which is every common block. At 9-16 bits they differ: a 12-bit
/// value is `byte0 | top4(byte1) << 8`, not `(byte0 << 4) | top4(byte1)` --
/// reading it the other way decodes every wide bank to different values than
/// the reference plays.
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
        let mut assembled = 0u8;
        while assembled < bits {
            let take = (bits - assembled).min(8);
            let mut chunk = 0u32;
            for _ in 0..take {
                let byte = self.bytes[self.at / 8];
                let bit = (byte >> (7 - (self.at % 8))) & 1;
                chunk = (chunk << 1) | u32::from(bit);
                self.at += 1;
            }
            value |= chunk << assembled;
            assembled += take;
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
/// If the payload is shorter than its sub-header, names a compression scheme the
/// spec does not define, or declares a value width of zero or more than 32 bits
/// (`u32` cannot hold it, and the byte-emit and shift below would panic). A
/// block whose packed data runs out early decompresses to what it did carry
/// rather than failing: a truncated bank is a quieter fault than a refused file,
/// and the sample count is what the caller has to trust anyway.
pub fn decompress(payload: &[u8], table: Option<&DecompressionTable>) -> Result<Vec<u8>> {
    decompress_capped(payload, table, MAX_DECOMPRESSED)
}

/// The most a single compressed block may decompress to: 128 MiB. Real sample
/// banks top out in the tens of MiB; the cap is the absolute ceiling the
/// feasible-yield reservation below lacks on its own -- bit-packing can expand
/// its input up to 32x (1 bit in, 4 bytes out), so a large hostile block could
/// otherwise inflate to gigabytes while every reservation stays "feasible".
const MAX_DECOMPRESSED: usize = 128 * 1024 * 1024;

/// [`decompress`] with an explicit output ceiling, so a test can prove the cap
/// without a hundred-megabyte payload.
fn decompress_capped(
    payload: &[u8],
    table: Option<&DecompressionTable>,
    cap: usize,
) -> Result<Vec<u8>> {
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
    // A width past 32 bits does not fit a `u32`: `to_le_bytes()[..width]` would
    // slice past four bytes, and `packed << (bits_out - bits_in)` would overflow
    // the shift. One check closes both.
    if bits_out > 32 {
        return Err(Error::file(
            "a compressed data block declares a value wider than 32 bits".to_owned(),
        ));
    }
    // The same bound on the packed side. `BitReader::read` already refuses more
    // than 32 bits, but silently -- the block would decompress to nothing. A
    // width no `u32` can hold is unambiguously malformed; say so like `bits_out`.
    if bits_in > 32 {
        return Err(Error::file(
            "a compressed data block declares packed values wider than 32 bits".to_owned(),
        ));
    }
    let count = uncompressed_size / width;
    let mask = if bits_out >= 32 {
        u32::MAX
    } else {
        (1u32 << bits_out) - 1
    };

    // `uncompressed_size` is attacker-controlled and only an upper bound: the
    // packed data can yield no more values than its own bits allow, and the loop
    // below stops the moment the reader runs dry. Reserve against what the input
    // can actually produce, so a block declaring gigabytes while carrying a
    // handful of bytes cannot turn into a gigabyte reservation.
    let max_values = data.len().saturating_mul(8) / usize::from(bits_in);
    let capacity = uncompressed_size.min(max_values.saturating_mul(width));
    // `capacity` is also exactly what the loop can emit (count x width and the
    // reader's supply both bound it), so this one check caps the real output,
    // not just the reservation. Feasibility alone is not a ceiling: bit-packing
    // expands up to 32x, so a large block must be refused outright.
    if capacity > cap {
        return Err(Error::file(format!(
            "a compressed data block would decompress to {capacity} bytes; \
             the cap is {cap}"
        )));
    }
    let mut reader = BitReader::new(data);
    let mut out = Vec::with_capacity(capacity);
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

        // The reference's chunking, mirrored (`WRITE_BITS`): each value goes
        // out in chunks of up to eight bits, low chunk first, each chunk
        // written to the stream most significant bit first.
        let mut bits = Vec::new();
        for value in values {
            let mut emitted = 0u8;
            while emitted < bits_in {
                let take = (bits_in - emitted).min(8);
                let chunk = (value >> emitted) & ((1u32 << take) - 1);
                for shift in (0..take).rev() {
                    bits.push(((chunk >> shift) & 1) as u8);
                }
                emitted += take;
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
    fn a_block_declaring_more_than_thirty_two_bits_is_refused() {
        // `bits_out` of 40 would slice a five-byte value out of a four-byte
        // buffer and shift a `u32` by more than 31; refuse it, do not panic.
        let payload = packed(0x00, 8, 40, 8, 0, 0, &[1]);
        let error = decompress(&payload, None).unwrap_err();
        assert!(error.to_string().contains("wider than 32"), "{error}");
    }

    #[test]
    fn a_declared_size_does_not_become_an_unbounded_reservation() {
        // Declares four gigabytes of output while carrying one packed byte.
        let mut payload = vec![0x00];
        payload.extend_from_slice(&u32::MAX.to_le_bytes());
        payload.extend_from_slice(&[8, 8, 0]); // bits_out, bits_in, sub_type
        payload.extend_from_slice(&0u16.to_le_bytes()); // operand
        payload.push(0x77); // one packed value
        let out = decompress(&payload, None).unwrap();
        assert_eq!(out, vec![0x77]);
        // The reservation followed the input, not the four-gigabyte claim.
        assert!(out.capacity() <= 16, "reserved {} bytes", out.capacity());
    }

    #[test]
    fn packed_values_wider_than_thirty_two_bits_are_refused_not_emptied() {
        // `bits_in` of 40: `BitReader::read` would refuse every read, so before
        // this check the block silently decompressed to nothing. Malformed is
        // malformed -- say so, symmetrically with `bits_out`.
        let payload = packed(0x00, 8, 8, 40, 0, 0, &[]);
        let error = decompress(&payload, None).unwrap_err();
        assert!(error.to_string().contains("packed values wider"), "{error}");
    }

    #[test]
    fn a_block_that_would_decompress_past_the_cap_is_refused() {
        // 1-bit packed values inflating to 4-byte ones: 32x amplification, the
        // worst the format allows. Five packed bytes can feasibly yield 160
        // bytes; with the ceiling below that, the block is refused outright --
        // the declared size is honest, the input is really there, and the
        // output would still be too big.
        let values: Vec<u32> = vec![1; 40];
        let payload = packed(0x00, 160, 32, 1, 0, 0, &values);
        let error = decompress_capped(&payload, None, 100).unwrap_err();
        assert!(error.to_string().contains("the cap is 100"), "{error}");

        // The same block against a big enough ceiling decompresses fine.
        let out = decompress_capped(&payload, None, 160).unwrap();
        assert_eq!(out.len(), 160);
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

    /// Wide values assemble low chunk first, as the reference's `READ_BITS`
    /// does. Hand-derived bytes, **not** the test packer -- the packer shares
    /// the reader's convention, so a round trip alone cannot catch an order
    /// bug on either side (which is how the original one survived).
    #[test]
    fn wide_values_assemble_low_chunk_first_as_the_reference_reads() {
        // Stream 0xAB 0xCD 0x50, read as two 12-bit values:
        // value 1: chunk 0xAB into bits 0-7, chunk 0xC into bits 8-11 -> 0xCAB
        // value 2: chunk 0xD5 into bits 0-7, chunk 0x0 into bits 8-11 -> 0x0D5
        let mut reader = BitReader::new(&[0xAB, 0xCD, 0x50]);
        assert_eq!(reader.read(12), Some(0x0CAB));
        assert_eq!(reader.read(12), Some(0x00D5));

        // A 9-bit read: eight bits then one, the single bit into bit 8.
        let mut reader = BitReader::new(&[0xFF, 0x80]);
        assert_eq!(reader.read(9), Some(0x1FF));
    }
}
