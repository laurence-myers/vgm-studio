//! Undo/redo, ported from `dro_undo.py`.
//!
//! The Python commands captured the song in their constructor and mutated it
//! through a lock. Rust's borrow rules make that awkward and unnecessary: a
//! command here is a pure description of an edit, and the target is handed to it
//! by the controller.

use core::fmt;

use crate::song::{InsertEntry, Song};

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
/// The Python tracked a `position` index that was `-1` when nothing had been
/// applied, which every predicate then had to special-case. Counting *applied*
/// commands instead removes the sentinel: `applied` is both the number of
/// commands in effect and the index of the next command to redo.
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
    /// Returns its description, or `None` when there is nothing to undo -- the
    /// Python silently ignored the call in that case, and so does this.
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
/// Indices are sorted and de-duplicated on construction. The Python passed the wx
/// list control's selection straight through, and `DRODataV1.insert_multiple`
/// silently corrupted the song if it ever arrived out of order.
#[derive(Debug, Default)]
pub struct DeleteInstructions {
    indices: Vec<usize>,
    /// The removed instructions, ascending by index. Captured on `apply`.
    deleted: Vec<InsertEntry>,
    /// The total delay removed, subtracted from the song's header length.
    delay_diff: u32,
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
            delay_diff: 0,
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

        let mut delay_diff = 0u32;
        let mut deleted = Vec::with_capacity(self.indices.len());
        for &index in &self.indices {
            let instruction = song
                .instruction(index)
                .expect("index was just bounds-checked");
            delay_diff += instruction.delay_ms();
            let bytes = song
                .data()
                .raw_instruction(index)
                .expect("index was just bounds-checked");
            deleted.push((index, bytes.to_vec().into_boxed_slice()));
        }
        self.delay_diff = delay_diff;
        self.deleted = deleted;

        song.delete_instructions(&self.indices);
        // Python let this go negative; a header length cannot be.
        song.ms_length = song.ms_length.saturating_sub(self.delay_diff);
    }

    fn revert(&mut self, song: &mut Song) {
        song.insert_instructions(&self.deleted);
        song.ms_length = song.ms_length.saturating_add(self.delay_diff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::fixtures::{SONG_LENGTH, dro_song_v1, dro_song_v2};

    /// The Python test's command: it only records that it ran.
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

    /// Port of `test_dro_undo.py::TestDroUndo::test_undo_and_redo`.
    ///
    /// Python's `position` is `applied - 1`, so `position == -1` becomes
    /// `applied == 0`.
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

    /// Port of `test_dro_data.py::TestDeleteInstructionsCommand::test_apply_and_revert`.
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

        // The Python passed [1, 6, 3, 4] -- deliberately out of order.
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
}
