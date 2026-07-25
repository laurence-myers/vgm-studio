//! The VGM metadata dialog. Modal.
//!
//! - The loop start is an *instruction index* (`VgmMeta::loop_point`), not a
//!   raw byte offset, and may be empty for "no loop".
//! - The loop length in samples is derived from the loop point, so it is
//!   displayed read-only rather than edited.
//! - Save applies the metadata, and invalid input gets an error box.

use dro_core::Song;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct VgmMetadataDialog {
    loop_point: String,
    loop_end: String,
    loop_base: String,
    loop_modifier: String,
    volume_modifier: String,
    /// The peak from the most recent "Measure", for the dBFS/clipping readout.
    /// `None` until a measurement lands.
    measured: Option<dro_synth::Peak>,
    /// One past the highest valid loop point.
    song_len: usize,
    /// Cumulative samples before each instruction (len = `song_len + 1`),
    /// captured at open so the read-only loop-length readout can be recomputed
    /// live from the typed loop point.
    samples_prefix: Vec<u32>,
}

impl VgmMetadataDialog {
    /// `None` if `song` is not a VGM.
    #[must_use]
    pub fn new(song: &Song) -> Option<Self> {
        let meta = song.vgm_meta()?;
        Some(Self {
            loop_point: meta.loop_point.map_or_else(String::new, |i| i.to_string()),
            loop_end: meta.loop_end.map_or_else(String::new, |i| i.to_string()),
            loop_base: meta.loop_base.to_string(),
            loop_modifier: meta.loop_modifier.to_string(),
            volume_modifier: meta.volume_modifier.to_string(),
            measured: None,
            song_len: song.len(),
            samples_prefix: song.delay_samples_prefix(),
        })
    }

    /// Fills the volume-modifier field with the value that would bring the just-
    /// measured `peak` to full scale, and remembers the peak for the readout.
    /// Called by the app when a "Measure" scan lands.
    pub fn apply_measured_peak(&mut self, peak: dro_synth::Peak) {
        self.volume_modifier = dro_core::suggest_volume_modifier(peak.max_level, None).to_string();
        self.measured = Some(peak);
    }

    /// The "= N.NNx" gloss beside the volume-modifier field: what factor the
    /// typed byte asks players for, plus the measured peak's dBFS (and a clipping
    /// note) once a measurement has landed. `(invalid)` if the byte does not parse.
    fn volume_modifier_readout(&self) -> String {
        let Ok(byte) = self.volume_modifier.trim().parse::<u8>() else {
            return "(invalid)".to_owned();
        };
        let factor = dro_core::volume_modifier_factor(byte);
        let mut text = format!("= {factor:.2}\u{00d7}");
        if let Some(peak) = self.measured {
            let dbfs = dro_core::peak_dbfs(peak.max_level);
            text.push_str(&format!("  (peak {dbfs:.1} dBFS"));
            if peak.clipped {
                text.push_str(", clipping");
            }
            text.push(')');
        }
        text
    }

    /// Draws the modal. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_modal(ctx, "vgm-metadata-modal", "VGM Metadata", palette, |ui| {
            egui::Grid::new("vgm-meta-grid")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Loop start (instruction):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.loop_point)
                            .hint_text("empty = no loop")
                            .text_color(palette.data_text)
                            .desired_width(160.0),
                    );
                    ui.end_row();

                    ui.label("Loop end (instruction):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.loop_end)
                            .hint_text("empty = end of song")
                            .text_color(palette.data_text)
                            .desired_width(160.0),
                    );
                    ui.end_row();

                    ui.label("Loop length (samples):");
                    let mut samples = self.loop_samples_display();
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut samples)
                            .text_color(palette.data_text)
                            .desired_width(160.0),
                    );
                    ui.end_row();

                    for (label, value) in [
                        ("Loop base:", &mut self.loop_base),
                        ("Loop modifier:", &mut self.loop_modifier),
                    ] {
                        ui.label(label);
                        ui.add(
                            egui::TextEdit::singleline(value)
                                .text_color(palette.data_text)
                                .desired_width(160.0),
                        );
                        ui.end_row();
                    }

                    // Volume modifier gets a "Measure" button that fills it from
                    // the song's peak, and a live gloss of what the byte means.
                    ui.label("Volume modifier:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.volume_modifier)
                                .text_color(palette.data_text)
                                .desired_width(70.0),
                        );
                        if bevel::button(ui, palette, "Measure")
                            .on_hover_text(
                                "Measure the song's peak and suggest a modifier that brings it \
                                 to full scale",
                            )
                            .clicked()
                        {
                            actions.push(Action::MeasureVolumeModifier);
                        }
                        ui.label(self.volume_modifier_readout());
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close = true;
                }
                if bevel::button(ui, palette, "Save").clicked() && self.save(actions) {
                    close = true;
                }
            });
        });
        open && !close
    }

    /// Parses and emits the save; `false` (with an error box queued) if any
    /// field is invalid.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        let trimmed = self.loop_point.trim();
        let loop_point = if trimmed.is_empty() {
            None
        } else {
            match trimmed.parse::<usize>() {
                Ok(index) if index < self.song_len => Some(index),
                _ => {
                    actions.push(Action::Alert {
                        title: "Invalid VGM metadata".to_owned(),
                        message: format!(
                            "Loop start must be an instruction index below {}.",
                            self.song_len
                        ),
                    });
                    return false;
                }
            }
        };

        // An end is only meaningful with a start, and must leave a region: the
        // engine refuses an empty one anyway, so catch it here where it can be
        // explained rather than silently ignored.
        let loop_end = match (loop_point, self.parsed_end()) {
            (None, _) => None,
            (Some(_), None) => {
                actions.push(Action::Alert {
                    title: "Invalid VGM metadata".to_owned(),
                    message: format!(
                        "Loop end must be an instruction index of {} or less, or empty for the \
                         end of the song.",
                        self.song_len
                    ),
                });
                return false;
            }
            (Some(start), Some(end)) if end <= start => {
                actions.push(Action::Alert {
                    title: "Invalid VGM metadata".to_owned(),
                    message: "Loop end must come after the loop start.".to_owned(),
                });
                return false;
            }
            // An end at the end of the song is the default, and storing it as
            // such is what lets a later trim widen the loop with the song.
            (Some(_), Some(end)) => (end < self.song_len).then_some(end),
        };

        let parsed = (
            self.loop_base.trim().parse::<u8>(),
            self.loop_modifier.trim().parse::<u8>(),
            self.volume_modifier.trim().parse::<u8>(),
        );
        let (Ok(loop_base), Ok(loop_modifier), Ok(volume_modifier)) = parsed else {
            actions.push(Action::Alert {
                title: "Invalid VGM metadata".to_owned(),
                message: "Error updating VGM metadata, check that the entered values are correct."
                    .to_owned(),
            });
            return false;
        };

        actions.push(Action::SaveVgmMetadata {
            loop_point,
            loop_end,
            loop_base,
            loop_modifier,
            volume_modifier,
        });
        true
    }

    /// The loop length in samples for the currently-typed pair, derived live from
    /// the prefix captured at open: "(no loop)" when there is no loop point, and
    /// "(invalid)" when either field is not a usable instruction index.
    ///
    /// This is the header's `loop # samples`, which is why it is shown rather
    /// than edited -- it is a consequence of the two indices, not a third fact.
    fn loop_samples_display(&self) -> String {
        let trimmed = self.loop_point.trim();
        if trimmed.is_empty() {
            return "(no loop)".to_owned();
        }
        let Ok(start) = trimmed.parse::<usize>() else {
            return "(invalid)".to_owned();
        };
        let Some(end) = self.parsed_end() else {
            return "(invalid)".to_owned();
        };
        if start >= self.song_len || start >= end {
            return "(invalid)".to_owned();
        }
        self.samples_prefix[end]
            .saturating_sub(self.samples_prefix[start])
            .to_string()
    }

    /// The typed loop end as an index into the samples prefix, defaulting to the
    /// end of the song when the field is empty. `None` if it does not parse or
    /// reaches past the song.
    fn parsed_end(&self) -> Option<usize> {
        let trimmed = self.loop_end.trim();
        if trimmed.is_empty() {
            return Some(self.song_len);
        }
        trimmed
            .parse::<usize>()
            .ok()
            .filter(|&end| end <= self.song_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialog over a 4-instruction song whose loop length from each index is
    /// `100 - prefix[index]`.
    fn dialog() -> VgmMetadataDialog {
        VgmMetadataDialog {
            loop_point: String::new(),
            loop_end: String::new(),
            loop_base: "0".to_owned(),
            loop_modifier: "0".to_owned(),
            volume_modifier: "0".to_owned(),
            measured: None,
            song_len: 4,
            samples_prefix: vec![0, 10, 30, 60, 100],
        }
    }

    #[test]
    fn readout_tracks_the_typed_loop_point() {
        let mut dialog = dialog();
        dialog.loop_point = "2".to_owned();
        assert_eq!(dialog.loop_samples_display(), "70"); // 100 - 30
        dialog.loop_point = "3".to_owned();
        assert_eq!(dialog.loop_samples_display(), "40"); // 100 - 60
    }

    #[test]
    fn readout_measures_between_the_two_markers() {
        let mut dialog = dialog();
        dialog.loop_point = "1".to_owned();
        assert_eq!(dialog.loop_samples_display(), "90", "empty end = 100 - 10");
        dialog.loop_end = "3".to_owned();
        assert_eq!(dialog.loop_samples_display(), "50", "60 - 10");
        dialog.loop_end = "4".to_owned(); // == len, the end of the song
        assert_eq!(dialog.loop_samples_display(), "90");
    }

    #[test]
    fn readout_rejects_an_end_that_bounds_nothing() {
        let mut dialog = dialog();
        dialog.loop_point = "2".to_owned();
        for end in ["2", "1", "5", "x"] {
            dialog.loop_end = end.to_owned();
            assert_eq!(dialog.loop_samples_display(), "(invalid)", "end {end:?}");
        }
    }

    #[test]
    fn saving_an_end_at_the_songs_end_stores_the_default() {
        let mut dialog = dialog();
        dialog.loop_point = "1".to_owned();
        dialog.loop_end = "4".to_owned(); // == song_len
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::SaveVgmMetadata {
                loop_point: Some(1),
                loop_end: None,
                ..
            }]
        ));
    }

    #[test]
    fn saving_an_end_that_bounds_nothing_is_refused_with_a_reason() {
        let mut dialog = dialog();
        dialog.loop_point = "2".to_owned();
        dialog.loop_end = "2".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(actions.as_slice(), [Action::Alert { .. }]));
    }

    #[test]
    fn measuring_fills_the_field_from_the_peak() {
        let mut dialog = dialog();
        // A half-scale peak suggests a +6 dB modifier: byte 0x20 = 32.
        dialog.apply_measured_peak(dro_synth::Peak {
            max_level: 0x4000,
            clipped: false,
        });
        assert_eq!(dialog.volume_modifier, "32");
        // Saving carries the freshly measured byte through.
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::SaveVgmMetadata {
                volume_modifier: 32,
                ..
            }]
        ));
    }

    #[test]
    fn the_volume_readout_decodes_the_byte_and_notes_clipping() {
        let mut dialog = dialog();
        // Byte 32 = 0x20 = a 2x factor, before any measurement.
        dialog.volume_modifier = "32".to_owned();
        assert_eq!(dialog.volume_modifier_readout(), "= 2.00\u{00d7}");
        // After a clipping measurement, the peak and a clipping note are appended.
        dialog.apply_measured_peak(dro_synth::Peak {
            max_level: 0x7FFF,
            clipped: true,
        });
        let readout = dialog.volume_modifier_readout();
        assert!(readout.contains("dBFS"), "{readout}");
        assert!(readout.contains("clipping"), "{readout}");
        // A non-numeric byte reads as invalid rather than panicking.
        dialog.volume_modifier = "??".to_owned();
        assert_eq!(dialog.volume_modifier_readout(), "(invalid)");
    }

    #[test]
    fn readout_handles_empty_and_out_of_range() {
        let mut dialog = dialog();
        dialog.loop_point = "   ".to_owned();
        assert_eq!(dialog.loop_samples_display(), "(no loop)");
        dialog.loop_point = "4".to_owned(); // == song_len, out of range
        assert_eq!(dialog.loop_samples_display(), "(invalid)");
        dialog.loop_point = "x".to_owned();
        assert_eq!(dialog.loop_samples_display(), "(invalid)");
    }
}
