//! Cropping a song down to its marked region, and cutting that region out.
//!
//! Two edits driven by the same pair of markers the loop region uses:
//!
//! - [`crop_to_region`] keeps `[start, end)` and throws the rest away.
//! - [`delete_region`] throws `[start, end)` away and keeps the rest.
//!
//! Both leave an *edge* where instructions that used to run no longer do, and an
//! OPL register is a latch: whatever survives would otherwise play on whatever
//! the chip happened to hold at that point. So each edit splices in the writes
//! that carry the chip across the edge -- a *state patch*,
//! the diff between the register state at one point in the original stream and
//! the state at another. A crop's prelude is the diff from a blank chip to the
//! state at `start`; a delete's seam patch is the diff from the state at `start`
//! to the state at `end`. Trimming an intro is just the latter with `start == 0`,
//! where the diff from blank is the whole state replay.
//!
//! Neither patch carries any delay, so what survives keeps its original timing to
//! the millisecond.
//!
//! The stream is rebuilt wholesale rather than expressed as a delete and an
//! insert: the patch is new instructions in the middle of surviving ones, and the
//! loop markers have to be remapped across it, which the incremental slide rule
//! behind [`slide_index_past_deletion`](crate::slide_index_past_deletion) cannot
//! express. [`ReplaceStream`](crate::undo::ReplaceStream) makes that undoable.

use crate::song::{DroDataV1, DroDataV2, DroSong, DroSongData, StreamSnapshot};
use crate::state_patch::{StateFold, append_patch};

/// A rebuilt stream and everything derived from it, ready to install.
///
/// Both edits return one; [`Self::install`] puts it into the song, and
/// [`ReplaceStream`](crate::undo::ReplaceStream) is how that is made undoable.
#[derive(Debug, Clone)]
pub struct CropOutcome {
    /// The rebuilt stream, in the same encoding as the source's.
    pub data: DroSongData,
    /// The summed delay of what was kept: the DRO header field.
    pub ms_length: u32,
    /// How many instructions the state patch contributed -- the register writes
    /// that carry the chip across the edge, plus any DRO v1 bank switches. Zero
    /// when nothing had to be restored.
    pub patch_len: usize,
}

impl CropOutcome {
    /// The number of instructions the edited song has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the edit leaves nothing behind. Always `false` in practice: both
    /// edits decline a region that would empty the song, so this exists to pair
    /// with [`Self::len`] rather than to be branched on.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Installs this outcome into `song`, which must be the song it came from.
    pub fn install(self, song: &mut DroSong) {
        song.replace_data(self.into());
    }
}

impl From<CropOutcome> for StreamSnapshot {
    fn from(outcome: CropOutcome) -> Self {
        Self {
            data: outcome.data,
            ms_length: outcome.ms_length,
        }
    }
}

/// Keeps only the instructions in `[start, end)`, prefixed with the writes that
/// recreate the register state the stream had reached at `start`.
///
/// Returns `None` when there is nothing to do: an empty or inverted range, one
/// reaching past the end of the song, or one that already covers all of it.
#[must_use]
pub fn crop_to_region(song: &DroSong, start: usize, end: usize) -> Option<CropOutcome> {
    let len = song.len();
    if start >= end || end > len || (start == 0 && end == len) {
        return None;
    }

    let mut bytes = Vec::new();
    // From a blank chip up to the state the stream had reached at `start`.
    let patch_len = append_patch(
        &mut bytes,
        song,
        &StateFold::blank(),
        &StateFold::over(song, start),
    );
    append_range(&mut bytes, song, start, end);

    Some(CropOutcome {
        data: rebuild(song, bytes),
        ms_length: span_ms(song, start, end),
        patch_len,
    })
}

/// Cuts the instructions in `[start, end)` out, splicing in the writes that carry
/// the chip's register state across the gap so what follows still plays on the
/// state it was written against.
///
/// Returns `None` when there is nothing sensible to do: an empty or inverted
/// range, one reaching past the end of the song, or one covering the whole of it
/// -- an empty song is not a useful thing to arrive at, and the same guard on
/// [`crop_to_region`] declines the mirror-image no-op.
#[must_use]
pub fn delete_region(song: &DroSong, start: usize, end: usize) -> Option<CropOutcome> {
    let len = song.len();
    if start >= end || end > len || (start == 0 && end == len) {
        return None;
    }

    // One walk over `[0, end)`: fold up to the seam, then carry the same fold on
    // through the doomed region to reach what the surviving tail expects.
    let at_start = StateFold::over(song, start);
    let mut at_end = at_start.clone();
    at_end.advance(song, start, end);

    let mut bytes = Vec::new();
    append_range(&mut bytes, song, 0, start);
    // Nothing follows a deleted tail, so there is nothing to carry the state to.
    let patch_len = if end < len {
        append_patch(&mut bytes, song, &at_start, &at_end)
    } else {
        0
    };
    append_range(&mut bytes, song, end, len);

    Some(CropOutcome {
        data: rebuild(song, bytes),
        ms_length: song.total_delay_ms() - span_ms(song, start, end),
        patch_len,
    })
}

/// Appends `song`'s instructions `[from, to)`, byte for byte from its stream.
fn append_range(bytes: &mut Vec<u8>, song: &DroSong, from: usize, to: usize) {
    for index in from..to {
        bytes.extend_from_slice(
            song.data()
                .raw_instruction(index)
                .expect("the range was bounds-checked against the song"),
        );
    }
}

/// The summed delay of instructions `[from, to)`, in milliseconds.
fn span_ms(song: &DroSong, from: usize, to: usize) -> u32 {
    let at = |index: usize| {
        song.ms_offset_at(index)
            .unwrap_or_else(|| song.total_delay_ms())
    };
    at(to).saturating_sub(at(from))
}

/// Wraps `bytes` in a stream of the same encoding as `song`'s.
///
/// Every byte came from that stream (or, for a v1 bank switch, is a fixed opcode),
/// so each constructor is being handed instructions it already knows how to index.
fn rebuild(song: &DroSong, bytes: Vec<u8>) -> DroSongData {
    match song.data() {
        DroSongData::V1(_) => DroSongData::V1(
            DroDataV1::new(bytes).expect("the bytes are whole instructions from a v1 stream"),
        ),
        DroSongData::V2(source) => DroSongData::V2(
            DroDataV2::new(
                bytes,
                source.codemap().to_vec(),
                source.short_delay_code(),
                source.long_delay_code(),
            )
            .expect("the source codemap covers every code copied from its own stream"),
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::song::Bank;
    use crate::state_patch::{state_after_writes, state_over};

    fn apply(song: &DroSong, outcome: Option<CropOutcome>) -> (DroSong, usize) {
        let outcome = outcome.expect("the edit was expected to do something");
        let patch_len = outcome.patch_len;
        let mut edited = song.clone();
        outcome.install(&mut edited);
        (edited, patch_len)
    }

    fn cropped(song: &DroSong, start: usize, end: usize) -> (DroSong, usize) {
        apply(song, crop_to_region(song, start, end))
    }

    fn deleted(song: &DroSong, start: usize, end: usize) -> (DroSong, usize) {
        apply(song, delete_region(song, start, end))
    }

    /// Through the writer and back, so every assertion also proves the rebuilt
    /// stream is a file the reader accepts and decodes to the same instructions.
    fn round_trip(song: &DroSong) -> DroSong {
        let bytes = write_song(song).expect("an edited song writes");
        read_song(&song.name, &bytes).expect("and reads back")
    }

    // -- DRO formats ---------------------------------------------------------

    #[test]
    fn a_dro_v2_crop_restores_state_and_shortens_the_header() {
        let song = crate::song::fixtures::dro_song_v2();
        let before = song.ms_length;
        let (edited, _) = cropped(&song, 4, 12);

        assert_eq!(edited.file_type.name(), "DRO", "a DRO stays a DRO");
        let expected = state_over(&song, 4);
        assert!(!expected.is_empty(), "there is real state to restore");
        let reread = round_trip(&edited);
        assert_eq!(state_after_writes(&reread, expected.len()), expected);

        // The header records only what was kept, and the reader agrees.
        let kept = span_ms(&song, 4, 12);
        assert!(kept < before);
        assert_eq!(edited.ms_length, kept);
        assert_eq!(reread.total_delay_ms(), kept);
    }

    #[test]
    fn a_dro_v2_delete_bridges_the_seam_and_shortens_the_header() {
        let song = crate::song::fixtures::dro_song_v2();
        let (edited, patch_len) = deleted(&song, 3, 9);
        let reread = round_trip(&edited);
        assert_eq!(state_over(&reread, 3 + patch_len), state_over(&song, 9));
        assert_eq!(edited.ms_length, song.ms_length - span_ms(&song, 3, 9));
        assert_eq!(reread.total_delay_ms(), edited.ms_length);
    }

    #[test]
    fn a_dro_v1_crop_restores_state_across_bank_switches() {
        let song = crate::song::fixtures::dro_song_v1();
        // The fixture's high-bank switch is instruction 4, and the escaped write
        // after it lands in the high bank, so cropping past it needs a prelude
        // that gets both banks right.
        let (edited, _) = cropped(&song, 6, song.len());
        let expected = state_over(&song, 6);
        assert!(
            expected.iter().any(|&(bank, _, _)| bank == Bank::High),
            "the fixture reaches the high bank before the crop"
        );
        let reread = round_trip(&edited);
        assert_eq!(state_after_writes(&reread, expected.len()), expected);
    }

    #[test]
    fn a_dro_v1_delete_leaves_the_tail_in_the_right_bank() {
        let song = crate::song::fixtures::dro_song_v1();
        for start in 0..song.len() {
            for end in (start + 1)..song.len() {
                let (edited, patch_len) = deleted(&song, start, end);
                let reread = round_trip(&edited);
                assert_eq!(
                    state_over(&reread, start + patch_len),
                    state_over(&song, end),
                    "deleting {start}..{end} leaves the tail on the wrong state"
                );
                // A v1's bank-less tail writes only land correctly if the patch
                // left the chip in the bank the original was in.
                assert_eq!(
                    state_over(&reread, reread.len()),
                    state_over(&song, song.len()),
                    "deleting {start}..{end} changes the final state"
                );
            }
        }
    }
}
