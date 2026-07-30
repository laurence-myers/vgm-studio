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

use crate::song::{DroDataV1, DroDataV2, Song, SongData, StreamSnapshot};
use crate::state_patch::{StateFold, append_patch};
use crate::vgm::VgmData;

/// A rebuilt stream and everything derived from it, ready to install.
///
/// Both edits return one; [`Self::install`] puts it into the song, and
/// [`ReplaceStream`](crate::undo::ReplaceStream) is how that is made undoable.
#[derive(Debug, Clone)]
pub struct CropOutcome {
    /// The rebuilt stream, in the same encoding as the source's.
    pub data: SongData,
    /// The summed delay of what was kept. A DRO header field; a VGM derives its
    /// own, so this is advisory there.
    pub ms_length: u32,
    /// The loop point remapped onto the rebuilt stream, VGM only.
    pub loop_point: Option<usize>,
    /// The exclusive loop end, likewise.
    pub loop_end: Option<usize>,
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
    pub fn install(self, song: &mut Song) {
        song.replace_data(self.into());
    }
}

impl From<CropOutcome> for StreamSnapshot {
    fn from(outcome: CropOutcome) -> Self {
        Self {
            data: outcome.data,
            ms_length: outcome.ms_length,
            loop_point: outcome.loop_point,
            loop_end: outcome.loop_end,
        }
    }
}

/// Keeps only the instructions in `[start, end)`, prefixed with the writes that
/// recreate the register state the stream had reached at `start`.
///
/// Returns `None` when there is nothing to do: an empty or inverted range, one
/// reaching past the end of the song, or one that already covers all of it.
#[must_use]
pub fn crop_to_region(song: &Song, start: usize, end: usize) -> Option<CropOutcome> {
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

    // An index inside the kept region slides down by `start` and back up past the
    // prelude; one before it has lost its target, and one after it is gone.
    let map = |index: usize| match index {
        _ if index < start => Some(0),
        _ if index < end => Some(index - start + patch_len),
        _ => None,
    };
    let new_len = patch_len + (end - start);
    let (loop_point, loop_end) = remap_loop(song, new_len, map);

    Some(CropOutcome {
        data: rebuild(song, bytes),
        ms_length: span_ms(song, start, end),
        loop_point,
        loop_end,
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
pub fn delete_region(song: &Song, start: usize, end: usize) -> Option<CropOutcome> {
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

    // Everything before the cut stays put; everything after it slides down by the
    // cut's length and back up past the patch. An index inside the cut lands on
    // the seam, which is where its instruction now effectively is.
    let map = |index: usize| {
        Some(match index {
            _ if index < start => index,
            _ if index < end => start,
            _ => index - (end - start) + patch_len,
        })
    };
    let new_len = start + patch_len + (len - end);
    let (loop_point, loop_end) = remap_loop(song, new_len, map);

    Some(CropOutcome {
        data: rebuild(song, bytes),
        ms_length: song.total_delay_ms() - span_ms(song, start, end),
        loop_point,
        loop_end,
        patch_len,
    })
}

/// Appends `song`'s instructions `[from, to)`, byte for byte from its stream.
fn append_range(bytes: &mut Vec<u8>, song: &Song, from: usize, to: usize) {
    for index in from..to {
        bytes.extend_from_slice(
            song.data()
                .raw_instruction(index)
                .expect("the range was bounds-checked against the song"),
        );
    }
}

/// Moves a VGM's loop markers onto the rebuilt stream with `map`, which gives an
/// old index's new home or `None` if it no longer has one.
///
/// A song that does not loop stays that way. A loop point with nowhere to go
/// takes the end with it -- an end without a start describes a region with no
/// beginning -- and an end that no longer sits above the start, or that reaches
/// the new end of the song, falls back to `None`, which already means "to the
/// end". Those are the rules the metadata dialog stores loops by, so a remapped
/// loop and a typed one cannot mean different things.
fn remap_loop(
    song: &Song,
    new_len: usize,
    map: impl Fn(usize) -> Option<usize>,
) -> (Option<usize>, Option<usize>) {
    let Some(meta) = song.vgm_meta() else {
        return (None, None);
    };
    let Some(loop_point) = meta.loop_point else {
        return (None, None);
    };
    let Some(point) = map(loop_point) else {
        log::warn!("the VGM loop point fell outside the kept region; the song no longer loops");
        return (None, None);
    };
    let end = meta
        .loop_end
        .and_then(&map)
        .filter(|&end| end > point && end < new_len);
    (Some(point), end)
}

/// The summed delay of instructions `[from, to)`, in milliseconds.
fn span_ms(song: &Song, from: usize, to: usize) -> u32 {
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
fn rebuild(song: &Song, bytes: Vec<u8>) -> SongData {
    match song.data() {
        SongData::Vgm(_) => SongData::Vgm(
            VgmData::new(bytes).expect("the bytes are whole commands from a VGM stream"),
        ),
        SongData::V1(_) => SongData::V1(
            DroDataV1::new(bytes).expect("the bytes are whole instructions from a v1 stream"),
        ),
        SongData::V2(source) => SongData::V2(
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
    use crate::song::{Bank, OplType};
    use crate::state_patch::{state_after_writes, state_over};
    use crate::vgm::VgmMeta;
    use crate::vgm::data::command;
    use crate::vgm::io::synthesise_header;

    /// A low-bank OPL2 write, `0x5A reg value`.
    fn write(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812, reg, value]
    }

    /// A high-bank write, `0xAA reg value`.
    fn write_hi(reg: u8, value: u8) -> Vec<u8> {
        vec![command::YM3812_CHIP_2, reg, value]
    }

    fn wait(samples: u16) -> Vec<u8> {
        let mut bytes = vec![command::WAIT];
        bytes.extend_from_slice(&samples.to_le_bytes());
        bytes
    }

    fn vgm(chunks: &[Vec<u8>]) -> Song {
        Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(chunks.concat()).unwrap(),
            OplType::DualOpl2,
            VgmMeta::new(synthesise_header()),
        )
    }

    /// The fixture both edits are measured against. Three sections, and a
    /// register (`0x20`) the middle one rewrites, so a seam patch has both a
    /// changed register to carry and an unchanged one (`0x40`) to leave alone.
    ///
    /// ```text
    /// 0: 0x20 = 0x11   3: 0x60 = 0x33   8: 0xB0 = 0x55
    /// 1: wait 100      4: wait 200      9: wait 400
    /// 2: 0x40 = 0x22   5: 0x20 = 0x99
    ///                  6: wait 300
    ///                  7: 0xA0 = 0x44
    /// ```
    fn layered() -> Song {
        vgm(&[
            write(0x20, 0x11),
            wait(100),
            write(0x40, 0x22),
            write(0x60, 0x33),
            wait(200),
            write(0x20, 0x99),
            wait(300),
            write(0xA0, 0x44),
            write(0xB0, 0x55),
            wait(400),
        ])
    }

    /// The middle section of [`layered`].
    const BODY: (usize, usize) = (3, 8);

    fn apply(song: &Song, outcome: Option<CropOutcome>) -> (Song, usize) {
        let outcome = outcome.expect("the edit was expected to do something");
        let patch_len = outcome.patch_len;
        let mut edited = song.clone();
        outcome.install(&mut edited);
        (edited, patch_len)
    }

    fn cropped(song: &Song, start: usize, end: usize) -> (Song, usize) {
        apply(song, crop_to_region(song, start, end))
    }

    fn deleted(song: &Song, start: usize, end: usize) -> (Song, usize) {
        apply(song, delete_region(song, start, end))
    }

    /// Through the writer and back, so every assertion also proves the rebuilt
    /// stream is a file the reader accepts and decodes to the same instructions.
    fn round_trip(song: &Song) -> Song {
        let bytes = write_song(song).expect("an edited song writes");
        read_song(&song.name, &bytes).expect("and reads back")
    }

    // -- crop ----------------------------------------------------------------

    #[test]
    fn a_crop_keeps_the_region_behind_a_state_prelude() {
        let song = layered();
        let (edited, patch_len) = cropped(&song, BODY.0, BODY.1);

        // The prelude restores 0x20 and 0x40, the two registers set before the
        // body; then the body's own five instructions follow.
        assert_eq!(patch_len, 2);
        assert_eq!(edited.len(), 2 + 5);
        assert_eq!(
            edited.data().raw(),
            [
                write(0x20, 0x11), // the prelude, low file ascending
                write(0x40, 0x22),
                write(0x60, 0x33), // the body, verbatim
                wait(200),
                write(0x20, 0x99),
                wait(300),
                write(0xA0, 0x44),
            ]
            .concat()
        );
    }

    #[test]
    fn a_cropped_song_opens_on_the_state_the_original_had_reached() {
        let song = layered();
        // From 1: cropping `0..len` is the whole song, which is declined.
        for start in 1..song.len() {
            let (edited, _) = cropped(&song, start, song.len());
            let expected = state_over(&song, start);
            let reread = round_trip(&edited);
            assert_eq!(
                state_after_writes(&reread, expected.len()),
                expected,
                "a crop from {start} does not restore the original state"
            );
        }
    }

    #[test]
    fn a_crop_from_the_very_start_needs_no_prelude() {
        let song = layered();
        let (edited, patch_len) = cropped(&song, 0, 5);
        assert_eq!(
            patch_len, 0,
            "nothing has played yet, so nothing to restore"
        );
        assert_eq!(edited.len(), 5);
    }

    #[test]
    fn a_crop_keeps_only_the_regions_own_timing() {
        let song = layered();
        assert_eq!(song.total_delay_samples(), 1000);
        let (edited, _) = cropped(&song, BODY.0, BODY.1);
        // The body's two waits, and not the 100 before it or the 400 after.
        assert_eq!(edited.total_delay_samples(), 500);
    }

    #[test]
    fn a_no_op_crop_is_declined() {
        let song = layered();
        let len = song.len();
        assert!(crop_to_region(&song, 0, len).is_none(), "the whole song");
        assert!(crop_to_region(&song, 3, 3).is_none(), "an empty region");
        assert!(crop_to_region(&song, 5, 2).is_none(), "an inverted region");
        assert!(crop_to_region(&song, 0, len + 1).is_none(), "past the end");
    }

    // -- delete --------------------------------------------------------------

    #[test]
    fn a_delete_bridges_the_seam_with_only_what_changed() {
        let song = layered();
        let (edited, patch_len) = deleted(&song, BODY.0, BODY.1);

        // 0x20 changed (0x11 -> 0x99) and 0x60/0xA0 are new, so all three are
        // carried across; 0x40 was set before the cut and never touched inside
        // it, so rewriting it would be pure noise.
        assert_eq!(patch_len, 3);
        assert_eq!(
            edited.data().raw(),
            [
                write(0x20, 0x11), // the head, verbatim
                wait(100),
                write(0x40, 0x22),
                write(0x20, 0x99), // the seam patch, low file ascending
                write(0x60, 0x33),
                write(0xA0, 0x44),
                write(0xB0, 0x55), // the tail, verbatim
                wait(400),
            ]
            .concat()
        );
    }

    #[test]
    fn the_tail_of_a_delete_plays_on_the_state_it_was_written_against() {
        let song = layered();
        for start in 0..song.len() {
            for end in (start + 1)..song.len() {
                let (edited, patch_len) = deleted(&song, start, end);
                let reread = round_trip(&edited);
                // The patch sits at `[start, start + patch_len)`, so folding the
                // edited stream to the end of it must land on exactly the state
                // the original had reached where the tail resumes.
                assert_eq!(
                    state_over(&reread, start + patch_len),
                    state_over(&song, end),
                    "deleting {start}..{end} leaves the tail on the wrong state"
                );
                // And with the tail's own writes replayed on top, the song ends
                // in the state it always did.
                assert_eq!(
                    state_over(&reread, reread.len()),
                    state_over(&song, song.len()),
                    "deleting {start}..{end} changes the final state"
                );
            }
        }
    }

    #[test]
    fn deleting_from_the_start_replays_the_whole_state() {
        // The "trim the intro" case: with nothing before the cut, the seam patch
        // is the diff from a blank chip, i.e. the full state replay.
        let song = layered();
        let (edited, patch_len) = deleted(&song, 0, BODY.1);

        let expected = state_over(&song, BODY.1);
        assert_eq!(
            patch_len,
            expected.len(),
            "every touched register is restored"
        );
        assert_eq!(
            state_after_writes(&round_trip(&edited), expected.len()),
            expected
        );
        // The kept tail keeps its own timing; the intro's 600 samples are gone.
        assert_eq!(edited.total_delay_samples(), 400);
    }

    #[test]
    fn deleting_the_tail_needs_no_patch() {
        // Nothing follows the cut, so there is nothing to carry the state to.
        let song = layered();
        let (edited, patch_len) = deleted(&song, BODY.1, song.len());
        assert_eq!(patch_len, 0);
        assert_eq!(edited.len(), BODY.1);
        assert_eq!(edited.total_delay_samples(), 600);
    }

    #[test]
    fn a_no_op_delete_is_declined() {
        let song = layered();
        let len = song.len();
        assert!(delete_region(&song, 3, 3).is_none(), "an empty region");
        assert!(delete_region(&song, 5, 2).is_none(), "an inverted region");
        assert!(delete_region(&song, 0, len + 1).is_none(), "past the end");
        // An empty song is not a useful thing to arrive at.
        assert!(delete_region(&song, 0, len).is_none(), "the whole song");
    }

    #[test]
    fn neither_edit_can_empty_a_song() {
        // Whatever region is asked for, anything that comes back has something
        // left in it -- so `CropOutcome::is_empty` never fires.
        let song = layered();
        let len = song.len();
        for start in 0..len {
            for end in (start + 1)..=len {
                for outcome in [
                    crop_to_region(&song, start, end),
                    delete_region(&song, start, end),
                ]
                .into_iter()
                .flatten()
                {
                    assert!(!outcome.is_empty(), "{start}..{end} emptied the song");
                }
            }
        }
    }

    // -- loop metadata -------------------------------------------------------

    /// The loop markers a crop or a delete of [`BODY`] remaps `loop_point` to.
    fn remapped(
        song: &Song,
        crop: bool,
        point: usize,
        end: Option<usize>,
    ) -> (Option<usize>, Option<usize>) {
        let mut song = song.clone();
        {
            let meta = song.vgm_meta_mut().unwrap();
            meta.loop_point = Some(point);
            meta.loop_end = end;
        }
        let outcome = if crop {
            crop_to_region(&song, BODY.0, BODY.1)
        } else {
            delete_region(&song, BODY.0, BODY.1)
        }
        .expect("a real edit");
        (outcome.loop_point, outcome.loop_end)
    }

    #[test]
    fn a_crop_remaps_a_loop_point_onto_the_kept_region() {
        let song = layered();
        // Before the region: its target is gone, and the region now *is* the
        // song, so the loop becomes the whole of it -- replaying the prelude on
        // each wrap, which re-establishes the state the body opens on.
        assert_eq!(remapped(&song, true, 1, None).0, Some(0));
        // Inside it: slid down by the region's start and up past the prelude.
        assert_eq!(remapped(&song, true, 3, None).0, Some(2));
        assert_eq!(remapped(&song, true, 5, None).0, Some(4));
        // At or past the end: nothing survives to loop to.
        assert_eq!(remapped(&song, true, 8, None).0, None);
        assert_eq!(remapped(&song, true, 9, None).0, None);
    }

    #[test]
    fn a_delete_remaps_a_loop_point_across_the_seam() {
        let song = layered();
        // Before the cut: untouched.
        assert_eq!(remapped(&song, false, 1, None).0, Some(1));
        // Inside it: onto the seam, where its instruction now effectively is.
        assert_eq!(remapped(&song, false, 4, None).0, Some(3));
        // After it: down by the cut's length, up past the 3-write patch.
        assert_eq!(remapped(&song, false, 8, None).0, Some(6));
        assert_eq!(remapped(&song, false, 9, None).0, Some(7));
    }

    #[test]
    fn a_loop_end_is_only_kept_while_it_bounds_a_real_region() {
        let song = layered();
        // Inside the kept region and above the start: remapped and kept.
        assert_eq!(remapped(&song, true, 3, Some(5)), (Some(2), Some(4)));
        // Reaching past the cropped region: `None` already means "to the end".
        assert_eq!(remapped(&song, true, 3, Some(8)), (Some(2), None));
        // A loop point with nowhere to go takes the end with it.
        assert_eq!(remapped(&song, true, 9, Some(9)), (None, None));

        // And the same rules across a delete's seam.
        assert_eq!(remapped(&song, false, 1, Some(9)), (Some(1), Some(7)));
        assert_eq!(remapped(&song, false, 1, Some(2)), (Some(1), Some(2)));
    }

    #[test]
    fn a_song_that_does_not_loop_still_does_not() {
        let song = layered();
        assert!(song.vgm_meta().unwrap().loop_point.is_none());
        let outcome = crop_to_region(&song, BODY.0, BODY.1).unwrap();
        assert_eq!((outcome.loop_point, outcome.loop_end), (None, None));
    }

    #[test]
    fn a_remapped_loop_survives_a_write_and_read() {
        // The VGM writer panics on a loop point past the end of the stream, so a
        // remap that overshot would be caught here rather than in the wild.
        let mut song = layered();
        song.vgm_meta_mut().unwrap().loop_point = Some(9);
        let (edited, _) = deleted(&song, BODY.0, BODY.1);
        let reread = round_trip(&edited);
        assert_eq!(reread.vgm_meta().unwrap().loop_point, Some(7));
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

    #[test]
    fn a_high_bank_register_survives_a_crop() {
        // A dual-OPL2 capture whose intro writes the high bank: the prelude must
        // restore it as a high-bank write, not a low one.
        let song = vgm(&[
            write(0x20, 0x01),
            write_hi(0x20, 0x09),
            wait(500),
            write(0x40, 0x02),
        ]);
        let (edited, patch_len) = cropped(&song, 3, 4);
        assert_eq!(patch_len, 2);
        let reread = round_trip(&edited);
        assert_eq!(
            reread.instruction(1),
            Some(crate::song::DroInstruction::Register {
                reg: 0x20,
                value: 0x09,
                bank: Some(Bank::High),
            })
        );
        assert_eq!(state_after_writes(&reread, 2), state_over(&song, 3));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::io::{read_song, write_song};
    use crate::song::OplType;
    use crate::state_patch::{state_after_writes, state_over};
    use crate::vgm::VgmMeta;
    use crate::vgm::data::command;
    use crate::vgm::io::synthesise_header;
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

    fn random_song() -> impl Strategy<Value = Song> {
        proptest::collection::vec(command_bytes(), 2..40).prop_map(|commands| {
            Song::vgm(
                "t.vgm".to_owned(),
                0x151,
                VgmData::new(commands.concat()).unwrap(),
                OplType::DualOpl2,
                VgmMeta::new(synthesise_header()),
            )
        })
    }

    fn round_trip(song: &Song) -> Song {
        read_song(&song.name, &write_song(song).expect("writes")).expect("reads")
    }

    /// `(start, end)` inside `len`, from two arbitrary picks.
    fn region(len: usize, a: usize, b: usize) -> (usize, usize) {
        let (lo, hi) = (a % len, b % len);
        if lo <= hi { (lo, hi + 1) } else { (hi, lo + 1) }
    }

    proptest! {
        /// However a song is cropped, the kept region opens on exactly the
        /// register state a fold of the original's earlier writes reaches -- and
        /// that holds through a write/read round trip, so the prelude's opcodes
        /// decode back to the banks they were captured from.
        #[test]
        fn a_cropped_song_opens_on_the_original_state(
            song in random_song(),
            a in 0usize..40,
            b in 0usize..40,
        ) {
            let (start, end) = region(song.len(), a, b);
            let Some(outcome) = crop_to_region(&song, start, end) else {
                return Ok(()); // the whole song: nothing to do
            };
            let mut edited = song.clone();
            outcome.install(&mut edited);

            let expected = state_over(&song, start);
            let reread = round_trip(&edited);
            prop_assert_eq!(state_after_writes(&reread, expected.len()), expected);
            // The kept region's timing is untouched: the prelude adds no delay.
            prop_assert_eq!(
                reread.total_delay_samples(),
                song.samples_before(end) - song.samples_before(start)
            );
        }

        /// However a region is deleted, the surviving tail resumes on exactly the
        /// state the original had reached where it was cut back in, and the song
        /// still ends in the state it always did.
        #[test]
        fn a_deleted_region_leaves_the_tail_on_the_state_it_expects(
            song in random_song(),
            a in 0usize..40,
            b in 0usize..40,
        ) {
            let (start, end) = region(song.len(), a, b);
            prop_assume!(end < song.len()); // a deleted tail has nothing to patch
            let outcome = delete_region(&song, start, end).expect("a real region");
            let patch_len = outcome.patch_len;
            let mut edited = song.clone();
            outcome.install(&mut edited);

            let reread = round_trip(&edited);
            prop_assert_eq!(state_over(&reread, start + patch_len), state_over(&song, end));
            prop_assert_eq!(
                state_over(&reread, reread.len()),
                state_over(&song, song.len())
            );
            // Only the cut region's delays are gone.
            prop_assert_eq!(
                reread.total_delay_samples(),
                song.total_delay_samples()
                    - (song.samples_before(end) - song.samples_before(start))
            );
        }
    }
}
