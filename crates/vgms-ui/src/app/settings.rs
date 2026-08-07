use super::*;

impl VgmStudioApp {
    pub(super) fn apply_settings(&mut self, ctx: &egui::Context, mut config: AppConfig) {
        // The Settings dialog snapshots the config at open and doesn't expose the
        // boost, so a boost changed via the transport slider meanwhile would be
        // reverted on Save. Keep the live value.
        config.audio.boost = self.config.audio.boost;
        // Changing where or through what playback goes has to take effect now,
        // not at the next Play: otherwise the old backend keeps playing and
        // keeps hold of its device, so a user switching away from hardware
        // output cannot get the serial port back until they press Play again.
        // The rebuild happens below, once the new config is in force.
        let output_changed = config.audio.cores != self.config.audio.cores
            || config.audio.resampling != self.config.audio.resampling
            || config.audio.retrowave_port != self.config.audio.retrowave_port;
        // Keep the registry's copy of the choices current, so the reload (and
        // every offline render) builds the cores just saved.
        vgms_synth::registry::set_core_choices(config.audio.cores.clone());
        if let Err(error) = self.config_store.save(&config) {
            self.alerts
                .push_back(Alert::error(crate::strings::app_could_not_save_settings(
                    error,
                )));
        }
        // Repaint the whole UI in the new scheme before anything else reads it.
        // Compare against what is *on screen*, which a live preview may already
        // have moved to the saved theme -- reapplying it then is just a no-op.
        if config.ui.theme != self.shown_skin().0 {
            theme::apply_palette(ctx, config.ui.theme);
        }
        // Whatever was being previewed is now the saved skin.
        self.skin_preview = None;
        // Only an audio change needs an output reload or a fresh waveform; a
        // theme-only change keeps the existing buckets and just recolours them.
        let audio_changed = config.audio != self.config.audio;
        let waveform_changed = config.audio.frequency != self.config.audio.frequency;
        let new_frequency = config.audio.frequency;
        self.config = config;
        // Don't retune the position panel to the configured rate while a stream
        // is live: it reports frames at the stream's real (still-old) rate, so
        // the readout would mix a new-rate length with old-rate frames. On the
        // next reload, ensure_audio adopts the new rate from output_rate.
        if self.audio.output_rate().is_none() {
            self.position.set_frequency(new_frequency);
        }
        if let Some(timeline) = self.editor.timeline() {
            self.position.set_length_ms(timeline.total_ms());
        }
        if audio_changed {
            // Reload the audio output lazily on the next play.
            self.audio_revision = None;
        }
        if output_changed {
            // A live stream is rebuilt in place -- the saved cores are heard
            // from where playback had reached, and a backend switch releases
            // the device it walked away from. An idle transport stays lazy.
            self.reload_audio_in_place();
        }
        if waveform_changed {
            self.submit_waveform(None);
        }
        self.status = crate::strings::APP_STATUS_SETTINGS_SAVED.to_owned();
    }

    /// Repaints in a skin without committing it. A colour scheme can only really
    /// be judged on the whole window, so the Settings dropdowns apply as they are
    /// picked; Close re-previews the settings the dialog opened with, putting the
    /// old skin back.
    ///
    /// Deliberately *not* written into `config`: that is what reaches the ini,
    /// and the volume lever saves it from under us (see [`Self::set_boost`]), so
    /// a preview parked there would persist itself behind the user's back.
    pub(super) fn preview_skin(
        &mut self,
        ctx: &egui::Context,
        theme: ThemeChoice,
        pad_style: SurfaceChoice,
        deck_style: SurfaceChoice,
    ) {
        if self.shown_skin().0 != theme {
            theme::apply_palette(ctx, theme);
        }
        // Matching the saved settings *is* no preview, so Close leaves nothing
        // behind to go stale.
        let ui = &self.config.ui;
        self.skin_preview = ((theme, pad_style, deck_style)
            != (ui.theme, ui.pad_style, ui.deck_style))
            .then_some((theme, pad_style, deck_style));
    }

    // -- helpers -------------------------------------------------------------

    /// Closes every dialog bound to the current song. Goto (validated live)
    /// and Settings (song-independent) survive.
    pub(super) fn close_song_dialogs(&mut self) {
        self.dialogs.find_reg = None;
        self.dialogs.dro_info = None;
        self.dialogs.gd3_tag = None;
        self.dialogs.vgm_metadata = None;
        self.dialogs.render_wav = None;
        self.dialogs.split = None;
    }

    /// The loaded document's name and the chips it clocks, for the Settings
    /// dialog's "This song" section. `None` when nothing is open.
    ///
    /// A VGM reports its header chips in file order; a DRO reports the one OPL
    /// chip it is. A VGM is always held as a VGM (even an OPL one), so it is
    /// asked first.
    pub(super) fn settings_song_context(&self) -> Option<crate::dialogs::SongContext> {
        if let Some(file) = self.editor.vgm() {
            let mut chips: Vec<vgms_core::vgm::ChipKind> = Vec::new();
            for chip in file.header.chips() {
                if !chips.contains(&chip.kind) {
                    chips.push(chip.kind);
                }
            }
            if chips.is_empty() {
                return None;
            }
            Some(crate::dialogs::SongContext {
                name: file.name.clone(),
                chips,
            })
        } else {
            let song = self.editor.dro_song()?;
            Some(crate::dialogs::SongContext {
                name: song.name.clone(),
                chips: vec![vgms_core::vgm::ChipKind::Ymf262],
            })
        }
    }

    /// Auditions a core map without saving it: the Settings picker's live
    /// preview. The registry choices are replaced and the loaded stream --
    /// which holds the cores it was built with -- is rebuilt in place, so the
    /// picked core is heard from the position the old one had reached.
    pub(super) fn preview_cores(&mut self, cores: std::collections::BTreeMap<String, String>) {
        vgms_synth::registry::set_core_choices(cores);
        self.reload_audio_in_place();
    }

    /// Auditions a resampling mode without saving it: the loaded stream reads
    /// its resampling from the live config at build time, so the config's mode
    /// is set and the stream rebuilt in place. Closing the dialog re-emits the
    /// saved mode, which reverts this. Every document's live playback resamples
    /// -- a DRO plays through the generic engine over its projection now (ou-2)
    /// -- so a preview is audible on either format. (Only the DRO offline
    /// render/waveform pipelines still ignore the mode.)
    pub(super) fn preview_resampling(&mut self, mode: String) {
        if self.config.audio.resampling == mode {
            return;
        }
        self.config.audio.resampling = mode;
        self.reload_audio_in_place();
    }

    /// Rebuilds the loaded audio stream with today's cores and config, keeping
    /// the playback position and the playing/paused state.
    ///
    /// A stopped or unloaded transport has nothing to rebuild: the next Play
    /// builds its stream lazily and picks everything up then, as it always has.
    pub(super) fn reload_audio_in_place(&mut self) {
        let position_ms = self.audio.position().map(|position| position.elapsed_ms);
        let playing = self.audio.is_playing();
        if position_ms.is_none() {
            return;
        }
        self.audio.pause();
        self.audio.unload();
        self.audio_revision = None;
        if self.ensure_audio().is_err() {
            // Nothing to rebuild after all (the document went away, or the
            // device did); the lazy path will say so when Play is next pressed.
            return;
        }
        if let Some(ms) = position_ms {
            self.audio.seek_ms(ms);
        }
        if playing && let Err(error) = self.audio.play() {
            self.alerts
                .push_back(Alert::error(crate::strings::app_could_not_resume(error)));
        }
    }
}
