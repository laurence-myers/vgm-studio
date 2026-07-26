//! The headless editor: the loaded song, its undo history, selection and
//! per-row analysis, and every edit operation the UI invokes. No egui in here,
//! so the whole editing workflow is testable without a window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dro_core::undo::{DeleteCommands, DeleteInstructions, ReplaceStream, UpdateHeader};
use dro_core::{
    CropOutcome, DroInstruction, FindTarget, OplType, RowAnalysis, Song, SongFileType,
    UndoController, UndoableCommand, VgmCommand, VgmFile, convert, io,
};

use crate::analysis::AnalysisCache;
use crate::markers::RangeMarkers;
use crate::platform::PickedFile;
use crate::selection::Selection;

/// One row of the instruction table, filled from whichever document is open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowCells {
    pub position: String,
    /// The OPL bank, or the chip a command targets in a VGM held as one.
    pub bank: String,
    pub register: String,
    pub value: String,
    pub description: String,
    /// The long "every option this register has" text, shown on hover. Empty
    /// for a row with no OPL register behind it to describe.
    pub hover: String,
}

/// What the loaded document supports, for the panels that only suit some of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocCapabilities {
    /// There is an OPL stream to render: the transport, waveform, position
    /// readout, peak meter and channel controls all mean something.
    pub playable: bool,
    /// There are rows to select and delete.
    pub editable: bool,
}

/// Why a file did not open in the editor.
///
/// The distinction is the whole point: "this is not a song" and "this is a
/// perfectly good VGM whose chips are not OPL" deserve different answers, and
/// only the first is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadFailure {
    /// Not a song this app can read, with the reader's message.
    Unreadable(String),
    /// A readable VGM whose chips are not OPL, so the OPL editing surface
    /// does not apply to it.
    NotOpl {
        /// Boxed: far larger than the other arm, and this is the rare case.
        file: Box<VgmFile>,
        /// The folder it sits in, so the dialog can offer to open that folder
        /// as a pack. `None` on the web, where a picked file has no path.
        folder: Option<PathBuf>,
    },
}

impl LoadFailure {
    /// Decides which kind of failure a rejected file is, by offering it to the
    /// chip-agnostic reader that pack mode uses.
    fn classify(file: &PickedFile, opl_error: &str) -> Self {
        match dro_core::vgm::file::read(&file.name, &file.bytes) {
            Ok(vgm) => Self::NotOpl {
                file: Box::new(vgm),
                folder: file
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf),
            },
            Err(_) => Self::Unreadable(opl_error.to_owned()),
        }
    }
}

/// What loading a DRO found, for the two load-time warning dialogs.
/// Always all-clear for a VGM.
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
    /// A VGM held as a VGM, rather than materialised into [`Song`]'s OPL
    /// model. **At most one of `song` and `vgm` is ever `Some`** -- a load
    /// clears both before installing either.
    ///
    /// The two slots are not two kinds of file. There is one kind; this is
    /// about which representation the editor can act through. `Song` carries
    /// the OPL editing surface -- the analyser, the optimiser, the synth --
    /// and a VGM whose chips are not OPL has nothing to gain from it, so it
    /// lands here where the chip-agnostic operations live. Collapsing the two
    /// is the rest of uv-1; what they already share (the selection, the
    /// markers, the revision and dirty tracking, the path) sits beside them.
    vgm: Option<VgmFile>,
    /// Where the song was loaded from or last saved to. `None` on the web, and
    /// after Convert to VGM -- the converted song has no file yet, so Save
    /// falls through to Save As rather than writing VGM bytes over the
    /// original `.dro`.
    pub path: Option<PathBuf>,
    undo: UndoController<Song>,
    /// The VGM-held document's own history. Separate because the two stacks
    /// hold commands over different targets, and only one document is ever
    /// loaded, so they can never both be live.
    vgm_undo: UndoController<VgmFile>,
    pub selection: Selection,
    /// The marked-out loop region, tracked through edits alongside the
    /// selection. A view onto the song until [`Editor::apply_loop_to_metadata`]
    /// writes it into the file.
    pub markers: RangeMarkers,
    analysis: AnalysisCache,
    /// Bumped on every change to the song. Consumers (the waveform render, the
    /// audio snapshot) compare it to decide staleness.
    revision: u64,
    /// The revision at the last load / convert / successful save. The song is
    /// "clean" while `revision` still equals it. Revision is monotonic, so a
    /// simple equality check suffices (an undo back to the saved point reads
    /// dirty -- a safe over-prompt, never a missed one).
    saved_revision: Option<u64>,
    /// Whether a metadata edit (GD3 tag, VGM loop fields) is unsaved.
    ///
    /// Tracked apart from `revision` because the two answer different questions.
    /// `revision` means "the instruction stream changed", and the audio snapshot
    /// and waveform render key off it; a tag edit changes neither, so bumping it
    /// would reload the stream and re-render the wave for nothing -- and
    /// interrupt playback to do it. This flag carries the "unsaved" half on its
    /// own. Metadata edits are not undoable, so it only ever clears on a save or
    /// a fresh song.
    metadata_dirty: bool,
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

    /// The loaded document as a VGM, if it is one held that way.
    #[must_use]
    pub fn vgm(&self) -> Option<&VgmFile> {
        self.vgm.as_ref()
    }

    /// Whether anything at all is open, of either kind.
    ///
    /// Use this for "is there a document" questions -- the row count, the dirty
    /// prompt, whether Save means anything. [`Self::has_song`] stays the
    /// narrower question: is there a *song*, with instructions to analyse and
    /// audio to render.
    #[must_use]
    pub fn has_document(&self) -> bool {
        self.song.is_some() || self.vgm.is_some()
    }

    /// What the loaded document can do, for the panels that only make sense
    /// against some of it.
    #[must_use]
    pub fn capabilities(&self) -> DocCapabilities {
        DocCapabilities {
            // Playback, the waveform and the position readout need a decoded
            // OPL stream. With *nothing* loaded they still show, greyed -- that
            // is how an empty editor has always looked, and it is where the
            // transport lives. A document held as a VGM is one whose chips are
            // not OPL, so they go rather than sit there dead over a
            // permanently flat waveform.
            playable: self.vgm.is_none(),
            editable: self.has_document(),
        }
    }

    /// The number of rows, `0` with nothing open.
    #[must_use]
    pub fn len(&self) -> usize {
        match (&self.song, &self.vgm) {
            (Some(song), _) => song.len(),
            (None, Some(file)) => file.len(),
            (None, None) => 0,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the song has unsaved changes (drives the discard-changes prompts).
    ///
    /// Covers metadata edits as well as instruction edits, so a GD3 tag or a
    /// loop point cannot be typed in and lost to an Open or a close without a
    /// word -- a loop region is deliberate work, not a stray field edit.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.has_document() && (self.saved_revision != Some(self.revision) || self.metadata_dirty)
    }

    /// An immutable snapshot of the current song, for the audio output and
    /// background tasks. A full clone: snapshots must not alias the editable
    /// song.
    #[must_use]
    pub fn snapshot(&self) -> Option<Arc<Song>> {
        self.song.clone().map(Arc::new)
    }

    // -- loading and saving --------------------------------------------------

    /// Parses and installs `file`, replacing any current song and wiping the
    /// undo history (which makes the auto-trim non-undoable).
    ///
    /// # Errors
    /// [`LoadFailure`], which distinguishes a file this app cannot read at all
    /// from a VGM it can read but not edit. The current song is left untouched
    /// either way.
    pub fn load(&mut self, file: PickedFile) -> Result<LoadReport, LoadFailure> {
        let mut song = match io::read_song(&file.name, &file.bytes) {
            Ok(song) => song,
            Err(error) => return Err(LoadFailure::classify(&file, &error.to_string())),
        };
        self.vgm = None;
        self.vgm_undo.reset();

        let mut report = LoadReport::default();
        if song.file_type == SongFileType::Dro {
            if song.instruction(0).is_some_and(DroInstruction::is_delay) {
                // Applied directly rather than through the controller, so it is
                // never undoable.
                DeleteInstructions::new([0]).apply(&mut song);
                report.auto_trimmed = true;
            }
            report.delay_mismatch = song.ms_length != song.total_delay_ms();
        }

        self.markers = RangeMarkers::from_song(&song);
        self.song = Some(song);
        self.path = file.path;
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
        Ok(report)
    }

    /// Installs a VGM as a VGM, for the chip-agnostic operations.
    ///
    /// The counterpart of [`Self::load`] for the other representation, and the
    /// same teardown: any current document goes, and both undo histories with
    /// it.
    pub fn load_vgm(&mut self, file: VgmFile, path: Option<PathBuf>) {
        self.song = None;
        self.undo.reset();
        self.markers = RangeMarkers::default();
        self.vgm = Some(file);
        self.vgm_undo.reset();
        self.path = path;
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
    }

    /// Drops the document, leaving the editor as it starts: File > Close.
    ///
    /// The undo history goes with it. What it holds are edits to a song that is
    /// no longer here, and an empty editor has nothing to undo *into*.
    pub fn close(&mut self) {
        self.song = None;
        self.vgm = None;
        self.vgm_undo.reset();
        self.path = None;
        self.markers = RangeMarkers::default();
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        // Nothing is loaded, so nothing is unsaved: the dirty flag must not
        // keep prompting about a song that has gone.
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
    }

    /// The current song serialised in its own format, for saving.
    ///
    /// # Errors
    /// If no song is loaded, or its data and declared format disagree.
    pub fn save_bytes(&self) -> Result<Vec<u8>, String> {
        if let Some(file) = self.vgm.as_ref() {
            // Its own writer, never the OPL one: that re-derives the chip
            // clocks from a `Song`'s OPL type, which this file does not have.
            let bytes = if file.name.to_ascii_lowercase().ends_with(".vgz") {
                dro_core::vgm::file::write_gzipped(file)
            } else {
                dro_core::vgm::file::write(file)
            };
            return bytes.map_err(|e| e.to_string());
        }
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
        if let Some(file) = self.vgm.as_mut() {
            let was_vgz = file.name.to_ascii_lowercase().ends_with(".vgz");
            let is_vgz = name.to_ascii_lowercase().ends_with(".vgz");
            file.name = name;
            if path.is_some() {
                self.path = path;
            }
            return was_vgz != is_vgz;
        }
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

    /// Marks the current state as saved, clearing both halves of the dirty flag
    /// (the discard-changes prompts key off [`Self::is_dirty`]).
    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
    }

    // -- editing -------------------------------------------------------------

    /// Deletes the selected instructions, then selects the row that slid into
    /// the first deleted slot. Returns whether anything was deleted.
    pub fn delete_selection(&mut self) -> bool {
        if self.vgm.is_some() {
            return self.delete_vgm_selection();
        }
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
        let deleted: Vec<usize> = self.selection.iter().collect();
        let command = DeleteInstructions::new(deleted.iter().copied());
        self.undo.execute(Box::new(command), song);

        // The same rule the song's own loop point moved by, so a marked region
        // and the metadata it may have been applied to cannot drift apart.
        self.markers.after_delete(&deleted, song.len());
        self.selection.after_delete(first_deleted, song.len());
        self.analysis.invalidate();
        self.revision += 1;
        true
    }

    /// The VGM-held document's half of [`Self::delete_selection`].
    ///
    /// The same shape, over the other target: the marked-region markers are not
    /// carried (that document has no loop region to mark yet), and the analysis
    /// cache is OPL-only, so neither is touched.
    fn delete_vgm_selection(&mut self) -> bool {
        let Some(file) = self.vgm.as_mut() else {
            return false;
        };
        if self.selection.is_empty() {
            return false;
        }
        let first_deleted = self
            .selection
            .first()
            .expect("the selection was just checked non-empty");
        let deleted: Vec<usize> = self.selection.iter().collect();
        self.vgm_undo
            .execute(Box::new(DeleteCommands::new(deleted)), file);
        self.selection.after_delete(first_deleted, file.len());
        self.revision += 1;
        true
    }

    /// Optimises the loaded VGM: strips redundant OPL writes and merges the
    /// delays left behind, undoably. Returns `(commands_removed, bytes_saved)`,
    /// or `None` when no VGM is loaded or there is nothing to optimise.
    ///
    /// The stream is rebuilt wholesale (delay runs re-encode), so the selection
    /// is cleared and the loop markers are re-derived from the song's remapped
    /// loop metadata, exactly as a fresh load or conversion would.
    pub fn optimize_vgm(&mut self) -> Option<(usize, usize)> {
        let song = self.song.as_mut()?;
        let outcome = dro_core::optimize::optimize(song)?;
        let stats = (outcome.commands_removed, outcome.bytes_saved);
        self.undo
            .execute(Box::new(ReplaceStream::new("Optimize VGM", outcome)), song);
        self.markers = RangeMarkers::from_song(song);
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        Some(stats)
    }

    /// Crops the song to the marked region, deleting everything outside it.
    ///
    /// The kept region is prefixed with the writes that recreate the register
    /// state the stream had reached at its start, so it opens on the chip state
    /// it would have had mid-play rather than on a silent chip.
    ///
    /// Returns `(kept, restored)`: how many instructions survive, and how many of
    /// those the state prelude contributed. `None` when there is no song, or the
    /// markers still cover all of it -- there would be nothing to crop away.
    pub fn crop_to_markers(&mut self) -> Option<(usize, usize)> {
        // What survives and what the patch added are exactly what the shared
        // helper reports.
        self.replace_stream("Crop to Marked Region", dro_core::crop::crop_to_region)
    }

    /// Deletes the marked region, keeping everything outside it.
    ///
    /// The writes that carry the chip's register state across the cut are spliced
    /// in at the seam, so what follows still plays on the state it was written
    /// against -- including the "trim the intro" case, where the region starts at
    /// the very beginning and the patch is the whole state replay.
    ///
    /// Returns `(removed, bridged)`: how many instructions the region held, and
    /// how many the seam patch contributed. `None` on the same terms as
    /// [`Self::crop_to_markers`].
    pub fn delete_marked_region(&mut self) -> Option<(usize, usize)> {
        let removed = self.markers.end() - self.markers.start();
        let (_, bridged) =
            self.replace_stream("Delete Marked Region", dro_core::crop::delete_region)?;
        Some((removed, bridged))
    }

    /// Runs a marked-region edit and installs its rebuilt stream undoably,
    /// returning `(new_len, patch_len)`.
    ///
    /// The stream is rebuilt wholesale (a state patch is spliced in, and the loop
    /// metadata remapped across it), so the selection is cleared and the markers
    /// re-derived from the song's own remapped loop, exactly as
    /// [`Self::optimize_vgm`] does.
    ///
    /// A region covering the whole song is declined for both edits: it marks
    /// nothing out, so neither has anything to do -- which is also the predicate
    /// the menu items enable on.
    fn replace_stream(
        &mut self,
        description: &'static str,
        edit: fn(&Song, usize, usize) -> Option<CropOutcome>,
    ) -> Option<(usize, usize)> {
        let (start, end) = (self.markers.start(), self.markers.end());
        let song = self.song.as_mut()?;
        if self.markers.is_full(song.len()) {
            return None;
        }
        let outcome = edit(song, start, end)?;
        let stats = (outcome.len(), outcome.patch_len);
        self.undo
            .execute(Box::new(ReplaceStream::new(description, outcome)), song);
        self.markers = RangeMarkers::from_song(song);
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        Some(stats)
    }

    /// Reverts the last edit, returning its description for the status bar,
    /// or `None` when there is nothing to undo. Selection is left alone (its
    /// indices may now point at different rows), except that rows past the new
    /// end are dropped -- nonexistent rows cannot stay selected.
    pub fn undo(&mut self) -> Option<String> {
        if let Some(file) = self.vgm.as_mut() {
            let description = self.vgm_undo.undo(file)?.to_owned();
            self.selection.truncate_to(file.len());
            self.revision += 1;
            return Some(description);
        }
        let song = self.song.as_mut()?;
        let description = self.undo.undo(song)?.to_owned();
        self.selection.truncate_to(song.len());
        self.markers.clamp_to(song.len());
        self.analysis.invalidate();
        self.revision += 1;
        Some(description)
    }

    /// Re-applies the last undone edit.
    pub fn redo(&mut self) -> Option<String> {
        if let Some(file) = self.vgm.as_mut() {
            let description = self.vgm_undo.redo(file)?.to_owned();
            self.selection.truncate_to(file.len());
            self.revision += 1;
            return Some(description);
        }
        let song = self.song.as_mut()?;
        let description = self.undo.redo(song)?.to_owned();
        self.selection.truncate_to(song.len());
        self.markers.clamp_to(song.len());
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
        // The waveform and the audio snapshot re-key on the revision.
        self.revision += 1;
    }

    /// Replaces the DRO song with its VGM conversion. Not undoable: the
    /// history is wiped.
    ///
    /// # Errors
    /// If no song is loaded, or it is already a VGM.
    pub fn convert_to_vgm(&mut self) -> Result<(), String> {
        let song = self.song.as_ref().ok_or("no song is loaded")?;
        let converted = convert::dro_to_vgm(song).map_err(|e| e.to_string())?;
        // The instruction stream is re-encoded, so any marked region no longer
        // means what it did; start over from the converted song's own loop.
        self.markers = RangeMarkers::from_song(&converted);
        self.song = Some(converted);
        self.path = None;
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
        Ok(())
    }

    /// Replaces the DRO v2 song with its v1 conversion. Not undoable: the
    /// history is wiped, as [`Self::convert_to_vgm`] does.
    ///
    /// The song is renamed `<stem>_1.<ext>`, matching `drotrim convert`'s output
    /// name -- so a following Save As cannot silently overwrite the v2 original.
    ///
    /// # Errors
    /// If no song is loaded, or it is not a DRO v2.
    pub fn convert_to_dro1(&mut self) -> Result<(), String> {
        let song = self.song.as_ref().ok_or("no song is loaded")?;
        let mut converted = convert::dro2_to_dro1(song).map_err(|e| e.to_string())?;
        converted.name = convert::dro1_default_name(&converted.name);
        // v1 re-encodes the stream (bank switches, escapes), so a marked region
        // no longer means what it did.
        self.markers = RangeMarkers::from_song(&converted);
        self.song = Some(converted);
        self.path = None;
        self.undo.reset();
        self.selection.clear();
        self.analysis.invalidate();
        self.revision += 1;
        self.saved_revision = Some(self.revision);
        self.metadata_dirty = false;
        Ok(())
    }

    /// Applies the GD3 tag editor's Save. Not undoable. Ignored unless the song
    /// is a VGM.
    pub fn set_gd3_tag(&mut self, tag: dro_core::Gd3Tag) {
        let Some(meta) = self.song.as_mut().and_then(Song::vgm_meta_mut) else {
            return;
        };
        // Only a real change dirties the song: the dialog's Save fires whether or
        // not anything was typed, and prompting to discard nothing is noise.
        if meta.tag.as_ref() != Some(&tag) {
            meta.tag = Some(tag);
            self.metadata_dirty = true;
        }
    }

    /// Applies the VGM metadata dialog's Save. Not undoable.
    ///
    /// An out-of-range loop point is dropped rather than stored: the dialog
    /// validated against the length it captured at open, and the VGM writer
    /// panics on a loop point past the end. The dialog is modal now, so the
    /// song is unlikely to have shortened underneath it -- but a panic in the
    /// writer is not the way to find out otherwise.
    /// Applies the edited VGM header fields. Returns `true` if the loop point
    /// was out of range for the *current* (possibly shortened since the dialog
    /// opened) song and had to be dropped, so the caller can surface it instead
    /// of losing it silently.
    pub fn set_vgm_metadata(
        &mut self,
        loop_point: Option<usize>,
        loop_end: Option<usize>,
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
        // The end goes the same way as the start when the start is dropped: an
        // end without a start describes a region with no beginning. Otherwise it
        // must stay inside the song and above the start, or it falls back to the
        // song's end -- which is what `None` already means.
        let clamped_end = clamped
            .and_then(|start| loop_end.filter(|&end| end <= len && end > start && end < len));
        let before = (
            meta.loop_point,
            meta.loop_end,
            meta.loop_base,
            meta.loop_modifier,
            meta.volume_modifier,
        );
        meta.loop_point = clamped;
        meta.loop_end = clamped_end;
        meta.loop_base = loop_base;
        meta.loop_modifier = loop_modifier;
        meta.volume_modifier = volume_modifier;
        let changed = before
            != (
                clamped,
                clamped_end,
                loop_base,
                loop_modifier,
                volume_modifier,
            );
        // The stored loop may have been clamped to a since-shortened song; the
        // markers describe what was actually stored either way.
        self.markers = RangeMarkers::from_song(song);
        self.metadata_dirty |= changed;
        dropped
    }

    /// Writes the marked region into the song's VGM loop fields.
    ///
    /// Not undoable, matching the other metadata edits. Returns
    /// `false` for a DRO, which has nowhere to put a loop -- the caller turns
    /// that into the "convert to VGM first" message.
    ///
    /// A region covering the whole song still writes a loop: `0..len` is a
    /// perfectly ordinary "loop the lot". The end is stored only when it stops
    /// short of the tail, since `None` already means "to the end" and keeping it
    /// that way is what lets a later trim widen the loop with the song.
    pub fn apply_loop_to_metadata(&mut self) -> bool {
        let Some(song) = self.song.as_mut() else {
            return false;
        };
        let len = song.len();
        let (start, end) = (self.markers.start(), self.markers.end());
        let Some(meta) = song.vgm_meta_mut() else {
            return false;
        };
        let stored = (Some(start), (end < len).then_some(end));
        let changed = (meta.loop_point, meta.loop_end) != stored;
        (meta.loop_point, meta.loop_end) = stored;
        self.metadata_dirty |= changed;
        true
    }

    /// Whether the marked region differs from what the song's metadata records,
    /// i.e. whether applying it would change anything. Drives the unsaved cue on
    /// the waveform markers. Always `false` for a DRO, which stores no loop.
    #[must_use]
    pub fn loop_markers_are_unapplied(&self) -> bool {
        let Some(song) = self.song.as_ref() else {
            return false;
        };
        let Some(meta) = song.vgm_meta() else {
            return false;
        };
        let stored_end = meta.loop_end.unwrap_or(song.len());
        meta.loop_point != Some(self.markers.start()) || stored_end != self.markers.end()
    }

    // -- queries -------------------------------------------------------------

    /// Find Register / delay navigation: the next match strictly after (or
    /// before) the highest selected row, starting from the top when nothing is
    /// selected.
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

    /// What the instruction table's five columns are called for the loaded
    /// document. Only the second differs: an OPL song's rows have a bank, and
    /// a VGM held as one has rows that name the chip they write to.
    #[must_use]
    pub fn column_titles(&self) -> [&'static str; 5] {
        let second = if self.vgm.is_some() { "Chip" } else { "Bank" };
        ["Pos (hex)", second, "Reg.", "Value", "Description"]
    }

    /// The cells of row `index`, whichever kind of document is loaded.
    ///
    /// The table asks for these rather than reaching into a `Song`, so it does
    /// not have to know which kind it is drawing.
    #[must_use]
    pub fn row_cells(&mut self, index: usize) -> RowCells {
        let position = format!("{index:04X}");
        if let Some(file) = self.vgm.as_ref() {
            let Some(stream) = file.stream() else {
                return RowCells {
                    position,
                    ..RowCells::default()
                };
            };
            let (chip, register, value) = match stream.get(index) {
                Some(VgmCommand::Write { target, addr, data }) => (
                    target.label(),
                    format!("{addr:#06X}"),
                    format!("{data:#04X}"),
                ),
                _ => (String::new(), String::new(), String::new()),
            };
            return RowCells {
                position,
                bank: chip,
                register,
                value,
                description: stream.describe(index),
                hover: String::new(),
            };
        }

        let analysis = self.row_analysis(index);
        let Some(song) = self.song.as_ref() else {
            return RowCells {
                position,
                ..RowCells::default()
            };
        };
        RowCells {
            position,
            bank: analysis
                .as_ref()
                .map_or_else(String::new, |a| a.bank.index().to_string()),
            register: song.register_display(index).unwrap_or_default(),
            value: song.value_display(index).unwrap_or_default(),
            description: analysis.map_or_else(String::new, |a| a.description.into_owned()),
            hover: song
                .instruction_description(index)
                .unwrap_or_default()
                .to_owned(),
        }
    }

    // Only one document is ever loaded, so exactly one of the two histories can
    // be non-empty -- the VGM-held one is consulted first, and answers `false`
    // for an OPL song because it was reset when that song loaded.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        if self.vgm.is_some() {
            return self.vgm_undo.can_undo();
        }
        self.undo.can_undo()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        if self.vgm.is_some() {
            return self.vgm_undo.can_redo();
        }
        self.undo.can_redo()
    }

    #[must_use]
    pub fn undo_description(&self) -> Option<&str> {
        if self.vgm.is_some() {
            return self.vgm_undo.undo_description();
        }
        self.undo.undo_description()
    }

    #[must_use]
    pub fn redo_description(&self) -> Option<&str> {
        if self.vgm.is_some() {
            return self.vgm_undo.redo_description();
        }
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
        assert!(
            matches!(error, LoadFailure::Unreadable(message) if !message.is_empty()),
            "junk is unreadable, not a VGM for other chips"
        );
        assert_eq!(editor.len(), 14, "the old song survives a failed load");
        assert!(editor.path.is_some());
    }

    /// A VGM whose chips are not OPL is not a broken file, and must not be
    /// reported as one -- it comes back classified, with the parsed file for
    /// the caller to open or describe.
    #[test]
    fn a_vgm_for_other_chips_is_classified_rather_than_rejected() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let failure = editor
            .load(PickedFile {
                name: "sonic.vgm".to_owned(),
                path: Some(PathBuf::from("C:/rips/Sonic/sonic.vgm")),
                bytes: mega_drive_vgm(),
            })
            .unwrap_err();

        let LoadFailure::NotOpl { file, folder } = failure else {
            panic!("expected a non-OPL VGM");
        };
        assert_eq!(file.chip_list(), "YM2612");
        assert_eq!(folder, Some(PathBuf::from("C:/rips/Sonic")));
        assert_eq!(editor.len(), 14, "the old song survives");
    }

    /// A minimal YM2612 VGM whose body the OPL command table cannot even size.
    fn mega_drive_vgm() -> Vec<u8> {
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        bytes[0x08..0x0C].copy_from_slice(&0x161u32.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&(0x100u32 - 0x34).to_le_bytes());
        bytes[0x2C..0x30].copy_from_slice(&7_670_454u32.to_le_bytes());
        bytes[0x18..0x1C].copy_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&[0x52, 0x28, 0xF0, 0x80, 0x66]);
        let eof = bytes.len();
        bytes[0x04..0x08].copy_from_slice(&((eof - 4) as u32).to_le_bytes());
        bytes
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

        // Undo must restore the original ms_length, not the new value; this
        // pins that.
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
        // Save does not write VGM bytes over the original .dro path -- the
        // converted song has no path until Save As.
        assert!(editor.path.is_none());
        assert!(editor.selection.is_empty());
        assert!(!editor.can_undo());

        assert!(editor.convert_to_vgm().is_err(), "already a VGM");
    }

    #[test]
    fn convert_to_dro1_downgrades_the_song_and_renames_it() {
        let (mut editor, _) = loaded(&dro_song_v2());
        editor.selection.select_only(2);
        let before = editor.song().unwrap().total_delay_ms();

        editor.convert_to_dro1().unwrap();

        let song = editor.song().unwrap();
        assert_eq!(song.file_version, dro_core::song::DRO_FILE_V1);
        // The name `drotrim convert` would have written, so Save As cannot
        // silently overwrite the v2 the conversion came from.
        assert_eq!(song.name, "test_1.dro");
        assert_eq!(song.total_delay_ms(), before, "timing is preserved");
        assert!(editor.path.is_none());
        assert!(editor.selection.is_empty());
        assert!(!editor.can_undo(), "the conversion is not undoable");
        assert!(!editor.is_dirty(), "a fresh conversion has nothing to save");
    }

    #[test]
    fn convert_to_dro1_refuses_anything_that_is_not_a_v2() {
        // A v1 is already there...
        let (mut editor, _) = loaded(&tone_song());
        assert!(editor.convert_to_dro1().is_err());

        // ...and a VGM has no v1 to become.
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        assert!(editor.convert_to_dro1().is_err());
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
        let dropped = editor.set_vgm_metadata(Some(len + 50), None, 0, 0, 0);
        assert!(dropped, "the caller is told the loop point was dropped");
        assert_eq!(editor.song().unwrap().vgm_meta().unwrap().loop_point, None);
        // The write path must not panic on what was just stored.
        editor.save_bytes().unwrap();

        // A valid loop point still lands, and is not reported as dropped.
        let dropped = editor.set_vgm_metadata(Some(len - 1), None, 1, 2, 3);
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
    fn a_loop_end_is_only_kept_while_it_bounds_a_real_region() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        let len = editor.len();

        // Inside the song and after the start: kept, and the markers follow it.
        editor.set_vgm_metadata(Some(1), Some(len - 1), 0, 0, 0);
        assert_eq!(
            editor.song().unwrap().vgm_meta().unwrap().loop_end,
            Some(len - 1)
        );
        assert_eq!(
            (editor.markers.start(), editor.markers.end()),
            (1, len - 1),
            "the markers describe what was just stored"
        );

        // At the end of the song: stored as `None`, which already means that --
        // and is what lets a later trim widen the loop with the song.
        editor.set_vgm_metadata(Some(1), Some(len), 0, 0, 0);
        assert_eq!(editor.song().unwrap().vgm_meta().unwrap().loop_end, None);

        // At or before the start, or past the song: no region to bound.
        for end in [Some(1), Some(0), Some(len + 5)] {
            editor.set_vgm_metadata(Some(1), end, 0, 0, 0);
            assert_eq!(
                editor.song().unwrap().vgm_meta().unwrap().loop_end,
                None,
                "end {end:?} does not bound a region"
            );
        }

        // And an end without a start describes a region with no beginning.
        editor.set_vgm_metadata(None, Some(2), 0, 0, 0);
        let meta = editor.song().unwrap().vgm_meta().unwrap();
        assert_eq!((meta.loop_point, meta.loop_end), (None, None));
        editor.save_bytes().unwrap();
    }

    // -- crop / delete marked region -----------------------------------------

    #[test]
    fn cropping_keeps_the_marked_region_behind_a_state_prelude() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let len = editor.len();
        editor.markers.set_start(4, len);
        editor.markers.set_end(12, len);
        editor.selection.select_only(13);
        let before = editor.revision();

        let (kept, restored) = editor.crop_to_markers().expect("a real crop");
        assert!(restored > 0, "there is register state to restore");
        assert_eq!(kept, 8 + restored, "the region, behind its prelude");
        assert_eq!(editor.len(), kept);

        // The stream was rebuilt, so nothing may still point into the old one.
        assert!(editor.selection.is_empty());
        assert!(editor.markers.is_full(editor.len()));
        assert!(editor.revision() > before);
        assert!(editor.is_dirty());
        assert_eq!(editor.undo_description(), Some("Crop to Marked Region"));
    }

    #[test]
    fn deleting_the_marked_region_bridges_the_seam() {
        let (mut editor, _) = loaded(&dro_song_v2());
        let len = editor.len();
        editor.markers.set_start(3, len);
        editor.markers.set_end(9, len);

        let (removed, bridged) = editor.delete_marked_region().expect("a real cut");
        assert_eq!(removed, 6, "the region held six instructions");
        assert_eq!(editor.len(), len - removed + bridged);
        assert_eq!(editor.undo_description(), Some("Delete Marked Region"));
    }

    #[test]
    fn both_edits_undo_back_to_the_original_song() {
        for crop in [true, false] {
            let (mut editor, _) = loaded(&dro_song_v2());
            let original = editor.song().unwrap().clone();
            let len = editor.len();
            editor.markers.set_start(2, len);
            editor.markers.set_end(10, len);

            let done = if crop {
                editor.crop_to_markers()
            } else {
                editor.delete_marked_region()
            };
            assert!(done.is_some());
            assert_ne!(editor.song().unwrap(), &original);

            editor.undo();
            assert_eq!(
                editor.song().unwrap(),
                &original,
                "undo must restore the song exactly (crop: {crop})"
            );
            // The markers were reset by the edit, so they come back clamped to
            // the restored song rather than to where they were.
            assert!(editor.markers.end() <= editor.len());

            editor.redo();
            assert_ne!(editor.song().unwrap(), &original);
        }
    }

    #[test]
    fn neither_edit_runs_while_the_markers_cover_the_whole_song() {
        // The markers mark nothing out, so there is nothing to keep or cut --
        // which is also what the menu items enable on.
        let (mut editor, _) = loaded(&dro_song_v2());
        assert!(editor.markers.is_full(editor.len()));
        let before = editor.revision();

        assert!(editor.crop_to_markers().is_none());
        assert!(editor.delete_marked_region().is_none());
        assert_eq!(editor.revision(), before);
        assert!(!editor.can_undo());
        assert!(!editor.is_dirty());
    }

    #[test]
    fn neither_edit_runs_without_a_song() {
        let mut editor = Editor::new();
        assert!(editor.crop_to_markers().is_none());
        assert!(editor.delete_marked_region().is_none());
    }

    #[test]
    fn cropping_a_vgm_re_derives_the_markers_from_the_remapped_loop() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        let len = editor.len();
        // A loop that starts inside the region being kept.
        editor.set_vgm_metadata(Some(2), None, 0, 0, 0);
        editor.markers.set_start(1, len);
        editor.markers.set_end(len - 1, len);

        let (kept, restored) = editor.crop_to_markers().expect("a real crop");
        // The loop moved with the stream: down by the region's start, up past
        // the prelude -- and the markers were re-derived from it, so they now
        // describe the stored loop rather than the pre-crop region.
        let loop_point = editor.song().unwrap().vgm_meta().unwrap().loop_point;
        assert_eq!(loop_point, Some(2 - 1 + restored));
        assert_eq!(editor.markers.start(), 2 - 1 + restored);
        assert_eq!(editor.markers.end(), kept);
        // And the result is still a file the writer accepts.
        editor.save_bytes().unwrap();
    }

    #[test]
    fn deleting_the_intro_keeps_the_rest_playing_on_the_right_state() {
        // The case the whole state patch exists for: cut from the very start, so
        // the surviving tail would otherwise open on a silent chip.
        let (mut editor, _) = loaded(&dro_song_v2());
        let len = editor.len();
        editor.markers.set_start(0, len);
        editor.markers.set_end(8, len);

        let (removed, bridged) = editor.delete_marked_region().expect("a real cut");
        assert_eq!(removed, 8);
        assert!(bridged > 0, "the whole reached state has to be replayed");
        // Every register the intro had set is restored ahead of the tail.
        let song = editor.song().unwrap();
        assert_eq!(editor.len(), len - removed + bridged);
        assert!(song.instruction(0).is_some_and(|i| !i.is_delay()));
    }

    #[test]
    fn is_dirty_tracks_edits_and_saves() {
        assert!(!Editor::new().is_dirty(), "no song is never dirty");

        let (mut editor, _) = loaded(&dro_song_v2());
        assert!(!editor.is_dirty(), "a freshly loaded song is clean");

        editor.selection.select_only(0);
        assert!(editor.delete_selection(), "a row was deleted");
        assert!(editor.is_dirty(), "an edit dirties it");

        editor.mark_saved();
        assert!(!editor.is_dirty(), "saving cleans it");

        editor.selection.select_only(0);
        editor.delete_selection();
        assert!(editor.is_dirty(), "a further edit dirties it again");
    }

    #[test]
    fn metadata_edits_dirty_the_song_without_invalidating_the_audio() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        assert!(!editor.is_dirty(), "a fresh conversion is clean");
        let revision = editor.revision();

        editor.set_vgm_metadata(Some(1), None, 0, 0, 0);
        assert!(editor.is_dirty(), "a loop point is unsaved work");
        // The instruction stream did not change, so the audio snapshot and the
        // waveform must stay valid -- bumping the revision would reload the
        // stream and re-render the wave for nothing, interrupting playback.
        assert_eq!(editor.revision(), revision, "the stream is untouched");

        editor.mark_saved();
        assert!(!editor.is_dirty(), "saving clears it");

        editor.set_gd3_tag(dro_core::Gd3Tag {
            track_name_en: "Loop Test".to_owned(),
            ..Default::default()
        });
        assert!(editor.is_dirty(), "a tag edit is unsaved work too");
        assert_eq!(editor.revision(), revision);
    }

    #[test]
    fn a_metadata_save_that_changes_nothing_leaves_the_song_clean() {
        // Both dialogs' Save fires whether or not anything was typed, and the
        // apply-loop action re-applies whatever is already marked. Prompting to
        // discard nothing would train the prompt to be ignored.
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        editor.set_vgm_metadata(Some(1), None, 0, 0, 0);
        editor.mark_saved();

        editor.set_vgm_metadata(Some(1), None, 0, 0, 0);
        assert!(
            !editor.is_dirty(),
            "re-saving identical values changes nothing"
        );

        editor.set_gd3_tag(dro_core::Gd3Tag::default());
        editor.mark_saved();
        editor.set_gd3_tag(dro_core::Gd3Tag::default());
        assert!(!editor.is_dirty(), "re-saving an identical tag likewise");

        // The markers already match the metadata, so applying them is a no-op.
        assert!(editor.apply_loop_to_metadata());
        assert!(!editor.is_dirty(), "applying what is already stored");
    }

    #[test]
    fn applying_a_loop_region_dirties_the_song() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        let len = editor.len();
        editor.markers.set_start(1, len);
        editor.markers.set_end(len - 1, len);

        assert!(editor.apply_loop_to_metadata());
        assert!(
            editor.is_dirty(),
            "a loop region is deliberate work, and must not be lost silently"
        );
    }

    #[test]
    fn loading_a_song_clears_a_metadata_edit() {
        let (mut editor, _) = loaded(&tone_song());
        editor.convert_to_vgm().unwrap();
        editor.set_vgm_metadata(Some(1), None, 0, 0, 0);
        assert!(editor.is_dirty());

        editor.load(picked(&tone_song())).unwrap();
        assert!(!editor.is_dirty(), "a freshly loaded song is clean");
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
