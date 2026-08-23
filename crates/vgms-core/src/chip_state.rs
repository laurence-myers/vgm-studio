//! What a chip has been told, at a point in a VGM stream.
//!
//! Cutting a VGM anywhere but the start means the music from that point was
//! written against a chip already configured by writes in the discarded span.
//! The fix: fold the discarded span into a *state* and re-emit it as writes at
//! the new beginning -- for every chip at once, as `vgm_trim` does.
//!
//! A restore never synthesises a write; it re-emits the **original command
//! bytes** of the last write to each cell, so every encoding stays exact
//! (dual-chip opcodes, 16-bit addressing) without this module knowing how to
//! spell a write for forty-two chips.
//!
//! Restores are emitted in the order the writes occurred, not address order,
//! because some registers' meaning depends on a mode set earlier (OPL's `NEW`
//! bit, banking, envelope modes); replaying causal order keeps those intact.
//!
//! Data blocks loaded before the cut come back too, verbatim and in order,
//! before anything else: banks are cumulative, so a later DAC seek indexes the
//! concatenation of every block loaded.

use std::collections::BTreeMap;

use crate::vgm::stream::{VgmCommand, VgmStream};

/// One addressable cell of chip state: a register on a particular chip.
///
/// Ordered by chip, then instance, then port, then address -- which is not the
/// emission order (see the module docs) but makes the map deterministic and the
/// diffing in [`ChipState::changes_from`] straightforward.
///
/// `pub` because [`chip_docs`](crate::chip_docs) keys its own replay state the
/// same way -- one definition of "a register on a particular chip", not two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cell {
    pub chip: crate::vgm::ChipKind,
    pub instance: u8,
    pub port: u8,
    pub addr: u16,
}

/// What a cell was last told, and where that instruction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Latch {
    value: u16,
    /// The command index the write came from, so the restore can re-emit its
    /// bytes and sort by when it happened.
    index: usize,
}

/// Whether `chip` is an RF5C68/RF5C164, whose port-0 register file is
/// channel-indirected and cannot be modelled by the flat latch map -- see
/// [`RfChannels`].
fn is_rf5c(chip: crate::vgm::ChipKind) -> bool {
    use crate::vgm::ChipKind as K;
    matches!(chip, K::Rf5c68 | K::Rf5c164)
}

/// RF5C register `0x07` bit 6: set selects a channel (`data & 0x07`), clear sets
/// the RAM bank. Bit 7 is the chip's sound-enable, carried in the value either
/// way.
const RF5C_SELECT_BIT: u16 = 0x40;

/// The channel-indirected port-0 register file of one RF5C68/RF5C164 instance.
///
/// Registers `0x00`-`0x06` (envelope, pan, frequency, loop, start address)
/// address whichever of the eight channels register `0x07` (bit 6 set) last
/// *selected*. The flat [`Cell`] map keys only by register, so it keeps a single
/// channel's start address and lets the other seven fall back to their reset
/// default of zero -- silence. Cutting a stream mid-song then reconstructs only
/// the last-selected channel, which is why a seek into an RF5C164 track plays
/// its hi-hat (the channel selected last) but drops the kick and snare until the
/// loop restates them.
///
/// This keeps the last write to each `(channel, register)` pair, a select
/// command to re-reach each channel, and the chip-wide bank, current channel and
/// key-on mask. [`restore`](Self::restore) re-emits, per channel, a select then
/// that channel's params, and the key-on `0x08` last of all -- so every
/// channel's start address is in place before the key-on reloads its play cursor
/// from it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RfChannels {
    /// The channel `0x00`-`0x06` writes currently address, tracked as `0x07`
    /// selects go by.
    cur_chan: u8,
    /// The last write to each `(channel, register 0x00..=0x06)`.
    params: BTreeMap<(u8, u8), Latch>,
    /// A command that selects each channel (`0x07` bit 6 set), re-emitted before
    /// that channel's params. Channel 0 may have none: a reset leaves the current
    /// channel at 0 and channel 0 is restored first, so its params land without
    /// one.
    selects: BTreeMap<u8, usize>,
    /// The last `0x07` with bit 6 clear -- the RAM bank and the enable bit.
    bank: Option<Latch>,
    /// The last `0x07` with bit 6 set -- the final current channel and enable.
    select_ctrl: Option<Latch>,
    /// The last `0x08` key-on mask.
    key_on: Option<Latch>,
}

impl RfChannels {
    /// Folds one port-0 register write. `reg` is `0x00`-`0x08`.
    fn apply(&mut self, reg: u8, data: u16, index: usize) {
        match reg {
            0x07 => {
                if data & RF5C_SELECT_BIT != 0 {
                    self.cur_chan = (data & 0x07) as u8;
                    self.selects.insert(self.cur_chan, index);
                    self.select_ctrl = Some(Latch { value: data, index });
                } else {
                    self.bank = Some(Latch { value: data, index });
                }
            }
            0x08 => self.key_on = Some(Latch { value: data, index }),
            // 0x00..=0x06, the per-channel registers.
            _ => {
                self.params
                    .insert((self.cur_chan, reg), Latch { value: data, index });
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.params.is_empty()
            && self.bank.is_none()
            && self.select_ctrl.is_none()
            && self.key_on.is_none()
    }

    /// The command indices that reconstruct this register file from a reset chip.
    ///
    /// Per channel, in order: a select (so the params that follow land on it),
    /// then that channel's `0x00`-`0x06`. Then the chip-wide bank and final
    /// select, in the order they occurred so the later's enable bit wins, and the
    /// key-on mask last -- after every start address is set, so keying a channel
    /// on reloads it from the right place.
    fn restore(&self) -> Vec<usize> {
        let mut out = Vec::new();
        for chan in 0u8..8 {
            let params: Vec<usize> = (0u8..=0x06)
                .filter_map(|reg| self.params.get(&(chan, reg)).map(|latch| latch.index))
                .collect();
            if params.is_empty() {
                continue;
            }
            if let Some(&select) = self.selects.get(&chan) {
                out.push(select);
            }
            out.extend(params);
        }
        let mut ctrl: Vec<usize> = [self.bank, self.select_ctrl]
            .into_iter()
            .flatten()
            .map(|latch| latch.index)
            .collect();
        ctrl.sort_unstable();
        out.extend(ctrl);
        if let Some(latch) = self.key_on {
            out.push(latch.index);
        }
        out
    }
}

/// The chips' state after some span of a stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChipState {
    latches: BTreeMap<Cell, Latch>,
    /// Command indices of the `0x67` data blocks seen, in order. Banks are
    /// cumulative, so this is a list rather than a map: every block loaded
    /// before a cut is part of the state at that cut.
    blocks: Vec<usize>,
    /// The last `0x90`-`0x95` command seen per stream id, in the order the
    /// commands arrived. A DAC stream's setup, its data binding and its
    /// frequency are separate commands, so each is kept.
    dac_stream: BTreeMap<(u8, u8), usize>,
    /// The last `0xE0` seek, if the PCM bank position was moved.
    seek: Option<usize>,
    /// The channel-indirected RF5C68/RF5C164 register files, keyed by chip and
    /// instance. Their port-0 registers cannot ride in [`latches`](Self::latches)
    /// -- see [`RfChannels`]. Port-1 (RAM) writes still do, one cell per address.
    rf: BTreeMap<(crate::vgm::ChipKind, u8), RfChannels>,
}

impl ChipState {
    /// Folds `stream[..upto]` into the state it leaves the chips in.
    #[must_use]
    pub fn fold(stream: &VgmStream, upto: usize) -> Self {
        Self::fold_range(stream, 0, upto)
    }

    /// Folds `stream[from..to]`, for the state a span *changes* rather than the
    /// state it leaves.
    #[must_use]
    pub fn fold_range(stream: &VgmStream, from: usize, to: usize) -> Self {
        let mut state = Self::default();
        for index in from..to.min(stream.len()) {
            state.apply(stream, index);
        }
        state
    }

    fn apply(&mut self, stream: &VgmStream, index: usize) {
        let Some(command) = stream.get(index) else {
            return;
        };
        match command {
            // The RF5C's port-0 register file is channel-indirected, so it goes
            // to its own per-instance state; its port-1 (RAM) writes are ordinary
            // per-address cells and fall through to the flat map.
            VgmCommand::Write { target, addr, data }
                if is_rf5c(target.kind) && target.port == 0 && addr <= 0x08 =>
            {
                self.rf
                    .entry((target.kind, target.instance))
                    .or_default()
                    .apply(addr as u8, data, index);
            }
            VgmCommand::Write { target, addr, data } => {
                self.latches.insert(
                    Cell {
                        chip: target.kind,
                        instance: target.instance,
                        port: target.port,
                        addr,
                    },
                    Latch { value: data, index },
                );
            }
            VgmCommand::DataBlock { .. } => self.blocks.push(index),
            VgmCommand::DacStream { opcode, stream_id } => {
                self.dac_stream.insert((stream_id, opcode), index);
            }
            VgmCommand::SeekPcm(_) => self.seek = Some(index),
            // A wait moves the clock, not the chips. A PCM RAM write and an
            // unrecognised command are not modelled: neither can be replayed
            // from a latch, and pretending otherwise would be worse than
            // leaving them out. See `unmodelled_commands`.
            VgmCommand::Wait(_)
            | VgmCommand::DacWrite { .. }
            | VgmCommand::OverrideWait { .. }
            | VgmCommand::PcmRamWrite { .. }
            | VgmCommand::Raw { .. } => {}
        }
    }

    /// How many cells are latched. Mostly for tests and diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.latches.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.latches.is_empty()
            && self.blocks.is_empty()
            && self.rf.values().all(RfChannels::is_empty)
    }

    /// The command indices whose bytes reproduce this state from silence.
    ///
    /// In emission order: the data blocks first (cumulative, in the order they
    /// were loaded), then every latched cell's last write in the order those
    /// writes happened, then the DAC-stream setup and the PCM seek.
    #[must_use]
    pub fn restore_indices(&self) -> Vec<usize> {
        let mut out = self.blocks.clone();

        let mut writes: Vec<usize> = self.latches.values().map(|latch| latch.index).collect();
        writes.sort_unstable();
        out.extend(writes);

        let mut streams: Vec<usize> = self.dac_stream.values().copied().collect();
        streams.sort_unstable();
        out.extend(streams);

        out.extend(self.seek);

        // The RF5C register files last: their key-on has to follow every start
        // address, including the port-1 RAM writes above, so a keyed channel
        // reloads from data that is already in place.
        for rf in self.rf.values() {
            out.extend(rf.restore());
        }
        out
    }

    /// The command indices whose bytes carry the chips from `earlier` to this
    /// state -- the writes that actually differ.
    ///
    /// What a cut *out of the middle* of a stream needs: everything the removed
    /// span changed, and nothing it merely repeated.
    #[must_use]
    pub fn changes_from(&self, earlier: &Self) -> Vec<usize> {
        // Blocks are cumulative, so any loaded in the removed span are still
        // needed by what follows: take the ones `earlier` had not yet seen.
        let mut out: Vec<usize> = self
            .blocks
            .iter()
            .copied()
            .filter(|index| !earlier.blocks.contains(index))
            .collect();

        let mut writes: Vec<usize> = self
            .latches
            .iter()
            .filter(|(cell, latch)| {
                earlier
                    .latches
                    .get(cell)
                    .is_none_or(|before| before.value != latch.value)
            })
            .map(|(_, latch)| latch.index)
            .collect();
        writes.sort_unstable();
        out.extend(writes);

        let mut streams: Vec<usize> = self
            .dac_stream
            .iter()
            .filter(|(key, index)| earlier.dac_stream.get(*key) != Some(*index))
            .map(|(_, index)| *index)
            .collect();
        streams.sort_unstable();
        out.extend(streams);

        if self.seek != earlier.seek {
            out.extend(self.seek);
        }

        // An RF5C register file whose state the removed span changed is carried
        // in full: its channels are interdependent (a select reaches the params
        // that follow it), so re-emitting the whole file is both correct and, on
        // a chip written this densely, no larger than a real diff would be.
        for (key, rf) in &self.rf {
            if earlier.rf.get(key) != Some(rf) {
                out.extend(rf.restore());
            }
        }
        out
    }

    /// The bytes of the commands at `indices`, concatenated.
    ///
    /// The restore itself: each command's own bytes, copied from the stream
    /// they came from, so every encoding is exactly what the file used.
    #[must_use]
    pub fn bytes_for(stream: &VgmStream, indices: &[usize]) -> Vec<u8> {
        let mut out = Vec::new();
        for &index in indices {
            if let Some(bytes) = stream.raw_command(index) {
                out.extend_from_slice(bytes);
            }
        }
        out
    }
}

/// Whether writing `addr` again with the value it already holds is inaudible
/// on `chip`.
///
/// `None` means this app has no rules for the chip -- and therefore drops
/// nothing from it. That is the default, deliberately. A register that
/// *triggers* on write rather than latching (a phrase start, a counter reload,
/// a key that re-attacks) makes the generic "same value, drop it" rule
/// audible, and the failure is silent: the file gets smaller and plays wrong.
/// Chips earn a rule by being checked, not by being present.
#[must_use]
fn latch_rule(chip: crate::vgm::ChipKind) -> Option<fn(u16) -> bool> {
    use crate::vgm::ChipKind as K;
    match chip {
        // The OPL family latches everything, key-on included: `0xB0`'s key bit
        // is level-sensitive, so re-writing it does not re-attack.
        K::Ym3812 | K::Ymf262 | K::Ym3526 | K::Y8950 => Some(|_| true),
        // The YM2612 is disabled for now, and falls back to `vgm_cmp`.
        //
        // Its `0xA4`-`0xA6` write latches the block + F-Number high bits; the
        // *following* `0xA0`-`0xA2` low-byte write is what commits both to the
        // channel. So a low-byte write whose value is unchanged is NOT redundant
        // when the high byte was re-latched since -- dropping it leaves the
        // channel at the wrong pitch. The per-address model here cannot see that
        // commit pairing (each register is its own cell), so it dropped those
        // commits and corrupted 25/500 corpus files, all YM2612, audibly -- the
        // parity gate `the_builtin_optimizer_never_changes_audio` catches it.
        //
        // Modelling the OPN commit latch (the way `vgm_cmp`'s A0/A4 look-ahead
        // does) is part 3a; until then the tools do YM2612 redundant-write
        // removal. See `docs/optimizer-2026-08/PLAN.md`.
        K::Ym2612 => None,
        // The YM2413's `0x20`-`0x28` carry the key-on bits.
        K::Ym2413 => Some(|addr| !(0x20..=0x28).contains(&addr)),
        _ => None,
    }
}

/// Whether this app has redundancy rules for `chip`.
#[must_use]
pub fn has_latch_rules(chip: crate::vgm::ChipKind) -> bool {
    latch_rule(chip).is_some()
}

/// The writes that can be dropped without changing what is heard, ascending.
///
/// A write is redundant when its chip has rules, the rules call its register a
/// pure latch, and the cell already holds that value. Everything else stays --
/// including every command from a chip with no rules, which is why running this
/// over an unfamiliar file is safe rather than merely likely to be.
///
/// `loop_at` is the row the song loops back to, if any. Every cell is forgotten
/// there, so the loop body re-establishes its own state and still sounds right
/// on the second pass through -- the same rule `chip_cmp` applies.
#[must_use]
pub fn redundant_indices(stream: &VgmStream, loop_at: Option<usize>) -> Vec<usize> {
    let mut held: BTreeMap<Cell, u16> = BTreeMap::new();
    let mut redundant = Vec::new();

    for index in 0..stream.len() {
        if loop_at == Some(index) {
            held.clear();
        }
        let Some(VgmCommand::Write { target, addr, data }) = stream.get(index) else {
            continue;
        };
        let Some(is_latch) = latch_rule(target.kind) else {
            continue;
        };
        let cell = Cell {
            chip: target.kind,
            instance: target.instance,
            port: target.port,
            addr,
        };
        if is_latch(addr) && held.get(&cell) == Some(&data) {
            redundant.push(index);
        }
        held.insert(cell, data);
    }
    redundant
}

/// Commands in `stream[..upto]` that carry state this module cannot replay.
///
/// A PCM RAM write or an unrecognised command may have configured something,
/// and a crop past one cannot promise to restore it. The caller warns; it is
/// not a refusal, because the alternative -- declining to trim the file at all
/// -- is worse, and the overwhelming majority of streams have none of these.
#[must_use]
pub fn unmodelled_commands(stream: &VgmStream, upto: usize) -> Vec<usize> {
    (0..upto.min(stream.len()))
        .filter(|&index| {
            matches!(
                stream.get(index),
                Some(VgmCommand::PcmRamWrite { .. } | VgmCommand::Raw { .. })
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vgm::ChipKind;
    use crate::vgm::stream::END_OF_DATA;

    fn stream(bytes: Vec<u8>) -> VgmStream {
        VgmStream::parse(bytes, 0x171).unwrap()
    }

    /// Two writes to the same register leave one latch, holding the later one.
    #[test]
    fn a_later_write_replaces_an_earlier_one() {
        let s = stream(vec![
            0x52,
            0x28,
            0x00, // YM2612 0x28 <- 0x00
            0x62, //
            0x52,
            0x28,
            0xF0, // YM2612 0x28 <- 0xF0
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        assert_eq!(state.len(), 1);
        assert_eq!(state.restore_indices(), [2], "the last write wins");
    }

    /// Different chips, instances and ports are different cells, even at the
    /// same register number.
    #[test]
    fn chip_instance_and_port_all_separate_a_cell() {
        let s = stream(vec![
            0x52,
            0x28,
            0x01, // YM2612 port 0
            0x53,
            0x28,
            0x02, // YM2612 port 1
            0xA2,
            0x28,
            0x03, // YM2612 #2 port 0
            0x54,
            0x28,
            0x04, // YM2151
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        assert_eq!(state.len(), 4, "four distinct cells");
        assert_eq!(state.restore_indices(), [0, 1, 2, 3]);
    }

    /// The causal order is what comes back, not the address order: a mode
    /// register written first is restored first, whatever its number.
    #[test]
    fn restores_come_back_in_the_order_they_were_written() {
        let s = stream(vec![
            0x5E,
            0x05,
            0x01, // YMF262 0x05 (the NEW bit) -- written first
            0x5E,
            0xC0,
            0x30, // YMF262 0xC0 -- depends on it
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        assert_eq!(
            state.restore_indices(),
            [0, 1],
            "0x05 precedes 0xC0 because it was written first, not because 5 < 0xC0"
        );

        // The same two writes the other way round come back the other way round.
        let s = stream(vec![
            0x5E,
            0xC0,
            0x30, //
            0x5E,
            0x05,
            0x01, //
            END_OF_DATA,
        ]);
        assert_eq!(ChipState::fold(&s, s.len()).restore_indices(), [0, 1]);
    }

    /// The property the whole feature rests on: replaying a restore leaves the
    /// chips exactly where the discarded span left them.
    #[test]
    fn a_restore_folds_to_the_state_it_came_from() {
        let s = stream(vec![
            0x52,
            0x28,
            0x00, //
            0x50,
            0x9F, // SN76489
            0x62, //
            0x52,
            0x28,
            0xF0, //
            0x53,
            0x30,
            0x71, //
            0x61,
            0x10,
            0x27, //
            0x52,
            0x22,
            0x08, //
            END_OF_DATA,
        ]);
        for upto in 0..=s.len() {
            let state = ChipState::fold(&s, upto);
            let prelude = stream({
                let mut bytes = ChipState::bytes_for(&s, &state.restore_indices());
                bytes.push(END_OF_DATA);
                bytes
            });
            let replayed = ChipState::fold(&prelude, prelude.len());
            assert_eq!(
                replayed.latches.len(),
                state.latches.len(),
                "cell count at {upto}"
            );
            for (cell, latch) in &state.latches {
                assert_eq!(
                    replayed.latches.get(cell).map(|l| l.value),
                    Some(latch.value),
                    "cell {cell:?} at {upto}"
                );
            }
        }
    }

    /// Data blocks are cumulative -- a later seek indexes the concatenation of
    /// all of them -- so every block before the cut comes back, in order, and
    /// before any register write.
    #[test]
    fn every_data_block_comes_back_first_and_in_order() {
        let mut bytes = vec![0x52, 0x28, 0x01];
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 2, 0, 0, 0, 0xAA, 0xBB]);
        bytes.extend_from_slice(&[0x52, 0x29, 0x02]);
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 2, 0, 0, 0, 0xCC, 0xDD]);
        bytes.push(END_OF_DATA);
        let s = stream(bytes);

        let state = ChipState::fold(&s, s.len());
        assert_eq!(
            state.restore_indices(),
            [1, 3, 0, 2],
            "both blocks, in order, then the writes"
        );
    }

    #[test]
    fn dac_stream_setup_and_the_pcm_seek_are_carried() {
        let s = stream(vec![
            0x90,
            0x00,
            0x02,
            0x00,
            0x2A, // stream 0 setup
            0x92,
            0x00,
            0x44,
            0xAC,
            0x00,
            0x00, // stream 0 frequency
            0xE0,
            0x10,
            0x00,
            0x00,
            0x00, // seek
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        assert_eq!(state.restore_indices(), [0, 1, 2]);
    }

    // -- the RF5C's channel-indirected register file --------------------------

    /// The bug this fixes: an RF5C's `0x00`-`0x06` registers address whichever
    /// channel `0x07` last selected, so a flat "last write per register" keeps
    /// only the last-selected channel's start address and the rest fall silent.
    /// Every channel's start address must survive the fold, and the restore must
    /// re-emit each after its own select, with the key-on last.
    #[test]
    fn an_rf5c_fold_keeps_every_channels_start_address() {
        // 0xB1 is the RF5C164 register write. Select each channel, give it a
        // distinct start address (register 0x06), then key channels 0-2 on.
        let s = stream(vec![
            0xB1,
            0x07,
            0x40, // 0: select channel 0
            0xB1,
            0x06,
            0x10, // 1: ch0 start = 0x10
            0xB1,
            0x07,
            0x41, // 2: select channel 1
            0xB1,
            0x06,
            0x20, // 3: ch1 start = 0x20
            0xB1,
            0x07,
            0x42, // 4: select channel 2
            0xB1,
            0x06,
            0x30, // 5: ch2 start = 0x30
            0xB1,
            0x08,
            0x07, // 6: key channels 0-2 on
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        let rf = state
            .rf
            .get(&(ChipKind::Rf5c164, 0))
            .expect("an RF5C164 register file");
        assert_eq!(rf.params.get(&(0, 0x06)).map(|l| l.value), Some(0x10));
        assert_eq!(rf.params.get(&(1, 0x06)).map(|l| l.value), Some(0x20));
        assert_eq!(
            rf.params.get(&(2, 0x06)).map(|l| l.value),
            Some(0x30),
            "all three channels' starts are kept, not just the last"
        );

        // Every channel's start (indices 1, 3, 5) comes back, and the key-on
        // (index 6) is last so it reloads each channel from a start already set.
        let restore = state.restore_indices();
        for start in [1usize, 3, 5] {
            assert!(
                restore.contains(&start),
                "channel start at {start} restored"
            );
        }
        assert_eq!(restore.last(), Some(&6), "the key-on follows every start");
    }

    /// Replaying an RF5C restore leaves each channel exactly where the discarded
    /// span did -- the fold-equivalence property for the channel-indirected file.
    /// Indices differ between the source and the replayed prelude, so the check
    /// is on the values each channel holds.
    #[test]
    fn an_rf5c_restore_folds_back_to_each_channels_state() {
        let s = stream(vec![
            0xB1,
            0x07,
            0xC0, // select channel 0, enabled (bit 7)
            0xB1,
            0x06,
            0x11, // ch0 start
            0xB1,
            0x00,
            0x0F, // ch0 envelope
            0xB1,
            0x07,
            0xC3, // select channel 3, enabled
            0xB1,
            0x06,
            0x44, // ch3 start
            0xB1,
            0x01,
            0x80, // ch3 pan
            0xB1,
            0x07,
            0x0F, // bank select (bit 6 clear), enabled
            0xB1,
            0x08,
            0x09, // key channels 0 and 3 on
            END_OF_DATA,
        ]);
        // The values a register file holds, index-independent.
        let values = |rf: &RfChannels| {
            (
                rf.params
                    .iter()
                    .map(|(key, latch)| (*key, latch.value))
                    .collect::<BTreeMap<_, _>>(),
                rf.bank.map(|l| l.value),
                rf.select_ctrl.map(|l| l.value),
                rf.key_on.map(|l| l.value),
            )
        };
        for upto in 0..=s.len() {
            let state = ChipState::fold(&s, upto);
            let prelude = stream({
                let mut bytes = ChipState::bytes_for(&s, &state.restore_indices());
                bytes.push(END_OF_DATA);
                bytes
            });
            let replayed = ChipState::fold(&prelude, prelude.len());
            let key = (ChipKind::Rf5c164, 0);
            match state.rf.get(&key) {
                Some(rf) => {
                    let back = replayed.rf.get(&key).expect("rf present after replay");
                    assert_eq!(values(back), values(rf), "rf values at {upto}");
                }
                None => assert!(replayed.rf.get(&key).is_none_or(RfChannels::is_empty)),
            }
        }
    }

    // -- diffing two states --------------------------------------------------

    /// Cutting the middle out only needs the writes that actually changed
    /// something -- not every write the removed span happened to make.
    #[test]
    fn a_diff_carries_only_what_really_changed() {
        let s = stream(vec![
            0x52,
            0x28,
            0x01, // 0: before the cut
            0x52,
            0x29,
            0x02, // 1: before the cut
            0x52,
            0x28,
            0x01, // 2: inside -- rewrites the same value
            0x52,
            0x29,
            0x99, // 3: inside -- genuinely changes 0x29
            0x52,
            0x2A,
            0x07, // 4: inside -- a cell not seen before
            0x52,
            0x30,
            0x00, // 5: after the cut
            END_OF_DATA,
        ]);
        let before = ChipState::fold(&s, 2);
        let after = ChipState::fold(&s, 5);
        assert_eq!(
            after.changes_from(&before),
            [3, 4],
            "the repeat of 0x28 is not a change"
        );
    }

    #[test]
    fn a_diff_against_silence_is_the_whole_restore() {
        let s = stream(vec![0x52, 0x28, 0x01, 0x50, 0x9F, END_OF_DATA]);
        let state = ChipState::fold(&s, s.len());
        assert_eq!(
            state.changes_from(&ChipState::default()),
            state.restore_indices()
        );
    }

    /// A block loaded inside the removed span is still needed afterwards.
    #[test]
    fn a_diff_keeps_a_block_the_removed_span_loaded() {
        let mut bytes = vec![0x52, 0x28, 0x01];
        bytes.extend_from_slice(&[0x67, 0x66, 0x00, 1, 0, 0, 0, 0xAA]);
        bytes.extend_from_slice(&[0x52, 0x29, 0x02]);
        bytes.push(END_OF_DATA);
        let s = stream(bytes);

        let before = ChipState::fold(&s, 1);
        let after = ChipState::fold(&s, 3);
        assert_eq!(after.changes_from(&before), [1, 2]);
    }

    // -- redundancy ----------------------------------------------------------

    #[test]
    fn a_repeated_write_to_a_latch_is_redundant() {
        let s = stream(vec![
            0x5A,
            0x20,
            0x01, // 0
            0x62, // 1
            0x5A,
            0x20,
            0x01, // 2: the same value again
            0x5A,
            0x20,
            0x02, // 3: a different value
            0x5A,
            0x20,
            0x02, // 4: and its repeat
            END_OF_DATA,
        ]);
        assert_eq!(redundant_indices(&s, None), [2, 4]);
    }

    /// The default is to drop nothing. A chip this app has not checked keeps
    /// every write, however repetitive -- being smaller is not worth being
    /// silently wrong.
    #[test]
    fn a_chip_with_no_rules_keeps_every_write() {
        assert!(!has_latch_rules(ChipKind::Sn76489));
        let s = stream(vec![
            0x50,
            0x9F, // SN76489
            0x50,
            0x9F, // the same byte again
            END_OF_DATA,
        ]);
        assert!(redundant_indices(&s, None).is_empty());
    }

    /// Registers that trigger on write rather than latching are never dropped,
    /// even on a chip that has rules. The YM2413's `0x20`-`0x28` carry the key
    /// bit, so a value-identical repeat re-attacks and is kept; an ordinary
    /// latch (`0x30`, instrument + volume) is dropped on repeat.
    #[test]
    fn a_trigger_register_is_never_dropped() {
        let s = stream(vec![
            0x51,
            0x20,
            0x30, // YM2413 block + key
            0x51,
            0x20,
            0x30, // the same value again -- a trigger, kept
            0x51,
            0x30,
            0x0F, // an ordinary latch
            0x51,
            0x30,
            0x0F, // its repeat -- dropped
            END_OF_DATA,
        ]);
        assert_eq!(redundant_indices(&s, None), [3], "only the latch repeat");
    }

    /// Everything is forgotten at the loop point, so the loop body carries its
    /// own state and sounds the same on the second pass.
    #[test]
    fn the_loop_point_forgets_every_cell() {
        let s = stream(vec![
            0x5A,
            0x20,
            0x01, // 0
            0x5A,
            0x20,
            0x01, // 1: redundant
            0x5A,
            0x20,
            0x01, // 2: the loop point -- kept
            0x5A,
            0x20,
            0x01, // 3: redundant again
            END_OF_DATA,
        ]);
        assert_eq!(redundant_indices(&s, Some(2)), [1, 3]);
    }

    /// Two instances of a chip are two sets of registers.
    #[test]
    fn a_second_instance_holds_its_own_values() {
        let s = stream(vec![
            0x5A,
            0x20,
            0x01, // chip 1
            0xAA,
            0x20,
            0x01, // chip 2 -- not a repeat of chip 1
            0xAA,
            0x20,
            0x01, // now it is
            END_OF_DATA,
        ]);
        assert_eq!(redundant_indices(&s, None), [2]);
    }

    // -- what is not modelled ------------------------------------------------

    #[test]
    fn unmodelled_commands_are_reported_rather_than_silently_lost() {
        let s = stream(vec![
            0x52,
            0x28,
            0x01, //
            0x68,
            0x66,
            0x01,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9, // PCM RAM write
            0xC9,
            1,
            2,
            3, // reserved
            END_OF_DATA,
        ]);
        assert_eq!(unmodelled_commands(&s, s.len()), [1, 2]);
        assert!(unmodelled_commands(&s, 1).is_empty());
        // They contribute nothing to the state, rather than a wrong something.
        assert_eq!(ChipState::fold(&s, s.len()).restore_indices(), [0]);
    }

    #[test]
    fn waits_carry_no_state() {
        let s = stream(vec![0x61, 0x10, 0x27, 0x62, 0x85, END_OF_DATA]);
        assert!(ChipState::fold(&s, s.len()).is_empty());
    }

    #[test]
    fn an_empty_span_restores_nothing() {
        let s = stream(vec![0x52, 0x28, 0x01, END_OF_DATA]);
        assert!(ChipState::fold(&s, 0).restore_indices().is_empty());
    }

    /// The bytes come out of the source verbatim, whatever the encoding: a
    /// second-chip write comes back as the second-chip opcode it was.
    #[test]
    fn restored_bytes_are_the_sources_own() {
        let s = stream(vec![
            0xA2,
            0x28,
            0xF0, // YM2612 #2 via the 0xAn mirror
            0xB3,
            0x90,
            0x0F, // Game Boy #2 via bit 7 of the address
            END_OF_DATA,
        ]);
        let state = ChipState::fold(&s, s.len());
        let bytes = ChipState::bytes_for(&s, &state.restore_indices());
        assert_eq!(bytes, [0xA2, 0x28, 0xF0, 0xB3, 0x90, 0x0F]);

        // ...and they read back as the same targets.
        let replayed = stream({
            let mut b = bytes;
            b.push(END_OF_DATA);
            b
        });
        let Some(VgmCommand::Write { target, .. }) = replayed.get(0) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::Ym2612, 1));
        let Some(VgmCommand::Write { target, .. }) = replayed.get(1) else {
            panic!("a write");
        };
        assert_eq!((target.kind, target.instance), (ChipKind::GameBoyDmg, 1));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::vgm::stream::END_OF_DATA;
    use proptest::prelude::*;

    /// A stream of writes, waits and blocks -- the commands a state fold has to
    /// reason about.
    fn any_stateful_command() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            // Writes across several chips, instances and ports.
            (
                prop::sample::select(vec![0x52u8, 0x53, 0x5A, 0x5E, 0x5F, 0xA2, 0x50, 0xB3]),
                any::<u8>(),
                any::<u8>()
            )
                .prop_map(|(op, a, b)| if op == 0x50 {
                    vec![op, a]
                } else {
                    vec![op, a, b]
                }),
            Just(vec![0x62]),
            (any::<u8>(), any::<u8>()).prop_map(|(a, b)| vec![0x61, a, b]),
            prop::collection::vec(any::<u8>(), 0..4).prop_map(|payload| {
                let mut bytes = vec![0x67, 0x66, 0x00];
                bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&payload);
                bytes
            }),
        ]
    }

    proptest! {
        /// Fold-equivalence, over any stream and any cut point: replaying what
        /// a restore emits must leave the chips in exactly the state the
        /// discarded span left them in. This is what makes a crop honest, and
        /// it needs no emulator to check.
        #[test]
        fn a_restore_always_folds_back_to_its_own_state(
            commands in prop::collection::vec(any_stateful_command(), 0..30),
            cut in 0usize..30,
        ) {
            let mut bytes: Vec<u8> = commands.iter().flatten().copied().collect();
            bytes.push(END_OF_DATA);
            let stream = VgmStream::parse(bytes, 0x171)?;
            let upto = cut.min(stream.len());

            let state = ChipState::fold(&stream, upto);
            let mut prelude_bytes = ChipState::bytes_for(&stream, &state.restore_indices());
            prelude_bytes.push(END_OF_DATA);
            let prelude = VgmStream::parse(prelude_bytes, 0x171)?;
            let replayed = ChipState::fold(&prelude, prelude.len());

            prop_assert_eq!(replayed.latches.len(), state.latches.len());
            for (cell, latch) in &state.latches {
                prop_assert_eq!(
                    replayed.latches.get(cell).map(|l| l.value),
                    Some(latch.value)
                );
            }
            prop_assert_eq!(replayed.blocks.len(), state.blocks.len());
        }

        /// A diff plus the earlier state is the later state: cutting a span out
        /// and splicing its changes in leaves the chips where they would have
        /// been. The property `delete_region` rests on.
        #[test]
        fn a_diff_applied_to_the_earlier_state_reaches_the_later_one(
            commands in prop::collection::vec(any_stateful_command(), 0..30),
            a in 0usize..30,
            b in 0usize..30,
        ) {
            let mut bytes: Vec<u8> = commands.iter().flatten().copied().collect();
            bytes.push(END_OF_DATA);
            let stream = VgmStream::parse(bytes, 0x171)?;
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            let (start, end) = (start.min(stream.len()), end.min(stream.len()));

            let before = ChipState::fold(&stream, start);
            let after = ChipState::fold(&stream, end);

            // Replay: the prefix, then the diff of the removed span.
            let mut spliced = ChipState::bytes_for(&stream, &before.restore_indices());
            spliced.extend(ChipState::bytes_for(&stream, &after.changes_from(&before)));
            spliced.push(END_OF_DATA);
            let replayed = ChipState::fold(
                &VgmStream::parse(spliced, 0x171)?,
                usize::MAX,
            );

            for (cell, latch) in &after.latches {
                prop_assert_eq!(
                    replayed.latches.get(cell).map(|l| l.value),
                    Some(latch.value),
                    "cell {:?}", cell
                );
            }
        }
    }
}
