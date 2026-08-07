use super::*;

impl VgmStudioApp {
    // -- the workflows -----------------------------------------------------

    pub(super) fn load_file(&mut self, file: PickedFile) {
        // Loading a song belongs to the editor: stop any pack preview and show
        // the editor tab so the load isn't invisible (menu Open, drag-and-drop,
        // and the CLI initial load can all fire while the pack tab is active).
        // Idempotent with open_track_in_editor, which also sets the tab.
        self.stop_preview();
        self.active_tab = AppTab::Editor;
        let name = file.name.clone();
        match self.editor.load(file) {
            Ok(report) => {
                self.status = crate::strings::app_status_opened(&name);
                // A dialog left open across a load would edit the wrong song
                // -- a stale Save silently corrupting it -- so anything
                // song-bound closes with the song.
                self.close_song_dialogs();
                self.waveform = WaveformState::default();
                // The exports belong to the song being replaced; drop them
                // rather than write out a song no longer on screen. (Their own
                // kinds, so this does not disturb the waveform render below.)
                self.tasks.cancel(TaskKind::RenderWav);
                self.tasks.cancel(TaskKind::Split);
                // Likewise a running volume scan: its peak is the old song's,
                // and landing late it would set this song's volume from it.
                self.tasks.cancel(TaskKind::VolumeScan);
                self.split_flow = None;
                self.submit_waveform(None);
                // Unload, not pause: the old stream's position must not leak
                // into the fresh cursor/readout via the end-of-playback
                // update below.
                self.audio.unload();
                self.peak_meter = PeakMeterState::default();
                // A new song starts with no clipping ceiling; its own limiter has
                // not engaged yet.
                self.boost_ceiling = None;
                self.audio_revision = None;
                self.was_playing = false;
                self.last_first_selected = None;
                self.scroll_to = Some(table::ScrollTo::centered(0));

                // What the document can do decides the rest. A VGM for chips
                // there is no core for is not a broken file -- it opens for
                // trimming, with the panels that need an OPL stream gone -- so
                // the difference is which of these two runs, not an error.
                match self.editor.dro_song() {
                    Some(song) => {
                        // A fresh song starts with every channel audible and
                        // panning reset to Original (pans seeded from the song
                        // type); stale mute/pan state must not carry over.
                        self.channels = ChipPanels::for_song(song);
                        let file_version = song.file_version;
                        self.position.set_length_ms(song.total_delay_ms());
                        self.position.set_position_ms(0);
                        self.push_load_warnings(report, file_version);
                    }
                    None => {
                        let file = self.editor.vgm().expect("just loaded");
                        let chips = file.chip_list();
                        self.channels = ChipPanels::for_vgm(file);
                        // The stream's own summed length, matching the waveform's
                        // timeline -- not the header's claim, which can lie.
                        self.position.set_length_ms(file.stream_total_ms());
                        self.position.set_position_ms(0);
                        // What the status promises has to match what the
                        // registry can actually build: "not supported" is only
                        // true when *no* chip in the file has a core.
                        let kinds: Vec<_> =
                            file.header.chips().iter().map(|chip| chip.kind).collect();
                        self.status = match vgms_synth::playability(&kinds) {
                            vgms_synth::Playability::Full => {
                                crate::strings::app_status_opened_chips(&name, &chips)
                            }
                            vgms_synth::Playability::Partial(missing) => {
                                let missing: Vec<&str> =
                                    missing.iter().map(|kind| kind.name()).collect();
                                crate::strings::app_status_opened_missing(
                                    &name,
                                    &chips,
                                    &missing.join(", "),
                                )
                            }
                            vgms_synth::Playability::None => {
                                crate::strings::app_status_opened_unsupported(&name, &chips)
                            }
                        };
                    }
                }
                // Unless the volume is locked, a freshly opened document starts
                // at the volume its header modifier asks for (unity for a DRO),
                // so the boost does not carry over. This runs for either kind of
                // document -- a non-OPL VGM's modifier counts too -- and after
                // the status is set, since set_boost(_, false) writes none.
                if !self.config.audio.lock_boost {
                    let boost = self.song_modifier_boost();
                    self.set_boost(boost, false);
                }
            }
            // Readable as a container, but its commands will not walk, so there
            // are no rows to show. The dialog says what the file is instead.
            Err(LoadFailure::Unwalkable { file, folder }) => {
                self.status = crate::strings::app_status_unreadable_commands(&name);
                self.dialogs.unwalkable_vgm = Some(UnwalkableVgmDialog::new(&file, folder));
            }
            Err(LoadFailure::Unreadable(message)) => self
                .alerts
                .push_back(Alert::new(crate::strings::APP_ERR_LOAD_FILE_TITLE, message)),
        }
    }

    /// Unloads the song, leaving the editor as it starts: the same teardown a
    /// load does before installing the next song, minus the song.
    pub(super) fn close_song(&mut self) {
        self.editor.close();
        self.close_song_dialogs();
        // The exports and the analysis belong to a song that has gone.
        self.tasks.cancel(TaskKind::RenderWav);
        self.tasks.cancel(TaskKind::Split);
        self.tasks.cancel(TaskKind::VolumeScan);
        self.split_flow = None;
        self.audio.unload();
        self.audio_revision = None;
        self.was_playing = false;
        self.waveform = WaveformState::default();
        self.peak_meter = PeakMeterState::default();
        self.boost_ceiling = None;
        self.position.set_length_ms(0);
        self.position.set_position_ms(0);
        self.last_first_selected = None;
        self.status = crate::strings::APP_STATUS_SONG_CLOSED.to_owned();
    }

    pub(super) fn push_load_warnings(&mut self, report: LoadReport, file_version: u32) {
        if report.auto_trimmed {
            self.alerts.push_back(Alert::new(
                crate::strings::APP_AUTO_TRIM_TITLE,
                crate::strings::APP_AUTO_TRIM_TEXT,
            ));
        }
        if report.delay_mismatch {
            self.alerts
                .push_back(mismatch_alert(report.auto_trimmed, file_version));
        }
    }

    pub(super) fn save(&mut self, force_dialog: bool) {
        if !self.require_document() {
            return;
        }
        let bytes = match self.editor.save_bytes() {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        let request = match (&self.editor.path, force_dialog) {
            (Some(path), false) => SaveRequest::InPlace {
                path: path.clone(),
                bytes,
            },
            _ => SaveRequest::Dialog {
                suggested_name: self
                    .editor
                    .document_name()
                    .expect("gated: require_document above")
                    .to_owned(),
                bytes,
            },
        };
        self.pending_saves.push_back(SavePurpose::Song);
        self.files.save(request);
    }
}
