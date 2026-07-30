//! Which core plays each chip: one row per chip this app can play, each a **core
//! picker** -- a chip can have more than one emulator, they do not sound alike,
//! and which one runs is the user's call.
//!
//! Every row comes from [`vgms_synth::registry`]: a core that is not registered
//! cannot be chosen. A build that never registered a provider -- the web build
//! has no serial ports, so no RetroWave -- simply has no such entry, so the
//! dialog stops offering something it cannot deliver. Licences and authors are
//! registry facts too, but they belong to the About box's credits; the picker
//! shows only what a choice sounds like.
//!
//! Chips are grouped by what they share a core with: the OPL family is one row,
//! because one core (or one board) plays all four.
//!
//! [`plan`] shapes the roster for the Settings dialog: the loaded song's own
//! chips come first, the chips that offer a real choice come next, and the
//! single-core chips fold into a one-line summary per core. Chips with no core at
//! all are left out; a silent chip is not a setting.

use vgms_core::vgm::ChipKind;
use vgms_synth::registry::{self, CoreInfo};

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

/// How many of the spec's chips have no core yet. Not shown, but used by
/// [`every_chip_is_either_a_row_or_counted`](tests) to prove the roster covers
/// the whole chip table with nothing quietly dropped.
#[must_use]
pub fn without_cores() -> usize {
    let registry = registry::registry();
    ChipKind::all()
        .filter(|&chip| !registry::is_opl(chip) && !registry.has_core(chip))
        .count()
}

/// One chip the loaded song uses, as the "Current" section shows it: the chip,
/// and its roster row when this build has a core for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongChipRow {
    /// The chip the file clocks. Named directly (rather than via `row`) so a
    /// chip with no core is still shown -- the file uses it, whether or not this
    /// app can play it.
    pub kind: ChipKind,
    /// The roster row, when a core exists: a chooser if the chip has more than
    /// one, a fact if it has exactly one. `None` when this build plays it silent.
    pub row: Option<ChipOutputRow>,
}

/// The Settings roster, split into the two sections the Output tab shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputPlan {
    /// The loaded song's own chips, in file order, deduped by slot -- the
    /// "Current" section. Every chip the file uses is here, whether it offers a
    /// core choice, a single fixed core, or none at all. Empty when nothing is
    /// loaded.
    pub song: Vec<SongChipRow>,
    /// Every other chip that has a core -- the "All chips" section -- so a core
    /// can be configured for any chip, not just the ones the current song uses.
    /// Choosers and single-core facts alike; a chip the song already lists is
    /// not repeated here.
    pub all: Vec<ChipOutputRow>,
}

/// Shapes the roster for the Settings dialog around `song_chips` (the loaded
/// file's chips, in file order; empty when nothing is loaded).
///
/// A chip appears exactly once: in `song` if the file uses it (with or without a
/// core), otherwise in `all`. Pure, so the split is tested without a UI.
#[must_use]
pub fn plan(song_chips: &[ChipKind]) -> OutputPlan {
    let rows = rows();

    // The song's chips, in file order, one entry per slot (the OPL family and a
    // dual chip both collapse to a single row). A chip with no core still gets
    // an entry -- the file uses it regardless.
    let mut song = Vec::new();
    let mut song_slots: Vec<&'static str> = Vec::new();
    for &chip in song_chips {
        let slot = registry::slot_slug(chip);
        if song_slots.contains(&slot) {
            continue;
        }
        song_slots.push(slot);
        song.push(SongChipRow {
            kind: chip,
            row: rows.iter().find(|row| row.slot == slot).cloned(),
        });
    }

    // Every other chip with a core, kept as a row so all of them stay
    // configurable -- the song's are pulled up into `song`, not hidden.
    let all = rows
        .into_iter()
        .filter(|row| !song_slots.contains(&row.slot))
        .collect();

    OutputPlan { song, all }
}

/// Draws one "Current" section entry: the chip's core row when it has one, or
/// the chip named with a muted "no core yet" when this build plays it silent.
pub fn song_chip_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    cores: &mut std::collections::BTreeMap<String, String>,
    entry: &SongChipRow,
) {
    match &entry.row {
        Some(row) => chip_row(ui, palette, cores, row),
        None => {
            ui.label(entry.kind.name());
            ui.colored_label(palette.muted, crate::strings::CHIP_OUTPUT_NO_CORE);
            ui.end_row();
        }
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
/// `vgms-ui` alone knows only `vgms-synth`'s built-in cores, so without this its
/// OPL row would state one core rather than offer the three the app has. The
/// provider entries are declared here rather than depended on: `vgms-retrowave`
/// is native-only and `vgms-cores-nuked` needs a C toolchain, while `vgms-ui`
/// compiles to wasm, so this crate must not link either. That the declarations
/// agree with the app's is checked by `the_test_registry_matches_the_apps` in
/// `vgms-app`.
///
/// Idempotent and safe to call from any test in any order: every caller installs
/// the same content, and the first one wins.
#[cfg(test)]
pub(crate) fn install_test_cores() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut registry = vgms_synth::CoreRegistry::with_builtins();
        for chip in registry::OPL_CHIPS {
            registry.register(CoreInfo {
                id: "opl3.cqm",
                chip,
                label: "Nuked-CQM (Creative CQM)",
                authors: "Nuke.YKT",
                license: "LGPL-2.1-or-later",
                upstream: "https://github.com/nukeykt/Nuked-CQM",
                realtime: true,
                channel_pan: false,
                level: vgms_synth::LEVEL_UNITY,
                // The real one builds a chip; a stand-in only has to be
                // listable, and this crate cannot link the C provider.
                make: vgms_synth::CoreMaker::Routed,
            });
            registry.register(CoreInfo {
                id: "opl3.retrowave",
                chip,
                label: "RetroWave OPL3 (hardware)",
                authors: "SudoMaker (the board); this project (the protocol)",
                license: "GPL-2.0-or-later",
                upstream: "https://github.com/SudoMaker/RetroWave",
                realtime: true,
                channel_pan: false,
                level: vgms_synth::LEVEL_UNITY,
                make: vgms_synth::CoreMaker::Routed,
            });
        }
        // A generic chip the GUI tests can play: the app's SN76489 comes from
        // `vgms-cores-libvgm` (a C build this wasm-compatible crate must not
        // link), so the stand-in builds a small tone stub under the same id.
        // It has to *sound*, not merely build: the render test measures the
        // WAV's peak, so it obeys the SN76489's volume latches and squares a
        // fixed pitch while any channel is unmuted.
        registry.register(CoreInfo {
            id: "sn76489.libvgm",
            chip: ChipKind::Sn76489,
            label: "libvgm",
            authors: "the libvgm project and upstream core authors",
            license: "see PROVENANCE.md -- upstream publishes no grant",
            upstream: "https://github.com/ValleyBell/libvgm",
            realtime: true,
            channel_pan: false,
            level: vgms_synth::LEVEL_UNITY,
            make: vgms_synth::CoreMaker::Generic(|| Box::new(ToneStub::new())),
        });
        let _ = vgms_synth::install(registry);
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
    /// The channel mute mask, so the channel splitter sees silence for the
    /// channels it mutes when soloing one.
    muted: u32,
    /// Absolute frame counter, so chunked renders line up.
    at: u64,
}

#[cfg(test)]
impl ToneStub {
    fn new() -> Self {
        Self {
            volumes: [0xF; 4],
            muted: 0,
            at: 0,
        }
    }
}

#[cfg(test)]
impl vgms_synth::ChipCore for ToneStub {
    fn reset(&mut self, _clock: u32, _variant: bool) {
        self.volumes = [0xF; 4];
        self.muted = 0;
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

    fn set_channel_mutes(&mut self, muted: u32) {
        self.muted = muted;
    }

    fn render(&mut self, out: &mut [i32]) {
        // A channel sounds only if it is un-attenuated and not muted.
        let sounding = self
            .volumes
            .iter()
            .enumerate()
            .any(|(channel, &volume)| volume < 0xF && (self.muted >> channel) & 1 == 0);
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
        assert_eq!(rows[0].slot, vgms_core::config::OPL_SLOT);
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
    fn plan_splits_the_song_from_the_rest() {
        install_test_cores();
        // The stand-in registry gives the OPL family a choice (emulator, CQM,
        // hardware) and the SN76489 a single core.
        let plan = plan(&[ChipKind::Ymf262]);

        // The song's OPL chip is in "Current", carrying its chooser.
        assert_eq!(plan.song.len(), 1, "the one song chip is in Current");
        assert_eq!(plan.song[0].kind, ChipKind::Ymf262);
        let row = plan.song[0].row.as_ref().expect("OPL has cores");
        assert_eq!(row.slot, "opl3");
        assert!(row.is_choice(), "and it kept its dropdown");

        // "All chips" holds every *other* chip, still configurable, and does not
        // repeat the one already in Current.
        assert!(
            !plan.all.iter().any(|row| row.slot == "opl3"),
            "the current chip is not repeated in All"
        );
        assert!(
            plan.all.iter().any(|row| row.slot == "sn76489"),
            "the SN76489 stays configurable under All chips"
        );
    }

    /// A song chip this build has no core for is still listed in Current -- the
    /// file uses it whether or not we can play it.
    #[test]
    fn plan_lists_a_song_chip_with_no_core() {
        install_test_cores();
        // The SCSP has no core in the stand-in registry.
        let plan = plan(&[ChipKind::Scsp]);
        assert_eq!(plan.song.len(), 1);
        assert_eq!(plan.song[0].kind, ChipKind::Scsp);
        assert!(
            plan.song[0].row.is_none(),
            "no core, but still a Current entry"
        );
    }

    /// With nothing loaded there is no Current section, and the whole roster is
    /// under "All chips".
    #[test]
    fn plan_without_a_song_has_no_current_section() {
        install_test_cores();
        let plan = plan(&[]);
        assert!(plan.song.is_empty());
        assert!(
            plan.all.iter().any(|row| row.slot == "opl3"),
            "every chip is under All chips instead"
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
