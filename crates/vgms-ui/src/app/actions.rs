use super::*;

impl VgmStudioApp {
    // -- actions ---------------------------------------------------------

    pub(super) fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Edit(action) => self.handle_edit_action(action),
            Action::File(action) => self.handle_file_action(ctx, action),
            Action::Loop(action) => self.handle_loop_action(action),
            Action::Mixer(action) => self.handle_mixer_action(action),
            Action::Pack(action) => self.handle_pack_action(action),
            Action::Playback(action) => self.handle_playback_action(action),
            Action::Settings(action) => self.handle_settings_action(ctx, action),
            Action::Ui(action) => self.handle_ui_action(action),
        }
    }

    /// Edit actions: undo/redo, deletion, conversion, the header and tag
    /// dialogs with their saves, and the find dialogs.
    fn handle_edit_action(&mut self, action: EditAction) {
        match action {
            EditAction::AuditHeader => self.audit_header(),
            EditAction::ConfirmFixHeader => {
                let fixed = self.editor.fix_header();
                self.status = match fixed {
                    0 => crate::strings::APP_STATUS_HEADER_AGREES.to_owned(),
                    1 => crate::strings::APP_STATUS_HEADER_FIXED_ONE.to_owned(),
                    count => crate::strings::app_status_header_fixed(count),
                };
            }
            EditAction::ConvertToDro1 => self.on_convert_to_dro1(),
            EditAction::ConvertToVgm => self.on_convert_to_vgm(),
            EditAction::DeleteSelection => self.on_delete_selection(),
            EditAction::FindRegister { query, backwards } => {
                self.find_register(&query, backwards);
            }
            EditAction::GotoSubmitted(text) => self.goto_submitted(&text),
            EditAction::OpenDroInfo => self.on_open_dro_info(),
            EditAction::OpenEditTag => self.on_open_edit_tag(),
            EditAction::OpenFindRegister => self.on_open_find_register(),
            EditAction::OpenGoto => {
                if self.require_document() {
                    self.dialogs.goto = Some(GotoDialog::new());
                }
            }
            EditAction::OpenVgmMetadata => self.on_open_vgm_metadata(),
            EditAction::OptimizeVgm => self.on_optimize_vgm(),
            EditAction::Redo => self.on_redo(),
            EditAction::SaveGd3(tag) => self.editor.set_gd3_tag(*tag),
            EditAction::SaveVgmMetadata {
                loop_point,
                loop_end,
                loop_base,
                loop_modifier,
                volume_modifier,
            } => {
                self.on_save_vgm_metadata(
                    loop_point,
                    loop_end,
                    loop_base,
                    loop_modifier,
                    volume_modifier,
                );
            }
            EditAction::Undo => self.on_undo(),
            EditAction::UpdateHeader {
                opl_type,
                ms_length,
            } => self.on_update_header(opl_type, ms_length),
        }
    }

    fn on_convert_to_dro1(&mut self) {
        if !self.require_song() {
            return;
        }
        match self.editor.convert_to_dro1() {
            Ok(()) => {
                self.status = crate::strings::APP_STATUS_CONVERTED_DRO1.to_owned();
                self.close_song_dialogs();
                self.scroll_to = Some(table::ScrollTo::centered(0));
                self.after_edit();
            }
            Err(message) => self.alerts.push_back(Alert::error(message)),
        }
    }

    fn on_convert_to_vgm(&mut self) {
        // `require_song` gates on an editable DRO; a VGM document is held
        // as a `VgmFile` and never reaches here.
        if !self.require_song() {
            return;
        }
        match self.editor.convert_to_vgm() {
            Ok(()) => {
                self.status = crate::strings::APP_STATUS_CONVERTED_VGM.to_owned();
                self.close_song_dialogs();
                self.scroll_to = Some(table::ScrollTo::centered(0));
                self.after_edit();
            }
            Err(message) => self.alerts.push_back(Alert::error(message)),
        }
    }

    fn on_delete_selection(&mut self) {
        if !self.require_document() {
            return;
        }
        if self.editor.delete_selection() {
            self.scroll_to = self.editor.selection.first().map(table::ScrollTo::centered);
            self.after_edit();
        }
    }

    fn on_open_dro_info(&mut self) {
        // The menu hides this for a VGM, so the shortcut must agree --
        // otherwise Ctrl+I opens a dialog the menu says does not apply. A
        // VGM's header is the VGM Metadata dialog's job. Checked before
        // `require_song`, whose "needs an OPL song" message is for an empty
        // editor, not a loaded VGM.
        if self.editor.vgm().is_some() {
            self.status = crate::strings::APP_STATUS_DRO_INFO_VGM.to_owned();
            return;
        }
        if self.require_song() {
            let song = self.editor.dro_song().expect("gated -- a DRO");
            let edit_allowed = self.config.ui.dro_info_edit_enabled;
            self.dialogs.dro_info = Some(DroInfoDialog::new(song, edit_allowed));
        }
    }

    fn on_open_edit_tag(&mut self) {
        if !self.require_document() {
            return;
        }
        // The document itself, not its OPL projection: the tag lives
        // in the file, and the projection is only a view of the stream.
        match self.editor.vgm() {
            // The tag lives in the file; a DRO has none.
            Some(file) => {
                self.dialogs.gd3_tag = Some(Gd3TagDialog::new(file.tag.as_ref()));
            }
            None => self.status = crate::strings::APP_STATUS_ONLY_VGM_TAG.to_owned(),
        }
    }

    fn on_open_find_register(&mut self) {
        // Either document kind: an OPL song gets the token/register
        // list, any other VGM the chip picker.
        self.dialogs.find_reg = match (self.editor.dro_song(), self.editor.vgm()) {
            (Some(song), _) => Some(FindRegDialog::new(song)),
            (None, Some(file)) => Some(FindRegDialog::for_vgm(file)),
            (None, None) => {
                self.require_document();
                None
            }
        };
    }

    fn on_open_vgm_metadata(&mut self) {
        if !self.require_document() {
            return;
        }
        // Metadata lives in the VGM header; a DRO has none.
        let dialog = self.editor.vgm().and_then(VgmMetadataDialog::for_vgm);
        match dialog {
            Some(dialog) => self.dialogs.vgm_metadata = Some(dialog),
            None => self.status = crate::strings::APP_STATUS_NOT_VGM.to_owned(),
        }
    }

    fn on_optimize_vgm(&mut self) {
        if !self.editor.has_document() {
            self.status = crate::strings::APP_STATUS_OPEN_SONG_FIRST.to_owned();
            return;
        }
        // Optimize is VGM-only; an editable DRO (`editor.dro_song()`) is refused.
        if self.editor.dro_song().is_some() {
            self.status = crate::strings::APP_STATUS_ONLY_VGM_OPTIMIZE.to_owned();
            return;
        }
        match self.editor.optimize_vgm(self.config.optimizer) {
            OptimizeVgmOutcome::Optimized { removed, saved } => {
                self.status = crate::strings::app_status_optimized(removed, saved);
                self.scroll_to = Some(table::ScrollTo::centered(0));
                self.after_edit();
            }
            OptimizeVgmOutcome::NothingToDo => {
                self.status = crate::strings::APP_STATUS_NOTHING_TO_OPTIMIZE.to_owned();
            }
            OptimizeVgmOutcome::KeptOriginal(reason) => {
                // Verified: the smaller file played differently, so the original
                // stands and the user is told why (D-orw-4).
                self.status = crate::strings::app_status_optimize_reverted(&reason);
            }
        }
    }

    fn on_redo(&mut self) {
        if self.active_tab == AppTab::Pack {
            self.redo_pack_edit();
        } else if self.require_document() {
            match self.editor.redo() {
                Some(description) => {
                    self.status = crate::strings::app_status_redone(&description);
                    self.after_edit();
                }
                None => self.status = crate::strings::APP_STATUS_NOTHING_TO_REDO.to_owned(),
            }
        }
    }

    fn on_save_vgm_metadata(
        &mut self,
        loop_point: Option<usize>,
        loop_end: Option<usize>,
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
    ) {
        let dropped = self.editor.set_vgm_metadata(
            loop_point,
            loop_end,
            loop_base,
            loop_modifier,
            volume_modifier,
        );
        // The stored loop is now the marked one, so re-arm playback.
        self.push_loop_config();
        if dropped {
            self.alerts.push_back(Alert::new(
                crate::strings::APP_LOOP_CLEARED_TITLE,
                crate::strings::APP_LOOP_CLEARED_BODY,
            ));
        } else {
            self.status = crate::strings::APP_STATUS_VGM_METADATA_UPDATED.to_owned();
        }
    }

    fn on_undo(&mut self) {
        // On the pack tab, Undo reverses the last file edit; on the editor
        // tab it reverses the last song edit.
        if self.active_tab == AppTab::Pack {
            self.undo_pack_edit();
        } else if self.require_document() {
            match self.editor.undo() {
                Some(description) => {
                    self.status = crate::strings::app_status_undone(&description);
                    self.after_edit();
                }
                None => self.status = crate::strings::APP_STATUS_NOTHING_TO_UNDO.to_owned(),
            }
        }
    }

    fn on_update_header(&mut self, opl_type: vgms_core::OplType, ms_length: u32) {
        self.editor.update_header(opl_type, ms_length);
        // The chip type may have changed the projection chips and the Original
        // pan policy; after_edit invalidates the audio revision, so the next
        // ensure_audio pushes the fresh panning. Feed the deck the *playback*
        // type (a stored DualOPL2 whose init block enables OPL3 plays as one
        // YMF262), computed after the header update and before touching
        // self.channels so the editor borrow is released (OplType is Copy).
        let playback = self.editor.dro_song().map(|song| song.playback_opl_type());
        if let Some(playback) = playback {
            self.channels.set_opl_type(playback);
        }
        self.after_edit();
    }

    /// File actions: open/save/close, quit, and the render and split exports.
    fn handle_file_action(&mut self, ctx: &egui::Context, action: FileAction) {
        match action {
            FileAction::Close => self.on_close_file(),
            FileAction::ConfirmClose => self.close_song(),
            FileAction::ConfirmDiscardAndLoad => {
                if let Some(file) = self.pending_load.take() {
                    self.load_file(file);
                }
            }
            FileAction::ConfirmExit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            FileAction::Exit => self.on_exit_requested(ctx),
            FileAction::Open => self.files.pick_open(),
            FileAction::OpenRenderWav => self.on_open_render_wav(),
            FileAction::OpenSplit => self.on_open_split(),
            FileAction::OpenSplitSongs => self.on_open_split_songs(),
            FileAction::RenderWavSubmitted {
                use_toggles,
                use_panning,
                boost,
                core_choices,
            } => self.render_to_wav(use_toggles, use_panning, boost, core_choices),
            FileAction::Save => self.save(false),
            FileAction::SaveAs => self.save(true),
            FileAction::SplitSongsPreview { start_index } => self.preview_segment(start_index),
            FileAction::SplitSongsSubmitted {
                threshold_native,
                included,
                trailing_tail,
            } => self.start_split_songs(threshold_native, included, trailing_tail),
            FileAction::SplitSubmitted {
                format,
                use_skip_muted,
                use_panning,
                boost,
                core_choices,
            } => self.start_split(format, use_skip_muted, use_panning, boost, core_choices),
        }
    }

    fn on_close_file(&mut self) {
        if !self.require_document() {
            return;
        }
        if self.editor.is_dirty() {
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_TITLE,
                crate::strings::APP_CONFIRM_CLOSE_FILE_BODY,
                Action::File(FileAction::ConfirmClose),
            ));
        } else {
            self.close_song();
        }
    }

    fn on_exit_requested(&mut self, ctx: &egui::Context) {
        if self.editor.is_dirty() || self.pack_is_dirty() {
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_TITLE,
                crate::strings::APP_CONFIRM_QUIT_BODY,
                Action::File(FileAction::ConfirmExit),
            ));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn on_open_render_wav(&mut self) {
        if self.require_renderable() {
            // Seed the per-render core picker from the document's chips
            // and the current Settings choices; the dialog edits its own
            // copy and never writes vgmstudio.ini.
            let chips = self.document_chips();
            self.dialogs.render_wav = Some(RenderWavDialog::new(
                self.config.audio.boost,
                chips,
                self.config.audio.cores.clone(),
            ));
        }
    }

    fn on_open_split(&mut self) {
        if !self.require_splittable() {
            return;
        }
        if self.split_is_running() {
            self.status = crate::strings::APP_STATUS_ALREADY_SPLITTING_CHANNELS.to_owned();
            return;
        }
        // An OPL document always offers the DroSong format; a generic VGM
        // offers it when a gate-covered chip lets a channel be rewritten.
        // Either way the split gets the per-render core picker, seeded
        // from Settings.
        let chips = self.document_chips();
        self.dialogs.split = Some(SplitDialog::new(
            self.editor.is_opl(),
            chips,
            self.config.audio.cores.clone(),
            self.config.audio.boost,
        ));
    }

    fn on_open_split_songs(&mut self) {
        if !self.require_document() {
            return;
        }
        if self.split_is_running() {
            self.status = crate::strings::APP_STATUS_ALREADY_SPLITTING.to_owned();
            return;
        }
        if let Some(source) = self.split_source() {
            self.dialogs.split_songs = Some(SplitSongsDialog::new(source));
        }
    }

    /// Loop actions: the marked region, loop playback, and the loop search.
    fn handle_loop_action(&mut self, action: LoopAction) {
        match action {
            LoopAction::ApplyToMetadata => self.apply_loop_to_metadata(),
            LoopAction::CancelSearch => {
                self.tasks.cancel(TaskKind::LoopSearch);
                self.status = crate::strings::APP_STATUS_LOOP_SEARCH_CANCELLED.to_owned();
            }
            LoopAction::ClearMarkers => {
                self.editor.markers = RangeMarkers::full(self.editor.len());
                self.push_loop_config();
                self.status = crate::strings::APP_STATUS_LOOP_RESET.to_owned();
            }
            LoopAction::CropToMarkers => self.on_crop_to_markers(),
            LoopAction::DeleteMarkedRegion => self.on_delete_marked_region(),
            LoopAction::OpenSearch => self.on_open_loop_search(),
            LoopAction::Search { min_len_commands } => self.start_loop_search(min_len_commands),
            LoopAction::SetCount(count) => {
                self.loop_count = count;
                self.push_loop_config();
            }
            LoopAction::SetEnd(index) => self.set_loop_marker(None, Some(index)),
            LoopAction::SetStart(index) => self.set_loop_marker(Some(index), None),
            LoopAction::TogglePlayback => {
                self.loop_enabled = !self.loop_enabled;
                self.push_loop_config();
            }
        }
    }

    fn on_crop_to_markers(&mut self) {
        if !self.require_document() {
            return;
        }
        match self.editor.crop_to_markers() {
            Some((kept, restored)) => {
                // The restored writes are instructions the user did not
                // put there, so they are worth accounting for -- but only
                // when there were any; a "0" reads as a puzzle.
                self.status = match restored {
                    0 => crate::strings::app_status_cropped(kept),
                    n => crate::strings::app_status_cropped_restored(kept, n),
                };
                self.after_region_edit();
            }
            None => self.status = crate::strings::APP_NOTHING_MARKED.to_owned(),
        }
    }

    fn on_delete_marked_region(&mut self) {
        if !self.require_document() {
            return;
        }
        match self.editor.delete_marked_region() {
            Some((removed, bridged)) => {
                self.status = match bridged {
                    0 => crate::strings::app_status_deleted(removed),
                    n => crate::strings::app_status_deleted_bridged(removed, n),
                };
                self.after_region_edit();
            }
            None => self.status = crate::strings::APP_NOTHING_MARKED.to_owned(),
        }
    }

    fn on_open_loop_search(&mut self) {
        // Either representation: the dialog wants a time per row and a
        // command density, both of which a VGM can give directly.
        let doc = match (self.editor.snapshot(), self.editor.vgm()) {
            (Some(song), _) => Some(crate::dialogs::LoopSearchDoc::from_song(&song)),
            (None, Some(file)) => Some(crate::dialogs::LoopSearchDoc::from_vgm(file)),
            (None, None) => None,
        };
        match doc {
            Some(doc) => self.dialogs.find_loop = Some(FindLoopDialog::new(doc)),
            None => self.status = crate::strings::APP_STATUS_OPEN_SONG_FIRST.to_owned(),
        }
    }

    /// Mixer actions: channel toggles, panning, and the volume lever.
    fn handle_mixer_action(&mut self, action: MixerAction) {
        match action {
            MixerAction::MatchVolume => self.match_volume(),
            MixerAction::MeasureVolumeModifier => self.measure_volume_modifier(),
            MixerAction::MutingChanged => self.push_muting(),
            MixerAction::PanningChanged => self.push_panning(),
            MixerAction::SetBoost { value, persist } => self.set_boost(value, persist),
            MixerAction::SetLockBoost(lock) => self.set_lock_boost(lock),
            MixerAction::ToggleChannel(channel) => {
                self.channels.toggle_selected_channel(channel);
                self.push_muting();
            }
            MixerAction::TrimChanged => self.push_trims(),
            MixerAction::VolumeFieldFocused(focused) => self.volume_field_editing = focused,
        }
    }

    /// Pack actions: the VGMRips submission project and its file operations.
    fn handle_pack_action(&mut self, action: PackAction) {
        match action {
            PackAction::AddScreenshot => {
                self.pending_screenshot = Some(ScreenshotPick::Add);
                self.files.pick_image();
            }
            PackAction::AddScreenshotAs {
                file_name,
                bytes,
                recompress,
            } => self.add_screenshot_as(&file_name, bytes, recompress),
            PackAction::ApplySuggestedModifiers { album } => self.apply_pack_modifiers(album),
            PackAction::BulkTagSubmitted { targets, overlay } => {
                self.bulk_tag_submitted(targets, *overlay);
            }
            PackAction::Close => self.on_close_pack(),
            PackAction::ConfirmClose => self.close_pack(),
            PackAction::ConfirmDeleteScreenshot(name) => self.delete_screenshot(&name),
            PackAction::ConfirmExportZip => self.export_pack_zip(true),
            PackAction::ConfirmOpenFolder => self.files.pick_folder(),
            PackAction::ConfirmOpenZip => {
                if let Some(file) = self.pending_zip.take() {
                    self.do_open_zip_pack(file);
                }
            }
            PackAction::ConvertDatesToHyphens => self.convert_pack_dates_to_hyphens(),
            PackAction::DeleteScreenshot(index) => self.confirm_delete_screenshot(index),
            PackAction::ExportZip => self.export_pack_zip(false),
            PackAction::FocusTrack(index) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.focused_track = Some(index);
                }
            }
            PackAction::MoveFocusedTrack { delta } => self.move_focused_pack_track(delta),
            PackAction::MoveTrack { index, delta } => self.move_pack_track(index, delta),
            PackAction::MoveTrackTo { from, to } => self.move_pack_track_to(from, to),
            PackAction::OpenBulkTag => self.open_bulk_tag(),
            PackAction::OpenFolder => self.on_open_pack_folder(),
            PackAction::OpenFolderAt(path) => self.files.open_folder_path(path),
            PackAction::OpenTrackQuickEdit(index) => self.open_track_quick_edit(index),
            PackAction::OptimizeTrack(index) => self.optimize_track(index),
            PackAction::OptimizeAllTracks => self.optimize_all_tracks(),
            // No dirty prompt here: the picked `.zip` comes back through
            // `load_or_confirm`, which raises it once the file is in hand --
            // asking before the picker would prompt even for a dismissed one.
            PackAction::OpenZip => self.files.pick_pack_zip(),
            PackAction::QuickEditSubmitted {
                original_name,
                file_name,
                tag,
            } => self.quick_edit_submitted(original_name, file_name, *tag),
            PackAction::RecompressImage(index) => self.recompress_image(index),
            PackAction::RenameFromTags => self.rename_pack_tracks_from_tags(),
            PackAction::RenameScreenshot {
                original_name,
                file_name,
            } => self.rename_screenshot(&original_name, &file_name),
            PackAction::RenameScreenshotAt(index) => self.open_screenshot_rename(index),
            PackAction::ReplaceScreenshot(index) => self.replace_screenshot(index),
            PackAction::SaveArchive => self.save_pack_archive(),
            PackAction::SaveDocs => self.save_pack_docs(),
            PackAction::ScanVolumes => self.scan_pack_volumes(),
            PackAction::SelectSection(section) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.section = section;
                }
            }
            PackAction::SelectTab(tab) => self.select_tab(tab),
            PackAction::StopPreview => self.stop_preview(),
            PackAction::TrackOpen(index) => self.open_track_in_editor(index),
            PackAction::TrackPreview(index) => self.preview_track(index),
        }
    }

    fn on_close_pack(&mut self) {
        if self.pack_is_dirty() {
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_PACK_TITLE,
                crate::strings::APP_CONFIRM_PACK_CLOSE_BODY,
                Action::Pack(PackAction::ConfirmClose),
            ));
        } else {
            self.close_pack();
        }
    }

    fn on_open_pack_folder(&mut self) {
        if self.pack_is_dirty() {
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_PACK_TITLE,
                crate::strings::APP_CONFIRM_PACK_OPEN_BODY,
                Action::Pack(PackAction::ConfirmOpenFolder),
            ));
        } else {
            self.files.pick_folder();
        }
    }

    /// Playback actions: transport, seeking, and row navigation.
    fn handle_playback_action(&mut self, action: PlaybackAction) {
        match action {
            PlaybackAction::NextDelay => self.delay_navigate(false),
            PlaybackAction::Play => self.do_play(),
            PlaybackAction::PlaySeam => self.do_play_seam(),
            PlaybackAction::PlayTail => self.do_play_tail(),
            PlaybackAction::PreviousDelay => self.delay_navigate(true),
            PlaybackAction::RewindToStart => self.on_rewind_to_start(),
            PlaybackAction::SelectionMove { delta, extend } => {
                self.on_selection_move(delta, extend);
            }
            PlaybackAction::Stop => self.do_stop(),
            PlaybackAction::TogglePlayback => self.on_toggle_playback(),
            PlaybackAction::WaveformClicked { index, ms } => self.on_waveform_clicked(index, ms),
        }
    }

    fn on_rewind_to_start(&mut self) {
        // Restart live playback from the top; snap the cursor and the
        // readout to zero whether or not anything is playing.
        self.audio.rewind();
        self.waveform.cursor_ms = 0;
        self.editor.selection.select_only(0);
        if self.audio.is_playing() {
            self.audio.seek_ms(0);
        }
        self.position.set_position_ms(0);
    }

    fn on_selection_move(&mut self, delta: isize, extend: bool) {
        if let Some(row) = self
            .editor
            .selection
            .key_move(delta, extend, self.editor.len())
        {
            // Stepping into a folded run expands it, so the moved selection stays
            // on a row the user can see rather than vanishing under a summary.
            self.editor.reveal(row);
            self.scroll_to = Some(table::ScrollTo::centered(row));
        }
    }

    fn on_toggle_playback(&mut self) {
        if !self.require_playable() {
            return;
        }
        if self.audio.is_playing() {
            self.do_stop();
        } else {
            self.do_play();
        }
    }

    fn on_waveform_clicked(&mut self, index: usize, ms: u32) {
        self.editor.reveal(index);
        self.editor.selection.select_only(index);
        // Bring the table to where playback would start, that row at the
        // top: the click says "play from here", so what follows it is
        // what the user wants to read -- not the rows before it, which
        // is what centring would spend half the view on.
        self.scroll_to = Some(table::ScrollTo::to_top(index));
        if self.audio.is_playing() {
            // The click already carries the row's time; seek by it (the
            // engine addresses ms, not row index -- ou-2).
            self.audio.seek_ms(ms);
        }
        self.position.set_position_ms(ms);
    }

    /// Settings actions: the dialog, saving it, and its live previews.
    fn handle_settings_action(&mut self, ctx: &egui::Context, action: SettingsAction) {
        match action {
            SettingsAction::Apply(config) => self.apply_settings(ctx, *config),
            SettingsAction::Open => self.on_open_settings(),
            SettingsAction::PreviewCores(cores) => self.preview_cores(cores),
            SettingsAction::PreviewResampling(mode) => self.preview_resampling(mode),
            SettingsAction::PreviewSkin { theme, pad_style } => {
                self.preview_skin(ctx, theme, pad_style);
            }
        }
    }

    fn on_open_settings(&mut self) {
        // Listed at open, so the picker offers what is plugged in now.
        let mut dialog = SettingsDialog::new(&self.config, self.audio.list_hardware_ports());
        // Hand the dialog the loaded file's chips, so its Output tab can
        // surface the cores actually in use before the rest of the roster.
        if let Some(song) = self.settings_song_context() {
            dialog = dialog.with_song(song);
        }
        self.dialogs.settings = Some(dialog);
    }

    /// App-chrome actions: message boxes, the status bar, and the Help menu.
    fn handle_ui_action(&mut self, action: UiAction) {
        match action {
            UiAction::About => self.alerts.push_back(Alert::new("About", about_text())),
            UiAction::Alert { title, message } => self.alerts.push_back(Alert::new(title, message)),
            UiAction::Help => self.dialogs.help = Some(HelpDialog),
            UiAction::Status(message) => self.status = message,
        }
    }
}
