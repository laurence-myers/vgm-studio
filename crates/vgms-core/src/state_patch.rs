//! Carrying the chip from one register state to another.
//!
//! Several edits lift a piece out of the middle of a stream: [`split_songs`]
//! materialises one song of a capture, [`crop`] keeps or drops a marked region.
//! Whatever survives has to open on the register state the original had reached
//! at that point, or an instrument set up earlier plays with whatever the chip
//! happens to be holding.
//!
//! The fix is the same shape every time: a *patch*, the writes that carry the
//! chip from the state at one point in the stream to the state at another. A
//! prelude for a piece taken from the middle is the patch from the blank state;
//! the seam a deleted region leaves behind is the patch across it. Both fall out
//! of one diff, so both live here.
//!
//! Writes are re-used byte for byte from the source stream, so the encoding is
//! exact whatever the format -- a VGM's chip-routing opcode, a DRO v2's codemap
//! code, a v1's bank-less pair.
//!
//! [`split_songs`]: crate::split_songs
//! [`crop`]: crate::crop

use crate::opl_state::OplState;
use crate::song::dro_data::v1_opcode;
use crate::song::{Bank, DroSong, DroSongData, Instruction};

/// The size of one OPL register file (low or high).
const REGISTER_COUNT: usize = 256;
/// The number of register files tracked: the low file and the high file.
const FILE_COUNT: usize = 2;

/// The register state a stream has reached at some point, and how it got there.
///
/// Alongside the values ([`OplState`]) it keeps the source index of each
/// register's last write, so a patch can re-emit the source's own bytes rather
/// than synthesise a write, and the bank the stream is left in, which a DRO v1's
/// bank-less writes depend on.
#[derive(Debug, Clone)]
pub(crate) struct StateFold {
    state: OplState,
    /// The source index of the last write to each (file, register).
    last_write: [[Option<usize>; REGISTER_COUNT]; FILE_COUNT],
    /// The bank current at the fold point.
    bank: Bank,
}

impl StateFold {
    /// The state a stream is in before any of it has played: no register
    /// written, and the low bank current, which is where a DRO stream starts.
    pub(crate) fn blank() -> Self {
        Self {
            state: OplState::new(),
            last_write: [[None; REGISTER_COUNT]; FILE_COUNT],
            bank: Bank::Low,
        }
    }

    /// The state `song` has reached after its first `upto` instructions.
    pub(crate) fn over(song: &DroSong, upto: usize) -> Self {
        let mut fold = Self::blank();
        fold.advance(song, 0, upto);
        fold
    }

    /// Folds `song`'s instructions `[from, to)` into the state, so a fold taken
    /// at one point can be carried on to a later one without re-walking the
    /// stream from the beginning.
    pub(crate) fn advance(&mut self, song: &DroSong, from: usize, to: usize) {
        for index in from..to.min(song.len()) {
            match song.instruction(index) {
                Some(Instruction::Register { reg, value, bank }) => {
                    // A v1 write carries no bank of its own: it lands in
                    // whichever bank the last switch selected.
                    let bank = bank.unwrap_or(self.bank);
                    self.state.record(Some(bank), reg, value);
                    self.last_write[usize::from(bank.index())][usize::from(reg)] = Some(index);
                }
                Some(Instruction::BankSwitch(bank)) => self.bank = bank,
                _ => {}
            }
        }
    }
}

/// Appends the writes that carry the chip from `from` to `to`, leaving it in the
/// bank `to` was reached in. Returns how many instructions were appended.
///
/// Only registers `to` holds at a value `from` does not are written: one the two
/// agree on is already right, and re-writing it would be noise -- and, for a
/// key-on register, an audible retrigger. A `to` fold always covers a superset of
/// `from`'s writes (it is the same stream, folded further), so a register `from`
/// holds is never absent from `to`: there is no un-write case to express, which
/// is just as well, since an OPL register cannot be returned to "never written".
///
/// The writes go out low file before high, each file in ascending register order.
/// A register file is a bank of independent latches, so the state reached is the
/// same whatever order they land in; ascending is simply the canonical choice, as
/// it is in [`OplState::replay_writes`].
///
/// A DRO v1's writes carry no bank, so the patch emits its own bank switches
/// around each file's group, entering in the bank `from` left current and leaving
/// in the one `to` expects. Every other format carries the bank in the write, so
/// no switches are emitted at all.
pub(crate) fn append_patch(
    bytes: &mut Vec<u8>,
    song: &DroSong,
    from: &StateFold,
    to: &StateFold,
) -> usize {
    let is_v1 = matches!(song.data(), DroSongData::V1(_));
    let mut emit_bank = from.bank;
    let mut appended = 0;

    for file in [Bank::Low, Bank::High] {
        let writes: Vec<usize> = (0..REGISTER_COUNT)
            .filter_map(|reg| {
                let index = to.last_write[usize::from(file.index())][reg]?;
                let reg = reg as u8;
                let changed = to.state.get(Some(file), reg) != from.state.get(Some(file), reg);
                changed.then_some(index)
            })
            .collect();
        if writes.is_empty() {
            continue;
        }
        if is_v1 && emit_bank != file {
            bytes.extend_from_slice(bank_switch_bytes(file));
            emit_bank = file;
            appended += 1;
        }
        for index in writes {
            bytes.extend_from_slice(
                song.data()
                    .raw_instruction(index)
                    .expect("a folded index is an index into the song"),
            );
            appended += 1;
        }
    }

    // Leave a v1 chip in the bank the following body's bank-less writes expect.
    if is_v1 && emit_bank != to.bank {
        bytes.extend_from_slice(bank_switch_bytes(to.bank));
        appended += 1;
    }
    appended
}

/// The one-byte DRO v1 bank-switch instruction for `bank`.
fn bank_switch_bytes(bank: Bank) -> &'static [u8] {
    match bank {
        Bank::Low => &[v1_opcode::BANK_LOW],
        Bank::High => &[v1_opcode::BANK_HIGH],
    }
}

/// The register state a naive replay of `song`'s writes over `[0, upto)` reaches,
/// as `(bank, reg, value)` replay triples -- the reference a patched stream must
/// reproduce.
#[cfg(test)]
pub(crate) fn state_over(song: &DroSong, upto: usize) -> Vec<(Bank, u8, u8)> {
    StateFold::over(song, upto).state.replay_writes()
}

/// The state reached after folding the first `n` register writes of `song`,
/// tracking the current bank from the bank-switch opcodes as [`state_over`] does.
///
/// This is how a patched stream is checked: the patch is `n` writes long, so
/// folding exactly that many says what state the body opens on.
#[cfg(test)]
pub(crate) fn state_after_writes(song: &DroSong, n: usize) -> Vec<(Bank, u8, u8)> {
    let mut fold = StateFold::blank();
    let mut seen = 0;
    for index in 0..song.len() {
        if seen == n {
            break;
        }
        if matches!(song.instruction(index), Some(Instruction::Register { .. })) {
            seen += 1;
        }
        fold.advance(song, index, index + 1);
    }
    fold.state.replay_writes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::OplType;

    /// The patch bytes carrying `song` from state-at-`from` to state-at-`to`,
    /// and the instruction count reported.
    fn patch_between(song: &DroSong, from: usize, to: usize) -> (Vec<u8>, usize) {
        let mut bytes = Vec::new();
        let count = append_patch(
            &mut bytes,
            song,
            &StateFold::over(song, from),
            &StateFold::over(song, to),
        );
        (bytes, count)
    }

    // -- DRO v1 bank tracking -------------------------------------------------

    /// A DRO v1 register write: just `reg value`, the bank coming from whichever
    /// switch was last seen.
    fn v1_write(reg: u8, value: u8) -> Vec<u8> {
        vec![reg, value]
    }

    fn switch(bank: Bank) -> Vec<u8> {
        bank_switch_bytes(bank).to_vec()
    }

    /// A v1 stream: low 0x20=0x01, switch high, 0x40=0x10, switch low, 0xB0=0x31.
    fn v1_song() -> DroSong {
        let data = [
            v1_write(0x20, 0x01),
            switch(Bank::High),
            v1_write(0x40, 0x10),
            switch(Bank::Low),
            v1_write(0xB0, 0x31),
        ]
        .concat();
        DroSong::dro_v1(
            "t.dro".to_owned(),
            crate::song::DroDataV1::new(data).unwrap(),
            0,
            OplType::DualOpl2,
        )
    }

    #[test]
    fn only_the_last_value_of_a_rewritten_register_is_emitted() {
        // 0x20 is written twice to *different* values; the fold records the
        // latest, so the patch carries 0x99 rather than the stale 0x11. A fold
        // that kept the first write would silently re-emit the old value.
        let data = [v1_write(0x20, 0x11), v1_write(0x20, 0x99)].concat();
        let song = DroSong::dro_v1(
            "rewrite.dro".to_owned(),
            crate::song::DroDataV1::new(data).unwrap(),
            0,
            OplType::Opl2,
        );
        let (bytes, count) = patch_between(&song, 0, song.len());
        assert_eq!(count, 1, "one register, one write");
        assert_eq!(bytes, v1_write(0x20, 0x99));
    }

    #[test]
    fn a_v1_fold_tracks_the_bank_across_switches() {
        let song = v1_song();
        // After the whole stream the chip is back in the low bank.
        assert_eq!(StateFold::over(&song, song.len()).bank, Bank::Low);
        // Mid-stream, just after the switch to high, it is not.
        assert_eq!(StateFold::over(&song, 2).bank, Bank::High);

        let state = state_over(&song, song.len());
        assert!(
            state.contains(&(Bank::High, 0x40, 0x10)),
            "high write tracked"
        );
        assert!(state.contains(&(Bank::Low, 0x20, 0x01)));
        assert!(state.contains(&(Bank::Low, 0xB0, 0x31)));
    }

    #[test]
    fn a_v1_patch_emits_its_own_bank_switches() {
        let song = v1_song();
        let (bytes, count) = patch_between(&song, 0, song.len());
        // The low group ascending, a switch up for the high group, and a switch
        // back to the low bank the body's bank-less writes expect.
        assert_eq!(
            bytes,
            [
                v1_write(0x20, 0x01),
                v1_write(0xB0, 0x31),
                switch(Bank::High),
                v1_write(0x40, 0x10),
                switch(Bank::Low),
            ]
            .concat()
        );
        assert_eq!(count, 5, "three writes and two switches");
    }

    #[test]
    fn a_v1_patch_enters_in_the_bank_it_was_handed() {
        // Spliced in at index 2 the chip is already in the *high* bank, so the
        // low group has to switch down to itself first -- a patch that assumed a
        // low entry would land these two bytes in the high bank.
        let song = v1_song();
        let (bytes, count) = patch_between(&song, 2, song.len());
        // 0x20 was already set at the entry point and never changed, so it is not
        // rewritten; the low group is just 0xB0, and reaching it from the high
        // bank costs the leading switch.
        assert_eq!(
            bytes,
            [
                switch(Bank::Low),
                v1_write(0xB0, 0x31),
                switch(Bank::High),
                v1_write(0x40, 0x10),
                switch(Bank::Low),
            ]
            .concat()
        );
        assert_eq!(count, 5, "two writes and three switches");
    }

    #[test]
    fn a_v1_patch_with_no_writes_still_emits_nothing() {
        // Even though the entry and exit banks differ, an empty diff means the
        // body was never interrupted: there is nothing to leave the chip on.
        let song = v1_song();
        let mut bytes = Vec::new();
        let fold = StateFold::over(&song, song.len());
        let count = append_patch(&mut bytes, &song, &fold, &fold);
        assert!(bytes.is_empty());
        assert_eq!(count, 0);
    }
}
