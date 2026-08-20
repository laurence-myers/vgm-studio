use super::*;

impl VgmStudioApp {
    pub(super) fn submit_waveform(&mut self, debounce: Option<Duration>) {
        let Some(source) = self.audio_source() else {
            return;
        };
        self.tasks.submit(
            TaskRequest::RenderWaveform {
                source,
                num_buckets: waveform::NUM_BUCKETS,
                sample_rate: self.config.audio.frequency,
                resampling: self.resample_mode(),
            },
            debounce,
        );
    }

    /// The configured resampling method, decoded from its config slug. An
    /// unknown spelling -- a config written by a newer build -- falls back to
    /// the accurate default rather than failing the whole config.
    pub(super) fn resample_mode(&self) -> vgms_synth::resample::ResampleMode {
        vgms_synth::resample::ResampleMode::from_slug(&self.config.audio.resampling)
            .unwrap_or_default()
    }

    pub(super) fn audio_source(&self) -> Option<vgms_synth::AudioSource> {
        // `AudioSource` is `DocSource`; the OPL-first doc source is exactly what
        // this built by hand, now without cloning the file.
        self.editor.doc_source()
    }

    /// Loads the current song into the audio output if it is not already
    /// there. Cheap when nothing changed.
    pub(super) fn ensure_audio(&mut self) -> Result<(), String> {
        if self.audio_revision == Some(self.editor.revision()) {
            return Ok(());
        }
        let source = self
            .audio_source()
            .ok_or_else(|| "No song is loaded.".to_owned())?;
        self.audio.load(source, &self.config.audio)?;
        self.push_muting();
        self.push_panning();
        self.push_trims();
        self.audio_revision = Some(self.editor.revision());
        // The device may have rejected the configured frequency; positions
        // report frames at the stream's real rate, so the panel must too.
        if let Some(rate) = self.audio.output_rate() {
            self.position.set_frequency(rate);
            if let Some(timeline) = self.editor.timeline() {
                self.position.set_length_ms(timeline.total_ms());
            }
        }
        // Only now is the stream's real rate known, and the loop's start frame is
        // denominated in it -- so this must follow the load, not precede it.
        self.push_loop_config();
        Ok(())
    }

    /// Pushes the current muting to the audio output -- both the OPL muting and
    /// the any-chip mutes, since a document is one or the other and each is a
    /// no-op on the engine it does not drive.
    pub(super) fn push_muting(&mut self) {
        self.audio.set_muting(self.channels.muting());
        self.audio.set_chip_muting(self.channels.chip_muting());
    }

    /// Pushes the current panning to the audio output, both kinds, as
    /// [`push_muting`](Self::push_muting) does.
    pub(super) fn push_panning(&mut self) {
        self.audio.set_panning(self.channels.panning());
        self.audio.set_chip_panning(self.channels.chip_panning());
    }

    /// Pushes the current per-chip trims to the audio output. Generic-only:
    /// an OPL document has no trim (its level is the transport's Volume).
    pub(super) fn push_trims(&mut self) {
        self.audio.set_chip_trims(self.channels.chip_trims());
    }

    /// Refreshes the waveform's loop brackets from the markers.
    ///
    /// Nothing is drawn for an untouched region with looping off -- brackets at
    /// both extremes would be noise on a song nobody has marked up. Marking, or
    /// switching looping on, brings them in.
    pub(super) fn sync_loop_overlay(&mut self) {
        let markers = self.editor.markers;
        let len = self.editor.len();
        // Any playable document with a waveform can carry a loop overlay, not
        // only an OPL one -- the markers and the timeline are both generic.
        let worth_showing =
            self.editor.timeline().is_some() && (!markers.is_full(len) || self.loop_enabled);
        self.waveform.loop_overlay = worth_showing
            .then(|| {
                let timeline = self.editor.timeline()?;
                Some(waveform::LoopOverlay {
                    start_ms: timeline.ms_offset_at(markers.start())?,
                    // The end is exclusive, so its time is where the *next*
                    // instruction starts -- which for `len` is the end of the song.
                    end_ms: timeline
                        .ms_offset_at(markers.end())
                        .unwrap_or_else(|| timeline.total_ms()),
                    active: self.loop_enabled,
                    unapplied: self.editor.loop_markers_are_unapplied(),
                })
            })
            .flatten();
    }

    /// The housekeeping after a crop or a cut: the usual post-edit refresh, plus
    /// the loop config, since both edits reset the markers.
    ///
    /// The whole stream was rebuilt, so everything that pointed into the old one
    /// goes back to the top: the view, and the playback start (its marker on the
    /// waveform, the readout, and the stream's own position). Row 400 of the old
    /// numbering is a different instruction now -- or no instruction at all.
    pub(super) fn after_region_edit(&mut self) {
        self.scroll_to = Some(table::ScrollTo::centered(0));
        self.reset_playback_start();
        self.after_edit();
        self.push_loop_config();
    }

    /// Puts the playback start back at the beginning of the song: the waveform's
    /// start marker and cursor, the position readout, and the audio stream.
    pub(super) fn reset_playback_start(&mut self) {
        self.waveform.start_ms = 0;
        self.waveform.cursor_ms = 0;
        // The afterglow belongs to the old document's playback; a fresh song
        // starts with a clean screen.
        self.waveform.trail.clear();
        self.position.set_position_ms(0);
        self.audio.rewind();
    }

    /// Hands the audio service the region to repeat, or `None` when looping is
    /// off. Cheap and idempotent; call it after anything that moves the markers,
    /// changes the count, or reloads the stream.
    pub(super) fn push_loop_config(&mut self) {
        // The stream's real rate while one is live, else the configured one --
        // the same rule the position readout follows. `ensure_audio` re-pushes
        // once a device has negotiated its rate, so a mismatch cannot outlive
        // the next load.
        let rate = self
            .audio
            .output_rate()
            .unwrap_or(self.config.audio.frequency);
        let markers = self.editor.markers;
        let config = self
            .loop_enabled
            .then(|| match (self.editor.dro_song(), self.editor.vgm()) {
                (Some(song), _) => Some(LoopConfig::for_song(
                    song,
                    markers.start(),
                    markers.end(),
                    self.loop_count,
                    rate,
                )),
                (None, Some(file)) => Some(LoopConfig::for_vgm(
                    file,
                    markers.start(),
                    markers.end(),
                    self.loop_count,
                    rate,
                )),
                (None, None) => None,
            });
        // The armed config carries the scaled count (a file's own loop is
        // rescaled by its base/modifier); surface that as the readout total so
        // it agrees with what plays. With nothing armed, fall back to the
        // user's chosen count. This is the one chokepoint every count change,
        // marker move, and load routes through, and both shells read the same
        // already-scaled config -- so no per-backend plumbing is needed.
        let armed = config.flatten();
        self.loop_total = armed.map_or(self.loop_count, |config| config.count);
        self.audio.set_loop(armed);
    }

    pub(super) fn menu_state(&self) -> MenuState {
        let on_pack_tab = self.active_tab == AppTab::Pack;
        // Undo/Redo act on whichever tab shows: the pack file-edit stacks on the
        // pack tab, the editor's song-undo stack otherwise. On the pack tab they are
        // held off while a sequence is still running.
        let (can_undo, can_redo, undo_description, redo_description) = if on_pack_tab {
            let idle = !self.pack_busy();
            (
                idle && !self.pack_undo.is_empty(),
                idle && !self.pack_redo.is_empty(),
                self.pack_undo.last().map(|txn| txn.label.clone()),
                self.pack_redo.last().map(|txn| txn.label.clone()),
            )
        } else {
            (
                self.editor.can_undo(),
                self.editor.can_redo(),
                self.editor.undo_description().map(str::to_owned),
                self.editor.redo_description().map(str::to_owned),
            )
        };
        MenuState {
            pack_has_peaks: self
                .pack
                .as_ref()
                .is_some_and(|pack| !pack.peaks.is_empty()),
            can_undo,
            can_redo,
            undo_description,
            redo_description,
            on_pack_tab,
            focused_row: self.editor.selection.first(),
            // Any document, not just an OPL one: cropping a Mega Drive rip is
            // the same operation, and `vgms_core` carries the chip state across
            // the cut for either.
            has_marked_region: self.editor.has_document()
                && !self.editor.markers.is_full(self.editor.len()),
            // A VGM is a VGM whichever slot holds it: the format-gated items
            // (Edit Tag, VGM Metadata, Optimize, Fix Header) apply either way.
            song_type: self
                .editor
                .dro_song()
                .map(|song| song.file_type)
                .or_else(|| self.editor.vgm().map(|_| SongFileType::Vgm)),
            is_dro_v2: self.editor.dro_song().is_some_and(|song| {
                song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V2
            }),
            // Shown for an empty editor too, so the File menu looks as it always
            // has with nothing loaded; the click is gated by require_renderable.
            can_render: self.editor.capabilities().renderable || !self.editor.has_document(),
            // Anything that renders can be split per channel -- an OPL stream,
            // or a VGM with a core. Shown for an empty editor, like the rest of
            // the menu.
            can_split_channels: self.editor.capabilities().renderable
                || !self.editor.has_document(),
        }
    }

    /// `"Play last 3 seconds"`, formatted with two
    /// decimals only for fractional lengths, singular for exactly one second.
    pub(super) fn play_tail_label(&self) -> String {
        let ms = self.config.ui.tail_length;
        let value = if ms.is_multiple_of(1000) {
            (ms / 1000).to_string()
        } else {
            format!("{:.2}", f64::from(ms) / 1000.0)
        };
        let plural = if ms == 1000 { "" } else { "s" };
        crate::strings::app_play_tail_label(&value, plural)
    }

    pub(super) fn play_seam_label(&self) -> String {
        crate::strings::app_play_seam_label(
            self.play_tail_label()
                .trim_start_matches("Play last ")
                .to_owned(),
        )
    }
}
