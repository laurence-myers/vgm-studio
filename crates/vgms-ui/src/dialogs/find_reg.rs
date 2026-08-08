//! The Find Register dialog. Modeless.
//!
//! One chip/register picker for either document kind. A DRO offers its single
//! OPL chip; a multichip VGM offers each chip its header declares. Per chip the
//! register dropdown lists "any delay", "any write" and the chip's documented
//! registers by name, with a free hex box for an address the docs do not name.
//! What a selection becomes differs only at the very end: a DRO builds a
//! [`FindTarget`], a VGM a [`VgmFindTarget`] (see [`Emit`]).

use vgms_core::vgm::{ChipKind, VgmFile, VgmFindTarget};
use vgms_core::{DroSong, FindTarget, OplType, chip_docs};

use crate::action::{Action, EditAction, FindQuery};
use crate::theme::{Palette, bevel};

#[derive(Debug)]
pub struct FindRegDialog {
    find: Find,
}

/// Which document the dialog searches -- the one thing that differs between a
/// DRO and a VGM, deciding how the shared chip/register selection becomes a
/// query.
#[derive(Debug, Clone, Copy)]
enum Emit {
    /// A DRO's OPL instruction stream: the selection becomes a [`FindTarget`].
    Dro,
    /// A multichip VGM stream: the selection becomes a [`VgmFindTarget`].
    Vgm,
}

/// One chip the document declares, expanded per instance.
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

/// What the current selection picks, before it becomes a per-document query:
/// the hex box when it holds a valid address, else the register dropdown.
#[derive(Debug, Clone, Copy)]
enum Selected {
    AnyDelay,
    AnyWrite,
    Addr(u16),
}

/// The chip/register picker itself, shared by both document kinds. Only
/// [`Self::query`] cares which one it is drawing for.
#[derive(Debug)]
struct Find {
    emit: Emit,
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
    /// The dialog for a DRO: its single OPL chip.
    #[must_use]
    pub fn new(song: &DroSong) -> Self {
        let kind = opl_find_kind(song.opl_type);
        let chips = vec![ChipChoice {
            label: kind.name().to_owned(),
            kind,
            instance: 0,
        }];
        Self {
            find: Find::new(Emit::Dro, chips),
        }
    }

    /// The dialog for a multichip VGM: one entry per chip instance.
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
        Self {
            find: Find::new(Emit::Vgm, chips),
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
        let open = super::dialog_window(ctx, palette, "Find Register", area, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(2.0);
            let query = self.find.draw(ui, palette);
            crate::theme::separator(ui, palette);
            super::dialog_footer(ui, |ui| {
                if bevel::button(ui, palette, "Close").clicked() {
                    close_clicked = true;
                }
                if bevel::button(ui, palette, "Find Next").clicked()
                    && let Some(query) = query
                {
                    actions.push(Action::Edit(EditAction::FindRegister {
                        query,
                        backwards: false,
                    }));
                }
                if bevel::button(ui, palette, "Find Previous").clicked()
                    && let Some(query) = query
                {
                    actions.push(Action::Edit(EditAction::FindRegister {
                        query,
                        backwards: true,
                    }));
                }
            });
        });
        open && !close_clicked
    }
}

impl Find {
    fn new(emit: Emit, chips: Vec<ChipChoice>) -> Self {
        let mut find = Self {
            emit,
            chips,
            chip: 0,
            registers: Vec::new(),
            reg: 0,
            hex: String::new(),
            hex_digits: 2,
        };
        find.rebuild_registers();
        find
    }

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

    /// The body: a chip dropdown, a register dropdown, and a free hex box.
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

    /// What the current selection picks: the hex box when it holds a valid
    /// address, else the register dropdown.
    fn selected(&self) -> Option<Selected> {
        let hex = self.hex.trim();
        if !hex.is_empty() {
            let digits = hex
                .strip_prefix("0x")
                .or_else(|| hex.strip_prefix("0X"))
                .unwrap_or(hex);
            let addr = u16::from_str_radix(digits, 16).ok()?;
            return Some(Selected::Addr(addr));
        }
        Some(match self.registers.get(self.reg)?.what {
            RegWhat::AnyDelay => Selected::AnyDelay,
            RegWhat::AnyWrite => Selected::AnyWrite,
            RegWhat::Addr(addr) => Selected::Addr(addr),
        })
    }

    /// The query the current selection describes, in the document's own
    /// vocabulary.
    fn query(&self) -> Option<FindQuery> {
        let chip = self.chips.get(self.chip)?;
        let selected = self.selected()?;
        Some(match self.emit {
            Emit::Vgm => FindQuery::Vgm(match selected {
                Selected::AnyDelay => VgmFindTarget::AnyDelay,
                Selected::AnyWrite => VgmFindTarget::Write {
                    kind: chip.kind,
                    instance: Some(chip.instance),
                    addr: None,
                },
                Selected::Addr(addr) => VgmFindTarget::Write {
                    kind: chip.kind,
                    instance: Some(chip.instance),
                    addr: Some(addr),
                },
            }),
            Emit::Dro => FindQuery::Dro(match selected {
                Selected::AnyDelay => FindTarget::AnyDelay,
                Selected::AnyWrite => FindTarget::AnyRegister,
                // A DRO addresses registers by a single byte; the port/bank an
                // OPL3 register carries in the high byte is not part of the DRO
                // register code, so the search matches the byte in either bank.
                Selected::Addr(addr) => FindTarget::Register((addr & 0xFF) as u8),
            }),
        })
    }
}

/// The chip a DRO's OPL type projects to, for the find dialog's one-chip
/// picker. A dual OPL2 is two `Ym3812` instances at playback, but its register
/// codes are identical on either, so the picker offers just the one chip.
///
/// The single source of truth for the DRO-to-chip projection, shared with the
/// Settings and render/split pickers.
fn opl_find_kind(opl_type: OplType) -> ChipKind {
    vgms_synth::opl_projection_kind(opl_type)
}
