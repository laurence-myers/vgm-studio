use super::*;

impl VgmStudioApp {
    pub(super) fn render_to_wav(
        &mut self,
        use_toggles: bool,
        use_panning: bool,
        boost: f32,
        core_choices: std::collections::BTreeMap<String, String>,
    ) {
        let Some(source) = self.editor.doc_source() else {
            self.require_document();
            return;
        };
        // One render at a time: a second would finish into the same save queue,
        // and the first's dialog is already in the user's way.
        if self.tasks.is_busy_kind(TaskKind::RenderWav) {
            self.status = crate::strings::APP_STATUS_ALREADY_RENDERING.to_owned();
            return;
        }
        // The mix speaks the source's vocabulary: an OPL document mutes and pans
        // by register policy, a generic VGM by the per-chip masks its panels
        // describe. Each opt-in off means that dimension's neutral value, so the
        // all-off render stays byte-identical to `vgmstudio render`.
        let mix = match &source {
            WavSource::Opl(_) => RenderWavMix::Opl(RenderMix {
                muting: if use_toggles {
                    self.channels.muting()
                } else {
                    Muting::all()
                },
                panning: if use_panning {
                    self.channels.panning()
                } else {
                    Panning::Original
                },
                boost,
            }),
            WavSource::Vgm(_) => RenderWavMix::Vgm(VgmRenderMix {
                muting: if use_toggles {
                    self.channels.chip_muting()
                } else {
                    ChipMuting::new()
                },
                panning: if use_panning {
                    self.channels.chip_panning()
                } else {
                    ChipPanning::new()
                },
                boost,
            }),
        };
        self.tasks.submit(
            TaskRequest::RenderWav {
                source,
                mix,
                sample_rate: self.config.audio.frequency,
                bit_depth: self.config.audio.bit_depth,
                resampling: self.resample_mode(),
                core_choices,
            },
            None,
        );
        self.status = crate::strings::APP_STATUS_RENDERING_WAV.to_owned();
    }

    /// Whether a split (of either kind) is somewhere between its dialog and its
    /// last written file.
    pub(super) fn split_is_running(&self) -> bool {
        self.split_flow.is_some()
            || self.tasks.is_busy_kind(TaskKind::Split)
            || self.tasks.is_busy_kind(TaskKind::SplitSongs)
    }

    /// Asks where the channel split's files should go. The split itself starts
    /// once the answer arrives in `poll_services`.
    ///
    /// The panning and skip-muted opt-ins are resolved against the live mixer in
    /// `split_into`, once the document kind (OPL vs generic) is known -- the
    /// mixer is stable across the folder picker. Boost is a plain value, resolved
    /// here.
    pub(super) fn start_split(
        &mut self,
        format: SplitFormat,
        use_skip_muted: bool,
        use_panning: bool,
        boost: f32,
        core_choices: std::collections::BTreeMap<String, String>,
    ) {
        // Pan and skip-muted are resolved against the live mixer in `split_into`,
        // once the document kind is known; format, boost and cores ride here.
        self.begin_split(PendingSplit::Channels {
            format,
            boost,
            core_choices,
            use_panning,
            use_skip_muted,
        });
    }

    /// The loaded document as something a song split can run over, of either
    /// kind. `None` with nothing open.
    pub(super) fn split_source(&self) -> Option<crate::tasks::SplitSource> {
        // Ask the VGM slot first, like Crop's `replace_stream`. An OPL VGM has
        // a Some projection too, but splitting it through the OPL path
        // re-synthesises a v1.51 header with hard-coded clocks -- so a rip at a
        // non-canonical clock would split at the wrong pitch and tempo. The VGM
        // stack keeps the source header verbatim. A DRO has no `vgm()` and falls
        // through to the OPL path.
        match (self.editor.vgm_arc(), self.editor.snapshot()) {
            (Some(file), _) => Some(crate::tasks::SplitSource::Vgm(file)),
            (_, Some(song)) => Some(crate::tasks::SplitSource::Opl(song)),
            (None, None) => None,
        }
    }

    /// Asks where the song split's files should go, then starts on the answer.
    pub(super) fn start_split_songs(
        &mut self,
        threshold_native: u32,
        included: Vec<bool>,
        trailing_tail: u32,
    ) {
        self.begin_split(PendingSplit::Songs {
            threshold_native,
            included,
            trailing_tail,
        });
    }

    /// The shared entry both splits use: guard, stash the request, open the
    /// output-folder picker.
    pub(super) fn begin_split(&mut self, pending: PendingSplit) {
        // The song split only needs a document; the channel split needs
        // something that would actually render -- an OPL stream, or a VGM with
        // a core for at least one chip.
        let gate = if pending.is_songs() {
            Self::require_document
        } else {
            Self::require_splittable
        };
        if !gate(self) || self.split_is_running() {
            return;
        }
        self.split_flow = Some(SplitFlow::AwaitingFolder(pending));
        self.files.pick_output_folder();
    }

    /// Whether the loaded document has channels worth splitting: something that
    /// renders. Sets the status when not.
    pub(super) fn require_splittable(&mut self) -> bool {
        if self.editor.capabilities().renderable {
            true
        } else {
            self.status = crate::strings::APP_STATUS_NOTHING_TO_SPLIT.to_owned();
            false
        }
    }

    /// Starts the split now that `dir` is known, or gives up if the picker was
    /// dismissed.
    pub(super) fn split_into(&mut self, dir: Option<PathBuf>) {
        let Some(SplitFlow::AwaitingFolder(pending)) = self.split_flow.clone() else {
            // A folder arrived with no split waiting for it; nothing to do.
            return;
        };
        let songs = pending.is_songs();
        let request = match pending {
            PendingSplit::Channels {
                format,
                boost,
                core_choices,
                use_panning,
                use_skip_muted,
            } => {
                // Every split runs through the generic splitter now (ou-4): a
                // multichip VGM directly, an OPL document over a VGM of its
                // register stream. The mixer is stable across the folder picker,
                // so its pan/skip-muted opt-ins resolve here, translated into the
                // generic vocabulary for an OPL document.
                let audio = self.config.audio.clone();
                let resampling = self.resample_mode();
                let built: Option<(std::sync::Arc<vgms_core::VgmFile>, VgmSplitOptions)> =
                    if let Some(song) = self.editor.snapshot() {
                        // An OPL document: the OPL panel speaks Muting/Panning, so
                        // translate to the generic mutes/pans keyed on the chips
                        // its type projects to.
                        let opl_type = song.opl_type;
                        let panning = if use_panning {
                            vgms_synth::opl_chip_panning(&self.channels.panning(), opl_type)
                        } else {
                            ChipPanning::new()
                        };
                        let skip_muted = use_skip_muted.then(|| {
                            vgms_synth::opl_chip_muting(&self.channels.muting(), opl_type)
                        });
                        let options = VgmSplitOptions {
                            format,
                            audio,
                            resampling,
                            panning,
                            boost,
                            skip_muted,
                            core_choices,
                        };
                        // An OPL VGM splits from its own file so the header's
                        // clocks stay verbatim; a DRO has none, so it projects to
                        // a canonical-clock VGM.
                        match self.editor.vgm_arc() {
                            Some(file) => Some((file, options)),
                            None => match vgms_core::convert::opl_song_to_vgm_file(&song) {
                                Ok(file) => Some((std::sync::Arc::new(file), options)),
                                Err(error) => {
                                    self.alerts.push_back(Alert::error(format!(
                                        "Could not prepare the split: {error}"
                                    )));
                                    None
                                }
                            },
                        }
                    } else if let Some(file) = self.editor.vgm_arc() {
                        // A generic VGM: the chip mixer already speaks the generic
                        // vocabulary.
                        let panning = if use_panning {
                            self.channels.chip_panning()
                        } else {
                            ChipPanning::new()
                        };
                        let skip_muted = use_skip_muted.then(|| self.channels.chip_muting());
                        Some((
                            file,
                            VgmSplitOptions {
                                format,
                                audio,
                                resampling,
                                panning,
                                boost,
                                skip_muted,
                                core_choices,
                            },
                        ))
                    } else {
                        None
                    };
                built.map(|(file, options)| {
                    (
                        TaskRequest::Split {
                            source: crate::tasks::SplitTaskSource::Vgm { file, options },
                        },
                        crate::strings::APP_STATUS_SPLITTING_CHANNELS,
                    )
                })
            }
            PendingSplit::Songs {
                threshold_native,
                included,
                trailing_tail,
            } => self.split_source().map(|source| {
                (
                    TaskRequest::SplitSongs {
                        source,
                        threshold_native,
                        included,
                        trailing_tail,
                    },
                    crate::strings::APP_STATUS_SPLITTING_SONGS,
                )
            }),
        };
        let (Some(dir), Some((request, status))) = (dir, request) else {
            self.split_flow = None;
            self.status = crate::strings::APP_STATUS_SPLIT_CANCELLED.to_owned();
            return;
        };
        self.tasks.submit(request, None);
        self.split_flow = Some(SplitFlow::Rendering { dir, songs });
        self.status = status.to_owned();
    }

    /// Writes a finished split's files into the folder chosen for it.
    pub(super) fn write_split(&mut self, outputs: Result<Vec<(String, Vec<u8>)>, String>) {
        // Only the split still being waited on: a result from one the user
        // abandoned (by loading another song) has nowhere to go.
        let Some(SplitFlow::Rendering { dir, songs }) = self.split_flow.clone() else {
            return;
        };
        let files = match outputs {
            Ok(files) => files,
            Err(message) => {
                self.split_flow = None;
                self.status = crate::strings::APP_STATUS_SPLIT_FAILED.to_owned();
                self.alerts.push_back(Alert::error(message));
                return;
            }
        };
        if files.is_empty() {
            self.split_flow = None;
            self.status = if songs {
                crate::strings::APP_STATUS_NO_SONGS_SPLIT.to_owned()
            } else {
                crate::strings::APP_STATUS_NO_CHANNELS_SPLIT.to_owned()
            };
            return;
        }
        for (name, bytes) in files {
            self.pending_saves.push_back(SavePurpose::SplitFile);
            // In place, not a dialog: the user already chose the folder, and
            // there may be eighteen of these. Existing files are overwritten,
            // as `vgmstudio split` does.
            self.files.save(SaveRequest::InPlace {
                path: dir.join(name),
                bytes,
            });
        }
        self.split_flow = Some(SplitFlow::Writing {
            dir,
            written: 0,
            failed: false,
            songs,
        });
    }

    /// Counts off one split file's save, reporting once the last one lands.
    pub(super) fn split_file_saved(&mut self, ok: bool) {
        let Some(SplitFlow::Writing {
            dir,
            written,
            failed,
            songs,
        }) = &mut self.split_flow
        else {
            return;
        };
        if ok {
            *written += 1;
        } else {
            *failed = true;
        }
        // The whole batch is queued at once, so the last outcome is the one with
        // no `SplitFile` left behind it -- the same rule pack mode's docs use.
        if self
            .pending_saves
            .iter()
            .any(|purpose| *purpose == SavePurpose::SplitFile)
        {
            return;
        }
        let (dir, written, failed, songs) = (dir.clone(), *written, *failed, *songs);
        self.split_flow = None;
        if failed {
            self.status = crate::strings::APP_STATUS_SPLIT_WRITE_FAILED.to_owned();
            return;
        }
        self.finish_split(&dir, written, songs);
    }

    /// The success report once every split file has landed. A song split also
    /// offers to open the folder it filled as a pack project.
    pub(super) fn finish_split(&mut self, dir: &Path, written: usize, songs: bool) {
        if songs {
            self.status = crate::strings::app_status_wrote_songs(written, dir.display());
            self.alerts.push_back(Alert::confirm(
                crate::strings::APP_SONGS_EXPORTED_TITLE,
                crate::strings::app_songs_exported_body(written, dir.display()),
                Action::OpenPackFolderAt(dir.to_path_buf()),
            ));
        } else {
            self.status = crate::strings::app_status_wrote_files(written, dir.display());
        }
    }

    /// Previews a detected song: seek playback to its first instruction and play.
    pub(super) fn preview_segment(&mut self, start_index: usize) {
        if !self.require_playable() {
            return;
        }
        if let Err(message) = self.ensure_audio() {
            self.alerts.push_back(Alert::error(message));
            return;
        }
        self.audio.rewind();
        self.seek_to_row(start_index);
        if let Err(message) = self.audio.play() {
            self.alerts.push_back(Alert::error(message));
        }
    }
}
