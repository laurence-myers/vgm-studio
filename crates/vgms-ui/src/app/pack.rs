use super::*;

impl VgmStudioApp {
    // -- pack mode ----------------------------------------------------------

    pub(super) fn pack_is_dirty(&self) -> bool {
        self.pack.as_ref().is_some_and(|pack| pack.dirty)
    }

    /// Whether any pack file mutation is in flight (a reorder/undo/redo sequence,
    /// or a quick-edit rewrite/rename), so a new one is deferred rather than
    /// interleaved with it.
    pub(super) fn pack_busy(&self) -> bool {
        self.pack_run.is_some()
            || self.pending_pack_undo.is_some()
            || self.pending_rewrite.is_some()
            || self.pack_service.is_busy()
            // An "Optimize All" sweep still has tracks to send: it holds the
            // service between one track's write-back and the next dispatch, so
            // count it busy or a click in that window could interleave.
            || !self.pending_song_optimize.is_empty()
    }

    /// Starts running `transaction` -- its `forward` mutations, or (for `Undo`)
    /// its `inverse` -- one at a time through the file service.
    pub(super) fn start_pack_run(&mut self, transaction: PackTransaction, kind: PackRunKind) {
        // Every pack edit passes through here, so this is where a run of
        // keyboard reorders ends: anything that is not the next press in that
        // run -- a drag, an undo, a batch rename -- is an edit of its own.
        if !self.pack_run_coalesces {
            self.coalesce_next_reorder = None;
        }
        self.stop_preview();
        let mutations = if kind == PackRunKind::Undo {
            transaction.inverse.clone()
        } else {
            transaction.forward.clone()
        };
        self.pack_run = Some(PackRun {
            queue: mutations.into(),
            transaction,
            kind,
            rename_in_flight: false,
        });
        self.advance_pack_run();
    }

    /// Runs the next mutation of the in-flight sequence, or -- once the queue
    /// drains -- lands its transaction on the right stack and rescans the folder.
    pub(super) fn advance_pack_run(&mut self) {
        let next = match self.pack_run.as_mut() {
            Some(run) => run.queue.pop_front(),
            None => return,
        };
        match next {
            Some(PackMutation::Rename { from, to }) => {
                if let Some(run) = self.pack_run.as_mut() {
                    run.rename_in_flight = true;
                }
                self.files.rename(from, to);
            }
            Some(PackMutation::Write { path, bytes }) => {
                self.pending_saves.push_back(SavePurpose::PackOp);
                self.files.save(SaveRequest::InPlace { path, bytes });
            }
            Some(PackMutation::Delete { path }) => self.files.delete(path),
            None => {
                let Some(run) = self.pack_run.take() else {
                    return;
                };
                let PackRun {
                    mut transaction,
                    kind,
                    ..
                } = run;
                let label = transaction.label.clone();
                match kind {
                    PackRunKind::NewEdit => {
                        // A run of Alt+arrow presses on one track is one edit as
                        // far as the user is concerned: nine presses to lift a
                        // track to the top must not be nine undos back down.
                        if std::mem::take(&mut self.pack_run_coalesces)
                            && let Some(previous) = self.pack_undo.pop()
                        {
                            transaction = previous.then(transaction);
                        }
                        self.pack_undo.push(transaction);
                        self.pack_redo.clear();
                    }
                    PackRunKind::Redo => self.pack_undo.push(transaction),
                    PackRunKind::Undo => self.pack_redo.push(transaction),
                }
                // A memory-backed (zip) pack's file edits live only in the
                // in-service archive until Save Pack, so any mutation makes it
                // dirty -- the discard prompt and web beforeunload guard read this.
                if self
                    .pack
                    .as_ref()
                    .is_some_and(|pack| pack.origin.is_memory())
                    && let Some(pack) = self.pack.as_mut()
                {
                    pack.dirty = true;
                }
                self.rescan_pack_folder();
                self.status = match kind {
                    PackRunKind::Undo => crate::strings::app_status_pack_undone(&label),
                    PackRunKind::Redo => crate::strings::app_status_pack_redone(&label),
                    PackRunKind::NewEdit => format!("{label}."),
                };
            }
        }
    }

    /// Aborts the in-flight sequence after a failed rename/write, resyncing the
    /// folder to whatever actually landed. The transaction is discarded (not
    /// stacked), since it did not fully apply.
    pub(super) fn abort_pack_run(&mut self, message: String) {
        self.pack_run = None;
        self.alerts
            .push_back(Alert::new(crate::strings::APP_ERR_TRACK_OP_TITLE, message));
        self.rescan_pack_folder();
    }

    /// Drops the pack undo/redo history and any in-flight sequence -- for opening
    /// a new project or closing the current one. (A same-folder rescan keeps it.)
    pub(super) fn clear_pack_edits(&mut self) {
        self.pack_run = None;
        self.pack_undo.clear();
        self.pack_redo.clear();
        self.pending_pack_undo = None;
    }

    /// Moves the track at `index` by `delta` (`-1` up, `+1` down), renumbering the
    /// affected files. Ignored while another sequence runs or the move is a no-op.
    pub(super) fn move_pack_track(&mut self, index: usize, delta: isize) {
        if self.pack_busy() {
            return;
        }
        let Some(to) = index.checked_add_signed(delta) else {
            return;
        };
        self.move_pack_track_to(index, to);
    }

    /// Moves the focused track one slot: Alt+Up / Alt+Down, the keyboard path to
    /// the drag handle.
    ///
    /// The focus and the scroll request move with the track, so the keys can be
    /// pressed again immediately and the row cannot walk off the view. Nothing
    /// happens with no focused row, at the ends of the list, or while another
    /// file sequence is still running -- moving the focus then would put it on a
    /// track that did not move.
    pub(super) fn move_focused_pack_track(&mut self, delta: isize) {
        if self.pack_busy() {
            return;
        }
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let Some(from) = pack.focused_track else {
            self.status = crate::strings::APP_STATUS_CLICK_TRACK_FIRST.to_owned();
            return;
        };
        let Some(to) = from
            .checked_add_signed(delta)
            .filter(|to| *to < pack.tracks.len())
        else {
            return; // already at the end it is being pushed towards
        };
        // Picking up where the last keyboard move left off continues that edit
        // rather than starting another: the presses fold into one undo. Whether
        // it folds is decided when the run lands, since that is when there is a
        // transaction to fold into.
        self.pack_run_coalesces = self.coalesce_next_reorder == Some(from);
        if let Some(pack) = self.pack.as_mut() {
            pack.focused_track = Some(to);
            pack.scroll_to_track = Some(to);
        }
        self.move_pack_track_to(from, to);
        // Arm the *next* press to fold into this one.
        self.coalesce_next_reorder = Some(to);
    }

    /// Moves the track at `from` to `to`, renumbering every file the move
    /// displaces -- what dropping a dragged row runs. Ignored while another
    /// sequence runs or the move changes nothing.
    pub(super) fn move_pack_track_to(&mut self, from: usize, to: usize) {
        if self.pack_busy() {
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.reorder_transaction(from, to));
        if let Some(transaction) = transaction {
            self.start_pack_run(transaction, PackRunKind::NewEdit);
        }
    }

    /// Undo the most recent pack edit, running its inverse. Ignored while busy.
    pub(super) fn undo_pack_edit(&mut self) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        if let Some(transaction) = self.pack_undo.pop() {
            self.start_pack_run(transaction, PackRunKind::Undo);
        } else {
            self.status = crate::strings::APP_STATUS_NOTHING_TO_UNDO.to_owned();
        }
    }

    /// Redo the most recently undone pack edit, re-running its forward. Ignored
    /// while busy.
    pub(super) fn redo_pack_edit(&mut self) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        if let Some(transaction) = self.pack_redo.pop() {
            self.start_pack_run(transaction, PackRunKind::Redo);
        } else {
            self.status = crate::strings::APP_STATUS_NOTHING_TO_REDO.to_owned();
        }
    }

    /// Installs a freshly scanned folder as the pack project, or -- when it is a
    /// redelivery of the folder already open -- rescans in place, keeping the
    /// edited metadata.
    pub(super) fn open_folder(&mut self, folder: PickedFolder) {
        // Any folder delivery invalidates a running whole-pack volume scan: a
        // rescan may have renamed or rewritten the files it snapshotted, and a
        // different folder's scan must never fill this one's Peak column. The
        // peaks map itself is pruned per track in `refresh_files`.
        self.tasks.cancel(TaskKind::PackVolumeScan);
        self.pack_scan_progress = None;
        let same = self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.folder_path.is_some() && pack.folder_path == folder.path);
        if same {
            // Keep a running preview alive across an in-place rescan (e.g. after
            // a screenshot optimise redelivers the folder): refresh_files
            // re-matches it by name. Only stop the audio if that track vanished.
            let preview_lost = if let Some(pack) = self.pack.as_mut() {
                let had_preview = pack.preview.is_some();
                pack.refresh_files(folder);
                had_preview && pack.preview.is_none()
            } else {
                false
            };
            if preview_lost {
                self.audio.pause();
                self.audio.rewind();
                self.audio_revision = None;
            }
            // A rescan can reorder or drop tracks; the quick-edit dialog is bound
            // to one track, so close it rather than let it act on a stale list.
            self.close_pack_dialogs();
            return;
        }
        self.stop_preview();
        // A brand-new project starts with an empty edit history.
        self.clear_pack_edits();
        let today = self.pack_service.today();
        // A zip-opened pack is delivered under a `/vgms-zip-N` token path; that is
        // how any shell signals "this pack lives in memory" without a new field on
        // every `PickedFolder`.
        let origin = if folder_is_archive(folder.path.as_deref()) {
            crate::platform::PackOrigin::MemoryZip { source: None }
        } else {
            crate::platform::PackOrigin::Directory
        };
        let mut state = PackState::from_folder(folder, today);
        state.origin = origin;
        let warning = state.parse_warning.clone();
        let name = state.folder_name.clone();
        self.pack = Some(state);
        self.active_tab = AppTab::Pack;
        self.close_song_dialogs();
        self.close_pack_dialogs();
        // The editor's audio must not keep playing under the pack view.
        self.audio.unload();
        self.audio_revision = None;
        self.status = crate::strings::app_status_pack_opened(&name);
        if let Some(warning) = warning {
            self.alerts.push_back(Alert::new(
                crate::strings::APP_DESC_NOT_PARSED_TITLE,
                crate::strings::app_desc_not_parsed_body(&warning),
            ));
        }
    }

    pub(super) fn select_tab(&mut self, tab: AppTab) {
        if self.pack.is_none() {
            self.active_tab = AppTab::Editor;
            return;
        }
        if self.active_tab == tab {
            return;
        }
        self.stop_preview();
        // Leaving the editor: its audio must not keep playing under the pack view
        // (mirrors open_folder's rule). Resetting the revision makes the next
        // editor Play reload cleanly.
        if self.active_tab == AppTab::Editor {
            self.audio.unload();
            self.audio_revision = None;
        }
        self.active_tab = tab;
        if tab == AppTab::Pack {
            // DroSong-bound dialogs (Find Register, DRO Info, GD3, VGM metadata)
            // and Goto don't belong on the pack tab -- mirror the menu gating
            // that disables them there.
            self.close_song_dialogs();
            self.dialogs.goto = None;
            // Returning to the pack tab re-scans the folder so edits made in the
            // editor (or renames) are reflected.
            if let Some(path) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) {
                self.files.open_folder_path(path);
            }
        }
    }

    pub(super) fn close_pack(&mut self) {
        self.stop_preview();
        self.close_pack_dialogs();
        self.clear_pack_edits();
        self.pack = None;
        self.active_tab = AppTab::Editor;
        self.status = crate::strings::APP_STATUS_PACK_CLOSED.to_owned();
    }

    /// Saves `Game Name.txt` and `Game Name.m3u` into the folder.
    pub(super) fn save_pack_docs(&mut self) {
        if !self.pack.as_ref().is_some_and(PackState::can_save) {
            if self.pack.is_some() {
                self.alerts
                    .push_back(Alert::error(crate::strings::APP_ERR_NEED_GAME_NAME));
            }
            return;
        }
        // Fresh batch: forget any failure from a previous save-docs run.
        self.pack_docs_failed = false;
        let pack = self.pack.as_ref().expect("checked");
        let stem = pack.doc_stem();
        let description = pack.description_text().into_bytes();
        let m3u = pack.m3u_text(false).into_bytes();
        let folder = pack.folder_path.clone();
        let docs = [
            (format!("{stem}.txt"), description),
            (format!("{stem}.m3u"), m3u),
        ];
        for (name, bytes) in docs {
            self.pending_saves.push_back(SavePurpose::PackDoc);
            let request = match &folder {
                Some(folder) => SaveRequest::InPlace {
                    path: folder.join(&name),
                    bytes,
                },
                None => SaveRequest::Dialog {
                    suggested_name: name,
                    bytes,
                },
            };
            self.files.save(request);
        }
    }

    /// Builds and saves the release zip. Blocking validation errors abort;
    /// non-blocking warnings prompt first unless `confirmed`.
    pub(super) fn export_pack_zip(&mut self, confirmed: bool) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let validations = pack.validations();
        let mut request = pack.export_request();
        request.optimizer = self.config.optimizer;
        request.sample_roms = self.config.optimize_sample_roms;
        request.dac_runs = self.config.optimize_dac_runs;
        // The `pack` borrow ends here (validations and request are owned).
        if !validations.errors.is_empty() {
            self.alerts
                .push_back(Alert::error(validations.errors.join("\n")));
            return;
        }
        // Only errors block and only warnings prompt; the note tier is for the
        // submission checklist and deliberately never reaches the export dialog.
        if !validations.warnings.is_empty() && !confirmed {
            // The title already asks the question, so the body is just the list
            // -- repeating it at the end would only bury it under a scroll when
            // a pack trips a lot of checks.
            let listed = validations
                .warnings
                .iter()
                .map(|warning| format!("\u{2022} {warning}"))
                .collect::<Vec<_>>()
                .join("\n");
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_EXPORT_TITLE,
                crate::strings::app_export_warnings_body(&listed),
                Action::Pack(PackAction::ConfirmExportZip),
            ));
            return;
        }
        // Keep the folder's own docs in step with the zip's.
        if self.pack.as_ref().is_some_and(|pack| pack.dirty) {
            self.save_pack_docs();
        }
        self.pack_service.submit(request);
        self.status = crate::strings::APP_STATUS_BUILDING_ZIP.to_owned();
    }

    /// Save Pack: re-export a memory-backed (zip) pack. Runs the same export job
    /// as Export Zip -- songs optimised + gzipped, docs regenerated -- but names
    /// the result after the source pack and clears the dirty flag once it lands
    /// (see [`SavePurpose::SaveArchive`]). Delivered as a download / Save As; an
    /// in-place write to the source `.zip` on native is a later nicety.
    pub(super) fn save_pack_archive(&mut self) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        if !pack.origin.is_memory() {
            return;
        }
        // A game name is still required (it names every entry); block on the same
        // hard errors Export Zip does rather than build a nameless zip.
        let validations = pack.validations();
        if !validations.errors.is_empty() {
            self.alerts
                .push_back(Alert::error(validations.errors.join("\n")));
            return;
        }
        let mut request = pack.export_request();
        request.optimizer = self.config.optimizer;
        request.sample_roms = self.config.optimize_sample_roms;
        request.dac_runs = self.config.optimize_dac_runs;
        // Save back under the pack's own name, not the game-name-derived one.
        request.zip_name = format!("{}.zip", pack.folder_name);
        self.pack_saving_archive = true;
        self.pack_service.submit(request);
        self.status = crate::strings::APP_STATUS_BUILDING_ZIP.to_owned();
    }

    /// Previews a track through the audio output.
    pub(super) fn preview_track(&mut self, index: usize) {
        let source = self
            .pack
            .as_ref()
            .and_then(|pack| pack.tracks.get(index))
            .and_then(crate::pack::PackTrack::preview_source);
        let Some(source) = source else {
            return;
        };
        // Preview with the track's own default panning and no channel mutes: the
        // editor's channel panel is for a different song, and its stored
        // panning/muting would otherwise leak into the preview (e.g. a dual-OPL2
        // editor song's fixed hard-L/R image applied to a mono track plays it
        // hard left). A DRO track carries an OPL image; every other track -- an
        // OPL VGM included, now that it plays the generic path -- resets the chip
        // mixer instead. Both are sent below; the service applies whichever the
        // source speaks and ignores the other, exactly as the editor's load does.
        let preview_panning = source
            .dro()
            .map(|song| crate::widgets::chip_panels::default_opl_panning(song));
        // `load` below tears down the editor's stream the instant it runs --
        // success or not -- so the editor's audio snapshot is gone regardless.
        // Invalidate the revision *before* the load so the editor's next Play
        // reloads its own song instead of wedging on "No song is loaded" or
        // resuming this preview. Clear any prior preview marker up front too, so
        // a failure below can't strand a stop button on the old track.
        self.audio_revision = None;
        if let Some(pack) = self.pack.as_mut() {
            pack.preview = None;
        }
        // Preview at the track's own volume: unless the volume is locked, start it
        // from the track's header modifier -- on a copy of the config, so the
        // preview does not disturb the editor's volume. Both kinds of source
        // carry one: an OPL snapshot in its meta, any other VGM in its header.
        // (Black Knight 2000's rips ask for as little as 0.25x, and previewing
        // them at the editor's boost was most of why they played too loud.)
        let mut preview_config = self.config.audio.clone();
        if !preview_config.lock_boost {
            preview_config.boost = match &source {
                vgms_synth::AudioSource::Dro(song) => Self::modifier_boost(song),
                vgms_synth::AudioSource::Vgm(file) => {
                    vgms_core::volume_modifier_factor(file.header.volume_modifier())
                }
            };
        }
        self.audio.pause();
        if let Err(message) = self.audio.load(source, &preview_config) {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.set_muting(Muting::all());
        self.audio.set_chip_muting(ChipMuting::new());
        if let Some(panning) = preview_panning {
            self.audio.set_panning(panning);
        }
        self.audio.set_chip_panning(ChipPanning::new());
        self.audio.set_chip_trims(ChipTrims::new());
        if let Err(message) = self.audio.play() {
            // Load succeeded but playback won't start: drop the half-started
            // preview so the service isn't left holding it (and the editor's
            // next Play reloads cleanly via the reset revision above).
            self.audio.unload();
            self.alerts.push_back(Alert::error(message));
            return;
        }
        if let Some(pack) = self.pack.as_mut() {
            pack.preview = Some(index);
        }
    }

    pub(super) fn stop_preview(&mut self) {
        if self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.preview.is_some())
        {
            self.audio.pause();
            self.audio.rewind();
            if let Some(pack) = self.pack.as_mut() {
                pack.preview = None;
            }
            self.audio_revision = None;
        }
    }

    /// Loads a track into the editor and switches to the editor tab. The pack
    /// project is retained; returning to it rescans the folder.
    pub(super) fn open_track_in_editor(&mut self, index: usize) {
        let Some(track) = self.pack.as_ref().and_then(|pack| pack.tracks.get(index)) else {
            return;
        };
        // Every readable track opens: `load_file` sorts out which
        // representation it needs. Only one whose commands will not walk has
        // nothing to show, and that gets the dialog rather than an empty table.
        let file = PickedFile {
            name: track.file_name.clone(),
            path: track.path.clone(),
            bytes: track.bytes.clone(),
        };
        // load_file stops any preview and switches to the editor tab; the
        // discard-changes prompt (if the editor is dirty) defers both until the
        // load is confirmed.
        self.load_or_confirm(file);
    }

    pub(super) fn open_track_quick_edit(&mut self, index: usize) {
        let dialog = self.pack.as_ref().and_then(|pack| {
            let track = pack.tracks.get(index)?;
            if !track.is_readable() {
                return None;
            }
            let tag = track.tag();
            // Every other track's name, so a rename can't collide with one.
            let siblings = pack
                .tracks
                .iter()
                .filter(|other| other.file_name != track.file_name)
                .map(|other| other.file_name.clone())
                .collect();
            Some(TrackEditDialog::new(
                index + 1,
                track.file_name.clone(),
                tag,
                siblings,
            ))
        });
        if let Some(dialog) = dialog {
            self.dialogs.track_edit = Some(dialog);
        }
    }

    /// Opens the per-track optimiser-options dialog, seeded with the track's
    /// effective options (its override, or the global default).
    pub(super) fn open_track_optimize_options(&mut self, index: usize) {
        let global = self.config.optimize_options();
        let dialog = self.pack.as_ref().and_then(|pack| {
            let track = pack.tracks.get(index)?;
            if !track.is_readable() {
                return None;
            }
            let had_override = pack.track_optimize_overrides.contains_key(&track.file_name);
            let options = pack.effective_optimize_options(&track.file_name, global);
            Some(crate::dialogs::TrackOptimizeDialog::new(
                track.file_name.clone(),
                options,
                had_override,
            ))
        });
        if let Some(dialog) = dialog {
            self.dialogs.track_optimize = Some(dialog);
        }
    }

    /// Sets a track's optimiser-options override, or clears it (`None`) back to
    /// the global default. Keyed by the file name the dialog opened on.
    pub(super) fn set_track_optimize_options(
        &mut self,
        file_name: String,
        options: Option<vgms_core::config::OptimizeOptions>,
    ) {
        if let Some(pack) = self.pack.as_mut() {
            match options {
                Some(options) => {
                    pack.track_optimize_overrides.insert(file_name, options);
                }
                None => {
                    pack.track_optimize_overrides.remove(&file_name);
                }
            }
        }
    }

    /// Applies a quick edit: rewrite the track's bytes with the new tag (and, if
    /// the name changed, rename the file). The list rescans on the outcomes, and
    /// the edit's inverse is stashed so it becomes undoable once it lands.
    pub(super) fn quick_edit_submitted(
        &mut self,
        original_name: String,
        new_name: String,
        tag: Gd3Tag,
    ) {
        if self.pack_busy() {
            return;
        }
        self.stop_preview();
        // Re-resolve the target by the name the dialog opened on: a rescan may
        // have reordered the list since, so the original index is unreliable.
        let Some(track) = self.pack.as_ref().and_then(|pack| {
            pack.tracks
                .iter()
                .find(|track| track.file_name == original_name)
        }) else {
            self.alerts
                .push_back(Alert::error(crate::strings::app_err_edit_gone(
                    &original_name,
                )));
            return;
        };
        let old_name = track.file_name.clone();
        let old_path = track.path.clone();
        // The bytes before this edit, for the undo transaction's inverse write.
        let old_bytes = track.bytes.clone();
        let Some(new_bytes) = track.retagged(&new_name, tag) else {
            return;
        };
        let new_bytes = match new_bytes {
            Ok(bytes) => bytes,
            Err(message) => {
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        let Some(old_path) = old_path else {
            return;
        };
        let new_path = old_path.with_file_name(&new_name);

        // Stash the reversible transaction: its forward matches what the bespoke
        // save path does below (so redo can replay it), its inverse restores the
        // old name and bytes. Committed to the undo stack when the save lands.
        let (forward, inverse) = if new_name == old_name {
            (
                vec![PackMutation::Write {
                    path: old_path.clone(),
                    bytes: new_bytes.clone(),
                }],
                vec![PackMutation::Write {
                    path: old_path.clone(),
                    bytes: old_bytes,
                }],
            )
        } else {
            (
                vec![
                    PackMutation::Rename {
                        from: old_path.clone(),
                        to: new_name.clone(),
                    },
                    PackMutation::Write {
                        path: new_path.clone(),
                        bytes: new_bytes.clone(),
                    },
                ],
                vec![
                    PackMutation::Rename {
                        from: new_path.clone(),
                        to: old_name.clone(),
                    },
                    PackMutation::Write {
                        path: old_path.clone(),
                        bytes: old_bytes,
                    },
                ],
            )
        };
        self.pending_pack_undo = Some(PackTransaction {
            label: format!("Edit {new_name}"),
            forward,
            inverse,
        });

        if new_name == old_name {
            // No rename: rewrite the bytes in place, in the unchanged format.
            self.pending_saves.push_back(SavePurpose::TrackRewrite);
            self.files.save(SaveRequest::InPlace {
                path: old_path,
                bytes: new_bytes,
            });
        } else {
            // Rename first, then rewrite the target-format bytes to the new path
            // once the rename lands (see poll_renamed) -- so a failed rename
            // can't strand the old file holding bytes its extension no longer
            // matches.
            self.pending_rewrite = Some((new_path, new_bytes));
            self.files.rename(old_path.clone(), new_name.clone());
            // If the renamed file is the one open in the editor, drop its stale
            // path so a later Ctrl+S does not resurrect the old name.
            if self.editor.path.as_deref() == Some(old_path.as_path()) {
                self.editor.path = None;
            }
        }
        self.status = crate::strings::app_status_updated(&new_name);
    }

    /// Opens the bulk-tag dialog over every readable track, its fields seeded
    /// from the package metadata. A no-op with no pack open or no readable tracks.
    pub(super) fn open_bulk_tag(&mut self) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let tracks: Vec<(String, String)> = pack
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| track.is_readable())
            .map(|(index, track)| {
                let title = track
                    .entry
                    .as_ref()
                    .map_or("", |entry| entry.title.as_str());
                (track.file_name.clone(), format!("{:02} {title}", index + 1))
            })
            .collect();
        if tracks.is_empty() {
            self.status = crate::strings::APP_STATUS_NO_TAGGABLE.to_owned();
            return;
        }
        let overlay = crate::pack::seed_from_meta(&pack.meta);
        self.dialogs.bulk_tag = Some(BulkTagDialog::new(tracks, overlay));
    }

    /// Applies a bulk GD3 edit: overlay the checked fields onto each target
    /// track's existing tag and rewrite the files as one undoable batch. Tracks
    /// whose tag would not change (and any not currently VGMs) are skipped, so a
    /// no-op selection writes nothing.
    pub(super) fn bulk_tag_submitted(&mut self, targets: Vec<String>, overlay: BulkTagOverlay) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        self.stop_preview();
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let mut forward = Vec::new();
        let mut inverse = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for name in &targets {
            let Some(track) = pack.tracks.iter().find(|track| &track.file_name == name) else {
                continue;
            };
            // Only VGMs carry a GD3 tag; the pack list is VGM/VGZ only, but guard
            // anyway so a non-VGM can never be rewritten by a bulk edit.
            let (true, Some(path)) = (track.is_readable(), track.path.clone()) else {
                continue;
            };
            let current = track.tag().cloned().unwrap_or_default();
            let new_tag = overlay.apply_to(&current);
            if new_tag == current {
                continue;
            }
            let Some(written) = track.retagged(&track.file_name, new_tag) else {
                continue;
            };
            match written {
                Ok(bytes) => {
                    forward.push(PackMutation::Write {
                        path: path.clone(),
                        bytes,
                    });
                    inverse.push(PackMutation::Write {
                        path,
                        bytes: track.bytes.clone(),
                    });
                }
                Err(message) => errors.push(format!("{name}: {message}")),
            }
        }

        if !errors.is_empty() {
            self.alerts.push_back(Alert::error(errors.join("\n")));
        }
        if forward.is_empty() {
            self.status = crate::strings::APP_STATUS_BULK_TAG_NOOP.to_owned();
            return;
        }
        let count = forward.len();
        let transaction = PackTransaction {
            label: format!(
                "Bulk tag {count} track{}",
                if count == 1 { "" } else { "s" }
            ),
            forward,
            inverse,
        };
        self.start_pack_run(transaction, PackRunKind::NewEdit);
    }

    /// Measures every readable track's peak in one background task (so the pack's
    /// many songs never freeze the UI); the results reach
    /// [`Self::handle_pack_peaks`] through `poll_services` and fill the Peak column.
    pub(super) fn scan_pack_volumes(&mut self) {
        let sample_rate = self.config.audio.frequency;
        let resampling = self.resample_mode();
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        let tracks: Vec<(String, vgms_synth::AudioSource)> = pack
            .tracks
            .iter()
            .filter_map(|track| {
                // Measuring a peak means rendering, so only tracks with a chip
                // this app can render are scanned -- the same rule the preview
                // button applies (an OPL stream, or any chip with a core).
                Some((track.file_name.clone(), track.preview_source()?))
            })
            .collect();
        if tracks.is_empty() {
            self.status = crate::strings::APP_STATUS_NO_RENDERABLE_TRACKS.to_owned();
            return;
        }
        let count = tracks.len();
        self.tasks.submit(
            TaskRequest::PackVolumeScan {
                tracks,
                sample_rate,
                resampling,
            },
            None,
        );
        self.status = crate::strings::app_status_scanning_volumes(count);
        // Seed the busy readout so it counts from "1 / count" the instant the
        // first track reports, rather than flashing "0 / 0".
        self.pack_scan_progress = Some((0, count));
    }

    /// Routes a streamed loop-search snapshot into the Find Loop dialog, if it is
    /// still open (it may have been closed mid-search, in which case the result is
    /// simply dropped, like the volume scan's).
    pub(super) fn handle_loop_candidates(&mut self, candidates: Vec<vgms_core::Candidate>) {
        let count = candidates.len();
        if let Some(dialog) = self.dialogs.find_loop.as_mut() {
            dialog.set_candidates(candidates);
        }
        self.status = if count == 0 {
            crate::strings::APP_STATUS_NO_LOOPS_FOUND.to_owned()
        } else {
            crate::strings::app_status_loop_candidates(count)
        };
    }

    /// Stores a finished pack volume scan's peaks (keyed by file name) for the Peak
    /// column and the suggested modifiers.
    pub(super) fn handle_pack_peaks(&mut self, peaks: Vec<(String, vgms_synth::Peak)>) {
        let Some(pack) = self.pack.as_mut() else {
            return;
        };
        let count = peaks.len();
        for (name, peak) in peaks {
            pack.peaks.insert(name, peak);
        }
        self.status = crate::strings::app_status_scanned_volumes(count);
        self.pack_scan_progress = None;
    }

    /// Sets each scanned track's VGM volume modifier so the pack is levelled, as
    /// one undoable batch. The skip-unchanged logic and the serialisation live in
    /// [`PackState::suggested_modifier_transaction`].
    ///
    /// `album` levels the whole pack by its loudest track (the VGMRips
    /// convention); otherwise each track is normalised to its own peak. It is
    /// written back to the Album latch, so the pad reflects what actually ran
    /// when the menu item was the one that asked.
    pub(super) fn apply_pack_modifiers(&mut self, album: bool) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        if let Some(pack) = self.pack.as_mut() {
            pack.album_normalize = album;
        }
        // Applying mid-scan would use the peaks from *before* the scan and then
        // discard its result (the rewrite's rescan cancels it) -- confusing both
        // ways, so wait it out.
        if self.tasks.is_busy_kind(TaskKind::PackVolumeScan) {
            self.status = crate::strings::APP_STATUS_STILL_SCANNING.to_owned();
            return;
        }
        self.stop_preview();
        let Some(transaction) = self
            .pack
            .as_ref()
            .and_then(PackState::suggested_modifier_transaction)
        else {
            self.status = crate::strings::APP_STATUS_MODIFIERS_NOOP.to_owned();
            return;
        };
        self.start_pack_run(transaction, PackRunKind::NewEdit);
    }

    /// The checklist's date fix-assist: rewrite every slash-separated release date
    /// to hyphens. The pack meta's own date is a form-level edit (applied at once,
    /// like typing); every track's GD3 date is rewritten as one undoable file
    /// batch, mirroring [`Self::apply_pack_modifiers`].
    pub(super) fn convert_pack_dates_to_hyphens(&mut self) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        self.stop_preview();
        let meta_changed = self
            .pack
            .as_mut()
            .is_some_and(PackState::hyphenate_meta_date);
        match self
            .pack
            .as_ref()
            .and_then(PackState::date_hyphenation_transaction)
        {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            None if meta_changed => {
                self.status = crate::strings::APP_STATUS_DATE_CONVERTED.to_owned();
            }
            None => self.status = crate::strings::APP_STATUS_NO_DATES.to_owned(),
        }
    }

    /// The name fix-assist: rename every file whose name has drifted from its GD3
    /// Track Name to the one `vgm_ren` would give it, as one undoable batch --
    /// the bulk counterpart of the quick-edit dialog's per-track rename.
    pub(super) fn rename_pack_tracks_from_tags(&mut self) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        self.stop_preview();
        match self
            .pack
            .as_ref()
            .and_then(PackState::rename_from_tags_transaction)
        {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            None => self.status = crate::strings::APP_STATUS_NAMES_MATCH.to_owned(),
        }
    }

    /// Kicks off an explicit lossless recompression of a screenshot.
    pub(super) fn recompress_image(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let image = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .cloned();
        let Some(image) = image else {
            return;
        };
        self.status = crate::strings::app_status_optimizing(&image.name);
        self.pack_service.optimize(image.name, image.bytes.to_vec());
    }

    /// Routes a finished optimisation: save a smaller file in place, or report
    /// that the original was already optimal.
    pub(super) fn image_optimized(&mut self, optimized: OptimizedImage) {
        if optimized.bytes.len() >= optimized.original_len {
            self.status =
                crate::strings::app_status_already_optimal(&optimized.name, optimized.original_len);
            return;
        }
        // The path and the pre-optimise bytes (for the undo transaction's inverse).
        let found = self.pack.as_ref().and_then(|pack| {
            pack.images
                .iter()
                .find(|image| image.name == optimized.name)
                .and_then(|image| image.path.clone().map(|path| (path, image.bytes.to_vec())))
        });
        let Some((path, old_bytes)) = found else {
            self.status = crate::strings::app_status_no_path(&optimized.name);
            return;
        };
        self.status = crate::strings::app_status_optimized_bytes(
            &optimized.name,
            optimized.original_len,
            optimized.bytes.len(),
        );
        self.pending_pack_undo = Some(PackTransaction {
            label: format!("Optimize {}", optimized.name),
            forward: vec![PackMutation::Write {
                path: path.clone(),
                bytes: optimized.bytes.clone(),
            }],
            inverse: vec![PackMutation::Write {
                path: path.clone(),
                bytes: old_bytes,
            }],
        });
        self.pending_saves.push_back(SavePurpose::ImageWritten);
        self.files.save(SaveRequest::InPlace {
            path,
            bytes: optimized.bytes,
        });
    }

    /// Optimises one track's VGM, verifying it renders identically before the
    /// smaller file is written back in place (D-orw-5). A single-track optimise
    /// is a sweep of one.
    pub(super) fn optimize_track(&mut self, index: usize) {
        let name = self
            .pack
            .as_ref()
            .and_then(|pack| pack.tracks.get(index))
            .filter(|track| track.is_readable() && track.path.is_some())
            .map(|track| track.file_name.clone());
        if let Some(name) = name {
            self.start_song_optimize(vec![name]);
        }
    }

    /// Optimises every readable track that has a path, one verified pass after
    /// another, each written back in place.
    pub(super) fn optimize_all_tracks(&mut self) {
        let names: Vec<String> = self.pack.as_ref().map_or_else(Vec::new, |pack| {
            pack.tracks
                .iter()
                .filter(|track| track.is_readable() && track.path.is_some())
                .map(|track| track.file_name.clone())
                .collect()
        });
        self.start_song_optimize(names);
    }

    /// Begins optimising `names` in order, off the UI thread, one at a time.
    fn start_song_optimize(&mut self, names: Vec<String>) {
        if self.pack_busy() || names.is_empty() {
            return;
        }
        self.song_optimize_progress = Some((0, names.len()));
        self.pending_song_optimize = names.into_iter().collect();
        self.dispatch_next_song_optimize();
    }

    /// Sends the next queued track to the optimiser, skipping any that have since
    /// vanished, and finishing the sweep when the queue is empty.
    fn dispatch_next_song_optimize(&mut self) {
        while let Some(name) = self.pending_song_optimize.pop_front() {
            if let Some(request) = self.song_optimize_request(&name) {
                self.status = self.song_optimize_status(&name);
                self.pack_service.optimize_song(request);
                return;
            }
            // The track was renamed or removed between queueing and now: count
            // it done so the progress total still lines up, and move on.
            self.bump_song_optimize_done();
        }
        self.finish_song_optimize_sweep();
    }

    /// Builds the optimise request for the track named `name`, or `None` when it
    /// is gone, unreadable, or has no path to write back to.
    fn song_optimize_request(&self, name: &str) -> Option<SongOptimizeRequest> {
        let pack = self.pack.as_ref()?;
        let track = pack.tracks.iter().find(|track| track.file_name == name)?;
        if !track.is_readable() || track.path.is_none() {
            return None;
        }
        // The track's own options if it has an override, else the global default.
        let options =
            pack.effective_optimize_options(&track.file_name, self.config.optimize_options());
        Some(SongOptimizeRequest {
            name: track.file_name.clone(),
            bytes: track.bytes.clone(),
            sample_roms: options.sample_roms,
            dac_runs: options.dac_runs,
            optimizer: options.optimizer,
            output_rate: self.config.audio.frequency,
        })
    }

    /// The status line while `name` is being optimised: a plain "Optimizing X…"
    /// for a single track, an "N / M" readout during a sweep.
    fn song_optimize_status(&self, name: &str) -> String {
        match self.song_optimize_progress {
            Some((done, total)) if total > 1 => {
                crate::strings::app_status_optimizing_track(name, done + 1, total)
            }
            _ => crate::strings::app_status_optimizing(name),
        }
    }

    /// Counts one track finished (optimised, kept, or skipped) for the progress
    /// readout.
    fn bump_song_optimize_done(&mut self) {
        if let Some(progress) = self.song_optimize_progress.as_mut() {
            progress.0 = (progress.0 + 1).min(progress.1);
        }
    }

    /// Clears the sweep's progress, reporting a one-line summary when more than
    /// one track was swept.
    fn finish_song_optimize_sweep(&mut self) {
        if let Some((done, total)) = self.song_optimize_progress.take()
            && total > 1
        {
            self.status = crate::strings::app_status_optimized_tracks(done, total);
        }
    }

    /// Routes a finished per-track optimise: record its savings-column status,
    /// write a verified shrink back in place, and (for the write-back case) let
    /// the save's outcome advance the sweep; otherwise advance it now.
    pub(super) fn song_optimized(&mut self, result: SongOptimizeResult) {
        self.bump_song_optimize_done();

        let status = match &result.outcome {
            SongOptimizeOutcome::Optimized(bytes) => TrackOptimizeStatus::Saved {
                from: result.original_len,
                to: bytes.len(),
            },
            SongOptimizeOutcome::Unchanged => TrackOptimizeStatus::Optimal,
            SongOptimizeOutcome::KeptDiffered(_) => TrackOptimizeStatus::KeptDiffered,
            SongOptimizeOutcome::Unverifiable(_) | SongOptimizeOutcome::Failed(_) => {
                TrackOptimizeStatus::Unverifiable
            }
        };
        if let Some(pack) = self.pack.as_mut() {
            pack.optimize_results.insert(result.name.clone(), status);
        }

        match result.outcome {
            SongOptimizeOutcome::Optimized(bytes) => {
                // The path and pre-optimise bytes, for the undo transaction's
                // inverse -- mirroring the screenshot optimise (`image_optimized`).
                let found = self.pack.as_ref().and_then(|pack| {
                    pack.tracks
                        .iter()
                        .find(|track| track.file_name == result.name)
                        .and_then(|track| {
                            track.path.clone().map(|path| (path, track.bytes.clone()))
                        })
                });
                let Some((path, old_bytes)) = found else {
                    // The track vanished before its result landed: nothing to
                    // write, but the sweep must still move on.
                    self.status = crate::strings::app_status_no_path(&result.name);
                    self.dispatch_next_song_optimize();
                    return;
                };
                self.status = crate::strings::app_status_optimized_bytes(
                    &result.name,
                    result.original_len,
                    bytes.len(),
                );
                self.pending_pack_undo = Some(PackTransaction {
                    label: format!("Optimize {}", result.name),
                    forward: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: bytes.clone(),
                    }],
                    inverse: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: old_bytes,
                    }],
                });
                self.pending_saves.push_back(SavePurpose::SongOptimized);
                self.files.save(SaveRequest::InPlace { path, bytes });
                // The sweep advances from the save's outcome, not here:
                // `pending_pack_undo` is a single slot the next track's write
                // would clobber before this one committed.
            }
            SongOptimizeOutcome::Unchanged => {
                self.status =
                    crate::strings::app_status_already_optimal(&result.name, result.original_len);
                self.dispatch_next_song_optimize();
            }
            SongOptimizeOutcome::KeptDiffered(reason)
            | SongOptimizeOutcome::Unverifiable(reason)
            | SongOptimizeOutcome::Failed(reason) => {
                self.status = crate::strings::app_status_optimize_kept(&result.name, &reason);
                self.dispatch_next_song_optimize();
            }
        }
    }

    /// Continues an "Optimize All" sweep after a track's write-back has landed
    /// (or failed): the previous transaction is settled, so the next track is
    /// safe to send.
    pub(super) fn advance_song_optimize_sweep(&mut self) {
        self.dispatch_next_song_optimize();
    }

    /// Copies a picked screenshot into the open pack's folder, then rescans so
    /// the Screenshots section picks it up.
    ///
    /// It lands as `<Game Name>.png`, joining the `.txt` and `.m3u` the pack
    /// already names that way; a screenshot out of DOSBox is called something
    /// like `dosbox_000.png`, and renaming it by hand is a step this tool saves.
    /// With no game name yet it keeps its own. The name is made unique against
    /// the folder (`... (2).png`) so a second screenshot never overwrites the
    /// first; Rename... then earns it a name of its own.
    pub(super) fn add_screenshot(&mut self, file: PickedFile) {
        let Some(pack) = self.pack.as_ref() else {
            return;
        };
        // The picker filters to .png, but a determined user can still get past
        // that -- and a non-PNG here would ship in the zip and fail review.
        if vgms_core::pack::PngInfo::parse(&file.bytes).is_none() {
            self.pending_screenshot = None;
            self.alerts.push_back(Alert::new(
                crate::strings::APP_NOT_PNG_TITLE,
                crate::strings::app_not_png_body(&file.name),
            ));
            return;
        }
        match self.pending_screenshot.take() {
            // Replace keeps the existing file's name: that is what makes it a
            // replacement rather than a second screenshot. Its old bytes are the
            // inverse, so it lands on the undo stack like any other rewrite.
            Some(ScreenshotPick::Replace(path)) => {
                let old_bytes = pack
                    .images
                    .iter()
                    .find(|image| image.path.as_ref() == Some(&path))
                    .map(|image| image.bytes.to_vec());
                let Some(old_bytes) = old_bytes else {
                    return; // rescanned away while the picker was open
                };
                self.pending_pack_undo = Some(PackTransaction {
                    label: format!("Replace {}", file_label(&path)),
                    forward: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: file.bytes.clone(),
                    }],
                    inverse: vec![PackMutation::Write {
                        path: path.clone(),
                        bytes: old_bytes,
                    }],
                });
                self.status = crate::strings::app_status_replacing(&file_label(&path));
                self.pending_saves.push_back(SavePurpose::ImageWritten);
                self.files.save(SaveRequest::InPlace {
                    path,
                    bytes: file.bytes,
                });
            }
            _ => {
                if pack.folder_path.is_none() {
                    // Native-only; a folder with no path cannot be written to.
                    return;
                }
                let proposed = pack.next_screenshot_name().unwrap_or_else(|| {
                    let (stem, ext) = file
                        .name
                        .rsplit_once('.')
                        .unwrap_or((file.name.as_str(), "png"));
                    pack.free_image_name(stem, ext)
                });
                let siblings = pack.images.iter().map(|image| image.name.clone()).collect();
                // The name is settled before anything is written: the dialog
                // holds the bytes, so closing it leaves the folder untouched.
                self.dialogs.screenshot_rename = Some(ScreenshotRenameDialog::adding(
                    file.name, &proposed, file.bytes, siblings,
                ));
            }
        }
    }

    /// Opens the picker to overwrite the screenshot at `index`.
    pub(super) fn replace_screenshot(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let path = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .and_then(|image| image.path.clone());
        let Some(path) = path else {
            return;
        };
        self.pending_screenshot = Some(ScreenshotPick::Replace(path));
        self.files.pick_image();
    }

    /// Writes a picked screenshot into the pack folder under the name the naming
    /// dialog settled on -- the second half of [`Self::add_screenshot`], once the
    /// user has seen and approved where it lands.
    /// With `recompress` the bytes go through oxipng *first* and the result is
    /// what lands, so the file is optimal in one write. Rewriting it afterwards
    /// would work too, but it would touch the disk twice and push a
    /// recompression of a file nobody has seen onto the undo stack.
    pub(super) fn add_screenshot_as(&mut self, file_name: &str, bytes: Vec<u8>, recompress: bool) {
        let Some(folder) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) else {
            return;
        };
        let add = PendingAdd {
            path: folder.join(file_name),
            bytes,
        };
        if recompress {
            self.status = crate::strings::app_status_recompressing(file_name);
            self.pack_service
                .optimize(file_name.to_owned(), add.bytes.clone());
            self.pending_add = Some(add);
            return;
        }
        self.write_added_screenshot(add);
    }

    /// Writes an added screenshot's bytes into the pack folder.
    pub(super) fn write_added_screenshot(&mut self, add: PendingAdd) {
        self.status = crate::strings::app_status_adding(&file_label(&add.path));
        self.pending_saves.push_back(SavePurpose::ScreenshotAdded);
        self.files.save(SaveRequest::InPlace {
            path: add.path,
            bytes: add.bytes,
        });
    }

    /// Opens the rename dialog on the screenshot at `index`, proposing the
    /// pack's own file-name stem.
    pub(super) fn open_screenshot_rename(&mut self, index: usize) {
        let dialog = self.pack.as_ref().and_then(|pack| {
            let image = pack.images.get(index)?;
            // Every other screenshot's name, so a rename can't collide with one.
            let siblings = pack
                .images
                .iter()
                .filter(|other| other.name != image.name)
                .map(|other| other.name.clone())
                .collect();
            Some(ScreenshotRenameDialog::new(
                image.name.clone(),
                &pack.doc_stem(),
                siblings,
            ))
        });
        if let Some(dialog) = dialog {
            self.dialogs.screenshot_rename = Some(dialog);
        }
    }

    /// Runs the screenshot rename as a pack transaction, so Edit > Undo puts the
    /// old name back.
    pub(super) fn rename_screenshot(&mut self, original_name: &str, file_name: &str) {
        if self.pack_busy() {
            self.status = crate::strings::APP_STATUS_TRACK_OP_RUNNING.to_owned();
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.rename_image_transaction(original_name, file_name));
        match transaction {
            Some(transaction) => self.start_pack_run(transaction, PackRunKind::NewEdit),
            // Rescanned away while the dialog was open.
            None => self
                .alerts
                .push_back(Alert::error(crate::strings::app_err_renamed_gone(
                    original_name,
                ))),
        }
    }

    /// Asks before removing a screenshot from the folder. Undo can put it back
    /// while the pack stays open, but the file does leave the disk, so this is
    /// not something to do on a stray click.
    pub(super) fn confirm_delete_screenshot(&mut self, index: usize) {
        if self.pack_busy() {
            return;
        }
        let Some(name) = self
            .pack
            .as_ref()
            .and_then(|pack| pack.images.get(index))
            .map(|image| image.name.clone())
        else {
            return;
        };
        self.alerts.push_back(Alert::confirm(
            crate::strings::APP_CONFIRM_DELETE_SCREENSHOT_TITLE,
            crate::strings::app_delete_screenshot_body(&name),
            Action::Pack(PackAction::ConfirmDeleteScreenshot(name)),
        ));
    }

    /// Runs the delete as a pack transaction, so Edit > Undo writes it back.
    pub(super) fn delete_screenshot(&mut self, name: &str) {
        if self.pack_busy() {
            return;
        }
        let transaction = self
            .pack
            .as_ref()
            .and_then(|pack| pack.delete_image_transaction(name));
        if let Some(transaction) = transaction {
            self.start_pack_run(transaction, PackRunKind::NewEdit);
        }
    }

    pub(super) fn rescan_pack_folder(&mut self) {
        if let Some(path) = self.pack.as_ref().and_then(|pack| pack.folder_path.clone()) {
            self.files.open_folder_path(path);
        }
    }

    /// Closes pack-bound dialogs (quick-edit, bulk-tag and screenshot rename),
    /// analogous to [`Self::close_song_dialogs`]. Each binds to the folder's
    /// current contents, so a rescan that can reorder or drop files must dismiss
    /// them.
    pub(super) fn close_pack_dialogs(&mut self) {
        self.dialogs.track_edit = None;
        self.dialogs.track_optimize = None;
        self.dialogs.bulk_tag = None;
        self.dialogs.screenshot_rename = None;
    }
}
