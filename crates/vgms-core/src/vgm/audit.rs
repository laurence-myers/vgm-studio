//! Where a VGM's header disagrees with its own command stream, and how that
//! disagreement is shown and (when the user asks) corrected.
//!
//! The reader trusts the header, warns when the two disagree, and leaves the
//! file exactly as it found it. Modelled on `vgm_ptch -Check`.
//!
//! # Never automatic
//!
//! Nothing here runs on save: a file that is only retagged keeps its header
//! byte for byte, so a file the user did not ask to correct is never quietly
//! altered.

use crate::vgm::VgmFile;
use crate::vgm::stream::END_OF_DATA;

/// One way a header disagrees with its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderFinding {
    /// The declared song length is not what the waits sum to.
    TotalSamples { declared: u32, actual: u32 },
    /// The loop offset does not land on the start of a command.
    LoopAdrift,
    /// The declared loop length is not what follows the loop point.
    LoopSamples { declared: u32, actual: u32 },
    /// The command stream runs to the end of its span with no `0x66`.
    MissingEndMarker,
    /// Bytes sit between the end marker and whatever follows it.
    TrailingBytes { count: usize },
    /// The file uses something its declared version does not define.
    ///
    /// Only this direction is reported: declaring more than is needed is
    /// harmless and near-universal, but declaring less is a fault, because a
    /// player that trusts the version may not look for what the file contains.
    VersionUnderclaimed { declared: u32, needed: u32 },
}

impl HeaderFinding {
    /// One line for the confirm box: what is wrong, and what fixing it does.
    #[must_use]
    pub fn describe(&self) -> String {
        match *self {
            Self::TotalSamples { declared, actual } => format!(
                "Song length: the header says {declared} samples, the stream sums to {actual}."
            ),
            Self::LoopAdrift => {
                "Loop point: it does not land on a command, so it would be dropped.".to_owned()
            }
            Self::LoopSamples { declared, actual } => format!(
                "Loop length: the header says {declared} samples, {actual} follow the loop point."
            ),
            Self::MissingEndMarker => {
                "End marker: the command stream has no 0x66, so one would be added.".to_owned()
            }
            Self::TrailingBytes { count } => {
                format!("Trailing data: {count} byte(s) sit after the end marker.")
            }
            Self::VersionUnderclaimed { declared, needed } => format!(
                "Version: the header says {}, but the file uses something from {}.",
                crate::vgm::header::format_version(declared),
                crate::vgm::header::format_version(needed)
            ),
        }
    }
}

/// Every disagreement between `file`'s header and its stream.
///
/// Empty for the overwhelming majority of files, which is the point: a
/// non-empty result is worth telling the user about.
#[must_use]
pub fn audit(file: &VgmFile) -> Vec<HeaderFinding> {
    let mut findings = Vec::new();
    let Some(stream) = file.stream() else {
        // Without a walked stream there is nothing to compare the header to.
        return findings;
    };

    let actual = u32::try_from(stream.total_samples()).unwrap_or(u32::MAX);
    if file.header.total_samples() != actual {
        findings.push(HeaderFinding::TotalSamples {
            declared: file.header.total_samples(),
            actual,
        });
    }

    match (file.header.loop_offset(), file.loop_index()) {
        // A declared loop that resolves to no command is adrift.
        (Some(_), None) => findings.push(HeaderFinding::LoopAdrift),
        (Some(_), Some(at)) => {
            // The spec defines the field as the wait total from the loop point
            // to the end. A *shorter* value is how this editor expresses a loop
            // that stops early, so only a longer one is a disagreement.
            let declared = file.header.loop_samples().unwrap_or(0);
            let to_end = u32::try_from(stream.samples_from(at)).unwrap_or(u32::MAX);
            if declared > to_end {
                findings.push(HeaderFinding::LoopSamples {
                    declared,
                    actual: to_end,
                });
            }
        }
        (None, _) => {}
    }

    let raw = stream.raw();
    match stream.byte_offset(stream.len()) {
        Some(end) if raw.get(end) == Some(&END_OF_DATA) => {
            let trailing = raw.len() - end - 1;
            if trailing > 0 {
                findings.push(HeaderFinding::TrailingBytes { count: trailing });
            }
        }
        _ => findings.push(HeaderFinding::MissingEndMarker),
    }

    // Last, and one-directional: see `VersionUnderclaimed`. The floor-less
    // `content_version` is what makes this honest -- a genuine pre-1.50 file
    // needs less than the writer's floor and must not be flagged for it.
    let needed = crate::vgm::version::content_version(&file.header, Some(stream));
    if file.header.version() < needed {
        findings.push(HeaderFinding::VersionUnderclaimed {
            declared: file.header.version(),
            needed,
        });
    }
    findings
}

/// Applies every fix `audit` would report, and returns what it did.
///
/// Only called when the user asks. Each fix takes the *stream's* word, because
/// the stream is the music: a header can be re-derived from it, and it cannot
/// be re-derived from a header.
pub fn fix(file: &mut VgmFile) -> Vec<HeaderFinding> {
    let findings = audit(file);
    if findings.is_empty() {
        return findings;
    }

    // The two structural fixes rebuild the stream, so they go first.
    let rebuilt = findings.iter().any(|finding| {
        matches!(
            finding,
            HeaderFinding::MissingEndMarker | HeaderFinding::TrailingBytes { .. }
        )
    });
    if rebuilt && let Some(stream) = file.stream() {
        let mut bytes = stream.commands().to_vec();
        bytes.push(END_OF_DATA);
        file.replace_stream(bytes);
    }

    // Then the derived fields -- but only the ones the audit actually reported.
    // A fix triggered by one finding must not silently rewrite a field the audit
    // was content with: a loop the user deliberately shortened (a valid early-stop
    // loop, never a finding) has to survive an unrelated fix.
    let fixes_total = findings
        .iter()
        .any(|f| matches!(f, HeaderFinding::TotalSamples { .. }));
    let fixes_loop = findings
        .iter()
        .any(|f| matches!(f, HeaderFinding::LoopSamples { .. } | HeaderFinding::LoopAdrift));
    let fixes_version = findings
        .iter()
        .any(|f| matches!(f, HeaderFinding::VersionUnderclaimed { .. }));

    // All stream reads first, then the mutations -- so the stream borrow ends
    // before the header is written.
    let Some(stream) = file.stream() else {
        return findings;
    };
    let total = u32::try_from(stream.total_samples()).unwrap_or(u32::MAX);
    let loop_at = file.loop_index();
    let loop_samples = loop_at.map_or(0, |at| {
        u32::try_from(stream.samples_from(at)).unwrap_or(u32::MAX)
    });
    let absolute = loop_at.and_then(|at| Some(file.header.data_start() + stream.byte_offset(at)?));
    if fixes_total {
        file.header.set_total_samples(total);
    }
    if fixes_loop {
        // A loop that resolved to nothing is cleared rather than guessed at.
        file.header.set_loop(absolute, loop_samples);
    }
    // The version last, computed from the file as it now stands: a structural
    // fix can remove the very command that was holding the version up. Only
    // ever raised -- lowering one is a tidy-up nobody asked for here.
    if fixes_version
        && let Some(stream) = file.stream()
    {
        let needed = crate::vgm::version::content_version(&file.header, Some(stream));
        if file.header.version() < needed {
            file.header.set_version(needed);
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgm::ChipKind;
    use crate::vgm::header::offset;

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A YM2612 file whose two waits sum to 10735 samples, looping at row 1.
    fn honest() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x161);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 10_735);
        put_u32(&mut bytes, offset::LOOP_OFFSET, (0x103 - 0x1C) as u32);
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 10_735);
        bytes.extend_from_slice(&[
            0x52,
            0x28,
            0xF0, // 0
            0x61,
            0x10,
            0x27, // 1: wait 10000  <- the loop
            0x62, // 2: wait 735
            END_OF_DATA,
        ]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);
        bytes
    }

    fn read(bytes: &[u8]) -> VgmFile {
        crate::vgm::file::read("t.vgm", bytes).unwrap()
    }

    #[test]
    fn an_honest_header_has_nothing_to_report() {
        let file = read(&honest());
        assert_eq!(
            file.loop_index(),
            Some(1),
            "the fixture loops where it says"
        );
        assert!(audit(&file).is_empty());
    }

    #[test]
    fn a_wrong_sample_total_is_found_and_corrected() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 44_100);
        let mut file = read(&bytes);
        assert_eq!(
            audit(&file),
            [HeaderFinding::TotalSamples {
                declared: 44_100,
                actual: 10_735
            }]
        );

        fix(&mut file);
        assert_eq!(file.header.total_samples(), 10_735);
        assert!(audit(&file).is_empty(), "and stays fixed");
    }

    #[test]
    fn a_loop_that_lands_mid_command_is_found_and_cleared() {
        let mut bytes = honest();
        // One byte into the wait at row 1.
        put_u32(&mut bytes, offset::LOOP_OFFSET, (0x104 - 0x1C) as u32);
        let mut file = read(&bytes);
        assert_eq!(audit(&file), [HeaderFinding::LoopAdrift]);

        fix(&mut file);
        assert_eq!(file.header.loop_offset(), None, "cleared, not guessed at");
        assert_eq!(file.header.loop_samples(), None);
        assert!(audit(&file).is_empty());
    }

    #[test]
    fn a_loop_longer_than_the_stream_is_found_and_shortened() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 99_999);
        let mut file = read(&bytes);
        assert_eq!(
            audit(&file),
            [HeaderFinding::LoopSamples {
                declared: 99_999,
                actual: 10_735
            }]
        );

        fix(&mut file);
        assert_eq!(file.header.loop_samples(), Some(10_735));
    }

    /// A loop *shorter* than the tail is how this editor expresses a loop that
    /// stops early, not a mistake -- so it is not reported.
    #[test]
    fn a_loop_that_deliberately_stops_early_is_not_a_finding() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 10_000);
        assert!(audit(&read(&bytes)).is_empty());
    }

    /// And that deliberately-short loop must survive a fix triggered by an
    /// *unrelated* finding -- `fix` must not widen a loop the audit never
    /// flagged (sw-2). The unrelated finding is needed because `fix` early-returns
    /// when the audit is empty.
    #[test]
    fn a_deliberately_short_loop_survives_an_unrelated_fix() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 10_000); // short: not a finding
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 44_100); // wrong: the finding
        let mut file = read(&bytes);
        assert_eq!(
            audit(&file),
            [HeaderFinding::TotalSamples {
                declared: 44_100,
                actual: 10_735
            }],
            "only the total is a finding"
        );

        fix(&mut file);
        assert_eq!(file.header.total_samples(), 10_735, "the flagged total is fixed");
        assert_eq!(
            file.header.loop_samples(),
            Some(10_000),
            "the short loop the audit never flagged is left as it was"
        );
    }

    #[test]
    fn a_missing_end_marker_is_found_and_added() {
        let mut bytes = honest();
        bytes.pop(); // the 0x66
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read(&bytes);
        assert_eq!(audit(&file), [HeaderFinding::MissingEndMarker]);

        fix(&mut file);
        assert!(audit(&file).is_empty());
        assert_eq!(
            file.body.raw().last(),
            Some(&END_OF_DATA),
            "the marker is there now"
        );
    }

    #[test]
    fn bytes_after_the_end_marker_are_found_and_dropped() {
        let mut bytes = honest();
        bytes.extend_from_slice(&[0xDE, 0xAD]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read(&bytes);
        assert_eq!(audit(&file), [HeaderFinding::TrailingBytes { count: 2 }]);

        fix(&mut file);
        assert!(audit(&file).is_empty());
        assert_eq!(file.body.raw().last(), Some(&END_OF_DATA));
    }

    /// Several at once, and fixing leaves nothing behind.
    #[test]
    fn every_finding_at_once_is_reported_and_fixed() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 1);
        put_u32(&mut bytes, offset::LOOP_NUM_SAMPLES, 99_999);
        bytes.extend_from_slice(&[0xDE]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let mut file = read(&bytes);
        assert_eq!(audit(&file).len(), 3);
        let fixed = fix(&mut file);
        assert_eq!(fixed.len(), 3, "it reports what it did");
        assert!(audit(&file).is_empty());
        // And every description says something.
        assert!(fixed.iter().all(|finding| !finding.describe().is_empty()));
    }

    /// The load-bearing promise: an audit reads, and only `fix` writes.
    #[test]
    fn auditing_never_changes_the_file() {
        let mut bytes = honest();
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 44_100);
        let file = read(&bytes);
        let before = crate::vgm::file::write(&file).unwrap();
        assert!(!audit(&file).is_empty());
        assert_eq!(crate::vgm::file::write(&file).unwrap(), before);
    }

    /// A file using a 1.60 command while calling itself 1.51 -- twenty real
    /// corpus files do exactly this -- is reported and raised.
    #[test]
    fn a_version_the_file_outgrew_is_found_and_raised() {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x151);
        put_u32(
            &mut bytes,
            offset::DATA_OFFSET,
            (0x100 - offset::DATA_OFFSET) as u32,
        );
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        // `0x90`: a DAC stream setup, which arrived in 1.60.
        bytes.extend_from_slice(&[0x90, 0x00, 0x02, 0x00, 0x2A, 0x66]);
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - 4) as u32);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 0);

        let mut file = read(&bytes);
        assert_eq!(
            audit(&file),
            [HeaderFinding::VersionUnderclaimed {
                declared: 0x151,
                needed: 0x160,
            }]
        );

        fix(&mut file);
        assert_eq!(file.header.version(), 0x160);
        assert!(audit(&file).is_empty(), "and it stays fixed");
    }

    /// A genuine pre-1.50 file that declares its true version is not a finding:
    /// the writer's floor is not the file's need, and reporting it would open the
    /// box for a perfectly valid old file (sw-3).
    #[test]
    fn a_genuine_pre_1_50_file_is_not_reported_as_underclaimed() {
        // A v1.10 header, legacy layout (data at 0x40), clocking a 1.10 chip and
        // using nothing past it.
        let mut bytes = vec![0u8; 0x40];
        bytes[..4].copy_from_slice(crate::vgm::io::MAGIC);
        put_u32(&mut bytes, offset::VERSION, 0x110);
        put_u32(&mut bytes, ChipKind::Ym2612.clock_offset(), 7_670_454);
        put_u32(&mut bytes, offset::TOTAL_SAMPLES, 0);
        bytes.push(END_OF_DATA); // the whole stream: just the end marker
        let eof = bytes.len();
        put_u32(&mut bytes, offset::EOF, (eof - offset::EOF) as u32);

        let file = read(&bytes);
        assert_eq!(file.header.version(), 0x110);
        assert!(
            audit(&file).is_empty(),
            "a 1.10 file needing only 1.10 must not be flagged for the writer's 1.50 floor"
        );
    }

    /// Declaring more than is needed is not a finding: almost every file does,
    /// and a dialog that opens for almost every file is a dialog nobody reads.
    #[test]
    fn a_version_higher_than_needed_is_left_alone() {
        let mut file = read(&honest());
        assert!(audit(&file).is_empty());
        assert_eq!(file.header.version(), 0x161, "it over-claims...");

        fix(&mut file);
        assert_eq!(file.header.version(), 0x161, "...and keeps doing so");
    }
}
