//! On-demand detailed register analysis (Python `DRODetailedRegisterAnalyzer`).
//!
//! The Python analyser produced, for *every* instruction, a
//! `(bank, description, ms_offset)` tuple, and the GUI stored the whole list --
//! hundreds of thousands of heap strings, recomputed from scratch after every
//! edit and undo. Two of those three fields no longer need an analyser at all:
//! the ms offset is [`Song::ms_offset_at`] (the delay prefix sum, built at load),
//! and the bank falls out of tracking bank switches. What is left is the
//! *changed-bits* description for the instruction table -- which fields of a
//! register a write actually altered.
//!
//! This is a lazy replay **cursor**, not the eager list. It holds the chip's
//! register state and replays forward one instruction at a time, so a table
//! painting visible rows top-to-bottom pays `O(1)` amortised per row. Jumping to
//! an earlier row resets and replays from the start. Nothing here is scheduled on
//! a background thread: it is pure and wasm-clean, and a caller queries it
//! directly while painting. (Periodic keyframes remain a possible later
//! optimisation if the reset-and-replay of a backward jump ever measures too
//! slow; the access pattern that matters -- scrolling a virtual table -- does not
//! need them.)

use std::borrow::Cow;
use std::fmt;

use crate::regdata::{self, RegisterKind};
use crate::song::{Bank, DroInstruction, Song};

/// The analysis of one instruction, for the table's Bank and Description columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowAnalysis {
    /// The register bank in effect once this instruction has executed. A bank
    /// switch reports the bank it switches *to*; a v2/VGM register reports its
    /// own bank; a delay or a v1 register leaves the running bank unchanged.
    pub bank: Bank,
    /// The instruction-table Description column: the register fields this write
    /// changed, joined with `" / "`, or a delay / bank-switch / unknown-register
    /// line.
    ///
    /// Borrowed for the common cases -- `"(no changes)"`, a single changed field
    /// -- and owned only when several fields changed at once or a value must be
    /// formatted in.
    pub description: Cow<'static, str>,
}

/// A replay cursor over a song's register writes.
///
/// Build one per song. After any edit to that song -- delete, insert, undo or
/// redo -- call [`Self::reset`], because an edit invalidates the replayed chip
/// state. Query rows with [`Self::row`].
#[derive(Clone)]
pub struct RegisterAnalyzer {
    /// Instructions `[0, applied)` have been replayed into `state` and `bank`.
    applied: usize,
    /// The bank in effect after `[0, applied)`.
    bank: Bank,
    /// The last value written to each `(bank << 8) | reg`, or `None` if never.
    ///
    /// Sized `0x200`, not `0x1FF`: the key reaches `0x1FF` (high bank, register
    /// `0xFF`). The Python's `[None] * 0x1FF` escaped an out-of-bounds write only
    /// because an unknown register returned before it ever indexed the array.
    state: Box<[Option<u8>; 0x200]>,
}

impl RegisterAnalyzer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            applied: 0,
            bank: Bank::Low,
            state: Box::new([None; 0x200]),
        }
    }

    /// Discards the replayed chip state and returns to the start of the song.
    pub fn reset(&mut self) {
        self.applied = 0;
        self.bank = Bank::Low;
        self.state.iter_mut().for_each(|slot| *slot = None);
    }

    /// Analyses the instruction at `index`, replaying as far as needed.
    ///
    /// Returns `None` iff `index` is out of range. Querying rows in ascending
    /// order (as a virtual table paints them) is `O(1)` amortised; a lower
    /// `index` than the last resets the cursor and replays from `0`.
    pub fn row(&mut self, song: &Song, index: usize) -> Option<RowAnalysis> {
        if index >= song.len() {
            return None;
        }
        if self.applied > index {
            self.reset();
        }
        // Replay up to (but not including) `index`, discarding descriptions,
        // so that the chip state reflects exactly instructions `[0, index)`...
        while self.applied < index {
            let instruction = song
                .instruction(self.applied)
                .expect("index < song.len(), so every earlier instruction decodes");
            self.step(instruction);
            self.applied += 1;
        }
        // ...then describe `index` itself against the state it now sees.
        let instruction = song
            .instruction(index)
            .expect("index < song.len() checked above");
        let analysis = self.step(instruction);
        self.applied += 1;
        Some(analysis)
    }

    /// The analysis of every instruction, in order.
    ///
    /// A convenience over [`Self::row`] for callers that want the whole song at
    /// once -- tests, or a caller that prefers the Python's precompute-everything
    /// model. The GUI queries `row` per visible line instead.
    #[must_use]
    pub fn analyze_all(song: &Song) -> Vec<RowAnalysis> {
        let mut analyzer = Self::new();
        (0..song.len())
            .map(|index| {
                analyzer
                    .row(song, index)
                    .expect("index is in 0..song.len()")
            })
            .collect()
    }

    /// Applies one instruction to the cursor and returns its analysis.
    fn step(&mut self, instruction: DroInstruction) -> RowAnalysis {
        // A bank switch, or a v2/VGM register that carries its own bank, moves
        // the running bank *before* the row is described. A v1 register carries
        // no bank and leaves it on whatever the last switch selected.
        if let Some(selected) = instruction.selected_bank() {
            self.bank = selected;
        }
        let description = match instruction {
            DroInstruction::DelayMs { ms, .. } => Cow::Owned(format!("Delay: {ms} ms")),
            DroInstruction::DelaySamples { samples, .. } => {
                Cow::Owned(format!("Delay: {samples} smp"))
            }
            DroInstruction::BankSwitch(_) => Cow::Borrowed(match self.bank {
                Bank::Low => "Bank switch: low",
                Bank::High => "Bank switch: high",
            }),
            DroInstruction::Register { reg, value, .. } => self.describe_register(reg, value),
        };
        RowAnalysis {
            bank: self.bank,
            description,
        }
    }

    /// Describes a register write and records its value in the chip state.
    fn describe_register(&mut self, reg: u8, value: u8) -> Cow<'static, str> {
        let Some(kind) = kind_for(self.bank, reg) else {
            // The Python returned here *before* recording the value, so an
            // unknown register never populates the state array. Its `%s` on an
            // int register number is a decimal, unlike the hex `register_display`.
            return Cow::Owned(format!("Unknown register: {reg}"));
        };

        let key = (usize::from(self.bank.index()) << 8) | usize::from(reg);
        let previous = self.state[key];
        self.state[key] = Some(value);

        // A field counts as changed on the first write to this register+bank, or
        // when the bits under its mask differ from last time.
        let changed = |mask: u8| match previous {
            None => true,
            Some(old) => (old ^ value) & mask != 0,
        };
        // Count first so the 0- and 1-field cases -- the overwhelming majority of
        // writes -- borrow a `&'static str` instead of allocating.
        let mut count = 0usize;
        let mut only = "";
        for bitmask in kind.bitmasks() {
            if changed(bitmask.mask) {
                count += 1;
                only = bitmask.description;
            }
        }
        match count {
            0 => Cow::Borrowed("(no changes)"),
            1 => Cow::Borrowed(only),
            _ => Cow::Owned(
                kind.bitmasks()
                    .iter()
                    .filter(|bitmask| changed(bitmask.mask))
                    .map(|bitmask| bitmask.description)
                    .collect::<Vec<_>>()
                    .join(" / "),
            ),
        }
    }
}

impl Default for RegisterAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RegisterAnalyzer {
    /// The 1 KiB state array is noise; summarise the cursor position instead.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterAnalyzer")
            .field("applied", &self.applied)
            .field("bank", &self.bank)
            .finish_non_exhaustive()
    }
}

/// The register kind a write addresses, using the **hardware-correct**
/// precedence: a high-bank write resolves to the high-bank register if one
/// exists (`0x104` Four-Operator Enable, `0x105` OPL3 Mode Enable), otherwise to
/// the shared register.
///
/// This is the reverse of [`Song::instruction_description`], which resolves the
/// low bank first to feed the table's "all register options" column. It matches
/// the Python `DRODetailedRegisterAnalyzer`, which tried `0x100 | reg` first.
fn kind_for(bank: Bank, reg: u8) -> Option<RegisterKind> {
    let reg = u16::from(reg);
    match bank {
        Bank::High => regdata::register_kind(0x100 | reg).or_else(|| regdata::register_kind(reg)),
        Bank::Low => regdata::register_kind(reg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io;
    use crate::song::fixtures::{dro_song_v1, dro_song_v2};
    use crate::song::{DroDataV1, OplType};

    const DRO_V2_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up_dro2.dro");
    const VGM_FIXTURE: &[u8] = include_bytes!("../../../tests/lsl3_score_up.vgm");

    /// A v1 stream exercising every description path: a first write (all fields),
    /// a repeat (no changes), a partial change, a high-bank `0x04` (Four-Operator
    /// Enable, *not* Timer Control), a separate low-bank `0x04` (Timer Control),
    /// an unknown register, and a delay.
    fn synthetic_v1() -> Song {
        let data = DroDataV1::new(vec![
            0x20, 0x01, // 0: reg 0x20 = 0x01 (low)  -- first write, all fields
            0x20, 0x01, // 1: reg 0x20 = 0x01 (low)  -- no changes
            0x20, 0x11, // 2: reg 0x20 = 0x11 (low)  -- only the KSR bit flips
            0x03, // 3: bank switch high
            0x04, 0x04, 0x01, // 4: reg 0x04 = 0x01 (high) -- Four-Operator Enable
            0x02, // 5: bank switch low
            0x04, 0x04, 0x01, // 6: reg 0x04 = 0x01 (low)  -- Timer Control
            0x50, 0x00, // 7: reg 0x50 = 0x00 (low)  -- Key Scale Level / Output Level
            0xFF, 0x99, // 8: reg 0xFF = 0x99 (low)  -- unknown register
            0x00, 0xB0, // 9: short delay, 0xB0 + 1 = 177 ms
        ])
        .expect("synthetic stream is well-formed v1");
        Song::dro_v1("synthetic.dro".to_owned(), data, 177, OplType::Opl3)
    }

    /// An independent oracle: the same algorithm written plainly, with a
    /// `HashMap` for state, sharing no code with [`RegisterAnalyzer`].
    fn reference_rows(song: &Song) -> Vec<(Bank, String)> {
        use std::collections::HashMap;

        let mut bank = Bank::Low;
        let mut state: HashMap<u16, u8> = HashMap::new();
        let mut out = Vec::with_capacity(song.len());
        for index in 0..song.len() {
            let instruction = song.instruction(index).unwrap();
            if let Some(selected) = instruction.selected_bank() {
                bank = selected;
            }
            let description = match instruction {
                DroInstruction::DelayMs { ms, .. } => format!("Delay: {ms} ms"),
                DroInstruction::DelaySamples { samples, .. } => format!("Delay: {samples} smp"),
                DroInstruction::BankSwitch(_) => format!("Bank switch: {}", bank.name()),
                DroInstruction::Register { reg, value, .. } => {
                    let kind = match bank {
                        Bank::High => regdata::register_kind(0x100 | u16::from(reg))
                            .or_else(|| regdata::register_kind(u16::from(reg))),
                        Bank::Low => regdata::register_kind(u16::from(reg)),
                    };
                    match kind {
                        None => format!("Unknown register: {reg}"),
                        Some(kind) => {
                            let key = (u16::from(bank.index()) << 8) | u16::from(reg);
                            let old = state.insert(key, value);
                            let changed: Vec<&str> = kind
                                .bitmasks()
                                .iter()
                                .filter(|bm| match old {
                                    None => true,
                                    Some(o) => (o ^ value) & bm.mask != 0,
                                })
                                .map(|bm| bm.description)
                                .collect();
                            if changed.is_empty() {
                                "(no changes)".to_owned()
                            } else {
                                changed.join(" / ")
                            }
                        }
                    }
                }
            };
            out.push((bank, description));
        }
        out
    }

    fn as_pairs(rows: &[RowAnalysis]) -> Vec<(Bank, &str)> {
        rows.iter()
            .map(|row| (row.bank, &*row.description))
            .collect()
    }

    #[test]
    fn synthetic_stream_descriptions_are_exact() {
        let song = synthetic_v1();
        let rows = RegisterAnalyzer::analyze_all(&song);
        let expected: [(Bank, &str); 10] = [
            (
                Bank::Low,
                "Tremolo / Vibrato / Sustain / KSR (envelope scaling) / Frequency Multiplication Factor",
            ),
            (Bank::Low, "(no changes)"),
            (Bank::Low, "KSR (envelope scaling)"),
            (Bank::High, "Bank switch: high"),
            (
                Bank::High,
                "4-Operator enable for ch. 11 & 14 / 4-Operator enable for ch. 10 & 13 / \
                 4-Operator enable for ch. 9 & 12 / 4-Operator enable for ch. 2 & 5 / \
                 4-Operator enable for ch. 1 & 4 / 4-Operator enable for ch. 0 & 3",
            ),
            (Bank::Low, "Bank switch: low"),
            (
                Bank::Low,
                "IRQ Reset / Timer 1 Mask / Timer 2 Mask / Timer 1 Start / Timer 2 Start",
            ),
            (Bank::Low, "Key Scale Level / Output Level"),
            (Bank::Low, "Unknown register: 255"),
            (Bank::Low, "Delay: 177 ms"),
        ];
        assert_eq!(as_pairs(&rows), expected.to_vec());
    }

    /// The load-bearing acceptance test: the cursor equals a full independent
    /// linear scan for every instruction of the DRO fixture, and gives the same
    /// answer no matter what order rows are queried in.
    #[test]
    fn cursor_matches_an_independent_reference_on_the_dro_fixture() {
        let song = io::read_song("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
        let reference_owned = reference_rows(&song);
        let reference: Vec<(Bank, &str)> = reference_owned
            .iter()
            .map(|(bank, description)| (*bank, description.as_str()))
            .collect();

        // (a) linear forward scan.
        assert_eq!(as_pairs(&RegisterAnalyzer::analyze_all(&song)), reference);

        // (b) strictly backwards -- forces a reset and replay on every single row.
        let mut analyzer = RegisterAnalyzer::new();
        for index in (0..song.len()).rev() {
            let row = analyzer.row(&song, index).unwrap();
            assert_eq!(
                (row.bank, &*row.description),
                reference[index],
                "reverse row {index}"
            );
        }

        // (c) a deterministic mix of forward runs and backward jumps.
        let len = song.len();
        let probes = [
            0,
            1,
            2,
            len / 2,
            3,
            len - 1,
            len / 2,
            0,
            len - 1,
            len / 3,
            len / 3 + 1,
        ];
        let mut analyzer = RegisterAnalyzer::new();
        for &index in probes.iter().filter(|&&i| i < len) {
            let row = analyzer.row(&song, index).unwrap();
            assert_eq!(
                (row.bank, &*row.description),
                reference[index],
                "probe row {index}"
            );
        }
    }

    #[test]
    fn analyze_all_matches_the_reference_on_the_small_fixtures() {
        for song in [dro_song_v1(), dro_song_v2()] {
            let reference = reference_rows(&song);
            let reference: Vec<(Bank, &str)> = reference
                .iter()
                .map(|(bank, description)| (*bank, description.as_str()))
                .collect();
            assert_eq!(as_pairs(&RegisterAnalyzer::analyze_all(&song)), reference);
        }
    }

    #[test]
    fn high_bank_and_low_bank_zero_four_track_separate_state() {
        // Row 4 (high 0x04) and row 6 (low 0x04) both write 0x01, and both report
        // *all* their fields, because they key different state slots -- 0x104 vs
        // 0x04. And they resolve to different registers entirely.
        let rows = RegisterAnalyzer::analyze_all(&synthetic_v1());
        assert!(rows[4].description.starts_with("4-Operator enable"));
        assert!(rows[6].description.starts_with("IRQ Reset"));
    }

    #[test]
    fn sample_delays_are_described_in_smp() {
        let song = io::read_song("lsl3_score_up.vgm", VGM_FIXTURE).unwrap();
        let rows = RegisterAnalyzer::analyze_all(&song);
        let (index, samples) = (0..song.len())
            .find_map(|i| match song.instruction(i).unwrap() {
                DroInstruction::DelaySamples { samples, .. } => Some((i, samples)),
                _ => None,
            })
            .expect("the VGM fixture contains sample delays");
        assert_eq!(
            &*rows[index].description,
            format!("Delay: {samples} smp").as_str()
        );
    }

    #[test]
    fn out_of_range_row_is_none() {
        let song = dro_song_v2();
        assert!(RegisterAnalyzer::new().row(&song, song.len()).is_none());
        assert!(RegisterAnalyzer::new().row(&song, 999).is_none());
    }

    #[test]
    fn reset_lets_the_cursor_be_reused() {
        let song = synthetic_v1();
        let mut analyzer = RegisterAnalyzer::new();
        let first: Vec<_> = (0..song.len())
            .map(|i| analyzer.row(&song, i).unwrap())
            .collect();
        analyzer.reset();
        let again: Vec<_> = (0..song.len())
            .map(|i| analyzer.row(&song, i).unwrap())
            .collect();
        assert_eq!(first, again);
    }

    /// A Step-3 acceptance marker: the delay sums the analyser's rows describe
    /// agree with the header length the prefix sum reports.
    #[test]
    fn delay_sums_match_the_fixture_header() {
        let song = io::read_song("lsl3_score_up_dro2.dro", DRO_V2_FIXTURE).unwrap();
        assert_eq!(song.total_delay_ms(), song.ms_length);
    }
}
