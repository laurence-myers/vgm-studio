//! The Find Loop dialog: search a captured song for its loop point. Modal.
//!
//! A game rip usually contains the loop body played several times over. The
//! dialog runs [`dro_core::find_loops`] in the background (so a long song does
//! not freeze the UI) and lists the repeats it finds, best-first. Clicking a row
//! drops the editor's loop markers on it -- the waveform highlights the region
//! instantly -- and the footer buttons audition the seam or, for a VGM, write
//! the loop into the file's metadata. Every one of those is an existing editor
//! action; the dialog only points them at the chosen candidate.
//!
//! The minimum match length is entered in seconds and converted to a command
//! count from the song's own command density, so "2 s" means roughly two
//! seconds of music whatever the song's tempo.

use dro_core::{Candidate, Song};
use egui_extras::{Column, TableBuilder};

use crate::action::Action;
use crate::theme::{Palette, bevel};

/// The minimum match length the dialog opens with, in seconds. A permissive
/// floor: it only rejects repeats shorter than this, and real loops are longer.
const DEFAULT_MIN_SECS: f32 = 2.0;
/// The minimum-length slider's range, in seconds.
const MIN_SECS: f32 = 0.5;
const MAX_SECS: f32 = 30.0;

/// What the dialog needs to know about the document being searched.
///
/// Not the document itself: all it ever wanted was a time against each row and
/// a sense of how dense the commands are. Taking those instead of a [`Song`]
/// is what lets the dialog serve a VGM for any chip, which has no `Song` to
/// give it.
#[derive(Debug, Clone)]
pub struct LoopSearchDoc {
    /// The millisecond offset of each row, for the results' time display. One
    /// entry per row, built once at open -- a candidate can point anywhere, and
    /// re-deriving a time per row per frame would walk the stream each time.
    row_ms: Vec<u32>,
    /// Non-delay commands, the ceiling on the estimated minimum length.
    total_commands: usize,
    /// The song's length in seconds, for the commands-per-second estimate.
    total_secs: f32,
    /// Whether the document can store a loop -- Apply is VGM-only, because a
    /// DRO header has nowhere to put one.
    can_store_loop: bool,
}

impl LoopSearchDoc {
    #[must_use]
    pub fn from_song(song: &Song) -> Self {
        Self {
            row_ms: (0..song.len())
                .map(|index| song.ms_offset_at(index).unwrap_or(0))
                .collect(),
            total_commands: song.data().iter().filter(|i| !i.is_delay()).count(),
            total_secs: song.total_delay_ms() as f32 / 1000.0,
            can_store_loop: song.is_vgm(),
        }
    }

    #[must_use]
    pub fn from_vgm(file: &dro_core::VgmFile) -> Self {
        let Some(stream) = file.stream() else {
            return Self {
                row_ms: Vec::new(),
                total_commands: 0,
                total_secs: 0.0,
                can_store_loop: true,
            };
        };
        let total = stream.total_samples();
        let mut row_ms = Vec::with_capacity(stream.len());
        let mut elapsed = 0u64;
        let mut total_commands = 0usize;
        for index in 0..stream.len() {
            row_ms.push(dro_core::util::smp_to_ms(
                u32::try_from(elapsed).unwrap_or(u32::MAX),
                dro_core::vgm::VGM_SAMPLE_RATE,
            ));
            let wait = stream.wait_samples(index);
            if wait == 0 {
                total_commands += 1;
            }
            elapsed += u64::from(wait);
        }
        Self {
            row_ms,
            total_commands,
            total_secs: total as f32 / dro_core::vgm::VGM_SAMPLE_RATE as f32,
            can_store_loop: true,
        }
    }

    /// The millisecond offset of row `index`.
    fn ms_at(&self, index: usize) -> u32 {
        self.row_ms.get(index).copied().unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct FindLoopDialog {
    /// What is being searched, reduced to what the results need to show.
    doc: LoopSearchDoc,
    /// Non-delay commands per second, for the seconds -> command-count estimate.
    commands_per_sec: f32,
    /// Total non-delay commands, the ceiling on the estimated minimum length.
    total_commands: usize,
    /// Whether the document can store a loop -- the Apply button is VGM-only.
    is_vgm: bool,
    /// The minimum match length the slider edits, in seconds.
    min_secs: f32,
    /// The candidates found so far, best-first (the task ranks them).
    candidates: Vec<Candidate>,
    /// The selected row, which Audition and Apply act on. Pre-set to the best
    /// candidate whenever a fresh set arrives.
    selected: Option<usize>,
    /// A search is running: show the spinner, offer Cancel not Search.
    searching: bool,
    /// A search has finished at least once, so an empty table means "none found"
    /// rather than "not searched yet".
    searched: bool,
}

impl FindLoopDialog {
    #[must_use]
    pub fn new(doc: LoopSearchDoc) -> Self {
        let total_commands = doc.total_commands;
        let commands_per_sec = if doc.total_secs > 0.0 {
            total_commands as f32 / doc.total_secs
        } else {
            total_commands.max(1) as f32
        };
        Self {
            is_vgm: doc.can_store_loop,
            doc,
            commands_per_sec,
            total_commands,
            min_secs: DEFAULT_MIN_SECS,
            candidates: Vec::new(),
            selected: None,
            searching: false,
            searched: false,
        }
    }

    /// How many candidates have arrived so far.
    #[must_use]
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// Replaces the candidate list from a streamed task snapshot, pre-selecting
    /// the top (best) candidate so Apply and Audition work straight away.
    pub fn set_candidates(&mut self, candidates: Vec<Candidate>) {
        self.selected = (!candidates.is_empty()).then_some(0);
        self.candidates = candidates;
    }

    /// Tracks whether the search task is still running, driven each frame from
    /// the task service. The rising-to-falling edge marks a search complete.
    pub fn set_busy(&mut self, busy: bool) {
        if self.searching && !busy {
            self.searched = true;
        }
        self.searching = busy;
    }

    /// The chosen minimum length in delay-stripped commands, from the seconds the
    /// slider shows and the song's command density. Never zero, never more than
    /// the song has.
    fn min_len_commands(&self) -> usize {
        let estimate = (self.min_secs * self.commands_per_sec).round();
        (estimate as usize).clamp(1, self.total_commands.max(1))
    }

    fn selected_candidate(&self) -> Option<Candidate> {
        self.selected
            .and_then(|row| self.candidates.get(row).copied())
    }

    /// Submits a fresh search, clearing the old results.
    fn on_search(&mut self, actions: &mut Vec<Action>) {
        self.candidates.clear();
        self.selected = None;
        self.searching = true; // optimistic; the task service confirms it next frame
        actions.push(Action::FindLoopSearch {
            min_len_commands: self.min_len_commands(),
        });
    }

    /// Sets the editor's loop markers to `candidate`.
    fn mark(candidate: Candidate, actions: &mut Vec<Action>) {
        actions.push(Action::SetLoopStart(candidate.loop_point));
        actions.push(Action::SetLoopEnd(candidate.loop_end));
    }

    /// Marks the selected candidate and plays its seam.
    fn on_audition(&self, actions: &mut Vec<Action>) {
        if let Some(candidate) = self.selected_candidate() {
            Self::mark(candidate, actions);
            actions.push(Action::PlaySeam);
        }
    }

    /// Marks the selected candidate and writes it into the VGM metadata.
    fn on_apply(&self, actions: &mut Vec<Action>) {
        if let Some(candidate) = self.selected_candidate() {
            Self::mark(candidate, actions);
            actions.push(Action::ApplyLoopToMetadata);
        }
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        // The body borrows `self` and `actions` mutably, so the footer works
        // from values read here and reports clicks through cells; the deferred
        // handlers after the call re-check the live selection.
        let has_selection = self.selected_candidate().is_some();
        let is_vgm = self.is_vgm;
        let close = std::cell::Cell::new(false);
        let apply_clicked = std::cell::Cell::new(false);
        let audition_clicked = std::cell::Cell::new(false);
        let open = super::dialog_modal(
            ctx,
            "find-loop-modal",
            "Find Loop",
            palette,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Minimum loop length:")
                            .color(palette.data_label)
                            .strong(),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.min_secs)
                            .range(MIN_SECS..=MAX_SECS)
                            .speed(0.1)
                            .fixed_decimals(1)
                            .suffix(" s"),
                    );
                });
                ui.add_space(2.0);
                ui.colored_label(palette.muted, crate::strings::FIND_LOOP_MIN_LENGTH_HELP);

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.searching {
                        if bevel::button(ui, palette, "Cancel").clicked() {
                            actions.push(Action::CancelLoopSearch);
                        }
                        ui.spinner();
                        ui.colored_label(
                            palette.muted,
                            crate::strings::find_loop_searching_count(self.candidates.len()),
                        );
                    } else if bevel::button(ui, palette, "Search").clicked() {
                        self.on_search(actions);
                    }
                });

                ui.add_space(6.0);
                // Clipped, not full-width: the full-width groove paints into the
                // background layer, which under a modal would be a line drawn right
                // across the dimmed app behind it.
                crate::theme::separator_clipped(ui, palette);
                self.results_table(ui, palette, actions);
            },
            |ui| {
                super::dialog_footer(ui, |ui| {
                    if bevel::button(ui, palette, "Close").clicked() {
                        close.set(true);
                    }
                    ui.add_enabled_ui(has_selection && is_vgm, |ui| {
                        let apply = bevel::button(ui, palette, "Apply");
                        if apply.clicked() {
                            apply_clicked.set(true);
                        }
                        if !is_vgm {
                            apply.on_hover_text(crate::strings::FIND_LOOP_APPLY_VGM_ONLY_HINT);
                        }
                    });
                    ui.add_enabled_ui(has_selection, |ui| {
                        if bevel::button(ui, palette, "Audition").clicked() {
                            audition_clicked.set(true);
                        }
                    });
                });
            },
        );
        if apply_clicked.get() {
            self.on_apply(actions);
        }
        if audition_clicked.get() {
            self.on_audition(actions);
        }
        open && !close.get()
    }

    /// The scrollable results table: one row per candidate, clickable to mark it.
    fn results_table(&mut self, ui: &mut egui::Ui, palette: &Palette, actions: &mut Vec<Action>) {
        if self.candidates.is_empty() {
            let message = if self.searching {
                crate::strings::FIND_LOOP_SEARCHING
            } else if self.searched {
                crate::strings::FIND_LOOP_NONE_FOUND
            } else {
                crate::strings::FIND_LOOP_PROMPT
            };
            ui.colored_label(palette.muted, message);
            return;
        }

        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 6.0;
        let frame = egui::Frame::new()
            .fill(palette.data_bg)
            .inner_margin(egui::Margin::same(4));

        // Disjoint borrows, so the row closures can read the candidates and song
        // while taking `selected` mutably, without touching `self`.
        let candidates = &self.candidates;
        let doc = &self.doc;
        let selected = &mut self.selected;
        frame.show(ui, |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            let output = egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .sense(egui::Sense::click())
                        .vscroll(false)
                        .column(Column::auto().at_least(28.0)) // #
                        .column(Column::auto().at_least(64.0)) // Start
                        .column(Column::auto().at_least(64.0)) // End
                        .column(Column::auto().at_least(64.0)) // Length
                        .column(Column::remainder().at_least(70.0)) // Quality
                        .header(row_height + 2.0, |mut header| {
                            for title in ["#", "Start", "End", "Length", "Quality"] {
                                header.col(|ui| {
                                    ui.label(
                                        egui::RichText::new(title)
                                            .monospace()
                                            .color(palette.data_text),
                                    );
                                });
                            }
                        })
                        .body(|mut body| {
                            for (row_index, candidate) in candidates.iter().enumerate() {
                                let start = doc.ms_at(candidate.loop_point);
                                let end = doc.ms_at(candidate.loop_end);
                                body.row(row_height, |mut row| {
                                    row.set_selected(*selected == Some(row_index));
                                    cell(&mut row, palette.muted, &format!("{}", row_index + 1));
                                    cell(&mut row, palette.data_text, &fmt_time(start));
                                    cell(&mut row, palette.data_text, &fmt_time(end));
                                    cell(
                                        &mut row,
                                        palette.data_text,
                                        &fmt_time(end.saturating_sub(start)),
                                    );
                                    row.col(|ui| {
                                        ui.label(
                                            egui::RichText::new(candidate.quality_label())
                                                .monospace()
                                                .color(palette.data_text),
                                        )
                                        .on_hover_text(quality_help(*candidate));
                                    });
                                    if row.response().clicked() {
                                        *selected = Some(row_index);
                                        Self::mark(*candidate, actions);
                                    }
                                });
                            }
                        });
                });
            crate::theme::frame_scroll_output(ui, palette, output.inner_rect, output.content_size);
        });
    }
}

/// One monospace table cell.
fn cell(row: &mut egui_extras::TableRow<'_, '_>, color: egui::Color32, text: &str) {
    row.col(|ui| {
        ui.label(egui::RichText::new(text).monospace().color(color));
    });
}

/// A `M:SS.s` time from a millisecond offset.
fn fmt_time(ms: u32) -> String {
    let total_secs = f64::from(ms) / 1000.0;
    let minutes = (total_secs / 60.0).floor() as u32;
    let seconds = total_secs - f64::from(minutes) * 60.0;
    format!("{minutes}:{seconds:04.1}")
}

/// The hover text explaining a candidate's quality flags and match length.
fn quality_help(candidate: Candidate) -> String {
    let shape = match (candidate.ends_at_eof, candidate.clean_repeat) {
        (true, true) => crate::strings::FIND_LOOP_QUALITY_IDEAL,
        (true, false) => crate::strings::FIND_LOOP_QUALITY_TO_END,
        (false, true) => crate::strings::FIND_LOOP_QUALITY_CLEAN,
        (false, false) => crate::strings::FIND_LOOP_QUALITY_PARTIAL,
    };
    crate::strings::find_loop_quality_help(shape, candidate.match_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::{looping_vgm, tone_song};

    fn dialog(song: Song) -> FindLoopDialog {
        FindLoopDialog::new(LoopSearchDoc::from_song(&song))
    }

    fn candidate(loop_point: usize, loop_end: usize) -> Candidate {
        Candidate {
            loop_point,
            loop_end,
            match_len: 8,
            ends_at_eof: true,
            clean_repeat: true,
        }
    }

    #[test]
    fn a_fresh_dialog_prompts_for_a_search() {
        let dialog = dialog(looping_vgm());
        assert!(dialog.candidates.is_empty());
        assert!(!dialog.searched);
        assert_eq!(dialog.selected, None);
    }

    #[test]
    fn setting_candidates_preselects_the_best() {
        let mut dialog = dialog(looping_vgm());
        dialog.set_candidates(vec![candidate(3, 9), candidate(3, 15)]);
        assert_eq!(
            dialog.selected,
            Some(0),
            "the top candidate is pre-selected"
        );

        dialog.set_candidates(Vec::new());
        assert_eq!(
            dialog.selected, None,
            "an empty result clears the selection"
        );
    }

    #[test]
    fn searching_emits_a_command_count_from_the_seconds() {
        let mut dialog = dialog(looping_vgm());
        // A controlled density: 100 commands per second.
        dialog.commands_per_sec = 100.0;
        dialog.total_commands = 10_000;
        dialog.min_secs = 2.0;

        let mut actions = Vec::new();
        dialog.on_search(&mut actions);
        assert_eq!(
            actions,
            vec![Action::FindLoopSearch {
                min_len_commands: 200
            }]
        );
        assert!(
            dialog.searching,
            "the dialog shows the spinner optimistically"
        );
    }

    #[test]
    fn the_minimum_length_never_exceeds_the_song() {
        let mut dialog = dialog(looping_vgm());
        dialog.commands_per_sec = 1_000_000.0;
        dialog.total_commands = 12;
        dialog.min_secs = 30.0;
        assert_eq!(
            dialog.min_len_commands(),
            12,
            "clamped to the command count"
        );

        dialog.commands_per_sec = 0.0;
        assert_eq!(dialog.min_len_commands(), 1, "never zero");
    }

    #[test]
    fn selecting_a_candidate_marks_the_region() {
        let mut dialog = dialog(looping_vgm());
        dialog.set_candidates(vec![candidate(3, 9)]);
        let mut actions = Vec::new();
        // The row-click path marks the selected candidate.
        FindLoopDialog::mark(dialog.selected_candidate().unwrap(), &mut actions);
        assert_eq!(
            actions,
            vec![Action::SetLoopStart(3), Action::SetLoopEnd(9)]
        );
    }

    #[test]
    fn auditioning_marks_then_plays_the_seam() {
        let mut dialog = dialog(looping_vgm());
        dialog.set_candidates(vec![candidate(3, 9)]);
        let mut actions = Vec::new();
        dialog.on_audition(&mut actions);
        assert_eq!(
            actions,
            vec![
                Action::SetLoopStart(3),
                Action::SetLoopEnd(9),
                Action::PlaySeam
            ]
        );
    }

    #[test]
    fn applying_marks_then_writes_the_metadata() {
        let mut dialog = dialog(looping_vgm());
        dialog.set_candidates(vec![candidate(3, 9)]);
        let mut actions = Vec::new();
        dialog.on_apply(&mut actions);
        assert_eq!(
            actions,
            vec![
                Action::SetLoopStart(3),
                Action::SetLoopEnd(9),
                Action::ApplyLoopToMetadata
            ]
        );
    }

    #[test]
    fn a_dro_song_is_not_apply_capable() {
        // Apply is VGM-only; the dialog still opens and searches for a DRO.
        let dialog = dialog(tone_song());
        assert!(!dialog.is_vgm);
    }

    #[test]
    fn set_busy_marks_a_search_complete_on_its_falling_edge() {
        let mut dialog = dialog(looping_vgm());
        dialog.set_busy(true);
        assert!(dialog.searching);
        assert!(!dialog.searched);
        dialog.set_busy(false);
        assert!(!dialog.searching);
        assert!(dialog.searched, "the search is now known to have finished");
    }
}
