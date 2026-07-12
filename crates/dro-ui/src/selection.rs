//! The instruction table's selection model.
//!
//! wx gave the Python multi-selection for free; egui's table has none, so the
//! semantics are re-implemented here, headless and tested: plain click
//! replaces, Ctrl+click toggles, Shift+click ranges from the anchor, arrows
//! move (Shift+arrows extend), and deleting selects the row that slid into the
//! first deleted slot (`wxapp.button_delete`).

use std::collections::BTreeSet;

/// Which modifier keys accompanied a click or arrow key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClickModifiers {
    /// Ctrl (or Cmd on macOS): toggle the clicked row.
    pub toggle: bool,
    /// Shift: select the range from the anchor to the clicked row.
    pub extend: bool,
}

/// A multi-row selection over `0..len` rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    selected: BTreeSet<usize>,
    /// Where a Shift-range extends from: the last plainly-clicked row.
    anchor: Option<usize>,
    /// The row the keyboard acts from (wx's focused item).
    focus: Option<usize>,
}

impl Selection {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.focus = None;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    #[must_use]
    pub fn contains(&self, row: usize) -> bool {
        self.selected.contains(&row)
    }

    /// The selected rows, ascending (Python `get_all_selected`).
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    /// The lowest selected row (Python `GetFirstSelected`), or `None`.
    #[must_use]
    pub fn first(&self) -> Option<usize> {
        self.selected.first().copied()
    }

    /// The highest selected row (Python `get_last_selected`), or `None`.
    #[must_use]
    pub fn last(&self) -> Option<usize> {
        self.selected.last().copied()
    }

    /// Selects exactly `row` (Python's `deselect()` + `select_item_manual`).
    pub fn select_only(&mut self, row: usize) {
        self.selected.clear();
        self.selected.insert(row);
        self.anchor = Some(row);
        self.focus = Some(row);
    }

    /// Applies a mouse click on `row`.
    pub fn click(&mut self, row: usize, modifiers: ClickModifiers) {
        if modifiers.extend {
            let anchor = self.anchor.unwrap_or(row);
            self.selected = range_set(anchor, row);
            self.focus = Some(row);
            // The anchor survives, so a further Shift+click re-ranges from it.
        } else if modifiers.toggle {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
            self.anchor = Some(row);
            self.focus = Some(row);
        } else {
            self.select_only(row);
        }
    }

    /// Moves the focused row by `delta` (arrow keys), clamped to `0..len`.
    /// With `extend`, ranges from the anchor instead of replacing.
    ///
    /// Returns the row moved to, so the table can scroll it into view.
    pub fn key_move(&mut self, delta: isize, extend: bool, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let from = self.focus.or_else(|| self.first()).unwrap_or(0);
        let target = from.saturating_add_signed(delta).min(len - 1);
        self.click(
            target,
            ClickModifiers {
                toggle: false,
                extend,
            },
        );
        Some(target)
    }

    /// Drops any rows at or past `len` -- after a redo shrinks the song, the
    /// wx list control could not keep nonexistent rows selected, and neither
    /// can this.
    pub fn truncate_to(&mut self, len: usize) {
        self.selected.retain(|&row| row < len);
        if self.anchor.is_some_and(|row| row >= len) {
            self.anchor = None;
        }
        if self.focus.is_some_and(|row| row >= len) {
            self.focus = None;
        }
    }

    /// Re-selects after a deletion, given the first (lowest) deleted row and
    /// the new row count: that same index if it still exists, else the new last
    /// row, else nothing (the song is empty). Exactly `wxapp.button_delete`'s
    /// rule.
    ///
    /// Returns the newly selected row, for scrolling it into view.
    pub fn after_delete(&mut self, first_deleted: usize, new_len: usize) -> Option<usize> {
        self.clear();
        if new_len == 0 {
            return None;
        }
        let row = first_deleted.min(new_len - 1);
        self.select_only(row);
        Some(row)
    }
}

fn range_set(a: usize, b: usize) -> BTreeSet<usize> {
    (a.min(b)..=a.max(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: ClickModifiers = ClickModifiers {
        toggle: false,
        extend: false,
    };
    const CTRL: ClickModifiers = ClickModifiers {
        toggle: true,
        extend: false,
    };
    const SHIFT: ClickModifiers = ClickModifiers {
        toggle: false,
        extend: true,
    };

    fn rows(selection: &Selection) -> Vec<usize> {
        selection.iter().collect()
    }

    #[test]
    fn a_plain_click_replaces_the_selection() {
        let mut selection = Selection::new();
        selection.click(3, PLAIN);
        selection.click(7, PLAIN);
        assert_eq!(rows(&selection), [7]);
        assert_eq!(selection.first(), Some(7));
        assert_eq!(selection.last(), Some(7));
    }

    #[test]
    fn ctrl_click_toggles_individual_rows() {
        let mut selection = Selection::new();
        selection.click(3, PLAIN);
        selection.click(7, CTRL);
        selection.click(5, CTRL);
        assert_eq!(rows(&selection), [3, 5, 7]);
        selection.click(7, CTRL);
        assert_eq!(rows(&selection), [3, 5]);
    }

    #[test]
    fn shift_click_ranges_from_the_anchor_in_either_direction() {
        let mut selection = Selection::new();
        selection.click(4, PLAIN);
        selection.click(7, SHIFT);
        assert_eq!(rows(&selection), [4, 5, 6, 7]);
        // Re-ranging from the same anchor, upwards.
        selection.click(2, SHIFT);
        assert_eq!(rows(&selection), [2, 3, 4]);
    }

    #[test]
    fn shift_click_with_no_anchor_selects_just_that_row() {
        let mut selection = Selection::new();
        selection.click(5, SHIFT);
        assert_eq!(rows(&selection), [5]);
    }

    #[test]
    fn arrows_move_and_shift_arrows_extend() {
        let mut selection = Selection::new();
        selection.click(5, PLAIN);
        assert_eq!(selection.key_move(1, false, 10), Some(6));
        assert_eq!(rows(&selection), [6]);
        assert_eq!(selection.key_move(1, true, 10), Some(7));
        assert_eq!(selection.key_move(1, true, 10), Some(8));
        assert_eq!(rows(&selection), [6, 7, 8]);
        // Shift ranges from the anchor set by the last plain move.
        assert_eq!(selection.key_move(-3, true, 10), Some(5));
        assert_eq!(rows(&selection), [5, 6]);
    }

    #[test]
    fn arrows_clamp_to_the_table() {
        let mut selection = Selection::new();
        selection.click(0, PLAIN);
        assert_eq!(selection.key_move(-1, false, 10), Some(0));
        selection.click(9, PLAIN);
        assert_eq!(selection.key_move(1, false, 10), Some(9));
        assert_eq!(selection.key_move(1, false, 0), None);
    }

    #[test]
    fn arrows_with_no_selection_start_from_the_top() {
        let mut selection = Selection::new();
        assert_eq!(selection.key_move(1, false, 10), Some(1));
        assert_eq!(rows(&selection), [1]);
    }

    #[test]
    fn truncating_drops_rows_past_the_end() {
        let mut selection = Selection::new();
        selection.click(3, PLAIN);
        selection.click(8, CTRL);
        selection.truncate_to(5);
        assert_eq!(rows(&selection), [3]);
        // A shorter truncation empties it entirely.
        selection.truncate_to(2);
        assert!(selection.is_empty());
        // And the cleared anchor/focus no longer influence the next move.
        assert_eq!(selection.key_move(1, false, 10), Some(1));
    }

    // -- the after-delete rule (wxapp.button_delete) ------------------------

    #[test]
    fn after_delete_selects_the_row_that_slid_into_the_first_deleted_slot() {
        let mut selection = Selection::new();
        selection.click(3, PLAIN);
        selection.click(5, CTRL);
        // 10 rows, deleted {3, 5} -> 8 remain; row 3 still exists.
        assert_eq!(selection.after_delete(3, 8), Some(3));
        assert_eq!(rows(&selection), [3]);
    }

    #[test]
    fn after_deleting_the_tail_the_new_last_row_is_selected() {
        let mut selection = Selection::new();
        // Deleted rows 8 and 9 of 10 -> 8 remain, first deleted index 8 is gone.
        assert_eq!(selection.after_delete(8, 8), Some(7));
        assert_eq!(rows(&selection), [7]);
    }

    #[test]
    fn after_deleting_everything_nothing_is_selected() {
        let mut selection = Selection::new();
        selection.click(0, PLAIN);
        assert_eq!(selection.after_delete(0, 0), None);
        assert!(selection.is_empty());
    }
}
