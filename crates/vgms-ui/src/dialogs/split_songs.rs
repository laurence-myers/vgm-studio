//! The Split Songs dialog: cut one long capture into its per-song files.
//!
//! A whole sound-test session logged in one go is many songs end to end, parted
//! by silence. The dialog runs [`detect_segments`] over the loaded capture (VGM
//! or DRO) and lists what it found; dragging the gap-threshold slider re-detects
//! live (one cheap pass), so the boundary list updates as you tune it. Each row
//! has an include checkbox -- to drop a false positive without leaving the dialog
//! -- and a Preview button that seeks playback to the song's start. A decay-tail
//! slider keeps a little of the trimmed silence after each piece. Export asks
//! where to put the files and writes `NN <stem>.<ext>` for every checked song.
//!
//! Times work in the capture's native delay unit (samples for a VGM,
//! milliseconds for a DRO); [`native_rate`] converts to and from the seconds the
//! sliders show, so the dialog is format-agnostic.

use vgms_core::Segment;

use crate::tasks::SplitSource;

use crate::action::{Action, FileAction, UiAction};
use crate::theme::{Palette, bevel};

/// The default gap threshold, in seconds. `vgm_sptd` uses 0x8000 = 32768 samples
/// (~0.74 s); 0.75 s is the same, rounded.
const DEFAULT_THRESHOLD_SECS: f32 = 0.75;
/// The gap-threshold slider's range, in seconds.
const MIN_THRESHOLD_SECS: f32 = 0.2;
const MAX_THRESHOLD_SECS: f32 = 5.0;
/// The decay-tail slider's maximum, in seconds. Default is 0 (no tail kept).
const MAX_TAIL_SECS: f32 = 2.0;

#[derive(Debug)]
pub struct SplitSongsDialog {
    /// A snapshot of the capture, taken at open; detection re-runs against it.
    source: SplitSource,
    /// Native delay units per second: 44100 for a VGM, 1000 for a DRO. Cached so
    /// the seconds/native conversions do not keep re-deriving it.
    rate: u32,
    /// Whether a piece can be auditioned -- the source's renderability, computed
    /// once (it can project a VGM, which is not a per-frame cost to pay).
    can_preview: bool,
    /// The gap threshold the slider edits, in seconds.
    threshold_secs: f32,
    /// The decay tail to keep after each piece, in seconds.
    tail_secs: f32,
    /// The songs detected at the current threshold.
    segments: Vec<Segment>,
    /// One include flag per segment, in the same order. Reset to all-on whenever
    /// re-detection changes the segment list.
    included: Vec<bool>,
}

impl SplitSongsDialog {
    #[must_use]
    pub fn new(source: SplitSource) -> Self {
        let rate = source.rate();
        let can_preview = crate::tasks::can_preview(&source);
        let mut dialog = Self {
            source,
            rate,
            can_preview,
            threshold_secs: DEFAULT_THRESHOLD_SECS,
            tail_secs: 0.0,
            segments: Vec::new(),
            included: Vec::new(),
        };
        dialog.redetect();
        dialog
    }

    /// A seconds value in the capture's native unit, rounded to the nearest unit.
    fn to_native(&self, seconds: f32) -> u32 {
        (seconds * self.rate as f32).round() as u32
    }

    /// The current gap threshold in native units, as the detector and export want.
    fn threshold_native(&self) -> u32 {
        self.to_native(self.threshold_secs)
    }

    /// The current decay tail in native units.
    fn tail_native(&self) -> u32 {
        self.to_native(self.tail_secs)
    }

    /// Re-runs detection at the current threshold and resets every include flag,
    /// since the old flags no longer line up with the new segment list.
    fn redetect(&mut self) {
        self.segments = self.source.detect(self.threshold_native());
        self.included = vec![true; self.segments.len()];
    }

    #[must_use]
    fn included_count(&self) -> usize {
        self.included.iter().filter(|&&on| on).count()
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let footer = super::Footer::new(palette, "Export...");
        let open = super::dialog_modal(
            ctx,
            "split-songs-modal",
            "Split Songs",
            palette,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Gap threshold:")
                            .color(palette.data_label)
                            .strong(),
                    );
                    let slider = bevel::slider(
                        ui,
                        palette,
                        &mut self.threshold_secs,
                        MIN_THRESHOLD_SECS..=MAX_THRESHOLD_SECS,
                        0.05,
                        " s",
                    );
                    if slider.changed() {
                        self.redetect();
                    }
                });
                ui.add_space(2.0);
                ui.colored_label(palette.muted, crate::strings::SPLIT_SONGS_GAP_EXPLAIN);

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Keep decay tail:")
                            .color(palette.data_label)
                            .strong(),
                    );
                    bevel::slider(
                        ui,
                        palette,
                        &mut self.tail_secs,
                        0.0..=MAX_TAIL_SECS,
                        0.05,
                        " s",
                    )
                    .on_hover_text(crate::strings::SPLIT_SONGS_TAIL_HOVER);
                });

                ui.add_space(6.0);
                // Clipped, not full-width: the full-width groove paints into the
                // background layer, which under a modal would be a line drawn right
                // across the dimmed app behind it.
                crate::theme::separator_clipped(ui, palette);
                self.boundary_table(ui, palette, actions);
            },
            |ui| footer.show(ui),
        );
        // Only a clicked Export runs the save; a refused one leaves the dialog open.
        let exported = footer.primary_clicked() && self.save(actions);
        open && !(footer.closed() || exported)
    }

    /// The song count and the scrollable boundary list (number, start, length, an
    /// include checkbox, and a Preview button per row).
    fn boundary_table(&mut self, ui: &mut egui::Ui, palette: &Palette, actions: &mut Vec<Action>) {
        if self.segments.is_empty() {
            ui.colored_label(palette.muted, crate::strings::SPLIT_SONGS_NONE_FOUND);
            return;
        }

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(crate::strings::split_songs_found(self.segments.len()))
                    .color(palette.data_label)
                    .strong(),
            );
            ui.colored_label(
                palette.muted,
                crate::strings::split_songs_to_export(self.included_count()),
            );
        });
        ui.add_space(4.0);

        // Disjoint borrows so the checkbox can take one field mutably while the
        // segment is read from another, all without touching `self` in the loop.
        let segments = &self.segments;
        let included = &mut self.included;
        let rate = self.rate;
        // Auditioning a piece plays it, which needs a chip we can render.
        let can_preview = self.can_preview;
        let output = egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("split-songs-boundaries")
                    .num_columns(5)
                    .spacing([12.0, 6.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for label in ["#", "Start", "Length", "", ""] {
                            ui.label(
                                egui::RichText::new(label)
                                    .color(palette.data_label)
                                    .strong(),
                            );
                        }
                        ui.end_row();

                        for (index, segment) in segments.iter().enumerate() {
                            ui.label(format!("{}", index + 1));
                            ui.label(super::fmt_time(segment.start_time, rate));
                            ui.label(super::fmt_time(segment.duration, rate));
                            ui.checkbox(&mut included[index], "Include");
                            ui.add_enabled_ui(can_preview, |ui| {
                                let preview = bevel::button(ui, palette, "Preview");
                                if preview.clicked() {
                                    actions.push(Action::File(FileAction::SplitSongsPreview {
                                        start_index: segment.start,
                                    }));
                                }
                                if !can_preview {
                                    preview.on_hover_text(
                                        crate::strings::SPLIT_SONGS_PREVIEW_UNAVAILABLE,
                                    );
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        crate::theme::frame_scroll_output(ui, palette, output.inner_rect, output.content_size);
    }

    /// Emits the export request, or queues an alert and stays open if nothing is
    /// checked. Returns whether the dialog should close.
    fn save(&self, actions: &mut Vec<Action>) -> bool {
        if self.included_count() == 0 {
            actions.push(Action::Ui(UiAction::Alert {
                title: crate::strings::SPLIT_SONGS_NOTHING_TITLE.to_owned(),
                message: crate::strings::SPLIT_SONGS_NOTHING_MESSAGE.to_owned(),
            }));
            return false;
        }
        actions.push(Action::File(FileAction::SplitSongsSubmitted {
            threshold_native: self.threshold_native(),
            included: self.included.clone(),
            trailing_tail: self.tail_native(),
        }));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::{multi_song_capture, multi_song_capture_dro, tone_song};
    use std::sync::Arc;

    fn dialog() -> SplitSongsDialog {
        SplitSongsDialog::new(SplitSource::Vgm(Arc::new(multi_song_capture())))
    }

    #[test]
    fn new_detects_at_the_default_threshold_with_all_included() {
        let dialog = dialog();
        assert_eq!(dialog.segments.len(), 3, "three songs at 0.75 s");
        assert_eq!(dialog.included, vec![true, true, true]);
        assert_eq!(dialog.included_count(), 3);
    }

    #[test]
    fn a_vgm_threshold_is_in_samples() {
        let dialog = dialog();
        assert_eq!(dialog.rate, 44_100);
        // 0.75 s * 44100 = 33075 samples, next to vgm_sptd's 0x8000 = 32768.
        assert_eq!(dialog.threshold_native(), 33_075);
    }

    #[test]
    fn a_dro_threshold_is_in_milliseconds() {
        let dialog = SplitSongsDialog::new(SplitSource::Opl(Arc::new(multi_song_capture_dro())));
        assert_eq!(dialog.rate, 1000);
        assert_eq!(dialog.threshold_native(), 750, "0.75 s = 750 ms");
        assert_eq!(dialog.segments.len(), 3, "three DRO songs at 0.75 s");
    }

    #[test]
    fn a_higher_threshold_re_detects_and_resets_the_flags() {
        let mut dialog = dialog();
        dialog.included[1] = false;
        // The gaps are one second; a two-second threshold merges everything back
        // into a single song.
        dialog.threshold_secs = 2.0;
        dialog.redetect();
        assert_eq!(dialog.segments.len(), 1);
        assert_eq!(dialog.included, vec![true], "flags reset on re-detect");
    }

    #[test]
    fn export_submits_the_threshold_tail_and_include_flags() {
        let mut dialog = dialog();
        dialog.included[1] = false; // drop the middle song
        dialog.tail_secs = 0.5; // keep half a second of decay

        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        match actions.as_slice() {
            [
                Action::File(FileAction::SplitSongsSubmitted {
                    threshold_native,
                    included,
                    trailing_tail,
                }),
            ] => {
                assert_eq!(*threshold_native, 33_075);
                assert_eq!(included, &[true, false, true]);
                assert_eq!(*trailing_tail, 22_050, "0.5 s in samples");
            }
            other => panic!("expected a split-songs submit, got {other:?}"),
        }
    }

    #[test]
    fn export_with_nothing_checked_alerts_instead() {
        let mut dialog = dialog();
        dialog.included = vec![false, false, false];

        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::Ui(UiAction::Alert { .. })]
        ));
    }

    #[test]
    fn a_capture_with_no_gaps_is_one_song() {
        // A plain single tone converted to VGM has no gaps: one segment.
        let mut song = tone_song();
        song.name = "tone.vgm".to_owned();
        let dialog = SplitSongsDialog::new(SplitSource::Opl(Arc::new(song)));
        assert!(dialog.segments.len() <= 1);
    }
}
