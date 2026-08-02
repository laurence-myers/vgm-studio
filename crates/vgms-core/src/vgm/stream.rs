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
use crate::song::splice::{InsertEntry, byte_ranges_to_delete, splice_in, splice_out};
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

/// What a search over a multichip stream looks for -- the peer of
/// [`FindTarget`](crate::FindTarget) for the Find Register dialog and the delay
/// navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VgmFindTarget {
    /// Any command that advances time. What ArrowLeft/Right step through.
    AnyDelay,
    /// A write to a chip: a given instance and address, or any of either.
    Write {
        kind: ChipKind,
        /// A specific instance, or any of the chip's instances when `None`.
        instance: Option<u8>,
        /// A specific register address, or any write to the chip when `None`
        /// (the dialog's "any write", for chips whose register travels in the
        /// data byte, like the SN76489).
        addr: Option<u16>,
    },
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
/// The most commands a stream may hold, a hostile-file bound rather than a real
/// limit. The index costs twelve bytes a command, so this caps it near 768 MiB;
/// no real rip approaches it (a busy hour of music is a few million commands).
/// See [`VgmStream::parse`].
const MAX_COMMANDS: usize = 64 * 1024 * 1024;
/// `0x00`, which the spec defines as nothing at all -- so in a real file it is
/// padding, and this reader steps over it. See [`command_size`].
pub const PADDING: u8 = 0x00;
/// `0x66`, end of sound data.
pub const END_OF_DATA: u8 = 0x66;
/// `0x67`, a data block. Followed by a `0x66` compatibility byte.
const DATA_BLOCK: u8 = 0x67;
/// `0x68`, a PCM RAM write. Also followed by a `0x66`.
const PCM_RAM_WRITE: u8 = 0x68;
/// The data-block size field's high bit: the block is the second chip's.
const SECOND_CHIP_BLOCK: u32 = 0x8000_0000;

/// The port a chip's *memory* window takes, where the format gives one chip
/// both a register file and a memory space at overlapping addresses.
///
/// This library's own convention, not the format's: `0xB0`/`0xC1` are two
/// commands into one RF5C68, and a core handed `(0x07, 0xFF)` cannot otherwise
/// tell "register 7" from "the eighth byte of wave RAM". The WonderSwan's wave
/// RAM (`0xBC` against `0xC6`) is the same shape.
pub const MEMORY_PORT: u8 = 1;

/// The port the MultiPCM's bank registers take -- a *third* space of the same
/// chip, and neither its register file nor a memory window.
///
/// See the `0xC3` arm of [`decode`] for why a bank select gets a port of its
/// own rather than being folded into an address.
pub const BANK_PORT: u8 = 2;

/// The port the AY8910's stereo mask arrives on -- `0x31` is an instruction to
/// the player rather than a chip write (upstream hands it to a dedicated
/// per-core mask function, not the register file), so it cannot share port 0.
///
/// Port numbers are namespaced per chip: this shares [`BANK_PORT`]'s value on
/// a different chip, and neither collides with anything.
pub const STEREO_PORT: u8 = 2;

/// The port an ES5506 16-bit register write (`0xD6`) arrives on, beside
/// `0xBE`'s 8-bit writes on port 0.
///
/// The two commands address one register file at two widths, and a core must
/// know which width the driver used -- `0xD6`'s value is `ReadLE16` upstream
/// (`Cmd_Ofs8_Data16`) and goes to a dedicated 16-bit writer. Folding both
/// onto port 0 loses exactly that bit: an 8-bit write of register 0 and a
/// 16-bit write of register 0 would arrive identical.
pub const DATA16_PORT: u8 = 3;

/// The chip a `0xB0`-`0xBF` (`aa dd`) write targets. `0xB2` keeps its row for
/// the table's shape but decodes in its own arm: the PWM's operands are a
/// nibble register and a 12-bit value, not `aa dd`.
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
        // A zero byte is not a command; the spec assigns `0x00` no length at
        // all. It is what padding looks like, and real rips carry it (a run of
        // zeros before the first command, or a stray zero before the end
        // marker).
        //
        // Skipping it is safe *because* the walk is self-checking: a
        // desynchronised stream would not walk cleanly to an end marker and
        // would be rejected on the next undefined opcode. Every other undefined
        // byte still stops the walk.
        PADDING => 1,
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
        // 0x01..=0x2F and 0x60, 0x65, 0x69..=0x6F have no defined length: the
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

/// How long `command` waits, in samples. Zero for anything that does not wait.
const fn command_wait(command: &VgmCommand) -> u32 {
    match command {
        VgmCommand::Wait(samples) => *samples,
        VgmCommand::DacWrite { wait } => *wait,
        _ => 0,
    }
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
        // `0x31 dd` is the AY8910's stereo mask -- upstream's `Cmd_AY_Stereo`,
        // an instruction to the player rather than a register write: the low
        // six bits go to a dedicated mask function, bit 6 retargets the whole
        // command at a YM2203's SSG section, and bit 7 is the second chip. It
        // rides on [`STEREO_PORT`] so a core that models none of it (both of
        // ours are mono) can ignore the port instead of eating a register write.
        0x31 => {
            let kind = if byte(1) & 0x40 == 0 {
                ChipKind::Ay8910
            } else {
                ChipKind::Ym2203
            };
            write(
                second_if(ChipTarget::port(kind, STEREO_PORT), byte(1)),
                0,
                u16::from(byte(1) & 0x3F),
            )
        }

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
        // `0xB2 ad dd` is the PWM's write, and not the `aa dd` shape the rest
        // of its range takes: upstream's `Cmd_Ofs4_Data12`, not
        // `Cmd_Ofs8_Data8`. The register is a *nibble* -- bits 6-4 of the
        // first operand -- and the value is twelve bits, its high nibble in
        // bits 3-0 and its low byte in the second operand (big-endian). Bit 7
        // is the second chip, as the rest of the range has it.
        0xB2 => write(
            second_if(ChipTarget::first(ChipKind::Pwm), byte(1)),
            u16::from((byte(1) >> 4) & 0x07),
            ((u16::from(byte(1)) << 8) | u16::from(byte(2))) & 0x0FFF,
        ),
        0xB0..=0xBF => write(
            second_if(
                ChipTarget::first(B_RANGE[(opcode & 0x0F) as usize]),
                byte(1),
            ),
            u16::from(byte(1) & 0x7F),
            u16::from(byte(2)),
        ),
        // QSound is its own arrangement: `0xC4 mm ll rr` is a 16-bit value
        // (big-endian) into register `rr` -- the address and data trade
        // places relative to the rest of the range, so it is normalised
        // here: `addr` is the register, `data` the 16-bit value.
        0xC4 => write(
            ChipTarget::first(ChipKind::QSound),
            u16::from(byte(3)),
            (u16::from(byte(1)) << 8) | u16::from(byte(2)),
        ),
        // `0xC3 cc bbaa` is the MultiPCM's bank select, and the only command in
        // the 16-bit-address range that addresses nothing at all -- so it is
        // decoded here rather than below, where every field would be a lie.
        //
        // `cc` is a bank mask, not a channel (one bit per 512 KiB bank, as
        // upstream's `Cmd_YMW_Bank` reads it: `fData[0x01] & 0x03`), and `bbaa`
        // is a little-endian bank offset in 64 KiB units. Neither operand is an
        // address, so rather than pack them into `addr`, the bank file takes its
        // own [`BANK_PORT`] (as the register/memory pairs use [`MEMORY_PORT`])
        // and the two operands mean what the command says: `addr` the mask,
        // `data` the offset.
        0xC3 => {
            // Upstream reads the second instance from bit 7 of this byte, not
            // from bit 15 of an address -- there is no address.
            let target = second_if(ChipTarget::port(ChipKind::MultiPcm, BANK_PORT), byte(1));
            write(target, u16::from(byte(1) & 0x03), word(2))
        }
        0xC0..=0xC8 => {
            // 16-bit address, one data byte -- `0xC0`-`0xC2` and `0xC5`-`0xC8`,
            // the bank select and the QSound having taken their own arms above.
            // The spec switches byte order mid-range: `0xC0`-`0xC2` write their
            // address as `bbaa` (little-endian), `0xC5`-`0xC8` as `mmll`
            // (big-endian).
            //
            // The second instance is marked in bit 15 of the *assembled
            // address* (the byte the `0x7FFF` mask below clears), not in bit 7
            // of the first operand as the rest of the write range does -- so
            // which byte carries the flag follows the byte order. Assembling the
            // address first means the flag cannot drift from the mask. Upstream
            // states it per convention: `Cmd_SegaPCM_Mem` tests `fData[0x02]`,
            // `Cmd_Ofs16_Data8` tests `fData[0x01]`.
            let address = if opcode >= 0xC5 {
                (u16::from(byte(1)) << 8) | u16::from(byte(2))
            } else {
                word(1)
            };
            let mut target = ChipTarget::first(C_RANGE[(opcode & 0x0F) as usize]);
            if address & 0x8000 != 0 {
                target = target.to_second();
            }
            let addr = address & 0x7FFF;
            // `0xC1`/`0xC2` are the RF chips' direct *memory* pokes, where
            // `0xB0`/`0xB1` carry their register writes -- and the two address
            // spaces overlap from a core's point of view. The port carries the
            // distinction (this library's own convention): registers on port 0,
            // memory on [`MEMORY_PORT`]. `0xC6`, the WonderSwan's wave RAM
            // against `0xBC`'s registers, is the same shape.
            if matches!(opcode, 0xC1 | 0xC2 | 0xC6) {
                target.port = MEMORY_PORT;
            }
            write(target, addr, u16::from(byte(3)))
        }
        // `0xD6 pp aa bb` is the ES5506's 16-bit register write -- upstream's
        // `Cmd_Ofs8_Data16`, not the range's port+register shape: the first
        // operand is the *register* (bit 7 the second chip) and the value is
        // little-endian across the last two. It rides [`DATA16_PORT`] so the
        // width survives to the core; `0xBE`'s 8-bit writes stay on port 0.
        0xD6 => write(
            second_if(ChipTarget::port(ChipKind::Es5505, DATA16_PORT), byte(1)),
            u16::from(byte(1) & 0x7F),
            word(2),
        ),
        0xD0..=0xD5 => write(
            second_if(
                ChipTarget::port(D_RANGE[(opcode & 0x0F) as usize], byte(1) & 0x7F),
                byte(1),
            ),
            u16::from(byte(2)),
            u16::from(byte(3)),
        ),
        // `0x40 aa dd` is the Mikey's register write from v1.72 -- upstream's
        // `Cmd_Ofs8_Data8`, bit 7 of the address the second chip. In an older
        // file the opcode is a one-operand reserved command; `command_size`
        // already sized it that way, and a Mikey write in a file whose header
        // cannot declare a Mikey is dropped by the engine's routing.
        0x40 => write(
            second_if(ChipTarget::first(ChipKind::Mikey), byte(1)),
            u16::from(byte(1) & 0x7F),
            u16::from(byte(2)),
        ),
        // The C352 write is the one command whose operands are big-endian:
        // the corpus's own streams say so (register addresses land on the
        // voice-times-eight grid only under that reading).
        0xE1 => write(
            ChipTarget::first(ChipKind::C352),
            (u16::from(byte(1)) << 8) | u16::from(byte(2)),
            (u16::from(byte(3)) << 8) | u16::from(byte(4)),
        ),

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
    /// Cumulative delay in samples: entry `i` is the samples waited before
    /// command `i`, and the last entry (index `len`) is the stream's total.
    /// The counterpart of `Song::delay_prefix`, and what makes the timeline
    /// questions -- "when does row N play", "what plays at 40%" -- O(log n)
    /// instead of a stream walk. Rebuilt with the index on every splice.
    wait_prefix: Vec<u64>,
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
            // The 256 MiB gunzip ceiling bounds the body, but the index built
            // here amplifies it: `offsets` costs 4 bytes a command and
            // `wait_prefix` 8, and a `0x00` byte is a one-byte command, so a body
            // of nothing but `0x00` would be a twelvefold blow-up -- ~3 GB of
            // index on wasm32's 4 GiB. Cap the command count so that cannot land.
            if offsets.len() >= MAX_COMMANDS {
                return Err(Error::file(format!(
                    "VGM data holds more than {MAX_COMMANDS} commands"
                )));
            }
            let size = command_size(&data[at..], version)?;
            offsets
                .push(u32::try_from(at).map_err(|_| Error::file("VGM data is larger than 4 GiB"))?);
            at += size;
        };
        let tail = u32::try_from(tail).map_err(|_| Error::file("VGM data is larger than 4 GiB"))?;
        let mut wait_prefix = Vec::with_capacity(offsets.len() + 1);
        let mut elapsed = 0u64;
        wait_prefix.push(elapsed);
        for &offset in &offsets {
            elapsed += u64::from(command_wait(&decode(&data[offset as usize..])));
            wait_prefix.push(elapsed);
        }
        Ok(Self {
            data,
            offsets,
            tail,
            version,
            wait_prefix,
        })
    }

    /// Every byte of the body, exactly as it sits in the file.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// The commands, without the end marker or anything after it.
    ///
    /// What an OPL [`VgmData`](crate::vgm::VgmData) snapshot is built over: it
    /// stores the stream minus its marker, which is exactly this span.
    #[must_use]
    pub fn commands(&self) -> &[u8] {
        &self.data[..self.tail as usize]
    }

    /// Where each command starts, in the same order as the rows.
    ///
    /// Handed to a snapshot so it need not re-walk a stream this has already
    /// walked -- and so a stream carrying a command the OPL table cannot size
    /// still yields a usable snapshot.
    pub(crate) fn offsets(&self) -> &[u32] {
        &self.offsets
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
        self.get(index).map_or(0, |command| command_wait(&command))
    }

    /// The stream's total length in samples, summed from its waits.
    #[must_use]
    pub fn total_samples(&self) -> u64 {
        *self
            .wait_prefix
            .last()
            .expect("the prefix always has len() + 1 entries")
    }

    /// The samples waited before command `index` -- the time at which it plays.
    /// `index == len()` (and beyond) yields the stream's total.
    #[must_use]
    pub fn samples_before(&self, index: usize) -> u64 {
        self.wait_prefix[index.min(self.len())]
    }

    /// The samples waited from command `index` to the end of the stream --
    /// which is exactly what the header's `loop # samples` field means when
    /// `index` is the loop point.
    #[must_use]
    pub fn samples_from(&self, index: usize) -> u64 {
        self.total_samples() - self.samples_before(index)
    }

    /// The command a seek to `target` samples lands on.
    ///
    /// Playback resumes *before* the target when the target falls inside a
    /// delay, stopping on that delay rather than overshooting; a target on a
    /// boundary lands on the first command at that time. May return `len()`,
    /// meaning "past the last command". The same rule as
    /// [`Song::seek_index_for_ms`](crate::Song::seek_index_for_ms), so the two
    /// engines agree about what seeking means.
    #[must_use]
    pub fn seek_index_for_samples(&self, target: u64) -> usize {
        let target = target.min(self.total_samples());
        let first_at_or_after = self.wait_prefix.partition_point(|&offset| offset < target);
        if self.wait_prefix.get(first_at_or_after) == Some(&target) {
            first_at_or_after
        } else {
            // The target fell strictly inside a delay: stop on that delay.
            first_at_or_after.saturating_sub(1)
        }
    }

    /// Maps a position along the waveform (`0.0 ..= 1.0`) to a command and the
    /// samples elapsed when it plays.
    ///
    /// The returned samples always equal `samples_before(index)`, so selecting
    /// the row and seeking to it agree. `None` for an empty stream or a
    /// non-finite percentage. The counterpart of
    /// [`Song::index_and_ms_offset_at_pct`](crate::Song::index_and_ms_offset_at_pct),
    /// with the same boundary rules.
    #[must_use]
    pub fn index_at_pct(&self, position_pct: f64) -> Option<(usize, u64)> {
        if self.is_empty() || !position_pct.is_finite() {
            return None;
        }
        // Compare in f64: the target rarely lands on a whole sample, and
        // rounding first would move the boundary between two commands.
        let target = self.total_samples() as f64 * position_pct.clamp(0.0, 1.0);
        let first_at_or_after = self
            .wait_prefix
            .partition_point(|&offset| (offset as f64) < target);

        let index = match self.wait_prefix.get(first_at_or_after) {
            Some(&offset) if offset as f64 == target => first_at_or_after,
            _ => first_at_or_after.saturating_sub(1),
        };
        let index = index.min(self.len() - 1);
        Some((index, self.wait_prefix[index]))
    }

    /// Removes the commands at `indices`, leaving everything else -- the end
    /// marker and any trailing bytes included -- exactly where it was relative
    /// to what survives.
    ///
    /// Out-of-range and repeated indices are ignored, as they are for the OPL
    /// stream. Deleting a data block takes its whole payload with it; that is
    /// the intent, and later commands referring to it are the caller's warning
    /// to give, not this module's veto.
    pub fn delete_many(&mut self, indices: &[usize]) {
        let len = self.len();
        let Some(byte_ranges) = byte_ranges_to_delete(indices, len, |index| {
            self.byte_offset(index).expect("an index inside the stream")
        }) else {
            return;
        };
        splice_out(&mut self.data, &byte_ranges);
        self.reindex();
    }

    /// Puts commands back where they were, for undo. Each entry is
    /// `(index_after_reinsertion, bytes)`, ascending.
    pub(crate) fn insert_many(&mut self, entries: &[InsertEntry]) {
        if entries.is_empty() {
            return;
        }
        self.data = splice_in(&self.data, entries, |index| {
            self.byte_offset(index).expect("an index inside the stream")
        });
        self.reindex();
    }

    /// Re-walks the stream after a splice.
    fn reindex(&mut self) {
        let rebuilt = Self::parse(std::mem::take(&mut self.data), self.version)
            .expect("splicing whole commands cannot corrupt them");
        *self = rebuilt;
    }

    /// The next command matching `target`, strictly after (or before) `start`.
    ///
    /// The multichip counterpart of
    /// [`Song::find_next_instruction`](crate::Song::find_next_instruction),
    /// with the same "strictly after / strictly before" boundary so repeated
    /// Find Next walks the stream without sticking.
    #[must_use]
    pub fn find_next(
        &self,
        start: usize,
        target: VgmFindTarget,
        look_backwards: bool,
    ) -> Option<usize> {
        let len = self.len();
        let matches = |index: usize| self.matches_target(index, target);
        if look_backwards {
            (0..start.min(len)).rev().find(|&index| matches(index))
        } else {
            (start.saturating_add(1)..len).find(|&index| matches(index))
        }
    }

    /// Whether command `index` matches `target`.
    fn matches_target(&self, index: usize, target: VgmFindTarget) -> bool {
        match target {
            // Any command that advances time -- a wait, or a DAC write that
            // also waits. The same set the delay navigation steps through.
            VgmFindTarget::AnyDelay => self.wait_samples(index) > 0,
            VgmFindTarget::Write {
                kind,
                instance,
                addr,
            } => matches!(
                self.get(index),
                Some(VgmCommand::Write { target, addr: a, .. })
                    if target.kind == kind
                        && instance.is_none_or(|i| target.instance == i)
                        && addr.is_none_or(|addr| a == addr)
            ),
        }
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
            VgmCommand::Wait(samples) => format!("delay {samples}"),
            VgmCommand::DacWrite { wait } => format!("YM2612 DAC, delay {wait}"),
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
                format!("override delay {which:#04X} = {samples}")
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

    /// Padding is stepped over, and everything else undefined still stops the
    /// walk -- which is what makes stepping over padding safe.
    #[test]
    fn a_zero_byte_is_padding_and_the_rest_of_the_undefined_range_is_not() {
        assert_eq!(command_size(&[PADDING], 0x151).unwrap(), 1);
        for opcode in [0x01u8, 0x2F, 0x60, 0x65, 0x69, 0x6F] {
            assert!(
                command_size(&[opcode], 0x151).is_err(),
                "{opcode:#04X} should still stop the walk"
            );
        }
    }

    #[test]
    fn a_stream_with_padding_around_its_commands_walks() {
        // Twelve zeros, a write, a stray zero, a wait, and the end -- the two
        // shapes the corpus actually contains, in one stream.
        let mut bytes = vec![PADDING; 12];
        bytes.extend_from_slice(&[0x5A, 0x20, 0x01, PADDING, 0x61, 0x2C, 0x00, END_OF_DATA]);
        let stream = VgmStream::parse(bytes, 0x151).expect("it walks");
        assert_eq!(stream.len(), 15, "twelve pads, a write, a pad, a wait");
        assert_eq!(stream.raw_command(12), Some([0x5A, 0x20, 0x01].as_slice()));
        assert_eq!(stream.raw_command(13), Some([PADDING].as_slice()));
        assert_eq!(stream.total_samples(), 0x2C);
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
        let cases: [(&[u8], ChipKind, u8, u8); 14] = [
            (&[0x50, 0x9F], ChipKind::Sn76489, 0, 0),
            (&[0x52, 0x28, 0xF0], ChipKind::Ym2612, 0, 0),
            (&[0x53, 0x28, 0xF0], ChipKind::Ym2612, 0, 1),
            (&[0x5E, 0x05, 0x01], ChipKind::Ymf262, 0, 0),
            (&[0x5F, 0x05, 0x01], ChipKind::Ymf262, 0, 1),
            (&[0xA2, 0x28, 0xF0], ChipKind::Ym2612, 1, 0),
            (&[0xB3, 0x10, 0x80], ChipKind::GameBoyDmg, 0, 0),
            (&[0xBF, 0x10, 0x80], ChipKind::Ga20, 0, 0),
            // The memory-versus-register pairs, where the port is what keeps
            // one from being read as the other: the RF chips, and the
            // WonderSwan's wave RAM against its register file. The MultiPCM's
            // bank select is the same shape one space further out -- neither
            // registers nor memory, so neither port 0 nor `MEMORY_PORT`.
            (&[0xB1, 0x07, 0xFF], ChipKind::Rf5c164, 0, 0),
            (&[0xC2, 0x00, 0x08, 0xFF], ChipKind::Rf5c164, 0, MEMORY_PORT),
            (&[0xB5, 0x02, 0x11], ChipKind::MultiPcm, 0, 0),
            (&[0xC3, 0x02, 0x40, 0x00], ChipKind::MultiPcm, 0, BANK_PORT),
            (&[0xBC, 0x0F, 0x20], ChipKind::WonderSwan, 0, 0),
            (
                &[0xC6, 0x01, 0x80, 0x8E],
                ChipKind::WonderSwan,
                0,
                MEMORY_PORT,
            ),
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
    /// the 16-bit-addressed range marks it in bit 15 of the address instead.
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

    /// The flag is bit 15 of the *assembled* address, so it moves between bytes
    /// with the byte order: byte 2 for `0xC0`'s little-endian address, byte 1
    /// for `0xC5`-`0xC8`'s big-endian one.
    #[test]
    fn a_high_low_address_byte_is_an_address_not_a_second_chip() {
        // Little-endian: the low byte comes first, and its top bit is address.
        let VgmCommand::Write { target, addr, .. } = decode(&[0xC0, 0xB4, 0x12, 0x01]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::SegaPcm, 0));
        assert_eq!(addr, 0x12B4);

        // Big-endian: the low byte comes second, and its top bit is address.
        for (opcode, kind) in [
            (0xC5u8, ChipKind::Scsp),
            (0xC6, ChipKind::WonderSwan),
            (0xC7, ChipKind::Vsu),
            (0xC8, ChipKind::X1010),
        ] {
            let VgmCommand::Write { target, addr, .. } = decode(&[opcode, 0x1F, 0x80, 0x42]) else {
                panic!("a write");
            };
            assert_eq!(
                (target.kind, target.instance),
                (kind, 0),
                "{opcode:#04X} 1F 80 is one chip's address 0x1F80"
            );
            assert_eq!(addr, 0x1F80);

            // ...and bit 7 of byte 1 is what marks the second instance, which
            // the mask then clears -- upstream's `Cmd_Ofs16_Data8`.
            let VgmCommand::Write { target, addr, .. } = decode(&[opcode, 0x9F, 0x80, 0x42]) else {
                panic!("a write");
            };
            assert_eq!((target.kind, target.instance), (kind, 1), "{opcode:#04X}");
            assert_eq!(addr, 0x1F80, "the flag is not part of the address");
        }
    }

    /// `0xB2 ad dd` is upstream's `Cmd_Ofs4_Data12`: a nibble register in bits
    /// 6-4 and a twelve-bit value split across the low nibble and the second
    /// byte, big-endian -- not the range's usual `aa dd`.
    #[test]
    fn the_pwm_write_is_a_nibble_register_and_a_12_bit_value() {
        // "left duty <- 0x2FF" is `B2 22 FF`.
        let VgmCommand::Write { target, addr, data } = decode(&[0xB2, 0x22, 0xFF]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::Pwm, 0));
        assert_eq!(addr, 0x02, "bits 6-4, not the whole operand");
        assert_eq!(data, 0x02FF, "the value spans both operands");

        // Bit 7 is the second chip, and no part of the register.
        let VgmCommand::Write { target, addr, data } = decode(&[0xB2, 0xA2, 0xFF]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::Pwm, 1));
        assert_eq!((addr, data), (0x02, 0x02FF), "the flag is not data either");
    }

    /// `0x31 dd` is `Cmd_AY_Stereo`: a six-bit mask on its own port, with bit
    /// 6 choosing a YM2203's SSG over the AY8910 and bit 7 the second chip.
    #[test]
    fn the_ay_stereo_mask_is_a_player_instruction_not_a_register_write() {
        let VgmCommand::Write { target, addr, data } = decode(&[0x31, 0x25]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::Ay8910, 0));
        assert_eq!(target.port, STEREO_PORT, "not the register file");
        assert_eq!((addr, data), (0, 0x25));

        // Bit 6: the same mask, aimed at a YM2203's SSG section.
        let VgmCommand::Write { target, data, .. } = decode(&[0x31, 0x65]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.port), (ChipKind::Ym2203, STEREO_PORT));
        assert_eq!(data, 0x25, "bit 6 routes; it is not part of the mask");

        // Bit 7: the second chip, of whichever kind bit 6 chose.
        let VgmCommand::Write { target, data, .. } = decode(&[0x31, 0xA5]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::Ay8910, 1));
        assert_eq!(data, 0x25);
    }

    /// `0xC3 cc bbaa` is a bank select, not an addressed write: `cc` is a mask
    /// naming which of the MultiPCM's two 512 KiB banks to move (upstream's
    /// `Cmd_YMW_Bank` takes `fData[0x01] & 0x03`) and `bbaa` is a little-endian
    /// offset in 64 KiB units.
    #[test]
    fn the_multipcm_bank_select_keeps_its_mask_and_its_offset_apart() {
        // Daytona USA's, and 125 of the corpus's 296 bank commands: both banks
        // to 0x10 * 64 KiB = the megabyte at 0x10_0000.
        let VgmCommand::Write { target, addr, data } = decode(&[0xC3, 0x03, 0x10, 0x00]) else {
            panic!("a write");
        };
        assert_eq!(target.kind, ChipKind::MultiPcm);
        assert_eq!(target.port, BANK_PORT);
        assert_eq!(addr, 0x03, "the bank mask, not an address");
        assert_eq!(data, 0x0010, "the offset whole, not its low byte");

        // The high offset byte is upstream's ">16 MB ROM" case: it ignores the
        // byte, so nothing in the corpus sets it, but the decoder still carries
        // what the command carries.
        let VgmCommand::Write { addr, data, .. } = decode(&[0xC3, 0x01, 0x28, 0x01]) else {
            panic!("a write");
        };
        assert_eq!((addr, data), (0x01, 0x0128));

        // Bit 7 of the mask byte is the second instance -- byte 1, as
        // `Cmd_YMW_Bank` reads it, and not bit 15 of an address word that does
        // not exist here. No corpus file sets it.
        let VgmCommand::Write { target, addr, .. } = decode(&[0xC3, 0x82, 0x20, 0x00]) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::MultiPcm, 1));
        assert_eq!(addr, 0x02, "the flag is not part of the mask");
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

    /// The table says "delay", whichever of the six spellings the file used --
    /// the app's word for a time advance, not the spec's "wait".
    #[test]
    fn every_delay_spelling_describes_as_a_delay() {
        let bytes = vec![
            0x61,
            0x20,
            0x4E, // 20000 samples
            0x62, // one 60 Hz frame
            0x63, // one 50 Hz frame
            0x70, // shortest short form
            0x8F, // DAC write plus 15 samples
            0x64,
            0x62,
            0x10,
            0x27, // override 0x62 to 10000
            END_OF_DATA,
        ];
        let stream = VgmStream::parse(bytes, 0x160).unwrap();
        assert_eq!(stream.describe(0), "delay 20000");
        assert_eq!(stream.describe(1), "delay 735");
        assert_eq!(stream.describe(2), "delay 882");
        assert_eq!(stream.describe(3), "delay 1");
        assert_eq!(stream.describe(4), "YM2612 DAC, delay 15");
        assert_eq!(stream.describe(5), "override delay 0x62 = 10000");
    }

    /// find_next over a small two-chip stream: delays, a specific chip, a
    /// specific register, a specific instance, and backwards.
    #[test]
    fn find_next_walks_chips_registers_and_delays() {
        let bytes = vec![
            0x50,
            0x9F, // 0: SN76489 write
            0x52,
            0x28,
            0xF0, // 1: YM2612 p0, addr 0x28
            0x61,
            0x64,
            0x00, // 2: delay 100
            0xA2,
            0x28,
            0x00, // 3: YM2612 #2 p0, addr 0x28
            0x52,
            0x2A,
            0x80, // 4: YM2612 p0, addr 0x2A
            0x61,
            0xC8,
            0x00, // 5: delay 200
            END_OF_DATA,
        ];
        let stream = VgmStream::parse(bytes, 0x160).unwrap();
        use VgmFindTarget::{AnyDelay, Write};

        // Any delay, forwards from the top.
        assert_eq!(stream.find_next(0, AnyDelay, false), Some(2));
        assert_eq!(stream.find_next(2, AnyDelay, false), Some(5));
        assert_eq!(stream.find_next(5, AnyDelay, false), None);
        // Backwards from the end.
        assert_eq!(stream.find_next(6, AnyDelay, true), Some(5));
        assert_eq!(stream.find_next(5, AnyDelay, true), Some(2));

        // Any write to the YM2612 (either instance, any register).
        let any_ym = Write {
            kind: ChipKind::Ym2612,
            instance: None,
            addr: None,
        };
        assert_eq!(stream.find_next(0, any_ym, false), Some(1));
        assert_eq!(stream.find_next(1, any_ym, false), Some(3));
        assert_eq!(stream.find_next(3, any_ym, false), Some(4));

        // A specific register on the YM2612.
        let key = Write {
            kind: ChipKind::Ym2612,
            instance: None,
            addr: Some(0x28),
        };
        assert_eq!(stream.find_next(0, key, false), Some(1));
        assert_eq!(
            stream.find_next(1, key, false),
            Some(3),
            "instance 2's 0x28"
        );
        assert_eq!(stream.find_next(3, key, false), None);

        // A specific instance.
        let second = Write {
            kind: ChipKind::Ym2612,
            instance: Some(1),
            addr: None,
        };
        assert_eq!(stream.find_next(0, second, false), Some(3));
        assert_eq!(stream.find_next(3, second, false), None);

        // A chip that never writes.
        let none = Write {
            kind: ChipKind::Ay8910,
            instance: None,
            addr: None,
        };
        assert_eq!(stream.find_next(0, none, false), None);
    }

    /// A write, delay 100, a write, delay 200 -- prefix `[0, 0, 100, 100, 300]`.
    fn two_delay_stream() -> VgmStream {
        let bytes = vec![
            0x50,
            0x9F, // write
            0x61,
            0x64,
            0x00, // delay 100
            0x50,
            0x8E, // write
            0x61,
            0xC8,
            0x00, // delay 200
            END_OF_DATA,
        ];
        VgmStream::parse(bytes, 0x151).unwrap()
    }

    /// The seek rule `Song` established: a target inside a delay stops *on*
    /// that delay, a boundary target lands on the first command at that time.
    #[test]
    fn seeking_by_samples_stops_on_the_delay() {
        let stream = two_delay_stream();
        assert_eq!(stream.seek_index_for_samples(0), 0);
        assert_eq!(stream.seek_index_for_samples(50), 1, "inside the delay");
        assert_eq!(stream.seek_index_for_samples(100), 2, "on the boundary");
        assert_eq!(stream.seek_index_for_samples(150), 3);
        assert_eq!(stream.seek_index_for_samples(300), 4, "the end is len()");
        assert_eq!(stream.seek_index_for_samples(999), 4, "clamped to the end");
    }

    /// The waveform contract: the returned samples always equal
    /// `samples_before(index)`, so a click and the row it selects agree.
    #[test]
    fn pct_maps_to_a_command_and_its_own_start_time() {
        let stream = two_delay_stream();
        assert_eq!(stream.index_at_pct(0.0), Some((0, 0)));
        // A sixth of 300 samples falls inside the first delay.
        let (index, samples) = stream.index_at_pct(1.0 / 6.0).unwrap();
        assert_eq!((index, samples), (1, 0));
        assert_eq!(samples, stream.samples_before(index));
        // The far edge clamps to the last command, not past it.
        assert_eq!(stream.index_at_pct(1.0), Some((3, 100)));
        assert_eq!(stream.index_at_pct(f64::NAN), None);
        assert_eq!(
            VgmStream::parse(vec![END_OF_DATA], 0x151)
                .unwrap()
                .index_at_pct(0.5),
            None,
            "an empty stream has no rows to land on"
        );
    }

    /// The prefix is rebuilt with the index: a splice cannot leave stale time.
    #[test]
    fn editing_rebuilds_the_prefix() {
        let mut stream = two_delay_stream();
        assert_eq!(stream.total_samples(), 300);
        stream.delete_many(&[1]); // drop the delay 100
        assert_eq!(stream.total_samples(), 200);
        assert_eq!(stream.samples_before(2), 0, "the second write moved up");
        assert_eq!(stream.samples_before(3), 200);
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
        // same waits. This is the projection the OPL editor leans on.
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

        /// The prefix sum is a cache, and this is its definition: every timing
        /// question answers exactly what a naive walk of the waits would say,
        /// whatever the stream holds.
        #[test]
        fn the_wait_prefix_matches_a_naive_walk(
            commands in prop::collection::vec(any_command(), 0..40),
        ) {
            let mut bytes: Vec<u8> = commands.iter().flatten().copied().collect();
            bytes.push(END_OF_DATA);
            let stream = VgmStream::parse(bytes, 0x172)?;

            let mut elapsed = 0u64;
            for index in 0..stream.len() {
                prop_assert_eq!(stream.samples_before(index), elapsed);
                elapsed += u64::from(stream.wait_samples(index));
            }
            prop_assert_eq!(stream.total_samples(), elapsed);
            for index in 0..=stream.len() {
                prop_assert_eq!(
                    stream.samples_from(index),
                    elapsed - stream.samples_before(index)
                );
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
