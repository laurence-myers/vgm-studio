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

use crate::opl_state::OplState;
use crate::song::{Bank, DroInstruction, OplType, Song};
use crate::vgm::data::command;
use crate::vgm::io::{CONVERSION_VERSION, synthesise_header};
use crate::vgm::{VgmData, VgmMeta};

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

/// Lifts `segment` out of `song` into a standalone VGM song.
///
/// The piece's own instructions are copied verbatim. When `state_replay` is set
/// -- the mode the UI always uses -- the OPL register state the stream had
/// reached by the segment's start is captured and prepended as the minimal set
/// of writes that recreates it, so a piece taken from the middle of the capture
/// begins on the same chip state it would have had mid-play rather than on
/// silence. A segment starting at instruction 0 gets no prelude: there is no
/// prior state to restore.
///
/// The result is a fresh v1.51 VGM -- a synthesised header (the writer fills in
/// the chip clocks from `opl_type`), the source's `opl_type`, a clone of its GD3
/// tag if it has one, and no loop. The piece is named after the source's file
/// stem with a `.vgm` extension, so [`write_song`](crate::io::write_song) emits
/// it uncompressed; the caller names the file it writes separately.
#[must_use]
pub fn materialise(song: &Song, segment: &Segment, state_replay: bool) -> Song {
    let opl_type = song.opl_type;
    let mut bytes = Vec::new();

    if state_replay {
        let mut state = OplState::new();
        for index in 0..segment.start {
            if let Some(DroInstruction::Register { reg, value, bank }) = song.instruction(index) {
                state.record(bank, reg, value);
            }
        }
        for (bank, reg, value) in state.replay_writes() {
            bytes.push(write_opcode(opl_type, bank));
            bytes.push(reg);
            bytes.push(value);
        }
    }

    // The piece's own commands, byte for byte from the source stream.
    for index in segment.start..segment.end {
        if let Some(raw) = song.data().raw_instruction(index) {
            bytes.extend_from_slice(raw);
        }
    }

    let mut meta = VgmMeta::new(synthesise_header());
    meta.tag = song.vgm_meta().and_then(|meta| meta.tag.clone());
    Song::vgm(
        piece_name(&song.name),
        CONVERSION_VERSION,
        VgmData::new(bytes).expect("materialise only emits commands the indexer knows"),
        opl_type,
        meta,
    )
}

/// The VGM write opcode that addresses `bank` on `opl_type`.
///
/// A single OPL2 has no high bank, so `(Opl2, High)` never arises from a real
/// capture's captured state; it maps to the second-chip opcode for totality.
fn write_opcode(opl_type: OplType, bank: Bank) -> u8 {
    match (opl_type, bank) {
        (OplType::Opl2 | OplType::DualOpl2, Bank::Low) => command::YM3812,
        (OplType::Opl2 | OplType::DualOpl2, Bank::High) => command::YM3812_CHIP_2,
        (OplType::Opl3, Bank::Low) => command::YMF262_PORT_0,
        (OplType::Opl3, Bank::High) => command::YMF262_PORT_1,
    }
}

/// The source's file stem with a `.vgm` extension. Any existing extension
/// (`.vgm`, `.vgz`, …) is replaced, so a piece cut from a `.vgz` is still
/// written uncompressed.
fn piece_name(source_name: &str) -> String {
    let stem = source_name
        .rsplit_once('.')
        .map_or(source_name, |(stem, _extension)| stem);
    format!("{stem}.vgm")
}

/// The register state a naive replay of `song`'s writes over `[0, upto)` reaches,
/// as replay triples -- the reference a materialised prelude must reproduce.
#[cfg(test)]
fn state_over(song: &Song, upto: usize) -> Vec<(Bank, u8, u8)> {
    let mut state = OplState::new();
    for index in 0..upto {
        if let Some(DroInstruction::Register { reg, value, bank }) = song.instruction(index) {
            state.record(bank, reg, value);
        }
    }
    state.replay_writes()
}

/// The state reached after folding the first `n` register writes of `song`.
#[cfg(test)]
fn state_after_writes(song: &Song, n: usize) -> Vec<(Bank, u8, u8)> {
    let mut state = OplState::new();
    let mut seen = 0;
    for index in 0..song.len() {
        if seen == n {
            break;
        }
        if let Some(DroInstruction::Register { reg, value, bank }) = song.instruction(index) {
            state.record(bank, reg, value);
            seen += 1;
        }
    }
    state.replay_writes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::vgm::Gd3Tag;

    /// A low-bank OPL2 write, `0x5A reg value`.
    fn write(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812, reg, value]
    }

    /// A high-bank write, `0xAA reg value` (dual-OPL2 second chip / decoded as the
    /// high bank).
    fn write_hi(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812_CHIP_2, reg, value]
    }

    /// A long wait of `samples` (0..=65535), as one `0x61` command.
    fn wait(samples: u16) -> Vec<u8> {
        let mut bytes = vec![command::WAIT];
        bytes.extend_from_slice(&samples.to_le_bytes());
        bytes
    }

    fn song_of(chunks: &[Vec<u8>]) -> Song {
        song_of_type(chunks, OplType::Opl2)
    }

    fn song_of_type(chunks: &[Vec<u8>], opl_type: OplType) -> Song {
        let bytes: Vec<u8> = chunks.concat();
        Song::vgm(
            "capture.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            opl_type,
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

    // -- materialise -------------------------------------------------------

    /// A four-song capture used by the materialise tests. Each song is two
    /// writes with a 500-sample delay between them, setting distinct registers so
    /// a later piece's prelude must restore the earlier ones; the songs are
    /// parted by 10000-sample gaps (over the 8000 threshold).
    fn capture() -> Song {
        song_of(&[
            // song 0
            write(0x20, 0x11),
            wait(500),
            write(0x40, 0x22),
            wait(10_000),
            // song 1
            write(0x60, 0x33),
            wait(500),
            write(0x80, 0x44),
            wait(10_000),
            // song 2
            write(0xA0, 0x55),
            wait(500),
            write(0xB0, 0x66),
            wait(10_000),
            // song 3
            write(0x21, 0x77),
            wait(500),
            write(0x41, 0x88),
        ])
    }

    #[test]
    fn a_piece_round_trips_through_read_song() {
        let song = capture();
        let segments = detect_segments(&song, 8000);
        for segment in &segments {
            let piece = materialise(&song, segment, true);
            let bytes = write_song(&piece).expect("a materialised VGM writes");
            let reread = read_song("piece.vgm", &bytes).expect("and reads back");
            assert_eq!(
                reread.data(),
                piece.data(),
                "stream survived the round trip"
            );
            assert_eq!(reread.opl_type, song.opl_type);
        }
    }

    #[test]
    fn the_register_state_at_a_piece_start_matches_the_original() {
        let song = capture();
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 4);

        for segment in &segments {
            let expected = state_over(&song, segment.start);
            let piece = materialise(&song, segment, true);
            // Re-read so the assertion also proves the prelude's opcodes decode
            // back to the banks they came from.
            let bytes = write_song(&piece).unwrap();
            let reread = read_song("piece.vgm", &bytes).unwrap();
            assert_eq!(
                state_after_writes(&reread, expected.len()),
                expected,
                "piece starting at {} does not restore the original state",
                segment.start
            );
        }
        // The third piece must have restored all four earlier registers.
        assert_eq!(state_over(&song, segments[3].start).len(), 6);
    }

    #[test]
    fn a_high_bank_prelude_uses_the_right_opcode() {
        // A dual-OPL2 capture whose first song writes the high bank; the second
        // song's prelude must restore it as a high-bank write, not a low one.
        let song = song_of_type(
            &[
                write(0x20, 0x01),
                write_hi(0x20, 0x09), // high bank
                wait(10_000),
                write(0x40, 0x02),
                wait(500),
            ],
            OplType::DualOpl2,
        );
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 2);

        let piece = materialise(&song, &segments[1], true);
        let reread = read_song("piece.vgm", &write_song(&piece).unwrap()).unwrap();
        // The high-bank value survives as a high-bank write.
        assert_eq!(
            reread.instruction(1),
            Some(DroInstruction::Register {
                reg: 0x20,
                value: 0x09,
                bank: Some(Bank::High),
            })
        );
        assert_eq!(
            state_after_writes(&reread, 2),
            state_over(&song, segments[1].start)
        );
    }

    #[test]
    fn a_piece_from_offset_zero_has_no_prelude() {
        let song = capture();
        let segments = detect_segments(&song, 8000);
        let first = &segments[0];
        assert_eq!(first.start, 0);

        let piece = materialise(&song, first, true);
        // No prelude: the piece is exactly the segment's own instructions.
        assert_eq!(piece.len(), first.len());
        assert_eq!(
            piece.instruction(0),
            Some(DroInstruction::Register {
                reg: 0x20,
                value: 0x11,
                bank: Some(Bank::Low),
            })
        );
    }

    #[test]
    fn state_replay_off_omits_the_prelude() {
        let song = capture();
        let segments = detect_segments(&song, 8000);
        let second = &segments[1];

        let with = materialise(&song, second, true);
        let without = materialise(&song, second, false);
        assert!(with.len() > without.len(), "replay adds a prelude");
        // Without a prelude the piece is just its own instructions.
        assert_eq!(without.len(), second.len());
    }

    #[test]
    fn the_gd3_tag_is_copied_into_each_piece() {
        let mut song = capture();
        let tag = Gd3Tag {
            game_name_en: "Sound Test".to_owned(),
            track_author_en: "Composer".to_owned(),
            ..Gd3Tag::default()
        };
        song.vgm_meta_mut().unwrap().tag = Some(tag.clone());

        for segment in &detect_segments(&song, 8000) {
            let piece = materialise(&song, segment, true);
            assert_eq!(piece.vgm_meta().unwrap().tag.as_ref(), Some(&tag));
            // And it survives a write/read cycle.
            let reread = read_song("piece.vgm", &write_song(&piece).unwrap()).unwrap();
            assert_eq!(reread.vgm_meta().unwrap().tag.as_ref(), Some(&tag));
        }
    }

    #[test]
    fn a_source_without_a_tag_yields_untagged_pieces() {
        let song = capture();
        assert!(song.vgm_meta().unwrap().tag.is_none());
        let piece = materialise(&song, &detect_segments(&song, 8000)[0], true);
        assert!(piece.vgm_meta().unwrap().tag.is_none());
    }

    #[test]
    fn the_piece_durations_sum_to_the_original_minus_the_gaps() {
        let song = capture();
        let segments = detect_segments(&song, 8000);

        // Each piece's duration is its segment length (the prelude adds no time).
        let mut total = 0u32;
        for segment in &segments {
            let piece = materialise(&song, segment, true);
            assert_eq!(piece.total_delay_samples(), segment.length_samples);
            total += piece.total_delay_samples();
        }
        // Four songs of 500 samples each; the three 10000 gaps are dropped.
        assert_eq!(total, 4 * 500);
        assert_eq!(song.total_delay_samples(), 4 * 500 + 3 * 10_000);
    }

    #[test]
    fn a_piece_that_ends_on_a_write_keeps_no_trailing_delay() {
        // A song whose last real command is a write, with a trailing wait before
        // the next gap, yields a piece ending exactly on that write.
        let song = song_of(&[
            write(0x20, 0x01),
            wait(300),
            write(0x21, 0x02),
            wait(400), // trailing decay, trimmed
            wait(10_000),
            write(0x22, 0x03),
        ]);
        let segments = detect_segments(&song, 8000);
        let piece = materialise(&song, &segments[0], true);
        // 300 internal only; the 400 trailing wait is not part of the piece.
        assert_eq!(piece.total_delay_samples(), 300);
    }

    #[test]
    fn a_vgz_source_still_produces_an_uncompressed_piece() {
        // The piece name drives write_song's compression choice; a `.vgz` source
        // must not yield gzipped pieces named `.vgm`.
        assert_eq!(piece_name("capture.vgz"), "capture.vgm");
        assert_eq!(piece_name("my.sound.test.vgm"), "my.sound.test.vgm");
        assert_eq!(piece_name("noext"), "noext.vgm");

        let mut song = capture();
        song.name = "capture.vgz".to_owned();
        let piece = materialise(&song, &detect_segments(&song, 8000)[0], true);
        assert!(piece.name.ends_with(".vgm"));
        let bytes = write_song(&piece).unwrap();
        assert!(
            !crate::vgm::io::is_gzipped(&bytes),
            "the piece should be plain VGM"
        );
    }

    // -- corpus sanity -----------------------------------------------------

    /// Corpus sanity on a real capture: three copies of the `dro2vgm` OPL2 rip,
    /// parted by one-second gaps, must split back into three pieces, each
    /// beginning on exactly the register state the stream had reached there. This
    /// is the programmatic stand-in for "listen to piece 2+ for state-replay
    /// correctness" -- a real few-hundred-command stream of ordinary music, not a
    /// synthetic one.
    #[test]
    fn a_real_capture_tripled_with_gaps_splits_and_replays() {
        const CAPTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");
        let base = read_song("lsl3_score_up.vgm", CAPTURE).unwrap();
        // At the 0.75 s default threshold the jingle itself has no internal gap.
        assert_eq!(
            detect_segments(&base, 33_075).len(),
            1,
            "one song on its own"
        );

        // Concatenate the command stream three times, parted by 44100-sample gaps.
        let body = base.data().raw();
        let gap = [command::WAIT, 0x44, 0xAC]; // 44100 = 0xAC44
        let mut bytes = Vec::new();
        for copy in 0..3 {
            if copy > 0 {
                bytes.extend_from_slice(&gap);
            }
            bytes.extend_from_slice(body);
        }
        let capture = Song::vgm(
            "triple.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            base.opl_type,
            VgmMeta::new(synthesise_header()),
        );

        let segments = detect_segments(&capture, 33_075);
        assert_eq!(segments.len(), 3, "three songs after concatenation");

        for (index, segment) in segments.iter().enumerate() {
            let expected = state_over(&capture, segment.start);
            let piece = materialise(&capture, segment, true);
            let reread = read_song("piece.vgm", &write_song(&piece).unwrap()).unwrap();
            assert_eq!(
                state_after_writes(&reread, expected.len()),
                expected,
                "piece {index} does not open on the capture's register state"
            );
            // Every piece carries the whole jingle: the same command count as the
            // original stream, plus its state-replay prelude.
            assert!(piece.len() >= base.len(), "piece {index} lost commands");
        }
        // Pieces 2 and 3 have a real prelude to restore (piece 1 does not).
        assert!(state_over(&capture, segments[0].start).is_empty());
        assert!(!state_over(&capture, segments[1].start).is_empty());
        assert!(!state_over(&capture, segments[2].start).is_empty());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::io::{read_song, write_song};
    use proptest::prelude::*;

    /// A random VGM command: a low- or high-bank register write, or a wait.
    fn command_bytes() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            (any::<u8>(), any::<u8>()).prop_map(|(reg, value)| vec![command::YM3812, reg, value]),
            (any::<u8>(), any::<u8>()).prop_map(|(reg, value)| vec![
                command::YM3812_CHIP_2,
                reg,
                value
            ]),
            any::<u16>().prop_map(|samples| {
                let mut command = vec![command::WAIT];
                command.extend_from_slice(&samples.to_le_bytes());
                command
            }),
        ]
    }

    fn random_stream() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(command_bytes(), 1..60).prop_map(|commands| commands.concat())
    }

    proptest! {
        /// However a capture is split, a state-replay piece begins on exactly the
        /// register state a naive fold of the original's earlier writes reaches --
        /// and that holds through a write/read round trip, so the prelude's
        /// opcodes decode back to the banks they were captured from.
        #[test]
        fn a_replayed_piece_restores_the_folded_state(
            bytes in random_stream(),
            pick in 0usize..60,
        ) {
            let song = Song::vgm(
                "capture.vgm".to_owned(),
                0x151,
                VgmData::new(bytes).unwrap(),
                OplType::DualOpl2,
                VgmMeta::new(synthesise_header()),
            );
            let segments = detect_segments(&song, 4000);
            prop_assume!(!segments.is_empty());
            let segment = segments[pick % segments.len()];

            let expected = state_over(&song, segment.start);
            let piece = materialise(&song, &segment, true);
            let reread = read_song("piece.vgm", &write_song(&piece).unwrap()).unwrap();
            prop_assert_eq!(state_after_writes(&reread, expected.len()), expected);
        }

        /// A piece's duration is exactly its segment's length, whatever the split.
        #[test]
        fn a_piece_duration_equals_its_segment_length(
            bytes in random_stream(),
            threshold in 1u32..20_000,
        ) {
            let song = Song::vgm(
                "capture.vgm".to_owned(),
                0x151,
                VgmData::new(bytes).unwrap(),
                OplType::Opl2,
                VgmMeta::new(synthesise_header()),
            );
            for segment in detect_segments(&song, threshold) {
                let piece = materialise(&song, &segment, true);
                prop_assert_eq!(piece.total_delay_samples(), segment.length_samples);
            }
        }
    }
}
