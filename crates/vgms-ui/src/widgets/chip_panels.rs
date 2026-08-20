//! The per-chip control deck: a chip mixer strip, and one panel per chip.
//!
//! A document declares its chips; each becomes a cell in the strip -- its
//! **lamp · trim knob · name** -- and the selected chip's name draws its own
//! [`GenericChannelPanel`](super::chip_channels::GenericChannelPanel) below: a
//! flat mute/solo list from [`vgms_core::vgm::channels_of`], with pan knobs
//! where the chip's core can pan.
//!
//! Both document kinds go through the same panels. A VGM declares its chips
//! directly; a DRO projects to its OPL chip set -- one `Ym3812` for OPL2, one
//! `Ymf262` for OPL3, two `Ym3812` for dual OPL2 -- so a DRO and an OPL VGM of
//! the same chips show the identical deck. A DRO's audio, render and split path
//! still speaks the OPL [`Muting`]/[`Panning`] vocabulary, so [`Self::muting`]
//! and [`Self::panning`] bridge the generic panels back to it (the inverse of
//! the [`opl_chip_muting`](vgms_synth::opl_chip_muting) translation the engine
//! applies on the way in); a VGM's generic mutes/pans go straight through
//! [`Self::chip_muting`] / [`Self::chip_panning`].
//!
//! The lamp is the whole-chip mute/solo, one per chip: left-click mutes,
//! right-click solos (exclusive), coloured by the chip's play state. The knob
//! trims that chip's level. Both act through the engine's own gain/mask, so they
//! work on every core. The strip is drawn **always** -- even for a single chip,
//! even an empty editor -- so the deck's shape does not jump as documents come
//! and go, and it wraps to a second row rather than scrolling when a wide chip
//! set outgrows the deck. A fold icon right-aligned beside the strip shows or
//! hides the selected chip's control panel (folded by default); folding hides
//! only those controls, never the strip, and the mix keeps applying.
//!
//! A generic multichip file gets one entry per chip *instance* -- a dual
//! SN76489 is two cells -- because a user mutes one of the pair, not the kind;
//! dual OPL2 is two `Ym3812` cells for the same reason.

use vgms_core::vgm::ChipKind;
use vgms_core::{DroSong, OplType, VgmFile};
use vgms_synth::{ChipMuting, ChipPanning, ChipTrims, Muting, Panning, opl_muting_from_chip};

use super::chip_channels::{ChannelsResponse, GenericChannelPanel};
use super::pan_controls::{PAN_CENTER, PAN_LEFT, PAN_RIGHT};
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
/// The gap below the selector well, so the deck's panel shows between the chip
/// tabs and the controls beneath them -- a bottom margin matching the space
/// above the well (the deck zeroes item spacing, so it is added explicitly).
const SELECTOR_GAP: f32 = 6.0;

/// One chip in the strip: its tab label and its own control panel.
#[derive(Debug)]
struct ChipEntry {
    label: String,
    panel: GenericChannelPanel,
}

/// The chips of the loaded document, and the controls for the selected one.
#[derive(Debug)]
pub(crate) struct ChipPanels {
    entries: Vec<ChipEntry>,
    selected: usize,
    /// The document's OPL type when it is an OPL projection (a DRO); `None` for
    /// a real VGM (OPL or not). Drives the bridge from the generic panels back
    /// to the OPL `Muting`/`Panning` vocabulary a DRO's audio path consumes, and
    /// keeps a DRO on the app's OPL pan/mute rules (see [`Self::selected_chip`]).
    opl_type: Option<OplType>,
}

impl Default for ChipPanels {
    fn default() -> Self {
        // An empty editor still shows a default OPL deck, so the strip does not
        // pop into existence only once a document loads.
        Self {
            entries: opl_entries(OplType::Opl3),
            selected: 0,
            opl_type: None,
        }
    }
}

impl ChipPanels {
    /// A deck with no document: a default OPL panel, ready.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The deck for an OPL song: the chip(s) its *playback* type projects to
    /// (a promoted DualOPL2 shows one YMF262 tab), stored so `muting`/`panning`
    /// key on the same chip the mixer seams do.
    #[must_use]
    pub(crate) fn for_song(song: &DroSong) -> Self {
        let opl_type = song.playback_opl_type();
        Self {
            entries: opl_entries(opl_type),
            selected: 0,
            opl_type: Some(opl_type),
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
                entries.push(chip_entry(chip.kind, instance, chip.variant));
            }
        }
        // A file that declares no chip at all still gets the default panel,
        // rather than an empty strip with nothing to draw.
        if entries.is_empty() {
            return Self::new();
        }
        Self {
            entries,
            selected: 0,
            opl_type: None,
        }
    }

    /// Adopts a new chip type after a live DRO Info edit. A type change alters
    /// the chip and its channel count, so per-channel state cannot carry across;
    /// the projection panels are rebuilt fresh, as a reload would.
    pub(crate) fn set_opl_type(&mut self, opl_type: OplType) {
        self.entries = opl_entries(opl_type);
        self.selected = 0;
        self.opl_type = Some(opl_type);
    }

    /// The OPL muting the deck describes, for the OPL playback/render/split path.
    ///
    /// A DRO's generic panels speak [`ChipMuting`] keyed by the projection chip;
    /// this reverse-translates that (solo folded in) into the OPL `Muting` the
    /// path still consumes. A real VGM has no OPL reading, so the value is
    /// neutral -- its engine ignores it and reads [`Self::chip_muting`] instead.
    #[must_use]
    pub(crate) fn muting(&self) -> Muting {
        match self.opl_type {
            Some(opl_type) => opl_muting_from_chip(&self.chip_muting(), opl_type),
            None => Muting::all(),
        }
    }

    /// The OPL panning the deck describes, for the OPL playback/render/split path.
    ///
    /// Every panel on its own image is `Original` -- except dual OPL2, whose
    /// authentic hard-L/R chip split is its default. Once any OPL instance is in
    /// Custom mode the whole 18-slot image is emitted: each melodic slot takes
    /// its instance's pan byte, an instance still on Original keeping the type's
    /// default. Neutral (`Original`) for a real VGM, whose engine ignores it.
    #[must_use]
    pub(crate) fn panning(&self) -> Panning {
        let Some(opl_type) = self.opl_type else {
            return Panning::Original;
        };
        let any_custom = self
            .entries
            .iter()
            .any(|entry| entry.panel.custom_pan_bytes().is_some());
        if !any_custom {
            return match opl_type {
                OplType::DualOpl2 => Panning::Custom(dual_opl2_image()),
                _ => Panning::Original,
            };
        }
        let mut image = default_opl_image(opl_type);
        for (slot, byte) in image.iter_mut().enumerate() {
            if let Some((instance, channel)) = opl_slot(opl_type, slot)
                && let Some(bytes) = self.instance_custom_bytes(instance)
                && let Some(&custom) = bytes.get(channel)
            {
                *byte = custom;
            }
        }
        Panning::Custom(image)
    }

    /// One OPL instance's custom pan bytes, or `None` if it is on Original.
    fn instance_custom_bytes(&self, instance: u8) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.panel.instance() == instance)
            .and_then(|entry| entry.panel.custom_pan_bytes())
    }

    /// The any-chip mutes every panel describes (for the generic playback path).
    /// Solo is folded in at the document level: a chip that is not soloed while
    /// any chip is soloed is silenced whole, on top of its own mask.
    #[must_use]
    pub(crate) fn chip_muting(&self) -> ChipMuting {
        let any_solo = self.any_solo();
        let mut muting = ChipMuting::new();
        for entry in &self.entries {
            muting.set(
                entry.panel.kind(),
                entry.panel.instance(),
                entry.panel.mask_effective(any_solo),
            );
        }
        muting
    }

    /// The per-chip listening trims every panel describes. Keyed by each panel's
    /// own `(kind, instance)` -- which for a DRO is the projection chip, so the
    /// trim reaches its projected voice. Neutral when nothing is loaded.
    #[must_use]
    pub(crate) fn chip_trims(&self) -> ChipTrims {
        let mut trims = ChipTrims::new();
        for entry in &self.entries {
            trims.set(
                entry.panel.kind(),
                entry.panel.instance(),
                entry.panel.trim(),
            );
        }
        trims
    }

    /// Whether any chip is soloed -- a document-level fact the lamps and the
    /// effective mute mask both read.
    fn any_solo(&self) -> bool {
        self.entries.iter().any(|entry| entry.panel.soloed())
    }

    /// The any-chip pans every panel in Custom mode describes.
    #[must_use]
    pub(crate) fn chip_panning(&self) -> ChipPanning {
        let mut panning = ChipPanning::new();
        for entry in &self.entries {
            if let Some(pans) = entry.panel.pan_entry() {
                panning.set(entry.panel.kind(), entry.panel.instance(), pans);
            }
        }
        panning
    }

    /// The chip whose controls are on screen for keying pan/mute support, or
    /// `None` for the OPL rules.
    ///
    /// A DRO *is* the OPL device: it reports `None` so the app applies its OPL
    /// pan/mute rules (the `None` arm of `pan_supported`/`mute_supported`),
    /// exactly as the old OPL panel did. A real VGM keys support by the selected
    /// chip's kind.
    #[must_use]
    pub(crate) fn selected_chip(&self) -> Option<ChipKind> {
        if self.opl_type.is_some() {
            return None;
        }
        self.entries.get(self.selected).map(|e| e.panel.kind())
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
    /// keys act on whatever tab is open.
    pub(crate) fn toggle_selected_channel(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(self.selected) {
            entry.panel.toggle_channel(index);
        }
    }

    /// Draws the selector strip (always), a right-aligned fold icon, and --
    /// while `expanded` -- the selected chip's controls. The icon toggles
    /// `expanded`; folding hides only the controls below the strip, so the
    /// lamps, trims and tabs stay reachable and the mix keeps applying.
    ///
    /// `pan_supported(chip)` / `mute_supported(chip)` answer whether pan and mute
    /// controls should be live for a given chip -- `None` for the OPL rules,
    /// `Some(kind)` for a generic chip. The app supplies them because the
    /// capability is a registry question the panel does not own.
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        expanded: &mut bool,
        pan_supported: impl Fn(Option<ChipKind>) -> bool,
        mute_supported: impl Fn(Option<ChipKind>) -> bool,
    ) -> ChannelsResponse {
        let mut response = ui
            .horizontal(|ui| {
                let response = self.selector(ui, palette);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    fold_icon(ui, palette, expanded);
                });
                response
            })
            .inner;
        // A bottom margin under the tabs so the deck shows between them and the
        // controls below, matching the space above the well.
        ui.add_space(SELECTOR_GAP);
        if !*expanded {
            return response;
        }
        let chip = self.selected_chip();
        let pan = pan_supported(chip);
        let mute = mute_supported(chip);
        if let Some(entry) = self.entries.get_mut(self.selected) {
            let body = entry.panel.show(ui, palette, pan, mute);
            response.muting_changed |= body.muting_changed;
            response.panning_changed |= body.panning_changed;
            response.trim_changed |= body.trim_changed;
        }
        response
    }

    /// Draws the chip selector: each chip a cell of **lamp · trim knob · name**.
    /// The lamp is the whole-chip mute/solo -- left-click mutes, right-click
    /// solos, for every chip rather than the selected one -- coloured by its play
    /// state. The name is drawn in the Editor/Pack tab chrome. The cells sit in a
    /// readout well and wrap to a second row when the deck is too narrow, never
    /// scrolling.
    fn selector(&mut self, ui: &mut egui::Ui, palette: &Palette) -> ChannelsResponse {
        let mut response = ChannelsResponse::default();
        let any_solo = self.any_solo();
        let selected_index = self.selected;
        let mut new_selected = self.selected;
        // The chip whose lamp was right-clicked this frame, applied as an
        // exclusive solo once the cell loop's borrow of `entries` is released.
        let mut solo_target: Option<usize> = None;
        // The cells go in a `Grid` broken every `cols` -- a plain wrapping layout
        // will not wrap a multi-widget cell, whose size it cannot know before
        // placing it, so the row count is worked out from the widest cell and the
        // deck's width. Each cell names its own chip, so the strip needs no
        // "Chip:" prefix.
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
                            let ChipEntry { label, panel } = entry;
                            let name = label.as_str();
                            let outcome = ui
                                .horizontal(|ui| {
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
                                .inner;
                            if outcome.select {
                                new_selected = at;
                            }
                            if outcome.solo_clicked {
                                solo_target = Some(at);
                            }
                            if (at + 1) % cols == 0 && at != last {
                                ui.end_row();
                            }
                        }
                    });
            });
        self.selected = new_selected;
        // Apply the lamp's right-click as an exclusive solo now the loop's
        // borrow of `entries` is gone: it touches every chip, not just the one
        // clicked, so it cannot live inside the per-cell closure.
        if let Some(at) = solo_target {
            self.solo_only(at);
            response.muting_changed = true;
        }
        response
    }

    /// Exclusive solo of chip `at` (the lamp's right-click): make it the only
    /// soloed chip, or -- if it was already the sole solo -- clear solo entirely,
    /// so a second right-click brings the rest back. Soloing also un-mutes that
    /// chip, so soloing a muted chip makes it heard rather than leaving it
    /// silent.
    fn solo_only(&mut self, at: usize) {
        let was_sole_solo = self.entries[at].panel.soloed()
            && self
                .entries
                .iter()
                .filter(|entry| entry.panel.soloed())
                .count()
                == 1;
        for (index, entry) in self.entries.iter_mut().enumerate() {
            entry.panel.set_soloed(index == at && !was_sole_solo);
        }
        if !was_sole_solo {
            self.entries[at].panel.set_chip_muted(false);
        }
    }

    /// How many chip cells fit on one row of the well before it must wrap: the
    /// deck's width (less the fold icon's slot) divided by the widest cell. At
    /// least one, so a deck narrower than a single cell still shows it (clipped)
    /// rather than dividing by zero.
    fn columns_that_fit(&self, ui: &mut egui::Ui) -> usize {
        let font = egui::TextStyle::Button.resolve(ui.style());
        // The name is a `tabs::tab_button`, padded on each side; a small
        // over-estimate only wraps a touch early, which is the safe direction.
        let name_pad = 20.0;
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
                // lamp + knob + name, a gap between each.
                LAMP_SIZE + CELL_INNER_GAP + pan_knob::SIZE + CELL_INNER_GAP + name_cell
            })
            .fold(1.0_f32, f32::max);
        let avail = ui.available_width() - FOLD_ICON_ALLOWANCE;
        (((avail + CELL_GAP) / (widest + CELL_GAP)).floor() as usize).max(1)
    }

    /// Test-only: engage Custom mode with an OPL 18-slot pan image, spreading its
    /// melodic slots across the projection panels -- the peer of the old OPL
    /// panel's showcase helper, so the panning tests read the same way.
    #[cfg(test)]
    pub(crate) fn set_showcase_pans(&mut self, image: [u8; 18]) {
        let Some(opl_type) = self.opl_type else {
            return;
        };
        for (slot, &byte) in image.iter().enumerate() {
            if let Some((instance, channel)) = opl_slot(opl_type, slot)
                && let Some(entry) = self
                    .entries
                    .iter_mut()
                    .find(|entry| entry.panel.instance() == instance)
            {
                entry.panel.set_showcase_pan(channel, byte);
            }
        }
    }
}

/// The generic panels an OPL document projects to: one `Ym3812` for OPL2, one
/// `Ymf262` for OPL3, two `Ym3812` for dual OPL2 -- the same chips a VGM of the
/// same OPL set declares.
fn opl_entries(opl_type: OplType) -> Vec<ChipEntry> {
    let (kind, instances) = match opl_type {
        OplType::Opl2 => (ChipKind::Ym3812, 1u8),
        OplType::Opl3 => (ChipKind::Ymf262, 1),
        OplType::DualOpl2 => (ChipKind::Ym3812, 2),
    };
    (0..instances)
        .map(|instance| chip_entry(kind, instance, false))
        .collect()
}

/// One chip instance's tab and panel.
fn chip_entry(kind: ChipKind, instance: u8, variant: bool) -> ChipEntry {
    ChipEntry {
        label: instance_label(kind, variant, instance),
        panel: GenericChannelPanel::new(kind, instance, variant),
    }
}

/// The OPL slot `0..18` (bank * 9 + channel) an instance's channel maps to, or
/// `None` when the slot has no channel for this type (OPL2's high bank).
fn opl_slot(opl_type: OplType, slot: usize) -> Option<(u8, usize)> {
    match opl_type {
        OplType::Opl2 => (slot < 9).then_some((0, slot)),
        // One Ymf262: its eighteen melodic channels are the slots directly.
        OplType::Opl3 => Some((0, slot)),
        // Two Ym3812: the low bank is instance 0, the high bank instance 1.
        OplType::DualOpl2 => Some(if slot < 9 { (0, slot) } else { (1, slot - 9) }),
    }
}

/// The default 18-slot pan image for a type: centred, except dual OPL2's
/// authentic hard-L/R chip split.
fn default_opl_image(opl_type: OplType) -> [u8; 18] {
    match opl_type {
        OplType::DualOpl2 => dual_opl2_image(),
        _ => [PAN_CENTER; 18],
    }
}

/// The fixed hard-L/R panning image a dual-OPL2 song plays: chip 1 (low bank,
/// slots 0..9) hard left, chip 2 (high bank, slots 9..18) hard right -- the
/// authentic SB Pro 1 image.
fn dual_opl2_image() -> [u8; 18] {
    let mut pans = [PAN_LEFT; 18];
    pans[9..].fill(PAN_RIGHT);
    pans
}

/// The default OPL panning image a fresh `song` plays, for callers (a pack
/// preview) that want the song's own default rather than the editor's live mix.
#[must_use]
pub(crate) fn default_opl_panning(song: &DroSong) -> Panning {
    // A promoted DualOPL2 plays as one OPL3, so its default is centred, not the
    // dual-OPL2 hard-L/R image.
    match song.playback_opl_type() {
        OplType::DualOpl2 => Panning::Custom(dual_opl2_image()),
        _ => Panning::Original,
    }
}

/// Width kept clear beside the selector well for the fold icon, so the strip's
/// wrap never pushes the icon off the row.
const FOLD_ICON_ALLOWANCE: f32 = 24.0;

/// The deck's fold icon, right-aligned beside the strip: a CP437 triangle in a
/// clickable muted label (the app's disclosure idiom). Toggles `expanded`.
fn fold_icon(ui: &mut egui::Ui, palette: &Palette, expanded: &mut bool) {
    let (glyph, tip) = if *expanded {
        ("\u{25BC}", crate::strings::CHIP_DECK_TIP_HIDE)
    } else {
        ("\u{25BA}", crate::strings::CHIP_DECK_TIP_SHOW)
    };
    let icon = ui
        .add(
            egui::Label::new(egui::RichText::new(glyph).color(palette.muted))
                // Not selectable: selection would swallow the click.
                .selectable(false)
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    // A stable accessible name: the bare triangle glyph would collide with the
    // volume stepper's arrows in the accessibility tree.
    icon.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "chip deck fold")
    });
    if icon.clicked() {
        *expanded = !*expanded;
    }
}

/// What a chip cell's controls did this frame.
struct CellOutcome {
    /// The name was clicked, selecting this chip's detail panel.
    select: bool,
    /// The lamp was right-clicked, requesting an exclusive solo (applied by the
    /// caller, which alone can reach the sibling chips it clears).
    solo_clicked: bool,
}

/// A generic chip's cell: lamp, trim knob, name.
fn generic_cell(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    panel: &mut GenericChannelPanel,
    any_solo: bool,
    selected: bool,
    response: &mut ChannelsResponse,
) -> CellOutcome {
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
    // Solo is exclusive across chips, so the click is only recorded here; the
    // selector applies it once it can reach the other chips.
    let solo_clicked = lamp.secondary_clicked();

    // The trim knob.
    let mut trim = panel.trim();
    if pan_knob::show_trim(ui, palette, &mut trim, &format!("{name} level")).changed() {
        panel.set_trim(trim);
        response.trim_changed = true;
    }

    // The name, in the Editor/Pack tab chrome; clicking it selects the chip's
    // detailed panel below.
    let select = tabs::tab_button(ui, palette, name, selected).clicked();
    CellOutcome {
        select,
        solo_clicked,
    }
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

    /// A single-chip OPL2 song: one YM3812 tab, on the OPL rules (`None`).
    #[test]
    fn a_single_chip_song_has_one_opl_tab() {
        let panels = ChipPanels::for_song(&tone_song());
        assert_eq!(labels(&panels), ["YM3812"]);
        assert_eq!(panels.selected_chip(), None, "a DRO uses the OPL rules");
    }

    /// Dual OPL2 is two YM3812 tabs now -- a user mutes one board of the pair.
    /// (A v1 DualOPL2 like this one is trusted, never promoted.)
    #[test]
    fn dual_opl2_is_two_instance_tabs() {
        let mut song = tone_song();
        song.opl_type = OplType::DualOpl2;
        let panels = ChipPanels::for_song(&song);
        assert_eq!(labels(&panels), ["YM3812", "YM3812 #2"]);
    }

    /// A v2 DualOPL2 capture whose init block enables OPL3 shows one YMF262 tab
    /// -- its playback type -- so the deck's mute/pan keys agree with the mixer
    /// seams. Before the seam fix it showed two phantom YM3812 tabs and every
    /// mute/pan went inert.
    #[test]
    fn a_promoted_dualopl2_capture_shows_one_ymf262_tab() {
        // codemap slot 0 -> reg 0x05; a high-bank code writes 0x105 = 0x01.
        let data = vgms_core::DroDataV2::new(vec![0x80, 0x01], vec![0x05], 0xFE, 0xFF).unwrap();
        let song = DroSong::dro_v2("t.dro".to_owned(), data, 0, OplType::DualOpl2);
        let panels = ChipPanels::for_song(&song);
        assert_eq!(labels(&panels), ["YMF262"]);
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

    #[test]
    fn a_dro_info_edit_rebuilds_the_projection_tab() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.set_opl_type(OplType::Opl3);
        assert_eq!(labels(&panels), ["YMF262"]);
        assert_eq!(panels.selected_label(), Some("YMF262"));
    }

    /// The OPL muting bridge reflects a channel muted on the projection panel:
    /// muting YM3812 channel 0 gates low-bank `0xB0`.
    #[test]
    fn opl_muting_bridges_from_the_projection_panel() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.toggle_selected_channel(0); // mute FM 1
        let muting = panels.muting();
        assert!(
            !muting.is_channel_audible(vgms_core::Bank::Low, 0xB0),
            "channel 1 is muted"
        );
        assert!(muting.is_channel_audible(vgms_core::Bank::Low, 0xB1));
    }

    /// A dual-OPL2 song with no custom pans still plays the authentic hard-L/R
    /// image; a real VGM's OPL panning is neutral.
    #[test]
    fn dual_opl2_defaults_to_the_hard_lr_image() {
        let mut song = tone_song();
        song.opl_type = OplType::DualOpl2;
        let panels = ChipPanels::for_song(&song);
        let Panning::Custom(image) = panels.panning() else {
            panic!("dual OPL2 defaults to a custom hard-L/R image");
        };
        assert_eq!(&image[..9], &[PAN_LEFT; 9]);
        assert_eq!(&image[9..], &[PAN_RIGHT; 9]);

        assert_eq!(
            ChipPanels::for_song(&tone_song()).panning(),
            Panning::Original,
            "a plain OPL2 song keeps its own image"
        );
    }

    /// The chip lamp's right-click is an exclusive solo, and soloing a chip
    /// un-mutes it: the two behaviours behind [`ChipPanels::solo_only`].
    #[test]
    fn soloing_a_chip_is_exclusive_and_unmutes_it() {
        let file = vgm_for(&[
            (ChipKind::Sn76489, 3_579_545),
            (ChipKind::Ym2612, 7_670_454),
        ]);
        let mut panels = ChipPanels::for_vgm(&file);
        // Header order: SN76489 is entry 0, YM2612 entry 1.
        // Mute the SN76489, then solo it: soloing un-mutes it (#5) and makes it
        // the only soloed chip.
        panels.entries[0].panel.set_chip_muted(true);
        panels.solo_only(0);
        assert!(
            !panels.entries[0].panel.chip_muted(),
            "soloing a muted chip un-mutes it"
        );
        assert!(panels.entries[0].panel.soloed());
        assert!(!panels.entries[1].panel.soloed());

        // Solo the other chip: the first chip's solo clears (exclusive).
        panels.solo_only(1);
        assert!(!panels.entries[0].panel.soloed(), "solo is exclusive");
        assert!(panels.entries[1].panel.soloed());

        // Re-solo the sole soloed chip: the solo lifts entirely.
        panels.solo_only(1);
        assert!(
            !panels.any_solo(),
            "re-soloing the only soloed chip brings the rest back"
        );
    }

    /// A generic multichip file gathers per-instance mutes and the number keys
    /// act on the selected tab.
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
