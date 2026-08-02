//! The Find Register dialog. Modeless.
//!
//! Two shapes, one per document kind. For an OPL song the choices are the delay
//! tokens, `BANK` where bank switches exist (DRO v1), then every register
//! `0x00`..`0xFF`. For a multichip VGM they are a chip picker and, per chip, its
//! documented registers plus "any write" and "any delay" -- and a free hex box
//! for an address the docs do not name.

use vgms_core::song::DRO_FILE_V1;
use vgms_core::vgm::{ChipKind, VgmFile, VgmFindTarget};
use vgms_core::{FindTarget, Song, SongFileType, chip_docs};

use crate::action::{Action, FindQuery};
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct FindRegDialog {
    mode: Mode,
}

#[derive(Debug)]
enum Mode {
    /// An OPL song: a flat list of tokens and 8-bit registers.
    Dro {
        choices: Vec<String>,
        selected: String,
    },
    /// A multichip VGM: a chip picker and its register choices.
    Vgm(VgmFind),
}

/// One chip a VGM declares, expanded per instance.
#[derive(Debug)]
struct ChipChoice {
    label: String,
    kind: ChipKind,
    instance: u8,
}

/// One register choice for the selected chip.
#[derive(Debug, Clone)]
struct RegChoice {
    label: String,
    what: RegWhat,
}

#[derive(Debug, Clone, Copy)]
enum RegWhat {
    AnyDelay,
    AnyWrite,
    Addr(u16),
}

#[derive(Debug)]
struct VgmFind {
    chips: Vec<ChipChoice>,
    chip: usize,
    /// The register choices for the selected chip, rebuilt when it changes.
    registers: Vec<RegChoice>,
    reg: usize,
    /// A free-form hex address, for a register the docs do not name. Non-empty
    /// wins over the dropdown.
    hex: String,
    /// The chip's address width in hex digits, so the box neither truncates a
    /// 16-bit address nor invites four digits for an 8-bit one.
    hex_digits: usize,
}

impl FindRegDialog {
    /// The dialog for an OPL song.
    #[must_use]
    pub fn new(song: &Song) -> Self {
        let is_v1 = song.file_type == SongFileType::Dro && song.file_version == DRO_FILE_V1;
        // The tokens come from vgms-core (shared with `FindTarget::from_str`), so
        // the dialog can't offer one the parser rejects. BANK is dropped for
        // anything but DRO v1, where no instruction could ever match it.
        let mut choices: Vec<String> = FindTarget::TOKENS
            .iter()
            .filter(|(_, target)| *target != FindTarget::BankSwitch || is_v1)
            .map(|(token, _)| (*token).to_owned())
            .collect();
        // Bare hex, matching the table's Reg. column; `FindTarget::from_str`
        // accepts it (an optional `0x` is stripped).
        choices.extend((0..=0xFFu16).map(|reg| format!("{reg:02X}")));
        Self {
            mode: Mode::Dro {
                choices,
                selected: String::new(),
            },
        }
    }

    /// The dialog for a multichip VGM: a chip picker and its registers.
    #[must_use]
    pub fn for_vgm(file: &VgmFile) -> Self {
        let mut chips = Vec::new();
        for chip in file.header.chips() {
            let instances = if chip.dual { 2 } else { 1 };
            for instance in 0..instances {
                let base = chip.label();
                chips.push(ChipChoice {
                    label: if instance == 0 {
                        base
                    } else {
                        format!("{base} #{}", instance + 1)
                    },
                    kind: chip.kind,
                    instance,
                });
            }
        }
        let mut find = VgmFind {
            chips,
            chip: 0,
            registers: Vec::new(),
            reg: 0,
            hex: String::new(),
            hex_digits: 2,
        };
        find.rebuild_registers();
        Self {
            mode: Mode::Vgm(find),
        }
    }

    /// Draws the window. Returns `false` once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        area: egui::Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        let mut close_clicked = false;
        let open = super::dialog_window(ctx, "Find Register", area, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(2.0);
            let query = match &mut self.mode {
                Mode::Dro { choices, selected } => Self::draw_dro(ui, palette, choices, selected),
                Mode::Vgm(find) => find.draw(ui, palette),
            };
            crate::theme::separator(ui, palette);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close_clicked = true;
                }
                if bevel::button(ui, palette, "Find Next").clicked()
                    && let Some(query) = query.clone()
                {
                    actions.push(Action::FindRegister {
                        query,
                        backwards: false,
                    });
                }
                if bevel::button(ui, palette, "Find Previous").clicked()
                    && let Some(query) = query.clone()
                {
                    actions.push(Action::FindRegister {
                        query,
                        backwards: true,
                    });
                }
            });
        });
        open && !close_clicked
    }

    /// The OPL body: one dropdown of tokens and registers. Returns the query
    /// the buttons would submit.
    fn draw_dro(
        ui: &mut egui::Ui,
        palette: &Palette,
        choices: &[String],
        selected: &mut String,
    ) -> Option<FindQuery> {
        ui.horizontal(|ui| {
            ui.label("Instruction:");
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                egui::ComboBox::from_id_salt("find-reg-choice")
                    .selected_text(selected.as_str())
                    .width(120.0)
                    .height(300.0)
                    .show_ui(ui, |ui| {
                        for choice in choices {
                            if ui.selectable_label(choice == selected, choice).clicked() {
                                *selected = choice.clone();
                            }
                        }
                    });
            });
        });
        (!selected.is_empty()).then(|| FindQuery::Dro(selected.clone()))
    }
}

impl VgmFind {
    /// Rebuilds the register choices from the selected chip: the always-there
    /// "any delay" and "any write", then each documented register by name.
    fn rebuild_registers(&mut self) {
        let mut registers = vec![
            RegChoice {
                label: "Any delay".to_owned(),
                what: RegWhat::AnyDelay,
            },
            RegChoice {
                label: "Any write".to_owned(),
                what: RegWhat::AnyWrite,
            },
        ];
        if let Some(chip) = self.chips.get(self.chip) {
            for (port, addr, name) in chip_docs::documented_registers(chip.kind) {
                // The port sits in the high byte of the address the search
                // matches, mirroring the stream decoder's addressing.
                let target = (u16::from(port) << 8) | addr;
                registers.push(RegChoice {
                    label: format!("{addr:#06X}  {name}"),
                    what: RegWhat::Addr(target),
                });
            }
            self.hex_digits = if chip_docs::address_width(chip.kind) > 8 {
                4
            } else {
                2
            };
        }
        self.registers = registers;
        self.reg = 0;
    }

    /// The VGM body: a chip dropdown, a register dropdown, and a free hex box.
    /// Returns the query the buttons would submit.
    fn draw(&mut self, ui: &mut egui::Ui, palette: &Palette) -> Option<FindQuery> {
        ui.horizontal(|ui| {
            ui.label("Chip:");
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                let selected = self
                    .chips
                    .get(self.chip)
                    .map_or("", |chip| chip.label.as_str());
                egui::ComboBox::from_id_salt("find-reg-chip")
                    .selected_text(selected)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for index in 0..self.chips.len() {
                            let label = self.chips[index].label.clone();
                            if ui.selectable_label(index == self.chip, label).clicked() {
                                self.chip = index;
                                self.rebuild_registers();
                            }
                        }
                    });
            });
        });
        ui.horizontal(|ui| {
            ui.label("Register:");
            ui.scope(|ui| {
                crate::theme::style_dropdown(ui, palette);
                let selected = self
                    .registers
                    .get(self.reg)
                    .map_or("", |choice| choice.label.as_str());
                egui::ComboBox::from_id_salt("find-reg-vgm-reg")
                    .selected_text(selected)
                    .width(200.0)
                    .height(300.0)
                    .show_ui(ui, |ui| {
                        for index in 0..self.registers.len() {
                            let label = self.registers[index].label.clone();
                            if ui.selectable_label(index == self.reg, label).clicked() {
                                self.reg = index;
                            }
                        }
                    });
            });
        });
        ui.horizontal(|ui| {
            ui.label("or address (hex):");
            // `+2` room for an optional `0x` prefix over the chip's own width.
            ui.add(
                egui::TextEdit::singleline(&mut self.hex)
                    .desired_width(60.0)
                    .char_limit(self.hex_digits + 2)
                    .hint_text("e.g. 28"),
            );
        });

        self.query()
    }

    /// The query the current selection describes: the hex box wins when it
    /// holds a valid address, else the register dropdown.
    fn query(&self) -> Option<FindQuery> {
        let chip = self.chips.get(self.chip)?;
        let kind = chip.kind;
        let instance = Some(chip.instance);

        let hex = self.hex.trim();
        if !hex.is_empty() {
            let digits = hex
                .strip_prefix("0x")
                .or_else(|| hex.strip_prefix("0X"))
                .unwrap_or(hex);
            let addr = u16::from_str_radix(digits, 16).ok()?;
            return Some(FindQuery::Vgm(VgmFindTarget::Write {
                kind,
                instance,
                addr: Some(addr),
            }));
        }

        let what = self.registers.get(self.reg)?.what;
        Some(FindQuery::Vgm(match what {
            RegWhat::AnyDelay => VgmFindTarget::AnyDelay,
            RegWhat::AnyWrite => VgmFindTarget::Write {
                kind,
                instance,
                addr: None,
            },
            RegWhat::Addr(addr) => VgmFindTarget::Write {
                kind,
                instance,
                addr: Some(addr),
            },
        }))
    }
}
