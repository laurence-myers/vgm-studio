//! The playback position panel.
//!
//! Three sections: "pos / len ms", "pos / len samples", and a read-only
//! sample-rate dropdown that re-denominates the sample counts to 44.1 kHz.
//! The counts are true frames, and the length is the *measured* total delay,
//! not the header's `ms_length`.

use dro_synth::{LoopCount, Position};

use crate::theme::Palette;

/// Round-half-up rescale to 44100 Hz: `floor(n / frequency * 44100 + 0.5)`.
fn rescale_to_44100(frames: u64, frequency: u32) -> u64 {
    (frames as f64 / f64::from(frequency) * 44_100.0 + 0.5).floor() as u64
}

fn ms_to_frames(ms: u32, frequency: u32) -> u64 {
    u64::from(ms) * u64::from(frequency) / 1000
}

#[derive(Debug)]
pub struct PositionPanel {
    frequency: u32,
    position_ms: u32,
    position_frames: u64,
    length_ms: u32,
    length_frames: u64,
    /// Whether the user picked "44.1 khz" while rendering at another rate.
    show_at_44100: bool,
    /// "Loop 2 / 5" while a loop is repeating, replacing the sample counter.
    loop_progress: Option<String>,
}

impl PositionPanel {
    #[must_use]
    pub fn new(frequency: u32) -> Self {
        Self {
            frequency,
            position_ms: 0,
            position_frames: 0,
            length_ms: 0,
            length_frames: 0,
            show_at_44100: false,
            loop_progress: None,
        }
    }

    /// Adopts a (possibly changed) rendering sample rate from the settings.
    pub fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    /// The rate the readout currently renders at (the stream's real rate while
    /// one is live, else the configured rate).
    #[must_use]
    pub fn frequency(&self) -> u32 {
        self.frequency
    }

    /// The song (or edit) length. Called on load and after every edit.
    pub fn set_length_ms(&mut self, ms: u32) {
        self.length_ms = ms;
        self.length_frames = ms_to_frames(ms, self.frequency);
    }

    /// A position expressed in time only -- row selection, waveform click.
    pub fn set_position_ms(&mut self, ms: u32) {
        self.position_ms = ms;
        self.position_frames = ms_to_frames(ms, self.frequency);
    }

    /// A live playback position, with its exact frame count.
    pub fn set_position(&mut self, position: Position) {
        self.position_ms = position.elapsed_ms;
        self.position_frames = position.frames_rendered;
    }

    /// Which pass of the loop is playing, or `None` to show the sample counter.
    ///
    /// `iteration` is how many times playback has jumped back, so the pass being
    /// heard is one more than that.
    pub fn set_loop_progress(&mut self, progress: Option<(u32, LoopCount)>) {
        self.loop_progress = progress.map(|(iteration, count)| match count {
            LoopCount::Infinite => format!("Loop {}", iteration + 1),
            LoopCount::Times(times) => format!("Loop {} / {}", iteration + 1, times.max(1)),
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let (position_frames, length_frames) = if self.show_at_44100 && self.frequency != 44_100 {
            (
                rescale_to_44100(self.position_frames, self.frequency),
                rescale_to_44100(self.length_frames, self.frequency),
            )
        } else {
            (self.position_frames, self.length_frames)
        };

        // Taken before the closures so they do not each need `self`.
        let (position_ms, length_ms) = (self.position_ms, self.length_ms);
        let loop_progress = self.loop_progress.clone();

        ui.columns(3, |columns| {
            columns[0].centered_and_justified(|ui| {
                ui.label(format!("{position_ms} / {length_ms} ms"));
            });
            columns[1].centered_and_justified(|ui| {
                match &loop_progress {
                    // While a loop is running, which pass it is on matters more
                    // than the sample count, and the two would not fit together.
                    Some(progress) => ui.label(progress),
                    None => ui.label(format!("{position_frames} / {length_frames} samples")),
                };
            });
            columns[2].with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.rate_picker(ui, palette);
            });
        });
    }

    fn rate_picker(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        // The choices are ["44.1 khz", "<config> khz"], lexicographically
        // sorted, defaulting to the rendering rate.
        let rendering = format!("{:.1} khz", f64::from(self.frequency) / 1000.0);
        let mut choices = vec!["44.1 khz".to_owned()];
        if self.frequency != 44_100 {
            choices.push(rendering.clone());
        }
        choices.sort();

        let selected = if self.show_at_44100 || self.frequency == 44_100 {
            "44.1 khz".to_owned()
        } else {
            rendering.clone()
        };
        let mut combo = egui::ComboBox::from_id_salt("sample-rate");
        combo = combo.selected_text(selected);
        ui.scope(|ui| {
            crate::theme::style_dropdown(ui, palette);
            combo.show_ui(ui, |ui| {
                for choice in &choices {
                    let is_44100 = choice == "44.1 khz";
                    let checked = if is_44100 {
                        self.show_at_44100 || self.frequency == 44_100
                    } else {
                        !self.show_at_44100
                    };
                    if ui.selectable_label(checked, choice).clicked() {
                        self.show_at_44100 = is_44100 && self.frequency != 44_100;
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescaling_to_44100_rounds_half_up() {
        // 48000 frames at 48 kHz is exactly one second: 44100 samples.
        assert_eq!(rescale_to_44100(48_000, 48_000), 44_100);
        // Half-up rounding: 1 frame at 48 kHz.
        assert_eq!(rescale_to_44100(1, 48_000), 1);
        assert_eq!(rescale_to_44100(0, 48_000), 0);
    }

    #[test]
    fn frames_derive_from_ms_at_the_rendering_rate() {
        assert_eq!(ms_to_frames(1000, 49_716), 49_716);
        assert_eq!(ms_to_frames(300, 48_000), 14_400);
    }
}
