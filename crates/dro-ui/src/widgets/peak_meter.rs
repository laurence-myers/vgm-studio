//! A vertical segmented stereo peak meter, beside the waveform well.
//!
//! Shows the post-boost/limiter output peaks published by the audio callback --
//! what the listener actually hears -- so the meter reflects the boosted
//! signal while the WAV render and the waveform stay faithful. Classic tracker
//! behaviour: instant attack, a steady fall, and a per-channel peak-hold
//! marker that lingers before falling. Every colour comes from the active
//! [`Palette`], so each theme styles it.

use egui::{Color32, Rect, Sense, pos2, vec2};

use crate::theme::Palette;
use crate::theme::bevel::{self, Bevel};

/// Total meter width: two channel columns, their gap, and the insets.
pub const WIDTH: f32 = 22.0;

/// How fast the displayed bar falls, in display fractions per second.
const FALL_PER_SEC: f32 = 1.5;
/// How long the peak-hold marker sits before it starts to fall.
const HOLD_SECONDS: f32 = 1.0;

/// One lit block plus its gap, in pixels; segments draw bottom-up.
const SEGMENT_PITCH: f32 = 7.0;
const SEGMENT_GAP: f32 = 2.0;
/// Padding between the well's bevel and the columns.
const INSET: f32 = 2.0;

/// Zone boundaries, as fractions of the meter's height.
const MID_ZONE: f32 = 0.60;
const HIGH_ZONE: f32 = 0.85;

/// The meter's displayed state, owned by the app and advanced every frame.
#[derive(Debug, Default)]
pub struct PeakMeterState {
    /// The displayed bar per channel, `0..=1` in display fractions.
    level: [f32; 2],
    /// The peak-hold marker per channel, in the same fractions.
    hold: [f32; 2],
    /// Seconds since each hold was last raised.
    hold_age: [f32; 2],
}

impl PeakMeterState {
    /// Advances the meter by `dt` seconds. `peaks` are the raw output peaks
    /// (`0..=1`) taken from the audio service, `None` when nothing is loaded.
    /// Attack is instant; release is a steady fall.
    pub fn update(&mut self, peaks: Option<[f32; 2]>, dt: f32) {
        let peaks = peaks.unwrap_or([0.0; 2]);
        for (channel, peak) in peaks.into_iter().enumerate() {
            // sqrt is a cheap perceptual-ish curve: a quiet capture still
            // shows a readable bar instead of hugging the bottom pixel.
            let target = peak.clamp(0.0, 1.0).sqrt();
            // `max` floors the fall at the new target (and at 0.0 in silence),
            // which is also what makes the attack instant.
            self.level[channel] = target.max(self.level[channel] - FALL_PER_SEC * dt);
            if target >= self.hold[channel] {
                self.hold[channel] = target;
                self.hold_age[channel] = 0.0;
            } else {
                self.hold_age[channel] += dt;
                if self.hold_age[channel] > HOLD_SECONDS {
                    self.hold[channel] = (self.hold[channel] - FALL_PER_SEC * dt).max(0.0);
                }
            }
        }
    }

    /// Whether anything is still lit, so the app keeps repainting until the
    /// bars and markers have fully decayed.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.level.iter().chain(&self.hold).any(|&v| v > 0.0)
    }
}

/// Draws the meter at [`WIDTH`], filling the available height.
pub fn show(ui: &mut egui::Ui, state: &PeakMeterState, palette: &Palette) {
    let (response, painter) =
        ui.allocate_painter(vec2(WIDTH, ui.available_height()), Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 0.0, palette.wf_bg);

    let inner = rect.shrink(INSET);
    let count = (inner.height() / SEGMENT_PITCH).floor() as usize;
    if count > 0 {
        let column_width = (inner.width() - SEGMENT_GAP) / 2.0;
        for channel in 0..2 {
            let left = inner.left() + channel as f32 * (column_width + SEGMENT_GAP);
            draw_column(
                &painter,
                left,
                column_width,
                inner.bottom(),
                count,
                state.level[channel],
                state.hold[channel],
                palette,
            );
        }
    }

    // The same sunken-well frame as the waveform beside it.
    bevel::paint_bevel(&painter, rect, palette, Bevel::Sunken);
}

/// One channel's column: `count` segments from `bottom` upward.
#[expect(clippy::too_many_arguments, reason = "plain geometry, locally called")]
fn draw_column(
    painter: &egui::Painter,
    left: f32,
    width: f32,
    bottom: f32,
    count: usize,
    level: f32,
    hold: f32,
    palette: &Palette,
) {
    let lit = (level * count as f32).round() as usize;
    // The hold marker's segment, shown only once it sits above the bar.
    let hold_segment =
        (hold > 0.0).then(|| ((hold * count as f32).ceil() as usize).clamp(1, count) - 1);
    for index in 0..count {
        let seg_bottom = bottom - index as f32 * SEGMENT_PITCH;
        let seg_top = seg_bottom - (SEGMENT_PITCH - SEGMENT_GAP);
        let color = if hold_segment == Some(index) && index >= lit {
            palette.meter_hold
        } else if index < lit {
            zone_color(index, count, palette)
        } else {
            palette.meter_off
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(left, seg_top), pos2(left + width, seg_bottom)),
            0.0,
            color,
        );
    }
}

/// Which colour zone a segment belongs to, by the fraction of the meter's
/// height at the segment's centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Low,
    Mid,
    High,
}

fn zone(index: usize, count: usize) -> Zone {
    let fraction = (index as f32 + 0.5) / count as f32;
    if fraction >= HIGH_ZONE {
        Zone::High
    } else if fraction >= MID_ZONE {
        Zone::Mid
    } else {
        Zone::Low
    }
}

fn zone_color(index: usize, count: usize, palette: &Palette) -> Color32 {
    match zone(index, count) {
        Zone::Low => palette.meter_low,
        Zone::Mid => palette.meter_mid,
        Zone::High => palette.meter_high,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_is_instant() {
        let mut meter = PeakMeterState::default();
        meter.update(Some([0.25, 1.0]), 0.016);
        // sqrt display mapping; the hold rises with the bar.
        assert_eq!(meter.level, [0.5, 1.0]);
        assert_eq!(meter.hold, [0.5, 1.0]);
    }

    #[test]
    fn release_falls_at_the_configured_rate() {
        let mut meter = PeakMeterState::default();
        meter.update(Some([1.0, 1.0]), 0.0);
        meter.update(Some([0.0, 0.0]), 0.1);
        let expected = 1.0 - FALL_PER_SEC * 0.1;
        assert!((meter.level[0] - expected).abs() < 1e-6);
        assert!((meter.level[1] - expected).abs() < 1e-6);
    }

    #[test]
    fn the_hold_marker_lingers_then_falls() {
        let mut meter = PeakMeterState::default();
        meter.update(Some([1.0, 1.0]), 0.0);
        // Within the hold time the marker stays put while the bar falls.
        meter.update(None, HOLD_SECONDS * 0.9);
        assert_eq!(meter.hold, [1.0, 1.0]);
        assert!(meter.level[0] < 1.0);
        // Past the hold time it starts to fall too.
        meter.update(None, HOLD_SECONDS * 0.2);
        assert!(meter.hold[0] < 1.0);
    }

    #[test]
    fn decays_to_inactive_and_floors_at_zero() {
        let mut meter = PeakMeterState::default();
        assert!(!meter.is_active());
        meter.update(Some([1.0, 0.5]), 0.016);
        assert!(meter.is_active());
        for _ in 0..40 {
            meter.update(None, 0.1);
        }
        assert_eq!(meter.level, [0.0, 0.0]);
        assert_eq!(meter.hold, [0.0, 0.0]);
        assert!(!meter.is_active());
    }

    #[test]
    fn zones_split_low_mid_high() {
        // 20 segments: centres at 0.025, 0.075, ..., 0.975.
        assert_eq!(zone(0, 20), Zone::Low);
        assert_eq!(zone(11, 20), Zone::Low); // centre 0.575, below 0.60
        assert_eq!(zone(12, 20), Zone::Mid); // centre 0.625
        assert_eq!(zone(16, 20), Zone::Mid); // centre 0.825, below 0.85
        assert_eq!(zone(17, 20), Zone::High); // centre 0.875
        assert_eq!(zone(19, 20), Zone::High);
    }
}
