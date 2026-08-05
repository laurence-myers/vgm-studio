//! Reading a VGM stream as OPL instructions.
//!
//! There is one VGM model -- a header, a command stream and a tag -- and it
//! holds a Mega Drive rip and an AdLib rip alike. OPL is not a second kind of
//! document; it is a *capability* of the chips a file declares. This module is
//! where that capability lives: given a stream whose chips are all OPL-family,
//! it presents the rows the editor's OPL features understand.
//!
//! # A projection, not a conversion
//!
//! Nothing is copied or decoded up front. [`OplProjection`] borrows the stream
//! and decodes on access, exactly as [`VgmData`](super::VgmData) did -- it is
//! on the path of every table-row paint, so it allocates nothing.
//!
//! # Rows that are not OPL
//!
//! An OPL file may still carry a command that is not an OPL instruction: a
//! `0x67` data block, a reserved opcode. Those project to `None`. They keep
//! their place in the row numbering, describe themselves generically, and --
//! because they wait for nothing -- leave every timing untouched. This is what
//! lets a rip carrying a data block open in the editor at all, without
//! `Instruction` having to grow an arm for something no OPL consumer could
//! act on.

use crate::song::OplType;
use crate::song::instruction::{Bank, DelayKind, Instruction};
use crate::vgm::data::command;
use crate::vgm::header::{ChipKind, VgmHeader};
use crate::vgm::stream::VgmStream;

/// Which OPL a header's chip clocks declare, or `None` if it declares none.
///
/// The dual-OPL2 marker is the second-chip bit on the YM3812 clock, which
/// `dro2vgm` writes alongside a meaningless bit 31 -- the header model reads
/// the two apart, so only the one that means something is consulted.
///
/// **Other declared chips are not consulted, deliberately.** Real rips declare
/// a chip their stream never writes to (the corpus has them), and refusing
/// those an OPL type would take the register analysis away from files that
/// have had it for years. What guards against half-reading a genuinely
/// multi-chip file is the *stream* test, [`is_wholly_opl`] -- the two together
/// are exactly the rule the old OPL-only reader enforced.
#[must_use]
pub fn opl_type_of(header: &VgmHeader) -> Option<OplType> {
    let chip = |kind| header.chips().iter().find(|chip| chip.kind == kind);
    match (chip(ChipKind::Ym3812), chip(ChipKind::Ymf262)) {
        (Some(ym3812), _) if ym3812.dual => Some(OplType::DualOpl2),
        (Some(_), _) => Some(OplType::Opl2),
        (None, Some(_)) => Some(OplType::Opl3),
        (None, None) => None,
    }
}

/// A VGM stream seen as OPL instructions.
///
/// Built by [`VgmFile::opl`](super::VgmFile::opl), which is the gate: it exists
/// only when the file's chips are an OPL set.
#[derive(Debug, Clone, Copy)]
pub struct OplProjection<'a> {
    stream: &'a VgmStream,
    opl_type: OplType,
}

impl<'a> OplProjection<'a> {
    pub(crate) const fn new(stream: &'a VgmStream, opl_type: OplType) -> Self {
        Self { stream, opl_type }
    }

    /// The OPL this file declares.
    #[must_use]
    pub const fn opl_type(&self) -> OplType {
        self.opl_type
    }

    /// The stream underneath, for the chip-agnostic questions.
    #[must_use]
    pub const fn stream(&self) -> &'a VgmStream {
        self.stream
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.stream.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.stream.is_empty()
    }

    /// The OPL instruction at `index`, or `None` for a command that is not one.
    ///
    /// The bank is the write opcode's own: the YMF262's second port and the
    /// second YM3812 of a dual pair are the high bank, everything else the low
    /// one.
    #[must_use]
    pub fn instruction(&self, index: usize) -> Option<Instruction> {
        let bytes = self.stream.raw_command(index)?;
        project(bytes)
    }
}

/// Decodes one command's bytes as an OPL instruction.
///
/// The whole OPL command table, in one place: four write opcodes and four ways
/// to spell a wait. Anything else -- including the `0x66` end marker, which the
/// stream index never yields -- is not an OPL instruction.
#[must_use]
pub fn project(bytes: &[u8]) -> Option<Instruction> {
    let opcode = *bytes.first()?;
    let operand = |at: usize| bytes.get(at).copied().unwrap_or(0);

    let register = |bank: Bank| {
        Some(Instruction::Register {
            reg: operand(1),
            value: operand(2),
            bank: Some(bank),
        })
    };
    let wait = |kind: DelayKind, samples: u32| Some(Instruction::DelaySamples { kind, samples });

    match opcode {
        command::YM3812 | command::YMF262_PORT_0 => register(Bank::Low),
        command::YMF262_PORT_1 | command::YM3812_CHIP_2 => register(Bank::High),
        command::WAIT => wait(
            DelayKind::Long,
            u32::from(operand(1)) | (u32::from(operand(2)) << 8),
        ),
        command::WAIT_60TH => wait(DelayKind::Short, command::SAMPLES_60TH),
        command::WAIT_50TH => wait(DelayKind::Short, command::SAMPLES_50TH),
        short @ command::SHORT_WAIT_BASE..=command::SHORT_WAIT_LAST => {
            wait(DelayKind::Short, u32::from(short & 0x0F) + 1)
        }
        _ => None,
    }
}

/// Whether every command in `stream` projects to an OPL instruction.
///
/// The question the old reader answered by refusing to open the file at all.
/// A `false` is no longer a rejection -- the file opens and trims like any
/// other VGM -- but it does withhold the OPL extras, and that is deliberate:
/// the analyser and the synth read a stream as if it were wholly OPL, so
/// letting them near one that is not would silently drop whatever else it
/// writes. A file that fails this is a multi-chip VGM, whatever its header
/// says.
#[must_use]
pub fn is_wholly_opl(stream: &VgmStream) -> bool {
    (0..stream.len()).all(|index| stream.raw_command(index).and_then(project).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgm::stream::END_OF_DATA;

    fn stream(bytes: Vec<u8>) -> VgmStream {
        VgmStream::parse(bytes, 0x151).unwrap()
    }

    fn opl_projection(stream: &VgmStream) -> OplProjection<'_> {
        OplProjection::new(stream, OplType::Opl3)
    }

    #[test]
    fn every_opl_write_projects_to_its_own_bank() {
        let stream = stream(vec![
            0x5A,
            0x20,
            0x01, // YM3812        -> low
            0x5E,
            0x21,
            0x02, // YMF262 port 0 -> low
            0x5F,
            0x22,
            0x03, // YMF262 port 1 -> high
            0xAA,
            0x23,
            0x04, // YM3812 chip 2 -> high
            END_OF_DATA,
        ]);
        let opl = opl_projection(&stream);
        let reg = |reg, value, bank| {
            Some(Instruction::Register {
                reg,
                value,
                bank: Some(bank),
            })
        };
        assert_eq!(opl.instruction(0), reg(0x20, 0x01, Bank::Low));
        assert_eq!(opl.instruction(1), reg(0x21, 0x02, Bank::Low));
        assert_eq!(opl.instruction(2), reg(0x22, 0x03, Bank::High));
        assert_eq!(opl.instruction(3), reg(0x23, 0x04, Bank::High));
        assert_eq!(opl.len(), 4);
    }

    #[test]
    fn every_wait_spelling_projects_with_its_kind() {
        let stream = stream(vec![
            0x61,
            0x34,
            0x12, // long
            0x62, // short, 735
            0x63, // short, 882
            0x70, // short, 1
            0x7F, // short, 16
            END_OF_DATA,
        ]);
        let opl = opl_projection(&stream);
        let long = |samples| {
            Some(Instruction::DelaySamples {
                kind: DelayKind::Long,
                samples,
            })
        };
        let short = |samples| {
            Some(Instruction::DelaySamples {
                kind: DelayKind::Short,
                samples,
            })
        };
        assert_eq!(opl.instruction(0), long(0x1234));
        assert_eq!(opl.instruction(1), short(735));
        assert_eq!(opl.instruction(2), short(882));
        assert_eq!(opl.instruction(3), short(1));
        assert_eq!(opl.instruction(4), short(16));
    }

    /// The gap this closes: a data block inside an otherwise-OPL file. It keeps
    /// its row, projects to nothing, and waits for nothing.
    #[test]
    fn a_data_block_is_a_row_that_is_simply_not_an_opl_instruction() {
        let mut bytes = vec![0x5A, 0x20, 0x01];
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 4, 0, 0, 0, 1, 2, 3, 4]);
        bytes.extend_from_slice(&[0x62, END_OF_DATA]);
        let stream = stream(bytes);
        let opl = opl_projection(&stream);

        assert_eq!(opl.len(), 3, "the block holds a row of its own");
        assert!(opl.instruction(0).is_some());
        assert_eq!(opl.instruction(1), None, "not an OPL instruction");
        assert!(opl.instruction(2).is_some());
        assert!(!is_wholly_opl(&stream));
        assert_eq!(
            stream.total_samples(),
            735,
            "and it contributes no time, so nothing shifts"
        );
    }

    #[test]
    fn a_wholly_opl_stream_says_so() {
        assert!(is_wholly_opl(&stream(vec![
            0x5A,
            0x20,
            0x01,
            0x62,
            END_OF_DATA
        ])));
    }

    #[test]
    fn out_of_range_rows_project_to_nothing() {
        let stream = stream(vec![0x62, END_OF_DATA]);
        assert_eq!(opl_projection(&stream).instruction(1), None);
        assert_eq!(opl_projection(&stream).instruction(99), None);
    }

    // -- which chip sets are OPL ---------------------------------------------

    fn header(chips: &[(ChipKind, u32)]) -> VgmHeader {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        bytes[0x08..0x0C].copy_from_slice(&0x151u32.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        for &(kind, clock) in chips {
            let at = kind.clock_offset();
            bytes[at..at + 4].copy_from_slice(&clock.to_le_bytes());
        }
        bytes.push(END_OF_DATA);
        VgmHeader::parse(&bytes).unwrap()
    }

    #[test]
    fn the_three_opl_shapes_are_recognised() {
        const DUAL: u32 = 3_579_545 | 0xC000_0000;
        assert_eq!(
            opl_type_of(&header(&[(ChipKind::Ym3812, 3_579_545)])),
            Some(OplType::Opl2)
        );
        assert_eq!(
            opl_type_of(&header(&[(ChipKind::Ym3812, DUAL)])),
            Some(OplType::DualOpl2)
        );
        assert_eq!(
            opl_type_of(&header(&[(ChipKind::Ymf262, 14_318_180)])),
            Some(OplType::Opl3)
        );
    }

    /// A declared chip the stream never writes to does not cost a file its OPL
    /// type. Real rips do this -- the corpus has two -- and they have had the
    /// register analysis for years. The stream test below is what disqualifies
    /// a file that genuinely uses more than one chip.
    #[test]
    fn a_declared_but_unwritten_chip_does_not_disqualify_the_header() {
        assert_eq!(
            opl_type_of(&header(&[
                (ChipKind::Ym3812, 3_579_545),
                (ChipKind::Okim6295, 1_000_000),
            ])),
            Some(OplType::Opl2)
        );
    }

    /// Where both OPLs are declared, the YM3812 wins -- the precedence the OPL
    /// reader has always applied, kept so no file changes its mind about which
    /// chip it is.
    #[test]
    fn the_ym3812_takes_precedence_over_a_ymf262() {
        assert_eq!(
            opl_type_of(&header(&[
                (ChipKind::Ym3812, 3_579_545),
                (ChipKind::Ymf262, 14_318_180),
            ])),
            Some(OplType::Opl2)
        );
    }

    /// The guard that matters: a stream writing to something other than the OPL
    /// is not an OPL stream, whatever its header declares. Letting the analyser
    /// or the synth at one would silently drop everything else it writes.
    #[test]
    fn a_stream_that_writes_another_chip_is_not_wholly_opl() {
        let opl_and_psg = stream(vec![
            0x5A,
            0x20,
            0x01, // YM3812
            0x50,
            0x9F, // SN76489
            END_OF_DATA,
        ]);
        assert!(!is_wholly_opl(&opl_and_psg));
        assert!(is_wholly_opl(&stream(vec![0x5A, 0x20, 0x01, END_OF_DATA])));
    }

    #[test]
    fn a_file_with_no_chips_or_other_chips_is_not_opl() {
        assert_eq!(opl_type_of(&header(&[])), None);
        assert_eq!(opl_type_of(&header(&[(ChipKind::Ym2612, 7_670_454)])), None);
    }

    /// The OPL cousins are OPL-family chips, but the editor models the two the
    /// synth can play. They stay ordinary VGMs until a core exists.
    #[test]
    fn the_opl_cousins_are_not_claimed() {
        assert_eq!(opl_type_of(&header(&[(ChipKind::Ym3526, 3_579_545)])), None);
        assert_eq!(opl_type_of(&header(&[(ChipKind::Y8950, 3_579_545)])), None);
    }
}
