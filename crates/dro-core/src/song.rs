//! The song model: a header, an instruction stream, and a cumulative-delay index.

pub mod dro_data;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod instruction;

use core::fmt;

pub use dro_data::{DroData, DroDataV1, DroDataV2, InsertEntry};
pub use instruction::{Bank, DelayKind, DroInstruction, FindTarget, ParseFindTargetError};

use crate::regdata;

pub const DRO_FILE_V1: u16 = 1;
pub const DRO_FILE_V2: u16 = 2;

/// Which container the song was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SongFileType {
    Dro,
    Vgm,
}

impl SongFileType {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dro => "DRO",
            Self::Vgm => "VGM",
        }
    }
}

impl fmt::Display for SongFileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The OPL hardware a capture targets.
///
/// The discriminants are the DRO v2 header's `iHardwareType` codes. DRO v1 uses a
/// different ordering, hence [`OplType::from_v1_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OplType {
    Opl2 = 0,
    DualOpl2 = 1,
    Opl3 = 2,
}

impl OplType {
    /// Every variant, in DRO v2 header order.
    pub const ALL: [Self; 3] = [Self::Opl2, Self::DualOpl2, Self::Opl3];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Opl2 => "OPL2",
            Self::DualOpl2 => "DUAL_OPL2",
            Self::Opl3 => "OPL3",
        }
    }

    /// The DRO v2 header code.
    #[must_use]
    pub const fn v2_code(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn from_v2_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Opl2),
            1 => Some(Self::DualOpl2),
            2 => Some(Self::Opl3),
            _ => None,
        }
    }

    /// The DRO v1 header code. v1 orders the types `(OPL2, OPL3, DUAL_OPL2)`.
    #[must_use]
    pub const fn v1_code(self) -> u8 {
        match self {
            Self::Opl2 => 0,
            Self::Opl3 => 1,
            Self::DualOpl2 => 2,
        }
    }

    #[must_use]
    pub const fn from_v1_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Opl2),
            1 => Some(Self::Opl3),
            2 => Some(Self::DualOpl2),
            _ => None,
        }
    }
}

impl fmt::Display for OplType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A loaded song: header fields, the instruction stream, and a cumulative-delay
/// prefix sum kept in step with it.
///
/// # The delay prefix
///
/// `delay_prefix[i]` is the total delay, in milliseconds, of instructions
/// `[0, i)` -- an *exclusive* prefix sum. It has `len() + 1` entries, so
/// `delay_prefix[len()]` is the song's total delay. Two consequences worth
/// stating, because the whole design leans on them:
///
/// - Instruction `i` is executed at time `delay_prefix[i]`, which is exactly what
///   `seek_to_pos(i)` reports as elapsed. Time and position lookups therefore
///   agree by construction.
/// - It is monotonically non-decreasing, so every lookup is a binary search.
///
/// The Python derived these offsets as a byproduct of the *detailed register
/// analysis*, which meant clicking the waveform did nothing until a background
/// task finished. Here the prefix is built at load, in one cheap pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    pub file_type: SongFileType,
    pub file_version: u16,
    pub name: String,
    pub opl_type: OplType,
    /// The length recorded in the file header. Not necessarily equal to
    /// [`Song::total_delay_ms`] -- a mismatch is what the trim warning reports.
    pub ms_length: u32,
    data: DroData,
    delay_prefix: Vec<u32>,
}

impl Song {
    #[must_use]
    pub fn new(
        file_type: SongFileType,
        file_version: u16,
        name: String,
        data: DroData,
        ms_length: u32,
        opl_type: OplType,
    ) -> Self {
        let mut song = Self {
            file_type,
            file_version,
            name,
            opl_type,
            ms_length,
            data,
            delay_prefix: Vec::new(),
        };
        song.rebuild_delay_prefix();
        song
    }

    #[must_use]
    pub fn dro_v1(name: String, data: DroDataV1, ms_length: u32, opl_type: OplType) -> Self {
        Self::new(
            SongFileType::Dro,
            DRO_FILE_V1,
            name,
            DroData::V1(data),
            ms_length,
            opl_type,
        )
    }

    #[must_use]
    pub fn dro_v2(name: String, data: DroDataV2, ms_length: u32, opl_type: OplType) -> Self {
        Self::new(
            SongFileType::Dro,
            DRO_FILE_V2,
            name,
            DroData::V2(data),
            ms_length,
            opl_type,
        )
    }

    #[must_use]
    pub fn data(&self) -> &DroData {
        &self.data
    }

    /// The number of instructions. (Python: `get_length_data`.)
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[must_use]
    pub fn instruction(&self, index: usize) -> Option<DroInstruction> {
        self.data.get(index)
    }

    // -- timing ------------------------------------------------------------

    /// The summed delay of every instruction, in milliseconds.
    ///
    /// Python walked the whole instruction list for this (`DROTotalDelayCalculator`);
    /// here it is the last entry of the prefix sum.
    #[must_use]
    pub fn total_delay_ms(&self) -> u32 {
        *self
            .delay_prefix
            .last()
            .expect("the prefix always has len() + 1 entries")
    }

    /// The time at which instruction `index` is executed, in milliseconds.
    #[must_use]
    pub fn ms_offset_at(&self, index: usize) -> Option<u32> {
        self.delay_prefix.get(index).copied()
    }

    /// The instruction a seek to `target_ms` lands on.
    ///
    /// Playback resumes *before* the target when the target falls inside a delay,
    /// matching the Python seeker, which broke out of its loop rather than
    /// overshoot. Where the target lands exactly on an instruction boundary, this
    /// returns the *first* instruction at that timestamp.
    ///
    /// The returned index may be `len()`, meaning "past the last instruction".
    #[must_use]
    pub fn seek_index_for_ms(&self, target_ms: u32) -> usize {
        let target = target_ms.min(self.total_delay_ms());
        let first_at_or_after = self.delay_prefix.partition_point(|&offset| offset < target);
        if self.delay_prefix.get(first_at_or_after) == Some(&target) {
            first_at_or_after
        } else {
            // The target fell strictly inside a delay: stop on that delay.
            first_at_or_after.saturating_sub(1)
        }
    }

    /// Maps a position along the waveform (`0.0 ..= 1.0`) to an instruction and
    /// the time at which it plays.
    ///
    /// The returned milliseconds always equal `ms_offset_at(index)`, so selecting
    /// the row and seeking to it agree. Returns `None` for an empty song or a
    /// non-finite percentage.
    ///
    /// Unlike the Python, this does not depend on the background analysis task,
    /// so clicking the waveform works the instant a file is loaded.
    #[must_use]
    pub fn index_and_ms_offset_at_pct(&self, position_pct: f64) -> Option<(usize, u32)> {
        if self.is_empty() || !position_pct.is_finite() {
            return None;
        }
        // Compare in f64: the target rarely lands on a whole millisecond, and
        // rounding first would move the boundary between two instructions.
        let target = f64::from(self.total_delay_ms()) * position_pct.clamp(0.0, 1.0);
        let first_at_or_after = self
            .delay_prefix
            .partition_point(|&offset| f64::from(offset) < target);

        let index = match self.delay_prefix.get(first_at_or_after) {
            Some(&offset) if f64::from(offset) == target => first_at_or_after,
            _ => first_at_or_after.saturating_sub(1),
        };
        let index = index.min(self.len() - 1);
        Some((index, self.delay_prefix[index]))
    }

    // -- searching ---------------------------------------------------------

    /// The next instruction matching `target`, strictly after (or before) `start`.
    ///
    /// Python returned `-1` for "not found" and took the target as a magic string.
    #[must_use]
    pub fn find_next_instruction(
        &self,
        start: usize,
        target: FindTarget,
        look_backwards: bool,
    ) -> Option<usize> {
        let len = self.len();
        let matches = |index: usize| self.data.get(index).is_some_and(|i| target.matches(i));
        if look_backwards {
            (0..start.min(len)).rev().find(|&index| matches(index))
        } else {
            (start.saturating_add(1)..len).find(|&index| matches(index))
        }
    }

    // -- display -----------------------------------------------------------

    /// The register column: `"DLYS"`, `"DLYL"`, `"BANK"`, or `"0x2A"`.
    #[must_use]
    pub fn register_display(&self, index: usize) -> Option<String> {
        Some(match self.data.get(index)? {
            DroInstruction::DelayMs { kind, .. } => kind.token().to_owned(),
            DroInstruction::BankSwitch(_) => "BANK".to_owned(),
            DroInstruction::Register { reg, .. } => format!("0x{reg:02X}"),
        })
    }

    /// The value column: `"177 ms"`, `"low"` / `"high"`, or `"0x2A (42)"`.
    #[must_use]
    pub fn value_display(&self, index: usize) -> Option<String> {
        Some(match self.data.get(index)? {
            DroInstruction::DelayMs { ms, .. } => format!("{ms} ms"),
            DroInstruction::BankSwitch(bank) => bank.name().to_owned(),
            DroInstruction::Register { value, .. } => format!("0x{value:02X} ({value})"),
        })
    }

    /// The description column, before detailed analysis has run.
    ///
    /// Every possible answer is a string literal, so this allocates nothing.
    #[must_use]
    pub fn instruction_description(&self, index: usize) -> Option<&'static str> {
        Some(match self.data.get(index)? {
            DroInstruction::DelayMs { kind, .. } => kind.description(),
            DroInstruction::BankSwitch(Bank::Low) => "Switch to low registers (Dual OPL-2 / OPL-3)",
            DroInstruction::BankSwitch(Bank::High) => {
                "Switch to high registers (Dual OPL-2 / OPL-3)"
            }
            DroInstruction::Register { reg, bank, .. } => register_description(reg, bank),
        })
    }

    #[must_use]
    pub fn pretty_string(&self) -> String {
        format!(
            "Song: {}\nFormat: {} v{}\nOPL Type: {}\nLength (ms): {}",
            self.name, self.file_type, self.file_version, self.opl_type, self.ms_length
        )
    }

    // -- mutation (drive this through `UndoController`) ---------------------

    pub(crate) fn delete_instructions(&mut self, indices: &[usize]) {
        self.data.delete_many(indices);
        self.rebuild_delay_prefix();
    }

    pub(crate) fn insert_instructions(&mut self, entries: &[InsertEntry]) {
        self.data.insert_many(entries);
        self.rebuild_delay_prefix();
    }

    fn rebuild_delay_prefix(&mut self) {
        self.delay_prefix.clear();
        self.delay_prefix.reserve(self.data.len() + 1);
        self.delay_prefix.push(0);
        let mut elapsed = 0u32;
        for instruction in self.data.iter() {
            elapsed = elapsed.saturating_add(instruction.delay_ms());
            self.delay_prefix.push(elapsed);
        }
    }
}

/// The description of a register write, following the Python's lookup order:
/// the low-bank table first, then the high-bank table only if the write itself
/// selected the high bank.
fn register_description(reg: u8, bank: Option<Bank>) -> &'static str {
    regdata::register_description(u16::from(reg))
        .or_else(|| match bank {
            Some(Bank::High) => regdata::register_description(0x100 | u16::from(reg)),
            _ => None,
        })
        .unwrap_or("(unknown)")
}

impl fmt::Display for Song {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}Song[name = '{}', ver = '{}', opl_type = '{}', ms_length = '{}']",
            self.file_type, self.name, self.file_version, self.opl_type, self.ms_length
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{SONG_LENGTH, dro_song_v1, dro_song_v2};
    use super::*;

    #[test]
    fn length_helpers() {
        let song = dro_song_v2();
        assert_eq!(song.len(), 14);
        assert_eq!(song.ms_length, SONG_LENGTH);
        assert!(!song.is_empty());
    }

    #[test]
    fn register_display_matches_python() {
        let song = dro_song_v2();
        assert_eq!(song.register_display(0).unwrap(), "0x10");
        assert_eq!(song.register_display(1).unwrap(), "0x30");
        assert_eq!(song.register_display(2).unwrap(), "0x50");
        assert_eq!(song.register_display(5).unwrap(), "DLYS");
        assert_eq!(song.register_display(6).unwrap(), "DLYL");
        assert_eq!(song.register_display(14), None);
    }

    #[test]
    fn value_display_matches_python() {
        let song = dro_song_v2();
        assert_eq!(song.value_display(0).unwrap(), "0x01 (1)");
        assert_eq!(song.value_display(1).unwrap(), "0x03 (3)");
        assert_eq!(song.value_display(2).unwrap(), "0x05 (5)");
        assert_eq!(song.value_display(5).unwrap(), "177 ms");
        assert_eq!(song.value_display(6).unwrap(), "49408 ms");
        assert_eq!(song.value_display(14), None);
    }

    #[test]
    fn instruction_description_matches_python() {
        let song = dro_song_v2();
        // codemap[0] = 0x10, which the register table has no entry for.
        assert_eq!(song.instruction_description(0).unwrap(), "(unknown)");
        assert_eq!(
            song.instruction_description(1).unwrap(),
            "Tremolo / Vibrato / Sustain / KSR / Frequency Multiplication Factor"
        );
        assert_eq!(
            song.instruction_description(2).unwrap(),
            "Key Scale Level / Output Level"
        );
        assert_eq!(song.instruction_description(5).unwrap(), "Delay (short)");
        assert_eq!(song.instruction_description(6).unwrap(), "Delay (long)");
        assert_eq!(song.instruction_description(14), None);
    }

    #[test]
    fn bank_switch_display() {
        let song = dro_song_v1();
        // The v1 fixture's instructions 3 and 4 are bank switches.
        assert_eq!(song.register_display(3).unwrap(), "BANK");
        assert_eq!(song.value_display(3).unwrap(), "low");
        assert_eq!(song.value_display(4).unwrap(), "high");
        assert_eq!(
            song.instruction_description(3).unwrap(),
            "Switch to low registers (Dual OPL-2 / OPL-3)"
        );
        assert_eq!(
            song.instruction_description(4).unwrap(),
            "Switch to high registers (Dual OPL-2 / OPL-3)"
        );
    }

    #[test]
    fn high_bank_only_registers_resolve_only_in_the_high_bank() {
        // 0x05 has no low-bank entry; 0x105 is "OPL3 Mode Enable".
        assert_eq!(
            register_description(0x05, Some(Bank::High)),
            "OPL3 Mode Enable"
        );
        assert_eq!(register_description(0x05, Some(Bank::Low)), "(unknown)");
        assert_eq!(register_description(0x05, None), "(unknown)");
        // 0x04 resolves in the low table first, even from the high bank --
        // exactly what `get_instruction_description` did.
        assert_eq!(
            register_description(0x04, Some(Bank::High)),
            "1: Timer Control Flags (IRQ Reset / Mask / Start)   2: Four-Operator Enable"
        );
    }

    #[test]
    fn find_next_instruction_matches_python() {
        let song = dro_song_v2();
        let find = |start, target: &str, backwards| {
            song.find_next_instruction(start, target.parse().unwrap(), backwards)
        };

        assert_eq!(find(0, "0x50", false), Some(2));
        assert_eq!(find(0, "0x40", false), None);
        assert_eq!(find(3, "0x50", false), Some(9));
        assert_eq!(find(3, "0x50", true), Some(2));
        assert_eq!(find(0, "0x50", true), None);

        assert_eq!(find(0, "DLYS", false), Some(5));
        assert_eq!(find(0, "DLYL", false), Some(6));
        assert_eq!(find(0, "DALL", false), Some(5));
        assert_eq!(find(5, "DALL", false), Some(6));
        // Bank switches do not exist in DRO v2 files.
        assert_eq!(find(0, "BANK", false), None);
    }

    #[test]
    fn find_next_instruction_finds_bank_switches_in_v1() {
        let song = dro_song_v1();
        let bank = FindTarget::BankSwitch;
        assert_eq!(song.find_next_instruction(0, bank, false), Some(3));
        assert_eq!(song.find_next_instruction(3, bank, false), Some(4));
        assert_eq!(song.find_next_instruction(4, bank, false), None);
        assert_eq!(song.find_next_instruction(6, bank, true), Some(4));
        assert_eq!(song.find_next_instruction(0, bank, true), None);
    }

    // -- the delay prefix --------------------------------------------------

    #[test]
    fn delay_prefix_is_an_exclusive_sum() {
        let song = dro_song_v2();
        // 5 registers, short delay 177, long delay 49408, 5 registers, short, long.
        assert_eq!(
            song.delay_prefix,
            vec![
                0, 0, 0, 0, 0, 0, 177, 49_585, 49_585, 49_585, 49_585, 49_585, 49_585, 49_762,
                99_170
            ]
        );
        assert_eq!(song.total_delay_ms(), SONG_LENGTH);
        assert_eq!(song.total_delay_ms(), song.ms_length);
        assert_eq!(song.ms_offset_at(0), Some(0));
        assert_eq!(song.ms_offset_at(6), Some(177));
        assert_eq!(song.ms_offset_at(14), Some(SONG_LENGTH));
        assert_eq!(song.ms_offset_at(15), None);
    }

    #[test]
    fn delay_prefix_matches_a_linear_scan() {
        for song in [dro_song_v1(), dro_song_v2()] {
            let mut elapsed = 0u32;
            for index in 0..=song.len() {
                assert_eq!(song.ms_offset_at(index), Some(elapsed), "index {index}");
                if let Some(instruction) = song.instruction(index) {
                    elapsed += instruction.delay_ms();
                }
            }
        }
    }

    #[test]
    fn index_and_ms_offset_at_pct() {
        let song = dro_song_v2();

        // Python's `test_get_index_and_ms_offset_by_position_pct` asserted
        // (7, SONG_LENGTH / 2) here, and it still holds.
        assert_eq!(
            song.index_and_ms_offset_at_pct(0.5),
            Some((7, SONG_LENGTH / 2))
        );
        assert_eq!(song.index_and_ms_offset_at_pct(0.0), Some((0, 0)));

        // Instruction 6 is the first long delay, spanning [177, 49585): a click
        // 25% of the way in lands inside it.
        assert_eq!(song.index_and_ms_offset_at_pct(0.25), Some((6, 177)));

        // At 100% the last instruction is the final long delay, which *begins* at
        // 49762 ms. Python's unit test said (13, 99170) only because its
        // hand-written offsets table was an inclusive sum; the real analyser
        // yields exclusive offsets, and 49762 is what `seek_to_pos(13)` elapses.
        assert_eq!(song.index_and_ms_offset_at_pct(1.0), Some((13, 49_762)));
    }

    #[test]
    fn index_and_ms_offset_at_pct_is_self_consistent() {
        let song = dro_song_v2();
        for step in 0..=1000 {
            let pct = f64::from(step) / 1000.0;
            let (index, ms) = song.index_and_ms_offset_at_pct(pct).unwrap();
            assert!(index < song.len());
            // The reported time must be what seeking to that row actually elapses.
            assert_eq!(song.ms_offset_at(index), Some(ms), "pct {pct}");
        }
    }

    #[test]
    fn index_and_ms_offset_at_pct_clamps_and_rejects_nonsense() {
        let song = dro_song_v2();
        assert_eq!(song.index_and_ms_offset_at_pct(-1.0), Some((0, 0)));
        assert_eq!(song.index_and_ms_offset_at_pct(2.0), Some((13, 49_762)));
        assert_eq!(song.index_and_ms_offset_at_pct(f64::NAN), None);
        assert_eq!(song.index_and_ms_offset_at_pct(f64::INFINITY), None);

        let empty = Song::dro_v2(
            "empty.dro".to_owned(),
            DroDataV2::new(vec![], vec![0x10], 0xFE, 0xFF).unwrap(),
            0,
            OplType::Opl3,
        );
        assert_eq!(empty.index_and_ms_offset_at_pct(0.5), None);
        assert_eq!(empty.total_delay_ms(), 0);
    }

    /// The prefix-sum search must land where the Python's linear scan would,
    /// given the same (real, exclusive) offsets.
    #[test]
    fn pct_search_matches_a_linear_reference() {
        let song = dro_song_v2();
        let offsets: Vec<u32> = (0..song.len())
            .map(|i| song.ms_offset_at(i).unwrap())
            .collect();

        for step in 0..=1000 {
            let pct = f64::from(step) / 1000.0;
            let target = f64::from(song.total_delay_ms()) * pct;

            // Verbatim port of the Python walk, seeded at the proportional guess.
            let mut index = ((offsets.len() as f64) * pct).floor() as usize;
            if index == offsets.len() {
                index -= 1;
            }
            let item = f64::from(offsets[index]);
            if item < target {
                while index < offsets.len() - 1 && f64::from(offsets[index + 1]) < target {
                    index += 1;
                }
            } else if item > target {
                while index > 0 && f64::from(offsets[index - 1]) > target {
                    index -= 1;
                }
            }

            let (actual, _) = song.index_and_ms_offset_at_pct(pct).unwrap();
            assert_eq!(actual, index, "pct {pct}");
        }
    }

    #[test]
    fn seek_index_for_ms() {
        let song = dro_song_v2();
        // Before any delay has elapsed, the first instruction.
        assert_eq!(song.seek_index_for_ms(0), 0);
        // Inside the first long delay (177..49585) -> stop on that delay.
        assert_eq!(song.seek_index_for_ms(1000), 6);
        assert_eq!(song.seek_index_for_ms(178), 6);
        // Exactly at a boundary -> the first instruction at that timestamp.
        assert_eq!(song.seek_index_for_ms(177), 6);
        assert_eq!(song.seek_index_for_ms(49_585), 7);
        assert_eq!(song.seek_index_for_ms(49_762), 13);
        // Past the end clamps to the total.
        assert_eq!(song.seek_index_for_ms(SONG_LENGTH), 14);
        assert_eq!(song.seek_index_for_ms(u32::MAX), 14);
    }

    /// `seek_index_for_ms` must land exactly where the Python `DROSeeker`
    /// `seek_to_time` loop would, for every reachable target.
    #[test]
    fn seek_index_matches_the_python_seeker() {
        let song = dro_song_v2();
        for target in 0..=SONG_LENGTH {
            // Verbatim port of DROSeeker.seek_to_time's stepping.
            let mut pos = 0usize;
            let mut elapsed = 0u32;
            while elapsed < target && pos < song.len() {
                let instruction = song.instruction(pos).unwrap();
                let delay = instruction.delay_ms();
                if delay > 0 {
                    if elapsed + delay > target {
                        break;
                    }
                    elapsed += delay;
                }
                pos += 1;
            }
            assert_eq!(song.seek_index_for_ms(target), pos, "target {target} ms");
        }
    }

    // -- text --------------------------------------------------------------

    #[test]
    fn display_and_pretty_string() {
        let song = dro_song_v2();
        // The Python interpolated `OPLType.OPL3` here, leaking an enum repr into
        // console output. Printing the bare name is the only difference.
        assert_eq!(
            song.to_string(),
            format!(
                "DROSong[name = 'test.dro', ver = '2', opl_type = 'OPL3', ms_length = '{SONG_LENGTH}']"
            )
        );
        assert_eq!(
            song.pretty_string(),
            format!("Song: test.dro\nFormat: DRO v2\nOPL Type: OPL3\nLength (ms): {SONG_LENGTH}")
        );
    }

    #[test]
    fn opl_type_codes_differ_between_v1_and_v2() {
        // v1: (OPL2, OPL3, DUAL_OPL2); v2: (OPL2, DUAL_OPL2, OPL3).
        assert_eq!(OplType::from_v1_code(1), Some(OplType::Opl3));
        assert_eq!(OplType::from_v2_code(1), Some(OplType::DualOpl2));
        assert_eq!(OplType::Opl3.v1_code(), 1);
        assert_eq!(OplType::Opl3.v2_code(), 2);
        assert_eq!(OplType::from_v1_code(3), None);
        assert_eq!(OplType::from_v2_code(3), None);
        for opl_type in OplType::ALL {
            assert_eq!(OplType::from_v1_code(opl_type.v1_code()), Some(opl_type));
            assert_eq!(OplType::from_v2_code(opl_type.v2_code()), Some(opl_type));
        }
    }
}
