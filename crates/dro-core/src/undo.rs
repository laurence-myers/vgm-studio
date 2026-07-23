//! Undo/redo.
//!
//! A command here is a pure description of an edit, and the target is handed to
//! it by the controller.

use core::fmt;

use crate::song::{InsertEntry, OplType, Song, StreamSnapshot};

/// A reversible edit.
///
/// `apply` and `revert` must be exact inverses: applying, reverting and applying
/// again must leave `target` exactly as a single `apply` would.
pub trait UndoableCommand<T> {
    /// What the Undo/Redo menu items say, e.g. `"Delete Instruction(s)"`.
    fn description(&self) -> &str;
    fn apply(&mut self, target: &mut T);
    fn revert(&mut self, target: &mut T);
}

/// The undo stack.
///
/// Counting *applied* commands avoids a `-1` sentinel: `applied` is both the
/// number of commands in effect and the index of the next command to redo.
pub struct UndoController<T> {
    buffer: Vec<Box<dyn UndoableCommand<T>>>,
    applied: usize,
}

impl<T> UndoController<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            applied: 0,
        }
    }

    /// Drops the whole history, e.g. when a new file is loaded.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.applied = 0;
    }

    /// The number of commands in the buffer, applied or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// The number of commands currently in effect.
    #[must_use]
    pub fn applied(&self) -> usize {
        self.applied
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.applied > 0
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.applied < self.buffer.len()
    }

    /// What the next [`Self::undo`] would undo, for the menu item's label.
    #[must_use]
    pub fn undo_description(&self) -> Option<&str> {
        self.buffer
            .get(self.applied.checked_sub(1)?)
            .map(|command| command.description())
    }

    /// What the next [`Self::redo`] would redo.
    #[must_use]
    pub fn redo_description(&self) -> Option<&str> {
        self.buffer
            .get(self.applied)
            .map(|command| command.description())
    }

    /// Applies `command` and pushes it onto the stack, discarding any redo tail.
    pub fn execute(&mut self, mut command: Box<dyn UndoableCommand<T>>, target: &mut T) {
        command.apply(target);
        // Anything we had undone is now unreachable.
        self.buffer.truncate(self.applied);
        self.buffer.push(command);
        self.applied += 1;
    }

    /// Reverts the most recently applied command, if any.
    ///
    /// Returns its description, or `None` when there is nothing to undo, in which
    /// case the call is silently ignored.
    pub fn undo(&mut self, target: &mut T) -> Option<&str> {
        if !self.can_undo() {
            return None;
        }
        self.applied -= 1;
        let command = &mut self.buffer[self.applied];
        command.revert(target);
        Some(command.description())
    }

    /// Re-applies the most recently undone command, if any.
    pub fn redo(&mut self, target: &mut T) -> Option<&str> {
        if !self.can_redo() {
            return None;
        }
        let command = &mut self.buffer[self.applied];
        command.apply(target);
        self.applied += 1;
        Some(command.description())
    }
}

impl<T> Default for UndoController<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for UndoController<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UndoController")
            .field("applied", &self.applied)
            .field(
                "buffer",
                &self
                    .buffer
                    .iter()
                    .map(|command| command.description())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Deletes a set of instructions, remembering their bytes so undo can restore them.
///
/// Indices are sorted and de-duplicated on construction.
#[derive(Debug, Default)]
pub struct DeleteInstructions {
    indices: Vec<usize>,
    /// The removed instructions, ascending by index. Captured on `apply`.
    deleted: Vec<InsertEntry>,
    /// The song's header length before the delete, restored verbatim on revert.
    previous_ms_length: u32,
    /// A VGM's loop markers before the delete, likewise restored verbatim.
    previous_loop_point: Option<usize>,
    previous_loop_end: Option<usize>,
}

impl DeleteInstructions {
    #[must_use]
    pub fn new(indices: impl IntoIterator<Item = usize>) -> Self {
        let mut indices: Vec<usize> = indices.into_iter().collect();
        indices.sort_unstable();
        indices.dedup();
        Self {
            indices,
            deleted: Vec::new(),
            previous_ms_length: 0,
            previous_loop_point: None,
            previous_loop_end: None,
        }
    }

    /// The instruction indices this command removes, ascending.
    #[must_use]
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }
}

impl UndoableCommand<Song> for DeleteInstructions {
    fn description(&self) -> &str {
        "Delete Instruction(s)"
    }

    fn apply(&mut self, song: &mut Song) {
        self.indices.retain(|&index| index < song.len());
        self.previous_ms_length = song.ms_length;
        self.previous_loop_point = song.vgm_meta().and_then(|meta| meta.loop_point);
        self.previous_loop_end = song.vgm_meta().and_then(|meta| meta.loop_end);

        let mut removed_ms = 0u32;
        let mut deleted = Vec::with_capacity(self.indices.len());
        for &index in &self.indices {
            let instruction = song
                .instruction(index)
                .expect("index was just bounds-checked");
            removed_ms = removed_ms.saturating_add(instruction.delay_ms());
            let bytes = song
                .data()
                .raw_instruction(index)
                .expect("index was just bounds-checked");
            deleted.push((index, bytes.to_vec().into_boxed_slice()));
        }
        self.deleted = deleted;

        song.delete_instructions(&self.indices);

        // A VGM's `ms_length` is derived from its sample delays, and
        // `delete_instructions` has already refreshed it. A DRO's is a header
        // field, adjusted by what we removed.
        if !song.is_vgm() {
            song.ms_length = self.previous_ms_length.saturating_sub(removed_ms);
        }
    }

    fn revert(&mut self, song: &mut Song) {
        song.insert_instructions(&self.deleted);
        // Restore the header verbatim rather than adding the delay back: exact,
        // and it survives a `saturating_sub` that clamped at zero.
        song.ms_length = self.previous_ms_length;
        if let Some(meta) = song.vgm_meta_mut() {
            meta.loop_point = self.previous_loop_point;
            meta.loop_end = self.previous_loop_end;
        }
    }
}

/// Edits the header fields the DRO Info dialog exposes: the OPL type and the
/// declared length.
///
/// The header is captured on `apply` so `revert` can restore `ms_length`
/// exactly. Only meaningful for DRO songs: a VGM's `ms_length` is derived
/// from its sample delays and would be overwritten by the next edit's rebuild.
#[derive(Debug)]
pub struct UpdateHeader {
    new_opl_type: OplType,
    new_ms_length: u32,
    /// The header before `apply`, restored verbatim on `revert`.
    previous: Option<(OplType, u32)>,
}

impl UpdateHeader {
    #[must_use]
    pub fn new(opl_type: OplType, ms_length: u32) -> Self {
        Self {
            new_opl_type: opl_type,
            new_ms_length: ms_length,
            previous: None,
        }
    }
}

impl UndoableCommand<Song> for UpdateHeader {
    fn description(&self) -> &str {
        // The description string, so the status bar says
        // "Undone: DRO Header Changes".
        "DRO Header Changes"
    }

    fn apply(&mut self, song: &mut Song) {
        self.previous = Some((song.opl_type, song.ms_length));
        song.opl_type = self.new_opl_type;
        song.ms_length = self.new_ms_length;
    }

    fn revert(&mut self, song: &mut Song) {
        let (opl_type, ms_length) = self
            .previous
            .expect("the controller only reverts a command it has applied");
        song.opl_type = opl_type;
        song.ms_length = ms_length;
    }
}

/// Swaps in a rebuilt instruction stream, snapshotting the whole of the old one
/// so undo restores it exactly.
///
/// Every edit that rebuilds a stream wholesale uses this: the optimiser
/// ([`optimize`](crate::optimize::optimize)), whose merge pass re-encodes delay
/// runs, and the crop edits ([`crop_to_region`](crate::crop::crop_to_region),
/// [`delete_region`](crate::crop::delete_region)), which splice a state patch in
/// among the survivors and move the loop markers across it. Unlike
/// [`DeleteInstructions`], none of those is a set of removals, and snapshotting
/// both streams is a plainer inverse than an insert and a delete that would have
/// to undo in the right order.
///
/// Streams are small, so the two clones are cheap.
#[derive(Debug)]
pub struct ReplaceStream {
    /// What the Undo menu item says. The edits share this command, so each names
    /// itself rather than the mechanism.
    description: &'static str,
    /// The rebuilt stream and everything derived from it.
    after: StreamSnapshot,
    /// The original, captured on `apply`.
    before: Option<StreamSnapshot>,
}

impl ReplaceStream {
    /// Builds the command from whatever the edit produced -- a
    /// [`CropOutcome`](crate::CropOutcome) or an
    /// [`OptimizeOutcome`](crate::OptimizeOutcome), both of which convert into a
    /// [`StreamSnapshot`]. `description` is what Undo and Redo call it, e.g.
    /// `"Crop to Marked Region"`.
    #[must_use]
    pub fn new(description: &'static str, after: impl Into<StreamSnapshot>) -> Self {
        Self {
            description,
            after: after.into(),
            before: None,
        }
    }
}

impl UndoableCommand<Song> for ReplaceStream {
    fn description(&self) -> &str {
        self.description
    }

    fn apply(&mut self, song: &mut Song) {
        self.before = Some(song.capture_stream());
        song.replace_data(self.after.clone());
    }

    fn revert(&mut self, song: &mut Song) {
        song.replace_data(
            self.before
                .clone()
                .expect("the controller only reverts a command it has applied"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::fixtures::{SONG_LENGTH, dro_song_v1, dro_song_v2};

    /// The test command: it only records that it ran.
    #[derive(Debug, Default)]
    struct Log {
        events: Vec<&'static str>,
    }

    struct Noop;

    impl UndoableCommand<Log> for Noop {
        fn description(&self) -> &str {
            "A test command"
        }
        fn apply(&mut self, log: &mut Log) {
            log.events.push("Do an action");
        }
        fn revert(&mut self, log: &mut Log) {
            log.events.push("Action undone");
        }
    }

    #[test]
    fn undo_and_redo_state_machine() {
        let mut undo = UndoController::new();
        let mut log = Log::default();
        let act = |undo: &mut UndoController<Log>, log: &mut Log| {
            undo.execute(Box::new(Noop), log);
        };

        assert_eq!(undo.len(), 0);
        assert_eq!(undo.applied(), 0);
        assert!(!undo.can_undo());
        assert!(!undo.can_redo());

        act(&mut undo, &mut log);
        assert_eq!(undo.len(), 1);
        assert_eq!(undo.applied(), 1);
        assert!(undo.can_undo());
        assert!(!undo.can_redo());

        act(&mut undo, &mut log);
        assert_eq!(undo.len(), 2);
        assert_eq!(undo.applied(), 2);
        assert!(undo.can_undo());
        assert!(!undo.can_redo());

        undo.undo(&mut log);
        assert_eq!(undo.len(), 2);
        assert_eq!(undo.applied(), 1);
        assert!(undo.can_undo());
        assert!(undo.can_redo());

        // A new command after an undo truncates the redo tail: the buffer stays
        // at 2 entries rather than growing to 3.
        act(&mut undo, &mut log);
        assert_eq!(undo.len(), 2);
        assert_eq!(undo.applied(), 2);
        assert!(undo.can_undo());
        assert!(!undo.can_redo());

        act(&mut undo, &mut log);
        assert_eq!(undo.len(), 3);
        assert_eq!(undo.applied(), 3);
        assert!(undo.can_undo());
        assert!(!undo.can_redo());

        undo.undo(&mut log);
        assert_eq!(undo.len(), 3);
        assert_eq!(undo.applied(), 2);
        assert!(undo.can_undo());
        assert!(undo.can_redo());

        undo.undo(&mut log);
        assert_eq!(undo.len(), 3);
        assert_eq!(undo.applied(), 1);
        assert!(undo.can_undo());
        assert!(undo.can_redo());

        undo.redo(&mut log);
        assert_eq!(undo.len(), 3);
        assert_eq!(undo.applied(), 2);
        assert!(undo.can_undo());
        assert!(undo.can_redo());
    }

    #[test]
    fn undo_and_redo_are_silent_when_there_is_nothing_to_do() {
        let mut undo: UndoController<Log> = UndoController::new();
        let mut log = Log::default();
        assert_eq!(undo.undo(&mut log), None);
        assert_eq!(undo.redo(&mut log), None);
        assert!(log.events.is_empty());
    }

    #[test]
    fn descriptions_are_reported() {
        let mut undo = UndoController::new();
        let mut log = Log::default();
        assert_eq!(undo.undo_description(), None);
        assert_eq!(undo.redo_description(), None);

        undo.execute(Box::new(Noop), &mut log);
        assert_eq!(undo.undo_description(), Some("A test command"));
        assert_eq!(undo.redo_description(), None);
        assert_eq!(undo.undo(&mut log), Some("A test command"));
        assert_eq!(undo.undo_description(), None);
        assert_eq!(undo.redo_description(), Some("A test command"));
        assert_eq!(undo.redo(&mut log), Some("A test command"));

        assert_eq!(
            log.events,
            ["Do an action", "Action undone", "Do an action"]
        );
    }

    #[test]
    fn reset_clears_the_history() {
        let mut undo = UndoController::new();
        let mut log = Log::default();
        undo.execute(Box::new(Noop), &mut log);
        undo.reset();
        assert!(undo.is_empty());
        assert_eq!(undo.applied(), 0);
        assert!(!undo.can_undo());
        assert!(!undo.can_redo());
    }

    // -- DeleteInstructions ------------------------------------------------

    #[test]
    fn delete_apply_and_revert() {
        let mut undo = UndoController::new();
        let mut song = dro_song_v2();

        let expected_1: &[u8] = &[0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let expected_2: &[u8] = &[
            0x00, 0x01, // deleted instruction 1 (0x02, 0x03)
            0x04, 0x05, // deleted instructions 3 and 4 (0x06..0x09)
            0xFE, 0xB0, // deleted instruction 6 (0xFF, 0xC0)
            0x00, 0x01,
        ];
        let expected_3: &[u8] = &[
            0x00, 0x01, // deleted instruction 1 (0x04, 0x05)
            0xFE, 0xB0, 0x00, 0x01, 0x02, 0x03,
        ];
        let head = |song: &Song| song.data().raw()[..8].to_vec();

        assert_eq!(song.len(), 14);
        assert_eq!(head(&song), expected_1);

        // [1, 6, 3, 4] is deliberately out of order.
        undo.execute(Box::new(DeleteInstructions::new([1, 6, 3, 4])), &mut song);
        assert_eq!(song.len(), 14 - 4);
        assert_eq!(head(&song), expected_2);

        undo.execute(Box::new(DeleteInstructions::new([1])), &mut song);
        assert_eq!(song.len(), 14 - 5);
        assert_eq!(head(&song), expected_3);

        undo.undo(&mut song);
        assert_eq!(head(&song), expected_2);
        undo.redo(&mut song);
        assert_eq!(head(&song), expected_3);
        undo.undo(&mut song);
        assert_eq!(head(&song), expected_2);
        undo.undo(&mut song);
        assert_eq!(head(&song), expected_1);
        undo.redo(&mut song);
        assert_eq!(head(&song), expected_2);
        undo.redo(&mut song);
        assert_eq!(head(&song), expected_3);
        undo.undo(&mut song);
        assert_eq!(head(&song), expected_2);
    }

    #[test]
    fn delete_sorts_and_dedups_indices() {
        let command = DeleteInstructions::new([6, 1, 4, 3, 1]);
        assert_eq!(command.indices(), [1, 3, 4, 6]);
    }

    #[test]
    fn deleting_delays_updates_ms_length_and_the_prefix() {
        let mut undo = UndoController::new();
        let mut song = dro_song_v2();
        assert_eq!(song.total_delay_ms(), SONG_LENGTH);

        // Instruction 6 is the first long delay (49408 ms).
        undo.execute(Box::new(DeleteInstructions::new([6])), &mut song);
        assert_eq!(song.ms_length, SONG_LENGTH - 49_408);
        assert_eq!(song.total_delay_ms(), SONG_LENGTH - 49_408);
        assert_eq!(song.len(), 13);

        undo.undo(&mut song);
        assert_eq!(song.ms_length, SONG_LENGTH);
        assert_eq!(song.total_delay_ms(), SONG_LENGTH);
        assert_eq!(song.len(), 14);
    }

    #[test]
    fn undo_restores_the_song_exactly() {
        for original in [dro_song_v1(), dro_song_v2()] {
            for selection in [vec![0], vec![1, 6, 3, 4], vec![0, 1, 2]] {
                let selection: Vec<usize> = selection
                    .into_iter()
                    .filter(|&i| i < original.len())
                    .collect();
                let mut song = original.clone();
                let mut undo = UndoController::new();
                undo.execute(
                    Box::new(DeleteInstructions::new(selection.clone())),
                    &mut song,
                );
                undo.undo(&mut song);
                assert_eq!(song, original, "selection {selection:?}");
            }
        }
    }

    #[test]
    fn deleting_everything_then_undoing_restores_the_song() {
        let original = dro_song_v2();
        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(DeleteInstructions::new(0..original.len())),
            &mut song,
        );
        assert!(song.is_empty());
        assert_eq!(song.ms_length, 0);
        assert_eq!(song.total_delay_ms(), 0);

        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    #[test]
    fn out_of_range_indices_are_ignored() {
        let original = dro_song_v2();
        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(Box::new(DeleteInstructions::new([99, 5, 1000])), &mut song);
        assert_eq!(song.len(), 13);
        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    // -- UpdateHeader --------------------------------------------------------

    #[test]
    fn update_header_applies_and_reverts_exactly() {
        let original = dro_song_v2();
        assert_eq!(original.opl_type, OplType::Opl3);

        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(UpdateHeader::new(OplType::Opl2, 12_345)),
            &mut song,
        );
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.ms_length, 12_345);
        // The instruction stream is untouched: only the header changed.
        assert_eq!(song.data(), original.data());
        assert_eq!(song.total_delay_ms(), original.total_delay_ms());

        assert_eq!(undo.undo(&mut song), Some("DRO Header Changes"));
        assert_eq!(song, original);

        undo.redo(&mut song);
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.ms_length, 12_345);
    }

    #[test]
    fn update_header_interleaves_with_deletes() {
        let original = dro_song_v2();
        let mut song = original.clone();
        let mut undo = UndoController::new();

        undo.execute(
            Box::new(UpdateHeader::new(OplType::DualOpl2, 777)),
            &mut song,
        );
        undo.execute(Box::new(DeleteInstructions::new([0])), &mut song);
        assert_eq!(undo.undo_description(), Some("Delete Instruction(s)"));

        undo.undo(&mut song);
        assert_eq!(undo.undo_description(), Some("DRO Header Changes"));
        assert_eq!(song.ms_length, 777);

        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    #[test]
    fn a_command_after_an_undo_discards_the_redo_tail() {
        let original = dro_song_v2();
        let mut song = original.clone();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteInstructions::new([0])), &mut song);
        undo.execute(Box::new(DeleteInstructions::new([0])), &mut song);
        undo.undo(&mut song);
        undo.undo(&mut song);
        assert_eq!(song, original);

        undo.execute(Box::new(DeleteInstructions::new([5])), &mut song);
        assert_eq!(undo.len(), 1);
        assert!(!undo.can_redo());
        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    // -- ReplaceStream, driven by the optimiser -----------------------------

    /// A VGM with a redundant register write between two delays, so the optimiser
    /// has both a write to strip and a pair of delays to merge.
    fn optimisable_vgm() -> Song {
        use crate::vgm::VgmData;
        use crate::vgm::io::synthesise_header;
        let bytes = vec![
            0x5A, 0x20, 0x01, // write
            0x61, 0x64, 0x00, // wait 100
            0x5A, 0x20, 0x01, // redundant write
            0x61, 0xC8, 0x00, // wait 200
            0x5A, 0x21, 0x02, // write
        ];
        Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            OplType::Opl2,
            crate::vgm::VgmMeta::new(synthesise_header()),
        )
    }

    #[test]
    fn optimize_vgm_applies_and_reverts_exactly() {
        use crate::optimize::optimize;
        let original = optimisable_vgm();
        let outcome = optimize(&original).expect("the fixture has a redundant write");
        let saved = outcome.bytes_saved;
        assert!(saved > 0);

        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(ReplaceStream::new("Optimize VGM", outcome)),
            &mut song,
        );

        // The stream shrank, and the total delay is conserved.
        assert!(song.data().raw().len() < original.data().raw().len());
        assert_eq!(song.total_delay_samples(), original.total_delay_samples());
        assert_eq!(undo.undo_description(), Some("Optimize VGM"));

        // Undo restores the original exactly; redo re-applies.
        undo.undo(&mut song);
        assert_eq!(song, original);
        undo.redo(&mut song);
        assert!(song.data().raw().len() < original.data().raw().len());
        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    #[test]
    fn optimize_vgm_preserves_loop_markers_through_undo() {
        use crate::optimize::optimize;
        let mut original = optimisable_vgm();
        {
            let meta = original.vgm_meta_mut().unwrap();
            meta.loop_point = Some(0); // loop the whole song
        }
        let outcome = optimize(&original).unwrap();
        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(ReplaceStream::new("Optimize VGM", outcome)),
            &mut song,
        );
        // The loop point is still present after the rebuild.
        assert!(song.vgm_meta().unwrap().loop_point.is_some());
        undo.undo(&mut song);
        assert_eq!(song, original);
    }

    // -- ReplaceStream, driven by the crop edits ----------------------------

    /// Every format, so the snapshot is exercised against a derived length (VGM)
    /// and a stored one (DRO), and against a v2's codemap.
    fn every_format() -> Vec<Song> {
        vec![dro_song_v1(), dro_song_v2(), optimisable_vgm()]
    }

    #[test]
    fn replace_stream_applies_and_reverts_every_format_exactly() {
        for original in every_format() {
            let len = original.len();
            // Crop away the first instruction: short enough to be a real edit on
            // every fixture, and it forces a state prelude on most.
            let outcome = crate::crop::crop_to_region(&original, 1, len)
                .expect("a crop that drops the first instruction");
            let cropped_len = outcome.len();

            let mut song = original.clone();
            let mut undo = UndoController::new();
            undo.execute(
                Box::new(ReplaceStream::new("Crop to Marked Region", outcome)),
                &mut song,
            );
            assert_eq!(song.len(), cropped_len, "{}", original.name);
            assert_eq!(undo.undo_description(), Some("Crop to Marked Region"));

            undo.undo(&mut song);
            assert_eq!(
                song, original,
                "undo must restore {} exactly",
                original.name
            );
            undo.redo(&mut song);
            assert_eq!(song.len(), cropped_len);
            undo.undo(&mut song);
            assert_eq!(song, original, "and again after a redo");
        }
    }

    #[test]
    fn replace_stream_restores_a_dros_header_length() {
        // A DRO's `ms_length` is a stored header field, not derived, so it is
        // part of what the snapshot has to carry.
        let original = dro_song_v2();
        let before = original.ms_length;
        let outcome = crate::crop::delete_region(&original, 0, 8).expect("a real cut");

        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(ReplaceStream::new("Delete Marked Region", outcome)),
            &mut song,
        );
        assert!(song.ms_length < before, "the cut shortened the song");

        undo.undo(&mut song);
        assert_eq!(song.ms_length, before);
        assert_eq!(song, original);
    }

    #[test]
    fn replace_stream_restores_loop_markers() {
        let mut original = optimisable_vgm();
        original.vgm_meta_mut().unwrap().loop_point = Some(3);
        let outcome = crate::crop::delete_region(&original, 0, 2).expect("a real cut");

        let mut song = original.clone();
        let mut undo = UndoController::new();
        undo.execute(
            Box::new(ReplaceStream::new("Delete Marked Region", outcome)),
            &mut song,
        );
        // The loop moved with the edit...
        assert_ne!(song.vgm_meta().unwrap().loop_point, Some(3));

        // ...and comes back exactly where it was.
        undo.undo(&mut song);
        assert_eq!(song.vgm_meta().unwrap().loop_point, Some(3));
        assert_eq!(song, original);
    }

    #[test]
    fn replace_stream_interleaves_with_the_other_commands() {
        let original = dro_song_v2();
        let mut song = original.clone();
        let mut undo = UndoController::new();

        undo.execute(Box::new(DeleteInstructions::new([0])), &mut song);
        let outcome = crate::crop::crop_to_region(&song, 1, song.len()).expect("a real crop");
        undo.execute(
            Box::new(ReplaceStream::new("Crop to Marked Region", outcome)),
            &mut song,
        );
        undo.execute(Box::new(UpdateHeader::new(OplType::Opl2, 42)), &mut song);

        assert_eq!(undo.undo_description(), Some("DRO Header Changes"));
        undo.undo(&mut song);
        assert_eq!(undo.undo_description(), Some("Crop to Marked Region"));
        undo.undo(&mut song);
        assert_eq!(undo.undo_description(), Some("Delete Instruction(s)"));
        undo.undo(&mut song);
        assert_eq!(song, original, "the whole stack unwinds to the original");
    }
}
