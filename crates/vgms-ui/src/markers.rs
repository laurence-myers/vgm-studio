//! A marked-out range of instructions: the loop region today, and whatever a
//! crop/trim comes to need tomorrow.
//!
//! Deliberately *not* stored in the `Song`. Dropping a marker is a view onto the
//! song, not an edit of it -- writing it into the file is the explicit
//! apply-to-metadata step -- so this lives beside [`Selection`](crate::selection)
//! in the editor, tracked through edits the same way.
//!
//! The range is half-open: `start` is the first instruction inside it, `end` is
//! one past the last, so `end == len` means "to the end of the song" and matches
//! how `VgmMeta::loop_end` is defined. It is never empty and never inverted --
//! `start < end <= len` holds for every operation below (bar the degenerate empty
//! song, where everything collapses to `0..0`).

use vgms_core::{Song, slide_index_past_deletion};

/// A half-open instruction range, `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RangeMarkers {
    start: usize,
    end: usize,
}

impl RangeMarkers {
    /// The whole song.
    #[must_use]
    pub fn full(len: usize) -> Self {
        Self { start: 0, end: len }
    }

    /// The song's own loop region if it declares one, else the whole song.
    ///
    /// A VGM without a loop, and every DRO, opens marked end to end -- which is
    /// also the region "play looped" would repeat, so the default needs no
    /// special-casing downstream.
    #[must_use]
    pub fn from_song(song: &Song) -> Self {
        let meta = song.vgm_meta();
        Self::from_loop(
            song.len(),
            meta.and_then(|meta| meta.loop_point),
            meta.and_then(|meta| meta.loop_end),
        )
    }

    /// The same, for a document held as a VGM: the loop it stores, or the whole
    /// file when it stores none.
    #[must_use]
    pub fn from_vgm(file: &vgms_core::VgmFile) -> Self {
        Self::from_loop(file.len(), file.loop_index(), file.loop_end_index())
    }

    /// The markers for a stored loop over `len` rows.
    fn from_loop(len: usize, start: Option<usize>, end: Option<usize>) -> Self {
        let mut markers = Self::full(len);
        if let Some(start) = start {
            markers.set_start(start, len);
            if let Some(end) = end {
                markers.set_end(end, len);
            }
        }
        markers
    }

    #[must_use]
    pub fn start(&self) -> usize {
        self.start
    }

    /// One past the last instruction in the range.
    #[must_use]
    pub fn end(&self) -> usize {
        self.end
    }

    /// Whether the range still covers the whole song, i.e. nothing has been
    /// marked out. Drives the "is this worth showing?" decisions in the UI.
    #[must_use]
    pub fn is_full(&self, len: usize) -> bool {
        self.start == 0 && self.end == len
    }

    /// Moves the start to `index`.
    ///
    /// The marker always lands where it was put; it is the *other* marker that
    /// gives way, retreating to the end of the song if this one would cross it.
    /// Deliberate: the click is an explicit intent, the opposite marker is
    /// whatever was left over from before, so the range always ends up
    /// containing what was just marked.
    pub fn set_start(&mut self, index: usize, len: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.start = index.min(len - 1);
        if self.end <= self.start {
            self.end = len;
        }
    }

    /// Moves the end to `index`, exclusive. See [`Self::set_start`] for how a
    /// crossing is resolved: here the start retreats to the beginning.
    pub fn set_end(&mut self, index: usize, len: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.end = index.clamp(1, len);
        if self.start >= self.end {
            self.start = 0;
        }
    }

    /// Slides both markers past a deletion of `sorted` (ascending, unique, in
    /// range), leaving `new_len` instructions.
    ///
    /// Uses the very rule the song's own loop point moves by, so a marked region
    /// and the metadata it was applied to cannot drift apart. A region the
    /// deletion consumed outright falls back to the whole song rather than
    /// linger somewhere arbitrary.
    pub fn after_delete(&mut self, sorted: &[usize], new_len: usize) {
        let start = slide_index_past_deletion(self.start, sorted, new_len);
        let end = slide_index_past_deletion(self.end, sorted, new_len);
        match (start, end) {
            // `end` slid off the tail: the range now runs to the new end.
            (Some(start), None) if start < new_len => {
                *self = Self {
                    start,
                    end: new_len,
                }
            }
            (Some(start), Some(end)) if start < end => *self = Self { start, end },
            _ => *self = Self::full(new_len),
        }
    }

    /// Pulls the range back inside a song of `len` instructions, after an undo or
    /// redo resized it. A range left with nothing to point at reverts to the
    /// whole song.
    pub fn clamp_to(&mut self, len: usize) {
        if len == 0 {
            *self = Self::default();
            return;
        }
        self.end = self.end.min(len).max(1);
        if self.start >= self.end {
            *self = Self::full(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_range_covers_the_whole_song() {
        let markers = RangeMarkers::full(10);
        assert_eq!((markers.start(), markers.end()), (0, 10));
        assert!(markers.is_full(10));
    }

    #[test]
    fn setting_a_marker_keeps_the_range_the_right_way_round() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(3, 10);
        markers.set_end(7, 10);
        assert_eq!((markers.start(), markers.end()), (3, 7));
        assert!(!markers.is_full(10));

        // A start past the end lands where it was put; the end gives way.
        markers.set_start(8, 10);
        assert_eq!((markers.start(), markers.end()), (8, 10));

        // And symmetrically, an end before the start sends the start home.
        markers.set_end(4, 10);
        assert_eq!((markers.start(), markers.end()), (0, 4));
    }

    #[test]
    fn markers_stay_inside_the_song() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(99, 10);
        assert_eq!(markers.start(), 9, "clamped to the last instruction");
        markers.set_end(99, 10);
        assert_eq!(markers.end(), 10);
        markers.set_end(0, 10);
        assert_eq!(markers.end(), 1, "an exclusive end of 0 would be empty");
    }

    #[test]
    fn an_empty_song_collapses_the_range() {
        let mut markers = RangeMarkers::full(0);
        markers.set_start(0, 0);
        assert_eq!((markers.start(), markers.end()), (0, 0));
        markers.set_end(3, 0);
        assert_eq!((markers.start(), markers.end()), (0, 0));
    }

    // -- edit tracking -------------------------------------------------------

    #[test]
    fn deleting_before_the_range_slides_it() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(4, 10);
        markers.set_end(8, 10);
        markers.after_delete(&[0, 1], 8);
        assert_eq!((markers.start(), markers.end()), (2, 6));
    }

    #[test]
    fn deleting_inside_the_range_shrinks_it() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(2, 10);
        markers.set_end(8, 10);
        markers.after_delete(&[4, 5], 8);
        assert_eq!((markers.start(), markers.end()), (2, 6));
    }

    #[test]
    fn deleting_the_tail_pulls_the_end_back_to_the_new_end() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(2, 10);
        markers.set_end(9, 10);
        // Everything from 8 on goes, so the end has nothing left to point at.
        markers.after_delete(&[8, 9], 8);
        assert_eq!((markers.start(), markers.end()), (2, 8));
    }

    #[test]
    fn deleting_the_whole_range_falls_back_to_the_whole_song() {
        let mut markers = RangeMarkers::full(10);
        markers.set_start(2, 10);
        markers.set_end(5, 10);
        markers.after_delete(&[2, 3, 4], 7);
        assert_eq!((markers.start(), markers.end()), (0, 7));
        assert!(markers.is_full(7));
    }

    #[test]
    fn deleting_everything_leaves_an_empty_range() {
        let mut markers = RangeMarkers::full(3);
        markers.after_delete(&[0, 1, 2], 0);
        assert_eq!((markers.start(), markers.end()), (0, 0));
    }

    #[test]
    fn clamping_pulls_a_stale_range_back_inside_the_song() {
        let mut markers = RangeMarkers::full(20);
        markers.set_start(12, 20);
        markers.set_end(18, 20);

        markers.clamp_to(15);
        assert_eq!((markers.start(), markers.end()), (12, 15));

        // Now the start itself is past the end: nothing survives to mark.
        markers.clamp_to(8);
        assert!(markers.is_full(8));
        markers.clamp_to(0);
        assert_eq!((markers.start(), markers.end()), (0, 0));
    }

    // -- reading the song's own loop -----------------------------------------

    #[test]
    fn from_vgm_adopts_a_vgms_loop_region() {
        use crate::test_song::tone_song;
        let mut file = vgms_core::convert::dro_to_vgm(&tone_song()).unwrap();
        let len = file.len();
        file.set_loop_rows(Some(2), Some(len - 1));
        // The markers adopt the file's own loop -- the start verbatim and the end
        // as the command boundary the VGM resolves it to (not necessarily the raw
        // row asked for; that resolution is the file's, pinned in `vgm::file`).
        let markers = RangeMarkers::from_vgm(&file);
        assert_eq!(markers.start(), 2, "the loop start is adopted");
        assert_eq!(
            Some(markers.end()),
            file.loop_end_index(),
            "the loop end is adopted from the file"
        );
    }

    #[test]
    fn markers_fall_back_to_the_whole_song() {
        use crate::test_song::{dro_song_v2, tone_song};
        // A DRO has no loop metadata at all.
        let dro = dro_song_v2();
        assert!(RangeMarkers::from_song(&dro).is_full(dro.len()));

        // A VGM that does not loop opens the same way.
        let file = vgms_core::convert::dro_to_vgm(&tone_song()).unwrap();
        assert!(RangeMarkers::from_vgm(&file).is_full(file.len()));

        // A loop point with no explicit end runs to the end of the song.
        let mut looping = file.clone();
        looping.set_loop_rows(Some(1), None);
        let markers = RangeMarkers::from_vgm(&looping);
        assert_eq!((markers.start(), markers.end()), (1, looping.len()));
    }
}
