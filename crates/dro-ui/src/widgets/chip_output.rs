//! Which core plays each chip, one row per chip this app can play at all.
//!
//! "Output" used to be one setting, because there was one chip: the RetroWave
//! board is an OPL3, so choosing it *was* choosing how OPL played. Then it
//! became one row per chip. Now each row is a **core picker** -- because a chip
//! can have more than one emulator, they do not sound alike, and which one is
//! running is the user's call.
//!
//! Every row comes from [`dro_synth::registry`], including the licenses shown
//! beside the labels: a core that is not registered cannot be chosen, and one
//! that is registered cannot be offered without its terms. A build that never
//! registered a provider -- the web build has no serial ports, so no RetroWave
//! -- simply has no such entry, and the dialog stops offering something it
//! cannot deliver rather than hiding the gap.
//!
//! Chips are grouped by what they share a core with: the OPL family is one row,
//! because one core (or one board) plays all four. A chip with no core at all
//! is not a row but a tally, which is the honest answer to "why is my Mega
//! Drive rip silent".

use dro_core::vgm::ChipKind;
use dro_synth::registry::{self, CoreInfo};

use crate::theme::Palette;

/// One core a row can be set to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreChoice {
    /// What `drotrim.ini` stores: the core id without its slot prefix.
    pub name: String,
    /// What the dropdown shows.
    pub label: String,
    /// SPDX expression, shown small. A user picking a core should see what it
    /// costs them before they pick it.
    pub license: String,
}

/// One row: a chip (or a family of them), and the cores it can play through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipOutputRow {
    /// The config slot, e.g. `"opl3"` -- what `core.<slot>=` names.
    pub slot: &'static str,
    /// What to call this group, e.g. `"OPL2 / OPL3"`.
    pub label: &'static str,
    /// A representative chip, for looking the row back up in the registry.
    pub chip: ChipKind,
    /// The cores available, best first. Never empty -- a chip with none is
    /// counted by [`without_cores`] instead of listed.
    pub cores: Vec<CoreChoice>,
}

impl ChipOutputRow {
    /// Whether this row offers a choice at all, rather than stating a fact.
    #[must_use]
    pub fn is_choice(&self) -> bool {
        self.cores.len() > 1
    }
}

/// How the OPL family is labelled: four chips, one core, one row.
///
/// A RetroWave OPL3 plays an OPL2 or an OPL3 rip because an OPL3 *is* an OPL2
/// with more of it; the Y8950 and YM3526 are the same register file again.
const OPL_LABEL: &str = "OPL2 / OPL3";

/// The chip whose registry entries stand for the whole OPL family.
const OPL_REPRESENTATIVE: ChipKind = ChipKind::Ymf262;

/// Every row the dialog shows, in the order it shows them.
///
/// The OPL row first because it is the one most likely to have a choice, then
/// every other chip with a core, in header order.
#[must_use]
pub fn rows() -> Vec<ChipOutputRow> {
    let mut rows = Vec::new();
    if let Some(row) = row_for(OPL_REPRESENTATIVE, OPL_LABEL) {
        rows.push(row);
    }
    for chip in ChipKind::all() {
        // The OPL family is already the first row; the other three OPL chips
        // would list it again under their own names.
        if registry::is_opl(chip) {
            continue;
        }
        if let Some(row) = row_for(chip, chip.name()) {
            rows.push(row);
        }
    }
    rows
}

/// A row for `chip`, or `None` when nothing plays it.
fn row_for(chip: ChipKind, label: &'static str) -> Option<ChipOutputRow> {
    let cores: Vec<CoreChoice> = registry::registry().for_chip(chip).map(choice).collect();
    (!cores.is_empty()).then(|| ChipOutputRow {
        slot: registry::slot_slug(chip),
        label,
        chip,
        cores,
    })
}

/// A registry entry as the dropdown needs it: the id stripped of the slot it is
/// already filed under, since the config stores `core.opl3=nuked`, not
/// `core.opl3=opl3.nuked`.
fn choice(info: &CoreInfo) -> CoreChoice {
    let prefix = format!("{}.", registry::slot_slug(info.chip));
    CoreChoice {
        name: info.id.strip_prefix(&prefix).unwrap_or(info.id).to_owned(),
        label: info.label.to_owned(),
        license: info.license.to_owned(),
    }
}

/// How many of the spec's chips have no core yet, for the closing line.
#[must_use]
pub fn without_cores() -> usize {
    let registry = registry::registry();
    ChipKind::all()
        .filter(|&chip| !registry::is_opl(chip) && !registry.has_core(chip))
        .count()
}

/// Draws the rows into an open two-column grid.
///
/// `cores` is the config's slot map, edited in place. `port_row` draws the
/// RetroWave device picker, which belongs under the row that selected the
/// board -- so it is a callback rather than a row of its own, and it appears
/// only when hardware is the OPL choice.
pub fn show(
    ui: &mut egui::Ui,
    palette: &Palette,
    cores: &mut std::collections::BTreeMap<String, String>,
    port_row: &mut dyn FnMut(&mut egui::Ui),
) {
    for row in rows() {
        ui.label(row.label);
        // A row with one core states it; the licence still shows, because the
        // notice is not conditional on there being an alternative.
        let selected = selected_name(cores, &row);
        if row.is_choice() {
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                let mut choice = selected.clone();
                egui::ComboBox::from_id_salt(format!("settings-core-{}", row.slot))
                    .selected_text(label_for(&row, &choice))
                    .show_ui(ui, |ui| {
                        for core in &row.cores {
                            ui.selectable_value(
                                &mut choice,
                                core.name.clone(),
                                format!("{}  --  {}", core.label, core.license),
                            );
                        }
                    });
                if choice != selected {
                    cores.insert(row.slot.to_owned(), choice);
                }
            });
            ui.end_row();
        } else {
            let core = &row.cores[0];
            ui.colored_label(palette.muted, &core.label)
                .on_hover_text(&core.license);
            ui.end_row();
        }

        // The board's port belongs to the row that chose the board.
        if row.slot == dro_core::config::OPL_SLOT
            && selected_name(cores, &row) == dro_core::config::RETROWAVE_CORE
        {
            port_row(ui);
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

/// The core selected for `row`: the configured one if this build has it, the
/// first otherwise.
///
/// A config can name a core this build lacks -- the web build has no RetroWave
/// -- and the dropdown must then show what will *actually* play rather than
/// echoing a setting that is not in effect.
fn selected_name(
    cores: &std::collections::BTreeMap<String, String>,
    row: &ChipOutputRow,
) -> String {
    cores
        .get(row.slot)
        .filter(|name| row.cores.iter().any(|core| &core.name == *name))
        .cloned()
        .unwrap_or_else(|| row.cores[0].name.clone())
}

/// The label for a core name, falling back to the name itself.
fn label_for(row: &ChipOutputRow, name: &str) -> String {
    row.cores
        .iter()
        .find(|core| core.name == name)
        .map_or_else(|| name.to_owned(), |core| core.label.clone())
}

/// Installs a registry shaped like the real app's, for tests.
///
/// `dro-ui` alone knows only `dro-synth`'s built-in cores, so without this its
/// OPL row would state one core rather than offer a choice -- and the dialog
/// snapshot would stop documenting the hardware picker, which is the part most
/// worth catching a regression in. The hardware entry is declared here rather
/// than depended on: `dro-retrowave` is native-only and `dro-ui` compiles to
/// wasm, so this crate must not link it. The two declarations agreeing is what
/// `the_test_registry_matches_the_apps` in `dro-trimmer` checks.
///
/// Idempotent and safe to call from any test in any order: every caller
/// installs the same content, and the first one wins.
#[cfg(test)]
pub(crate) fn install_test_cores() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut registry = dro_synth::CoreRegistry::with_builtins();
        for chip in registry::OPL_CHIPS {
            registry.register(CoreInfo {
                id: "opl3.retrowave",
                chip,
                label: "RetroWave OPL3 (hardware)",
                authors: "SudoMaker (the board); this project (the protocol)",
                license: "GPL-2.0-or-later",
                upstream: "https://github.com/SudoMaker/RetroWave",
                realtime: true,
                make: dro_synth::CoreMaker::Routed,
            });
        }
        // Already installed means another test got here first with the same
        // content, which is the point of the `Once`.
        let _ = dro_synth::install(registry);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_opl_row_comes_first_and_names_the_family() {
        install_test_cores();
        let rows = rows();
        assert_eq!(rows[0].label, OPL_LABEL);
        assert_eq!(rows[0].slot, dro_core::config::OPL_SLOT);
        assert!(
            rows[0].is_choice(),
            "emulator or board -- the one row that has always offered two"
        );
    }

    #[test]
    fn a_chip_with_a_core_gets_a_row_and_one_without_does_not() {
        install_test_cores();
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
        install_test_cores();
        let opl = ChipKind::all()
            .filter(|&chip| registry::is_opl(chip))
            .count();
        let listed = rows().len() - 1; // the OPL row stands for `opl` chips
        assert_eq!(opl + listed + without_cores(), ChipKind::all().count());
    }

    /// Every row is a picker over registry entries, so a core cannot appear
    /// without the license it must be offered under.
    #[test]
    fn every_offered_core_shows_its_license() {
        install_test_cores();
        for row in rows() {
            assert!(!row.cores.is_empty(), "{} is a row with no core", row.label);
            for core in &row.cores {
                assert!(!core.label.is_empty(), "{}: unnamed core", row.label);
                assert!(!core.license.is_empty(), "{}: no license", core.label);
                assert!(
                    !core.name.contains('.'),
                    "{} still carries its slot prefix; config would store it twice",
                    core.name
                );
            }
        }
    }

    /// A `drotrim.ini` naming a core this build lacks -- the native app's
    /// `retrowave` read by the web build -- must show what will really play,
    /// not echo a setting that is not in force.
    #[test]
    fn a_core_this_build_lacks_falls_back_to_the_first() {
        install_test_cores();
        let row = rows().into_iter().next().expect("an OPL row");
        let mut cores = BTreeMap::from([(row.slot.to_owned(), "nonesuch".to_owned())]);
        assert_eq!(selected_name(&cores, &row), row.cores[0].name);

        cores.insert(row.slot.to_owned(), row.cores[0].name.clone());
        assert_eq!(selected_name(&cores, &row), row.cores[0].name);
    }
}
