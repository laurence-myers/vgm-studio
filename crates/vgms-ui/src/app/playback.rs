use super::*;

impl VgmStudioApp {
    /// Seeks live playback to instruction `index`, addressed by time.
    ///
    /// Playback locates a position in milliseconds, not by row index: an OPL
    /// document plays through the generic engine over a projected VGM whose
    /// command indices need not line up with the document's rows (ou-2), so a
    /// row is found by the instant it plays at -- which both engines agree on --
    /// rather than by index. A row past the end (or a document with no timeline)
    /// falls back to the start.
    pub(super) fn seek_to_row(&mut self, index: usize) {
        let ms = self
            .editor
            .timeline()
            .and_then(|timeline| timeline.ms_offset_at(index))
            .unwrap_or(0);
        self.audio.seek_ms(ms);
    }

    pub(super) fn do_play(&mut self) {
        if !self.require_playable() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.rewind();
        if let Some(first) = self.editor.selection.first() {
            self.seek_to_row(first);
        }
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    pub(super) fn do_stop(&mut self) {
        if !self.require_playable() {
            return;
        }
        self.audio.pause();
        self.audio.rewind();
    }

    pub(super) fn do_play_tail(&mut self) {
        if !self.require_playable() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Measured length, not the header's: on a DRO whose header overstates
        // the length, the header value would seek past the end and play nothing.
        // The timeline serves either representation, so a non-OPL VGM (playable,
        // but with no OPL song) gets its ending auditioned too.
        let Some(total) = self.editor.timeline().map(|timeline| timeline.total_ms()) else {
            return;
        };
        self.audio.rewind();
        self.audio
            .seek_ms(total.saturating_sub(self.config.ui.tail_length));
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    /// Plays the loop join: the last `tail_length` ms of the region, looping, so
    /// the seam is heard on its own instead of after a full pass.
    ///
    /// Forces looping on -- auditioning a join with looping off would play the
    /// tail straight through and never reach the seam at all.
    pub(super) fn do_play_seam(&mut self) {
        if !self.require_playable() {
            return;
        }
        self.loop_enabled = true;
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        // Where the seam is, in milliseconds. Either representation can say:
        // a `DroSong` has the prefix sums already, and a VGM's is its own waits.
        let end = self.editor.markers.end();
        let Some(end_ms) = self.editor.dro_song().map_or_else(
            || {
                self.editor.vgm().map(|file| {
                    let elapsed = file.stream().map_or(0, |stream| {
                        stream.total_samples() - stream.samples_from(end)
                    });
                    vgms_core::util::smp_to_ms(
                        u32::try_from(elapsed).unwrap_or(u32::MAX),
                        vgms_core::vgm::VGM_SAMPLE_RATE,
                    )
                })
            },
            |song| song.ms_offset_at(end),
        ) else {
            return;
        };
        self.audio.rewind();
        self.audio
            .seek_ms(end_ms.saturating_sub(self.config.ui.tail_length));
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }

    /// Moves one loop marker (whichever is `Some`) and re-arms playback.
    pub(super) fn set_loop_marker(&mut self, start: Option<usize>, end: Option<usize>) {
        let len = self.editor.len();
        if len == 0 {
            return;
        }
        if let Some(index) = start {
            self.editor.markers.set_start(index, len);
        }
        if let Some(index) = end {
            self.editor.markers.set_end(index, len);
        }
        let markers = self.editor.markers;
        self.push_loop_config();
        self.status = crate::strings::app_status_loop_marked(
            markers.start(),
            markers.end(),
            markers.end() - markers.start(),
        );
    }

    /// Writes the marked region into the song's VGM loop fields.
    pub(super) fn apply_loop_to_metadata(&mut self) {
        if !self.require_document() {
            return;
        }
        let markers = self.editor.markers;
        let len = self.editor.len();
        if !self.editor.apply_loop_to_metadata() {
            self.alerts.push_back(Alert::new(
                crate::strings::APP_NOT_VGM_TITLE,
                crate::strings::APP_NOT_VGM_BODY,
            ));
            return;
        }
        // A VGM's loop length is defined as running to the end of the file, and
        // other players restart at the end-of-data command whatever the header
        // says. An end short of the tail is honoured here and survives a save,
        // but say so plainly rather than let it be discovered later.
        self.status = if markers.end() < len {
            crate::strings::app_status_loop_saved_range(markers.start(), markers.end())
        } else {
            crate::strings::app_status_loop_saved_end(markers.start())
        };
    }

    /// Submits a background loop search of the current song. The streamed
    /// candidates reach the Find Loop dialog through [`Self::handle_loop_candidates`];
    /// cancel-on-resubmit means clicking Search again just restarts it.
    pub(super) fn start_loop_search(&mut self, min_len_commands: usize) {
        // Either representation: a loop is a repeated block, which is not an
        // OPL idea. The cached doc source hands over the file without a clone.
        let Some(source) = self.editor.doc_source() else {
            self.status = crate::strings::APP_STATUS_OPEN_SONG_FIRST.to_owned();
            return;
        };
        self.tasks.submit(
            TaskRequest::LoopSearch {
                source,
                min_len_commands,
            },
            None,
        );
        self.status = crate::strings::APP_STATUS_SEARCHING_LOOPS.to_owned();
    }

    pub(super) fn delay_navigate(&mut self, backwards: bool) {
        if !self.require_document() {
            return;
        }
        // An OPL document searches its instruction stream; any other VGM
        // searches its command stream. Both step through the delays.
        let found = if self.editor.has_dro() {
            self.editor.find_next(FindTarget::AnyDelay, backwards)
        } else {
            self.editor
                .find_next_vgm(vgms_core::vgm::VgmFindTarget::AnyDelay, backwards)
        };
        match found {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(table::ScrollTo::centered(index));
            }
            None => self.status = crate::strings::APP_STATUS_NO_MORE_DELAYS.to_owned(),
        }
    }

    pub(super) fn goto_submitted(&mut self, text: &str) {
        if !self.require_document() {
            return;
        }
        let len = self.editor.len();
        // The Pos. column is hex, so Goto reads hex too (an optional 0x is fine),
        // and the messages echo the position in hex.
        let trimmed = text.trim();
        let digits = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        match usize::from_str_radix(digits, 16) {
            Err(_) => self.status = crate::strings::app_status_goto_invalid(text),
            Ok(position) if position >= len => {
                self.status = crate::strings::app_status_goto_out_of_range(position);
            }
            Ok(position) => {
                self.editor.selection.select_only(position);
                self.scroll_to = Some(table::ScrollTo::centered(position));
                self.status = crate::strings::app_status_goto_gone(position);
            }
        }
    }

    pub(super) fn find_register(&mut self, query: &crate::action::FindQuery, backwards: bool) {
        if !self.require_document() {
            return;
        }
        // Each query kind knows how to describe itself for the status line, and
        // which stream to search.
        let (found, label) = match query {
            crate::action::FindQuery::Dro(target) => (
                self.editor.find_next(*target, backwards),
                describe_dro_target(*target),
            ),
            crate::action::FindQuery::Vgm(target) => (
                self.editor.find_next_vgm(*target, backwards),
                describe_target(*target),
            ),
        };
        match found {
            Some(index) => {
                self.editor.selection.select_only(index);
                self.scroll_to = Some(table::ScrollTo::centered(index));
                self.status = crate::strings::app_status_find_found(&label, index);
            }
            None => self.status = crate::strings::app_status_find_not_found(&label),
        }
    }
}
