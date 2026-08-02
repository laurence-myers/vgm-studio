//! What version a file actually needs, and what it claims.
//!
//! A VGM's version field is a promise about which header fields and which
//! commands a player must understand. Rippers and converters are generous with
//! it: a file that uses nothing past 1.51 is routinely stamped 1.71, which costs
//! a hundred and sixty bytes of zero-padded header and shuts out players that
//! stop at the version they know.
//!
//! [`minimum_version`] works out what a file genuinely requires -- the highest
//! of its floor, its chips, its commands and its header fields -- so a converter
//! can stamp the truth and an editor can offer to bring an over-claimed file
//! down. Coming *down* is the operation that needs care, and
//! [`can_downgrade_to`] is what refuses when a field or a command would be lost.
//!
//! Written from the spec's own version notes, chip by chip and opcode by opcode.
//!
//! **Not part of [`audit`](crate::vgm::audit), deliberately.** That module's
//! contract is that a non-empty result is worth interrupting the user for, and
//! it holds because a header disagreeing with its own stream is rare. An
//! over-claimed version is not rare -- it is the norm -- so reporting it there
//! would make Edit > Fix Header open a box for almost every file it was shown.
//! This is the computation; its consumer is the deliberate "normalise headers"
//! operation, where the user has asked.

use crate::vgm::header::VgmHeader;
use crate::vgm::stream::{VgmCommand, VgmStream};

/// The floor this app writes: every file it emits carries a data-offset field,
/// which arrived in 1.50.
pub const FLOOR: u32 = 0x0000_0150;

/// The version a file's own contents genuinely require, ignoring the writer's
/// floor: the highest of the chips it clocks, the commands it uses and the
/// header fields it fills in.
///
/// This is the honest answer the [`audit`](crate::vgm::audit) needs. A genuine
/// pre-1.50 file needs less than [`FLOOR`], and folding the floor in here would
/// make the audit report every such file as underclaiming its version -- a
/// false finding, since the file is perfectly valid at the version it declares.
#[must_use]
pub fn content_version(header: &VgmHeader, stream: Option<&VgmStream>) -> u32 {
    let mut version = chips_version(header).max(fields_version(header));
    if let Some(stream) = stream {
        version = version.max(commands_version(stream));
    }
    version
}

/// The version a file's own contents require, never below the writer's floor.
///
/// [`content_version`] raised to [`FLOOR`]: the floor this writer never goes
/// below, because every file it emits carries a data-offset field. This is the
/// answer the converter stamps and the downgrade check uses.
#[must_use]
pub fn minimum_version(header: &VgmHeader, stream: Option<&VgmStream>) -> u32 {
    content_version(header, stream).max(FLOOR)
}

/// The version the file's clocked chips need: each chip's own introduction.
fn chips_version(header: &VgmHeader) -> u32 {
    header
        .chips()
        .iter()
        .map(|chip| {
            // The T6W28 is an SN76489 variant, and the flag that says so is a
            // 1.51 addition even though the chip itself is older.
            if chip.variant && chip.kind == crate::vgm::ChipKind::Sn76489 {
                chip.kind.since_version().max(0x0000_0151)
            } else {
                chip.kind.since_version()
            }
        })
        .max()
        .unwrap_or(0)
}

/// The version the header's non-zero fields need.
fn fields_version(header: &VgmHeader) -> u32 {
    let mut version = 0;
    if header.loop_modifier() != 0 {
        version = version.max(0x0000_0151);
    }
    if header.volume_modifier() != 0 || header.loop_base() != 0 {
        version = version.max(0x0000_0160);
    }
    if header.extra().is_some() {
        version = version.max(0x0000_0170);
    }
    version
}

/// The version the stream's commands need.
fn commands_version(stream: &VgmStream) -> u32 {
    let mut version = 0;
    for index in 0..stream.len() {
        let Some(command) = stream.get(index) else {
            continue;
        };
        version = version.max(match command {
            // A data block is 1.50; a *compressed* one, and the table it decodes
            // against, are 1.60.
            VgmCommand::DataBlock { kind, .. } => {
                if (0x40..=0x7F).contains(&kind) {
                    0x0000_0160
                } else {
                    0x0000_0150
                }
            }
            // PCM RAM writes and the whole DAC stream engine arrived together.
            VgmCommand::PcmRamWrite { .. } | VgmCommand::DacStream { .. } => 0x0000_0160,
            VgmCommand::OverrideWait { .. } => 0x0000_0150,
            // The opcodes whose own version is later than anything above.
            VgmCommand::Raw { opcode } => raw_opcode_version(opcode),
            _ => 0,
        });
    }
    version
}

/// The version a reserved-range opcode needs, for the few the spec has since
/// assigned.
const fn raw_opcode_version(opcode: u8) -> u32 {
    match opcode {
        // `0x31`: the AY8910 stereo mask.
        0x31 => 0x0000_0171,
        // `0x40`: the Mikey write, the newest opcode the spec defines.
        0x40 => 0x0000_0172,
        _ => 0,
    }
}

/// Whether `header` (and its `stream`) can be restamped as `version` without
/// losing anything.
///
/// A downgrade is only safe when the file needs nothing above the target. This
/// is the same question [`minimum_version`] answers, asked the other way round,
/// and it is separate because *why* a downgrade is refused is worth being able
/// to say.
#[must_use]
pub fn can_downgrade_to(header: &VgmHeader, stream: Option<&VgmStream>, version: u32) -> bool {
    minimum_version(header, stream) <= version
}

/// What stops a file being written as `version`, in the order the spec
/// introduced them. Empty when it can.
#[must_use]
pub fn blockers(header: &VgmHeader, stream: Option<&VgmStream>, version: u32) -> Vec<String> {
    let mut blockers = Vec::new();
    for chip in header.chips() {
        if chip.kind.since_version() > version {
            blockers.push(format!(
                "{} needs {}",
                chip.kind.name(),
                crate::vgm::header::format_version(chip.kind.since_version())
            ));
        }
    }
    if fields_version(header) > version {
        blockers.push(format!(
            "a header field needs {}",
            crate::vgm::header::format_version(fields_version(header))
        ));
    }
    if let Some(stream) = stream
        && commands_version(stream) > version
    {
        blockers.push(format!(
            "a command needs {}",
            crate::vgm::header::format_version(commands_version(stream))
        ));
    }
    blockers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgm::ChipKind;

    /// A header declaring `chips`, at `version`, with `stream` as its body.
    fn vgm(version: u32, chips: &[(ChipKind, u32)], stream: &[u8]) -> crate::VgmFile {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, version);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        for (kind, clock) in chips {
            put_u32(&mut bytes, kind.clock_offset(), *clock);
        }
        bytes.extend_from_slice(stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        crate::vgm::file::read("test.vgm", &bytes).expect("a walkable VGM")
    }

    fn minimum_of(file: &crate::VgmFile) -> u32 {
        minimum_version(&file.header, file.stream())
    }

    #[test]
    fn an_ordinary_opl_file_needs_far_less_than_it_claims() {
        // A YM3812's clock field sits at 0x50, and the header only grew that far
        // at 1.51 -- so the chip that predates the format still pins the version
        // there, and nothing in this file needs more.
        let file = vgm(
            0x171,
            &[(ChipKind::Ym3812, 3_579_545)],
            &[0x5A, 0x20, 0x01, 0x66],
        );
        assert_eq!(
            minimum_of(&file),
            0x0000_0151,
            "a 1.71 stamp on a 1.51 file"
        );
    }

    #[test]
    fn an_empty_header_needs_only_the_floor() {
        // Nothing clocked, nothing written: the data-offset field is the only
        // thing this writer insists on.
        let file = vgm(0x171, &[], &[0x66]);
        assert_eq!(minimum_of(&file), FLOOR);
    }

    #[test]
    fn content_version_ignores_the_writer_floor() {
        // A YM2612-only stream with nothing past 1.50 in it: its genuine content
        // need is below the floor the writer adds, so the audit sees the truth.
        let file = vgm(0x171, &[(ChipKind::Ym2612, 7_670_454)], &[0x66]);
        assert!(
            content_version(&file.header, file.stream()) < FLOOR,
            "content alone needs less than the writer's floor"
        );
        assert_eq!(
            minimum_version(&file.header, file.stream()),
            FLOOR,
            "but the writer still never goes below the floor"
        );
    }

    #[test]
    fn a_chip_carries_its_own_introduction() {
        // The C352 is a 1.71 chip whatever else the file does.
        let file = vgm(0x171, &[(ChipKind::C352, 24_192_000)], &[0x66]);
        assert_eq!(minimum_of(&file), 0x0000_0171);
        assert_eq!(ChipKind::C352.since_version(), 0x0000_0171);
    }

    #[test]
    fn the_dac_stream_commands_need_1_60() {
        // `0x90 ss tt pp cc`: the stream setup.
        let file = vgm(
            0x171,
            &[(ChipKind::Ym2612, 7_670_454)],
            &[0x90, 0x00, 0x02, 0x00, 0x2A, 0x66],
        );
        assert_eq!(minimum_of(&file), 0x0000_0160);
    }

    #[test]
    fn an_uncompressed_data_block_is_1_50_and_a_compressed_one_is_1_60() {
        let mut plain = vec![0x67, 0x66, 0x00];
        plain.extend_from_slice(&2u32.to_le_bytes());
        plain.extend_from_slice(&[1, 2, 0x66]);
        let file = vgm(0x171, &[(ChipKind::Ym2612, 7_670_454)], &plain);
        assert_eq!(minimum_of(&file), FLOOR);

        let mut packed = vec![0x67, 0x66, 0x40];
        packed.extend_from_slice(&2u32.to_le_bytes());
        packed.extend_from_slice(&[1, 2, 0x66]);
        let file = vgm(0x171, &[(ChipKind::Ym2612, 7_670_454)], &packed);
        assert_eq!(minimum_of(&file), 0x0000_0160);
    }

    #[test]
    fn a_volume_modifier_needs_1_60_and_a_loop_modifier_1_51() {
        let mut file = vgm(0x171, &[(ChipKind::Ym3812, 3_579_545)], &[0x66]);
        assert_eq!(minimum_of(&file), 0x0000_0151, "the chip's own field");

        file.header.set_loop_counts(0, 3);
        assert_eq!(
            minimum_of(&file),
            0x0000_0151,
            "a loop modifier needs no more"
        );

        file.header.set_volume_modifier(0x20);
        assert_eq!(minimum_of(&file), 0x0000_0160, "and a volume modifier");
    }

    #[test]
    fn a_downgrade_is_refused_with_a_reason() {
        let file = vgm(0x171, &[(ChipKind::C352, 24_192_000)], &[0x66]);
        assert!(!can_downgrade_to(&file.header, file.stream(), 0x0000_0151));
        let why = blockers(&file.header, file.stream(), 0x0000_0151);
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("C352"), "{why:?}");
        assert!(why[0].contains("1.71"), "{why:?}");

        // And allowed where nothing would be lost.
        assert!(can_downgrade_to(&file.header, file.stream(), 0x0000_0171));
        assert!(blockers(&file.header, file.stream(), 0x0000_0171).is_empty());
    }

    #[test]
    fn a_command_blocks_a_downgrade_too() {
        let file = vgm(
            0x171,
            &[(ChipKind::Ym2612, 7_670_454)],
            &[0x90, 0x00, 0x02, 0x00, 0x2A, 0x66],
        );
        let why = blockers(&file.header, file.stream(), FLOOR);
        assert_eq!(why.len(), 1);
        assert!(why[0].contains("command"), "{why:?}");
        assert!(why[0].contains("1.60"), "{why:?}");
    }
}
