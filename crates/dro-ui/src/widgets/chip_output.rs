//! Which core plays each chip, one row per chip this app can play at all.
//!
//! "Output" used to be one setting, because there was one chip: the RetroWave
//! board is an OPL3, so choosing it *was* choosing how OPL played. Then it
//! became one row per chip. Now each row is a **core picker** -- because a chip
//! can have more than one emulator, they do not sound alike, and which one is
//! running is the user's call.
//!
//! Every row comes from [`dro_synth::registry`]: a core that is not registered
//! cannot be chosen. A build that never registered a provider -- the web build
//! has no serial ports, so no RetroWave -- simply has no such entry, and the
//! dialog stops offering something it cannot deliver rather than hiding the
//! gap. Licences and authors are registry facts too, but they belong to the
//! About box's credits; the picker shows only what a choice sounds like.
//!
//! Chips are grouped by what they share a core with: the OPL family is one row,
//! because one core (or one board) plays all four.
//!
//! [`plan`] shapes the roster for the Settings dialog rather than handing it
//! over flat: the loaded song's own chips come first (so tuning what you are
//! hearing is two rows, not twenty), the chips that offer a real choice come
//! next, and the single-core chips -- the ones there is nothing to decide about
//! -- fold into a one-line summary per core instead of a dozen identical rows.
//! Chips with no core at all are simply left out; a silent chip is not a
//! setting.

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
    /// SPDX expression. Not displayed here -- the About box carries the
    /// credits -- but still required of every row, so a core cannot be
    /// registered without its terms on record.
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

/// How many of the spec's chips have no core yet.
///
/// No longer shown -- a silent chip is not a setting -- but kept because it is
/// how [`every_chip_is_either_a_row_or_counted`](tests) proves the roster
/// covers the whole chip table with nothing quietly dropped.
#[must_use]
pub fn without_cores() -> usize {
    let registry = registry::registry();
    ChipKind::all()
        .filter(|&chip| !registry::is_opl(chip) && !registry.has_core(chip))
        .count()
}

/// A group of single-core chips that share one core: the fold's one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedCore {
    /// The core every chip in the group plays through, e.g. `"libvgm (Valley Bell)"`.
    pub label: String,
    /// The chips it plays, by their display names, in roster order.
    pub chips: Vec<String>,
}

/// The Settings roster, shaped for display: the loaded song's chips first, the
/// remaining choices next, and the single-core chips folded away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputPlan {
    /// The loaded song's own chips, in file order, deduped by slot. Rendered
    /// first (choosers where they have alternatives, facts where they don't) so
    /// the file you are listening to is what you tune. Empty when nothing is
    /// loaded.
    pub song: Vec<ChipOutputRow>,
    /// Chips with a real choice to make -- more than one core -- that are not
    /// already shown for the song.
    pub choosers: Vec<ChipOutputRow>,
    /// Single-core chips not shown for the song, folded to one line per core:
    /// there is nothing to decide, so they are stated, not listed.
    pub folded: Vec<FoldedCore>,
}

/// Shapes the roster for the Settings dialog around `song_chips` (the loaded
/// file's chips, in file order; empty when nothing is loaded).
///
/// A chip appears exactly once: in `song` if the file uses it, otherwise in
/// `choosers` if it offers a choice or in `folded` if it does not. Pure, so the
/// partitioning is tested without a UI.
#[must_use]
pub fn plan(song_chips: &[ChipKind]) -> OutputPlan {
    let all = rows();

    // The song's chips, hoisted in file order, one row per slot (the OPL family
    // and a dual chip both collapse to a single row).
    let mut song = Vec::new();
    let mut song_slots: Vec<&'static str> = Vec::new();
    for &chip in song_chips {
        let slot = registry::slot_slug(chip);
        if song_slots.contains(&slot) {
            continue;
        }
        if let Some(row) = all.iter().find(|row| row.slot == slot) {
            song.push(row.clone());
            song_slots.push(slot);
        }
    }

    // Everything the song did not already claim: choices kept as rows,
    // single-core chips folded under the core that plays them.
    let mut choosers = Vec::new();
    let mut folded: Vec<FoldedCore> = Vec::new();
    for row in &all {
        if song_slots.contains(&row.slot) {
            continue;
        }
        if row.is_choice() {
            choosers.push(row.clone());
        } else {
            let label = row.cores[0].label.clone();
            match folded.iter_mut().find(|group| group.label == label) {
                Some(group) => group.chips.push(row.label.to_owned()),
                None => folded.push(FoldedCore {
                    label,
                    chips: vec![row.label.to_owned()],
                }),
            }
        }
    }

    OutputPlan {
        song,
        choosers,
        folded,
    }
}

/// Draws one roster row into an open two-column grid: the chip's name, then
/// either a core chooser (when it has alternatives) or the muted name of the one
/// core that plays it. Editing the chooser writes the choice into `cores`.
pub fn chip_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    cores: &mut std::collections::BTreeMap<String, String>,
    row: &ChipOutputRow,
) {
    ui.label(row.label);
    let selected = selected_name(cores, row);
    if row.is_choice() {
        ui.scope(|ui| {
            crate::theme::style_dropdown(ui, palette);
            let mut choice = selected.clone();
            egui::ComboBox::from_id_salt(format!("settings-core-{}", row.slot))
                .selected_text(label_for(row, &choice))
                .show_ui(ui, |ui| {
                    for core in &row.cores {
                        ui.selectable_value(&mut choice, core.name.clone(), &core.label);
                    }
                });
            if choice != selected {
                cores.insert(row.slot.to_owned(), choice);
            }
        });
    } else {
        ui.colored_label(palette.muted, &row.cores[0].label);
    }
    ui.end_row();
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
/// OPL row would state one core rather than offer the three the app has -- and
/// the dialog snapshot would stop documenting the picker, which is the part
/// most worth catching a regression in. The provider entries are declared here
/// rather than depended on: `dro-retrowave` is native-only and
/// `dro-cores-nuked` needs a C toolchain, while `dro-ui` compiles to wasm, so
/// this crate must not link either. The declarations agreeing with the app's is
/// what `the_test_registry_matches_the_apps` in `dro-trimmer` checks.
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
                id: "opl3.cqm",
                chip,
                label: "Nuked-CQM (Creative CQM)",
                authors: "Nuke.YKT",
                license: "LGPL-2.1-or-later",
                upstream: "https://github.com/nukeykt/Nuked-CQM",
                realtime: true,
                level: dro_synth::LEVEL_UNITY,
                // The real one builds a chip; a stand-in only has to be
                // listable, and this crate cannot link the C provider.
                make: dro_synth::CoreMaker::Routed,
            });
            registry.register(CoreInfo {
                id: "opl3.retrowave",
                chip,
                label: "RetroWave OPL3 (hardware)",
                authors: "SudoMaker (the board); this project (the protocol)",
                license: "GPL-2.0-or-later",
                upstream: "https://github.com/SudoMaker/RetroWave",
                realtime: true,
                level: dro_synth::LEVEL_UNITY,
                make: dro_synth::CoreMaker::Routed,
            });
        }
        // A generic chip the GUI tests can play: the app's SN76489 comes from
        // `dro-cores-libvgm` (a C build this wasm-compatible crate must not
        // link), so the stand-in builds a small tone stub under the same id.
        // It has to *sound*, not merely build: the render test measures the
        // WAV's peak, so it obeys the SN76489's volume latches and squares a
        // fixed pitch while any channel is unmuted.
        registry.register(CoreInfo {
            id: "sn76489.libvgm",
            chip: ChipKind::Sn76489,
            label: "libvgm (Valley Bell)",
            authors: "Valley Bell and the upstream core authors",
            license: "see PROVENANCE.md -- upstream publishes no grant",
            upstream: "https://github.com/ValleyBell/libvgm",
            realtime: true,
            level: dro_synth::LEVEL_UNITY,
            make: dro_synth::CoreMaker::Generic(|| Box::new(ToneStub::new())),
        });
        // Already installed means another test got here first with the same
        // content, which is the point of the `Once`.
        let _ = dro_synth::install(registry);
    });
}

/// A square wave that obeys the SN76489's volume latches and nothing else --
/// enough for a test file's `0x90` to make sound and its `0x9F` to stop it.
/// Deterministic and chunk-independent, so renders and waveforms agree.
#[cfg(test)]
#[derive(Debug)]
struct ToneStub {
    /// Per-channel attenuation, 0xF = silent, as the SN76489 has it.
    volumes: [u8; 4],
    /// Absolute frame counter, so chunked renders line up.
    at: u64,
}

#[cfg(test)]
impl ToneStub {
    fn new() -> Self {
        Self {
            volumes: [0xF; 4],
            at: 0,
        }
    }
}

#[cfg(test)]
impl dro_synth::ChipCore for ToneStub {
    fn reset(&mut self, _clock: u32, _variant: bool) {
        self.volumes = [0xF; 4];
        self.at = 0;
    }

    fn native_rate(&self) -> u32 {
        44_100
    }

    fn write(&mut self, _port: u8, addr: u16, data: u16) {
        // Address 1 is the Game Gear stereo mask; only the command byte at
        // address 0 carries latches.
        if addr != 0 {
            return;
        }
        let byte = data as u8;
        // A latch byte selecting a volume register: `1cc1vvvv`.
        if byte & 0x90 == 0x90 {
            self.volumes[usize::from((byte >> 5) & 3)] = byte & 0x0F;
        }
    }

    fn render(&mut self, out: &mut [i32]) {
        let sounding = self.volumes.iter().any(|&volume| volume < 0xF);
        for frame in out.chunks_exact_mut(2) {
            let sample = if sounding {
                // ~441 Hz square at 44.1 kHz: flip every 50 frames.
                if (self.at / 50).is_multiple_of(2) {
                    8_000
                } else {
                    -8_000
                }
            } else {
                0
            };
            frame[0] = sample;
            frame[1] = sample;
            self.at += 1;
        }
    }
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
            !rows.iter().any(|row| row.chip == ChipKind::Scsp),
            "the SCSP has no core yet, so it is counted rather than listed"
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
    /// without a licence on record -- shown in the About credits, not here,
    /// but required all the same.
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

    /// The song's chips are hoisted in file order, single-core chips fold, and
    /// nothing appears twice.
    #[test]
    fn plan_hoists_the_song_and_folds_the_rest() {
        install_test_cores();
        // The stand-in registry gives the OPL family a choice (emulator, CQM,
        // hardware) and the SN76489 a single core -- so an OPL song hoists a
        // chooser above the roster, and the single-core SN76489 folds below.
        let plan = plan(&[ChipKind::Ymf262]);

        assert_eq!(plan.song.len(), 1, "the one song chip is hoisted");
        assert_eq!(plan.song[0].slot, "opl3");
        assert!(plan.song[0].is_choice(), "and it kept its dropdown");

        // The hoisted chip is not repeated among the choosers or the fold.
        assert!(
            !plan.choosers.iter().any(|row| row.slot == "opl3"),
            "a hoisted chooser is not shown a second time below"
        );

        // Single-core chips are stated once, grouped under the core that plays
        // them -- the SN76489 through its libvgm stub here.
        assert!(
            plan.folded
                .iter()
                .any(|group| group.chips.iter().any(|name| name == "SN76489")),
            "the single-core SN76489 folds into a summary"
        );
        for group in &plan.folded {
            assert!(!group.label.is_empty());
            assert!(!group.chips.is_empty());
        }
    }

    /// With nothing loaded there is no song section, and the whole roster shows
    /// as choosers plus the fold.
    #[test]
    fn plan_without_a_song_has_no_hoisted_rows() {
        install_test_cores();
        let plan = plan(&[]);
        assert!(plan.song.is_empty());
        assert!(
            plan.choosers.iter().any(|row| row.slot == "opl3"),
            "the OPL chooser is in the main list instead"
        );
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
