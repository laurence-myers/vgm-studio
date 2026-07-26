//! The per-chip control deck: a chip selector, and one panel per chip.
//!
//! [`ChannelPanel`] is an OPL panel -- eighteen channels across two banks, two
//! drum groups, an OPL-shaped pan image. That is right for an OPL song and
//! meaningless for anything else, so it does not belong directly under the
//! transport where it can only ever describe one chip.
//!
//! This wraps it. A document declares its chips; each gets an entry, and the
//! selected entry draws its own controls. A song with one chip -- every DRO, and
//! every VGM the editor opens today -- shows no selector at all, so the deck
//! looks exactly as it always has. Only a file with more than one chip pays for
//! the strip.
//!
//! The OPL entry covers the whole OPL device, both banks together, rather than
//! splitting dual OPL2 into two entries: those eighteen channels are one panel's
//! worth of controls and muting channel 12 should not cost a chip switch.
//! Chips with no controls yet say so, which is a better answer than an absent
//! panel with no explanation.

use dro_core::{OplType, Song, VgmFile};
use dro_synth::{Muting, Panning};

use crate::theme::{Palette, bevel};
use crate::widgets::channels::{ChannelPanel, ChannelsResponse};

/// What a chip contributes to the deck.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChipControls {
    /// The OPL mute/pan panel.
    Opl,
    /// A chip this app has no controls for. Named so the deck can say which.
    None,
}

/// One chip in the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// audio output speaks OPL muting and panning whatever else is loaded.
    opl: ChannelPanel,
}

impl Default for ChipPanels {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            opl: ChannelPanel::new(),
        }
    }
}

impl ChipPanels {
    /// A deck with no document: no chips, and a fresh OPL panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The deck for an OPL song: one entry, so no selector is drawn.
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

    /// The deck for a VGM the editor cannot decode: one entry per chip the file
    /// declares, in header order, none of them with controls yet.
    #[must_use]
    pub fn for_vgm(file: &VgmFile) -> Self {
        Self {
            entries: file
                .header
                .chips()
                .iter()
                .map(|chip| ChipEntry {
                    label: chip.label(),
                    controls: ChipControls::None,
                })
                .collect(),
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
            .find(|entry| entry.controls == ChipControls::Opl)
        {
            entry.label = opl_label(opl_type).to_owned();
        }
    }

    /// The OPL panel, for the keyboard shortcuts and the tests that drive it.
    #[must_use]
    pub fn opl(&mut self) -> &mut ChannelPanel {
        &mut self.opl
    }

    /// The muting the OPL panel describes.
    #[must_use]
    pub fn muting(&self) -> Muting {
        self.opl.muting()
    }

    /// The panning the OPL panel describes.
    #[must_use]
    pub fn panning(&self) -> Panning {
        self.opl.panning()
    }

    /// Whether a chip selector is drawn -- true only with more than one chip.
    #[must_use]
    pub fn has_selector(&self) -> bool {
        self.entries.len() > 1
    }

    /// The label of the chip whose controls are on screen.
    #[must_use]
    pub fn selected_label(&self) -> Option<&str> {
        self.entries
            .get(self.selected)
            .map(|entry| entry.label.as_str())
    }

    /// Draws the selector (when there is more than one chip) and the selected
    /// chip's controls.
    ///
    /// `panning_supported` is false when the output cannot pan -- hardware
    /// playback mixes on the chip -- which greys the pan controls rather than
    /// leaving knobs that turn but do nothing.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        panning_supported: bool,
    ) -> ChannelsResponse {
        if self.has_selector() {
            self.selector(ui, palette);
        }
        match self.entries.get(self.selected).map(|entry| &entry.controls) {
            // With no document at all, the OPL panel still draws: an empty
            // editor has always shown its channel toggles.
            Some(ChipControls::Opl) | None => self.opl.show(ui, palette, panning_supported),
            Some(ChipControls::None) => {
                let label = self.selected_label().unwrap_or("This chip");
                ui.label(format!("No controls for {label} yet."));
                ChannelsResponse::default()
            }
        }
    }

    fn selector(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label("Chip:");
            for (index, entry) in self.entries.iter().enumerate() {
                let mut selected = index == self.selected;
                if bevel::toggle(ui, palette, &mut selected, &entry.label).clicked() {
                    self.selected = index;
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_song::tone_song;

    fn foreign(chips: &[(dro_core::ChipKind, u32)]) -> VgmFile {
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
        dro_core::vgm::file::read("x.vgm", &bytes).unwrap()
    }

    /// The shipping case: one chip, so the deck is exactly the panel it always
    /// was, with nothing extra drawn above it.
    #[test]
    fn a_single_chip_song_shows_no_selector() {
        let panels = ChipPanels::for_song(&tone_song());
        assert!(!panels.has_selector());
        assert_eq!(panels.selected_label(), Some("YM3812"));
    }

    /// Dual OPL2 is two chips, but one panel's worth of controls: eighteen
    /// channels the user should not have to switch chips to reach.
    #[test]
    fn dual_opl2_is_one_entry_covering_both_banks() {
        let mut song = tone_song();
        song.opl_type = OplType::DualOpl2;
        let panels = ChipPanels::for_song(&song);
        assert!(!panels.has_selector());
        assert_eq!(panels.selected_label(), Some("YM3812 x2"));
    }

    #[test]
    fn a_multi_chip_file_gets_one_entry_per_chip_in_header_order() {
        use dro_core::ChipKind;
        let file = foreign(&[
            (ChipKind::Ym2612, 7_670_454),
            (ChipKind::Sn76489, 3_579_545),
        ]);
        let panels = ChipPanels::for_vgm(&file);
        assert!(panels.has_selector());
        assert_eq!(panels.selected_label(), Some("SN76489"), "header order");
        assert_eq!(
            panels
                .entries
                .iter()
                .map(|e| e.label.as_str())
                .collect::<Vec<_>>(),
            ["SN76489", "YM2612"]
        );
        assert!(
            panels
                .entries
                .iter()
                .all(|e| e.controls == ChipControls::None),
            "no controls for either chip yet"
        );
    }

    /// A file declaring one foreign chip is still a single-chip document, so it
    /// gets no selector either -- there is nothing to select between.
    #[test]
    fn a_single_foreign_chip_shows_no_selector_either() {
        let file = foreign(&[(dro_core::ChipKind::Ym2612, 7_670_454)]);
        let panels = ChipPanels::for_vgm(&file);
        assert!(!panels.has_selector());
        assert_eq!(panels.selected_label(), Some("YM2612"));
    }

    #[test]
    fn a_dro_info_edit_renames_the_opl_entry() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.set_opl_type(OplType::Opl3, None);
        assert_eq!(panels.selected_label(), Some("YMF262"));
    }

    #[test]
    fn muting_and_panning_come_from_the_opl_panel() {
        let mut panels = ChipPanels::for_song(&tone_song());
        panels.opl().toggle_channel(3);
        assert_eq!(panels.muting(), panels.opl.muting());
        assert_eq!(panels.panning(), panels.opl.panning());
    }
}
