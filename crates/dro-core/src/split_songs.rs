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

use crate::song::dro_data::v1_opcode;
use crate::song::{Bank, DroDataV1, DroDataV2, DroInstruction, Song, SongData};
use crate::util::VGM_SAMPLE_RATE;
use crate::vgm::data::command;
use crate::vgm::io::{CONVERSION_VERSION, synthesise_header};
use crate::vgm::{VgmData, VgmMeta};

/// The size of one OPL register file (low or high), for the state-replay fold.
const REGISTER_COUNT: usize = 256;

/// One detected song within a capture: the half-open instruction range
/// `[start, end)` that holds it, and where it sits in time.
///
/// Times are in the capture's *native* delay unit -- samples for a VGM,
/// milliseconds for a DRO -- so the whole splitter works the same on both. Use
/// [`native_rate`] to convert to and from seconds. The fields are derived once by
/// [`detect_segments`] so a dialog can show each piece's position, length and
/// available decay tail without re-walking the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// The first instruction of the piece: a register write, since leading
    /// silence is trimmed.
    pub start: usize,
    /// One past the last instruction of the piece, also a register write --
    /// the trailing gap is left out, so the piece ends on its last real command.
    pub end: usize,
    /// Native-unit time elapsed before `start`: where the piece begins on the
    /// capture's clock.
    pub start_time: u32,
    /// The piece's own duration, in native units.
    pub duration: u32,
    /// The trimmed silence after `end`, up to the next song (or the end of the
    /// capture), in native units. This bounds how much decay tail a piece can
    /// keep -- [`materialise`] never appends more tail than actually followed.
    pub trailing_gap: u32,
}

/// The delay units per second in a song's native unit: `44100` for a VGM
/// (samples), `1000` for a DRO (milliseconds). Lets a UI convert the
/// native-unit [`Segment`] fields and its threshold to and from seconds.
#[must_use]
pub fn native_rate(song: &Song) -> u32 {
    if song.data().delays_in_samples() {
        VGM_SAMPLE_RATE
    } else {
        1000
    }
}

/// One instruction's delay in the song's native unit.
fn native_delay(instruction: DroInstruction, in_samples: bool) -> u32 {
    if in_samples {
        instruction.delay_samples()
    } else {
        instruction.delay_ms()
    }
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
/// instructions whose summed length reaches `threshold` (in the song's native
/// unit -- samples for a VGM, milliseconds for a DRO; see [`native_rate`]).
///
/// One cheap pass: it accumulates the native delay of consecutive delays and,
/// when a run reaches the threshold, treats it as a gap between two songs. A
/// piece runs from its first register write to its last -- the gap's delays
/// belong to neither the piece before it (which ends at its last real command)
/// nor the one after it (which starts at the first real command past the gap),
/// so leading and trailing silence is trimmed by construction. Sub-threshold
/// delays are part of the music and stay inside the piece.
///
/// Returns one [`Segment`] per song, in order. An empty stream, or one with no
/// register writes at all, yields none. A `threshold` of zero is treated as one
/// unit, so a piece is never split where no time actually passes.
#[must_use]
pub fn detect_segments(song: &Song, threshold: u32) -> Vec<Segment> {
    let threshold = u64::from(threshold).max(1);
    let in_samples = song.data().delays_in_samples();

    // A native-unit exclusive prefix sum, so a segment's start time and duration
    // are lookups rather than re-walks.
    let mut prefix = Vec::with_capacity(song.len() + 1);
    let mut acc = 0u32;
    prefix.push(acc);
    for index in 0..song.len() {
        let delay = native_delay(song.instruction(index).expect("index < len"), in_samples);
        acc = acc.saturating_add(delay);
        prefix.push(acc);
    }

    let mut segments = Vec::new();
    // The current piece: its first register write, its last so far, and the
    // native delay of the run seen since that last real command.
    let mut seg_start: Option<usize> = None;
    let mut last_real: Option<usize> = None;
    let mut gap: u64 = 0;

    for index in 0..song.len() {
        let instruction = song.instruction(index).expect("index < len");
        if instruction.is_delay() {
            gap += u64::from(native_delay(instruction, in_samples));
        } else {
            // A register write or a bank switch. A gap that reached the threshold
            // before it ends the current piece; this command opens the next.
            if gap >= threshold && seg_start.is_some() {
                push_segment(&mut segments, &prefix, seg_start, last_real, gap);
                // Reopen from this command; `get_or_insert` below sets `start`.
                seg_start = None;
            }
            seg_start.get_or_insert(index);
            last_real = Some(index);
            gap = 0;
        }
    }
    // The final piece's trailing delays run to the end of the capture.
    push_segment(&mut segments, &prefix, seg_start, last_real, gap);
    segments
}

/// Appends the piece `[start, last + 1)` if one is open, recording the `gap` of
/// trimmed silence that follows it. A piece always holds at least one register
/// write, so `start <= last` whenever both are set and the segment is never empty.
fn push_segment(
    segments: &mut Vec<Segment>,
    prefix: &[u32],
    seg_start: Option<usize>,
    last_real: Option<usize>,
    gap: u64,
) {
    let (Some(start), Some(last)) = (seg_start, last_real) else {
        return;
    };
    let end = last + 1;
    segments.push(Segment {
        start,
        end,
        start_time: prefix[start],
        duration: prefix[end] - prefix[start],
        trailing_gap: u32::try_from(gap).unwrap_or(u32::MAX),
    });
}

/// Lifts `segment` out of `song` into a standalone song of the same format (VGM,
/// DRO v1 or DRO v2).
///
/// The piece's own instructions are copied verbatim. When `state_replay` is set
/// -- the mode the UI always uses -- the OPL register state the stream had
/// reached by the segment's start is captured and prepended as the writes that
/// recreate it (each register's last write, reused byte for byte from the
/// source, so the encoding is exact whatever the format), so a piece taken from
/// the middle of the capture begins on the chip state it would have had mid-play
/// rather than on silence. A segment starting at instruction 0 gets no prelude.
///
/// `trailing_tail` (in the song's native unit) asks for up to that much of the
/// trimmed trailing silence back, as a decay tail; it is capped at the gap that
/// actually followed the piece ([`Segment::trailing_gap`]), so a piece never
/// gains time the capture did not have.
///
/// The result carries the source's `opl_type`; a VGM piece gets a fresh v1.51
/// header and the source's GD3 with the track title blanked (it named the whole
/// capture); a DRO piece reuses the source's codemap. It is named after the
/// source's file stem with the format's extension, so
/// [`write_song`](crate::io::write_song) writes it uncompressed; the caller names
/// the file separately.
#[must_use]
pub fn materialise(song: &Song, segment: &Segment, state_replay: bool, trailing_tail: u32) -> Song {
    let mut bytes = Vec::new();

    if state_replay {
        append_state_prelude(&mut bytes, song, segment.start);
    }

    // The piece's own commands, byte for byte from the source stream.
    for index in segment.start..segment.end {
        bytes.extend_from_slice(song.data().raw_instruction(index).expect("index < end"));
    }

    // An optional decay tail: a synthetic delay, never longer than the gap that
    // actually followed the piece.
    let tail = trailing_tail.min(segment.trailing_gap);
    if tail > 0 {
        append_delay(&mut bytes, song, tail);
    }

    build_piece(song, bytes, segment.duration.saturating_add(tail))
}

/// Captures the OPL state reached over `[0, start)` and appends the writes that
/// recreate it: each touched register's last write, reused verbatim from the
/// source, low file before high. For DRO v1 -- whose register writes carry no
/// bank -- the current bank is tracked from the bank-switch opcodes, and the
/// prelude emits its own switches so each write and the body land in the right
/// bank. VGM and DRO v2 carry the bank in every write, so no switches are needed.
fn append_state_prelude(bytes: &mut Vec<u8>, song: &Song, start: usize) {
    let is_v1 = matches!(song.data(), SongData::V1(_));
    let mut current_bank = Bank::Low;
    // The source index of the last write to each (file, register).
    let mut last_write = [[None::<usize>; REGISTER_COUNT]; 2];
    for index in 0..start {
        match song.instruction(index) {
            Some(DroInstruction::Register { reg, bank, .. }) => {
                let bank = bank.unwrap_or(current_bank);
                last_write[usize::from(bank.index())][usize::from(reg)] = Some(index);
            }
            Some(DroInstruction::BankSwitch(bank)) => current_bank = bank,
            _ => {}
        }
    }

    let mut emit_bank = Bank::Low; // a DRO stream starts in the low bank
    for file in [Bank::Low, Bank::High] {
        let writes: Vec<usize> = (0..REGISTER_COUNT)
            .filter_map(|reg| last_write[usize::from(file.index())][reg])
            .collect();
        if writes.is_empty() {
            continue;
        }
        if is_v1 && emit_bank != file {
            bytes.extend_from_slice(bank_switch_bytes(file));
            emit_bank = file;
        }
        for index in writes {
            bytes.extend_from_slice(song.data().raw_instruction(index).expect("index < start"));
        }
    }
    // Leave a v1 chip in the bank the body's first (bank-less) write expects.
    if is_v1 && emit_bank != current_bank {
        bytes.extend_from_slice(bank_switch_bytes(current_bank));
    }
}

/// The one-byte DRO v1 bank-switch instruction for `bank`.
fn bank_switch_bytes(bank: Bank) -> &'static [u8] {
    match bank {
        Bank::Low => &[v1_opcode::BANK_LOW],
        Bank::High => &[v1_opcode::BANK_HIGH],
    }
}

/// Appends a delay of `native` units (VGM samples, DRO milliseconds) encoded in
/// `song`'s own format.
fn append_delay(bytes: &mut Vec<u8>, song: &Song, native: u32) {
    match song.data() {
        SongData::Vgm(_) => {
            // `0x61 nn nn` waits up to 65535 samples; chunk anything longer.
            let mut samples = native;
            while samples > 0 {
                let chunk = samples.min(0xFFFF);
                bytes.push(command::WAIT);
                bytes.extend_from_slice(&(chunk as u16).to_le_bytes());
                samples -= chunk;
            }
        }
        SongData::V1(_) => {
            // `0x01 lo hi` waits (word + 1) ms, up to 65536; chunk longer waits.
            let mut ms = native;
            while ms > 0 {
                let chunk = ms.min(0x1_0000);
                bytes.push(v1_opcode::LONG_DELAY);
                bytes.extend_from_slice(&((chunk - 1) as u16).to_le_bytes());
                ms -= chunk;
            }
        }
        SongData::V2(data) => append_v2_delay(bytes, native, data),
    }
}

/// Appends a DRO v2 delay of `ms` milliseconds: long delays cover the whole
/// multiples of 256 ms, a short delay the 1..=255 ms remainder.
fn append_v2_delay(bytes: &mut Vec<u8>, mut ms: u32, data: &DroDataV2) {
    while ms >= 256 {
        // A long delay waits `(value + 1) << 8` ms -- 256..=65536 in steps of 256.
        let units = (ms / 256).min(256);
        bytes.push(data.long_delay_code());
        bytes.push((units - 1) as u8);
        ms -= units * 256;
    }
    if ms > 0 {
        // A short delay waits `value + 1` ms, 1..=256.
        bytes.push(data.short_delay_code());
        bytes.push((ms - 1) as u8);
    }
}

/// Wraps the piece's command bytes in a container of the source's format,
/// carrying its `opl_type` (and, for VGM, its GD3 with the title blanked; for
/// DRO v2, its codemap). `total_native` is the piece's total delay, which a DRO
/// header records in milliseconds.
fn build_piece(song: &Song, bytes: Vec<u8>, total_native: u32) -> Song {
    let name = piece_name(&song.name, if song.is_vgm() { "vgm" } else { "dro" });
    match song.data() {
        SongData::Vgm(_) => {
            let mut meta = VgmMeta::new(synthesise_header());
            // Copy the source GD3 but blank the track title: it names the whole
            // capture, not this one song. Game/system/author/date carry over; the
            // per-song title is set later in rip quick-edit or bulk tag.
            meta.tag = song
                .vgm_meta()
                .and_then(|meta| meta.tag.clone())
                .map(|mut tag| {
                    tag.track_name_en.clear();
                    tag.track_name_native.clear();
                    tag
                });
            Song::vgm(
                name,
                CONVERSION_VERSION,
                VgmData::new(bytes).expect("materialise only emits VGM commands the indexer knows"),
                song.opl_type,
                meta,
            )
        }
        SongData::V1(_) => Song::dro_v1(
            name,
            DroDataV1::new(bytes).expect("materialise only emits whole v1 instructions"),
            total_native,
            song.opl_type,
        ),
        SongData::V2(source) => Song::dro_v2(
            name,
            DroDataV2::new(
                bytes,
                source.codemap().to_vec(),
                source.short_delay_code(),
                source.long_delay_code(),
            )
            .expect("materialise reuses the source codemap, so every code is valid"),
            total_native,
            song.opl_type,
        ),
    }
}

/// The source's file stem with `extension` (`vgm` or `dro`). Any existing
/// extension is replaced, so a piece cut from a `.vgz` is still written
/// uncompressed (a `.vgm`), and a `.dro` stays a `.dro`.
fn piece_name(source_name: &str, extension: &str) -> String {
    let stem = source_name
        .rsplit_once('.')
        .map_or(source_name, |(stem, _extension)| stem);
    format!("{stem}.{extension}")
}

/// The register state a naive replay of `song`'s writes over `[0, upto)` reaches,
/// as replay triples -- the reference a materialised prelude must reproduce. The
/// current bank is tracked from the bank-switch opcodes, so DRO v1 (whose writes
/// carry no bank) is folded correctly too.
#[cfg(test)]
fn state_over(song: &Song, upto: usize) -> Vec<(Bank, u8, u8)> {
    let mut state = crate::opl_state::OplState::new();
    let mut current = Bank::Low;
    for index in 0..upto {
        match song.instruction(index) {
            Some(DroInstruction::Register { reg, value, bank }) => {
                state.record(Some(bank.unwrap_or(current)), reg, value);
            }
            Some(DroInstruction::BankSwitch(bank)) => current = bank,
            _ => {}
        }
    }
    state.replay_writes()
}

/// The state reached after folding the first `n` register writes of `song`,
/// tracking the current bank from the bank-switch opcodes as [`state_over`] does.
#[cfg(test)]
fn state_after_writes(song: &Song, n: usize) -> Vec<(Bank, u8, u8)> {
    let mut state = crate::opl_state::OplState::new();
    let mut current = Bank::Low;
    let mut seen = 0;
    for index in 0..song.len() {
        if seen == n {
            break;
        }
        match song.instruction(index) {
            Some(DroInstruction::Register { reg, value, bank }) => {
                state.record(Some(bank.unwrap_or(current)), reg, value);
                seen += 1;
            }
            Some(DroInstruction::BankSwitch(bank)) => current = bank,
            _ => {}
        }
    }
    state.replay_writes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::song::OplType;
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
        assert_eq!(segments[0].start_time, 0);
        assert_eq!(segments[0].duration, 1000);
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
        assert_eq!(segments[0].start_time, 0);
        assert_eq!(segments[0].duration, 500);
        // Piece B: starts at index 4 (the first write after the gap), ends at 7.
        assert_eq!((segments[1].start, segments[1].end), (4, 7));
        // It begins after A's 500 samples plus the 10000 gap.
        assert_eq!(segments[1].start_time, 10_500);
        assert_eq!(segments[1].duration, 500);
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
        // start_time counts the leading silence that came before it.
        assert_eq!(segments[0].start_time, 20_000);
        assert_eq!(segments[0].duration, 500);
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
        assert_eq!(segments[0].duration, 600);
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
            let piece = materialise(&song, segment, true, 0);
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
            let piece = materialise(&song, segment, true, 0);
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

        let piece = materialise(&song, &segments[1], true, 0);
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

        let piece = materialise(&song, first, true, 0);
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

        let with = materialise(&song, second, true, 0);
        let without = materialise(&song, second, false, 0);
        assert!(with.len() > without.len(), "replay adds a prelude");
        // Without a prelude the piece is just its own instructions.
        assert_eq!(without.len(), second.len());
    }

    #[test]
    fn the_gd3_tag_is_copied_but_the_title_is_cleared() {
        let mut song = capture();
        let tag = Gd3Tag {
            track_name_en: "Whole Capture".to_owned(),
            track_name_native: "\u{5168}".to_owned(),
            game_name_en: "Sound Test".to_owned(),
            track_author_en: "Composer".to_owned(),
            ..Gd3Tag::default()
        };
        song.vgm_meta_mut().unwrap().tag = Some(tag);

        for segment in &detect_segments(&song, 8000) {
            let piece = materialise(&song, segment, true, 0);
            let piece_tag = piece.vgm_meta().unwrap().tag.as_ref().unwrap();
            // The capture-wide title is blanked; the rest carries over.
            assert_eq!(piece_tag.track_name_en, "");
            assert_eq!(piece_tag.track_name_native, "");
            assert_eq!(piece_tag.game_name_en, "Sound Test");
            assert_eq!(piece_tag.track_author_en, "Composer");
            // And that survives a write/read cycle.
            let reread = read_song("piece.vgm", &write_song(&piece).unwrap()).unwrap();
            let reread_tag = reread.vgm_meta().unwrap().tag.as_ref().unwrap();
            assert_eq!(reread_tag.track_name_en, "");
            assert_eq!(reread_tag.game_name_en, "Sound Test");
        }
    }

    #[test]
    fn a_source_without_a_tag_yields_untagged_pieces() {
        let song = capture();
        assert!(song.vgm_meta().unwrap().tag.is_none());
        let piece = materialise(&song, &detect_segments(&song, 8000)[0], true, 0);
        assert!(piece.vgm_meta().unwrap().tag.is_none());
    }

    #[test]
    fn the_piece_durations_sum_to_the_original_minus_the_gaps() {
        let song = capture();
        let segments = detect_segments(&song, 8000);

        // Each piece's duration is its segment length (the prelude adds no time).
        let mut total = 0u32;
        for segment in &segments {
            let piece = materialise(&song, segment, true, 0);
            assert_eq!(piece.total_delay_samples(), segment.duration);
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
        let piece = materialise(&song, &segments[0], true, 0);
        // 300 internal only; the 400 trailing wait is not part of the piece.
        assert_eq!(piece.total_delay_samples(), 300);
    }

    #[test]
    fn a_vgz_source_still_produces_an_uncompressed_piece() {
        // The piece name drives write_song's compression choice; a `.vgz` source
        // must not yield gzipped pieces named `.vgm`.
        assert_eq!(piece_name("capture.vgz", "vgm"), "capture.vgm");
        assert_eq!(piece_name("my.sound.test.vgm", "vgm"), "my.sound.test.vgm");
        assert_eq!(piece_name("noext", "vgm"), "noext.vgm");
        assert_eq!(piece_name("capture.dro", "dro"), "capture.dro");

        let mut song = capture();
        song.name = "capture.vgz".to_owned();
        let piece = materialise(&song, &detect_segments(&song, 8000)[0], true, 0);
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
            let piece = materialise(&capture, segment, true, 0);
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

    // -- DRO captures ------------------------------------------------------

    /// A three-song DRO v2 capture: registers via a shared codemap, 100 ms
    /// internal delays, 1024 ms gaps (over a 750 ms threshold).
    fn dro_v2_capture() -> Song {
        let codemap = vec![0x20, 0x40, 0xB0, 0x21, 0xB1, 0x22, 0xB2];
        let (short, long) = (0xFE, 0xFF);
        // A 100 ms short delay is `[short, 99]`; a 1024 ms long delay is
        // `[long, 3]` -> (3 + 1) << 8.
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0x01, 1, 0x10, short, 99, 2, 0x31]); // song 0
        data.extend_from_slice(&[long, 3]); // gap
        data.extend_from_slice(&[3, 0x02, short, 99, 4, 0x32]); // song 1
        data.extend_from_slice(&[long, 3]); // gap
        data.extend_from_slice(&[5, 0x03, short, 99, 6, 0x33]); // song 2
        Song::dro_v2(
            "capture.dro".to_owned(),
            DroDataV2::new(data, codemap, short, long).unwrap(),
            0,
            OplType::Opl2,
        )
    }

    /// A two-song DRO v1 capture whose first song writes the high bank (via
    /// bank-switch opcodes), so the second song's prelude must restore it.
    fn dro_v1_capture() -> Song {
        let mut data = Vec::new();
        // song 0: low 0x20=0x01, high 0x40=0x10, back to low, 100 ms, low 0xB0=0x31
        data.extend_from_slice(&[0x20, 0x01]);
        data.push(0x03); // BANK_HIGH
        data.extend_from_slice(&[0x40, 0x10]);
        data.push(0x02); // BANK_LOW
        data.extend_from_slice(&[0x00, 99]); // 100 ms short delay
        data.extend_from_slice(&[0xB0, 0x31]);
        data.extend_from_slice(&[0x01, 0xFF, 0x03]); // 1024 ms long-delay gap
        // song 1: low 0x21=0x02, 100 ms, low 0xB1=0x32
        data.extend_from_slice(&[0x21, 0x02, 0x00, 99, 0xB1, 0x32]);
        Song::dro_v1(
            "capture.dro".to_owned(),
            DroDataV1::new(data).unwrap(),
            0,
            OplType::DualOpl2,
        )
    }

    #[test]
    fn a_dro_v2_capture_splits_into_dro_pieces() {
        let song = dro_v2_capture();
        // 750 ms threshold: the 1024 ms gaps split, the 100 ms delays do not.
        let segments = detect_segments(&song, 750);
        assert_eq!(segments.len(), 3);

        for (index, segment) in segments.iter().enumerate() {
            let expected = state_over(&song, segment.start);
            let piece = materialise(&song, segment, true, 0);
            assert_eq!(piece.file_type.name(), "DRO", "a DRO piece stays a DRO");
            assert!(piece.name.ends_with(".dro"));
            // Round-trips through the DRO writer/reader...
            let reread = read_song("piece.dro", &write_song(&piece).unwrap()).unwrap();
            // ...opening on the register state the capture had reached there.
            assert_eq!(
                state_after_writes(&reread, expected.len()),
                expected,
                "piece {index} does not restore the capture's state"
            );
        }
        // Piece 1's prelude restores song 0's three registers (0x20, 0x40, 0xB0).
        assert_eq!(state_over(&song, segments[1].start).len(), 3);
    }

    #[test]
    fn a_dro_v1_prelude_restores_state_across_bank_switches() {
        let song = dro_v1_capture();
        let segments = detect_segments(&song, 750);
        assert_eq!(segments.len(), 2);

        // Song 0 touched the low bank (0x20, 0xB0) and the high bank (0x40).
        let expected = state_over(&song, segments[1].start);
        assert_eq!(expected.len(), 3);
        assert!(
            expected.contains(&(Bank::High, 0x40, 0x10)),
            "high write tracked"
        );

        let piece = materialise(&song, &segments[1], true, 0);
        let reread = read_song("piece.dro", &write_song(&piece).unwrap()).unwrap();
        assert_eq!(
            state_after_writes(&reread, expected.len()),
            expected,
            "the v1 prelude must restore both banks"
        );
    }

    // -- decay tail --------------------------------------------------------

    #[test]
    fn a_decay_tail_appends_up_to_the_trailing_gap() {
        // song 0, a 10000-sample gap, song 1. With a 4000-sample tail, piece 0
        // keeps 4000 of that gap; without a tail it keeps none.
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
        assert_eq!(segments[0].trailing_gap, 10_000);

        let no_tail = materialise(&song, &segments[0], true, 0);
        assert_eq!(no_tail.total_delay_samples(), 500);

        let tailed = materialise(&song, &segments[0], true, 4000);
        assert_eq!(tailed.total_delay_samples(), 500 + 4000);
    }

    #[test]
    fn a_decay_tail_is_capped_by_the_actual_gap() {
        // The last song's trailing gap is only 500 samples; asking for 5000 keeps
        // just the 500 that were there.
        let song = song_of(&[write(0x20, 0x01), wait(500), write(0x21, 0x02), wait(500)]);
        let segments = detect_segments(&song, 8000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].trailing_gap, 500);
        let piece = materialise(&song, &segments[0], true, 5000);
        // 500 internal + 500 tail (all the gap there was), not 500 + 5000.
        assert_eq!(piece.total_delay_samples(), 500 + 500);
    }

    #[test]
    fn a_dro_decay_tail_is_encoded_in_milliseconds() {
        let song = dro_v2_capture();
        let segments = detect_segments(&song, 750);
        assert_eq!(segments[0].trailing_gap, 1024);
        // A 300 ms tail on a DRO piece adds 300 ms (encoded as v2 delays).
        let piece = materialise(&song, &segments[0], true, 300);
        let bare = materialise(&song, &segments[0], true, 0);
        assert_eq!(piece.total_delay_ms(), bare.total_delay_ms() + 300);
        // And it still reads back cleanly.
        let reread = read_song("piece.dro", &write_song(&piece).unwrap()).unwrap();
        assert_eq!(reread.total_delay_ms(), piece.total_delay_ms());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::song::OplType;
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
            let piece = materialise(&song, &segment, true, 0);
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
                let piece = materialise(&song, &segment, true, 0);
                prop_assert_eq!(piece.total_delay_samples(), segment.duration);
            }
        }
    }
}
