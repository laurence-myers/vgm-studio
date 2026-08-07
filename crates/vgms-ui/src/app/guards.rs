use super::*;

impl VgmStudioApp {
    /// Gates an action on a loaded song, setting a status message asking the
    /// user to open a file when none is loaded.
    pub(super) fn require_song(&mut self) -> bool {
        if self.editor.has_dro() {
            true
        } else {
            // The features still behind this gate are genuinely OPL-only: DRO
            // Info, Convert to VGM, Convert to DRO v1. All three are menu-gated
            // to a DRO, so this message only ever fires from a stale shortcut --
            // a loaded VGM is not "the wrong file", it just is not an OPL one.
            self.status = crate::strings::APP_STATUS_NEEDS_OPL.to_owned();
            false
        }
    }

    /// Whether the loaded document's sound passes through this program as
    /// samples, and can therefore be metered, boosted and panned.
    ///
    /// The config's own answer is about *OPL* output: hardware output sends the
    /// board's own sound out its own socket, so nothing here can measure or
    /// shape it. A document that is not OPL never reaches that board -- it is
    /// routed to the emulator whatever the setting says -- so for one of those
    /// the answer is yes regardless.
    pub(super) fn output_renders_samples(&self) -> bool {
        // "Not an OPL document" is the always-metered case: a non-OPL VGM never
        // reaches the board. An OPL VGM *can* (RetroWave), so it defers to the
        // config -- hence is_opl(), not has_dro() (which is a DRO alone now).
        self.config.audio.renders_samples() || !self.editor.is_opl()
    }

    /// [`Self::output_renders_samples`] for tests, which have only a shared
    /// reference and no frame to draw.
    #[cfg(test)]
    pub(crate) fn output_renders_samples_for_test(&self) -> bool {
        self.output_renders_samples()
    }

    /// The gate for the transport: is there anything to hear?
    ///
    /// Between [`Self::require_song`] (an OPL stream) and
    /// [`Self::require_document`] (anything open). Playing needs neither of
    /// those exactly -- it needs a chip this app has a core for, which an OPL
    /// song always is and a VGM sometimes is.
    pub(super) fn require_playable(&mut self) -> bool {
        if self.editor.capabilities().playable && self.editor.has_document() {
            true
        } else {
            self.status = crate::strings::APP_STATUS_NOTHING_TO_PLAY.to_owned();
            false
        }
    }

    /// The gate for everything that works on a document of either kind.
    ///
    /// [`Self::require_song`] is the narrower one: it asks for an OPL stream,
    /// which is what rendering, splitting and the register analyser need. Saving,
    /// deleting, cropping and undo are not OPL ideas, so they ask this instead --
    /// otherwise a VGM for a chip we have no core for would open in the editor
    /// and then refuse to be edited.
    pub(super) fn require_document(&mut self) -> bool {
        if self.editor.has_document() {
            true
        } else {
            self.status = crate::strings::APP_STATUS_OPEN_FILE_FIRST.to_owned();
            false
        }
    }

    /// The gate for the offline renders and scans: would a render carry sound?
    ///
    /// Renderable is the File menu's own predicate -- an OPL stream, or a VGM
    /// with a chip this app has a core for -- so a shortcut or dialog gates on
    /// exactly what the menu offered. A document that is open but silent (chips
    /// with no core) gets the "nothing to play" message; nothing open falls
    /// through to [`Self::require_document`]'s "open a file" prompt.
    pub(super) fn require_renderable(&mut self) -> bool {
        if self.editor.capabilities().renderable {
            true
        } else if self.editor.has_document() {
            self.status = crate::strings::APP_STATUS_NOTHING_TO_PLAY.to_owned();
            false
        } else {
            self.require_document()
        }
    }

    /// Reports where the loaded VGM's header disagrees with its stream, and
    /// offers to correct it.
    ///
    /// Offers, never does: a header is a claim about the file, and rewriting
    /// one the user did not ask about is how a pack of carefully-made rips
    /// quietly becomes a pack of subtly different ones.
    pub(super) fn audit_header(&mut self) {
        let findings = self.editor.audit_header();
        if findings.is_empty() {
            self.status = crate::strings::APP_STATUS_HEADER_AGREES_NOTHING.to_owned();
            return;
        }
        let mut message = String::from(crate::strings::APP_AUDIT_HEADER_INTRO);
        for finding in &findings {
            message.push_str("  - ");
            message.push_str(&finding.describe());
            message.push('\n');
        }
        message.push_str(crate::strings::APP_AUDIT_HEADER_OUTRO);
        self.alerts.push_back(Alert::confirm(
            crate::strings::APP_FIX_HEADER_TITLE,
            message,
            Action::Edit(EditAction::ConfirmFixHeader),
        ));
    }

    pub(super) fn after_edit(&mut self) {
        self.audio.pause();
        self.audio_revision = None;
        // Playback starts where the cursor is -- the selected row, and the time
        // the position readout and the waveform cursor show. A crop or a delete
        // can leave any of them past the end of what is left, so anything now
        // outside the song comes back to the top, the one position every song is
        // guaranteed to have.
        let len = self.editor.len();
        // The timeline serves either representation, so a crop or a delete on a
        // non-OPL VGM shrinks the readout too, not just an OPL song's.
        let length_ms = self
            .editor
            .timeline()
            .map_or(0, |timeline| timeline.total_ms());
        let row_outside = self.editor.selection.first().is_some_and(|row| row >= len);
        if row_outside || self.position.position_ms() > length_ms {
            if len == 0 {
                self.editor.selection.clear();
            } else if row_outside {
                self.editor.selection.select_only(0);
                self.scroll_to = Some(table::ScrollTo::to_top(0));
            }
            self.reset_playback_start();
        }
        self.position.set_length_ms(length_ms);
        self.waveform.buckets.clear();
        self.submit_waveform(Some(Duration::from_secs(1)));
        // The selected row's time may have changed; force the indicator sync.
        self.last_first_selected = None;
    }

    /// Renders the song to a WAV in the background; the result reaches a save
    /// dialog through `poll_services`.
    ///
    /// Each option is opt-in, so with none of them this is exactly what
    /// `vgmstudio render` writes.
    /// The chip slots the loaded document occupies, for a render/split dialog's
    /// core picker. An OPL document is the one OPL slot; a generic VGM is its
    /// header chips. Empty with nothing loaded.
    pub(super) fn document_chips(&self) -> Vec<vgms_core::vgm::ChipKind> {
        match self.editor.doc_source() {
            Some(vgms_core::DocSource::Dro(_)) => vec![vgms_core::vgm::ChipKind::Ymf262],
            Some(vgms_core::DocSource::Vgm(file)) => {
                file.header.chips().iter().map(|chip| chip.kind).collect()
            }
            None => Vec::new(),
        }
    }
}
