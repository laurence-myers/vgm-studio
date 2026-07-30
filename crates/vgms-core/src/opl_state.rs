//! The OPL register-file model shared by the optimiser and the multi-song
//! splitter.
//!
//! Every OPL register is a level-sensitive latch: the chip holds whatever value
//! was last written to it. Two 256-entry files cover every OPL this app reads --
//! the low file and the high file. For a dual OPL2 the high file is the second
//! chip; for an OPL3 it is port 1. A song only ever targets one of those pairs,
//! and the reader has already routed each write opcode onto the right file (via
//! its decoded [`Bank`]), so keying on the bank never conflates them.
//!
//! [`optimize`](crate::optimize) uses it to spot a write that changes nothing
//! (the cached value already equals the one being written); [`split_songs`] uses
//! it to capture the register state reached at a segment boundary and replay it
//! as the minimal set of writes that recreates it.
//!
//! [`split_songs`]: crate::split_songs

use crate::song::Bank;

/// The number of register files tracked: the low file and the high file.
const FILE_COUNT: usize = 2;
/// OPL registers are addressed by a single byte.
const REGISTER_COUNT: usize = 256;

/// The last value written to every OPL register, per file.
///
/// An entry is `None` until its register is first written -- since construction,
/// or since the last [`reset`](Self::reset). Power-on defaults are never assumed,
/// so the first write to a register is always significant.
#[derive(Debug, Clone)]
pub struct OplState {
    files: [[Option<u8>; REGISTER_COUNT]; FILE_COUNT],
}

impl Default for OplState {
    fn default() -> Self {
        Self::new()
    }
}

impl OplState {
    /// A blank state: no register written on either file.
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: [[None; REGISTER_COUNT]; FILE_COUNT],
        }
    }

    /// Forgets every cached value, so the next write to any register is treated
    /// as a first write. The optimiser calls this at the loop point.
    pub fn reset(&mut self) {
        self.files = [[None; REGISTER_COUNT]; FILE_COUNT];
    }

    /// The file a bank maps to: the low file (`0`) or the high file (`1`).
    ///
    /// A `None` bank -- only DRO v1, which tracks the bank separately and is
    /// never fed to this model -- maps to the low file.
    fn file(bank: Option<Bank>) -> usize {
        bank.map_or(0, |bank| usize::from(bank.index()))
    }

    /// Records a write of `value` to `reg` on `bank`.
    pub fn record(&mut self, bank: Option<Bank>, reg: u8, value: u8) {
        self.files[Self::file(bank)][usize::from(reg)] = Some(value);
    }

    /// The value `reg` on `bank` currently holds, or `None` if it has not been
    /// written since construction or the last [`reset`](Self::reset).
    #[must_use]
    pub fn get(&self, bank: Option<Bank>, reg: u8) -> Option<u8> {
        self.files[Self::file(bank)][usize::from(reg)]
    }

    /// Whether `reg` on `bank` already holds `value` -- i.e. writing it would be
    /// a no-op the optimiser can drop.
    #[must_use]
    pub fn is_set(&self, bank: Option<Bank>, reg: u8, value: u8) -> bool {
        self.get(bank, reg) == Some(value)
    }

    /// The minimal set of writes that recreates this state, as `(bank, reg,
    /// value)` triples: every register that has been written, the low file
    /// before the high file, each file in ascending register order.
    ///
    /// A register file is a bank of independent latches, so the *final* state is
    /// the same whatever order the writes land in; ascending, low-before-high is
    /// simply the canonical choice. It also happens to put each file's OPL3-mode
    /// and four-operator enables (registers `0x04`/`0x05`) ahead of the operator
    /// registers that pair under them, which reads naturally even though the
    /// nuked-opl3 core latches every write regardless.
    #[must_use]
    pub fn replay_writes(&self) -> Vec<(Bank, u8, u8)> {
        let mut writes = Vec::new();
        for (file, registers) in self.files.iter().enumerate() {
            let bank = if file == 0 { Bank::Low } else { Bank::High };
            for (reg, slot) in registers.iter().enumerate() {
                if let Some(value) = *slot {
                    writes.push((bank, reg as u8, value));
                }
            }
        }
        writes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_state_holds_nothing() {
        let state = OplState::new();
        assert_eq!(state.get(Some(Bank::Low), 0x20), None);
        assert_eq!(state.get(Some(Bank::High), 0x20), None);
        assert!(state.replay_writes().is_empty());
    }

    #[test]
    fn record_latches_the_last_value() {
        let mut state = OplState::new();
        state.record(Some(Bank::Low), 0x20, 0x01);
        assert!(state.is_set(Some(Bank::Low), 0x20, 0x01));
        assert!(!state.is_set(Some(Bank::Low), 0x20, 0x02));
        // A rewrite replaces it.
        state.record(Some(Bank::Low), 0x20, 0x02);
        assert_eq!(state.get(Some(Bank::Low), 0x20), Some(0x02));
        assert!(state.is_set(Some(Bank::Low), 0x20, 0x02));
    }

    #[test]
    fn the_two_files_are_independent() {
        let mut state = OplState::new();
        state.record(Some(Bank::Low), 0x20, 0x01);
        assert_eq!(state.get(Some(Bank::High), 0x20), None);
        state.record(Some(Bank::High), 0x20, 0x09);
        assert_eq!(state.get(Some(Bank::Low), 0x20), Some(0x01));
        assert_eq!(state.get(Some(Bank::High), 0x20), Some(0x09));
    }

    #[test]
    fn a_missing_bank_maps_to_the_low_file() {
        let mut state = OplState::new();
        state.record(None, 0x40, 0x3F);
        assert_eq!(state.get(Some(Bank::Low), 0x40), Some(0x3F));
        assert!(state.is_set(None, 0x40, 0x3F));
    }

    #[test]
    fn reset_forgets_everything() {
        let mut state = OplState::new();
        state.record(Some(Bank::Low), 0x20, 0x01);
        state.record(Some(Bank::High), 0x21, 0x02);
        state.reset();
        assert!(state.replay_writes().is_empty());
    }

    #[test]
    fn replay_lists_touched_registers_low_before_high_ascending() {
        let mut state = OplState::new();
        // Recorded out of order and across both files.
        state.record(Some(Bank::High), 0x40, 0x11);
        state.record(Some(Bank::Low), 0xB0, 0x22);
        state.record(Some(Bank::Low), 0x20, 0x33);
        state.record(Some(Bank::High), 0x05, 0x01);

        assert_eq!(
            state.replay_writes(),
            vec![
                (Bank::Low, 0x20, 0x33),
                (Bank::Low, 0xB0, 0x22),
                (Bank::High, 0x05, 0x01),
                (Bank::High, 0x40, 0x11),
            ]
        );
    }

    /// Replaying the captured writes onto a fresh state reproduces it exactly:
    /// the emitter is a faithful inverse of the recorder.
    #[test]
    fn replaying_reproduces_the_captured_state() {
        let mut original = OplState::new();
        for (bank, reg, value) in [
            (Bank::Low, 0x20u8, 0x01u8),
            (Bank::Low, 0x20, 0x05), // overwritten
            (Bank::High, 0xA0, 0x7F),
            (Bank::Low, 0xBD, 0x20),
        ] {
            original.record(Some(bank), reg, value);
        }

        let mut replayed = OplState::new();
        for (bank, reg, value) in original.replay_writes() {
            replayed.record(Some(bank), reg, value);
        }
        assert_eq!(replayed.replay_writes(), original.replay_writes());
        assert_eq!(replayed.get(Some(Bank::Low), 0x20), Some(0x05));
    }
}
