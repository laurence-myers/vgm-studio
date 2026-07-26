//! The data a VGM's `0x67` blocks carry, and what happens to each kind.
//!
//! A VGM's data blocks are three different things wearing one opcode, told
//! apart by the type byte:
//!
//! - **`0x00`–`0x3F`** — a stream of samples for the DAC engine to play back.
//!   The engine keeps these; they are addressed by *block index within their
//!   type*, which is what `0x95`'s "fast start" means.
//! - **`0x40`–`0x7E`** — the same, compressed. Decompressed on arrival, so
//!   everything downstream sees one kind of bank. `0x7F` is not a bank at all
//!   but the shared Huffman table the `0x41`-style blocks decode against.
//! - **`0x80`–`0xBF`** — a piece of a chip's ROM. Handed straight to the chip
//!   and not kept: the chip owns its ROM.
//! - **`0xC0`–`0xE1`** — a RAM write. Also handed straight over.
//!
//! Only the first two need storing, which is why this module is about them.

use dro_core::error::{Error, Result};

/// The highest type byte that is an uncompressed sample stream.
const UNCOMPRESSED_MAX: u8 = 0x3F;
/// The type byte carrying the shared decompression table.
pub const DECOMPRESSION_TABLE: u8 = 0x7F;
/// The first type byte that is a ROM image rather than a sample stream.
const ROM_MIN: u8 = 0x80;

/// What a data block is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// A sample stream the DAC engine plays back, uncompressed.
    Stream,
    /// The same, needing decompression first.
    CompressedStream,
    /// The shared table `0x41`-style compressed blocks decode against.
    DecompressionTable,
    /// A piece of a chip's ROM: `{u32 total_size, u32 start, data}`.
    Rom,
    /// A write into a chip's RAM.
    Ram,
}

impl BlockKind {
    /// What type byte `kind` means.
    #[must_use]
    pub const fn of(kind: u8) -> Self {
        match kind {
            0..=UNCOMPRESSED_MAX => Self::Stream,
            DECOMPRESSION_TABLE => Self::DecompressionTable,
            0x40..=0x7E => Self::CompressedStream,
            ROM_MIN..=0xBF => Self::Rom,
            // 0xC0 and up: a RAM write.
            _ => Self::Ram,
        }
    }

    /// The uncompressed type a compressed block decodes to.
    ///
    /// The two ranges are parallel: `0x40` is a compressed `0x00`, `0x41` a
    /// compressed `0x01`, and so on. The DAC engine only ever sees the
    /// uncompressed number, so a stream bound to bank type `0x00` finds its
    /// data whether the file compressed it or not.
    #[must_use]
    pub const fn uncompressed_type(kind: u8) -> u8 {
        if kind >= 0x40 && kind <= 0x7E {
            kind - 0x40
        } else {
            kind
        }
    }
}

/// A `0x80`-range ROM block's header, once split from its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomBlock {
    /// The whole image's size, which every piece repeats.
    pub total_size: u32,
    /// Where this piece belongs in it.
    pub start: u32,
}

/// A `0xC0`-range RAM write's header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamBlock {
    /// Where the payload goes. Two bytes for the `0xC0`-`0xDF` chips, four for
    /// `0xE0` and up.
    pub offset: u32,
}

/// Reads a ROM block's `{u32 total_size, u32 start}` header.
///
/// # Errors
/// If the payload is shorter than the eight header bytes.
pub fn rom_header(payload: &[u8]) -> Result<(RomBlock, &[u8])> {
    if payload.len() < 8 {
        return Err(Error::file(
            "a ROM data block is too short for its header".to_owned(),
        ));
    }
    let total_size = u32::from_le_bytes(payload[0..4].try_into().expect("checked"));
    let start = u32::from_le_bytes(payload[4..8].try_into().expect("checked"));
    Ok((RomBlock { total_size, start }, &payload[8..]))
}

/// Reads a RAM write's offset header: two bytes below type `0xE0`, four above.
///
/// # Errors
/// If the payload is shorter than that offset.
pub fn ram_header(kind: u8, payload: &[u8]) -> Result<(RamBlock, &[u8])> {
    let width = if kind >= 0xE0 { 4 } else { 2 };
    if payload.len() < width {
        return Err(Error::file(
            "a RAM data block is too short for its offset".to_owned(),
        ));
    }
    let offset = match width {
        2 => u32::from(u16::from_le_bytes(
            payload[0..2].try_into().expect("checked"),
        )),
        _ => u32::from_le_bytes(payload[0..4].try_into().expect("checked")),
    };
    Ok((RamBlock { offset }, &payload[width..]))
}

/// The sample streams a file has handed over, kept by type and arrival order.
///
/// `0x95` addresses a bank by "the nth block of this type", so order within a
/// type is what matters and the types are otherwise independent.
#[derive(Debug, Default, Clone)]
pub struct Banks {
    /// `(uncompressed type, payload)`, in arrival order.
    blocks: Vec<(u8, Vec<u8>)>,
}

impl Banks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a decoded sample stream of (uncompressed) type `kind`.
    pub fn push(&mut self, kind: u8, data: Vec<u8>) {
        self.blocks.push((kind, data));
    }

    /// The `index`th block of type `kind`, as `0x95` addresses it.
    #[must_use]
    pub fn nth(&self, kind: u8, index: usize) -> Option<&[u8]> {
        self.blocks
            .iter()
            .filter(|(block_kind, _)| *block_kind == kind)
            .nth(index)
            .map(|(_, data)| data.as_slice())
    }

    /// Every block of type `kind` end to end, which is how a stream bound with
    /// `0x91` addresses its data: the spec's offsets are into the type's whole
    /// concatenated bank, not into one block.
    #[must_use]
    pub fn concatenated(&self, kind: u8) -> Vec<u8> {
        self.blocks
            .iter()
            .filter(|(block_kind, _)| *block_kind == kind)
            .flat_map(|(_, data)| data.iter().copied())
            .collect()
    }

    /// How many blocks are stored, of every type.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Forgets everything, as a seek back to the start does.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_byte_says_what_a_block_is_for() {
        assert_eq!(BlockKind::of(0x00), BlockKind::Stream);
        assert_eq!(BlockKind::of(0x3F), BlockKind::Stream);
        assert_eq!(BlockKind::of(0x40), BlockKind::CompressedStream);
        assert_eq!(BlockKind::of(0x7E), BlockKind::CompressedStream);
        assert_eq!(BlockKind::of(0x7F), BlockKind::DecompressionTable);
        assert_eq!(BlockKind::of(0x80), BlockKind::Rom);
        assert_eq!(BlockKind::of(0xBF), BlockKind::Rom);
        assert_eq!(BlockKind::of(0xC0), BlockKind::Ram);
        assert_eq!(BlockKind::of(0xFF), BlockKind::Ram);
    }

    #[test]
    fn a_compressed_block_decodes_to_its_uncompressed_twin() {
        assert_eq!(BlockKind::uncompressed_type(0x40), 0x00);
        assert_eq!(BlockKind::uncompressed_type(0x42), 0x02);
        // Everything outside the compressed range is already itself.
        assert_eq!(BlockKind::uncompressed_type(0x00), 0x00);
        assert_eq!(BlockKind::uncompressed_type(0x7F), 0x7F);
        assert_eq!(BlockKind::uncompressed_type(0x80), 0x80);
    }

    #[test]
    fn a_rom_block_splits_into_its_header_and_its_piece() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        payload.extend_from_slice(&0x0000_0200u32.to_le_bytes());
        payload.extend_from_slice(&[1, 2, 3, 4]);

        let (rom, data) = rom_header(&payload).unwrap();
        assert_eq!(rom.total_size, 0x0001_0000);
        assert_eq!(rom.start, 0x200);
        assert_eq!(data, [1, 2, 3, 4]);

        assert!(rom_header(&[0, 0, 0]).is_err());
    }

    #[test]
    fn a_ram_offset_is_two_bytes_below_type_e0_and_four_above() {
        let (ram, data) = ram_header(0xC0, &[0x34, 0x12, 9, 9]).unwrap();
        assert_eq!(ram.offset, 0x1234);
        assert_eq!(data, [9, 9]);

        let (ram, data) = ram_header(0xE0, &[0x78, 0x56, 0x34, 0x12, 7]).unwrap();
        assert_eq!(ram.offset, 0x1234_5678);
        assert_eq!(data, [7]);

        assert!(ram_header(0xE0, &[1, 2, 3]).is_err());
    }

    #[test]
    fn banks_are_addressed_by_index_within_a_type_and_concatenated_across_it() {
        let mut banks = Banks::new();
        banks.push(0x00, vec![1, 2]);
        banks.push(0x01, vec![9]);
        banks.push(0x00, vec![3, 4]);

        assert_eq!(banks.nth(0x00, 0), Some([1, 2].as_slice()));
        assert_eq!(banks.nth(0x00, 1), Some([3, 4].as_slice()));
        assert_eq!(banks.nth(0x00, 2), None);
        assert_eq!(banks.nth(0x01, 0), Some([9].as_slice()));

        // A `0x91` binding addresses the whole type as one run.
        assert_eq!(banks.concatenated(0x00), vec![1, 2, 3, 4]);
        assert_eq!(banks.len(), 3);

        banks.clear();
        assert!(banks.is_empty());
    }
}
