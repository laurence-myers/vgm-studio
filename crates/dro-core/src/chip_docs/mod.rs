//! Per-chip register documentation, and the analyser that turns it into the
//! instruction table's Description column.
//!
//! The multichip counterpart of [`regdata`](crate::regdata) +
//! [`RegisterAnalyzer`](crate::analysis::RegisterAnalyzer): a static table of
//! register names and bit-fields per [`ChipKind`], written from the chips'
//! own datasheets and long-public programming documentation (each submodule
//! cites its sources), and a replay cursor that reports which fields a write
//! actually changed.
//!
//! Coverage is deliberately partial: the corpus's common chips first, the
//! rest returning `None` so callers fall back to the generic one-liner
//! ([`VgmStream::describe`]). Adding a chip is adding a submodule and a match
//! arm -- nothing else consults an emulator, and nothing here may: the GPL
//! cores' source comments are off-limits to this crate.
//!
//! The OPL family's entries mirror [`regdata`](crate::regdata) rather than
//! replacing it -- the OPL editor keeps its own tables -- and a test pins the
//! two against each other so they cannot drift.

mod ay8910;
mod gb_dmg;
mod huc6280;
mod nes_apu;
mod opl;
mod opn;
mod segapcm;
mod sn76489;
mod ym2151;
mod ym2413;
mod ym2612;

use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::chip_state::Cell;
use crate::vgm::ChipKind;
use crate::vgm::stream::{ChipTarget, VgmCommand, VgmStream};

/// A named field within a register's value, most significant first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitField {
    pub description: &'static str,
    /// The value bits this field occupies. `u16` because a few chips take
    /// 16-bit data; 8-bit chips use the low byte.
    pub mask: u16,
}

/// One documented register: its name, and the fields of its value.
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterDoc {
    pub name: &'static str,
    pub fields: &'static [BitField],
}

pub(crate) const fn bf(description: &'static str, mask: u16) -> BitField {
    BitField { description, mask }
}

/// The documentation for a write to `(chip, port, addr)`, or `None` when the
/// chip or the address is undocumented.
///
/// `port` and `addr` are exactly what [`VgmCommand::Write`] carries -- the
/// same addressing [`decode`](crate::vgm::stream::decode) produces.
#[must_use]
pub fn register_doc(chip: ChipKind, port: u8, addr: u16) -> Option<&'static RegisterDoc> {
    use ChipKind as K;
    match chip {
        K::Ym3812 | K::Ym3526 | K::Y8950 | K::Ymf262 => opl::doc(chip, port, addr),
        K::Ym2612 => ym2612::doc(port, addr),
        K::Ym2413 => ym2413::doc(port, addr),
        K::Ym2151 => ym2151::doc(port, addr),
        K::Ym2203 | K::Ym2608 | K::Ym2610 => opn::doc(chip, port, addr),
        K::Ay8910 => ay8910::doc(port, addr),
        K::GameBoyDmg => gb_dmg::doc(port, addr),
        K::NesApu => nes_apu::doc(port, addr),
        K::HuC6280 => huc6280::doc(port, addr),
        K::SegaPcm => segapcm::doc(port, addr),
        // The SN76489 has no address space -- the register travels in the
        // data byte -- so it is decoded by the analyser, not looked up here.
        _ => None,
    }
}

/// The registers worth offering in a find dropdown: `(port, addr, name)`.
///
/// A curated list of the registers someone hunts for (key-ons, DAC ports,
/// mode switches), not an enumeration of every documented address -- ranges
/// like "operator 1..22" are served by free hex entry instead. Empty for
/// undocumented chips and for the SN76489, whose writes have no address to
/// find; the dialog offers "any write" there.
#[must_use]
pub fn documented_registers(chip: ChipKind) -> &'static [(u8, u16, &'static str)] {
    use ChipKind as K;
    match chip {
        K::Ym3812 | K::Ym3526 | K::Y8950 | K::Ymf262 => opl::NOTABLE,
        K::Ym2612 => ym2612::NOTABLE,
        K::Ym2413 => ym2413::NOTABLE,
        K::Ym2151 => ym2151::NOTABLE,
        K::Ym2203 => opn::NOTABLE_2203,
        K::Ym2608 | K::Ym2610 => opn::NOTABLE_2608,
        K::Ay8910 => ay8910::NOTABLE,
        K::GameBoyDmg => gb_dmg::NOTABLE,
        K::NesApu => nes_apu::NOTABLE,
        K::HuC6280 => huc6280::NOTABLE,
        K::SegaPcm => segapcm::NOTABLE,
        _ => &[],
    }
}

/// How wide the chip's register addresses are, in bits: what a free hex
/// field should accept. Follows the addressing the stream decoder produces,
/// not the chip's pinout.
#[must_use]
pub const fn address_width(chip: ChipKind) -> u8 {
    use ChipKind as K;
    match chip {
        K::SegaPcm
        | K::Rf5c68
        | K::Rf5c164
        | K::MultiPcm
        | K::QSound
        | K::Scsp
        | K::WonderSwan
        | K::Vsu
        | K::X1010
        | K::C352 => 16,
        _ => 8,
    }
}

/// A replay cursor over a multichip stream's writes.
///
/// The peer of [`RegisterAnalyzer`](crate::analysis::RegisterAnalyzer), with
/// the same contract: build one per document, [`reset`](Self::reset) after
/// any edit, query rows in ascending order for `O(1)` amortised painting (a
/// lower index replays from the start).
#[derive(Clone, Default)]
pub struct ChipAnalyzer {
    /// Commands `[0, applied)` have been replayed into the state.
    applied: usize,
    /// The last value written to each documented cell.
    state: BTreeMap<Cell, u16>,
    /// The SN76489's latched register per (kind, instance): which register
    /// the next plain data byte extends.
    latches: BTreeMap<(ChipKind, u8), u8>,
}

impl ChipAnalyzer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards the replayed state and returns to the start of the stream.
    pub fn reset(&mut self) {
        self.applied = 0;
        self.state.clear();
        self.latches.clear();
    }

    /// The Description cell for command `index`, or `None` when this analyser
    /// has nothing better than the generic one-liner -- an undocumented chip,
    /// or a command that is not a register write.
    ///
    /// `None` still advances the replayed state correctly; the caller's
    /// fallback is about wording, not about skipping the command.
    pub fn row(&mut self, stream: &VgmStream, index: usize) -> Option<Cow<'static, str>> {
        if index >= stream.len() {
            return None;
        }
        if self.applied > index {
            self.reset();
        }
        while self.applied < index {
            let _ = self.step(stream, self.applied);
            self.applied += 1;
        }
        let description = self.step(stream, index);
        self.applied += 1;
        description
    }

    /// Applies command `index` to the cursor and describes it, or applies it
    /// silently when there is nothing documented to say.
    fn step(&mut self, stream: &VgmStream, index: usize) -> Option<Cow<'static, str>> {
        let VgmCommand::Write { target, addr, data } = stream.get(index)? else {
            return None;
        };
        if target.kind == ChipKind::Sn76489 {
            return self.describe_sn76489(target, addr, data);
        }
        let doc = register_doc(target.kind, target.port, addr)?;
        let cell = Cell {
            chip: target.kind,
            instance: target.instance,
            port: target.port,
            addr,
        };
        let previous = self.state.insert(cell, data);
        Some(describe_changes(doc, previous, data))
    }

    /// The SN76489's writes carry the register in the data byte: bit 7 set
    /// latches a register (and writes its low nibble), bit 7 clear extends
    /// whatever was latched. Port 1 is the Game Gear stereo mask.
    fn describe_sn76489(
        &mut self,
        target: ChipTarget,
        addr: u16,
        data: u16,
    ) -> Option<Cow<'static, str>> {
        if target.port == 1 || addr == 1 {
            return Some(Cow::Borrowed(sn76489::GG_STEREO));
        }
        let value = data as u8;
        let key = (target.kind, target.instance);
        if value & 0x80 != 0 {
            let register = (value >> 4) & 0x07;
            self.latches.insert(key, register);
            Some(Cow::Borrowed(sn76489::latch_description(register)))
        } else {
            let register = self.latches.get(&key).copied();
            Some(Cow::Borrowed(sn76489::data_description(register)))
        }
    }
}

impl std::fmt::Debug for ChipAnalyzer {
    /// The state maps are noise; summarise the cursor position instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChipAnalyzer")
            .field("applied", &self.applied)
            .finish_non_exhaustive()
    }
}

/// The Description wording both analysers share: the changed fields joined
/// with `" / "`, `"(no changes)"` for a value the register already held, and
/// every field on the first write.
fn describe_changes(
    doc: &'static RegisterDoc,
    previous: Option<u16>,
    value: u16,
) -> Cow<'static, str> {
    let changed = |mask: u16| match previous {
        None => true,
        Some(old) => (old ^ value) & mask != 0,
    };
    let mut count = 0usize;
    let mut only = "";
    for field in doc.fields {
        if changed(field.mask) {
            count += 1;
            only = field.description;
        }
    }
    match count {
        0 => Cow::Borrowed("(no changes)"),
        1 => Cow::Borrowed(only),
        _ => Cow::Owned(
            doc.fields
                .iter()
                .filter(|field| changed(field.mask))
                .map(|field| field.description)
                .collect::<Vec<_>>()
                .join(" / "),
        ),
    }
}

#[cfg(test)]
mod tests;
