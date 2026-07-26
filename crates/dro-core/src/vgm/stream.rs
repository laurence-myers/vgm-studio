//! The VGM command stream, for every chip the spec defines.
//!
//! [`VgmData`](super::VgmData) is the OPL stream: a closed table of eight
//! opcodes, each decoding to a register write or a delay. This is the open one.
//! It sizes every command the spec lists -- including the reserved ranges, whose
//! operand counts exist precisely so an unknown command can be stepped over --
//! and describes the ones it recognises, without needing to *understand* any of
//! them.
//!
//! # Nothing is ever dropped
//!
//! A trimmer must never lose bytes it cannot re-encode. The stream is stored
//! whole -- end marker, trailing padding and all -- with an index of where each
//! command starts, so writing it back is a memcpy and deleting a command is a
//! splice. A command this module does not model still has a length, and so
//! still survives.
//!
//! # Version-sensitive sizes
//!
//! Two reserved ranges changed width between versions: `0x40`-`0x4E` took one
//! operand before v1.60 and two from v1.60 on. Sizing them by the file's own
//! declared version is what keeps an old file's stream in step; get it wrong and
//! every command after the first such byte is misread.

use crate::error::{Error, Result};
use crate::vgm::header::ChipKind;

/// Where a write is going: the chip, and which of its (up to two) instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipTarget {
    pub kind: ChipKind,
    /// 0 for the first instance of the chip, 1 for the second.
    pub instance: u8,
    /// The chip's port, for the chips whose registers are banked across two.
    pub port: u8,
}

impl ChipTarget {
    const fn first(kind: ChipKind) -> Self {
        Self {
            kind,
            instance: 0,
            port: 0,
        }
    }

    const fn port(kind: ChipKind, port: u8) -> Self {
        Self {
            kind,
            instance: 0,
            port,
        }
    }

    const fn second(kind: ChipKind, port: u8) -> Self {
        Self {
            kind,
            instance: 1,
            port,
        }
    }

    /// The same target on the second instance of the chip.
    const fn to_second(self) -> Self {
        Self {
            instance: 1,
            ..self
        }
    }

    /// How to name this target in a row label: `"YM2612"`, `"YM2612 p1"`,
    /// `"YM2612 #2 p1"`.
    #[must_use]
    pub fn label(&self) -> String {
        let mut label = self.kind.name().to_owned();
        if self.instance == 1 {
            label.push_str(" #2");
        }
        if self.port != 0 {
            label.push_str(&format!(" p{}", self.port));
        }
        label
    }
}

/// What a command in the stream does, as far as this module models it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VgmCommand {
    /// A register write to a chip.
    Write {
        target: ChipTarget,
        addr: u16,
        data: u16,
    },
    /// Wait this many samples.
    Wait(u32),
    /// Write the next byte of the YM2612 PCM bank to port 0 register 0x2A,
    /// then wait `0..=15` samples.
    DacWrite { wait: u32 },
    /// A `0x67` data block: PCM, a ROM image, or a RAM write.
    DataBlock {
        /// The block type byte, whose ranges the spec assigns by purpose.
        kind: u8,
        /// The block's payload length in bytes, the header excluded.
        length: u32,
        /// Bit 31 of the size field: the block belongs to the second chip.
        second_chip: bool,
    },
    /// A `0x68` PCM RAM write.
    PcmRamWrite { kind: u8, length: u32 },
    /// One of the `0x90`-`0x95` DAC stream control commands.
    DacStream { opcode: u8, stream_id: u8 },
    /// `0xE0`: seek to an offset in the YM2612 PCM bank.
    SeekPcm(u32),
    /// `0x64`: override what `0x62` and `0x63` wait for.
    OverrideWait { which: u8, samples: u16 },
    /// A command with a known length but no meaning here -- a reserved opcode,
    /// or one whose chip this module does not name. Kept for its bytes.
    Raw { opcode: u8 },
}

/// The version at which `0x40`-`0x4E` grew from one operand to two.
const TWO_OPERAND_RESERVED_VERSION: u32 = 0x0000_0160;
/// `0x66`, end of sound data.
pub const END_OF_DATA: u8 = 0x66;
/// `0x67`, a data block. Followed by a `0x66` compatibility byte.
const DATA_BLOCK: u8 = 0x67;
/// `0x68`, a PCM RAM write. Also followed by a `0x66`.
const PCM_RAM_WRITE: u8 = 0x68;
/// The data-block size field's high bit: the block is the second chip's.
const SECOND_CHIP_BLOCK: u32 = 0x8000_0000;

/// The chip a `0xB0`-`0xBF` (`aa dd`) write targets.
const B_RANGE: [ChipKind; 16] = [
    ChipKind::Rf5c68,
    ChipKind::Rf5c164,
    ChipKind::Pwm,
    ChipKind::GameBoyDmg,
    ChipKind::NesApu,
    ChipKind::MultiPcm,
    ChipKind::Upd7759,
    ChipKind::Okim6258,
    ChipKind::Okim6295,
    ChipKind::HuC6280,
    ChipKind::K053260,
    ChipKind::Pokey,
    ChipKind::WonderSwan,
    ChipKind::Saa1099,
    ChipKind::Es5505,
    ChipKind::Ga20,
];

/// The chip a `0xC0`-`0xC8` (16-bit addressed) write targets.
const C_RANGE: [ChipKind; 9] = [
    ChipKind::SegaPcm,
    ChipKind::Rf5c68,
    ChipKind::Rf5c164,
    ChipKind::MultiPcm,
    ChipKind::QSound,
    ChipKind::Scsp,
    ChipKind::WonderSwan,
    ChipKind::Vsu,
    ChipKind::X1010,
];

/// The chip a `0xD0`-`0xD6` (port + register) write targets.
const D_RANGE: [ChipKind; 7] = [
    ChipKind::Ymf278b,
    ChipKind::Ymf271,
    ChipKind::K051649,
    ChipKind::K054539,
    ChipKind::C140,
    ChipKind::Es5503,
    ChipKind::Es5505,
];

/// The chip a `0x51`-`0x5F` write targets, with its port.
fn ym_family(opcode: u8) -> ChipTarget {
    use ChipKind as K;
    match opcode & 0x0F {
        0x1 => ChipTarget::first(K::Ym2413),
        0x2 => ChipTarget::port(K::Ym2612, 0),
        0x3 => ChipTarget::port(K::Ym2612, 1),
        0x4 => ChipTarget::first(K::Ym2151),
        0x5 => ChipTarget::first(K::Ym2203),
        0x6 => ChipTarget::port(K::Ym2608, 0),
        0x7 => ChipTarget::port(K::Ym2608, 1),
        0x8 => ChipTarget::port(K::Ym2610, 0),
        0x9 => ChipTarget::port(K::Ym2610, 1),
        0xA => ChipTarget::first(K::Ym3812),
        0xB => ChipTarget::first(K::Ym3526),
        0xC => ChipTarget::first(K::Y8950),
        0xD => ChipTarget::first(K::Ymz280b),
        0xE => ChipTarget::port(K::Ymf262, 0),
        _ => ChipTarget::port(K::Ymf262, 1),
    }
}

/// The length of the command starting at `bytes[0]`, operands included.
///
/// `version` is the file's declared version, which decides the width of the
/// reserved range that changed at v1.60.
///
/// # Errors
/// If the command runs past the end of `bytes`. Every opcode has a length --
/// that is what the spec's reserved ranges are for -- so an unknown one is not
/// an error here.
pub fn command_size(bytes: &[u8], version: u32) -> Result<usize> {
    let Some(&opcode) = bytes.first() else {
        return Err(Error::file("VGM stream ended where a command was expected"));
    };
    let size = match opcode {
        0x30..=0x3F => 2,
        // Reserved, and Mikey at 0x40 from v1.72. One operand before v1.60,
        // two from then on -- sizing this wrong desynchronises everything after.
        0x40..=0x4E => {
            if version >= TWO_OPERAND_RESERVED_VERSION {
                3
            } else {
                2
            }
        }
        0x4F | 0x50 => 2,
        0x51..=0x5F => 3,
        0x61 => 3,
        0x62 | 0x63 => 1,
        0x64 => 4,
        END_OF_DATA => 1,
        DATA_BLOCK => {
            // 0x67 0x66 tt ssssssss <payload>
            let length = u32_at(bytes, 3).ok_or_else(|| truncated(opcode, bytes.len()))?;
            7 + (length & !SECOND_CHIP_BLOCK) as usize
        }
        PCM_RAM_WRITE => 12,
        0x70..=0x8F => 1,
        0x90 | 0x91 => 5,
        0x92 => 6,
        0x93 => 11,
        0x94 => 2,
        0x95 => 5,
        0xA0..=0xBF => 3,
        0xC0..=0xDF => 4,
        0xE0..=0xFF => 5,
        // 0x00..=0x2F and 0x60, 0x65, 0x69..=0x6F have no defined length: the
        // spec assigns none, so the stream cannot be walked past one.
        _ => {
            return Err(Error::file(format!(
                "VGM command {opcode:#04X} has no defined length"
            )));
        }
    };
    if size > bytes.len() {
        return Err(truncated(opcode, bytes.len()));
    }
    Ok(size)
}

fn truncated(opcode: u8, remaining: usize) -> Error {
    Error::file(format!(
        "VGM stream ends mid-command: {opcode:#04X} needs more than the {remaining} bytes left"
    ))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes(slice.try_into().expect("four bytes")))
}

/// Decodes one command from the bytes it starts at.
///
/// `bytes` must begin at a command boundary and hold at least that command.
#[must_use]
pub fn decode(bytes: &[u8]) -> VgmCommand {
    let opcode = bytes[0];
    let byte = |at: usize| bytes.get(at).copied().unwrap_or(0);
    let word = |at: usize| u16::from(byte(at)) | (u16::from(byte(at + 1)) << 8);

    let write = |target: ChipTarget, addr: u16, data: u16| VgmCommand::Write { target, addr, data };

    match opcode {
        // The SN76489 and its stereo latch, first and second instances.
        0x50 => write(ChipTarget::first(ChipKind::Sn76489), 0, u16::from(byte(1))),
        0x30 => write(
            ChipTarget::second(ChipKind::Sn76489, 0),
            0,
            u16::from(byte(1)),
        ),
        0x4F => write(ChipTarget::first(ChipKind::Sn76489), 1, u16::from(byte(1))),
        0x3F => write(
            ChipTarget::second(ChipKind::Sn76489, 0),
            1,
            u16::from(byte(1)),
        ),
        0x31 => write(ChipTarget::first(ChipKind::Ay8910), 1, u16::from(byte(1))),

        // The YM family, and its 0xAn second-instance mirrors.
        0x51..=0x5F => write(ym_family(opcode), u16::from(byte(1)), u16::from(byte(2))),
        0xA1..=0xAF => write(
            ym_family(opcode).to_second(),
            u16::from(byte(1)),
            u16::from(byte(2)),
        ),
        // Everything below marks its second instance with bit 7 of the address.
        0xA0 => write(
            second_if(ChipTarget::first(ChipKind::Ay8910), byte(1)),
            u16::from(byte(1) & 0x7F),
            u16::from(byte(2)),
        ),
        0xB0..=0xBF => write(
            second_if(
                ChipTarget::first(B_RANGE[(opcode & 0x0F) as usize]),
                byte(1),
            ),
            u16::from(byte(1) & 0x7F),
            u16::from(byte(2)),
        ),
        0xC0..=0xC8 => {
            // 16-bit address, one data byte -- except Sega PCM, whose second
            // chip is marked in the high bit of the address word rather than of
            // the first byte.
            let address = word(1);
            let target = ChipTarget::first(C_RANGE[(opcode & 0x0F) as usize]);
            let (target, addr) = if opcode == 0xC0 {
                (
                    if address & 0x8000 == 0 {
                        target
                    } else {
                        target.to_second()
                    },
                    address & 0x7FFF,
                )
            } else {
                (second_if(target, byte(2)), address & 0x7FFF)
            };
            write(target, addr, u16::from(byte(3)))
        }
        0xD0..=0xD6 => write(
            second_if(
                ChipTarget::port(D_RANGE[(opcode & 0x0F) as usize], byte(1) & 0x7F),
                byte(1),
            ),
            u16::from(byte(2)),
            u16::from(byte(3)),
        ),
        0xE1 => write(ChipTarget::first(ChipKind::C352), word(1), word(3)),

        // Waits.
        0x61 => VgmCommand::Wait(u32::from(word(1))),
        0x62 => VgmCommand::Wait(super::data::command::SAMPLES_60TH),
        0x63 => VgmCommand::Wait(super::data::command::SAMPLES_50TH),
        0x70..=0x7F => VgmCommand::Wait(u32::from(opcode & 0x0F) + 1),
        0x80..=0x8F => VgmCommand::DacWrite {
            wait: u32::from(opcode & 0x0F),
        },
        0x64 => VgmCommand::OverrideWait {
            which: byte(1),
            samples: word(2),
        },

        // Bulk data.
        DATA_BLOCK => {
            let length = u32_at(bytes, 3).unwrap_or(0);
            VgmCommand::DataBlock {
                kind: byte(2),
                length: length & !SECOND_CHIP_BLOCK,
                second_chip: length & SECOND_CHIP_BLOCK != 0,
            }
        }
        PCM_RAM_WRITE => VgmCommand::PcmRamWrite {
            kind: byte(2),
            length: u32::from(byte(9)) | (u32::from(byte(10)) << 8) | (u32::from(byte(11)) << 16),
        },
        0x90..=0x95 => VgmCommand::DacStream {
            opcode,
            stream_id: byte(1),
        },
        0xE0 => VgmCommand::SeekPcm(u32_at(bytes, 1).unwrap_or(0)),

        _ => VgmCommand::Raw { opcode },
    }
}

/// Retargets to the chip's second instance when bit 7 of `first_operand` is set.
const fn second_if(target: ChipTarget, first_operand: u8) -> ChipTarget {
    if first_operand & 0x80 == 0 {
        target
    } else {
        target.to_second()
    }
}

/// What a data block's type byte means, in the spec's own ranges.
#[must_use]
pub fn data_block_purpose(kind: u8) -> &'static str {
    match kind {
        0x00..=0x3F => "uncompressed stream",
        0x40..=0x7E => "compressed stream",
        0x7F => "decompression table",
        0x80..=0xBF => "ROM image",
        _ => "RAM write",
    }
}

/// A parsed VGM command stream.
///
/// Holds the body whole -- the `0x66` end marker and anything after it included
/// -- with an index of the commands before that marker. Writing is a memcpy of
/// [`Self::raw`], so a stream that is read and written back is unchanged to the
/// byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmStream {
    data: Vec<u8>,
    offsets: Vec<u32>,
    /// Where the end marker sits (or `data.len()` if the stream has none).
    tail: u32,
    version: u32,
}

impl VgmStream {
    /// Walks `data` as a command stream declared by a file of `version`.
    ///
    /// # Errors
    /// If a command has no defined length or runs past the end of the stream.
    pub fn parse(data: Vec<u8>, version: u32) -> Result<Self> {
        let mut offsets = Vec::new();
        let mut at = 0usize;
        let tail = loop {
            if at >= data.len() {
                log::warn!("VGM data has no {END_OF_DATA:#04X} end-of-data marker");
                break data.len();
            }
            if data[at] == END_OF_DATA {
                break at;
            }
            let size = command_size(&data[at..], version)?;
            offsets
                .push(u32::try_from(at).map_err(|_| Error::file("VGM data is larger than 4 GiB"))?);
            at += size;
        };
        let tail = u32::try_from(tail).map_err(|_| Error::file("VGM data is larger than 4 GiB"))?;
        Ok(Self {
            data,
            offsets,
            tail,
            version,
        })
    }

    /// Every byte of the body, exactly as it sits in the file.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// How many commands the stream holds, before its end marker.
    #[must_use]
    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// The file version this stream was sized against.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// The byte offset of command `index` within the body. `index == len()`
    /// yields the end marker's offset.
    #[must_use]
    pub fn byte_offset(&self, index: usize) -> Option<usize> {
        match self.offsets.get(index) {
            Some(&offset) => Some(offset as usize),
            None if index == self.len() => Some(self.tail as usize),
            None => None,
        }
    }

    /// The index of the command that *starts* at `byte_offset`, or `None` if
    /// the offset falls inside a command or past the end.
    #[must_use]
    pub fn index_at_byte_offset(&self, byte_offset: usize) -> Option<usize> {
        let byte_offset = u32::try_from(byte_offset).ok()?;
        if byte_offset == self.tail {
            return Some(self.len());
        }
        self.offsets.binary_search(&byte_offset).ok()
    }

    /// Command `index`'s own bytes.
    #[must_use]
    pub fn raw_command(&self, index: usize) -> Option<&[u8]> {
        let start = self.byte_offset(index)?;
        let end = self.byte_offset(index + 1)?;
        self.data.get(start..end)
    }

    /// What command `index` does.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<VgmCommand> {
        if index >= self.len() {
            return None;
        }
        let start = self.byte_offset(index)?;
        Some(decode(&self.data[start..]))
    }

    /// How long command `index` waits, in samples. Zero for anything that does
    /// not wait.
    #[must_use]
    pub fn wait_samples(&self, index: usize) -> u32 {
        match self.get(index) {
            Some(VgmCommand::Wait(samples)) => samples,
            Some(VgmCommand::DacWrite { wait }) => wait,
            _ => 0,
        }
    }

    /// The stream's total length in samples, summed from its waits.
    #[must_use]
    pub fn total_samples(&self) -> u64 {
        (0..self.len())
            .map(|index| u64::from(self.wait_samples(index)))
            .sum()
    }

    /// A one-line description of command `index`, for the editor's table.
    #[must_use]
    pub fn describe(&self, index: usize) -> String {
        let Some(command) = self.get(index) else {
            return String::new();
        };
        match command {
            VgmCommand::Write { target, addr, data } => {
                format!("{} {addr:#06X} <- {data:#04X}", target.label())
            }
            VgmCommand::Wait(samples) => format!("wait {samples}"),
            VgmCommand::DacWrite { wait } => format!("YM2612 DAC, wait {wait}"),
            VgmCommand::DataBlock {
                kind,
                length,
                second_chip,
            } => format!(
                "data block {kind:#04X} ({}){}, {length} bytes",
                data_block_purpose(kind),
                if second_chip { ", chip 2" } else { "" }
            ),
            VgmCommand::PcmRamWrite { kind, length } => {
                format!("PCM RAM write {kind:#04X}, {length} bytes")
            }
            VgmCommand::DacStream { opcode, stream_id } => {
                let what = match opcode {
                    0x90 => "setup",
                    0x91 => "set data",
                    0x92 => "set frequency",
                    0x93 => "start",
                    0x94 => "stop",
                    _ => "start fast",
                };
                format!("DAC stream #{stream_id} {what}")
            }
            VgmCommand::SeekPcm(offset) => format!("seek PCM bank to {offset:#010X}"),
            VgmCommand::OverrideWait { which, samples } => {
                format!("override wait {which:#04X} = {samples}")
            }
            VgmCommand::Raw { opcode } => format!("unknown command {opcode:#04X}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command the spec defines, one of each, in opcode order.
    fn one_of_everything() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x30, 0x11]); // SN76489 #2
        bytes.extend_from_slice(&[0x31, 0x22]); // AY8910 stereo mask
        bytes.extend_from_slice(&[0x3F, 0x33]); // GG stereo #2
        bytes.extend_from_slice(&[0x40, 0x44, 0x55]); // Mikey (v1.72)
        bytes.extend_from_slice(&[0x4F, 0x66]); // GG stereo
        bytes.extend_from_slice(&[0x50, 0x77]); // SN76489
        for opcode in 0x51..=0x5Fu8 {
            bytes.extend_from_slice(&[opcode, 0x20, 0x01]);
        }
        bytes.extend_from_slice(&[0x61, 0x10, 0x27]); // wait 10000
        bytes.push(0x62);
        bytes.push(0x63);
        bytes.extend_from_slice(&[0x64, 0x62, 0xE8, 0x03]); // override
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 4, 0, 0, 0, 1, 2, 3, 4]); // data block
        bytes.extend_from_slice(&[0x68, 0x66, 0x01, 1, 2, 3, 4, 5, 6, 7, 8, 9]); // PCM RAM
        bytes.push(0x70);
        bytes.push(0x7F);
        bytes.push(0x80);
        bytes.push(0x8F);
        bytes.extend_from_slice(&[0x90, 0, 0x02, 0, 0x2A]);
        bytes.extend_from_slice(&[0x91, 0, 0, 1, 1]);
        bytes.extend_from_slice(&[0x92, 0, 0x44, 0xAC, 0, 0]);
        bytes.extend_from_slice(&[0x93, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[0x94, 0]);
        bytes.extend_from_slice(&[0x95, 0, 0, 0, 0]);
        for opcode in 0xA0..=0xBFu8 {
            bytes.extend_from_slice(&[opcode, 0x20, 0x01]);
        }
        for opcode in 0xC0..=0xDFu8 {
            bytes.extend_from_slice(&[opcode, 0x20, 0x01, 0x02]);
        }
        for opcode in 0xE0..=0xFFu8 {
            bytes.extend_from_slice(&[opcode, 1, 2, 3, 4]);
        }
        bytes.push(END_OF_DATA);
        bytes
    }

    #[test]
    fn every_defined_command_is_walked_and_the_stream_ends_where_it_should() {
        let bytes = one_of_everything();
        let stream = VgmStream::parse(bytes.clone(), 0x172).unwrap();
        assert_eq!(stream.raw(), bytes, "held whole");
        assert_eq!(
            stream.byte_offset(stream.len()),
            Some(bytes.len() - 1),
            "the index stops at the end marker"
        );
        // Every command's bytes, concatenated, are the stream minus its marker.
        let walked: Vec<u8> = (0..stream.len())
            .flat_map(|index| stream.raw_command(index).unwrap().to_vec())
            .collect();
        assert_eq!(walked, bytes[..bytes.len() - 1]);
    }

    /// The same five bytes are two different streams depending on the version
    /// the file declares, because `0x41` grew a second operand at v1.60. Get it
    /// wrong and every command after it is misread -- here, a whole extra wait.
    #[test]
    fn the_reserved_range_that_changed_width_is_sized_by_version() {
        let bytes = vec![0x41, 0x11, 0x62, 0x62, END_OF_DATA];

        let old = VgmStream::parse(bytes.clone(), 0x151).unwrap();
        assert_eq!(old.len(), 3, "0x41 takes one operand, leaving two waits");
        assert_eq!(old.get(0), Some(VgmCommand::Raw { opcode: 0x41 }));
        assert_eq!(old.total_samples(), 735 * 2);

        let new = VgmStream::parse(bytes, 0x160).unwrap();
        assert_eq!(new.len(), 2, "it takes two, swallowing the first wait");
        assert_eq!(new.get(1), Some(VgmCommand::Wait(735)));
        assert_eq!(new.total_samples(), 735);
    }

    #[test]
    fn waits_of_every_shape_are_totalled() {
        let bytes = vec![
            0x61,
            0x10,
            0x27, // 10000
            0x62, // 735
            0x63, // 882
            0x70, // 1
            0x7F, // 16
            0x85, // DAC write + 5
            END_OF_DATA,
        ];
        let stream = VgmStream::parse(bytes, 0x151).unwrap();
        assert_eq!(stream.total_samples(), 10_000 + 735 + 882 + 1 + 16 + 5);
        assert_eq!(stream.get(0), Some(VgmCommand::Wait(10_000)));
        assert_eq!(stream.get(5), Some(VgmCommand::DacWrite { wait: 5 }));
    }

    #[test]
    fn a_data_block_is_one_command_owning_its_payload() {
        let mut bytes = vec![0x67, 0x66, 0x00, 0x10, 0, 0, 0];
        bytes.extend_from_slice(&[0xAB; 0x10]);
        bytes.push(0x61);
        bytes.extend_from_slice(&[0x10, 0x27]);
        bytes.push(END_OF_DATA);

        let stream = VgmStream::parse(bytes, 0x160).unwrap();
        assert_eq!(stream.len(), 2, "the block and the wait");
        assert_eq!(
            stream.get(0),
            Some(VgmCommand::DataBlock {
                kind: 0x00,
                length: 0x10,
                second_chip: false
            })
        );
        assert_eq!(stream.raw_command(0).unwrap().len(), 7 + 0x10);
        assert_eq!(stream.get(1), Some(VgmCommand::Wait(10_000)));
    }

    /// A compressed block is not decompressed here -- that is the engine's job
    /// -- but it must still be one command with the right length.
    #[test]
    fn a_compressed_block_is_stepped_over_whole() {
        let mut bytes = vec![0x67, 0x66, 0x40, 8, 0, 0, 0];
        bytes.extend_from_slice(&[0; 8]);
        bytes.push(END_OF_DATA);
        let stream = VgmStream::parse(bytes, 0x160).unwrap();
        assert_eq!(stream.len(), 1);
        assert_eq!(
            stream.describe(0),
            "data block 0x40 (compressed stream), 8 bytes"
        );
    }

    #[test]
    fn a_second_chip_block_is_flagged_and_its_length_masked() {
        let mut bytes = vec![0x67, 0x66, 0x80];
        bytes.extend_from_slice(&(4u32 | SECOND_CHIP_BLOCK).to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        bytes.push(END_OF_DATA);
        let stream = VgmStream::parse(bytes, 0x160).unwrap();
        assert_eq!(
            stream.get(0),
            Some(VgmCommand::DataBlock {
                kind: 0x80,
                length: 4,
                second_chip: true
            })
        );
        assert_eq!(stream.raw_command(0).unwrap().len(), 11);
    }

    #[test]
    fn writes_are_routed_to_the_right_chip_and_port() {
        let cases: [(&[u8], ChipKind, u8, u8); 8] = [
            (&[0x50, 0x9F], ChipKind::Sn76489, 0, 0),
            (&[0x52, 0x28, 0xF0], ChipKind::Ym2612, 0, 0),
            (&[0x53, 0x28, 0xF0], ChipKind::Ym2612, 0, 1),
            (&[0x5E, 0x05, 0x01], ChipKind::Ymf262, 0, 0),
            (&[0x5F, 0x05, 0x01], ChipKind::Ymf262, 0, 1),
            (&[0xA2, 0x28, 0xF0], ChipKind::Ym2612, 1, 0),
            (&[0xB3, 0x10, 0x80], ChipKind::GameBoyDmg, 0, 0),
            (&[0xBF, 0x10, 0x80], ChipKind::Ga20, 0, 0),
        ];
        for (bytes, kind, instance, port) in cases {
            let VgmCommand::Write { target, .. } = decode(bytes) else {
                panic!("{bytes:?} should be a write");
            };
            assert_eq!(
                (target.kind, target.instance, target.port),
                (kind, instance, port),
                "{bytes:02X?}"
            );
        }
    }

    /// Most chips mark their second instance in bit 7 of the first operand;
    /// Sega PCM marks it in the high bit of its 16-bit address instead.
    #[test]
    fn the_second_instance_bit_is_read_where_each_chip_keeps_it() {
        let VgmCommand::Write { target, addr, .. } = decode(&[0xB3, 0x90, 0x80]) else {
            panic!("a write");
        };
        assert_eq!(target.instance, 1);
        assert_eq!(addr, 0x10, "the flag is not part of the address");

        let VgmCommand::Write { target, addr, .. } = decode(&[0xC0, 0x34, 0x92, 0x01]) else {
            panic!("a write");
        };
        assert_eq!(target.kind, ChipKind::SegaPcm);
        assert_eq!(target.instance, 1, "bit 15 of the address word");
        assert_eq!(addr, 0x1234);
    }

    #[test]
    fn dac_stream_control_is_recognised() {
        let stream = VgmStream::parse(
            vec![0x90, 0x03, 0x02, 0x00, 0x2A, 0x94, 0x03, END_OF_DATA],
            0x160,
        )
        .unwrap();
        assert_eq!(
            stream.get(0),
            Some(VgmCommand::DacStream {
                opcode: 0x90,
                stream_id: 3
            })
        );
        assert_eq!(stream.describe(0), "DAC stream #3 setup");
        assert_eq!(stream.describe(1), "DAC stream #3 stop");
    }

    #[test]
    fn a_reserved_opcode_survives_as_raw_bytes() {
        let bytes = vec![0xC9, 1, 2, 3, 0x62, END_OF_DATA];
        let stream = VgmStream::parse(bytes.clone(), 0x171).unwrap();
        assert_eq!(stream.len(), 2);
        assert_eq!(stream.get(0), Some(VgmCommand::Raw { opcode: 0xC9 }));
        assert_eq!(stream.describe(0), "unknown command 0xC9");
        assert_eq!(stream.raw(), bytes, "and its bytes are untouched");
    }

    #[test]
    fn bytes_after_the_end_marker_stay_in_the_stream() {
        let bytes = vec![0x62, END_OF_DATA, 0xDE, 0xAD];
        let stream = VgmStream::parse(bytes.clone(), 0x151).unwrap();
        assert_eq!(stream.len(), 1);
        assert_eq!(stream.byte_offset(1), Some(1), "the marker's offset");
        assert_eq!(stream.raw(), bytes);
    }

    #[test]
    fn a_stream_with_no_end_marker_still_parses() {
        let stream = VgmStream::parse(vec![0x62, 0x63], 0x151).unwrap();
        assert_eq!(stream.len(), 2);
        assert_eq!(stream.total_samples(), 735 + 882);
    }

    #[test]
    fn a_loop_offset_resolves_to_a_command_index() {
        let stream = VgmStream::parse(
            vec![
                0x5A,
                0x20,
                0x01,
                0x61,
                0x10,
                0x27,
                0x5A,
                0x21,
                0x02,
                END_OF_DATA,
            ],
            0x151,
        )
        .unwrap();
        assert_eq!(stream.index_at_byte_offset(0), Some(0));
        assert_eq!(stream.index_at_byte_offset(3), Some(1));
        assert_eq!(stream.index_at_byte_offset(6), Some(2));
        assert_eq!(stream.index_at_byte_offset(4), None, "inside a command");
        assert_eq!(stream.index_at_byte_offset(99), None);
    }

    #[test]
    fn rejects_a_command_with_no_defined_length() {
        let error = VgmStream::parse(vec![0x00, 0x01], 0x151)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no defined length"), "{error}");
    }

    #[test]
    fn rejects_a_truncated_command() {
        assert!(VgmStream::parse(vec![0x61, 0x10], 0x151).is_err());
        assert!(VgmStream::parse(vec![0x67, 0x66, 0x00, 0xFF, 0, 0, 0], 0x160).is_err());
    }

    #[test]
    fn the_opl_stream_decodes_the_same_way_the_editor_reads_it() {
        // The OPL reader's own opcodes, through the generic table: same chips,
        // same waits. This is the projection mc-5 leans on.
        let stream = VgmStream::parse(
            vec![
                0x5A,
                0x20,
                0x01, // OPL2
                0x5E,
                0x05,
                0x01, // OPL3 port 0
                0x5F,
                0x05,
                0x01, // OPL3 port 1
                0xAA,
                0x20,
                0x01, // second OPL2
                END_OF_DATA,
            ],
            0x151,
        )
        .unwrap();
        assert_eq!(stream.describe(0), "YM3812 0x0020 <- 0x01");
        assert_eq!(stream.describe(1), "YMF262 0x0005 <- 0x01");
        assert_eq!(stream.describe(2), "YMF262 p1 0x0005 <- 0x01");
        assert_eq!(stream.describe(3), "YM3812 #2 0x0020 <- 0x01");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// A generator for one syntactically valid command of any shape.
    fn any_command() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            // Fixed-size commands, over every opcode that has one.
            (0x30u8..=0x3F, any::<u8>()).prop_map(|(op, a)| vec![op, a]),
            (0x4Fu8..=0x50, any::<u8>()).prop_map(|(op, a)| vec![op, a]),
            (0x51u8..=0x5F, any::<u8>(), any::<u8>()).prop_map(|(op, a, b)| vec![op, a, b]),
            (0xA0u8..=0xBF, any::<u8>(), any::<u8>()).prop_map(|(op, a, b)| vec![op, a, b]),
            (0xC0u8..=0xDF, any::<u8>(), any::<u8>(), any::<u8>())
                .prop_map(|(op, a, b, c)| vec![op, a, b, c]),
            (
                0xE0u8..=0xFF,
                any::<u8>(),
                any::<u8>(),
                any::<u8>(),
                any::<u8>()
            )
                .prop_map(|(op, a, b, c, d)| vec![op, a, b, c, d]),
            (0x70u8..=0x8F).prop_map(|op| vec![op]),
            (0x62u8..=0x63).prop_map(|op| vec![op]),
            (any::<u8>(), any::<u8>()).prop_map(|(a, b)| vec![0x61, a, b]),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c)| vec![0x64, a, b, c]),
            Just(vec![0x94, 0x00]),
            (0x90u8..=0x91, any::<u8>()).prop_map(|(op, id)| vec![op, id, 0, 0, 0]),
            Just(vec![0x95, 0, 0, 0, 0]),
            Just(vec![0x92, 0, 0, 0, 0, 0]),
            Just(vec![0x93, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Just(vec![0x68, 0x66, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            // A data block, whose length field must match its payload.
            (any::<u8>(), prop::collection::vec(any::<u8>(), 0..24)).prop_map(|(kind, payload)| {
                let mut bytes = vec![0x67, 0x66, kind];
                bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&payload);
                bytes
            }),
        ]
    }

    proptest! {
        /// The property the whole trimmer rests on: whatever the stream holds,
        /// walking it and writing it back changes nothing, and every command's
        /// bytes are accounted for exactly once.
        #[test]
        fn any_stream_of_valid_commands_round_trips(
            commands in prop::collection::vec(any_command(), 0..40),
        ) {
            let mut bytes: Vec<u8> = commands.iter().flatten().copied().collect();
            bytes.push(END_OF_DATA);

            // v1.72 so the 0x40..0x4E range is at its two-operand width, which
            // is what the generator emits.
            let stream = VgmStream::parse(bytes.clone(), 0x172)?;
            prop_assert_eq!(stream.raw(), bytes.as_slice());
            prop_assert_eq!(stream.len(), commands.len());

            let walked: Vec<u8> = (0..stream.len())
                .flat_map(|i| stream.raw_command(i).unwrap().to_vec())
                .collect();
            prop_assert_eq!(walked, bytes[..bytes.len() - 1].to_vec());

            // Every command decodes and describes without panicking, and its
            // offset resolves back to its own index.
            for index in 0..stream.len() {
                prop_assert!(stream.get(index).is_some());
                prop_assert!(!stream.describe(index).is_empty());
                let at = stream.byte_offset(index).unwrap();
                prop_assert_eq!(stream.index_at_byte_offset(at), Some(index));
            }
        }

        /// Sizing must never read past what it was given: a truncated stream
        /// errors rather than panicking, whatever it was cut in the middle of.
        #[test]
        fn a_truncated_stream_errors_rather_than_panicking(
            commands in prop::collection::vec(any_command(), 1..12),
            cut in 1usize..40,
        ) {
            let mut bytes: Vec<u8> = commands.iter().flatten().copied().collect();
            bytes.push(END_OF_DATA);
            let keep = bytes.len().saturating_sub(cut);
            bytes.truncate(keep);
            // Either it parses (the cut landed on a boundary) or it errors.
            let _ = VgmStream::parse(bytes, 0x172);
        }
    }
}
