//! The VGM metadata dialog. Modal.
//!
//! - The loop start is an *instruction index* (`VgmMeta::loop_point`), not a
//!   raw byte offset, and may be empty for "no loop".
//! - The loop length in samples is derived from the loop point, so it is
//!   displayed read-only rather than edited.
//! - Save applies the metadata, and invalid input gets an error box.

use crate::action::{Action, EditAction, MixerAction, UiAction};
use crate::theme::{Palette, bevel};

/// These fields all hold a number, so they are sized for one rather than
/// stretched across the dialog. They still wrap and grow rather than hiding
/// whatever ends up typed or pasted into them.
const FIELD_WIDTH: f32 = 160.0;

/// Formats a loop-point instruction index for its field: hex with the `0x`
/// prefix, matching the editor's "Pos." column (`{:#06X}`), so a value looked up
/// in the table reads the same as it is typed here.
fn format_pos(index: usize) -> String {
    format!("{index:#06X}")
}

/// Parses a loop-point field as hex, tolerating an optional `0x`/`0X` prefix and
/// surrounding whitespace. `None` if it is not a hex number (the caller decides
/// whether an empty field means "no loop" / "end of song" before calling).
fn parse_pos(text: &str) -> Option<usize> {
    let trimmed = text.trim();
    let digits = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    usize::from_str_radix(digits, 16).ok()
}

#[derive(Debug)]
pub struct VgmMetadataDialog {
    loop_point: String,
    loop_end: String,
    loop_base: String,
    loop_modifier: String,
    volume_modifier: String,
    /// A linear multiplier the user can type instead of the raw byte; "Apply"
    /// floors it to the nearest ladder value and writes that into
    /// [`Self::volume_modifier`]. Kept as its own text so a half-typed value is
    /// not snapped out from under the user.
    volume_multiplier: String,
    /// The peak from the most recent "Measure", for the dBFS/clipping readout.
    /// `None` until a measurement lands.
    measured: Option<vgms_synth::Peak>,
    /// One past the highest valid loop point.
    song_len: usize,
    /// Cumulative samples before each instruction (len = `song_len + 1`),
    /// captured at open so the read-only loop-length readout can be recomputed
    /// live from the typed loop point.
    samples_prefix: Vec<u32>,
}

impl VgmMetadataDialog {
    /// The dialog for a document held as a VGM, whose fields are in the header
    /// itself and whose row times come from its own waits. (A DRO carries no VGM
    /// metadata, so there is no `DroSong`-based constructor.)
    #[must_use]
    pub fn for_vgm(file: &vgms_core::VgmFile, measured: Option<vgms_synth::Peak>) -> Option<Self> {
        let stream = file.stream()?;
        let mut prefix = Vec::with_capacity(stream.len() + 1);
        let mut elapsed = 0u32;
        prefix.push(elapsed);
        for index in 0..stream.len() {
            elapsed = elapsed.saturating_add(stream.wait_samples(index));
            prefix.push(elapsed);
        }
        let mut dialog = Self::from_fields(
            file.loop_index(),
            file.loop_end_index(),
            file.header.loop_base(),
            file.header.loop_modifier(),
            file.header.volume_modifier(),
            stream.len(),
            prefix,
        );
        // Seed the last measurement (an editor "Match" or a prior "Measure") so
        // the readout and the "From measured" button light up without a re-scan.
        // The header's own saved modifier is left as-is; only a button press fills.
        dialog.measured = measured;
        Some(dialog)
    }

    fn from_fields(
        loop_point: Option<usize>,
        loop_end: Option<usize>,
        loop_base: u8,
        loop_modifier: u8,
        volume_modifier: u8,
        song_len: usize,
        samples_prefix: Vec<u32>,
    ) -> Self {
        Self {
            loop_point: loop_point.map_or_else(String::new, format_pos),
            loop_end: loop_end.map_or_else(String::new, format_pos),
            loop_base: loop_base.to_string(),
            loop_modifier: loop_modifier.to_string(),
            volume_modifier: volume_modifier.to_string(),
            volume_multiplier: String::new(),
            measured: None,
            song_len,
            samples_prefix,
        }
    }

    /// Fills the volume-modifier field with the value that would bring the just-
    /// measured `peak` to full scale, and remembers the peak for the readout.
    /// Called by the app when a "Measure" scan lands.
    pub fn apply_measured_peak(&mut self, peak: vgms_synth::Peak) {
        self.set_modifier_from_peak(peak);
        self.measured = Some(peak);
    }

    /// Fills the modifier byte with the one that lifts `peak` to full scale,
    /// without touching the stored measurement. Shared by the "Measure" scan
    /// (which then stores the peak) and the "From measured" button (which reuses
    /// the peak already stored).
    fn set_modifier_from_peak(&mut self, peak: vgms_synth::Peak) {
        self.volume_modifier = vgms_core::suggest_volume_modifier(peak.max_level, None).to_string();
    }

    /// Floors a typed linear multiplier onto the modifier ladder and writes the
    /// resulting byte into the modifier field. Flooring (not rounding) keeps the
    /// applied factor at or below what was asked, so it never overshoots into
    /// clipping. A non-numeric field is left alone rather than cleared.
    fn apply_multiplier(&mut self) {
        if let Ok(factor) = self.volume_multiplier.trim().parse::<f32>() {
            self.volume_modifier = vgms_core::floor_volume_modifier(factor).to_string();
        }
    }

    /// The "= N.NNx" gloss beside the volume-modifier field: what factor the
    /// typed byte asks players for, plus the measured peak's dBFS (and a clipping
    /// note) once a measurement has landed. `(invalid)` if the byte does not parse.
    fn volume_modifier_readout(&self) -> String {
        let Ok(byte) = self.volume_modifier.trim().parse::<u8>() else {
            return "(invalid)".to_owned();
        };
        let factor = vgms_core::volume_modifier_factor(byte);
        let mut text = format!("= {factor:.2}\u{00d7}");
        if let Some(peak) = self.measured {
            let dbfs = vgms_core::peak_dbfs(peak.max_level);
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
        let footer = super::Footer::new(palette, "Save");
        let open = super::dialog_modal(
            ctx,
            "vgm-metadata-modal",
            "VGM Metadata",
            palette,
            |ui| {
                egui::Grid::new("vgm-meta-grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Loop start (Pos., hex):");
                        ui.add(
                            super::wrapping_edit(&mut self.loop_point, palette, FIELD_WIDTH, 1)
                                .return_key(None)
                                .hint_text("empty = no loop"),
                        );
                        ui.end_row();

                        ui.label("Loop end (Pos., hex):");
                        ui.add(
                            super::wrapping_edit(&mut self.loop_end, palette, FIELD_WIDTH, 1)
                                .return_key(None)
                                .hint_text("empty = end of song"),
                        );
                        ui.end_row();

                        ui.label("Loop length (samples):");
                        let mut samples = self.loop_samples_display();
                        ui.add_enabled(
                            false,
                            super::wrapping_edit(&mut samples, palette, FIELD_WIDTH, 1),
                        );
                        ui.end_row();

                        for (label, value) in [
                            ("Loop base:", &mut self.loop_base),
                            ("Loop modifier:", &mut self.loop_modifier),
                        ] {
                            ui.label(label);
                            super::text_field(ui, palette, value, FIELD_WIDTH);
                            ui.end_row();
                        }

                        // Volume modifier gets a "Measure" button that scans and
                        // fills it from the song's peak, a "From measured" button
                        // that reuses a peak already measured (in the editor or a
                        // prior scan) without re-scanning, and a live gloss of what
                        // the byte means.
                        ui.label("Volume modifier:");
                        ui.horizontal(|ui| {
                            super::text_field(ui, palette, &mut self.volume_modifier, 70.0);
                            if bevel::button(ui, palette, "Measure")
                                .on_hover_text(crate::strings::VGM_METADATA_MEASURE_HINT)
                                .clicked()
                            {
                                actions.push(Action::Mixer(MixerAction::MeasureVolumeModifier));
                            }
                            ui.add_enabled_ui(self.measured.is_some(), |ui| {
                                if bevel::button(ui, palette, "From measured")
                                    .on_hover_text(crate::strings::VGM_METADATA_FROM_MEASURED_HINT)
                                    .clicked()
                                    && let Some(peak) = self.measured
                                {
                                    self.set_modifier_from_peak(peak);
                                }
                            });
                            ui.label(self.volume_modifier_readout());
                        });
                        ui.end_row();

                        // An alternative way in: type a linear multiplier and
                        // floor it onto the ladder, rather than the raw byte above.
                        ui.label("Volume multiplier:");
                        ui.horizontal(|ui| {
                            super::text_field(ui, palette, &mut self.volume_multiplier, 70.0);
                            ui.label("\u{00d7}");
                            if bevel::button(ui, palette, "Apply")
                                .on_hover_text(crate::strings::VGM_METADATA_MULTIPLIER_HINT)
                                .clicked()
                            {
                                self.apply_multiplier();
                            }
                        });
                        ui.end_row();
                    });
            },
            |ui| footer.show(ui),
        );
        // Only a clicked Save runs the validation; a refused one leaves the dialog open.
        let saved = footer.primary_clicked() && self.save(actions);
        open && !(footer.closed() || saved)
    }

    /// Parses and emits the save; `false` (with an error box queued) if any
    /// field is invalid.
    fn save(&mut self, actions: &mut Vec<Action>) -> bool {
        let trimmed = self.loop_point.trim();
        let loop_point = if trimmed.is_empty() {
            None
        } else {
            match parse_pos(trimmed) {
                Some(index) if index < self.song_len => Some(index),
                _ => {
                    actions.push(Action::Ui(UiAction::Alert {
                        title: crate::strings::VGM_METADATA_INVALID_TITLE.to_owned(),
                        message: crate::strings::vgm_metadata_loop_start_message(self.song_len),
                    }));
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
                actions.push(Action::Ui(UiAction::Alert {
                    title: crate::strings::VGM_METADATA_INVALID_TITLE.to_owned(),
                    message: crate::strings::vgm_metadata_loop_end_message(self.song_len),
                }));
                return false;
            }
            (Some(start), Some(end)) if end <= start => {
                actions.push(Action::Ui(UiAction::Alert {
                    title: crate::strings::VGM_METADATA_INVALID_TITLE.to_owned(),
                    message: crate::strings::VGM_METADATA_LOOP_END_AFTER_START.to_owned(),
                }));
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
            actions.push(Action::Ui(UiAction::Alert {
                title: crate::strings::VGM_METADATA_INVALID_TITLE.to_owned(),
                message: crate::strings::VGM_METADATA_UPDATE_ERROR.to_owned(),
            }));
            return false;
        };

        actions.push(Action::Edit(EditAction::SaveVgmMetadata {
            loop_point,
            loop_end,
            loop_base,
            loop_modifier,
            volume_modifier,
        }));
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
        let Some(start) = parse_pos(trimmed) else {
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
        parse_pos(trimmed).filter(|&end| end <= self.song_len)
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
            volume_multiplier: String::new(),
            measured: None,
            song_len: 4,
            samples_prefix: vec![0, 10, 30, 60, 100],
        }
    }

    #[test]
    fn loop_points_parse_as_hex_with_an_optional_prefix() {
        assert_eq!(parse_pos("0x1A"), Some(0x1A));
        assert_eq!(parse_pos("1a"), Some(0x1A));
        assert_eq!(parse_pos("  0X0F  "), Some(0x0F));
        assert_eq!(
            parse_pos("10"),
            Some(0x10),
            "bare digits are hex, not decimal"
        );
        assert_eq!(parse_pos(""), None);
        assert_eq!(parse_pos("zz"), None);
    }

    #[test]
    fn loop_points_display_as_prefixed_hex() {
        assert_eq!(format_pos(0), "0x0000");
        assert_eq!(format_pos(0x1A), "0x001A");
        assert_eq!(format_pos(0x1_2345), "0x12345");
    }

    #[test]
    fn for_fields_seeds_the_loop_fields_in_hex() {
        let dialog =
            VgmMetadataDialog::from_fields(Some(0x2A), Some(0x30), 0, 0, 0, 0x40, vec![0; 0x41]);
        assert_eq!(dialog.loop_point, "0x002A");
        assert_eq!(dialog.loop_end, "0x0030");
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
            [Action::Edit(EditAction::SaveVgmMetadata {
                loop_point: Some(1),
                loop_end: None,
                ..
            })]
        ));
    }

    #[test]
    fn saving_an_end_that_bounds_nothing_is_refused_with_a_reason() {
        let mut dialog = dialog();
        dialog.loop_point = "2".to_owned();
        dialog.loop_end = "2".to_owned();
        let mut actions = Vec::new();
        assert!(!dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::Ui(UiAction::Alert { .. })]
        ));
    }

    #[test]
    fn measuring_fills_the_field_from_the_peak() {
        let mut dialog = dialog();
        // A half-scale peak suggests a +6 dB modifier: byte 0x20 = 32.
        dialog.apply_measured_peak(vgms_synth::Peak {
            max_level: 0x4000,
            clipped: false,
        });
        assert_eq!(dialog.volume_modifier, "32");
        // Saving carries the freshly measured byte through.
        let mut actions = Vec::new();
        assert!(dialog.save(&mut actions));
        assert!(matches!(
            actions.as_slice(),
            [Action::Edit(EditAction::SaveVgmMetadata {
                volume_modifier: 32,
                ..
            })]
        ));
    }

    #[test]
    fn applying_a_multiplier_floors_it_onto_the_ladder() {
        let mut dialog = dialog();
        // An exact ladder value lands on its byte: 2.0x -> 0x20 = 32.
        dialog.volume_multiplier = "2.0".to_owned();
        dialog.apply_multiplier();
        assert_eq!(dialog.volume_modifier, "32");
        // A between-rungs value floors below itself, never above (clip-safe):
        // 2.3x decodes back to <= 2.3.
        dialog.volume_multiplier = "2.3".to_owned();
        dialog.apply_multiplier();
        let byte: u8 = dialog.volume_modifier.parse().expect("a byte");
        assert!(vgms_core::volume_modifier_factor(byte) <= 2.3);
        // A non-numeric field is left alone, not cleared.
        dialog.volume_modifier = "5".to_owned();
        dialog.volume_multiplier = "??".to_owned();
        dialog.apply_multiplier();
        assert_eq!(dialog.volume_modifier, "5");
    }

    #[test]
    fn from_measured_fills_the_byte_from_a_seeded_peak() {
        // for_vgm seeds `measured` from the app's last measurement; the "From
        // measured" button reuses it with no fresh scan.
        let mut dialog = dialog();
        dialog.measured = Some(vgms_synth::Peak {
            max_level: 0x4000, // half scale -> +6 dB = 0x20 = 32
            clipped: false,
        });
        dialog.set_modifier_from_peak(dialog.measured.expect("seeded"));
        assert_eq!(dialog.volume_modifier, "32");
        assert!(
            dialog.measured.is_some(),
            "the stored peak is left in place"
        );
    }

    #[test]
    fn the_volume_readout_decodes_the_byte_and_notes_clipping() {
        let mut dialog = dialog();
        // Byte 32 = 0x20 = a 2x factor, before any measurement.
        dialog.volume_modifier = "32".to_owned();
        assert_eq!(dialog.volume_modifier_readout(), "= 2.00\u{00d7}");
        // After a clipping measurement, the peak and a clipping note are appended.
        dialog.apply_measured_peak(vgms_synth::Peak {
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
