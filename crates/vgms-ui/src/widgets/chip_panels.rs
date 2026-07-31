//! The per-chip control deck: a chip selector strip, and one panel per chip.
//!
//! A document declares its chips; each gets a tab in the strip, and the
//! selected tab draws its own controls. An OPL document (a DRO, or a VGM the
//! editor opens through its projection) shows the OPL
//! [`ChannelPanel`](super::channels::ChannelPanel) -- eighteen channels across
//! two banks, drum groups, the OPL stereo-ext image. Any other chip shows a
//! [`GenericChannelPanel`](super::chip_channels::GenericChannelPanel): a flat
//! mute/solo list from [`vgms_core::vgm::channels_of`], with pan knobs only
//! where the chip's core can pan.
//!
//! The strip is drawn **always** -- even for a single chip, even an empty
//! editor -- so the deck's shape does not jump as documents come and go, and a
//! one-chip file names its chip rather than leaving the controls unlabelled.
//! It is the same [`theme::tabs`](crate::theme::tabs) strip the Editor/Pack
//! views use.
//!
//! The OPL entry covers the whole OPL device, both banks together, rather than
//! splitting dual OPL2 into two entries: those eighteen channels are one
//! panel's worth of controls. A generic multichip file instead gets one entry
//! per chip *instance* -- a dual SN76489 is two tabs -- because a user mutes
//! one of the pair, not the kind.

use vgms_core::vgm::ChipKind;
use vgms_core::{OplType, Song, VgmFile};
use vgms_synth::{ChipMuting, ChipPanning, Muting, Panning};

use super::channels::{ChannelPanel, ChannelsResponse};
use super::chip_channels::GenericChannelPanel;
use crate::theme::{Palette, tabs};

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
pub struct ChipPanels {
    entries: Vec<ChipEntry>,
    selected: usize,
    /// The OPL panel. It outlives any particular document's chip list: the
    /// audio output speaks OPL muting and panning whenever an OPL document is
    /// loaded, and an empty editor still shows its channel toggles.
    opl: ChannelPanel,
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
        }
    }
}

impl ChipPanels {
    /// A deck with no document: the OPL panel, ready.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The deck for an OPL song: one OPL entry.
    #[must_use]
    pub fn for_song(song: &Song) -> Self {
        Self {
            entries: vec![ChipEntry {
                label: opl_label(song.opl_type).to_owned(),
                controls: ChipControls::Opl,
            }],
            selected: 0,
            opl: ChannelPanel::for_song(song),
        }
    }

    /// The deck for a generic multichip VGM: one entry per chip instance, in
    /// header order.
    #[must_use]
    pub fn for_vgm(file: &VgmFile) -> Self {
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
        }
    }

    /// Adopts a new chip type after a live DRO Info edit.
    pub fn set_opl_type(&mut self, opl_type: OplType, song: Option<&Song>) {
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
    #[must_use]
    pub fn opl(&mut self) -> &mut ChannelPanel {
        &mut self.opl
    }

    /// The OPL muting the OPL panel describes (for the OPL playback path).
    #[must_use]
    pub fn muting(&self) -> Muting {
        self.opl.muting()
    }

    /// The OPL panning the OPL panel describes.
    #[must_use]
    pub fn panning(&self) -> Panning {
        self.opl.panning()
    }

    /// The any-chip mutes every generic panel describes (for the generic
    /// playback path). Empty when the document is OPL.
    #[must_use]
    pub fn chip_muting(&self) -> ChipMuting {
        let mut muting = ChipMuting::new();
        for entry in &self.entries {
            if let ChipControls::Generic(panel) = &entry.controls {
                muting.set(panel.kind(), panel.instance(), panel.mask());
            }
        }
        muting
    }

    /// The any-chip pans every generic panel in Custom mode describes.
    #[must_use]
    pub fn chip_panning(&self) -> ChipPanning {
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
    pub fn selected_chip(&self) -> Option<ChipKind> {
        match self.entries.get(self.selected).map(|e| &e.controls) {
            Some(ChipControls::Generic(panel)) => Some(panel.kind()),
            _ => None,
        }
    }

    /// The label of the chip whose controls are on screen.
    #[must_use]
    pub fn selected_label(&self) -> Option<&str> {
        self.entries
            .get(self.selected)
            .map(|entry| entry.label.as_str())
    }

    /// Toggles channel `index` on the *selected* chip's panel -- so the number
    /// keys act on whatever tab is open, not always on the OPL one.
    pub fn toggle_selected_channel(&mut self, index: usize) {
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
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        pan_supported: impl Fn(Option<ChipKind>) -> bool,
        mute_supported: impl Fn(Option<ChipKind>) -> bool,
    ) -> ChannelsResponse {
        self.selector(ui, palette);
        let chip = self.selected_chip();
        let pan = pan_supported(chip);
        match self.entries.get_mut(self.selected).map(|e| &mut e.controls) {
            Some(ChipControls::Generic(panel)) => {
                panel.show(ui, palette, pan, mute_supported(chip))
            }
            // The OPL entry, or (defensively) an out-of-range selection.
            _ => self.opl.show(ui, palette, pan),
        }
    }

    fn selector(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let strip: Vec<tabs::Tab> = self
            .entries
            .iter()
            .map(|entry| tabs::Tab::new(entry.label.as_str()))
            .collect();
        ui.horizontal(|ui| {
            ui.label("Chip:");
            if let Some(clicked) = tabs::strip(ui, palette, &strip, self.selected) {
                self.selected = clicked;
            }
        });
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
