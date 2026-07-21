//! Splitting one long capture into its constituent songs at the silent gaps.
//!
//! A sound-test session logged in one go is many songs end to end, each parted
//! from the next by a stretch of silence. [`detect_segments`] finds those parts
//! by summing the samples of consecutive delay instructions and calling a run
//! that reaches a threshold a gap; [`materialise`] then lifts one part out into
//! a standalone song, prepending the register state the stream had reached by
//! that point so the piece starts on the same chip state it would have mid-play.
//!
//! This mirrors VGMRips' `vgm_sptd`, adapted to operate on this app's decoded
//! instruction stream rather than raw bytes. Detection is a single cheap pass,
//! so a UI can re-run it on every threshold change; materialisation touches only
//! the chosen part.

use crate::song::Song;

/// One detected song within a capture: the half-open instruction range
/// `[start, end)` that holds it, and where it sits on the sample clock.
///
/// The sample fields are derived once by [`detect_segments`] so a dialog can
/// show each piece's position and length without re-walking the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// The first instruction of the piece: a register write, since leading
    /// silence is trimmed.
    pub start: usize,
    /// One past the last instruction of the piece, also a register write --
    /// the trailing gap is left out, so the piece ends on its last real command.
    pub end: usize,
    /// Samples elapsed before `start`: where the piece begins on the capture's
    /// clock.
    pub start_samples: u32,
    /// The piece's own duration in samples.
    pub length_samples: u32,
}

impl Segment {
    /// The number of instructions in the piece.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the piece holds no instructions. [`detect_segments`] never yields
    /// one, but the accessor keeps clippy happy alongside [`Self::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// Finds the individual songs in `song`, split at every run of consecutive delay
/// instructions whose summed length reaches `threshold_samples`.
///
/// One cheap pass: it accumulates the samples of consecutive delays and, when a
/// run reaches the threshold, treats it as a gap between two songs. A piece runs
/// from its first register write to its last -- the gap's delays belong to
/// neither the piece before it (which ends at its last real command) nor the one
/// after it (which starts at the first real command past the gap), so leading
/// and trailing silence is trimmed by construction. Sub-threshold delays are
/// part of the music and stay inside the piece.
///
/// Returns one [`Segment`] per song, in order. An empty stream, or one with no
/// register writes at all, yields none. A `threshold_samples` of zero is treated
/// as one sample, so a piece is never split where no time actually passes.
#[must_use]
pub fn detect_segments(song: &Song, threshold_samples: u32) -> Vec<Segment> {
    let threshold = u64::from(threshold_samples).max(1);
    let prefix = song.delay_samples_prefix();
    let mut segments = Vec::new();

    // The current piece: its first register write, its last so far, and the
    // samples of the delays seen since that last real command.
    let mut seg_start: Option<usize> = None;
    let mut last_real: Option<usize> = None;
    let mut gap: u64 = 0;

    for index in 0..song.len() {
        match song.instruction(index) {
            Some(instruction) if instruction.is_delay() => {
                gap += u64::from(instruction.delay_samples());
            }
            Some(_) => {
                // A register write (a VGM stream has nothing else). A gap that
                // reached the threshold before it ends the current piece; this
                // command opens the next.
                if gap >= threshold && seg_start.is_some() {
                    push_segment(&mut segments, &prefix, seg_start, last_real);
                    // Reopen from this command; `get_or_insert` below sets `start`.
                    seg_start = None;
                }
                seg_start.get_or_insert(index);
                last_real = Some(index);
                gap = 0;
            }
            None => {}
        }
    }
    push_segment(&mut segments, &prefix, seg_start, last_real);
    segments
}

/// Appends the piece `[start, last + 1)` if one is open. A piece always holds at
/// least one register write, so `start <= last` whenever both are set and the
/// emitted segment is never empty.
fn push_segment(
    segments: &mut Vec<Segment>,
    prefix: &[u32],
    seg_start: Option<usize>,
    last_real: Option<usize>,
) {
    let (Some(start), Some(last)) = (seg_start, last_real) else {
        return;
    };
    let end = last + 1;
    segments.push(Segment {
        start,
        end,
        start_samples: prefix[start],
        length_samples: prefix[end] - prefix[start],
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::OplType;
    use crate::vgm::data::command;
    use crate::vgm::io::synthesise_header;
    use crate::vgm::{VgmData, VgmMeta};

    /// A one-sample-per-unit VGM builder: `write(reg, value)` and `wait(samples)`
    /// helpers concatenated into a stream, wrapped as an OPL2 song.
    fn write(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812, reg, value]
    }

    /// A long wait of `samples` (0..=65535), as one `0x61` command.
    fn wait(samples: u16) -> Vec<u8> {
        let mut bytes = vec![command::WAIT];
        bytes.extend_from_slice(&samples.to_le_bytes());
        bytes
    }

    fn song_of(chunks: &[Vec<u8>]) -> Song {
        let bytes: Vec<u8> = chunks.concat();
        Song::vgm(
            "capture.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            OplType::Opl2,
            VgmMeta::new(synthesise_header()),
        )
    }

    #[test]
    fn a_single_song_with_no_gaps_is_one_segment() {
        let song = song_of(&[write(0x20, 0x01), wait(1000), write(0x21, 0x02), wait(1000)]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].start, 0);
        // Ends at the last register write (index 2), excluding the trailing wait.
        assert_eq!(segments[0].end, 3);
        assert_eq!(segments[0].start_samples, 0);
        assert_eq!(segments[0].length_samples, 1000);
    }

    #[test]
    fn a_gap_over_the_threshold_splits_two_songs() {
        // song A: write, wait 500, write; gap of 10000; song B: write, wait 500, write.
        let song = song_of(&[
            write(0x20, 0x01),
            wait(500),
            write(0x21, 0x02),
            wait(10_000), // the gap
            write(0x22, 0x03),
            wait(500),
            write(0x23, 0x04),
        ]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 2);
        // Piece A: instructions [0, 3) -- ends before the gap wait at index 3.
        assert_eq!((segments[0].start, segments[0].end), (0, 3));
        assert_eq!(segments[0].start_samples, 0);
        assert_eq!(segments[0].length_samples, 500);
        // Piece B: starts at index 4 (the first write after the gap), ends at 7.
        assert_eq!((segments[1].start, segments[1].end), (4, 7));
        // It begins after A's 500 samples plus the 10000 gap.
        assert_eq!(segments[1].start_samples, 10_500);
        assert_eq!(segments[1].length_samples, 500);
    }

    #[test]
    fn a_gap_may_span_several_delay_instructions() {
        // Four 3000-sample waits in a row sum to 12000 -- over an 8000 threshold --
        // even though no single one reaches it.
        let song = song_of(&[
            write(0x20, 0x01),
            wait(3000),
            wait(3000),
            wait(3000),
            wait(3000),
            write(0x21, 0x02),
        ]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 2);
        assert_eq!((segments[0].start, segments[0].end), (0, 1));
        assert_eq!((segments[1].start, segments[1].end), (5, 6));
    }

    #[test]
    fn a_gap_interrupted_by_a_write_is_not_a_boundary() {
        // Two 5000 waits that would sum to 10000, but a register write between
        // them resets the accumulator, so neither run reaches 8000.
        let song = song_of(&[
            write(0x20, 0x01),
            wait(5000),
            write(0x21, 0x02), // resets the gap accumulator
            wait(5000),
            write(0x22, 0x03),
        ]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 1);
        assert_eq!((segments[0].start, segments[0].end), (0, 5));
    }

    #[test]
    fn the_threshold_is_inclusive() {
        // A gap of exactly the threshold splits; one sample under does not.
        let stream = |gap: u16| song_of(&[write(0x20, 0x01), wait(gap), write(0x21, 0x02)]);
        assert_eq!(detect_segments(&stream(8000), 8000).len(), 2, "== splits");
        assert_eq!(detect_segments(&stream(7999), 8000).len(), 1, "< does not");
    }

    #[test]
    fn leading_and_trailing_silence_is_trimmed() {
        // Silence, song, silence: the piece runs from the first write to the last,
        // and neither the leading nor the trailing wait is part of it.
        let song = song_of(&[
            wait(20_000), // leading silence
            write(0x20, 0x01),
            wait(500),
            write(0x21, 0x02),
            wait(20_000), // trailing silence
        ]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 1);
        assert_eq!((segments[0].start, segments[0].end), (1, 4));
        // start_samples counts the leading silence that came before it.
        assert_eq!(segments[0].start_samples, 20_000);
        assert_eq!(segments[0].length_samples, 500);
    }

    #[test]
    fn an_empty_or_writeless_stream_yields_no_segments() {
        assert!(detect_segments(&song_of(&[]), 8000).is_empty());
        assert!(detect_segments(&song_of(&[wait(9000), wait(9000)]), 8000).is_empty());
    }

    #[test]
    fn a_zero_threshold_still_needs_an_actual_gap() {
        // Two adjacent writes with no delay between them are one piece even at a
        // zero threshold; only a real (>= 1 sample) gap can split.
        let song = song_of(&[
            write(0x20, 0x01),
            write(0x21, 0x02),
            wait(1),
            write(0x22, 0x03),
        ]);
        let segments = detect_segments(&song, 0);
        assert_eq!(
            segments.len(),
            2,
            "the 1-sample gap splits, the adjacency does not"
        );
        assert_eq!((segments[0].start, segments[0].end), (0, 2));
        assert_eq!((segments[1].start, segments[1].end), (3, 4));
    }

    #[test]
    fn the_segment_length_sums_the_internal_delays() {
        // A piece with several sub-threshold delays: its length is their sum.
        let song = song_of(&[
            write(0x20, 0x01),
            wait(100),
            write(0x21, 0x02),
            wait(200),
            write(0x22, 0x03),
            wait(300),
            write(0x23, 0x04),
        ]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].length_samples, 600);
        assert_eq!(segments[0].len(), 7);
        assert!(!segments[0].is_empty());
    }
}
