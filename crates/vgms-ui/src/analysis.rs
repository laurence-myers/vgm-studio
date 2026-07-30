//! A cache over `vgms-core`'s [`RegisterAnalyzer`] replay cursor, sized for a
//! table that repaints its visible rows every frame.
//!
//! The cursor alone is O(1) per row *within* one top-to-bottom paint, but a
//! second paint of the same rows starts by querying a row the cursor has
//! already passed, which resets it and replays from instruction 0 -- every
//! frame. Memoising the produced rows makes the steady state (repainting an
//! unchanged window) free, while keeping memory bounded: the memo is dropped
//! whenever it outgrows [`AnalysisCache::CAPACITY`].

use std::collections::BTreeMap;

use vgms_core::chip_docs::ChipAnalyzer;
use vgms_core::vgm::VgmStream;
use vgms_core::{RegisterAnalyzer, RowAnalysis, Song};

#[derive(Debug, Default)]
pub struct AnalysisCache {
    analyzer: RegisterAnalyzer,
    rows: BTreeMap<usize, RowAnalysis>,
    /// The multichip counterpart of `analyzer`, for a VGM with no OPL reading.
    /// Its rows memoise `Option<String>`: `None` is "undocumented -- the caller
    /// falls back to the generic one-liner", cached so the analyser is not
    /// re-run for it.
    chip_analyzer: ChipAnalyzer,
    chip_rows: BTreeMap<usize, Option<String>>,
}

impl AnalysisCache {
    /// The memo bound. Generous for any plausible scroll session.
    const CAPACITY: usize = 50_000;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards everything. Call after any edit, undo or redo: both memos and
    /// both cursors' replayed chip state are stale.
    pub fn invalidate(&mut self) {
        self.analyzer.reset();
        self.rows.clear();
        self.chip_analyzer.reset();
        self.chip_rows.clear();
    }

    /// The Bank and Description columns for `index`, or `None` out of range.
    pub fn row(&mut self, song: &Song, index: usize) -> Option<RowAnalysis> {
        if let Some(row) = self.rows.get(&index) {
            return Some(row.clone());
        }
        let row = self.analyzer.row(song, index)?;
        if self.rows.len() >= Self::CAPACITY {
            self.rows.clear();
        }
        self.rows.insert(index, row.clone());
        Some(row)
    }

    /// The documented Description for a multichip row, or `None` when the chip
    /// or the command is undocumented (the caller shows the generic one-liner).
    ///
    /// Memoised like [`row`](Self::row); the cursor must be queried in order,
    /// which the memo preserves.
    pub fn chip_row(&mut self, stream: &VgmStream, index: usize) -> Option<String> {
        if let Some(cached) = self.chip_rows.get(&index) {
            return cached.clone();
        }
        let description = self
            .chip_analyzer
            .row(stream, index)
            .map(std::borrow::Cow::into_owned);
        if self.chip_rows.len() >= Self::CAPACITY {
            self.chip_rows.clear();
        }
        self.chip_rows.insert(index, description.clone());
        description
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::dro_song_v2;
    use vgms_core::UndoableCommand;
    use vgms_core::undo::DeleteInstructions;

    #[test]
    fn cached_rows_match_a_full_scan_in_any_query_order() {
        let song = dro_song_v2();
        let reference = RegisterAnalyzer::analyze_all(&song);
        let mut cache = AnalysisCache::new();

        // Scrambled first pass, then a repeat pass served from the memo.
        for _ in 0..2 {
            for index in [5, 0, 13, 7, 1, 12, 6, 2, 3, 4, 8, 9, 10, 11] {
                assert_eq!(cache.row(&song, index), Some(reference[index].clone()));
            }
        }
        assert_eq!(cache.row(&song, 14), None);
    }

    #[test]
    fn invalidate_reflects_an_edit() {
        let mut song = dro_song_v2();
        let mut cache = AnalysisCache::new();
        let before = cache.row(&song, 1).unwrap();

        // Delete instruction 0; row 1's analysis becomes what row 1 now is.
        let mut delete = DeleteInstructions::new([0]);
        delete.apply(&mut song);
        cache.invalidate();

        let reference = RegisterAnalyzer::analyze_all(&song);
        assert_eq!(cache.row(&song, 1), Some(reference[1].clone()));
        // Row 1 now describes what used to be row 2, not the memoised answer.
        assert_ne!(cache.row(&song, 1), Some(before));
    }
}
