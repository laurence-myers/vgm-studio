use super::*;

impl VgmStudioApp {
    #[must_use]
    pub fn new(
        files: Box<dyn FileService>,
        audio: Box<dyn AudioService>,
        tasks: Box<dyn TaskService>,
        pack_service: Box<dyn PackService>,
        config_store: Box<dyn ConfigStore>,
        initial_file: Option<PickedFile>,
    ) -> Self {
        let config = config_store.load();
        // The registry-side copy of `audio.core.<slug>`: every engine built
        // from here on (playback, WAV render, waveform, peak scan) reads it
        // through `core_for`, so the cores the user chose are the cores that
        // actually play.
        vgms_synth::registry::set_core_choices(config.audio.cores.clone());
        // And the machine-speed ratio beside it, so the picker's core-speed
        // estimates read as this machine from the first frame.
        vgms_synth::speed::set_machine_ratio(config.audio.machine_speed);
        let initial_frequency = config.audio.frequency;
        Self {
            editor: Editor::new(),
            pending_screenshot: None,
            pending_add: None,
            coalesce_next_reorder: None,
            pack_run_coalesces: false,
            files,
            audio,
            tasks,
            pack_service,
            config_store,
            config,
            status: String::new(),
            alerts: VecDeque::new(),
            dialogs: Dialogs::default(),
            pack: None,
            active_tab: AppTab::Editor,
            pending_saves: VecDeque::new(),
            split_flow: None,
            waveform: WaveformState::default(),
            peak_meter: PeakMeterState::default(),
            boost_ceiling: None,
            volume_scan_purpose: VolumeScanPurpose::MatchBoost,
            volume_field_editing: false,
            position: PositionPanel::new(initial_frequency),
            channels: ChipPanels::new(),
            status_shown: String::new(),
            pack_scan_progress: None,
            pending_song_optimize: VecDeque::new(),
            song_optimize_progress: None,
            chips_expanded: false,
            scroll_to: None,
            last_first_selected: None,
            audio_revision: None,
            loop_enabled: false,
            loop_count: LoopCount::Infinite,
            loop_total: LoopCount::Infinite,
            was_playing: false,
            pending_open: initial_file,
            pending_load: None,
            quitting: false,
            pending_rewrite: None,
            pack_docs_failed: false,
            pack_run: None,
            pack_undo: Vec::new(),
            pack_redo: Vec::new(),
            pending_pack_undo: None,
            skin_preview: None,
            pending_zip: None,
            pack_saving_archive: false,
            #[cfg(any(test, feature = "e2e"))]
            e2e_actions: VecDeque::new(),
        }
    }

    /// Queues an [`Action`] to run on the next frame, as if the UI had emitted it.
    /// The web e2e hook (`window.__vgms_e2e.dispatch`) calls this to drive the app
    /// without pixel-hitting the egui canvas; the queue is drained at the top of
    /// [`Self::update_impl`]. Test / `e2e` builds only.
    #[cfg(any(test, feature = "e2e"))]
    pub fn e2e_enqueue_action(&mut self, action: Action) {
        self.e2e_actions.push_back(action);
    }

    /// A read-only snapshot of the state the e2e specs assert on. Pure (no
    /// `Context`), so the web hook (`window.__vgms_e2e.state`) can call it between
    /// frames. Test / `e2e` builds only.
    #[cfg(any(test, feature = "e2e"))]
    #[must_use]
    pub fn e2e_snapshot(&self) -> E2eSnapshot {
        E2eSnapshot {
            has_document: self.editor.has_document(),
            document_name: self.editor.document_name().map(str::to_owned),
            row_count: self.editor.len(),
            dirty: self.editor.is_dirty(),
            can_undo: self.editor.can_undo(),
            can_redo: self.editor.can_redo(),
            playing: self.audio.is_playing(),
            status: self.status.clone(),
            active_tab: match self.active_tab {
                AppTab::Editor => "editor",
                AppTab::Pack => "pack",
            },
            alert: self.alerts.front().map(|alert| alert.message.clone()),
            dialog_open: self.dialogs.any_open(),
            pack: self.pack.as_ref().map(|pack| E2ePackSnapshot {
                name: pack.folder_name.clone(),
                dirty: pack.dirty,
                track_names: pack.tracks.iter().map(|t| t.file_name.clone()).collect(),
                image_names: pack.images.iter().map(|i| i.name.clone()).collect(),
            }),
        }
    }

    /// The skin on screen: the Settings dialog's live preview if one is up,
    /// else the saved settings.
    pub(super) fn shown_skin(&self) -> (ThemeChoice, SurfaceChoice) {
        self.skin_preview
            .unwrap_or((self.config.ui.theme, self.config.ui.pad_style))
    }

    /// The active colour scheme, with the configured pad override applied. Owned
    /// rather than borrowed: the override makes it a per-config value, not one of
    /// the twelve static case palettes.
    pub(super) fn palette(&self) -> Palette {
        let (theme, pad) = self.shown_skin();
        theme::palette_with(theme, pad)
    }
}
