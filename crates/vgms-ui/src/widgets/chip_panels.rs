//! The per-chip control deck: a chip mixer strip, and one panel per chip.
//!
//! A document declares its chips; each becomes a cell in the strip -- a generic
//! chip its **lamp · trim knob · name**, the OPL device a plain named cell --
//! and the selected chip's name draws its own controls below. An OPL document (a
//! DRO, or a VGM the editor opens through its projection) shows the OPL
//! [`ChannelPanel`](super::channels::ChannelPanel) -- eighteen channels across
//! two banks, drum groups, the OPL stereo-ext image. Any other chip shows a
//! [`GenericChannelPanel`](super::chip_channels::GenericChannelPanel): a flat
//! mute/solo list from [`vgms_core::vgm::channels_of`], with pan knobs only
//! where the chip's core can pan.
//!
//! The lamp is the whole-chip mute/solo, one per chip: left-click mutes,
//! right-click solos (additive), coloured by the chip's play state. The knob
//! trims that chip's level. Both act through the engine's own gain/mask, so they
//! work on every core. The strip is drawn **always** -- even for a single chip,
//! even an empty editor -- so the deck's shape does not jump as documents come
//! and go, and it wraps to a second row rather than scrolling when a wide chip
//! set outgrows the deck.
//!
//! The OPL entry covers the whole OPL device, both banks together, rather than
//! splitting dual OPL2 into two entries: those eighteen channels are one
//! panel's worth of controls. A generic multichip file instead gets one entry
//! per chip *instance* -- a dual SN76489 is two cells -- because a user mutes
//! one of the pair, not the kind.

use vgms_core::vgm::ChipKind;
use vgms_core::{OplType, Song, VgmFile};
use vgms_synth::{ChipMuting, ChipPanning, ChipTrims, Muting, Panning};

use super::channels::{ChannelPanel, ChannelsResponse};
use super::chip_channels::GenericChannelPanel;
use super::pan_knob;
use crate::theme::paint::darken;
use crate::theme::{Palette, tabs};

/// Padding between the selector well's edge and its cells.
const WELL_PAD: i8 = 3;
/// The selector well's corner radius.
const WELL_RADIUS: u8 = 3;
/// Gap between one chip's cell and the next, across a row.
const CELL_GAP: f32 = 10.0;
/// Gap between the strip's rows once it wraps.
const ROW_GAP: f32 = 4.0;
/// Gap between a cell's own lamp, knob and name.
const CELL_INNER_GAP: f32 = 4.0;
/// The lamp's drawn side, for measuring a cell's width.
const LAMP_SIZE: f32 = 12.0;
/// The full trim, and the level the OPL device rests at: 100%.
const TRIM_FULL: u8 = 100;

/// What a chip contributes to the deck.
#[derive(Debug)]
enum ChipControls {
    /// The OPL mute/pan panel, shared as [`ChipPanels::opl`].
    Opl,
    /// A generic chip's own mute/solo/pan panel.
    Generic(GenericChannelPanel),
}

/// One chip in the strip.
#[derive(Debug)]
struct ChipEntry {
    label: String,
    controls: ChipControls,
}

/// The chips of the loaded document, and the controls for the selected one.
#[derive(Debug)]
pub(crate) struct ChipPanels {
    entries: Vec<ChipEntry>,
    selected: usize,
    /// The OPL panel. It outlives any particular document's chip list: the
    /// audio output speaks OPL muting and panning whenever an OPL document is
    /// loaded, and an empty editor still shows its channel toggles.
    opl: ChannelPanel,
    /// The OPL device's whole-chip mute, the lamp's left-click on the OPL cell.
    /// Folded into [`muting`](Self::muting) as `Muting::silent`. The OPL has no
    /// sibling to solo against, so its lamp is mute-only.
    opl_muted: bool,
    /// The OPL device's listening trim, `0..=100`%. Keyed to the projected chip
    /// in [`chip_trims`](Self::chip_trims); listening-only, like every trim.
    opl_trim: u8,
}

impl Default for ChipPanels {
    fn default() -> Self {
        Self {
            // An empty editor still shows the OPL panel.
            entries: vec![ChipEntry {
                label: opl_label(OplType::Opl3).to_owned(),
                controls: ChipControls::Opl,
            }],
            selected: 0,
            opl: ChannelPanel::new(),
            opl_muted: false,
            opl_trim: TRIM_FULL,
        }
    }
}

impl ChipPanels {
    /// A deck with no document: the OPL panel, ready.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The deck for an OPL song: one OPL entry.
    #[must_use]
    pub(crate) fn for_song(song: &Song) -> Self {
        Self {
            entries: vec![ChipEntry {
                label: opl_label(song.opl_type).to_owned(),
                controls: ChipControls::Opl,
            }],
            selected: 0,
            opl: ChannelPanel::for_song(song),
            opl_muted: false,
            opl_trim: TRIM_FULL,
        }
    }

    /// The deck for a generic multichip VGM: one entry per chip instance, in
    /// header order.
    #[must_use]
    pub(crate) fn for_vgm(file: &VgmFile) -> Self {
        let mut entries = Vec::new();
        for chip in file.header.chips() {
            let instances = if chip.dual { 2 } else { 1 };
            for instance in 0..instances {
                entries.push(ChipEntry {
                    label: instance_label(chip.kind, chip.variant, instance),
                    controls: ChipControls::Generic(GenericChannelPanel::new(
                        chip.kind,
                        instance,
                        chip.variant,
                    )),
                });
            }
        }
        // A file that declares no chip at all still gets the OPL panel, rather
        // than an empty strip with nothing to draw.
        if entries.is_empty() {
            return Self::new();
        }
        Self {
            entries,
            selected: 0,
            opl: ChannelPanel::new(),
            opl_muted: false,
            opl_trim: TRIM_FULL,
        }
    }

    /// Adopts a new chip type after a live DRO Info edit.
    pub(crate) fn set_opl_type(&mut self, opl_type: OplType, song: Option<&Song>) {
        self.opl.set_opl_type(opl_type, song);
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| matches!(entry.controls, ChipControls::Opl))
        {
            entry.label = opl_label(opl_type).to_owned();
        }
    }

    /// The OPL panel, for the tests that drive it directly.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub(crate) fn opl(&mut self) -> &mut ChannelPanel {
        &mut self.opl
    }

    /// The OPL muting the OPL panel describes (for the OPL playback path). The
    /// OPL cell's whole-chip mute silences the lot, over the per-channel toggles
    /// -- the OPL counterpart of a generic chip's `chip_muted`.
    #[must_use]
    pub(crate) fn muting(&self) -> Muting {
        if self.opl_muted {
            Muting::silent()
        } else {
            self.opl.muting()
        }
    }

    /// The OPL panning the OPL panel describes.
    #[must_use]
    pub(crate) fn panning(&self) -> Panning {
        self.opl.panning()
    }

    /// The any-chip mutes every generic panel describes (for the generic
    /// playback path). Empty when the document is OPL. Solo is folded in here,
    /// at the document level: a chip that is not soloed while any chip is
    /// soloed is silenced whole, on top of its own mask.
    #[must_use]
    pub(crate) fn chip_muting(&self) -> ChipMuting {
        let any_solo = self.any_solo();
        let mut muting = ChipMuting::new();
        for entry in &self.entries {
            if let ChipControls::Generic(panel) = &entry.controls {
                muting.set(
                    panel.kind(),
                    panel.instance(),
                    panel.mask_effective(any_solo),
                );
            }
        }
        muting
    }

    /// The per-chip listening trims every panel describes. Generic chips key by
    /// their own `(kind, instance)`; the OPL device keys by the chip its
    /// projection plays through -- a dual OPL2 is two `Ym3812` instances, so it
    /// trims both. Neutral when nothing is loaded.
    #[must_use]
    pub(crate) fn chip_trims(&self) -> ChipTrims {
        let mut trims = ChipTrims::new();
        for entry in &self.entries {
            if let ChipControls::Generic(panel) = &entry.controls {
                trims.set(panel.kind(), panel.instance(), panel.trim());
            }
        }
        if let Some(opl_type) = self.opl.opl_type() {
            let kind = vgms_synth::opl_projection_kind(opl_type);
            trims.set(kind, 0, self.opl_trim);
            if opl_type == OplType::DualOpl2 {
                trims.set(kind, 1, self.opl_trim);
            }
        }
        trims
    }

    /// Whether any generic chip is soloed -- a document-level fact the lamps and
    /// the effective mute mask both read.
    fn any_solo(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(&entry.controls, ChipControls::Generic(panel) if panel.soloed()))
    }

    /// The any-chip pans every generic panel in Custom mode describes.
    #[must_use]
    pub(crate) fn chip_panning(&self) -> ChipPanning {
        let mut panning = ChipPanning::new();
        for entry in &self.entries {
            if let ChipControls::Generic(panel) = &entry.controls
                && let Some(pans) = panel.pan_entry()
            {
                panning.set(panel.kind(), panel.instance(), pans);
            }
        }
        panning
    }

    /// The chip whose controls are on screen, or `None` when the OPL panel is
    /// selected. What the app asks to decide the selected tab's pan support.
    #[must_use]
    pub(crate) fn selected_chip(&self) -> Option<ChipKind> {
        match self.entries.get(self.selected).map(|e| &e.controls) {
            Some(ChipControls::Generic(panel)) => Some(panel.kind()),
            _ => None,
        }
    }

    /// The label of the chip whose controls are on screen.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub(crate) fn selected_label(&self) -> Option<&str> {
        self.entries
            .get(self.selected)
            .map(|entry| entry.label.as_str())
    }

    /// Toggles channel `index` on the *selected* chip's panel -- so the number
    /// keys act on whatever tab is open, not always on the OPL one.
    pub(crate) fn toggle_selected_channel(&mut self, index: usize) {
        match self.entries.get_mut(self.selected).map(|e| &mut e.controls) {
            Some(ChipControls::Generic(panel)) => panel.toggle_channel(index),
            // The OPL panel, or an empty deck: the OPL toggles.
            _ => self.opl.toggle_channel(index),
        }
    }

    /// Draws the selector strip (always) and the selected chip's controls.
    ///
    /// `pan_supported(chip)` / `mute_supported(chip)` answer whether pan and mute
    /// controls should be live for a given chip -- `None` for the OPL panel,
    /// `Some(kind)` for a generic one. The app supplies them because the
    /// capability is a registry question the panel does not own. The OPL panel
    /// always mutes (register-gated), so only its pan support is consulted.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        pan_supported: impl Fn(Option<ChipKind>) -> bool,
        mute_supported: impl Fn(Option<ChipKind>) -> bool,
    ) -> ChannelsResponse {
        let mut response = self.selector(ui, palette);
        let chip = self.selected_chip();
        let pan = pan_supported(chip);
        let body = match self.entries.get_mut(self.selected).map(|e| &mut e.controls) {
            Some(ChipControls::Generic(panel)) => {
                panel.show(ui, palette, pan, mute_supported(chip))
            }
            // The OPL entry, or (defensively) an out-of-range selection.
            _ => self.opl.show(ui, palette, pan),
        };
        response.muting_changed |= body.muting_changed;
        response.panning_changed |= body.panning_changed;
        response.trim_changed |= body.trim_changed;
        response
    }

    /// Draws the chip selector: each generic chip a cell of **lamp · trim knob ·
    /// name**, and the OPL device the same once a song is loaded (mute-only, keyed
    /// to the whole device). The lamp is the whole-chip mute/solo the pads used to
    /// be -- left-click mutes, right-click solos, for every chip rather than the
    /// selected one -- coloured by its play state. The name is drawn in the
    /// Editor/Pack tab chrome. The cells sit in a readout well and wrap to a
    /// second row when the deck is too narrow, never scrolling.
    fn selector(&mut self, ui: &mut egui::Ui, palette: &Palette) -> ChannelsResponse {
        let mut response = ChannelsResponse::default();
        let any_solo = self.any_solo();
        let selected_index = self.selected;
        let opl_type = self.opl.opl_type();
        let mut new_selected = self.selected;
        // The OPL device's mixer state lives on `self` beside `entries`; copy it
        // out so the cell loop can borrow `self.entries` mutably without also
        // borrowing these, then write back after.
        let mut opl_muted = self.opl_muted;
        let mut opl_trim = self.opl_trim;
        // The well is placed straight in the deck's vertical layout. The cells go
        // in a `Grid` broken every `cols` -- a plain wrapping layout will not wrap
        // a multi-widget cell, whose size it cannot know before placing it, so the
        // row count is worked out from the widest cell and the deck's width. Each
        // cell names its own chip, so the strip needs no "Chip:" prefix.
        egui::Frame::new()
            .fill(palette.data_bg)
            .stroke(egui::Stroke::new(1.0, palette.plate_border))
            .corner_radius(egui::CornerRadius::same(WELL_RADIUS))
            .inner_margin(egui::Margin::same(WELL_PAD))
            .show(ui, |ui| {
                let cols = self.columns_that_fit(ui);
                let last = self.entries.len().saturating_sub(1);
                egui::Grid::new("chip-selector-grid")
                    .spacing([CELL_GAP, ROW_GAP])
                    .show(ui, |ui| {
                        for (at, entry) in self.entries.iter_mut().enumerate() {
                            let selected = at == selected_index;
                            let ChipEntry { label, controls } = entry;
                            let name = label.as_str();
                            let clicked = match controls {
                                ChipControls::Generic(panel) => {
                                    ui.horizontal(|ui| {
                                        generic_cell(
                                            ui,
                                            palette,
                                            name,
                                            panel,
                                            any_solo,
                                            selected,
                                            &mut response,
                                        )
                                    })
                                    .inner
                                }
                                ChipControls::Opl => {
                                    ui.horizontal(|ui| {
                                        opl_cell(
                                            ui,
                                            palette,
                                            name,
                                            opl_type,
                                            &mut opl_muted,
                                            &mut opl_trim,
                                            selected,
                                            &mut response,
                                        )
                                    })
                                    .inner
                                }
                            };
                            if clicked {
                                new_selected = at;
                            }
                            if (at + 1) % cols == 0 && at != last {
                                ui.end_row();
                            }
                        }
                    });
            });
        self.selected = new_selected;
        self.opl_muted = opl_muted;
        self.opl_trim = opl_trim;
        response
    }

    /// How many chip cells fit on one row of the well before it must wrap: the
    /// deck's width divided by the widest cell. At least one, so a deck narrower
    /// than a single cell still shows it (clipped) rather than dividing by zero.
    fn columns_that_fit(&self, ui: &mut egui::Ui) -> usize {
        let font = egui::TextStyle::Button.resolve(ui.style());
        // The name is a `tabs::tab_button`, padded on each side; a small
        // over-estimate only wraps a touch early, which is the safe direction.
        let name_pad = 20.0;
        let opl_has_controls = self.opl.opl_type().is_some();
        let widest = self
            .entries
            .iter()
            .map(|entry| {
                let name = ui.fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            entry.label.clone(),
                            font.clone(),
                            egui::Color32::PLACEHOLDER,
                        )
                        .size()
                        .x
                });
                let name_cell = name + name_pad;
                let with_controls = match entry.controls {
                    ChipControls::Generic(_) => true,
                    ChipControls::Opl => opl_has_controls,
                };
                if with_controls {
                    // lamp + knob + name, a gap between each.
                    LAMP_SIZE + CELL_INNER_GAP + pan_knob::SIZE + CELL_INNER_GAP + name_cell
                } else {
                    name_cell
                }
            })
            .fold(1.0_f32, f32::max);
        let avail = ui.available_width();
        (((avail + CELL_GAP) / (widest + CELL_GAP)).floor() as usize).max(1)
    }
}

/// A generic chip's cell: lamp, trim knob, name. Returns whether the name was
/// clicked to select the chip.
fn generic_cell(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    panel: &mut GenericChannelPanel,
    any_solo: bool,
    selected: bool,
    response: &mut ChannelsResponse,
) -> bool {
    ui.spacing_mut().item_spacing.x = CELL_INNER_GAP;
    // The lamp: whole-chip mute (left-click) and solo (right-click), on every
    // core -- a whole-chip mask silences the voice in the engine itself. It
    // never gates on the core's per-channel mute.
    let lamp = crate::theme::led_button(ui, led_color(palette, panel, any_solo))
        .on_hover_text(led_hover(panel, any_solo));
    lamp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{name} lamp"))
    });
    if lamp.clicked() {
        panel.set_chip_muted(!panel.chip_muted());
        response.muting_changed = true;
    }
    if lamp.secondary_clicked() {
        panel.set_soloed(!panel.soloed());
        response.muting_changed = true;
    }

    // The trim knob.
    let mut trim = panel.trim();
    if pan_knob::show_trim(ui, palette, &mut trim, &format!("{name} level")).changed() {
        panel.set_trim(trim);
        response.trim_changed = true;
    }

    // The name, in the Editor/Pack tab chrome; clicking it selects the chip's
    // detailed panel below.
    tabs::tab_button(ui, palette, name, selected).clicked()
}

/// The OPL device's cell. Once a song is loaded it is lamp · trim knob · name
/// like a generic chip, but mute-only -- a one-chip document has nothing to solo
/// against -- and keyed to the whole device. With nothing loaded it is a plain
/// named tab. Returns whether the name was clicked to select it.
#[allow(clippy::too_many_arguments)]
fn opl_cell(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    opl_type: Option<OplType>,
    muted: &mut bool,
    trim: &mut u8,
    selected: bool,
    response: &mut ChannelsResponse,
) -> bool {
    ui.spacing_mut().item_spacing.x = CELL_INNER_GAP;
    // No OPL song: nothing to mix, so just a name.
    if opl_type.is_none() {
        return tabs::tab_button(ui, palette, name, selected).clicked();
    }
    let color = if *muted {
        palette.meter_off
    } else {
        palette.meter_low
    };
    let hover = if *muted {
        crate::strings::CHIP_LAMP_MUTED
    } else {
        crate::strings::CHIP_LAMP_PLAYING
    };
    let lamp = crate::theme::led_button(ui, color).on_hover_text(hover);
    lamp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{name} lamp"))
    });
    if lamp.clicked() {
        *muted = !*muted;
        response.muting_changed = true;
    }
    let mut t = *trim;
    if pan_knob::show_trim(ui, palette, &mut t, &format!("{name} level")).changed() {
        *trim = t;
        response.trim_changed = true;
    }
    tabs::tab_button(ui, palette, name, selected).clicked()
}

/// The lamp colour for a chip's play state (the meter roles, no new palette):
/// green playing, yellow soloed, unlit muted by the user, dim green silenced by
/// another chip's solo. The fourth state keeps "muted by you" and "silenced for
/// you" from collapsing into one dark lamp.
fn led_color(palette: &Palette, panel: &GenericChannelPanel, any_solo: bool) -> egui::Color32 {
    if panel.chip_muted() {
        palette.meter_off
    } else if panel.soloed() {
        palette.meter_mid
    } else if any_solo {
        darken(palette.meter_low, 0.6)
    } else {
        palette.meter_low
    }
}

/// The lamp's hover text for its state.
fn led_hover(panel: &GenericChannelPanel, any_solo: bool) -> &'static str {
    if panel.chip_muted() {
        crate::strings::CHIP_LAMP_MUTED
    } else if panel.soloed() {
        crate::strings::CHIP_LAMP_SOLOED
    } else if any_solo {
        crate::strings::CHIP_LAMP_SILENCED
    } else {
        crate::strings::CHIP_LAMP_PLAYING
    }
}

/// How the deck names an OPL device.
const fn opl_label(opl_type: OplType) -> &'static str {
    match opl_type {
        OplType::Opl2 => "YM3812",
        OplType::DualOpl2 => "YM3812 x2",
        OplType::Opl3 => "YMF262",
    }
}

/// A per-instance tab label: the chip's display name (honouring its variant),
/// with `" #2"` on the second instance of a dual chip.
fn instance_label(kind: ChipKind, variant: bool, instance: u8) -> String {
    let base = match (variant, kind.variant_name()) {
        (true, Some(name)) => name,
        _ => kind.name(),
    };
    if instance == 0 {
        base.to_owned()
    } else {
        format!("{base} #{}", instance + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::tone_song;

    fn vgm_for(chips: &[(ChipKind, u32)]) -> VgmFile {
        fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let mut bytes = vec![0u8; 0x100];
        bytes[..4].copy_from_slice(b"Vgm ");
        put_u32(&mut bytes, 0x08, 0x161);
        put_u32(&mut bytes, 0x34, 0x100 - 0x34);
        for &(kind, clock) in chips {
            put_u32(&mut bytes, kind.clock_offset(), clock);
        }
        bytes.push(0x66);
        let eof = bytes.len();
        put_u32(&mut bytes, 0x04, (eof - 4) as u32);
        vgms_core::vgm::file::read("x.vgm", &bytes).unwrap()
    }

    fn labels(panels: &ChipPanels) -> Vec<&str> {
        panels.entries.iter().map(|e| e.label.as_str()).collect()
    }

    /// A single-chip OPL song: one OPL tab.
    #[test]
    fn a_single_chip_song_has_one_opl_tab() {
        let panels = ChipPanels::for_song(&tone_song());
        assert_eq!(labels(&panels), ["YM3812"]);
        assert_eq!(panels.selected_chip(), None, "the OPL panel, not a chip");
    }

    /// Dual OPL2 is two chips, but one panel's worth of controls -- one tab.
    #[test]
    fn dual_opl2_is_one_tab_covering_both_banks() {
        let mut song = tone_song();
        song.opl_type = OplType::DualOpl2;
        let panels = ChipPanels::for_song(&song);
        assert_eq!(labels(&panels), ["YM3812 x2"]);
    }

    #[test]
    fn a_multi_chip_file_gets_one_tab_per_chip_in_header_order() {
        let file = vgm_for(&[
            (ChipKind::Ym2612, 7_670_454),
            (ChipKind::Sn76489, 3_579_545),
        ]);
        let panels = ChipPanels::for_vgm(&file);
        assert_eq!(labels(&panels), ["SN76489", "YM2612"], "header order");
        assert_eq!(panels.selected_chip(), Some(ChipKind::Sn76489));
    }

    /// A dual chip is two tabs, so a user can mute one instance without the
    /// other.
    #[test]
    fn a_dual_chip_gets_two_instance_tabs() {
        let file = vgm_for(&[(ChipKind::Sn76489, 3_579_545 | 0x4000_0000)]);
        let panels = ChipPanels::for_vgm(&file);
        assert_eq!(labels(&panels), ["SN76489", "SN76489 #2"]);
    }

    /// A single non-OPL chip still gets a (one-tab) strip, so its controls are
    /// labelled.
    #[test]
    fn a_single_non_opl_chip_has_one_named_tab() {
        let file = vgm_for(&[(ChipKind::Ym2612, 7_670_454)]);
        let panels = ChipPanels::for_vgm(&file);
        assert_eq!(labels(&panels), ["YM2612"]);
        assert_eq!(panels.selected_chip(), Some(ChipKind::Ym2612));
    }

    #[test]
    fn a_dro_info_edit_renames_the_opl_tab() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.set_opl_type(OplType::Opl3, None);
        assert_eq!(panels.selected_label(), Some("YMF262"));
    }

    /// The OPL muting/panning still come from the OPL panel.
    #[test]
    fn opl_muting_comes_from_the_opl_panel() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.opl().toggle_channel(3);
        assert_eq!(panels.muting(), panels.opl.muting());
        assert!(panels.chip_muting().is_neutral(), "no generic chips here");
    }

    /// The generic mutes gather every instance's mask, and the number keys act
    /// on the selected tab.
    #[test]
    fn generic_mutes_gather_the_selected_tabs_toggles() {
        let file = vgm_for(&[
            (ChipKind::Sn76489, 3_579_545),
            (ChipKind::Ym2612, 7_670_454),
        ]);
        let mut panels = ChipPanels::for_vgm(&file);
        // The SN76489 tab is selected; number-key channel 1 mutes its Tone 1.
        panels.toggle_selected_channel(0);
        let muting = panels.chip_muting();
        assert_eq!(muting.mask_for(ChipKind::Sn76489, 0), 0b0001);
        assert_eq!(muting.mask_for(ChipKind::Ym2612, 0), 0, "untouched");
    }
}
