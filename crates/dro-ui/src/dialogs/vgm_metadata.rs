//! The VGM metadata dialog (`vgm_metadata_dialog.py`). Modeless.
//!
//! Diverges from the Python where the Rust model does:
//! - The loop start is an *instruction index* (`VgmMeta::loop_point`), not a
//!   raw byte offset, and may be empty for "no loop".
//! - The loop length in samples is derived from the loop point, so it is
//!   displayed read-only rather than edited.
//! - Save actually works (the Python bound its Save button to the wrong id,
//!   so it never fired), and invalid input gets an error box instead of an
//!   uncaught exception.

use dro_core::Song;

use crate::action::Action;
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct VgmMetadataDialog {
    loop_point: String,
    loop_base: String,
    loop_modifier: String,
    volume_modifier: String,
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
            loop_base: meta.loop_base.to_string(),
            loop_modifier: meta.loop_modifier.to_string(),
            volume_modifier: meta.volume_modifier.to_string(),
            song_len: song.len(),
            samples_prefix: song.delay_samples_prefix(),
        })
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close = false;
        let open = super::dialog_window(ctx, "VGM Metadata", area, |ui| {
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
                        ("Volume modifier:", &mut self.volume_modifier),
                    ] {
                        ui.label(label);
                        ui.add(
                            egui::TextEdit::singleline(value)
                                .text_color(palette.data_text)
                                .desired_width(160.0),
                        );
                        ui.end_row();
                    }
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
            loop_base,
            loop_modifier,
            volume_modifier,
        });
        true
    }

    /// The loop length in samples for the currently-typed loop point, derived
    /// live from the prefix captured at open: "(no loop)" when the field is
    /// empty, "(invalid)" when it isn't a valid instruction index.
    fn loop_samples_display(&self) -> String {
        let trimmed = self.loop_point.trim();
        if trimmed.is_empty() {
            return "(no loop)".to_owned();
        }
        match trimmed.parse::<usize>() {
            Ok(index) if index < self.song_len => {
                let total = self.samples_prefix.last().copied().unwrap_or(0);
                total.saturating_sub(self.samples_prefix[index]).to_string()
            }
            _ => "(invalid)".to_owned(),
        }
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
            loop_base: "0".to_owned(),
            loop_modifier: "0".to_owned(),
            volume_modifier: "0".to_owned(),
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
