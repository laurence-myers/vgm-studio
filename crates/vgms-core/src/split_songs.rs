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
use crate::song::{DroDataV1, DroDataV2, Song, SongData};
use crate::state_patch::{StateFold, append_patch};
use crate::vgm::VgmFile;
#[cfg(test)]
use crate::vgm::data::command;
use crate::vgm::stream::VgmCommand;

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

/// The delay units per second in a DRO song's native unit: `1000`
/// (milliseconds). Lets a UI convert the native-unit [`Segment`] fields and its
/// threshold to and from seconds. (A VGM capture splits through
/// [`detect_segments_in_vgm`], which works in samples.)
#[must_use]
pub const fn native_rate(_song: &Song) -> u32 {
    1000
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
    detect(song.len(), threshold, |index| {
        let instruction = song.instruction(index).expect("index < len");
        (instruction.delay_ms(), instruction.is_delay())
    })
}

/// The same detection over a VGM command stream, for a capture of any chips.
///
/// The threshold is in samples, the VGM's native unit. A `0x8n` DAC write is not
/// a gap even though it waits: it writes a sample first, so the chip is being
/// played, not left silent. Everything else that waits and nothing else is.
///
/// Returns none when the file's stream did not walk.
#[must_use]
pub fn detect_segments_in_vgm(file: &VgmFile, threshold: u32) -> Vec<Segment> {
    let Some(stream) = file.stream() else {
        return Vec::new();
    };
    detect(stream.len(), threshold, |index| {
        (
            stream.wait_samples(index),
            matches!(stream.get(index), Some(VgmCommand::Wait(_))),
        )
    })
}

/// The detection itself, over whatever `at(index)` reports for each of `len`
/// commands: how long it waits in the native unit, and whether it is *only* a
/// wait. A command that waits but also does something is time passing inside a
/// piece, never a gap between two.
fn detect(len: usize, threshold: u32, at: impl Fn(usize) -> (u32, bool)) -> Vec<Segment> {
    let threshold = u64::from(threshold).max(1);

    // A native-unit exclusive prefix sum, so a segment's start time and duration
    // are lookups rather than re-walks.
    let mut prefix = Vec::with_capacity(len + 1);
    let mut acc = 0u32;
    prefix.push(acc);
    for index in 0..len {
        acc = acc.saturating_add(at(index).0);
        prefix.push(acc);
    }

    let mut segments = Vec::new();
    // The current piece: its first register write, its last so far, and the
    // native delay of the run seen since that last real command.
    let mut seg_start: Option<usize> = None;
    let mut last_real: Option<usize> = None;
    let mut gap: u64 = 0;

    for index in 0..len {
        let (delay, is_delay) = at(index);
        if is_delay {
            gap += u64::from(delay);
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

/// The same lift, for a VGM of any chips.
///
/// The piece is a file in its own right: it declares the source's chips at the
/// source's clocks, carries the source's GD3 with the track title blanked (that
/// title named the whole capture), and does not loop -- the source's loop
/// described the capture, not this piece of it. Its header's sample total and
/// the file name follow from the piece itself.
///
/// `state_replay` prefixes the chip state the capture had reached by the
/// segment's start, folded through [`crate::chip_state`] and re-emitted as the
/// source's own bytes: the same treatment the OPL path gives an OPL capture,
/// for whatever chips this one turns out to have. `trailing_tail` is in samples
/// and capped at the gap that actually followed the piece.
///
/// `None` if the segment is empty or the source's stream did not walk.
#[must_use]
pub fn materialise_vgm(
    file: &VgmFile,
    segment: &Segment,
    state_replay: bool,
    trailing_tail: u32,
) -> Option<VgmFile> {
    // A piece that starts at the very beginning has no state to restore: the
    // chips were blank there too.
    let replay = state_replay && segment.start > 0;
    let tail = trailing_tail.min(segment.trailing_gap);
    let (mut piece, _report) = file.extract_region(segment.start, segment.end, replay, tail)?;

    piece.name = piece_name(&file.name, "vgm");
    if let Some(tag) = piece.tag.as_mut() {
        tag.track_name_en.clear();
        tag.track_name_native.clear();
    }
    Some(piece)
}

/// Captures the OPL state reached over `[0, start)` and appends the writes that
/// recreate it -- the patch from a blank chip to the state at `start`. See
/// [`append_patch`](crate::state_patch::append_patch) for how it is emitted, and
/// how a DRO v1's bank switches are handled.
fn append_state_prelude(bytes: &mut Vec<u8>, song: &Song, start: usize) {
    append_patch(
        bytes,
        song,
        &StateFold::blank(),
        &StateFold::over(song, start),
    );
}

/// Appends a delay of `native` units (VGM samples, DRO milliseconds) encoded in
/// `song`'s own format.
fn append_delay(bytes: &mut Vec<u8>, song: &Song, native: u32) {
    match song.data() {
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
    let name = piece_name(&song.name, "dro");
    match song.data() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::song::{Bank, OplType};
    // The reference folds every state-replay assertion is written against, shared
    // with the crop edits that emit the same patches.
    use crate::state_patch::{state_after_writes, state_over};

    /// A low-bank OPL2 write, `0x5A reg value`.
    fn write(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812, reg, value]
    }

    /// A long wait of `samples` (0..=65535), as one `0x61` VGM command.
    fn wait(samples: u16) -> Vec<u8> {
        let mut bytes = vec![command::WAIT];
        bytes.extend_from_slice(&samples.to_le_bytes());
        bytes
    }

    /// A capture for chips the OPL model knows nothing about, as a `VgmFile`.
    /// Two songs, parted by `gap_samples` of silence; each song writes one
    /// register to each of its two chips.
    fn other_chip_capture(gap_samples: u16) -> VgmFile {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let song = |ym: u8, ay: u8| {
            let mut bytes = vec![0x58, 0x28, ym];
            bytes.extend(wait(1000));
            bytes.extend([0xA0, 0x07, ay]);
            bytes
        };
        let mut stream = song(0xF0, 0x38);
        stream.extend(wait(gap_samples));
        stream.extend(song(0xF1, 0x39));
        stream.push(0x66);

        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x161);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        put_u32(
            &mut bytes,
            crate::vgm::ChipKind::Ym2610.clock_offset(),
            8_000_000,
        );
        put_u32(
            &mut bytes,
            crate::vgm::ChipKind::Ay8910.clock_offset(),
            2_000_000,
        );
        bytes.extend_from_slice(&stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        crate::vgm::file::read("capture.vgm", &bytes).expect("a walkable capture")
    }

    #[test]
    fn a_capture_for_other_chips_splits_at_its_silence() {
        let file = other_chip_capture(30_000);
        // A gap shorter than the threshold is music, not a boundary.
        assert_eq!(detect_segments_in_vgm(&file, 40_000).len(), 1);
        let songs = detect_segments_in_vgm(&file, 20_000);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].start, 0);
        // The gap's own wait belongs to neither piece.
        assert_eq!(songs[1].start, 4);
        assert_eq!(songs[1].start_time, 1000 + 30_000);
    }

    #[test]
    fn a_piece_opens_on_the_state_the_capture_had_reached() {
        let file = other_chip_capture(30_000);
        let songs = detect_segments_in_vgm(&file, 20_000);
        let piece = materialise_vgm(&file, &songs[1], true, 0).expect("the second song");

        // The piece is the second song's three commands, with one restore per
        // chip in front: the values the *first* song left behind.
        let stream = piece.stream().expect("the piece walks");
        assert_eq!(stream.len(), 5);
        assert_eq!(stream.raw_command(0), Some([0x58, 0x28, 0xF0].as_slice()));
        assert_eq!(stream.raw_command(1), Some([0xA0, 0x07, 0x38].as_slice()));
        assert_eq!(stream.raw_command(2), Some([0x58, 0x28, 0xF1].as_slice()));

        // It stands alone: the capture's chips at the capture's clocks, its own
        // sample total, and no loop.
        assert_eq!(piece.chip_list(), "YM2610, AY8910");
        assert_eq!(piece.header.total_samples(), 1000);
        assert_eq!(piece.loop_index(), None);
        assert_eq!(piece.name, "capture.vgm");
    }

    #[test]
    fn the_first_piece_needs_no_prelude_and_a_tail_is_capped() {
        let file = other_chip_capture(30_000);
        let songs = detect_segments_in_vgm(&file, 20_000);

        // Nothing preceded the first song, so it is only its own commands.
        let first = materialise_vgm(&file, &songs[0], true, 0).expect("the first song");
        assert_eq!(first.stream().unwrap().len(), 3);

        // A tail is capped at the silence that actually followed the piece.
        let with_tail = materialise_vgm(&file, &songs[0], true, 99_999).expect("the first song");
        assert_eq!(with_tail.header.total_samples(), 1000 + 30_000);
    }

    /// An OPL2 capture whose YM3812 clock is deliberately non-canonical. Two
    /// songs parted by `gap_samples` of silence, each a single OPL2 write.
    fn opl2_capture_with_clock(clock: u32, gap_samples: u16) -> VgmFile {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let song = |reg: u8, value: u8| {
            let mut bytes = write(reg, value);
            bytes.extend(wait(1000));
            bytes
        };
        let mut stream = song(0x20, 0x01);
        stream.extend(wait(gap_samples));
        stream.extend(song(0x20, 0x02));
        stream.push(0x66);

        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x161);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        put_u32(
            &mut bytes,
            crate::vgm::ChipKind::Ym3812.clock_offset(),
            clock,
        );
        bytes.extend_from_slice(&stream);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        crate::vgm::file::read("capture.vgm", &bytes).expect("a walkable OPL2 capture")
    }

    /// The bug mg-6 fixes, at its root: splitting through the VGM stack keeps
    /// the source header verbatim, so a non-canonical chip clock survives. The
    /// OPL split path re-synthesised a v1.51 header from hard-coded clocks
    /// (OPL2 = 3_579_545), which would have moved this rip's pitch and tempo;
    /// `materialise_vgm` -> `extract_region` clones the header instead.
    #[test]
    fn splitting_a_vgm_preserves_a_non_canonical_clock() {
        const ODD_CLOCK: u32 = 4_000_000; // canonical OPL2 is 3_579_545
        let ym3812_clock = |file: &VgmFile| {
            file.header
                .chips()
                .iter()
                .find(|chip| chip.kind == crate::vgm::ChipKind::Ym3812)
                .map(|chip| chip.clock)
        };

        let file = opl2_capture_with_clock(ODD_CLOCK, 30_000);
        assert_eq!(ym3812_clock(&file), Some(ODD_CLOCK), "the source clock");

        let songs = detect_segments_in_vgm(&file, 20_000);
        assert_eq!(songs.len(), 2);
        let piece = materialise_vgm(&file, &songs[1], true, 0).expect("the second song");
        assert_eq!(
            ym3812_clock(&piece),
            Some(ODD_CLOCK),
            "the split piece keeps the source clock, not the canonical 3_579_545"
        );
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
