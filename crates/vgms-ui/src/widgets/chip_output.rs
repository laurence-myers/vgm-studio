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
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CoreChoice {
    /// What `vgmstudio.ini` stores: the core id without its slot prefix.
    pub(crate) name: String,
    /// What the dropdown shows.
    pub(crate) label: String,
    /// SPDX expression. Not displayed here -- the About box carries the
    /// credits -- but still required of every row, so a core cannot be
    /// registered without its terms on record.
    pub(crate) license: String,
    /// The provenance tier, shown as the row's badge and half of the
    /// fidelity auto-select's sort key.
    pub(crate) tier: vgms_synth::CoreTier,
    /// Whether this core emulates the very chip of its row (the other half
    /// of the fidelity sort -- an exact model beats a neighbouring die).
    pub(crate) exact: bool,
    /// A routed (hardware) entry: listed and choosable, but never what the
    /// fidelity auto-select writes -- picking hardware is a backend decision.
    pub(crate) routed: bool,
    /// Estimated speed on this machine, ×realtime -- the baseline scaled by
    /// the measured machine ratio. `None` for a core the speed table does
    /// not track. Kept for the fidelity gate and the tooltip; the picker shows
    /// [`speed_tier`](Self::speed_tier) instead of the number.
    pub(crate) speed: Option<f32>,
    /// The coarse speed band the picker shows (Offline / Slow / Fast). An
    /// untracked realtime core reads Fast; an untracked non-realtime core
    /// (there are none today) would read Offline.
    pub(crate) speed_tier: vgms_synth::speed::SpeedTier,
}

/// One row: a chip (or a family of them), and the cores it can play through.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChipOutputRow {
    /// The config slot, e.g. `"opl3"` -- what `core.<slot>=` names.
    pub(crate) slot: &'static str,
    /// The row title: the representative chip's part name, e.g. `"YMF262"`.
    pub(crate) label: &'static str,
    /// A representative chip, for looking the row back up in the registry.
    pub(crate) chip: ChipKind,
    /// The cores available, best first. Never empty -- a chip with none is
    /// counted by [`without_cores`] instead of listed.
    pub(crate) cores: Vec<CoreChoice>,
}

impl ChipOutputRow {
    /// Whether this row offers a choice at all, rather than stating a fact.
    #[must_use]
    pub(crate) fn is_choice(&self) -> bool {
        self.cores.len() > 1
    }
}

/// The chip whose registry entries stand for the whole OPL family (and, when
/// split, for the OPL3 half). Its part name titles the row -- "YMF262" -- the
/// same as every other row uses its own chip's [`name`](ChipKind::name); an
/// OPL3 *is* an OPL2 with more of it, and the Y8950/YM3526 are the same
/// register file again, so one core (or one board) plays all four.
const OPL_REPRESENTATIVE: ChipKind = ChipKind::Ymf262;

/// The chip whose registry entries stand for the split OPL2 row -- titled
/// "YM3812", and governing the YM3526 and Y8950 with it.
const OPL2_REPRESENTATIVE: ChipKind = ChipKind::Ym3812;

/// Whether `cores` splits the OPL selector: the optional `opl2` key *is* the
/// split state, so the UI and the resolver cannot disagree about it.
#[must_use]
pub(crate) fn opl_split(cores: &std::collections::BTreeMap<String, String>) -> bool {
    cores.contains_key(vgms_core::config::OPL2_SLOT)
}

/// Every row the dialog shows, in the order it shows them.
///
/// The OPL row(s) first because they are the ones most likely to have a
/// choice -- one combined row, or the OPL2/OPL3 pair when `split` -- then
/// every other chip with a core, in header order.
#[must_use]
pub(crate) fn rows(split: bool) -> Vec<ChipOutputRow> {
    let mut rows = Vec::new();
    if split {
        // The OPL2 row omits routed (hardware) entries: the board is a whole
        // OPL3 and the backend switch keys off the family slot, so offering
        // it here would set a key nothing routes on -- silence, not hardware.
        if let Some(row) = row_for(OPL2_REPRESENTATIVE, vgms_core::config::OPL2_SLOT, false) {
            rows.push(row);
        }
        if let Some(row) = row_for(OPL_REPRESENTATIVE, vgms_core::config::OPL_SLOT, true) {
            rows.push(row);
        }
    } else if let Some(row) = row_for(OPL_REPRESENTATIVE, vgms_core::config::OPL_SLOT, true) {
        rows.push(row);
    }
    for chip in ChipKind::all() {
        // The OPL family already leads the list; the other OPL chips would
        // list it again under their own names.
        if registry::is_opl(chip) {
            continue;
        }
        if let Some(row) = row_for(chip, registry::slot_slug(chip), true) {
            rows.push(row);
        }
    }
    rows
}

/// A row for `chip` editing `slot`, or `None` when nothing plays it. The row
/// is titled by the chip's own [`name`](ChipKind::name), so every row -- OPL
/// included -- reads as its part number.
fn row_for(chip: ChipKind, slot: &'static str, include_routed: bool) -> Option<ChipOutputRow> {
    let cores: Vec<CoreChoice> = registry::registry()
        .for_chip(chip)
        .filter(|info| include_routed || !matches!(info.make, vgms_synth::CoreMaker::Routed))
        .map(choice)
        .collect();
    (!cores.is_empty()).then_some(ChipOutputRow {
        slot,
        label: chip.name(),
        chip,
        cores,
    })
}

/// The roster slot `chip`'s row edits, given the split state: the OPL2
/// generation moves to the `opl2` row when split, and everything else keeps
/// [`registry::slot_slug`].
fn display_slot(chip: ChipKind, split: bool) -> &'static str {
    if split && registry::is_opl2_generation(chip) {
        vgms_core::config::OPL2_SLOT
    } else {
        registry::slot_slug(chip)
    }
}

/// A registry entry as the dropdown needs it: the id stripped of the slot it is
/// already filed under, since the config stores `core.opl3=nuked`, not
/// `core.opl3=opl3.nuked`.
fn choice(info: &CoreInfo) -> CoreChoice {
    let prefix = format!("{}.", registry::slot_slug(info.chip));
    let speed = vgms_synth::speed::effective_speed(info.id);
    // A tracked core reads its measured band; an untracked one falls back to
    // the registry's realtime promise -- the fast behavioural cores (libvgm)
    // are untracked, so this is what gives them a "Fast" readout.
    let speed_tier = match speed {
        Some(speed) => vgms_synth::speed::SpeedTier::of(speed),
        None if info.realtime => vgms_synth::speed::SpeedTier::Fast,
        None => vgms_synth::speed::SpeedTier::Offline,
    };
    CoreChoice {
        name: info.id.strip_prefix(&prefix).unwrap_or(info.id).to_owned(),
        label: info.label.to_owned(),
        license: info.license.to_owned(),
        tier: info.tier,
        exact: info.exact,
        routed: matches!(info.make, vgms_synth::CoreMaker::Routed),
        speed,
        speed_tier,
    }
}

/// The fixed dropdown width, so the core column is the same width on every
/// row and the badge and speed columns line up beside it.
const COMBO_WIDTH: f32 = 188.0;

/// The badge colour for an accuracy tier -- a fixed accent per rung, tinted to
/// read on either a light or dark dialog face.
fn tier_color(tier: vgms_synth::CoreTier) -> egui::Color32 {
    use vgms_synth::CoreTier::*;
    match tier {
        Behavioural => egui::Color32::from_rgb(0x7B, 0xC8, 0xE0), // cyan
        Cycle => egui::Color32::from_rgb(0x7B, 0xD0, 0x8C),       // green
        DieSim => egui::Color32::from_rgb(0xC0, 0x9B, 0xF0),      // violet
        Hardware => egui::Color32::from_rgb(0xE8, 0xB0, 0x5C),    // amber
    }
}

/// The readout colour for a speed band, from the meter palette so it tracks
/// the theme: red below realtime, amber for slow, green for fast.
fn speed_color(tier: vgms_synth::speed::SpeedTier, palette: &Palette) -> egui::Color32 {
    use vgms_synth::speed::SpeedTier::*;
    match tier {
        Offline => palette.meter_high,
        Slow => palette.meter_mid,
        Fast => palette.meter_low,
    }
}

/// The chip-name cell: the part name, with the family nickname (OPL3, OPN2,
/// SSG...) in small type underneath when the chip has one.
///
/// Both labels extend rather than wrap: a grid cell's first sizing pass can be
/// narrow, and a wrapping part name breaks "YMF262" across two lines.
fn chip_name_cell(ui: &mut egui::Ui, palette: &Palette, chip: ChipKind) {
    ui.vertical(|ui| {
        ui.add(egui::Label::new(chip.name()).wrap_mode(egui::TextWrapMode::Extend));
        if let Some(nickname) = chip.nickname() {
            ui.add(
                egui::Label::new(egui::RichText::new(nickname).small().color(palette.muted))
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        }
    });
}

/// The accuracy badge cell: a colour-coded box, right-aligned, with the tier's
/// one-line description on hover (plus a note when the core is a stand-in for a
/// related chip rather than the exact one).
fn tier_badge_cell(ui: &mut egui::Ui, palette: &Palette, core: &CoreChoice) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let color = tier_color(core.tier);
        let hover = if core.exact {
            core.tier.description().to_owned()
        } else {
            format!(
                "{} This core emulates a close relative of the chip, not the exact part.",
                core.tier.description()
            )
        };
        egui::Frame::new()
            .fill(color.gamma_multiply(0.18))
            .stroke(egui::Stroke::new(1.0, palette.bevel_border))
            .inner_margin(egui::Margin::symmetric(4, 1))
            .corner_radius(2.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(core.tier.badge()).small().color(color));
            })
            .response
            .on_hover_text(hover);
    });
}

/// The speed cell: the band word (Offline / Slow / Fast), right-aligned and
/// colour-coded, with the band's meaning and the exact estimate on hover.
fn speed_cell(ui: &mut egui::Ui, palette: &Palette, core: &CoreChoice) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let hover = match core.speed {
            Some(speed) => format!(
                "{} (about {speed:.1}\u{d7} realtime here)",
                core.speed_tier.description()
            ),
            None => core.speed_tier.description().to_owned(),
        };
        ui.label(
            egui::RichText::new(core.speed_tier.label())
                .small()
                .color(speed_color(core.speed_tier, palette)),
        )
        .on_hover_text(hover);
    });
}

/// The headroom the fidelity auto-select demands over bare realtime: an
/// estimate at 1.0× leaves the audio callback nothing for the rest of the
/// app, so "holds realtime" means half again over the line.
const REALTIME_HEADROOM: f32 = 1.5;

/// The highest-fidelity core in `row` that holds realtime: sorted by exact
/// first, then tier, registry order breaking ties; routed (hardware) rows
/// never picked; a tracked speed must clear [`REALTIME_HEADROOM`], an
/// untracked one always does.
fn fidelity_pick(row: &ChipOutputRow) -> Option<&CoreChoice> {
    let mut best: Option<&CoreChoice> = None;
    for core in &row.cores {
        if core.routed || core.speed.is_some_and(|speed| speed < REALTIME_HEADROOM) {
            continue;
        }
        let beats = best.is_none_or(|held| (core.exact, core.tier) > (held.exact, held.tier));
        if beats {
            best = Some(core);
        }
    }
    best
}

/// Sets every choosable slot to its [`fidelity_pick`]: the auto-select the
/// Settings button runs. A slot whose row offers no realtime-holding core is
/// left alone rather than blanked.
pub(crate) fn auto_select(cores: &mut std::collections::BTreeMap<String, String>) {
    let split = opl_split(cores);
    for row in rows(split) {
        if !row.is_choice() {
            continue;
        }
        if let Some(best) = fidelity_pick(&row) {
            cores.insert(row.slot.to_owned(), best.name.clone());
        }
    }
}

/// How many of the spec's chips have no core yet. Not shown, but used by
/// [`every_chip_is_either_a_row_or_counted`](tests) to prove the roster covers
/// the whole chip table with nothing quietly dropped.
// Test-only: the roster UI never counts these, only the coverage test does.
#[cfg_attr(not(test), allow(dead_code))]
#[must_use]
pub(crate) fn without_cores() -> usize {
    let registry = registry::registry();
    ChipKind::all()
        .filter(|&chip| !registry::is_opl(chip) && !registry.has_core(chip))
        .count()
}

/// One chip the loaded song uses, as the "Current" section shows it: the chip,
/// and its roster row when this build has a core for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SongChipRow {
    /// The chip the file clocks. Named directly (rather than via `row`) so a
    /// chip with no core is still shown -- the file uses it, whether or not this
    /// app can play it.
    pub(crate) kind: ChipKind,
    /// The roster row, when a core exists: a chooser if the chip has more than
    /// one, a fact if it has exactly one. `None` when this build plays it silent.
    pub(crate) row: Option<ChipOutputRow>,
}

/// The Settings roster, split into the two sections the Output tab shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct OutputPlan {
    /// The loaded song's own chips, in file order, deduped by slot -- the
    /// "Current" section. Every chip the file uses is here, whether it offers a
    /// core choice, a single fixed core, or none at all. Empty when nothing is
    /// loaded.
    pub(crate) song: Vec<SongChipRow>,
    /// Every other chip that has a core -- the "All chips" section -- so a core
    /// can be configured for any chip, not just the ones the current song uses.
    /// Choosers and single-core facts alike; a chip the song already lists is
    /// not repeated here.
    pub(crate) all: Vec<ChipOutputRow>,
}

/// Shapes the roster for the Settings dialog around `song_chips` (the loaded
/// file's chips, in file order; empty when nothing is loaded) and the map's
/// OPL split state (pass [`opl_split`] of the map the rows will edit).
///
/// A chip appears exactly once: in `song` if the file uses it (with or without a
/// core), otherwise in `all`. Pure, so the split is tested without a UI.
#[must_use]
pub(crate) fn plan(song_chips: &[ChipKind], split: bool) -> OutputPlan {
    let rows = rows(split);

    // The song's chips, in file order, one entry per slot (the OPL family and a
    // dual chip both collapse to a single row). A chip with no core still gets
    // an entry -- the file uses it regardless.
    let mut song = Vec::new();
    let mut song_slots: Vec<&'static str> = Vec::new();
    for &chip in song_chips {
        let slot = display_slot(chip, split);
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

/// Splits the OPL selector in `cores`: the `opl2` slot appears, seeded with
/// the combined choice when the OPL2 row offers it (else that row's default),
/// so the moment of splitting changes what is *editable*, not what is heard.
///
/// A no-op when this build has no cores for the OPL2 generation at all.
pub(crate) fn split_opl(cores: &mut std::collections::BTreeMap<String, String>) {
    let Some(row) = row_for(OPL2_REPRESENTATIVE, vgms_core::config::OPL2_SLOT, false) else {
        return;
    };
    let seed = cores
        .get(vgms_core::config::OPL_SLOT)
        .filter(|name| row.cores.iter().any(|core| &core.name == *name))
        .cloned()
        .unwrap_or_else(|| row.cores[0].name.clone());
    cores.insert(vgms_core::config::OPL2_SLOT.to_owned(), seed);
}

/// Merges the OPL selector back to one core for the whole family: removing
/// the `opl2` slot is all a merge is, and the family slot keeps its choice.
pub(crate) fn merge_opl(cores: &mut std::collections::BTreeMap<String, String>) {
    cores.remove(vgms_core::config::OPL2_SLOT);
}

/// Draws one "Current" section entry across the four-column grid: the chip's
/// core row when it has one, or the chip named with a muted "no core yet" when
/// this build plays it silent.
///
/// `salt_prefix` disambiguates this row's `ComboBox` from the same slot's row in
/// another dialog open at the same time (Settings vs. a render dialog); pass a
/// stable per-dialog string like `"settings"` or `"render"`.
pub(crate) fn song_chip_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    salt_prefix: &str,
    cores: &mut std::collections::BTreeMap<String, String>,
    entry: &SongChipRow,
) {
    match &entry.row {
        Some(row) => chip_row(ui, palette, salt_prefix, cores, row),
        None => {
            chip_name_cell(ui, palette, entry.kind);
            ui.colored_label(palette.muted, crate::strings::CHIP_OUTPUT_NO_CORE);
            ui.end_row();
        }
    }
}

/// Draws one roster row across four grid columns: the chip name (with its
/// nickname), a fixed-width core chooser, the chosen core's accuracy badge, and
/// its speed band. Editing the chooser writes the choice into `cores`.
///
/// The fixed dropdown width and the four cells make every row a real table:
/// the badges and speeds line up down the page rather than floating after
/// labels of different lengths.
///
/// `salt_prefix` scopes the `ComboBox` id so two dialogs showing the same slot
/// (Settings and a render/split dialog) do not collide -- pass `"settings"`,
/// `"render"`, or `"split"`.
pub(crate) fn chip_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    salt_prefix: &str,
    cores: &mut std::collections::BTreeMap<String, String>,
    row: &ChipOutputRow,
) {
    // Column 1: the chip name and nickname.
    chip_name_cell(ui, palette, row.chip);
    let selected = selected_name(cores, row);
    let selected_core = row.cores.iter().find(|core| core.name == selected);
    // Column 2: the core chooser, or the single core's name as a fact.
    if row.is_choice() {
        ui.scope(|ui| {
            crate::theme::style_dropdown(ui, palette);
            let mut choice = selected.clone();
            egui::ComboBox::from_id_salt(format!("{salt_prefix}-core-{}", row.slot))
                .width(COMBO_WIDTH)
                .selected_text(label_for(row, &choice))
                .show_ui(ui, |ui| {
                    for core in &row.cores {
                        ui.selectable_value(
                            &mut choice,
                            core.name.clone(),
                            option_text(ui, palette, core),
                        );
                    }
                });
            if choice != selected {
                cores.insert(row.slot.to_owned(), choice);
            }
        });
    } else {
        ui.colored_label(palette.muted, &row.cores[0].label);
    }
    // Columns 3 and 4: the accuracy badge and the speed band of the core that
    // will actually play.
    match selected_core {
        Some(core) => {
            tier_badge_cell(ui, palette, core);
            speed_cell(ui, palette, core);
        }
        None => {
            ui.label("");
            ui.label("");
        }
    }
    ui.end_row();
}

/// One open-list row's text: the label, then the tier badge and the speed band
/// in small type, so the popup carries the same trade-off the cells show.
fn option_text(ui: &egui::Ui, palette: &Palette, core: &CoreChoice) -> egui::text::LayoutJob {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &core.label,
        0.0,
        egui::TextFormat::simple(body, ui.visuals().text_color()),
    );
    job.append(
        core.tier.badge(),
        10.0,
        egui::TextFormat::simple(small.clone(), tier_color(core.tier)),
    );
    job.append(
        core.speed_tier.label(),
        8.0,
        egui::TextFormat::simple(small, speed_color(core.speed_tier, palette)),
    );
    job
}

/// The OPL Combined/Separate control: one core for the whole family, or a core
/// per generation. Drawn once, below the roster, because the split is a
/// family-wide, config-wide choice -- not a property of any one row.
///
/// Editing the map reshapes the roster on the next frame (an OPL2 row appears
/// or folds away), which is why the radios live beside the grid, not in it.
pub(crate) fn opl_mode_radios(
    ui: &mut egui::Ui,
    palette: &Palette,
    cores: &mut std::collections::BTreeMap<String, String>,
) {
    let split = opl_split(cores);
    ui.horizontal(|ui| {
        ui.colored_label(palette.label, crate::strings::CHIP_OUTPUT_OPL_MODE);
        if ui
            .radio(!split, crate::strings::CHIP_OUTPUT_OPL_COMBINED)
            .on_hover_text(crate::strings::CHIP_OUTPUT_OPL_COMBINED_HOVER)
            .clicked()
            && split
        {
            merge_opl(cores);
        }
        if ui
            .radio(split, crate::strings::CHIP_OUTPUT_OPL_SEPARATE)
            .on_hover_text(crate::strings::CHIP_OUTPUT_OPL_SEPARATE_HOVER)
            .clicked()
            && !split
        {
            split_opl(cores);
        }
    });
}

/// The picker legend: what the accuracy badge colours and the speed words
/// mean. Folded away by default (a `CollapsingHeader`), so it explains the
/// labels on demand without widening or lengthening the dialog.
pub(crate) fn legend(ui: &mut egui::Ui, palette: &Palette) {
    egui::CollapsingHeader::new(
        egui::RichText::new(crate::strings::CHIP_OUTPUT_LEGEND)
            .small()
            .color(palette.muted),
    )
    .id_salt("chip-output-legend")
    .show(ui, |ui| {
        ui.colored_label(
            palette.data_label,
            crate::strings::CHIP_OUTPUT_LEGEND_ACCURACY,
        );
        for tier in vgms_synth::CoreTier::ALL {
            ui.horizontal(|ui| {
                let color = tier_color(tier);
                egui::Frame::new()
                    .fill(color.gamma_multiply(0.18))
                    .stroke(egui::Stroke::new(1.0, palette.bevel_border))
                    .inner_margin(egui::Margin::symmetric(4, 1))
                    .corner_radius(2.0)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(tier.badge()).small().color(color));
                    });
                ui.label(
                    egui::RichText::new(tier.description())
                        .small()
                        .color(palette.muted),
                );
            });
        }
        ui.add_space(4.0);
        ui.colored_label(palette.data_label, crate::strings::CHIP_OUTPUT_LEGEND_SPEED);
        for band in vgms_synth::speed::SpeedTier::ALL {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(band.label())
                        .small()
                        .color(speed_color(band, palette)),
                );
                ui.label(
                    egui::RichText::new(band.description())
                        .small()
                        .color(palette.muted),
                );
            });
        }
    });
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
                tier: vgms_synth::CoreTier::Cycle,
                exact: false,
                realtime: true,
                channel_pan: false,
                // Deliberately `true` while the real CQM is now `channel_mute:
                // false` (see `vgms-cores-nuked`). This stand-in is `Routed`,
                // which bypasses the write-gate, so `mute_capable` here is just
                // this flag: `false` would report *all four* OPL chips
                // un-muteable -- a worse divergence than the one Y8950 toggle.
                // Faithfully matching the app would need a real `make: Opl`
                // core, which this crate cannot link. Do not "sync" it.
                channel_mute: true,
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
                tier: vgms_synth::CoreTier::Hardware,
                exact: chip == registry::OPL_CHIPS[0],
                realtime: true,
                channel_pan: false,
                channel_mute: true,
                level: vgms_synth::LEVEL_UNITY,
                make: vgms_synth::CoreMaker::Routed,
            });
        }
        // The OPL2-generation die sim, as the app's `vgms-cores-gpl` registers
        // it: a Generic maker for the OPL2-generation chips only, so the split
        // OPL2 row offers a genuine alternative (and one the combined row must
        // NOT list -- an OPL3 song cannot play through an OPL2 die).
        for chip in [ChipKind::Ym3812, ChipKind::Ym3526, ChipKind::Y8950] {
            registry.register(CoreInfo {
                id: "opl3.ym3812-lle",
                chip,
                label: "YM3812-LLE",
                authors: "Nuke.YKT",
                license: "GPL-2.0-or-later",
                upstream: "https://github.com/nukeykt/YM3812-LLE",
                tier: vgms_synth::CoreTier::DieSim,
                exact: chip == ChipKind::Ym3812,
                realtime: false,
                channel_pan: false,
                channel_mute: false,
                level: vgms_synth::LEVEL_UNITY,
                make: vgms_synth::CoreMaker::Generic(|| Box::new(ToneStub::new())),
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
            tier: vgms_synth::CoreTier::Behavioural,
            exact: true,
            realtime: true,
            // As the real one: the SN76489's Maxim core carries `SetPanning`,
            // so this stand-in claims it too and the GUI tests exercise the
            // chip panel's pan controls. The stub itself ignores the positions
            // -- what is under test is the panel and the service call.
            channel_pan: true,
            channel_mute: true,
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
        let rows = rows(false);
        // The row is titled by its representative chip's part name, as every
        // other row is (the combined OPL row stands for the YMF262).
        assert_eq!(rows[0].label, ChipKind::Ymf262.name());
        assert_eq!(rows[0].slot, vgms_core::config::OPL_SLOT);
        assert!(
            rows[0].is_choice(),
            "emulator or board -- the one row that has always offered two"
        );
        assert!(
            !rows[0].cores.iter().any(|core| core.name == "ym3812-lle"),
            "the combined row must not offer an OPL2-only core -- an OPL3 \
             song cannot play through the OPL2 die"
        );
    }

    /// The split roster: the OPL2 row leads (its own slot, its own cores, the
    /// OPL2-only die included, routed hardware excluded), the OPL3 row keeps
    /// the family slot, and everything else is untouched.
    #[test]
    fn the_split_roster_offers_opl2_and_opl3_rows() {
        install_test_cores();
        let split = rows(true);
        assert_eq!(split[0].label, ChipKind::Ym3812.name());
        assert_eq!(split[0].slot, vgms_core::config::OPL2_SLOT);
        assert!(
            split[0].cores.iter().any(|core| core.name == "ym3812-lle"),
            "the split OPL2 row is how the OPL2-only die becomes choosable"
        );
        assert!(
            !split[0]
                .cores
                .iter()
                .any(|core| core.name == vgms_core::config::RETROWAVE_CORE),
            "hardware is a whole-family backend keyed on the OPL3 slot, so \
             the OPL2 row must not offer a key nothing routes on"
        );
        assert_eq!(split[1].label, ChipKind::Ymf262.name());
        assert_eq!(split[1].slot, vgms_core::config::OPL_SLOT);
        assert!(
            split[1]
                .cores
                .iter()
                .any(|core| core.name == vgms_core::config::RETROWAVE_CORE),
            "the family slot keeps the hardware row"
        );
        assert_eq!(
            split.len(),
            rows(false).len() + 1,
            "splitting adds exactly the OPL2 row"
        );
    }

    /// Splitting seeds the OPL2 slot with what is already heard; a combined
    /// choice the OPL2 row cannot honour (routed hardware) seeds its default
    /// instead, and merging is just removing the key.
    #[test]
    fn split_seeds_the_opl2_slot_and_merge_removes_it() {
        install_test_cores();

        let mut cores =
            BTreeMap::from([(vgms_core::config::OPL_SLOT.to_owned(), "cqm".to_owned())]);
        assert!(!opl_split(&cores));
        split_opl(&mut cores);
        assert!(opl_split(&cores));
        // The test registry's CQM stand-in is Routed, so the seed falls back
        // to the OPL2 row's first core rather than parroting the name.
        let opl2_row = rows(true).into_iter().next().expect("an OPL2 row");
        assert_eq!(
            cores.get(vgms_core::config::OPL2_SLOT),
            Some(&opl2_row.cores[0].name),
            "an un-honourable combined choice seeds the OPL2 default"
        );

        merge_opl(&mut cores);
        assert!(!opl_split(&cores));
        assert_eq!(
            cores.get(vgms_core::config::OPL_SLOT).map(String::as_str),
            Some("cqm"),
            "merging leaves the family choice alone"
        );
    }

    /// With the split on, an OPL2 song's row is the OPL2 row, an OPL3 song's
    /// the OPL3 row, and the other generation stays configurable under All.
    #[test]
    fn plan_with_split_routes_each_generation_to_its_row() {
        install_test_cores();

        let opl2_song = plan(&[ChipKind::Ym3812], true);
        let row = opl2_song.song[0].row.as_ref().expect("OPL2 has cores");
        assert_eq!(row.slot, vgms_core::config::OPL2_SLOT);
        assert!(
            opl2_song
                .all
                .iter()
                .any(|row| row.slot == vgms_core::config::OPL_SLOT),
            "the OPL3 row stays configurable under All chips"
        );

        let opl3_song = plan(&[ChipKind::Ymf262], true);
        let row = opl3_song.song[0].row.as_ref().expect("OPL3 has cores");
        assert_eq!(row.slot, vgms_core::config::OPL_SLOT);
        assert!(
            opl3_song
                .all
                .iter()
                .any(|row| row.slot == vgms_core::config::OPL2_SLOT),
            "the OPL2 row stays configurable under All chips"
        );
    }

    #[test]
    fn a_chip_with_a_core_gets_a_row_and_one_without_does_not() {
        install_test_cores();
        let rows = rows(false);
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
        let listed = rows(false).len() - 1; // the OPL row stands for `opl` chips
        assert_eq!(opl + listed + without_cores(), ChipKind::all().count());
    }

    /// Every row is a picker over registry entries, so a core cannot appear
    /// without a licence on record -- shown in the About credits, not here,
    /// but required all the same.
    #[test]
    fn every_offered_core_shows_its_license() {
        install_test_cores();
        for row in rows(false).into_iter().chain(rows(true)) {
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

    /// The fidelity sort: exact beats tier, tier beats registry order,
    /// hardware is never picked, and a tracked speed must hold realtime with
    /// headroom -- which is exactly what lets a faster machine promote a die
    /// sim some day.
    #[test]
    fn fidelity_pick_prefers_exact_then_tier_and_respects_the_speed_gate() {
        fn core(
            name: &str,
            tier: vgms_synth::CoreTier,
            exact: bool,
            speed: Option<f32>,
        ) -> CoreChoice {
            CoreChoice {
                name: name.to_owned(),
                label: name.to_owned(),
                license: "GPL-2.0-or-later".to_owned(),
                tier,
                exact,
                routed: false,
                speed,
                speed_tier: match speed {
                    Some(speed) => vgms_synth::speed::SpeedTier::of(speed),
                    None => vgms_synth::speed::SpeedTier::Fast,
                },
            }
        }
        let mut row = ChipOutputRow {
            slot: "opl2",
            label: "YM3812",
            chip: ChipKind::Ym3812,
            cores: vec![
                // The real OPL2 roster's shape: a cycle model of the wrong
                // generation, a fast exact model, and an exact die sim that
                // is too slow on this machine.
                core("nuked", vgms_synth::CoreTier::Cycle, false, Some(50.0)),
                core(
                    "opl2-lite",
                    vgms_synth::CoreTier::Behavioural,
                    true,
                    Some(40.0),
                ),
                core("ym3812-lle", vgms_synth::CoreTier::DieSim, true, Some(1.2)),
            ],
        };
        assert_eq!(
            fidelity_pick(&row).map(|core| core.name.as_str()),
            Some("opl2-lite"),
            "an exact model beats a wrong-generation cycle model, and the \
             die sim below the headroom bar is skipped"
        );

        // The blistering-fast future: the same die sim, now clearing the bar.
        row.cores[2].speed = Some(3.0);
        assert_eq!(
            fidelity_pick(&row).map(|core| core.name.as_str()),
            Some("ym3812-lle"),
            "a die sim that holds realtime is the fidelity pick"
        );

        // Hardware is a backend decision, never auto-picked.
        row.cores.push(CoreChoice {
            routed: true,
            ..core("retrowave", vgms_synth::CoreTier::Hardware, true, None)
        });
        assert_eq!(
            fidelity_pick(&row).map(|core| core.name.as_str()),
            Some("ym3812-lle")
        );
    }

    /// The auto-select writes the pick into every choosable slot and leaves
    /// the rest of the map alone.
    #[test]
    fn auto_select_fills_the_choosable_slots() {
        install_test_cores();
        let mut cores = BTreeMap::from([("keep".to_owned(), "me".to_owned())]);
        auto_select(&mut cores);
        assert_eq!(
            cores.get(vgms_core::config::OPL_SLOT).map(String::as_str),
            Some("nuked"),
            "the stand-in OPL roster's only non-routed core"
        );
        assert_eq!(cores.get("keep").map(String::as_str), Some("me"));
    }

    /// The song's chips are hoisted in file order, single-core chips fold, and
    /// nothing appears twice.
    #[test]
    fn plan_splits_the_song_from_the_rest() {
        install_test_cores();
        // The stand-in registry gives the OPL family a choice (emulator, CQM,
        // hardware) and the SN76489 a single core.
        let plan = plan(&[ChipKind::Ymf262], false);

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
        let plan = plan(&[ChipKind::Scsp], false);
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
        let plan = plan(&[], false);
        assert!(plan.song.is_empty());
        assert!(
            plan.all.iter().any(|row| row.slot == "opl3"),
            "every chip is under All chips instead"
        );
    }

    /// A `vgmstudio.ini` naming a core this build lacks -- the native app's
    /// `retrowave` read by the web build -- must show what will really play,
    /// not echo a setting that is not in force.
    #[test]
    fn a_core_this_build_lacks_falls_back_to_the_first() {
        install_test_cores();
        let row = rows(false).into_iter().next().expect("an OPL row");
        let mut cores = BTreeMap::from([(row.slot.to_owned(), "nonesuch".to_owned())]);
        assert_eq!(selected_name(&cores, &row), row.cores[0].name);

        cores.insert(row.slot.to_owned(), row.cores[0].name.clone());
        assert_eq!(selected_name(&cores, &row), row.cores[0].name);
    }
}
