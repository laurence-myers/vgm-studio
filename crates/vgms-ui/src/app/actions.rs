use super::*;

impl VgmStudioApp {
    // -- actions ---------------------------------------------------------

    pub(super) fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenFile => self.files.pick_open(),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::CloseFile => {
                if !self.require_document() {
                    return;
                }
                if self.editor.is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        crate::strings::APP_CONFIRM_DISCARD_TITLE,
                        crate::strings::APP_CONFIRM_CLOSE_FILE_BODY,
                        Action::ConfirmCloseFile,
                    ));
                } else {
                    self.close_song();
                }
            }
            Action::ConfirmCloseFile => self.close_song(),
            Action::OpenRenderWav => {
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
            Action::RenderWavSubmitted {
                use_toggles,
                use_panning,
                boost,
                core_choices,
            } => self.render_to_wav(use_toggles, use_panning, boost, core_choices),
            Action::OpenSplit => {
                if !self.require_splittable() {
                    return;
                }
                if self.split_is_running() {
                    self.status = crate::strings::APP_STATUS_ALREADY_SPLITTING_CHANNELS.to_owned();
                    return;
                }
                // An OPL document always offers the Song format; a generic VGM
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
            Action::SplitSubmitted {
                format,
                use_skip_muted,
                use_panning,
                boost,
                core_choices,
            } => self.start_split(format, use_skip_muted, use_panning, boost, core_choices),
            Action::OpenSplitSongs => {
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
            Action::SplitSongsSubmitted {
                threshold_native,
                included,
                trailing_tail,
            } => self.start_split_songs(threshold_native, included, trailing_tail),
            Action::SplitSongsPreview { start_index } => self.preview_segment(start_index),
            Action::Exit => {
                if self.editor.is_dirty() || self.pack_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        crate::strings::APP_CONFIRM_DISCARD_TITLE,
                        crate::strings::APP_CONFIRM_QUIT_BODY,
                        Action::ConfirmExit,
                    ));
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::ConfirmExit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::ConfirmDiscardAndLoad => {
                if let Some(file) = self.pending_load.take() {
                    self.load_file(file);
                }
            }

            Action::Undo => {
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
            Action::Redo => {
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
            Action::OpenGoto => {
                if self.require_document() {
                    self.dialogs.goto = Some(GotoDialog::new());
                }
            }
            Action::OpenFindRegister => {
                // Either document kind: an OPL song gets the token/register
                // list, any other VGM the chip picker.
                self.dialogs.find_reg = match (self.editor.song(), self.editor.vgm()) {
                    (Some(song), _) => Some(FindRegDialog::new(song)),
                    (None, Some(file)) => Some(FindRegDialog::for_vgm(file)),
                    (None, None) => {
                        self.require_document();
                        None
                    }
                };
            }
            Action::OpenFindLoop => {
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
            Action::OpenDroInfo => {
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
                    let song = self.editor.song().expect("gated -- a DRO");
                    let edit_allowed = self.config.ui.dro_info_edit_enabled;
                    self.dialogs.dro_info = Some(DroInfoDialog::new(song, edit_allowed));
                }
            }
            Action::OpenEditTag => {
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
            Action::OpenVgmMetadata => {
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
            Action::ConvertToVgm => {
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
            Action::ConvertToDro1 => {
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
            Action::DeleteSelection => {
                if !self.require_document() {
                    return;
                }
                if self.editor.delete_selection() {
                    self.scroll_to = self.editor.selection.first().map(table::ScrollTo::centered);
                    self.after_edit();
                }
            }
            Action::AuditHeader => self.audit_header(),
            Action::ConfirmFixHeader => {
                let fixed = self.editor.fix_header();
                self.status = match fixed {
                    0 => crate::strings::APP_STATUS_HEADER_AGREES.to_owned(),
                    1 => crate::strings::APP_STATUS_HEADER_FIXED_ONE.to_owned(),
                    count => crate::strings::app_status_header_fixed(count),
                };
            }
            Action::OptimizeVgm => {
                if !self.editor.has_document() {
                    self.status = crate::strings::APP_STATUS_OPEN_SONG_FIRST.to_owned();
                    return;
                }
                // Optimize is VGM-only; an editable DRO (`editor.song()`) is refused.
                if self.editor.song().is_some() {
                    self.status = crate::strings::APP_STATUS_ONLY_VGM_OPTIMIZE.to_owned();
                    return;
                }
                match self.editor.optimize_vgm(self.config.optimizer) {
                    Some((commands, bytes)) => {
                        self.status = crate::strings::app_status_optimized(commands, bytes);
                        self.scroll_to = Some(table::ScrollTo::centered(0));
                        self.after_edit();
                    }
                    None => self.status = crate::strings::APP_STATUS_NOTHING_TO_OPTIMIZE.to_owned(),
                }
            }

            Action::OpenPackFolder => {
                if self.pack_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        crate::strings::APP_CONFIRM_DISCARD_PACK_TITLE,
                        crate::strings::APP_CONFIRM_PACK_OPEN_BODY,
                        Action::ConfirmOpenPackFolder,
                    ));
                } else {
                    self.files.pick_folder();
                }
            }
            Action::ConfirmOpenPackFolder => self.files.pick_folder(),
            Action::OpenPackFolderAt(path) => self.files.open_folder_path(path),
            // No dirty prompt here: the picked `.zip` comes back through
            // `load_or_confirm`, which raises it once the file is in hand --
            // asking before the picker would prompt even for a dismissed one.
            Action::OpenPackZip => self.files.pick_pack_zip(),
            Action::SelectTab(tab) => self.select_tab(tab),
            Action::ClosePack => {
                if self.pack_is_dirty() {
                    self.alerts.push_back(Alert::confirm(
                        crate::strings::APP_CONFIRM_DISCARD_PACK_TITLE,
                        crate::strings::APP_CONFIRM_PACK_CLOSE_BODY,
                        Action::ConfirmClosePack,
                    ));
                } else {
                    self.close_pack();
                }
            }
            Action::ConfirmClosePack => self.close_pack(),
            Action::PackSaveDocs => self.save_pack_docs(),
            Action::PackScanVolumes => self.scan_pack_volumes(),
            Action::PackApplySuggestedModifiers { album } => self.apply_pack_modifiers(album),
            Action::PackConvertDatesToHyphens => self.convert_pack_dates_to_hyphens(),
            Action::PackRenameFromTags => self.rename_pack_tracks_from_tags(),
            Action::PackSelectSection(section) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.section = section;
                }
            }
            Action::PackAddScreenshot => {
                self.pending_screenshot = Some(ScreenshotPick::Add);
                self.files.pick_image();
            }
            Action::PackReplaceScreenshot(index) => self.replace_screenshot(index),
            Action::PackRenameScreenshotAt(index) => self.open_screenshot_rename(index),
            Action::PackAddScreenshotAs {
                file_name,
                bytes,
                recompress,
            } => self.add_screenshot_as(&file_name, bytes, recompress),
            Action::PackRenameScreenshot {
                original_name,
                file_name,
            } => self.rename_screenshot(&original_name, &file_name),
            Action::PackDeleteScreenshot(index) => self.confirm_delete_screenshot(index),
            Action::ConfirmDeleteScreenshot(name) => self.delete_screenshot(&name),
            Action::PackExportZip => self.export_pack_zip(false),
            Action::ConfirmExportZip => self.export_pack_zip(true),
            Action::PackSaveArchive => self.save_pack_archive(),
            Action::ConfirmOpenZipPack => {
                if let Some(file) = self.pending_zip.take() {
                    self.do_open_zip_pack(file);
                }
            }
            Action::PackTrackOpen(index) => self.open_track_in_editor(index),
            Action::PackTrackPreview(index) => self.preview_track(index),
            Action::PackStopPreview => self.stop_preview(),
            Action::OpenTrackQuickEdit(index) => self.open_track_quick_edit(index),
            Action::PackMoveTrack { index, delta } => self.move_pack_track(index, delta),
            Action::PackMoveTrackTo { from, to } => self.move_pack_track_to(from, to),
            Action::PackFocusTrack(index) => {
                if let Some(pack) = self.pack.as_mut() {
                    pack.focused_track = Some(index);
                }
            }
            Action::PackMoveFocusedTrack { delta } => self.move_focused_pack_track(delta),
            Action::RecompressImage(index) => self.recompress_image(index),
            Action::QuickEditSubmitted {
                original_name,
                file_name,
                tag,
            } => self.quick_edit_submitted(original_name, file_name, *tag),
            Action::OpenBulkTag => self.open_bulk_tag(),
            Action::BulkTagSubmitted { targets, overlay } => {
                self.bulk_tag_submitted(targets, *overlay);
            }

            Action::Play => self.do_play(),
            Action::Stop => self.do_stop(),
            Action::PlayTail => self.do_play_tail(),
            Action::PlaySeam => self.do_play_seam(),
            Action::TogglePlayback => {
                if !self.require_playable() {
                    return;
                }
                if self.audio.is_playing() {
                    self.do_stop();
                } else {
                    self.do_play();
                }
            }

            Action::NextDelay => self.delay_navigate(false),
            Action::PreviousDelay => self.delay_navigate(true),
            Action::SelectionMove { delta, extend } => {
                if let Some(row) = self
                    .editor
                    .selection
                    .key_move(delta, extend, self.editor.len())
                {
                    self.scroll_to = Some(table::ScrollTo::centered(row));
                }
            }
            Action::WaveformClicked { index, ms } => {
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
            Action::RewindToStart => {
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

            Action::SetLoopStart(index) => self.set_loop_marker(Some(index), None),
            Action::SetLoopEnd(index) => self.set_loop_marker(None, Some(index)),
            Action::ClearLoopMarkers => {
                self.editor.markers = RangeMarkers::full(self.editor.len());
                self.push_loop_config();
                self.status = crate::strings::APP_STATUS_LOOP_RESET.to_owned();
            }
            Action::ToggleLoopPlayback => {
                self.loop_enabled = !self.loop_enabled;
                self.push_loop_config();
            }
            Action::SetLoopCount(count) => {
                self.loop_count = count;
                self.push_loop_config();
            }
            Action::ApplyLoopToMetadata => self.apply_loop_to_metadata(),
            Action::CropToMarkers => {
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
            Action::DeleteMarkedRegion => {
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
            Action::FindLoopSearch { min_len_commands } => self.start_loop_search(min_len_commands),
            Action::CancelLoopSearch => {
                self.tasks.cancel(TaskKind::LoopSearch);
                self.status = crate::strings::APP_STATUS_LOOP_SEARCH_CANCELLED.to_owned();
            }

            Action::ToggleChannel(channel) => {
                self.channels.toggle_selected_channel(channel);
                self.push_muting();
            }
            Action::MutingChanged => self.push_muting(),
            Action::PanningChanged => self.push_panning(),
            Action::SetBoost { value, persist } => self.set_boost(value, persist),
            Action::SetLockBoost(lock) => self.set_lock_boost(lock),
            Action::MatchVolume => self.match_volume(),
            Action::MeasureVolumeModifier => self.measure_volume_modifier(),
            Action::VolumeFieldFocused(focused) => self.volume_field_editing = focused,

            Action::GotoSubmitted(text) => self.goto_submitted(&text),
            Action::FindRegister { query, backwards } => self.find_register(&query, backwards),
            Action::UpdateHeader {
                opl_type,
                ms_length,
            } => {
                self.editor.update_header(opl_type, ms_length);
                // The chip type may have changed the high-bank visibility and the
                // Original pan policy; after_edit invalidates the audio revision,
                // so the next ensure_audio pushes the fresh panning.
                self.channels.set_opl_type(opl_type, self.editor.song());
                self.after_edit();
            }
            Action::SaveGd3(tag) => self.editor.set_gd3_tag(*tag),
            Action::SaveVgmMetadata {
                loop_point,
                loop_end,
                loop_base,
                loop_modifier,
                volume_modifier,
            } => {
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
            Action::Settings(action) => self.handle_settings_action(ctx, action),
            Action::Ui(action) => self.handle_ui_action(action),
        }
    }

    /// Settings actions: the dialog, saving it, and its live previews.
    fn handle_settings_action(&mut self, ctx: &egui::Context, action: SettingsAction) {
        match action {
            SettingsAction::Apply(config) => self.apply_settings(ctx, *config),
            SettingsAction::Open => self.on_open_settings(),
            SettingsAction::PreviewCores(cores) => self.preview_cores(cores),
            SettingsAction::PreviewResampling(mode) => self.preview_resampling(mode),
            SettingsAction::PreviewSkin {
                theme,
                pad_style,
                deck_style,
            } => self.preview_skin(ctx, theme, pad_style, deck_style),
        }
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
}
