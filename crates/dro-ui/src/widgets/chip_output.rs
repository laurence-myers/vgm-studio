//! Where each chip's sound comes out, one row per chip this app can play.
//!
//! "Output" used to be one setting, because there was one chip: the RetroWave
//! board is an OPL3, so choosing it *was* choosing how OPL played. Now that a
//! VGM can name forty-two chips and this app has cores for some of them, the
//! question is per chip -- and asking it that way answers a second one the old
//! row could not: which chips can this app play at all, and how?
//!
//! Chips are grouped by what they share an output with. The OPL family is one
//! row because one board plays all of it; a chip with a single core is a row
//! stating that core; a chip with none is a row saying so, which is the honest
//! answer to "why is my Mega Drive rip silent".

use dro_core::config::OutputBackend;
use dro_core::vgm::ChipKind;

use crate::theme::Palette;

/// One row of the output settings: some chips, and where their sound goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChipOutputRow {
    /// What to call this group, e.g. `"OPL2 / OPL3"`.
    pub label: &'static str,
    /// A representative chip, for the rows that have only one core to name.
    pub chip: ChipKind,
    /// Whether this row's chips can go to the RetroWave board as well as the
    /// emulator. Only the OPL family can: the board is an OPL3.
    pub has_hardware: bool,
    /// What the emulated core is called. Named where the name means something
    /// -- "Nuked OPL3" is a specific well-known implementation -- and plain
    /// `"emulated"` where repeating the chip's own name would say nothing.
    /// `None` when there is no core yet.
    pub core: Option<&'static str>,
}

impl ChipOutputRow {
    /// Whether this row offers a choice at all, rather than stating a fact.
    #[must_use]
    pub const fn is_choice(&self) -> bool {
        self.has_hardware
    }
}

/// The OPL family: four chips, one board, one row.
///
/// A RetroWave OPL3 plays an OPL2 or an OPL3 rip because an OPL3 *is* an OPL2
/// with more of it; the Y8950 and YM3526 are the same register file again.
const OPL_ROW: ChipOutputRow = ChipOutputRow {
    label: "OPL2 / OPL3",
    chip: ChipKind::Ymf262,
    has_hardware: true,
    core: Some("Nuked OPL3"),
};

/// Every row the dialog shows, in the order it shows them.
///
/// The OPL row first because it is the one with a choice, then every chip this
/// app has a core for, then a single line for the rest. The list of cores is
/// [`dro_synth::core_for`]'s, asked rather than restated, so a core landing in
/// `dro-synth` shows up here without anyone remembering to add it.
#[must_use]
pub fn rows() -> Vec<ChipOutputRow> {
    let mut rows = vec![OPL_ROW];
    for chip in ChipKind::all() {
        // The OPL family is already the first row; asking the registry about it
        // would list it twice (and it has no `ChipCore` -- it plays through the
        // OPL player, which is why it is spelled out above).
        if is_opl(chip) {
            continue;
        }
        if dro_synth::core_for(chip).is_some() {
            rows.push(ChipOutputRow {
                label: chip.name(),
                chip,
                has_hardware: false,
                core: Some("emulated"),
            });
        }
    }
    rows
}

/// Whether `chip` is one the OPL row governs.
const fn is_opl(chip: ChipKind) -> bool {
    matches!(
        chip,
        ChipKind::Ym3812 | ChipKind::Ymf262 | ChipKind::Ym3526 | ChipKind::Y8950
    )
}

/// How many of the spec's chips have no core yet, for the closing line.
#[must_use]
pub fn without_cores() -> usize {
    ChipKind::all()
        .filter(|&chip| !is_opl(chip) && dro_synth::core_for(chip).is_none())
        .count()
}

/// Draws the rows into an open two-column grid, and returns the OPL backend the
/// user chose.
///
/// Only the OPL row is interactive, because it is the only one with two places
/// its sound could go. The rest are read-outs -- which is the point: a settings
/// page that lists what it *cannot* play is more use than one that hides it.
pub fn show(
    ui: &mut egui::Ui,
    palette: &Palette,
    backend: &mut OutputBackend,
    port_row: &mut dyn FnMut(&mut egui::Ui),
) {
    for row in rows() {
        ui.label(row.label);
        if row.is_choice() {
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                egui::ComboBox::from_id_salt(format!("settings-output-{}", row.chip.name()))
                    .selected_text(backend_label(*backend))
                    .show_ui(ui, |ui| {
                        for choice in [OutputBackend::Emulated, OutputBackend::RetroWave] {
                            ui.selectable_value(backend, choice, backend_label(choice));
                        }
                    });
            });
            ui.end_row();
            // The board's port belongs to the row that chose the board.
            if *backend == OutputBackend::RetroWave {
                port_row(ui);
            }
        } else {
            ui.colored_label(palette.muted, row.core.map_or("no core yet", |core| core));
            ui.end_row();
        }
    }

    let missing = without_cores();
    if missing > 0 {
        ui.colored_label(palette.muted, "Other chips");
        ui.colored_label(palette.muted, format!("{missing} with no core yet"))
            .on_hover_text(
                "A VGM naming one of these opens and edits like any other; it just \
                 plays silence where that chip would have been.",
            );
        ui.end_row();
    }
}

/// How a backend is named in the dropdown.
#[must_use]
pub fn backend_label(backend: OutputBackend) -> &'static str {
    match backend {
        OutputBackend::Emulated => "Nuked OPL3 (emulated)",
        OutputBackend::RetroWave => "RetroWave OPL3 (hardware)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opl_row_comes_first_and_is_the_only_choice() {
        let rows = rows();
        assert_eq!(rows[0].label, "OPL2 / OPL3");
        assert!(rows[0].is_choice(), "one board, two places to send OPL");
        assert!(
            rows[1..].iter().all(|row| !row.is_choice()),
            "every other chip has one core or none"
        );
    }

    #[test]
    fn a_chip_with_a_core_gets_a_row_and_one_without_does_not() {
        let rows = rows();
        assert!(
            rows.iter().any(|row| row.chip == ChipKind::Sn76489),
            "the SN76489 has a core, so it is listed"
        );
        assert!(
            !rows.iter().any(|row| row.chip == ChipKind::Ym2612),
            "the YM2612 does not yet, so it is counted rather than listed"
        );
    }

    /// The rows and the count between them must cover the whole chip table, or
    /// the dialog is quietly leaving chips out.
    #[test]
    fn every_chip_is_either_a_row_or_counted() {
        let opl = ChipKind::all().filter(|&chip| is_opl(chip)).count();
        let listed = rows().len() - 1; // the OPL row stands for `opl` chips
        assert_eq!(opl + listed + without_cores(), ChipKind::all().count());
    }
}
