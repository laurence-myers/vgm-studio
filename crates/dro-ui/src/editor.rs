//! The headless editor: the loaded song, its undo history, selection and
//! per-row analysis, and every edit operation the UI invokes. No egui in here,
//! so the whole editing workflow is testable without a window.

use std::path::PathBuf;
use std::sync::Arc;

use dro_core::undo::{DeleteInstructions, UpdateHeader};
use dro_core::{
    DroInstruction, FindTarget, OplType, RowAnalysis, Song, SongFileType, UndoController,
    UndoableCommand, convert, io,
};

use crate::analysis::AnalysisCache;
use crate::platform::PickedFile;
use crate::selection::Selection;

/// What loading a DRO found, for the two load-time warning dialogs
/// (`wxapp.__load_file`). Always all-clear for a VGM.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// A bogus leading delay was removed -- the "DRO auto-trimmed" box.
    pub auto_trimmed: bool,
    /// The header length disagrees with the summed delays -- the "DRO timing
    /// mismatch" box. Checked *after* the auto-trim.
    pub delay_mismatch: bool,
}

#[derive(Debug, Default)]
pub struct Editor {
    song: Option<Song>,
    /// Where the song was loaded from or last saved to. `None` on the web, and
    /// after Convert to VGM -- the converted song has no file yet, so Save
    /// falls through to Save As rather than writing VGM bytes over the
    /// original `.dro` (which is what the Python did).
    pub path: Option<PathBuf>,
    undo: UndoController<Song>,
    pub selection: Selection,
    analysis: AnalysisCache,
    /// Bumped on every change to the song. Consumers (the waveform render, the
    /// audio snapshot) compare it to decide staleness.
    revision: u64,
}

impl Editor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn song(&self) -> Option<&Song> {
        self.song.as_ref()
    }

    #[must_use]
    pub fn has_song(&self) -> bool {
        self.song.is_some()
    }

    /// The number of instructions, `0` with no song.
    #[must_use]
    pub fn len(&self) -> usize {
        self.song.as_ref().map_or(0, Song::len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// An immutable snapshot of the current song, for the audio output and
    /// background tasks. A full clone: snapshots must not alias the editable
    /// song (Python instead shared it under a lock, which blocked edits).
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<Song>> {
        self.song.clone().map(Arc::new)
    }

    // -- loading and saving --------------------------------------------------

    /// Parses and installs `file`, replacing any current song and wiping the
    /// undo history (which makes the auto-trim non-undoable, as in Python).
    ///
    /// # Errors
    /// The parse error's message, for the "Failed to load file" alert. The
    /// current song is left untouched on failure.
    pub fn load(&mut self, file: PickedFile) -> Result<LoadReport, String> {
        let mut song = io::read_song(&file.name, &file.bytes).map_err(|e| e.to_string())?;

        let mut report = LoadReport::default();
        if song.file_type == SongFileType::Dro {
            if song.instruction(0).is_some_and(DroInstruction::is_delay) {
                // Applied directly rather than through the controller: the
                // Python routed it through the controller and then immediately
                // reset the history, so it was never undoable there either.
                DeleteInstructions::new([0]).apply(&mut song);
                report.auto_trimmed = true;
            }
            report.delay_mismatch = song.ms_length != song.total_delay_ms();
        }

        self.song = Some(song);
        self.path = file.path;
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        Ok(report)
    }

    /// The current song serialised in its own format, for saving.
    ///
    /// # Errors
    /// If no song is loaded, or its data and declared format disagree.
    pub fn save_bytes(&self) -> Result<Vec<u8>, String> {
        let song = self.song.as_ref().ok_or("no song is loaded")?;
        io::write_song(song).map_err(|e| e.to_string())
    }

    /// Records where a save landed: the song takes the saved name, and the
    /// path (when the platform has one) becomes the target for the next Save.
    ///
    /// Returns `true` when the new name flips a VGM between `.vgm` and `.vgz`
    /// -- the serialised bytes predate the rename, so the caller must re-save
    /// to get the compression the name promises.
    pub fn record_saved(&mut self, name: String, path: Option<PathBuf>) -> bool {
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        let was_vgz = song.name.to_ascii_lowercase().ends_with(".vgz");
        let is_vgz = name.to_ascii_lowercase().ends_with(".vgz");
        song.name = name;
        if path.is_some() {
            self.path = path;
        }
        song.is_vgm() && was_vgz != is_vgz
    }

    // -- editing -------------------------------------------------------------

    /// Deletes the selected instructions, then selects the row that slid into
    /// the first deleted slot. Returns whether anything was deleted.
    pub fn delete_selection(&mut self) -> bool {
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        if self.selection.is_empty() {
            return false;
        }
        let first_deleted = self
            .selection
            .first()
            .expect("the selection was just checked non-empty");
        let command = DeleteInstructions::new(self.selection.iter());
        self.undo.execute(Box::new(command), song);

        self.selection.after_delete(first_deleted, song.len());
        self.analysis.invalidate();
        self.revision += 1;
        true
    }

    /// Reverts the last edit, returning its description for the status bar,
    /// or `None` when there is nothing to undo. Selection is left alone, as
    /// in Python (its indices may now point at different rows), except that
    /// rows past the new end are dropped -- wx could not keep nonexistent
    /// rows selected either.
    pub fn undo(&mut self) -> Option<String> {
        let song = self.song.as_mut()?;
        let description = self.undo.undo(song)?.to_owned();
        self.selection.truncate_to(song.len());
        self.analysis.invalidate();
        self.revision += 1;
        Some(description)
    }

    /// Re-applies the last undone edit.
    pub fn redo(&mut self) -> Option<String> {
        let song = self.song.as_mut()?;
        let description = self.undo.redo(song)?.to_owned();
        self.selection.truncate_to(song.len());
        self.analysis.invalidate();
        self.revision += 1;
        Some(description)
    }

    /// Applies the DRO Info dialog's header edit, undoably.
    pub fn update_header(&mut self, opl_type: OplType, ms_length: u32) {
        let Some(song) = self.song.as_mut() else {
            return;
        };
        self.undo
            .execute(Box::new(UpdateHeader::new(opl_type, ms_length)), song);
        // The waveform and the audio snapshot re-key on the revision; the
        // Python likewise re-rendered after a header edit.
        self.revision += 1;
    }

    /// Replaces the DRO song with its VGM conversion. Not undoable: the
    /// history is wiped, as in Python.
    ///
    /// # Errors
    /// If no song is loaded, or it is already a VGM.
    pub fn convert_to_vgm(&mut self) -> Result<(), String> {
        let song = self.song.as_ref().ok_or("no song is loaded")?;
        let converted = convert::dro_to_vgm(song).map_err(|e| e.to_string())?;
        self.song = Some(converted);
        self.path = None;
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        Ok(())
    }

    /// Applies the GD3 tag editor's Save. Not undoable, matching the Python's
    /// `on_tag_update`. Ignored unless the song is a VGM.
    pub fn set_gd3_tag(&mut self, tag: dro_core::Gd3Tag) {
        if let Some(meta) = self.song.as_mut().and_then(Song::vgm_meta_mut) {
            meta.tag = Some(tag);
        }
    }

    /// Applies the VGM metadata dialog's Save. Not undoable, as in Python.
    ///
    /// An out-of-range loop point is dropped rather than stored: the dialog
    /// validated against the song it captured, which edits behind its
    /// modeless window may since have shortened -- and the VGM writer panics
    /// on a loop point past the end.
    /// Applies the edited VGM header fields. Returns `true` if the loop point
    /// was out of range for the *current* (possibly shortened since the dialog
    /// opened) song and had to be dropped, so the caller can surface it instead
    /// of losing it silently.
    pub fn set_vgm_metadata(
        &mut self,
        loop_point: Option<usize>,
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
    ) -> bool {
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        let len = song.len();
        let Some(meta) = song.vgm_meta_mut() else {
            return false;
        };
        let clamped = loop_point.filter(|&index| index < len);
        let dropped = clamped != loop_point;
        meta.loop_point = clamped;
        meta.loop_base = loop_base;
        meta.loop_modifier = loop_modifier;
        meta.volume_modifier = volume_modifier;
        dropped
    }

    // -- queries -------------------------------------------------------------

    /// Find Register / delay navigation: the next match strictly after (or
    /// before) the highest selected row, starting from the top when nothing is
    /// selected. (Python `button_find_reg` used the same start for both
    /// directions.)
    #[must_use]
    pub fn find_next(&self, target: FindTarget, look_backwards: bool) -> Option<usize> {
        let song = self.song.as_ref()?;
        let start = self.selection.last().unwrap_or(0);
        song.find_next_instruction(start, target, look_backwards)
    }

    /// The Bank and Description columns for one table row.
    pub fn row_analysis(&mut self, index: usize) -> Option<RowAnalysis> {
        let song = self.song.as_ref()?;
        self.analysis.row(song, index)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    #[must_use]
    pub fn undo_description(&self) -> Option<&str> {
        self.undo.undo_description()
    }

    #[must_use]
    pub fn redo_description(&self) -> Option<&str> {
        self.undo.redo_description()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::ClickModifiers;
    use crate::test_song::{bogus_leading_delay_song, dro_song_v2, tone_song};

    fn picked(song: &Song) -> PickedFile {
        PickedFile {
            name: song.name.clone(),
            path: Some(PathBuf::from(format!("C:/songs/{}", song.name))),
            bytes: io::write_song(song).unwrap(),
        }
    }

    fn loaded(song: &Song) -> (Editor, LoadReport) {
        let mut editor = Editor::new();
        let report = editor.load(picked(song)).unwrap();
        (editor, report)
    }

    #[test]
    fn loading_a_clean_dro_reports_nothing() {
        let (editor, report) = loaded(&dro_song_v2());
        assert_eq!(report, LoadReport::default());
        assert_eq!(editor.len(), 14);
        assert!(editor.path.is_some());
        assert!(!editor.can_undo());
    }

    #[test]
    fn a_bogus_leading_delay_is_trimmed_and_both_warnings_fire() {
        let (editor, report) = loaded(&bogus_leading_delay_song());
        assert!(report.auto_trimmed);
        assert!(report.delay_mismatch, "999 in the header, 200 measured");
        // The delay is gone, and the trim is not undoable.
        let song = editor.song().unwrap();
        assert_eq!(song.len(), 2);
        assert!(!song.instruction(0).unwrap().is_delay());
        assert!(!editor.can_undo());
    }

    #[test]
    fn the_mismatch_check_runs_after_the_trim() {
        // A header honest about the full 300 ms: deleting the 100 ms leading
        // delay also subtracts it from the header, so post-trim they agree --
        // the mismatch check must run *after* the trim to see that.
        let mut source = bogus_leading_delay_song();
        source.ms_length = 300;
        let (editor, report) = loaded(&source);
        assert!(report.auto_trimmed);
        assert!(!report.delay_mismatch);
        assert_eq!(editor.song().unwrap().ms_length, 200);
    }

    #[test]
    fn vgm_songs_are_never_auto_trimmed() {
        // A VGM opening on a sample delay must load untouched (the DRO-only
        // gate the plan calls out). Convert the bogus-delay song directly, so
        // its leading delay survives into the VGM.
        let vgm = convert::dro_to_vgm(&bogus_leading_delay_song()).unwrap();
        assert!(vgm.instruction(0).unwrap().is_delay());

        let (editor, report) = loaded(&vgm);
        assert_eq!(report, LoadReport::default());
        assert_eq!(editor.len(), vgm.len());
        assert!(editor.song().unwrap().instruction(0).unwrap().is_delay());
    }

    #[test]
    fn a_failed_load_keeps_the_current_song() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let error = editor
            .load(PickedFile {
                name: "junk.dro".to_owned(),
                path: None,
                bytes: vec![0; 4],
            })
            .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(editor.len(), 14, "the old song survives a failed load");
        assert!(editor.path.is_some());
    }

    #[test]
    fn deleting_the_selection_reselects_and_bumps_the_revision() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let before = editor.revision();
        editor.selection.click(1, ClickModifiers::default());
        editor.selection.click(
            3,
            ClickModifiers {
                toggle: true,
                extend: false,
            },
        );

        assert!(editor.delete_selection());
        assert_eq!(editor.len(), 12);
        assert_eq!(editor.selection.iter().collect::<Vec<_>>(), [1]);
        assert!(editor.revision() > before);
        assert!(editor.can_undo());
        assert_eq!(editor.undo_description(), Some("Delete Instruction(s)"));
    }

    #[test]
    fn deleting_with_no_selection_does_nothing() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let before = editor.revision();
        assert!(!editor.delete_selection());
        assert_eq!(editor.revision(), before);
    }

    #[test]
    fn undo_and_redo_round_trip_with_descriptions() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let original = editor.song().unwrap().clone();
        editor.selection.select_only(0);
        editor.delete_selection();

        assert_eq!(editor.undo(), Some("Delete Instruction(s)".to_owned()));
        assert_eq!(editor.song().unwrap(), &original);
        assert_eq!(editor.redo(), Some("Delete Instruction(s)".to_owned()));
        assert_eq!(editor.len(), 13);
        assert_eq!(editor.redo(), None);
    }

    #[test]
    fn header_edits_are_undoable() {
        let (mut editor, _) = loaded(&dro_song_v2());
        editor.update_header(OplType::Opl2, 42);
        let song = editor.song().unwrap();
        assert_eq!(song.opl_type, OplType::Opl2);
        assert_eq!(song.ms_length, 42);

        // The Python's UpdateHeaderCommand failed to restore ms_length on
        // undo (it captured the new value); this pins the fix.
        assert_eq!(editor.undo(), Some("DRO Header Changes".to_owned()));
        let song = editor.song().unwrap();
        assert_eq!(song.opl_type, OplType::Opl3);
        assert_eq!(song.ms_length, 99_170);
    }

    #[test]
    fn convert_to_vgm_replaces_the_song_and_clears_the_path() {
        let (mut editor, _) = loaded(&tone_song());
        editor.selection.select_only(2);
        editor.convert_to_vgm().unwrap();

        let song = editor.song().unwrap();
        assert!(song.is_vgm());
        assert!(song.name.ends_with(".vgm"));
        // Divergence from Python: Save no longer writes VGM bytes over the
        // original .dro path -- the converted song has no path until Save As.
        assert!(editor.path.is_none());
        assert!(editor.selection.is_empty());
        assert!(!editor.can_undo());

        assert!(editor.convert_to_vgm().is_err(), "already a VGM");
    }

    #[test]
    fn find_next_starts_from_the_highest_selected_row() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let target: FindTarget = "0x50".parse().unwrap();
        assert_eq!(editor.find_next(target, false), Some(2));

        editor.selection.select_only(2);
        assert_eq!(editor.find_next(target, false), Some(9));
        assert_eq!(editor.find_next(target, true), None, "nothing before 2");
    }

    #[test]
    fn redo_drops_selected_rows_past_the_new_end() {
        let (mut editor, _) = loaded(&dro_song_v2());
        // Delete the last four rows, undo, select the (restored) last row,
        // then redo the delete: the selection would point past the end.
        for row in 10..14 {
            editor.selection.click(
                row,
                ClickModifiers {
                    toggle: true,
                    extend: false,
                },
            );
        }
        editor.delete_selection();
        editor.undo();
        editor.selection.select_only(13);

        editor.redo();
        assert_eq!(editor.len(), 10);
        assert!(
            editor.selection.is_empty(),
            "rows past the end cannot stay selected"
        );
        assert!(!editor.delete_selection(), "so there is nothing to delete");
    }

    #[test]
    fn an_out_of_range_loop_point_is_dropped_not_stored() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        let len = editor.len();

        // The dialog captured a longer song than the one being edited now.
        let dropped = editor.set_vgm_metadata(Some(len + 50), 0, 0, 0);
        assert!(dropped, "the caller is told the loop point was dropped");
        assert_eq!(editor.song().unwrap().vgm_meta().unwrap().loop_point, None);
        // The write path must not panic on what was just stored.
        editor.save_bytes().unwrap();

        // A valid loop point still lands, and is not reported as dropped.
        let dropped = editor.set_vgm_metadata(Some(len - 1), 1, 2, 3);
        assert!(!dropped);
        let meta = editor.song().unwrap().vgm_meta().unwrap();
        assert_eq!(meta.loop_point, Some(len - 1));
        assert_eq!(
            (meta.loop_base, meta.loop_modifier, meta.volume_modifier),
            (1, 2, 3)
        );
        editor.save_bytes().unwrap();
    }

    #[test]
    fn snapshots_do_not_alias_the_editable_song() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let snapshot = editor.snapshot().unwrap();
        editor.selection.select_only(0);
        editor.delete_selection();
        assert_eq!(snapshot.len(), 14, "the snapshot is unaffected by edits");
        assert_eq!(editor.len(), 13);
    }
}
