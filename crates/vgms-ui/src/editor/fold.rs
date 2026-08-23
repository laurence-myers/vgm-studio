//! Folding contiguous runs of same-kind commands in the instruction table.
//!
//! A PCM-heavy VGM is mostly DAC-write-and-wait commands, and a DRO is dotted
//! with delays; a long run of them is noise between the register writes that
//! carry the music. This collapses each run of at least [`MIN_FOLD`] same-kind
//! commands to one summary row that expands on click, so the table shows
//! structure rather than a wall of "wait 1 sample".
//!
//! The map bridges two coordinate spaces: **instruction indices**, which the
//! rest of the editor (selection, find, delete) speaks, and **visible rows**,
//! which the table draws. Folding is a view over the document, never an edit to
//! it -- collapsing a run changes what is shown, not what would be saved.

/// A command category that folds. Contiguous runs of one kind collapse; a
/// register write -- anything not named here -- never folds, so the music stays
/// legible between the runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldKind {
    /// A delay: a VGM wait, or a DRO delay instruction.
    Wait,
    /// A YM2612 DAC write (which carries its own short wait).
    Dac,
}

impl FoldKind {
    /// The noun a summary row uses for a run of this kind, singular/plural by
    /// count.
    fn noun(self, count: usize) -> &'static str {
        match (self, count) {
            (Self::Wait, 1) => "wait",
            (Self::Wait, _) => "waits",
            (Self::Dac, 1) => "DAC write",
            (Self::Dac, _) => "DAC writes",
        }
    }
}

/// The shortest run that folds: shorter runs stay as individual rows, since
/// folding a pair or triple saves nothing worth a summary line.
const MIN_FOLD: usize = 4;

/// One stretch of the instruction list: either plain rows shown one-to-one, or a
/// foldable run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    /// `count` consecutive plain instructions from `first`, each its own row.
    Plain { first: usize, count: usize },
    /// A foldable run of `len` commands of one `kind`, from `start`.
    Fold {
        start: usize,
        len: usize,
        kind: FoldKind,
        expanded: bool,
    },
}

impl Segment {
    /// Instructions this segment covers.
    fn instructions(&self) -> usize {
        match self {
            Self::Plain { count, .. } | Self::Fold { len: count, .. } => *count,
        }
    }

    /// Visible rows this segment contributes: one per plain instruction; a
    /// collapsed fold is a single summary row; an expanded fold is its summary
    /// plus its instructions.
    fn visible(&self) -> usize {
        match self {
            Self::Plain { count, .. } => *count,
            Self::Fold {
                expanded: false, ..
            } => 1,
            Self::Fold {
                expanded: true,
                len,
                ..
            } => 1 + len,
        }
    }
}

/// What a visible row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VisibleRow {
    /// A real instruction at this index.
    Instruction(usize),
    /// A fold's summary row.
    Summary(FoldSummary),
}

/// The summary row of a fold: enough to draw it and to toggle it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoldSummary {
    /// The segment this summary belongs to, for [`FoldMap::toggle`].
    segment: usize,
    /// The first instruction the run covers, for the position column.
    pub(crate) start: usize,
    /// How many commands the run folds.
    pub(crate) len: usize,
    kind: FoldKind,
    /// Whether the run is currently expanded.
    pub(crate) expanded: bool,
}

impl FoldSummary {
    /// The description a summary row shows, e.g. `"42 x DAC writes"`.
    pub(crate) fn label(&self) -> String {
        format!("{} \u{00D7} {}", self.len, self.kind.noun(self.len))
    }
}

/// The folding view over a document's instruction list.
#[derive(Debug, Clone, Default)]
pub(crate) struct FoldMap {
    segments: Vec<Segment>,
    /// Cumulative visible rows before each segment; `visible_prefix[segments.len()]`
    /// is [`Self::visible_len`].
    visible_prefix: Vec<usize>,
    /// Cumulative instructions before each segment, for the instruction -> row
    /// lookup.
    instr_prefix: Vec<usize>,
    /// The instruction count the segments were built for, so the editor can tell
    /// when a document change has left them stale.
    built_for: Option<usize>,
}

impl FoldMap {
    /// Builds the map from the fold kind of each of `len` instructions.
    ///
    /// `kind_of(i)` is the foldable category of instruction `i`, or `None` for a
    /// row that never folds. Runs of one kind at least [`MIN_FOLD`] long become
    /// folds (collapsed); everything else is plain.
    pub(crate) fn build(len: usize, kind_of: impl Fn(usize) -> Option<FoldKind>) -> Self {
        let mut segments: Vec<Segment> = Vec::new();
        let mut i = 0;
        while i < len {
            if let Some(kind) = kind_of(i) {
                let start = i;
                i += 1;
                while i < len && kind_of(i) == Some(kind) {
                    i += 1;
                }
                let run = i - start;
                if run >= MIN_FOLD {
                    segments.push(Segment::Fold {
                        start,
                        len: run,
                        kind,
                        expanded: false,
                    });
                    continue;
                }
                push_plain(&mut segments, start, run);
            } else {
                push_plain(&mut segments, i, 1);
                i += 1;
            }
        }
        let mut map = Self {
            segments,
            visible_prefix: Vec::new(),
            instr_prefix: Vec::new(),
            built_for: Some(len),
        };
        map.rebuild_prefixes();
        map
    }

    /// Whether the map still describes a document of `len` instructions.
    pub(crate) fn is_current_for(&self, len: usize) -> bool {
        self.built_for == Some(len)
    }

    fn rebuild_prefixes(&mut self) {
        self.visible_prefix.clear();
        self.instr_prefix.clear();
        self.visible_prefix.push(0);
        self.instr_prefix.push(0);
        let (mut vis, mut instr) = (0, 0);
        for seg in &self.segments {
            vis += seg.visible();
            instr += seg.instructions();
            self.visible_prefix.push(vis);
            self.instr_prefix.push(instr);
        }
    }

    /// How many rows the table draws.
    pub(crate) fn visible_len(&self) -> usize {
        self.visible_prefix.last().copied().unwrap_or(0)
    }

    /// What visible row `visible` shows, or `None` if it is past the end.
    pub(crate) fn row_at(&self, visible: usize) -> Option<VisibleRow> {
        if visible >= self.visible_len() {
            return None;
        }
        let seg_index = segment_for(&self.visible_prefix, visible);
        let offset = visible - self.visible_prefix[seg_index];
        Some(match self.segments[seg_index] {
            Segment::Plain { first, .. } => VisibleRow::Instruction(first + offset),
            Segment::Fold {
                start,
                len,
                kind,
                expanded,
            } => {
                if offset == 0 {
                    VisibleRow::Summary(FoldSummary {
                        segment: seg_index,
                        start,
                        len,
                        kind,
                        expanded,
                    })
                } else {
                    // Expanded: the rows after the summary are the run itself.
                    VisibleRow::Instruction(start + offset - 1)
                }
            }
        })
    }

    /// The visible row that shows instruction `index`: its own row when plain or
    /// in an expanded fold, else the summary of the collapsed fold hiding it.
    pub(crate) fn visible_of(&self, index: usize) -> usize {
        if self.segments.is_empty() {
            return index;
        }
        let seg_index = segment_for(&self.instr_prefix, index);
        let offset = index - self.instr_prefix[seg_index];
        let visible_start = self.visible_prefix[seg_index];
        match self.segments[seg_index] {
            Segment::Plain { .. } => visible_start + offset,
            Segment::Fold {
                expanded: false, ..
            } => visible_start,
            Segment::Fold { expanded: true, .. } => visible_start + 1 + offset,
        }
    }

    /// Toggles the fold whose summary is at visible row `visible`. A no-op on any
    /// other row.
    pub(crate) fn toggle(&mut self, visible: usize) {
        if let Some(VisibleRow::Summary(summary)) = self.row_at(visible)
            && let Some(Segment::Fold { expanded, .. }) = self.segments.get_mut(summary.segment)
        {
            *expanded = !*expanded;
            self.rebuild_prefixes();
        }
    }

    /// Expands the fold hiding instruction `index`, so a jump to it (a search
    /// hit, an arrow key) lands on a row the user can see. A no-op when the
    /// instruction is already visible.
    pub(crate) fn reveal(&mut self, index: usize) {
        if self.segments.is_empty() {
            return;
        }
        let seg_index = segment_for(&self.instr_prefix, index);
        if let Some(Segment::Fold { expanded, .. }) = self.segments.get_mut(seg_index)
            && !*expanded
        {
            *expanded = true;
            self.rebuild_prefixes();
        }
    }
}

/// Appends `count` plain instructions from `first`, coalescing with a plain
/// segment they abut so runs shorter than [`MIN_FOLD`] merge into their
/// neighbours rather than littering the list.
fn push_plain(segments: &mut Vec<Segment>, first: usize, count: usize) {
    if count == 0 {
        return;
    }
    if let Some(Segment::Plain {
        first: prev_first,
        count: prev_count,
    }) = segments.last_mut()
        && *prev_first + *prev_count == first
    {
        *prev_count += count;
        return;
    }
    segments.push(Segment::Plain { first, count });
}

/// The segment index whose range contains `pos`, given the cumulative prefix
/// (`prefix[s] <= pos < prefix[s + 1]`). `prefix` is strictly increasing, so a
/// hit is the exact segment start and a miss is the segment just below.
fn segment_for(prefix: &[usize], pos: usize) -> usize {
    match prefix.binary_search(&pos) {
        Ok(exact) => exact,
        Err(after) => after - 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kind pattern from a compact spec: `w`=wait, `d`=DAC, `.`=plain.
    fn map_of(spec: &str) -> FoldMap {
        let kinds: Vec<Option<FoldKind>> = spec
            .chars()
            .map(|c| match c {
                'w' => Some(FoldKind::Wait),
                'd' => Some(FoldKind::Dac),
                _ => None,
            })
            .collect();
        FoldMap::build(kinds.len(), |i| kinds[i])
    }

    /// Every visible row round-trips: the instruction a row shows maps back to
    /// that same row (for plain rows and expanded fold rows), and a collapsed
    /// fold's instructions all map to its one summary row.
    fn assert_consistent(map: &FoldMap, len: usize) {
        // Visible rows are dense and each instruction has a home row.
        for index in 0..len {
            let visible = map.visible_of(index);
            assert!(visible < map.visible_len(), "index {index} off the end");
        }
        // Each visible row resolves, and instruction rows are strictly ascending.
        let mut last_instr = None;
        for visible in 0..map.visible_len() {
            match map.row_at(visible).expect("row in range") {
                VisibleRow::Instruction(index) => {
                    if let Some(prev) = last_instr {
                        assert!(index > prev, "instructions ascend");
                    }
                    last_instr = Some(index);
                    assert_eq!(
                        map.visible_of(index),
                        visible,
                        "instruction row round-trips"
                    );
                }
                VisibleRow::Summary(_) => {}
            }
        }
    }

    #[test]
    fn a_short_run_does_not_fold() {
        // Three waits is under MIN_FOLD, so every row stays.
        let map = map_of("..www..");
        assert_eq!(map.visible_len(), 7);
        for visible in 0..7 {
            assert!(matches!(
                map.row_at(visible),
                Some(VisibleRow::Instruction(_))
            ));
        }
    }

    #[test]
    fn a_long_run_folds_to_one_summary_row() {
        // Two plain, six DAC writes, two plain -> 2 + 1 (summary) + 2 = 5 rows.
        let map = map_of("..dddddd..");
        assert_eq!(map.visible_len(), 5);
        assert_eq!(map.row_at(0), Some(VisibleRow::Instruction(0)));
        assert_eq!(map.row_at(1), Some(VisibleRow::Instruction(1)));
        let Some(VisibleRow::Summary(summary)) = map.row_at(2) else {
            panic!("row 2 is the fold summary");
        };
        assert_eq!((summary.start, summary.len), (2, 6));
        assert_eq!(summary.label(), "6 \u{00D7} DAC writes");
        assert!(!summary.expanded);
        // The plain rows after the collapsed run follow the summary.
        assert_eq!(map.row_at(3), Some(VisibleRow::Instruction(8)));
        assert_eq!(map.row_at(4), Some(VisibleRow::Instruction(9)));
        // Every instruction inside the run maps to the one summary row.
        for index in 2..8 {
            assert_eq!(map.visible_of(index), 2);
        }
        assert_consistent(&map, 10);
    }

    #[test]
    fn expanding_reveals_the_run_under_its_summary() {
        let mut map = map_of("dddd..");
        assert_eq!(map.visible_len(), 3, "collapsed: summary + two plain");
        map.toggle(0);
        // Expanded: summary + four rows + two plain.
        assert_eq!(map.visible_len(), 7);
        assert!(matches!(map.row_at(0), Some(VisibleRow::Summary(s)) if s.expanded));
        for (visible, index) in (1..=4).zip(0..4) {
            assert_eq!(map.row_at(visible), Some(VisibleRow::Instruction(index)));
        }
        assert_eq!(map.row_at(5), Some(VisibleRow::Instruction(4)));
        assert_consistent(&map, 6);
        // Toggling again collapses it back.
        map.toggle(0);
        assert_eq!(map.visible_len(), 3);
    }

    #[test]
    fn different_kinds_break_a_run() {
        // Waits then DAC writes are two runs, not one folded block.
        let map = map_of("wwwwdddd");
        assert_eq!(map.visible_len(), 2, "two summary rows");
        let Some(VisibleRow::Summary(first)) = map.row_at(0) else {
            panic!("wait summary");
        };
        let Some(VisibleRow::Summary(second)) = map.row_at(1) else {
            panic!("DAC summary");
        };
        assert_eq!(first.kind, FoldKind::Wait);
        assert_eq!(second.kind, FoldKind::Dac);
        assert_eq!(second.start, 4);
    }

    #[test]
    fn reveal_expands_the_hiding_fold() {
        let mut map = map_of("..dddddd..");
        // Instruction 5 is inside the collapsed run -> its row is the summary.
        assert_eq!(map.visible_of(5), 2);
        map.reveal(5);
        // Now it has its own row, one past the summary and the run's start.
        assert_eq!(map.visible_of(5), 2 + 1 + 3);
        assert_eq!(
            map.row_at(map.visible_of(5)),
            Some(VisibleRow::Instruction(5))
        );
    }

    #[test]
    fn built_for_tracks_the_document_length() {
        let map = map_of("....");
        assert!(map.is_current_for(4));
        assert!(!map.is_current_for(5));
    }

    #[test]
    fn an_empty_document_has_no_rows() {
        let map = FoldMap::build(0, |_| None);
        assert_eq!(map.visible_len(), 0);
        assert_eq!(map.row_at(0), None);
    }
}
