use super::*;

impl VgmStudioApp {
    pub(super) fn update_impl(&mut self, ui: &mut egui::Ui) {
        // The panels carve up `ui`; everything context-wide (input, dialogs,
        // repaint scheduling) still wants a `Context`, which is cheaply Arc-cloned.
        let ctx = ui.ctx().clone();
        self.intercept_close(&ctx);
        if let Some(file) = self.pending_open.take() {
            self.load_file(file);
        }
        self.poll_services();
        self.handle_drops(&ctx);
        self.sync_window_title(&ctx);

        let mut actions: Vec<Action> = Vec::new();
        // Actions injected by the e2e hook run first, through the same handler the
        // UI feeds. Compiled out of release builds.
        #[cfg(any(test, feature = "e2e"))]
        actions.extend(self.e2e_actions.drain(..));
        // OS media-transport keys posted from outside the UI loop (native SMTC,
        // web media session) run through the same transport path a button does.
        actions.extend(self.media_keys.take_actions());
        self.gather_key_input(&ctx, &mut actions);

        let active_palette = self.palette();
        let p = &active_palette;
        // Chrome panels are fascia plates: a transparent frame, with the plate
        // gradient painted behind the content inside each panel (see the
        // `theme::plate` calls below). The waveform is a data well, so its
        // margins take the main dark background rather than the chrome tint.
        let chrome = egui::Frame::side_top_panel(ui.style()).fill(egui::Color32::TRANSPARENT);
        // No side margins: the reset button owns the left edge and the waveform
        // runs flush to the right edge.
        let well = egui::Frame::side_top_panel(ui.style())
            .fill(p.data_bg)
            .inner_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 2,
                bottom: 2,
            });

        let menu = egui::Panel::top("menu-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::plate_panel(ui, p, |ui| {
                    menus::bar(ui, p, &self.menu_state(), &mut actions);
                });
            });
        // The tab strip switches the editor and pack views. It is always present,
        // so the app keeps one shape; Pack is simply greyed until a pack project
        // is open, which says the view exists rather than hiding it.
        let tabs = egui::Panel::top("tab-strip")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::plate_panel(ui, p, |ui| {
                    const VIEWS: [AppTab; 2] = [AppTab::Editor, AppTab::Pack];
                    let strip = [
                        theme::tabs::Tab::new("Editor"),
                        theme::tabs::Tab::new("Pack").enabled(self.pack.is_some()),
                    ];
                    let selected = VIEWS.iter().position(|t| *t == self.active_tab);
                    if let Some(i) = theme::tabs::strip(ui, p, &strip, selected.unwrap_or(0)) {
                        actions.push(Action::Pack(PackAction::SelectTab(VIEWS[i])));
                    }
                });
            });
        // The editor-only panels (waveform, transport/boost, position) are hidden
        // on the pack tab, which owns the whole central area.
        let editor_tab = self.active_tab == AppTab::Editor;
        // A VGM for other chips has no OPL stream, so the panels that exist to show or
        // drive audio have nothing to say about it. They go, rather than sit
        // there as a dead transport over a permanently flat waveform.
        let playable = self.editor.capabilities().playable;
        let audio_panels = editor_tab && playable;
        let waveform = audio_panels.then(|| {
            egui::Panel::top("waveform")
                .frame(well)
                .resizable(true)
                .default_size(150.0)
                .min_size(80.0)
                .show_separator_line(false)
                .show(ui, |ui| {
                    let height = ui.available_height();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        // A full-height "skip to start" transport button on the left.
                        if theme::bevel::button_sized(ui, p, "\u{23EE}", egui::vec2(34.0, height))
                            .on_hover_text(crate::strings::APP_TIP_REWIND)
                            .clicked()
                        {
                            actions.push(Action::Playback(PlaybackAction::RewindToStart));
                        }
                        // Hardware output sends no samples through this program,
                        // so there is nothing to meter -- and a meter pinned at
                        // silence through a whole song reads as a fault. Drop it
                        // and give the waveform the room.
                        let metered = self.output_renders_samples();
                        // Reserve the peak meter's width up front: the waveform
                        // fills whatever space it is given.
                        let wave_width = if metered {
                            ui.available_width() - peak_meter::WIDTH - ui.spacing().item_spacing.x
                        } else {
                            ui.available_width()
                        };
                        ui.allocate_ui(egui::vec2(wave_width, height), |ui| {
                            let response =
                                waveform::show(ui, &self.waveform, self.editor.timeline(), p);
                            if let Some((index, ms)) = response.clicked {
                                actions.extend(waveform_action(
                                    index,
                                    ms,
                                    response.secondary,
                                    response.modifiers.shift,
                                ));
                            }
                        });
                        if metered {
                            peak_meter::show(ui, &self.peak_meter, p);
                        }
                    });
                })
        });
        let status = egui::Panel::bottom("status-bar")
            .frame(chrome)
            .show_separator_line(false)
            .show(ui, |ui| {
                theme::plate_panel(ui, p, |ui| {
                    // A brief amber wash over the row when the status text
                    // changes, so a new message catches the eye. The slot is
                    // claimed before the row so the wash paints under the text;
                    // egui's animation drives the decay (and its repaints).
                    let flash_slot = ui.painter().add(egui::Shape::Noop);
                    if self.status != self.status_shown {
                        self.status_shown.clone_from(&self.status);
                        ui.ctx()
                            .animate_value_with_time(egui::Id::new("status-flash"), 1.0, 0.0);
                    }
                    let flash = ui.ctx().animate_value_with_time(
                        egui::Id::new("status-flash"),
                        0.0,
                        STATUS_FLASH_SECS,
                    );
                    ui.horizontal(|ui| {
                        // Truncate rather than run off the right edge, and reveal
                        // the whole message on hover -- some statuses (a crop
                        // summary, a saved path) are longer than the bar is wide.
                        let status = ui.add(egui::Label::new(&self.status).truncate());
                        if !self.status.is_empty() {
                            status.on_hover_text(&self.status);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Each busy label carries the spinner, so a long job
                            // visibly lives (the busy repaint cadence comes from
                            // `playback_tick`'s 100ms request while tasks run).
                            let spin = spinner_frame(ui.input(|input| input.time));
                            // A per-track optimise counts its tracks off, like the
                            // volume scan; anything else busy just shows liveness.
                            if let Some((done, total)) = self.song_optimize_progress {
                                ui.label(format!(
                                    "{spin} {}",
                                    crate::strings::app_busy_optimizing_tracks(done, total)
                                ));
                            } else if self.pack_service.is_busy() {
                                // The status text names the operation (export or a
                                // screenshot optimise); this just shows liveness.
                                ui.label(format!("{spin} {}", crate::strings::APP_BUSY_WORKING));
                            }
                            // Name the job rather than just "busy": a WAV render can
                            // take a while, and the waveform's own render runs after
                            // every edit.
                            if self.tasks.is_busy_kind(TaskKind::RenderWav) {
                                ui.label(format!("{spin} {}", crate::strings::APP_BUSY_RENDER_WAV));
                            }
                            if self.tasks.is_busy_kind(TaskKind::Split) {
                                ui.label(format!(
                                    "{spin} {}",
                                    crate::strings::APP_STATUS_SPLITTING_CHANNELS
                                ));
                            }
                            if self.tasks.is_busy_kind(TaskKind::RenderWaveform) {
                                ui.label(format!(
                                    "{spin} {}",
                                    crate::strings::APP_BUSY_RENDER_WAVEFORM
                                ));
                            }
                            // Scanning a pack's volumes counts its tracks off, so
                            // the user sees it advance rather than just spin.
                            if self.tasks.is_busy_kind(TaskKind::PackVolumeScan) {
                                let (done, total) = self.pack_scan_progress.unwrap_or((0, 0));
                                ui.label(format!(
                                    "{spin} {}",
                                    crate::strings::app_busy_scanning_volumes(done, total)
                                ));
                            }
                        });
                    });
                    if flash > 0.0 {
                        let rect = ui.min_rect().expand2(egui::vec2(0.0, 2.0));
                        ui.painter().set(
                            flash_slot,
                            egui::Shape::rect_filled(
                                rect,
                                0.0,
                                p.latch_bottom.gamma_multiply(flash * 0.25),
                            ),
                        );
                    }
                });
            });
        let position = audio_panels.then(|| {
            egui::Panel::bottom("position-panel")
                .frame(chrome)
                .show_separator_line(false)
                .show(ui, |ui| {
                    theme::plate_panel(ui, p, |ui| {
                        self.position.show(ui, p);
                    });
                })
        });
        let controls = editor_tab.then(|| {
            // The controls own their vertical spacing (equal padding above and
            // below each row band), so drop the frame's vertical margin/spacing.
            let controls_frame = egui::Frame::side_top_panel(ui.style())
                .fill(egui::Color32::TRANSPARENT)
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 0,
                });
            egui::Panel::bottom("controls")
                .frame(controls_frame)
                .show_separator_line(false)
                .show(ui, |ui| {
                    theme::deck_panel(ui, p, |ui| {
                        const PAD: f32 = 6.0;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(PAD);
                        ui.horizontal(|ui| {
                            ui.set_min_height(ui.spacing().interact_size.y);
                            ui.spacing_mut().item_spacing.x = 12.0;
                            if theme::bevel::icon_button(ui, p, theme::icon::Icon::Del, "Del.")
                                .on_hover_text(crate::strings::APP_TIP_DELETE)
                                .clicked()
                            {
                                actions.push(Action::Edit(EditAction::DeleteSelection));
                            }
                            // Delete applies to any document; everything
                            // after it drives playback, which needs a stream
                            // this app can render.
                            if playable {
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Play, "Play")
                                    .on_hover_text(crate::strings::APP_TIP_PLAY)
                                    .clicked()
                                {
                                    actions.push(Action::Playback(PlaybackAction::Play));
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Stop, "Stop")
                                    .on_hover_text(crate::strings::APP_TIP_STOP)
                                    .clicked()
                                {
                                    actions.push(Action::Playback(PlaybackAction::Stop));
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Tail, "Tail")
                                    .on_hover_text(self.play_tail_label())
                                    .clicked()
                                {
                                    actions.push(Action::Playback(PlaybackAction::PlayTail));
                                }
                                if theme::bevel::icon_button(ui, p, theme::icon::Icon::Seam, "Seam")
                                    .on_hover_text(self.play_seam_label())
                                    .clicked()
                                {
                                    actions.push(Action::Playback(PlaybackAction::PlaySeam));
                                }
                                let mut looping = self.loop_enabled;
                                if theme::bevel::icon_toggle(
                                    ui,
                                    p,
                                    &mut looping,
                                    theme::icon::Icon::Loop,
                                    "Loop",
                                )
                                .on_hover_text(crate::strings::APP_TIP_LOOP)
                                .clicked()
                                {
                                    actions.push(Action::Loop(LoopAction::TogglePlayback));
                                }
                                loop_stepper::loop_count_stepper(
                                    ui,
                                    p,
                                    self.loop_count,
                                    &mut actions,
                                );
                                // The "Loop N/M" indicator sits right beside the
                                // loop controls it describes (it used to live in
                                // the position panel, which now hides its sample
                                // counter while a loop wraps).
                                if let Some((iteration, count)) = self.loop_progress {
                                    ui.label(egui::RichText::new(
                                        crate::strings::position_panel_loop_progress(
                                            iteration, count,
                                        ),
                                    ));
                                }
                                // The boost is applied to rendered samples, of which
                                // hardware output produces none -- the board has its
                                // own volume.
                                let shapes_output = self.output_renders_samples();
                                ui.add_enabled_ui(shapes_output, |ui| {
                                    boost_stepper::boost_stepper(
                                        ui,
                                        p,
                                        self.config.audio.boost,
                                        self.boost_ceiling,
                                        self.config.audio.lock_boost,
                                        self.tasks.is_busy_kind(TaskKind::VolumeScan),
                                        &mut actions,
                                    );
                                });
                            }
                        });
                        ui.add_space(PAD);
                        theme::separator_full(ui, p);
                        ui.add_space(PAD);
                        self.chip_deck(ui, p, &mut actions);
                        ui.add_space(PAD);
                    });
                })
        });
        // The pack view's output deck: the readiness lamp and everything that
        // turns the folder into a submission, pinned to the foot of the window
        // so they stay reachable however far the form and track list scroll.
        // The editor's transport deck occupies the same slot on the other tab.
        let pack_deck = (!editor_tab && self.pack.is_some()).then(|| {
            let deck_frame = egui::Frame::side_top_panel(ui.style())
                .fill(egui::Color32::TRANSPARENT)
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 0,
                });
            egui::Panel::bottom("pack-deck")
                .frame(deck_frame)
                .show_separator_line(false)
                .show(ui, |ui| {
                    theme::deck_panel(ui, p, |ui| {
                        const PAD: f32 = 6.0;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.add_space(PAD);
                        if let Some(pack) = self.pack.as_mut() {
                            crate::pack::deck(ui, pack, p, &mut actions);
                        }
                        ui.add_space(PAD);
                    });
                })
        });
        // The editor's central panel is one big data well; the pack view sits on
        // the FT2 desktop tint, with its own sunken wells inside.
        let central_fill = if editor_tab { p.data_bg } else { p.desktop };
        let central = egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(central_fill))
            .show(ui, |ui| match self.active_tab {
                AppTab::Editor => {
                    if self.editor.has_document() {
                        // Row hover reads `widgets.hovered.bg_fill`, which is the
                        // bright face colour; scope it to the data-well tone so it
                        // does not flash teal under the yellow text.
                        ui.visuals_mut().widgets.hovered.bg_fill = p.data_hover;
                        table::show(ui, &mut self.editor, self.scroll_to.take(), p);
                    } else {
                        ui.visuals_mut().override_text_color = Some(p.data_label);
                        ui.centered_and_justified(|ui| {
                            ui.label(crate::strings::APP_EMPTY_STATE);
                        });
                    }
                }
                AppTab::Pack => {
                    let scanning = self.tasks.is_busy_kind(TaskKind::PackVolumeScan);
                    // Whichever the user last touched is in charge: moving the
                    // pointer hands the row back to hover, so the keyboard's lit
                    // row does not linger under a mouse that has moved on.
                    // The event, not `pointer.is_moving()`: that reads a velocity
                    // averaged over a few frames, which a single deliberate move
                    // can leave at zero.
                    let pointer_moved = ui.input(|input| {
                        input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::PointerMoved(_)))
                    });
                    let optimizing = self.song_optimize_progress.is_some()
                        || !self.pending_song_optimize.is_empty();
                    if let Some(pack) = self.pack.as_mut() {
                        if pointer_moved {
                            pack.focused_track = None;
                        }
                        crate::pack::show(ui, pack, p, scanning, optimizing, &mut actions);
                    }
                }
            });
        // While the OS hovers a file over the window, the central well invites
        // the drop.
        drop_target(&ctx, p, central.response.rect, self.dialogs.any_open());

        // 2px beveled grooves at the seams between the stacked panels. Painted
        // into the shared background layer *after* the panels, so they sit over
        // the panel content but below every Window/menu/popup (which live in
        // higher orders) -- an ad-hoc Middle layer would draw over dialogs. The
        // waveform panel is resizable, so the seams are recomputed each frame.
        let divider = ctx.layer_painter(egui::LayerId::background());
        let x_range = ctx.viewport_rect().x_range();
        // Only the panels actually drawn this frame contribute a seam.
        let mut seams = vec![menu.response.rect.bottom(), tabs.response.rect.bottom()];
        if let Some(waveform) = &waveform {
            seams.push(waveform.response.rect.bottom());
        }
        if let Some(controls) = &controls {
            seams.push(controls.response.rect.top());
        }
        if let Some(position) = &position {
            seams.push(position.response.rect.top());
        }
        if let Some(pack_deck) = &pack_deck {
            seams.push(pack_deck.response.rect.top());
        }
        seams.push(status.response.rect.top());
        for seam in seams {
            theme::bevel::groove_h(&divider, x_range, seam - 1.0, p);
        }

        // Keep the modeless dialogs off the menu bar and tab strip: egui's top
        // panels no longer reserve context space, so an unconstrained window
        // auto-places at the top of the viewport.
        let chrome_bottom = tabs.response.rect.bottom();
        let dialog_area = egui::Rect::from_min_max(
            egui::pos2(ctx.content_rect().left(), chrome_bottom),
            ctx.content_rect().max,
        );
        self.dialogs.show_all(&ctx, p, dialog_area, &mut actions);
        alert::show_front(&ctx, p, &mut self.alerts, &mut actions);

        for action in actions {
            self.handle_action(&ctx, action);
        }

        self.sync_selection_indicator();
        // Cheap, and derived from three separate pieces of state (markers, the
        // loop toggle, the song's stored loop), so it is refreshed per frame
        // rather than at each of the places any of them can change.
        self.sync_loop_overlay();
        self.playback_tick(&ctx);
    }

    /// Draws the chip mixer deck: the selector strip (always) with its fold
    /// icon, and -- while unfolded -- the selected chip's controls, with the
    /// capability answers the panels need. Folded by default.
    fn chip_deck(&mut self, ui: &mut egui::Ui, p: &Palette, actions: &mut Vec<Action>) {
        // Whether the selected chip's pan controls should be
        // drawn: for the OPL panel, the output must render
        // samples (hardware mixes its own) and the chosen OPL
        // core must pan (CQM and OPL2-Lite cannot); for a
        // generic chip, its core must pan. Computed before the
        // panel's mutable borrow so the closure captures a bool,
        // not `self`.
        //
        // Key off the chip the DRO's OPL type projects to
        // (Ym3812 for OPL2/dual, Ymf262 for OPL3), not a fixed
        // YMF262: an OPL2-only core (the YM3812 die sim) is not
        // registered for the YMF262, so asking about the YMF262
        // would resolve to the default OPL3 core and wrongly
        // report an OPL2 song pannable.
        let opl_projection = self
            .editor
            .dro_song()
            .map_or(vgms_core::vgm::ChipKind::Ymf262, |song| {
                vgms_synth::opl_projection_kind(song.playback_opl_type())
            });
        let opl_can_pan =
            self.output_renders_samples() && vgms_synth::registry().pan_capable(opl_projection);
        let pan_supported = move |chip: Option<vgms_core::vgm::ChipKind>| match chip {
            None => opl_can_pan,
            Some(kind) => vgms_synth::registry().pan_capable(kind),
        };
        // Muting: an OPL document is always mutable (register
        // gating); a generic chip only when its resolved core
        // honours channel mutes (the Nuked family does not).
        let mute_supported = move |chip: Option<vgms_core::vgm::ChipKind>| match chip {
            None => true,
            Some(kind) => vgms_synth::registry().mute_capable(kind),
        };
        // The panel hides its own high bank for a plain OPL2 song.
        let channels = self.channels.show(
            ui,
            p,
            &mut self.chips_expanded,
            pan_supported,
            mute_supported,
        );
        if channels.muting_changed {
            actions.push(Action::Mixer(MixerAction::MutingChanged));
        }
        if channels.panning_changed {
            actions.push(Action::Mixer(MixerAction::PanningChanged));
        }
        if channels.trim_changed {
            actions.push(Action::Mixer(MixerAction::TrimChanged));
        }
    }

    // -- frame plumbing ------------------------------------------------------

    pub(super) fn poll_services(&mut self) {
        if let Some(result) = self.files.poll_picked() {
            match result {
                Ok(file) => self.load_or_confirm(file),
                Err(message) => self
                    .alerts
                    .push_back(Alert::new(crate::strings::APP_ERR_OPEN_FILE_TITLE, message)),
            }
        }
        if let Some(result) = self.files.poll_picked_image() {
            match result {
                Ok(file) => self.add_screenshot(file),
                Err(message) => self.alerts.push_back(Alert::new(
                    crate::strings::APP_ERR_READ_IMAGE_TITLE,
                    message,
                )),
            }
        }
        if let Some(result) = self.files.poll_folder() {
            match result {
                Ok(folder) => self.open_folder(folder),
                Err(message) => self.alerts.push_back(Alert::new(
                    crate::strings::APP_ERR_OPEN_FOLDER_TITLE,
                    message,
                )),
            }
        }
        if let Some(result) = self.files.poll_renamed() {
            let is_pack_op = self
                .pack_run
                .as_ref()
                .is_some_and(|run| run.rename_in_flight);
            match result {
                Ok(()) if is_pack_op => {
                    if let Some(run) = self.pack_run.as_mut() {
                        run.rename_in_flight = false;
                    }
                    self.advance_pack_run();
                }
                Ok(()) => {
                    // A quick-edit rename paired with a byte rewrite: now that the
                    // file has its new name, write the target-format bytes to it
                    // (its own TrackRewrite outcome then rescans the folder).
                    if let Some((path, bytes)) = self.pending_rewrite.take() {
                        self.pending_saves.push_back(SavePurpose::TrackRewrite);
                        self.files.save(SaveRequest::InPlace { path, bytes });
                    } else {
                        self.rescan_pack_folder();
                        self.status = crate::strings::APP_STATUS_RENAMED_TRACK.to_owned();
                    }
                }
                Err(message) if is_pack_op => self.abort_pack_run(message),
                Err(message) => {
                    self.pending_rewrite = None;
                    self.alerts
                        .push_back(Alert::new(crate::strings::APP_ERR_RENAME_TITLE, message));
                }
            }
        }
        if let Some(result) = self.files.poll_deleted() {
            // Deletes are only ever issued by the pack file-op executor, so an
            // outcome always belongs to the run in flight.
            match result {
                Ok(()) => self.advance_pack_run(),
                Err(message) => self.abort_pack_run(message),
            }
        }
        if let Some(chosen) = self.files.poll_output_folder() {
            self.split_into(chosen);
        }
        if let Some(chosen) = self.files.poll_save_path() {
            self.render_wav_into(chosen);
        }
        if let Some(outcome) = self.files.poll_saved() {
            // Outcomes arrive in the order the saves were made, so a FIFO of
            // purposes routes each one to the editor or the pack project.
            let purpose = self.pending_saves.pop_front().unwrap_or(SavePurpose::Song);
            self.handle_save_outcome(purpose, outcome);
        }
        if let Some(outcome) = self.pack_service.poll() {
            match outcome {
                PackJobOutcome::Done {
                    zip_name,
                    bytes,
                    log,
                } => {
                    // A Save Pack export clears the memory pack's dirty flag when
                    // it lands; a plain Export Zip does not.
                    let purpose = if std::mem::take(&mut self.pack_saving_archive) {
                        SavePurpose::SaveArchive
                    } else {
                        SavePurpose::ExportZip
                    };
                    self.pending_saves.push_back(purpose);
                    self.files.save(SaveRequest::Dialog {
                        suggested_name: zip_name,
                        bytes,
                    });
                    // The zip exists in memory, not on disk: the picker is still
                    // to come, and saying "built" without saying "choose where"
                    // reads as finished (which is what a cancel then contradicts).
                    self.status = if log.is_empty() {
                        crate::strings::APP_STATUS_PACK_ZIP_BUILT.to_owned()
                    } else {
                        crate::strings::app_pack_zip_built_log(&log.join(" "))
                    };
                }
                PackJobOutcome::Failed(message) => {
                    // Replace the stale "Building pack zip..." status.
                    self.status = crate::strings::APP_STATUS_PACK_EXPORT_FAILED.to_owned();
                    self.alerts.push_back(Alert::new(
                        crate::strings::APP_ERR_PACK_EXPORT_TITLE,
                        message,
                    ));
                }
            }
        }
        if let Some(result) = self.pack_service.poll_optimized() {
            // An add's recompression is a step on the way in, not an edit of a
            // file in the folder: it writes the file rather than rewriting it,
            // and a failed pass just means the picked bytes go in as they are.
            if let Some(add) = self.pending_add.take() {
                let smaller = result
                    .ok()
                    .filter(|optimized| optimized.bytes.len() < add.bytes.len())
                    .map(|optimized| optimized.bytes);
                self.write_added_screenshot(match smaller {
                    Some(bytes) => PendingAdd { bytes, ..add },
                    None => add,
                });
            } else {
                match result {
                    Ok(optimized) => self.image_optimized(optimized),
                    Err(message) => {
                        self.status = crate::strings::APP_STATUS_SCREENSHOT_OPT_FAILED.to_owned();
                        self.alerts
                            .push_back(Alert::new(crate::strings::APP_ERR_OPTIMISE_TITLE, message));
                    }
                }
            }
        }
        if let Some(result) = self.pack_service.poll_optimized_song() {
            self.song_optimized(result);
        }
        for result in self.tasks.poll() {
            match result {
                TaskResult::Waveform(buckets) => self.waveform.buckets = buckets,
                TaskResult::Wav(rendered) => self.handle_wav_result(rendered),
                TaskResult::Split(outputs) | TaskResult::SplitSongs(outputs) => {
                    self.write_split(outputs);
                }
                TaskResult::Peak(peak) => self.handle_volume_scan(peak),
                TaskResult::PackPeaks(peaks) => self.handle_pack_peaks(peaks),
                TaskResult::PackScanProgress { done, total } => {
                    self.pack_scan_progress = Some((done, total));
                }
                TaskResult::LoopCandidates(candidates) => self.handle_loop_candidates(candidates),
            }
        }
        // Keep the Find Loop dialog's progress state in step with the task, so its
        // spinner shows while a search runs and clears the moment it finishes.
        let searching = self.tasks.is_busy_kind(TaskKind::LoopSearch);
        if let Some(dialog) = self.dialogs.find_loop.as_mut() {
            dialog.set_busy(searching);
        }
    }

    /// Writes a finished render straight to the path chosen before it ran, or
    /// reports the failure. The destination was picked up front (`render_wav_into`),
    /// so there is no dialog here -- the bytes go where the user already said.
    pub(super) fn handle_wav_result(&mut self, rendered: Result<(String, Vec<u8>), String>) {
        // A result with no Rendering flow belongs to a render the user abandoned
        // (by loading another song, which cancels the task); drop it.
        let Some(RenderWavFlow::Rendering { path }) = self.render_flow.take() else {
            return;
        };
        match rendered {
            Ok((_name, bytes)) => {
                self.pending_saves.push_back(SavePurpose::WavExport);
                self.files.save(SaveRequest::InPlace { path, bytes });
            }
            Err(message) => {
                self.status = crate::strings::APP_STATUS_WAV_RENDER_FAILED.to_owned();
                self.alerts.push_back(Alert::error(message));
            }
        }
    }

    /// Changes the live playback volume, updating the config, the audio engine and
    /// (when `persist`) `vgmstudio.ini`. The shared path behind the volume lever and
    /// the "Match Volume" scan.
    pub(super) fn set_boost(&mut self, value: f32, persist: bool) {
        self.config.audio.boost = value;
        // A loaded stream gets the boost live via the command queue; an unloaded
        // one picks it up from `config.audio` on the next load, so this
        // deliberately does not force an audio reload.
        self.audio.set_boost(value);
        // Only write to vgmstudio.ini when the volume is locked: an unlocked boost
        // is per-song (re-derived from the modifier on the next open), so
        // persisting it would resurrect a stale value on the next launch.
        if persist
            && self.config.audio.lock_boost
            && let Err(error) = self.config_store.save(&self.config)
        {
            self.alerts
                .push_back(Alert::error(crate::strings::app_could_not_save_settings(
                    error,
                )));
        }
    }

    /// The playback volume `song`'s header volume modifier asks for: always unity
    /// for a DRO, which carries no modifier. What an unlocked song starts at, in
    /// the editor and in a pack preview. (A VGM's own modifier is applied on the
    /// `VgmFile` path.)
    pub(super) fn modifier_boost(_song: &vgms_core::DroSong) -> f32 {
        1.0
    }

    /// The volume a freshly opened *editor* song should start at when the volume
    /// is not locked.
    pub(super) fn song_modifier_boost(&self) -> f32 {
        // The document's own header modifier. A VGM is read straight from its
        // header -- the OPL projection's meta only mirrors it -- so a VGM whose
        // chips are not OPL (and so has no projection) opens at the volume it
        // asks for too. A DRO has no modifier and stays at unity.
        if let Some(file) = self.editor.vgm() {
            vgms_core::volume_modifier_factor(file.header.volume_modifier())
        } else {
            self.editor.dro_song().map_or(1.0, Self::modifier_boost)
        }
    }

    /// Applies the "Lock" toggle. Locking remembers the current volume across
    /// songs (and persists it); unlocking hands control back to each song's
    /// header modifier, snapping the current song to its modifier now so the
    /// lever reflects the change immediately.
    pub(super) fn set_lock_boost(&mut self, lock: bool) {
        self.config.audio.lock_boost = lock;
        if !lock {
            let boost = self.song_modifier_boost();
            self.config.audio.boost = boost;
            self.audio.set_boost(boost);
        }
        if let Err(error) = self.config_store.save(&self.config) {
            self.alerts
                .push_back(Alert::error(crate::strings::app_could_not_save_settings(
                    error,
                )));
        }
    }

    /// Kicks off a background peak scan of the current song for the volume lever's
    /// "Match" button; the finished scan reaches [`Self::handle_volume_scan`]
    /// through `poll_services`. Cancels any scan already running (same
    /// [`TaskKind`]), so mashing the button just re-measures.
    pub(super) fn match_volume(&mut self) {
        self.submit_volume_scan(
            VolumeScanPurpose::MatchBoost,
            crate::strings::APP_STATUS_MEASURING_VOLUME,
        );
    }

    /// Kicks off a background peak scan for the VGM dialog's "Measure" button; the
    /// finished scan fills the volume-modifier field via [`Self::handle_volume_scan`].
    pub(super) fn measure_volume_modifier(&mut self) {
        self.submit_volume_scan(
            VolumeScanPurpose::FillModifier,
            crate::strings::APP_STATUS_MEASURING_PEAK,
        );
    }

    /// Submits a volume scan of the current document for `purpose`, or asks for
    /// a file if there is nothing to measure. Shared by the "Match" and
    /// "Measure" buttons; the purpose is remembered so
    /// [`Self::handle_volume_scan`] routes the result.
    pub(super) fn submit_volume_scan(&mut self, purpose: VolumeScanPurpose, status: &str) {
        // Measuring means rendering, so the gate is the render's own: any
        // document with a chip this app can render, OPL or not -- the same rule
        // the File menu and the pack scan apply. A coreless document is refused
        // here rather than measured as silence and handed a bogus suggestion.
        if !self.require_renderable() {
            return;
        }
        let Some(source) = self.audio_source() else {
            return;
        };
        self.volume_scan_purpose = purpose;
        self.tasks.submit(
            TaskRequest::VolumeScan {
                source,
                sample_rate: self.config.audio.frequency,
                resampling: self.resample_mode(),
            },
            None,
        );
        self.status = status.to_owned();
    }

    /// Applies a finished volume scan to whatever asked for it: the playback
    /// volume lever (the "Match" button) or the VGM dialog's volume-modifier field
    /// (the "Measure" button).
    pub(super) fn handle_volume_scan(&mut self, peak: vgms_synth::Peak) {
        // Keep the peak whatever the scan was for, so the VGM header dialog can
        // populate its volume boost from an earlier "Match" without re-scanning.
        self.last_measured_peak = Some(peak);
        match self.volume_scan_purpose {
            VolumeScanPurpose::MatchBoost => {
                if peak.max_level == 0 {
                    self.status = crate::strings::APP_STATUS_SONG_SILENT.to_owned();
                    return;
                }
                // The modifier-ladder volume that lifts the peak to full scale.
                let volume = vgms_core::matched_volume(peak.max_level);
                self.set_boost(volume, true);
                let dbfs = vgms_core::peak_dbfs(peak.max_level);
                self.status = crate::strings::app_status_matched_volume(dbfs, volume);
            }
            VolumeScanPurpose::FillModifier => {
                // The dialog may have been closed while the scan ran; if so, the
                // result is simply dropped.
                if let Some(dialog) = self.dialogs.vgm_metadata.as_mut() {
                    dialog.apply_measured_peak(peak);
                    let modifier = vgms_core::suggest_volume_modifier(peak.max_level, None);
                    let dbfs = vgms_core::peak_dbfs(peak.max_level);
                    self.status = crate::strings::app_status_measured_modifier(dbfs, modifier);
                }
            }
        }
    }

    pub(super) fn handle_save_outcome(&mut self, purpose: SavePurpose, outcome: SaveOutcome) {
        match outcome {
            SaveOutcome::Saved { name, path } => match purpose {
                SavePurpose::Song => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    // Save As can change the .vgm/.vgz extension after the
                    // bytes were serialised; re-save once so the compression
                    // matches the chosen name.
                    if self.editor.record_saved(name, path)
                        && let (Ok(bytes), Some(path)) =
                            (self.editor.save_bytes(), self.editor.path.clone())
                    {
                        self.pending_saves.push_back(SavePurpose::Song);
                        self.files.save(SaveRequest::InPlace { path, bytes });
                    }
                    // The song on disk now matches the editor: mark it clean so
                    // the discard-changes prompts stop firing.
                    self.editor.mark_saved();
                    self.status = crate::strings::app_status_file_saved(&shown);
                }
                SavePurpose::PackDoc => {
                    // The description and playlist save back to back; report and
                    // clear the dirty flag once the last of them lands -- but only
                    // if none of the batch failed, so edits aren't lost.
                    let more = self
                        .pending_saves
                        .iter()
                        .any(|purpose| *purpose == SavePurpose::PackDoc);
                    if !more {
                        if self.pack_docs_failed {
                            self.status = crate::strings::APP_STATUS_PACKAGE_SAVE_FAILED.to_owned();
                        } else {
                            if let Some(pack) = self.pack.as_mut() {
                                pack.dirty = false;
                            }
                            // Extensions only: the stem is the game's full name,
                            // and printing it twice runs the status line off on
                            // a pack with a subtitle.
                            self.status = crate::strings::APP_STATUS_PACKAGE_SAVED.to_owned();
                        }
                    }
                }
                SavePurpose::TrackRewrite | SavePurpose::ImageWritten => {
                    // The file's bytes were rewritten; rescan so the list (or
                    // the inline screenshot and its size) reflects the change. A
                    // rename, if any, rescans on its own outcome too -- both
                    // refresh in place, harmlessly. The edit landed, so its undo
                    // transaction (stashed at submit) becomes reversible.
                    if let Some(transaction) = self.pending_pack_undo.take() {
                        self.pack_undo.push(transaction);
                        self.pack_redo.clear();
                    }
                    self.rescan_pack_folder();
                }
                SavePurpose::SongOptimized => {
                    // As TrackRewrite: the optimised file landed, so its undo
                    // transaction becomes reversible and the folder is rescanned
                    // (which keeps the savings column). Only now is the single
                    // undo slot free, so the sweep advances to the next track.
                    if let Some(transaction) = self.pending_pack_undo.take() {
                        self.pack_undo.push(transaction);
                        self.pack_redo.clear();
                    }
                    self.rescan_pack_folder();
                    self.advance_song_optimize_sweep();
                }
                SavePurpose::ScreenshotAdded => {
                    self.rescan_pack_folder();
                    self.status = crate::strings::app_status_screenshot_added(&name);
                }
                SavePurpose::PackOp => self.advance_pack_run(),
                SavePurpose::ExportZip => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = crate::strings::app_status_exported(&shown);
                }
                SavePurpose::SaveArchive => {
                    // The memory pack's edits are now in a saved zip: it is clean.
                    if let Some(pack) = self.pack.as_mut() {
                        pack.dirty = false;
                    }
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = crate::strings::app_status_pack_saved(&shown);
                }
                SavePurpose::WavExport => {
                    let shown = path
                        .as_ref()
                        .map_or_else(|| name.clone(), |p| p.display().to_string());
                    self.status = crate::strings::app_status_rendered(&shown);
                }
                SavePurpose::SplitFile => self.split_file_saved(true),
            },
            SaveOutcome::Cancelled => match purpose {
                SavePurpose::PackDoc => self.pack_docs_failed = true,
                SavePurpose::PackOp => {
                    self.abort_pack_run(crate::strings::APP_MSG_SAVE_CANCELLED.to_owned())
                }
                SavePurpose::TrackRewrite | SavePurpose::ImageWritten => {
                    self.pending_pack_undo = None;
                }
                SavePurpose::SongOptimized => {
                    // The write was cancelled; drop its transaction and carry
                    // the sweep on rather than letting it stall.
                    self.pending_pack_undo = None;
                    self.advance_song_optimize_sweep();
                }
                // The build's status is still on the bar, reading as a finished
                // export -- gzipped tracks and all. Say what actually happened.
                SavePurpose::ExportZip => {
                    self.status = crate::strings::APP_STATUS_EXPORT_CANCELLED.to_owned();
                }
                // Split files save in place, so there is no picker to cancel --
                // but the tally still has to move on, or the batch never ends.
                SavePurpose::SplitFile => self.split_file_saved(false),
                _ => {}
            },
            SaveOutcome::Failed(message) => match purpose {
                SavePurpose::PackOp => self.abort_pack_run(message),
                SavePurpose::SplitFile => {
                    // One alert at the end for the whole batch, not eighteen.
                    log::warn!("split file could not be written: {message}");
                    self.split_file_saved(false);
                }
                SavePurpose::SongOptimized => {
                    // The write failed; drop its transaction, tell the user, and
                    // let the sweep move on rather than stall on this track.
                    self.pending_pack_undo = None;
                    self.alerts
                        .push_back(Alert::new(crate::strings::APP_ERR_SAVE_FILE_TITLE, message));
                    self.advance_song_optimize_sweep();
                }
                other => {
                    if other == SavePurpose::PackDoc {
                        self.pack_docs_failed = true;
                    }
                    if matches!(other, SavePurpose::TrackRewrite | SavePurpose::ImageWritten) {
                        self.pending_pack_undo = None;
                    }
                    self.alerts
                        .push_back(Alert::new(crate::strings::APP_ERR_SAVE_FILE_TITLE, message));
                }
            },
        }
    }

    pub(super) fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // A modal dialog cannot block raw OS drop events -- they never pass
        // through egui's interaction layer, so a `Modal` never sees them. A drop
        // while any dialog is open would swap the song underneath it, and a later
        // Apply (Find Loop, say) would write the old song's row indices into the
        // new one. Refuse the drop with a status line instead of letting the
        // document change out from under an open dialog.
        if self.dialogs.any_open() {
            self.status = crate::strings::APP_STATUS_DROP_DIALOG_OPEN.to_owned();
            return;
        }
        // Only single-file drops; say so rather than silently ignoring a
        // multi-drop.
        if dropped.len() > 1 {
            self.status = crate::strings::APP_STATUS_DROP_SINGLE.to_owned();
            return;
        }
        let file = dropped.into_iter().next().expect("len checked");
        let name = file
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.name.clone());
        let lower = name.to_ascii_lowercase();
        let is_song = lower.ends_with(".dro") || lower.ends_with(".vgm") || lower.ends_with(".vgz");
        if let Some(bytes) = file.bytes {
            // The web path: eframe delivers the dropped file's contents. A song
            // opens in the editor, a .zip opens as an in-memory pack; a dropped
            // folder has no bytes and never reaches here.
            if is_zip_name(&name) {
                self.open_zip_pack(PickedFile {
                    name,
                    path: None,
                    bytes: bytes.to_vec(),
                });
            } else if is_song {
                self.load_file(PickedFile {
                    name,
                    path: None,
                    bytes: bytes.to_vec(),
                });
            } else {
                self.status = crate::strings::app_status_unsupported_type(&name);
            }
        } else if let Some(path) = file.path {
            // Native: a song opens in the editor; a .zip opens as a pack; a folder
            // (no extension) is scanned into pack mode. The file service's read
            // routes each; `open_folder` recognises a zip pack by its token path.
            // A junk file surfaces the usual "bad format" alert.
            if is_zip_name(&name) || is_song || path.extension().is_none() {
                self.files.open_path(path);
            } else {
                self.status = crate::strings::app_status_unsupported_type(&name);
            }
        }
    }

    /// Cancels a window-close request while there are unsaved changes, raising a
    /// discard-changes confirm instead. A confirmed quit (`quitting`) is let
    /// straight through.
    pub(super) fn intercept_close(&mut self, ctx: &egui::Context) {
        if self.quitting || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.editor.is_dirty() || self.pack_is_dirty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let already_asking = self.alerts.iter().any(|alert| {
                alert.confirm.as_deref() == Some(&Action::File(FileAction::ConfirmExit))
            });
            if !already_asking {
                self.alerts.push_back(Alert::confirm(
                    crate::strings::APP_CONFIRM_DISCARD_TITLE,
                    crate::strings::APP_CONFIRM_QUIT_BODY,
                    Action::File(FileAction::ConfirmExit),
                ));
            }
        }
    }

    /// Loads `file` into the editor, or -- if the current song has unsaved edits
    /// -- stashes it behind a discard-changes confirm first. A `.zip` is a pack,
    /// not a song, so it routes to the zip-pack open instead (wt-8).
    pub(super) fn load_or_confirm(&mut self, file: PickedFile) {
        if is_zip_name(&file.name) {
            self.open_zip_pack(file);
        } else if self.editor.is_dirty() {
            self.pending_load = Some(file);
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_TITLE,
                crate::strings::APP_CONFIRM_DISCARD_LOAD_BODY,
                Action::File(FileAction::ConfirmDiscardAndLoad),
            ));
        } else {
            self.load_file(file);
        }
    }

    /// Opens a `.zip` as an in-memory pack, prompting first if the open pack has
    /// unsaved edits (a memory-backed pack's edits are lost on discard).
    pub(super) fn open_zip_pack(&mut self, file: PickedFile) {
        if self.pack_is_dirty() {
            self.pending_zip = Some(file);
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_CONFIRM_DISCARD_PACK_TITLE,
                crate::strings::APP_CONFIRM_PACK_OPEN_BODY,
                Action::Pack(PackAction::ConfirmOpenZip),
            ));
        } else {
            self.do_open_zip_pack(file);
        }
    }

    /// Hands a `.zip`'s bytes to the file service as an in-memory pack. The
    /// delivered folder carries a `/vgms-zip-N` token path, which `open_folder`
    /// recognises to stamp the memory origin.
    pub(super) fn do_open_zip_pack(&mut self, file: PickedFile) {
        self.files.open_pack_archive(file.name, file.bytes);
    }

    pub(super) fn gather_key_input(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // An alert or any open dialog owns the keyboard: the editor's shortcuts
        // (Space, Delete, ...) must not fire behind it, and Ctrl+Z in a tag field
        // must undo the *text*, not the song. A blanket `egui_wants_keyboard_input`
        // gate would also swallow shortcuts whenever a chrome button merely holds
        // focus, so instead the one editor-view text input (the volume field)
        // reports its own focus (see the boost_stepper gate below).
        if !self.alerts.is_empty() || self.dialogs.any_open() {
            return;
        }
        // The pack tab hides the editor, so the editor's playback/navigation keys
        // must not fire there. Save (the package files), Undo/Redo (the file
        // edits) and Help remain.
        if self.active_tab == AppTab::Pack {
            // Alt+arrow reorders the focused track: the keyboard path to the
            // drag handle, and the only one, so it is offered wherever a pack is
            // open rather than only while the Tracks section is drawn.
            ctx.input_mut(|input| {
                for (shortcut, delta) in [
                    (menus::MOVE_TRACK_UP, -1_isize),
                    (menus::MOVE_TRACK_DOWN, 1),
                ] {
                    if input.consume_shortcut(&shortcut) {
                        actions.push(Action::Pack(PackAction::MoveFocusedTrack { delta }));
                    }
                }
                if input.consume_shortcut(&menus::SAVE) {
                    actions.push(Action::Pack(PackAction::SaveDocs));
                }
                // Shifted variants first (egui ignores a surplus Shift).
                if input.consume_shortcut(&menus::REDO_ALT) {
                    actions.push(Action::Edit(EditAction::Redo));
                }
                if input.consume_shortcut(&menus::UNDO) {
                    actions.push(Action::Edit(EditAction::Undo));
                }
                if input.consume_shortcut(&menus::REDO) {
                    actions.push(Action::Edit(EditAction::Redo));
                }
                if input.consume_shortcut(&menus::HELP) {
                    actions.push(Action::Ui(UiAction::Help));
                }
            });
            return;
        }
        // The transport's volume field is the editor view's one focusable text
        // input; while it holds keyboard focus it owns the keyboard, so typed
        // numbers edit the value instead of toggling channels 1-9 (and Delete /
        // arrows edit the text, not the song). Tab is intentionally left
        // unconsumed here so it can move focus out of the field as usual.
        if self.volume_field_editing {
            return;
        }
        ctx.input_mut(|input| {
            // Aside from the volume field handled just above, the editor view has
            // no focusable text, so swallow Tab/Shift+Tab: a stray Tab would
            // otherwise move focus onto a chrome button, where Space activates it
            // (e.g. "Del.") instead of toggling playback.
            input.consume_key(egui::Modifiers::NONE, Key::Tab);
            input.consume_key(egui::Modifiers::SHIFT, Key::Tab);
            // egui's shortcut matching ignores a surplus Shift, so the
            // shifted variants must be consumed before their plain forms.
            if input.consume_shortcut(&menus::SAVE_AS) {
                actions.push(Action::File(FileAction::SaveAs));
            }
            if input.consume_shortcut(&menus::SAVE) {
                actions.push(Action::File(FileAction::Save));
            }
            if input.consume_shortcut(&menus::REDO_ALT) {
                actions.push(Action::Edit(EditAction::Redo));
            }
            if input.consume_shortcut(&menus::UNDO) {
                actions.push(Action::Edit(EditAction::Undo));
            }
            if input.consume_shortcut(&menus::REDO) {
                actions.push(Action::Edit(EditAction::Redo));
            }
            if input.consume_shortcut(&menus::OPEN) {
                actions.push(Action::File(FileAction::Open));
            }
            if input.consume_shortcut(&menus::GOTO) {
                actions.push(Action::Edit(EditAction::OpenGoto));
            }
            if input.consume_shortcut(&menus::FIND_REGISTER) {
                actions.push(Action::Edit(EditAction::OpenFindRegister));
            }
            if input.consume_shortcut(&menus::DRO_INFO) {
                actions.push(Action::Edit(EditAction::OpenDroInfo));
            }
            if input.consume_shortcut(&menus::HELP) {
                actions.push(Action::Ui(UiAction::Help));
            }
        });

        ctx.input(|input| {
            let mods = input.modifiers;
            // Plain editor keys must not fire with Command/Alt held (those form
            // menu shortcuts, handled above). Shift stays meaningful: extend the
            // selection on the arrows, and select the high channel bank on the
            // number row.
            if mods.command || mods.alt {
                return;
            }
            if input.key_pressed(menus::DELETE_SELECTION.logical_key)
                || input.key_pressed(menus::DELETE_SELECTION_ALT.logical_key)
            {
                actions.push(Action::Edit(EditAction::DeleteSelection));
            }
            if input.key_pressed(menus::PLAY_STOP.logical_key) {
                actions.push(Action::Playback(PlaybackAction::TogglePlayback));
            }
            if input.key_pressed(menus::PREVIOUS_DELAY.logical_key) {
                actions.push(Action::Playback(PlaybackAction::PreviousDelay));
            }
            if input.key_pressed(menus::NEXT_DELAY.logical_key) {
                actions.push(Action::Playback(PlaybackAction::NextDelay));
            }
            if input.key_pressed(menus::SELECTION_UP.logical_key) {
                actions.push(Action::Playback(PlaybackAction::SelectionMove {
                    delta: -1,
                    extend: mods.shift,
                }));
            }
            if input.key_pressed(menus::SELECTION_DOWN.logical_key) {
                actions.push(Action::Playback(PlaybackAction::SelectionMove {
                    delta: 1,
                    extend: mods.shift,
                }));
            }
            // [ and ] bracket the loop around the focused row -- the fastest way
            // to mark a region, since the table is where an exact instruction is
            // found. The end is exclusive, so ] marks *past* the focused row,
            // taking it into the loop rather than stopping just short of it.
            if let Some(row) = self.editor.selection.first() {
                if input.key_pressed(menus::SET_LOOP_START.logical_key) {
                    actions.push(Action::Loop(LoopAction::SetStart(row)));
                }
                if input.key_pressed(menus::SET_LOOP_END.logical_key) {
                    actions.push(Action::Loop(LoopAction::SetEnd(row + 1)));
                }
            }
            // 1..9 toggle channels 0..8; Shift+1..9 the high bank, channels 9..17.
            const NUMBER_KEYS: [Key; 9] = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let bank = if mods.shift { 9 } else { 0 };
            for (offset, key) in NUMBER_KEYS.into_iter().enumerate() {
                if input.key_pressed(key) {
                    actions.push(Action::Mixer(MixerAction::ToggleChannel(bank + offset)));
                }
            }
        });
    }

    pub(super) fn sync_selection_indicator(&mut self) {
        let first = self.editor.selection.first();
        if first == self.last_first_selected {
            return;
        }
        self.last_first_selected = first;
        // An emptied selection leaves the indicator where it was.
        let Some(index) = first else {
            return;
        };
        let Some(timeline) = self.editor.timeline() else {
            return;
        };
        if let Some(ms) = timeline.ms_offset_at(index) {
            self.waveform.start_ms = ms;
            self.position.set_position_ms(ms);
        }
    }

    /// Names the open file in the OS window title (with a `*` while it has
    /// unsaved changes), or the app name when nothing is open. Only re-sends the
    /// command when the title changes, so a paused event loop is not churned.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match (self.active_tab, self.pack.as_ref()) {
            (AppTab::Pack, Some(pack)) => {
                crate::strings::app_window_title(Some(&pack.folder_name), pack.dirty)
            }
            _ => crate::strings::app_window_title(
                self.editor.document_name(),
                self.editor.is_dirty(),
            ),
        };
        if title != self.window_title {
            self.window_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    pub(super) fn playback_tick(&mut self, ctx: &egui::Context) {
        // Advance the peak meter with the post-limiter peaks the callback
        // published. dt is clamped so a stalled frame cannot snap the bars to
        // zero. Kept separate from the position block below: the meter must
        // keep repainting through its decay after playback ends, without
        // re-running the position updates (which would overwrite the exact
        // end-of-song snap).
        // A backend can fail away from any call we made -- a device unplugged
        // mid-song -- so its complaint has nowhere to surface but here.
        if let Some(error) = self.audio.last_error() {
            self.alerts
                .push_back(Alert::error(crate::strings::app_playback_stopped(error)));
        }

        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        // The limiter's flag is read every tick, clip or not: it is destructive,
        // and left unread it would report a clip from a minute ago.
        let limited = self.audio.take_limited();
        self.peak_meter
            .update_with(self.audio.take_peaks(), dt, limited);
        if self.peak_meter.is_active() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        // Cap the volume where clipping starts. The backend reports the lowest
        // boost that has clipped this song (ratcheting down as quieter boosts
        // still clip), which is exactly the ceiling: the volume lever cannot rise
        // above it. A fresh (or unloaded) stream reports `None`, clearing the cap.
        self.boost_ceiling = self.audio.min_engaged_boost();

        let playing = self.audio.is_playing();
        // The loop indicator (drawn by the transport deck) and the position
        // panel's looping gate track the same fact: a loop actively repeating in
        // the editor. The scaled total, not the user's pick: the numerator counts
        // passes the engine derives from the same scaled count, so the two agree.
        self.loop_progress = (self.active_tab == AppTab::Editor && self.loop_enabled && playing)
            .then(|| self.audio.position())
            .flatten()
            .map(|position| (position.loop_iteration, self.loop_total));
        self.position.set_looping(self.loop_progress.is_some());
        if self.active_tab == AppTab::Editor {
            // One more update after playback ends, so the readout and cursor land
            // on the exact final position instead of freezing a buffer short of
            // it.
            if playing || self.was_playing {
                // A song that reached its end lands ~1 ms short of its length,
                // because the frame counter and the ms readout each floor at a
                // rate that need not divide evenly. Snap to the exact end so the
                // ms and sample counters agree. A manual Stop is not `is_finished`,
                // so its position is left exactly where playback paused.
                let ended = !playing && self.was_playing && self.audio.is_finished();
                if let Some(end) = ended
                    .then(|| self.editor.timeline().map(|t| t.total_ms()))
                    .flatten()
                {
                    self.waveform.cursor_ms = end;
                    self.position.set_position_ms(end);
                } else if let Some(position) = self.audio.position() {
                    // A wrap rewinds the engine's frame count to the loop start,
                    // so the cursor and readout follow the loop without any
                    // special handling here.
                    self.waveform.cursor_ms = position.elapsed_ms;
                    self.position.set_position(position);
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        } else if self
            .pack
            .as_ref()
            .is_some_and(|pack| pack.preview.is_some())
        {
            // A pack preview: clear it once it finishes, and keep the frames
            // coming while it plays (the pack view has no position readout).
            if self.audio.is_finished() {
                self.stop_preview();
            } else if playing {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        self.was_playing = playing;
        // The cursor's phosphor afterglow: record its position while playing,
        // age the ghosts out, and keep the repaints coming while any still glow
        // (so the trail finishes fading after playback stops).
        let now = ctx.input(|input| input.time);
        if playing && self.active_tab == AppTab::Editor {
            self.waveform.record_trail(now);
        }
        if self.waveform.prune_trail(now) {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.tasks.is_busy() || self.pack_service.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

/// How long the status bar's change flash takes to fade.
const STATUS_FLASH_SECS: f32 = 0.35;

/// The CP437 spinner frame for `time`: a quarter-turn every eighth of a second,
/// prefixed to the busy labels so a long job visibly lives.
fn spinner_frame(time: f64) -> char {
    const FRAMES: [char; 4] = ['-', '\\', '|', '/'];
    FRAMES[((time * 8.0) as usize) % FRAMES.len()]
}

/// The drag-over invitation: while the OS hovers a file over the window, tint
/// the central well, frame it with a dashed inset border, and say what the
/// drop does (or, with a dialog open, why it is refused). Foreground order, so
/// it reads over the table; the hint is a real label so the headless tests can
/// see it.
fn drop_target(ctx: &egui::Context, p: &Palette, rect: egui::Rect, blocked: bool) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop-target"),
    ));
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(0x70));
    let inset = rect.shrink(10.0);
    let corners = [
        inset.left_top(),
        inset.right_top(),
        inset.right_bottom(),
        inset.left_bottom(),
        inset.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &corners,
        egui::Stroke::new(2.0, p.data_label),
        8.0,
        6.0,
    ));
    let hint = if blocked {
        crate::strings::APP_STATUS_DROP_DIALOG_OPEN
    } else {
        crate::strings::APP_DROP_HINT
    };
    egui::Area::new(egui::Id::new("drop-target-hint"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_pos(inset.center())
        .interactable(false)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(hint).heading().color(p.data_label));
        });
}
