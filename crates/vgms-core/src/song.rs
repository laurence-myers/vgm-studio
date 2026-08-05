//! The song model: a header, an instruction stream, and a cumulative-delay index.

pub mod dro_data;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod instruction;
pub(crate) mod splice;

use core::fmt;

pub use dro_data::{DroDataV1, DroDataV2};
pub use instruction::{Bank, DelayKind, FindTarget, Instruction, ParseFindTargetError};
pub use splice::InsertEntry;

use crate::regdata;
use crate::util::{VGM_SAMPLE_RATE, smp_to_ms};
use crate::vgm::{VgmData, VgmMeta};

pub const DRO_FILE_V1: u32 = 1;
pub const DRO_FILE_V2: u32 = 2;

/// The instruction stream, in whichever encoding it was read.
///
/// A closed enum fits: there are exactly three encodings, and dispatch on the
/// table-paint path becomes a jump rather than a vtable call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SongData {
    V1(DroDataV1),
    V2(DroDataV2),
    Vgm(VgmData),
}

/// A whole instruction stream, and everything a song derives from it: the header
/// length a DRO stores, and the two loop markers a VGM does.
///
/// What every stream-rebuilding edit produces and what
/// [`ReplaceStream`](crate::undo::ReplaceStream) snapshots to undo one, so the
/// optimiser ([`OptimizeOutcome`](crate::OptimizeOutcome)) and the crop edits
/// ([`CropOutcome`](crate::CropOutcome)) share a single install-and-revert path
/// rather than each growing its own.
#[derive(Debug, Clone)]
pub struct StreamSnapshot {
    pub data: SongData,
    /// A DRO's header length. A VGM's is derived from its sample delays, so the
    /// value here is ignored and recomputed when the stream is installed.
    pub ms_length: u32,
    /// The loop markers, VGM only.
    pub loop_point: Option<usize>,
    pub loop_end: Option<usize>,
}

impl SongData {
    /// The number of instructions.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::V1(data) => data.len(),
            Self::V2(data) => data.len(),
            Self::Vgm(data) => data.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::V1(data) => data.is_empty(),
            Self::V2(data) => data.is_empty(),
            Self::Vgm(data) => data.is_empty(),
        }
    }

    /// Decodes the instruction at `index`, or `None` if out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Instruction> {
        match self {
            Self::V1(data) => data.get(index),
            Self::V2(data) => data.get(index),
            Self::Vgm(data) => data.get(index),
        }
    }

    /// Iterates the decoded instructions. Allocates nothing.
    pub fn iter(&self) -> impl Iterator<Item = Instruction> + '_ {
        (0..self.len()).map(move |index| {
            self.get(index)
                .expect("the index map and the raw bytes agree by construction")
        })
    }

    /// The whole instruction stream, exactly as it sits in the file.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        match self {
            Self::V1(data) => data.raw(),
            Self::V2(data) => data.raw(),
            Self::Vgm(data) => data.raw(),
        }
    }

    #[must_use]
    pub fn raw_len(&self) -> usize {
        self.raw().len()
    }

    /// The raw bytes of one instruction. This is what undo captures.
    #[must_use]
    pub fn raw_instruction(&self, index: usize) -> Option<&[u8]> {
        match self {
            Self::V1(data) => data.raw_instruction(index),
            Self::V2(data) => data.raw_instruction(index),
            Self::Vgm(data) => data.raw_instruction(index),
        }
    }

    /// Removes the instructions at `indices` in a single compaction pass.
    ///
    /// `indices` need not be sorted or unique. This is `O(n)` regardless of how
    /// fragmented the selection is.
    pub fn delete_many(&mut self, indices: &[usize]) {
        match self {
            Self::V1(data) => data.delete_many(indices),
            Self::V2(data) => data.delete_many(indices),
            Self::Vgm(data) => data.delete_many(indices),
        }
    }

    /// Re-inserts previously deleted instructions at their original indices.
    ///
    /// `entries` must be sorted ascending by index, as [`Self::delete_many`]
    /// captured them. This is the exact inverse of a `delete_many` with the same
    /// indices.
    pub fn insert_many(&mut self, entries: &[InsertEntry]) {
        match self {
            Self::V1(data) => data.insert_many(entries),
            Self::V2(data) => data.insert_many(entries),
            Self::Vgm(data) => data.insert_many(entries),
        }
    }

    /// Whether delays in this stream are counted in samples rather than milliseconds.
    #[must_use]
    pub const fn delays_in_samples(&self) -> bool {
        matches!(self, Self::Vgm(_))
    }
}

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
/// The prefix is built at load, in one cheap pass.
///
/// VGM counts its delays in samples, not milliseconds. Those are accumulated in
/// samples and converted once per entry, so per-instruction rounding cannot drift
/// over a long song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    pub file_type: SongFileType,
    /// `1` or `2` for DRO; the BCD version for VGM, e.g. `0x151`.
    pub file_version: u32,
    pub name: String,
    pub opl_type: OplType,
    /// The length recorded in the file header. Not necessarily equal to
    /// [`Song::total_delay_ms`] -- a mismatch is what the trim warning reports.
    ///
    /// VGM files carry a sample count instead, so this is derived for them and
    /// the two always agree.
    pub ms_length: u32,
    data: SongData,
    delay_prefix: Vec<u32>,
    /// `Some` exactly when `data` is [`SongData::Vgm`].
    vgm: Option<Box<VgmMeta>>,
}

impl Song {
    #[must_use]
    pub fn new(
        file_type: SongFileType,
        file_version: u32,
        name: String,
        data: SongData,
        ms_length: u32,
        opl_type: OplType,
    ) -> Self {
        Self::with_vgm_meta(
            file_type,
            file_version,
            name,
            data,
            ms_length,
            opl_type,
            None,
        )
    }

    fn with_vgm_meta(
        file_type: SongFileType,
        file_version: u32,
        name: String,
        data: SongData,
        ms_length: u32,
        opl_type: OplType,
        vgm: Option<Box<VgmMeta>>,
    ) -> Self {
        debug_assert_eq!(
            data.delays_in_samples(),
            vgm.is_some(),
            "VGM metadata must accompany a VGM stream, and only a VGM stream"
        );
        let mut song = Self {
            file_type,
            file_version,
            name,
            opl_type,
            ms_length,
            data,
            delay_prefix: Vec::new(),
            vgm,
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
            SongData::V1(data),
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
            SongData::V2(data),
            ms_length,
            opl_type,
        )
    }

    /// A VGM song. `ms_length` is derived from the command stream, so the header's
    /// sample count is only ever a cross-check.
    #[must_use]
    pub fn vgm(
        name: String,
        file_version: u32,
        data: VgmData,
        opl_type: OplType,
        meta: VgmMeta,
    ) -> Self {
        Self::with_vgm_meta(
            SongFileType::Vgm,
            file_version,
            name,
            SongData::Vgm(data),
            0, // replaced by `rebuild_delay_prefix`
            opl_type,
            Some(Box::new(meta)),
        )
    }

    #[must_use]
    pub fn data(&self) -> &SongData {
        &self.data
    }

    /// The VGM-only header fields, or `None` for a DRO song.
    #[must_use]
    pub fn vgm_meta(&self) -> Option<&VgmMeta> {
        self.vgm.as_deref()
    }

    /// The VGM-only header fields, for the metadata dialog to edit.
    pub fn vgm_meta_mut(&mut self) -> Option<&mut VgmMeta> {
        self.vgm.as_deref_mut()
    }

    #[must_use]
    pub const fn is_vgm(&self) -> bool {
        self.vgm.is_some()
    }

    /// The number of instructions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[must_use]
    pub fn instruction(&self, index: usize) -> Option<Instruction> {
        self.data.get(index)
    }

    // -- timing ------------------------------------------------------------

    /// The summed delay of every instruction, in milliseconds.
    ///
    /// It is the last entry of the prefix sum.
    #[must_use]
    pub fn total_delay_ms(&self) -> u32 {
        *self
            .delay_prefix
            .last()
            .expect("the prefix always has len() + 1 entries")
    }

    /// The summed delay of every instruction, in samples at 44100 Hz.
    ///
    /// Only VGM streams carry sample delays, so this is `0` for a DRO song. It is
    /// what the VGM header's `total # samples` field must say.
    #[must_use]
    pub fn total_delay_samples(&self) -> u32 {
        self.samples_before(self.len())
    }

    /// The summed delay of instructions `[0, index)`, in samples at 44100 Hz.
    #[must_use]
    pub fn samples_before(&self, index: usize) -> u32 {
        self.data
            .iter()
            .take(index)
            .map(Instruction::delay_samples)
            .fold(0u32, u32::saturating_add)
    }

    /// Cumulative delay in samples: element `i` is the number of samples elapsed
    /// before instruction `i`, and the last element (index `len`) is the song's
    /// total. Lets a caller derive a loop length for any candidate loop point
    /// (`prefix[len] - prefix[loop_point]`) without re-walking the song each time.
    #[must_use]
    pub fn delay_samples_prefix(&self) -> Vec<u32> {
        let mut prefix = Vec::with_capacity(self.len() + 1);
        let mut acc = 0u32;
        prefix.push(acc);
        for instruction in self.data.iter() {
            acc = acc.saturating_add(instruction.delay_samples());
            prefix.push(acc);
        }
        prefix
    }

    /// The number of samples in one loop: from the loop point to
    /// [`VgmMeta::loop_end`](crate::VgmMeta::loop_end), or to the end of the song
    /// when no end is set. `None` for a DRO song, or a VGM that does not loop.
    ///
    /// This is the VGM header's `loop # samples`. Deriving it, rather than carrying
    /// the header's copy, is what stops a trim inside the loop from leaving it
    /// stale.
    #[must_use]
    pub fn loop_num_samples(&self) -> Option<u32> {
        let meta = self.vgm.as_deref()?;
        let loop_point = meta.loop_point?;
        let end = meta.loop_end.unwrap_or_else(|| self.len());
        // Saturating: the editor's own paths keep `loop_end` above `loop_point`,
        // but a hand-set pair must never underflow the writer into a panic.
        Some(
            self.samples_before(end)
                .saturating_sub(self.samples_before(loop_point)),
        )
    }

    /// The time at which instruction `index` is executed, in milliseconds.
    #[must_use]
    pub fn ms_offset_at(&self, index: usize) -> Option<u32> {
        self.delay_prefix.get(index).copied()
    }

    /// The instruction a seek to `target_ms` lands on.
    ///
    /// Playback resumes *before* the target when the target falls inside a delay,
    /// stopping on that delay rather than overshooting. Where the target lands
    /// exactly on an instruction boundary, this returns the *first* instruction at
    /// that timestamp.
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
    /// This does not depend on any background analysis, so clicking the waveform
    /// works the instant a file is loaded.
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

    /// The register column: `"DLYS"`, `"DLYL"`, `"BANK"`, or bare hex `"2A"`.
    #[must_use]
    pub fn register_display(&self, index: usize) -> Option<String> {
        self.data.get(index).map(Instruction::register_display)
    }

    /// The value column: `"177 ms"`, `"176 smp"`, `"low"` / `"high"`, or bare
    /// hex with the decimal, `"2A (42)"`.
    #[must_use]
    pub fn value_display(&self, index: usize) -> Option<String> {
        self.data.get(index).map(Instruction::value_display)
    }

    /// The description column, before detailed analysis has run.
    #[must_use]
    pub fn instruction_description(&self, index: usize) -> Option<&'static str> {
        self.data.get(index).map(Instruction::description)
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
        let mut sorted: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.len())
            .collect();
        sorted.sort_unstable();
        sorted.dedup();

        self.move_loop_markers_past_deletion(&sorted);
        self.data.delete_many(&sorted);
        self.rebuild_delay_prefix();
    }

    pub(crate) fn insert_instructions(&mut self, entries: &[InsertEntry]) {
        self.data.insert_many(entries);
        self.rebuild_delay_prefix();
    }

    /// Snapshots the whole stream and everything derived from it, so an edit can
    /// put it back exactly ([`ReplaceStream`](crate::undo::ReplaceStream)).
    pub(crate) fn capture_stream(&self) -> StreamSnapshot {
        StreamSnapshot {
            data: self.data.clone(),
            ms_length: self.ms_length,
            loop_point: self.vgm_meta().and_then(|meta| meta.loop_point),
            loop_end: self.vgm_meta().and_then(|meta| meta.loop_end),
        }
    }

    /// Replaces the whole instruction stream, and everything a song derives from
    /// it.
    ///
    /// This is how an edit that rebuilds the stream wholesale installs the
    /// result: the optimiser, whose merge pass re-encodes delay runs, and the
    /// crop edits, which splice a state patch in among the survivors. Neither is
    /// expressible as a delete and an insert.
    ///
    /// The new data must be the same encoding as the old: a song does not change
    /// format under an edit.
    pub(crate) fn replace_data(&mut self, stream: StreamSnapshot) {
        let StreamSnapshot {
            data,
            ms_length,
            loop_point,
            loop_end,
        } = stream;
        debug_assert_eq!(
            core::mem::discriminant(&self.data),
            core::mem::discriminant(&data),
            "an edit must not change a song's encoding"
        );
        self.data = data;
        match self.vgm.as_deref_mut() {
            Some(meta) => {
                meta.loop_point = loop_point;
                meta.loop_end = loop_end;
            }
            // A DRO: no loop to remap, and a header length only the caller knows.
            // The rebuild below would leave a VGM's derived length correct anyway.
            None => self.ms_length = ms_length,
        }
        self.rebuild_delay_prefix();
    }

    /// Slides a VGM's loop markers left by however many instructions before them
    /// are about to be deleted.
    ///
    /// If the loop instruction itself is deleted, the loop point lands on whatever
    /// now occupies its slot -- the next surviving instruction. If nothing survives
    /// at or after it, the file no longer loops. The end marker follows the same
    /// arithmetic, but its `None` means "the end of the song", so a deletion that
    /// consumes everything from it onward simply restores that default.
    ///
    /// `sorted` must be ascending, unique and in range.
    fn move_loop_markers_past_deletion(&mut self, sorted: &[usize]) {
        let surviving = self.len() - sorted.len();
        let Some(meta) = self.vgm.as_deref_mut() else {
            return;
        };
        let Some(loop_point) = meta.loop_point else {
            return;
        };

        let Some(moved_point) = slide_index_past_deletion(loop_point, sorted, surviving) else {
            log::warn!(
                "the VGM loop point, and everything after it, was deleted; the song no longer loops"
            );
            // No loop, no end: a surviving end marker would describe a region
            // that no longer has a start.
            meta.loop_point = None;
            meta.loop_end = None;
            return;
        };
        meta.loop_point = Some(moved_point);
        // An end that slid onto the loop point leaves no region at all (the whole
        // loop was deleted); fall back to the end of the song rather than keep an
        // empty one.
        meta.loop_end = meta
            .loop_end
            .and_then(|end| slide_index_past_deletion(end, sorted, surviving))
            .filter(|&end| end > moved_point);
    }

    /// Rebuilds the exclusive prefix sum of delays, in milliseconds.
    ///
    /// For a VGM, also refreshes [`Song::ms_length`], which is derived rather than
    /// stored: its header records samples, not milliseconds.
    fn rebuild_delay_prefix(&mut self) {
        self.delay_prefix.clear();
        self.delay_prefix.reserve(self.data.len() + 1);
        self.delay_prefix.push(0);

        if self.data.delays_in_samples() {
            // Accumulate in samples and convert each running total once, so the
            // rounding cannot compound. Converting each delay separately would
            // drift by up to half a millisecond per delay.
            let mut samples = 0u64;
            for instruction in self.data.iter() {
                samples += u64::from(instruction.delay_samples());
                let clamped = u32::try_from(samples).unwrap_or(u32::MAX);
                self.delay_prefix.push(smp_to_ms(clamped, VGM_SAMPLE_RATE));
            }
            self.ms_length = self.total_delay_ms();
        } else {
            let mut elapsed = 0u32;
            for instruction in self.data.iter() {
                elapsed = elapsed.saturating_add(instruction.delay_ms());
                self.delay_prefix.push(elapsed);
            }
        }
    }
}

/// Slides a stored instruction index left past the deletion of `sorted`, or
/// `None` when nothing survives at or after it.
///
/// The shared primitive behind every index the song stores about itself -- the
/// two VGM loop markers, and the UI's own loop-region markers. Anything else that
/// comes to reference instructions by index should reuse this rather than
/// re-derive the arithmetic, so all of them move identically.
///
/// `sorted` must be ascending, unique and in range; `surviving` is the number of
/// instructions left after the deletion.
pub fn slide_index_past_deletion(
    index: usize,
    sorted: &[usize],
    surviving: usize,
) -> Option<usize> {
    // At most `index` of the deletions are below it, so this cannot underflow.
    let moved = index - sorted.partition_point(|&deleted| deleted < index);
    (moved < surviving).then_some(moved)
}

/// The description of a register write: the low-bank table first, then the
/// high-bank table only if the write itself selected the high bank.
fn register_description(reg: u8, bank: Option<Bank>) -> &'static str {
    regdata::register_description(u16::from(reg))
        .or_else(|| match bank {
            Some(Bank::High) => regdata::register_description(0x100 | u16::from(reg)),
            _ => None,
        })
        .unwrap_or("(unknown)")
}

/// The three instruction-table cells that read only the decoded instruction --
/// no chip-state replay, no song context. They live here rather than on
/// `Instruction` itself so they can share `register_description`, and they are
/// what lets a row be drawn from a decoded VGM command (an OPL VGM, since k-4)
/// exactly as from a DRO's own instruction.
impl Instruction {
    /// The register column: `"DLYS"`, `"DLYL"`, `"BANK"`, or bare hex `"2A"`.
    #[must_use]
    pub fn register_display(self) -> String {
        match self {
            Self::BankSwitch(_) => "BANK".to_owned(),
            Self::Register { reg, .. } => format!("{reg:02X}"),
            Self::DelayMs { kind, .. } | Self::DelaySamples { kind, .. } => kind.token().to_owned(),
        }
    }

    /// The value column: `"177 ms"`, `"176 smp"`, `"low"` / `"high"`, or bare hex
    /// with the decimal, `"2A (42)"`.
    #[must_use]
    pub fn value_display(self) -> String {
        match self {
            Self::DelayMs { ms, .. } => format!("{ms} ms"),
            Self::DelaySamples { samples, .. } => format!("{samples} smp"),
            Self::BankSwitch(bank) => bank.name().to_owned(),
            Self::Register { value, .. } => format!("{value:02X} ({value})"),
        }
    }

    /// The description column before detailed analysis runs. Every answer is a
    /// string literal, so this allocates nothing.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::DelayMs { kind, .. } | Self::DelaySamples { kind, .. } => kind.description(),
            Self::BankSwitch(Bank::Low) => "Switch to low registers (Dual OPL-2 / OPL-3)",
            Self::BankSwitch(Bank::High) => "Switch to high registers (Dual OPL-2 / OPL-3)",
            Self::Register { reg, bank, .. } => register_description(reg, bank),
        }
    }
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
    fn delay_samples_prefix_matches_samples_before() {
        let song = dro_song_v2();
        let prefix = song.delay_samples_prefix();
        assert_eq!(prefix.len(), song.len() + 1);
        assert_eq!(prefix[0], 0);
        for (index, &prefixed) in prefix.iter().enumerate() {
            assert_eq!(prefixed, song.samples_before(index), "at {index}");
        }
        assert_eq!(*prefix.last().unwrap(), song.total_delay_samples());
    }

    #[test]
    fn register_display_is_correct() {
        let song = dro_song_v2();
        assert_eq!(song.register_display(0).unwrap(), "10");
        assert_eq!(song.register_display(1).unwrap(), "30");
        assert_eq!(song.register_display(2).unwrap(), "50");
        assert_eq!(song.register_display(5).unwrap(), "DLYS");
        assert_eq!(song.register_display(6).unwrap(), "DLYL");
        assert_eq!(song.register_display(14), None);
    }

    #[test]
    fn value_display_is_correct() {
        let song = dro_song_v2();
        assert_eq!(song.value_display(0).unwrap(), "01 (1)");
        assert_eq!(song.value_display(1).unwrap(), "03 (3)");
        assert_eq!(song.value_display(2).unwrap(), "05 (5)");
        assert_eq!(song.value_display(5).unwrap(), "177 ms");
        assert_eq!(song.value_display(6).unwrap(), "49408 ms");
        assert_eq!(song.value_display(14), None);
    }

    #[test]
    fn instruction_description_is_correct() {
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
        // 0x04 resolves in the low table first, even from the high bank.
        assert_eq!(
            register_description(0x04, Some(Bank::High)),
            "1: Timer Control Flags (IRQ Reset / Mask / Start)   2: Four-Operator Enable"
        );
    }

    #[test]
    fn find_next_instruction_is_correct() {
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

        // Halfway through lands on instruction 7, at half the song length.
        assert_eq!(
            song.index_and_ms_offset_at_pct(0.5),
            Some((7, SONG_LENGTH / 2))
        );
        assert_eq!(song.index_and_ms_offset_at_pct(0.0), Some((0, 0)));

        // Instruction 6 is the first long delay, spanning [177, 49585): a click
        // 25% of the way in lands inside it.
        assert_eq!(song.index_and_ms_offset_at_pct(0.25), Some((6, 177)));

        // At 100% the last instruction is the final long delay, which *begins* at
        // 49762 ms. The offsets are an exclusive sum, so 49762 is the elapsed
        // time on reaching that row.
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

    /// The prefix-sum search must land where a linear scan would, given the same
    /// (real, exclusive) offsets.
    #[test]
    fn pct_search_matches_a_linear_reference() {
        let song = dro_song_v2();
        let offsets: Vec<u32> = (0..song.len())
            .map(|i| song.ms_offset_at(i).unwrap())
            .collect();

        for step in 0..=1000 {
            let pct = f64::from(step) / 1000.0;
            let target = f64::from(song.total_delay_ms()) * pct;

            // A linear reference walk, seeded at the proportional guess.
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

    /// `seek_index_for_ms` must land exactly where a step-by-step seek loop
    /// would, for every reachable target.
    #[test]
    fn seek_index_matches_expected() {
        let song = dro_song_v2();
        for target in 0..=SONG_LENGTH {
            // A step-by-step reference seek.
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
        // The output prints the bare OPL type name, e.g. `OPL3`.
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
