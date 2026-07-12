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

#[derive(Debug)]
pub struct VgmMetadataDialog {
    loop_point: String,
    loop_samples_display: String,
    loop_base: String,
    loop_modifier: String,
    volume_modifier: String,
    /// One past the highest valid loop point.
    song_len: usize,
}

impl VgmMetadataDialog {
    /// `None` if `song` is not a VGM.
    #[must_use]
    pub fn new(song: &Song) -> Option<Self> {
        let meta = song.vgm_meta()?;
        Some(Self {
            loop_point: meta.loop_point.map_or_else(String::new, |i| i.to_string()),
            loop_samples_display: song
                .loop_num_samples()
                .map_or_else(|| "(no loop)".to_owned(), |samples| samples.to_string()),
            loop_base: meta.loop_base.to_string(),
            loop_modifier: meta.loop_modifier.to_string(),
            volume_modifier: meta.volume_modifier.to_string(),
            song_len: song.len(),
        })
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) -> bool {
        let mut open = true;
        let mut close = false;
        egui::Window::new("VGM Metadata")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("vgm-meta-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Loop start (instruction):");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.loop_point)
                                .hint_text("empty = no loop")
                                .desired_width(160.0),
                        );
                        ui.end_row();

                        ui.label("Loop length (samples):");
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut self.loop_samples_display)
                                .desired_width(160.0),
                        );
                        ui.end_row();

                        for (label, value) in [
                            ("Loop base:", &mut self.loop_base),
                            ("Loop modifier:", &mut self.loop_modifier),
                            ("Volume modifier:", &mut self.volume_modifier),
                        ] {
                            ui.label(label);
                            ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() && self.save(actions) {
                        close = true;
                    }
                    if ui.button("Close").clicked() {
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
                        title: "Error".to_owned(),
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
                title: "Error".to_owned(),
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
}
