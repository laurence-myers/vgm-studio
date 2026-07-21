//! On-demand detailed register analysis.
//!
//! Of the `(bank, description, ms_offset)` fields an instruction needs, two do
//! not need an analyser at all: the ms offset is [`Song::ms_offset_at`] (the
//! delay prefix sum, built at load), and the bank falls out of tracking bank
//! switches. What is left is the *changed-bits* description for the instruction
//! table -- which fields of a register a write actually altered.
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
use std::collections::{BTreeMap, BTreeSet};
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
    /// `0xFF`).
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
    /// once -- tests, or a caller that prefers to precompute everything. The GUI
    /// queries `row` per visible line instead.
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
            // Return here *before* recording the value, so an unknown register
            // never populates the state array. The register number is shown in
            // decimal here, unlike the hex `register_display` uses.
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
/// low bank first to feed the table's "all register options" column. Here the
/// high bank is tried first (`0x100 | reg`).
fn kind_for(bank: Bank, reg: u8) -> Option<RegisterKind> {
    let reg = u16::from(reg);
    match bank {
        Bank::High => regdata::register_kind(0x100 | reg).or_else(|| regdata::register_kind(reg)),
        Bank::Low => regdata::register_kind(reg),
    }
}

/// Which registers, and which percussion voices, a song writes.
///
/// `dro_split` uses this to skip channels a song never touches. Keys are
/// `(bank << 8) | reg`; the bank is tracked across DRO v1 bank switches and DRO
/// v2 / VGM per-write banks.
#[derive(Debug, Clone, Default)]
pub struct RegisterUsage {
    counts: BTreeMap<u16, u32>,
    percussion: BTreeSet<u16>,
}

impl RegisterUsage {
    /// Counts every register write, keyed by `(bank << 8) | reg`.
    ///
    /// With `detailed_percussion`, also records which percussion bits were ever
    /// set in a write to `0xBD`, keyed by `(bank << 8) | bitmask` -- the map
    /// `dro_split`'s `--isolate-percussion` needs. The count is what matters to
    /// the splitter (it only tests for zero), but it is kept exact so tests
    /// asserting a specific count still pin it.
    #[must_use]
    pub fn analyze(song: &Song, detailed_percussion: bool) -> Self {
        let mut usage = Self::default();
        let mut bank = Bank::Low;
        for instruction in song.data().iter() {
            if let Some(selected) = instruction.selected_bank() {
                bank = selected;
            }
            if let DroInstruction::Register { reg, value, .. } = instruction {
                let key = (u16::from(bank.index()) << 8) | u16::from(reg);
                *usage.counts.entry(key).or_default() += 1;
                if detailed_percussion && reg == regdata::PERCUSSION_REGISTER {
                    for bitmask in RegisterKind::PercussionControl.bitmasks() {
                        if value & bitmask.mask != 0 {
                            let perc_key = (u16::from(bank.index()) << 8) | u16::from(bitmask.mask);
                            usage.percussion.insert(perc_key);
                        }
                    }
                }
            }
        }
        usage
    }

    /// How many times register `key` (`(bank << 8) | reg`) was written.
    #[must_use]
    pub fn count(&self, key: u16) -> u32 {
        self.counts.get(&key).copied().unwrap_or(0)
    }

    /// Whether percussion bit `key` (`(bank << 8) | bitmask`) was ever set in a
    /// `0xBD` write.
    #[must_use]
    pub fn percussion_used(&self, key: u16) -> bool {
        self.percussion.contains(&key)
    }
}

/// The per-channel pan byte each melodic channel's **first** `0xC0..=0xC8` write
/// implies, for seeding the GUI's Custom-pan defaults on an OPL3 song.
///
/// Register `0xC0+n` carries the OPL3 speaker-enable bits: bit 4 (`0x10`) routes
/// the channel to the left output, bit 5 (`0x20`) to the right. These map onto the
/// `0x00` (hard left) .. `0x80` (centre) .. `0xFF` (hard right) scale the
/// `stereo-ext` panpots use:
///
/// - left only  -> `0x00`
/// - right only -> `0xFF`
/// - both set, or neither -> `0x80` (centre)
///
/// Indexed `bank.index() * 9 + (reg - 0xC0)` -- slots `0..=8` are the low bank,
/// `9..=17` the high. A channel the song never writes `0xC0` for stays centred
/// (`0x80`), and only the **first** write to each slot is honoured, capturing the
/// song's initial stereo image rather than a later repan.
#[must_use]
pub fn initial_channel_pans(song: &Song) -> [u8; 18] {
    let mut pans = [0x80u8; 18];
    let mut seen = [false; 18];
    let mut bank = Bank::Low;
    for instruction in song.data().iter() {
        if let Some(selected) = instruction.selected_bank() {
            bank = selected;
        }
        if let DroInstruction::Register { reg, value, .. } = instruction
            && (0xC0..=0xC8).contains(&reg)
        {
            let slot = usize::from(bank.index()) * 9 + usize::from(reg - 0xC0);
            if !seen[slot] {
                seen[slot] = true;
                pans[slot] = match (value & 0x10 != 0, value & 0x20 != 0) {
                    (true, false) => 0x00,
                    (false, true) => 0xFF,
                    _ => 0x80,
                };
            }
        }
    }
    pans
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

    // -- register usage -----------------------------------------------------

    #[test]
    fn register_usage_tracks_the_bank_across_switches() {
        // reg 0x20 written twice on the low bank, once on the high bank.
        let song = Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01, 0x03, 0x20, 0x02, 0x02, 0x20, 0x03]).unwrap(),
            0,
            OplType::Opl3,
        );
        let usage = RegisterUsage::analyze(&song, false);
        assert_eq!(usage.count(0x020), 2, "low bank, twice");
        assert_eq!(usage.count(0x120), 1, "high bank, once");
        assert_eq!(usage.count(0x040), 0, "never written");
    }

    #[test]
    fn detailed_percussion_records_only_set_bits() {
        // 0xBD = 0x31 = percussion mode (0x20) | BD (0x10) | HH (0x01).
        let song = Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0xBD, 0x31]).unwrap(),
            0,
            OplType::Opl3,
        );
        let usage = RegisterUsage::analyze(&song, true);
        assert!(usage.percussion_used(0x20));
        assert!(usage.percussion_used(0x10));
        assert!(usage.percussion_used(0x01));
        assert!(!usage.percussion_used(0x08), "SD was not set");
        // Without the detailed pass, no percussion is recorded at all.
        assert!(!RegisterUsage::analyze(&song, false).percussion_used(0x20));
    }

    // -- initial_channel_pans ------------------------------------------------

    #[test]
    fn initial_channel_pans_map_speaker_bits_and_track_the_bank() {
        // Low bank: left-only, right-only, both, neither; a repan of ch0 that must
        // be ignored (first write wins); then a high-bank ch0 right-only write.
        let song = Song::dro_v1(
            "pans.dro".to_owned(),
            DroDataV1::new(vec![
                0xC0, 0x10, // ch0 low: left only  -> 0x00
                0xC1, 0x20, // ch1 low: right only -> 0xFF
                0xC2, 0x30, // ch2 low: both       -> 0x80
                0xC3, 0x00, // ch3 low: neither    -> 0x80
                0xC0, 0x20, // ch0 low again: first-write-wins, ignored
                0x03, // bank switch high
                0xC0, 0x20, // ch0 high: right only -> slot 9 = 0xFF
            ])
            .unwrap(),
            0,
            OplType::Opl3,
        );

        let pans = initial_channel_pans(&song);
        assert_eq!(pans[0], 0x00, "left only");
        assert_eq!(pans[1], 0xFF, "right only");
        assert_eq!(pans[2], 0x80, "both speakers -> centre");
        assert_eq!(pans[3], 0x80, "neither -> centre");
        assert_eq!(pans[9], 0xFF, "high-bank ch0, right only");
        // Every channel the song never wrote 0xC0 for stays centred.
        for (slot, &pan) in pans.iter().enumerate() {
            if matches!(slot, 4..=8 | 10..=17) {
                assert_eq!(pan, 0x80, "unwritten slot {slot} stays centred");
            }
        }
    }

    #[test]
    fn initial_channel_pans_default_to_centre_without_c0_writes() {
        // A song that writes no 0xC0..=0xC8 leaves every channel centred.
        let song = Song::dro_v1(
            "nopan.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01, 0xB0, 0x31]).unwrap(),
            0,
            OplType::Opl2,
        );
        assert_eq!(initial_channel_pans(&song), [0x80; 18]);
    }
}
