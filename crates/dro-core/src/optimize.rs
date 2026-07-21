//! The VGM optimiser: strips audibly-redundant OPL writes and merges the delays
//! left behind, the way VGMRips' `vgm_cmp` shrinks a pack's files.
//!
//! # What it removes
//!
//! Every OPL register is a level-sensitive latch: a write matters only when it
//! changes the latched value. Key-on (`0xB0..=0xB8` bit 5, `0xBD` bits 0..=4)
//! retriggers on a 0->1 *transition*, so rewriting an already-set bit with the
//! same value is silent; the timers (`0x02..=0x04`) drive IRQs no VGM player uses
//! for audio. So a write whose value equals the register's cached value is
//! inaudible, and dropping it cannot change the rendered output. The first write
//! to a register is always kept -- power-on defaults are never assumed.
//!
//! This is a chip-facts derivation (YM3812 / YMF262 datasheets, and the vendored
//! nuked-opl3 write path), not a transcription of `vgm_cmp`; the render-parity
//! tests in `dro-synth` are the correctness net.
//!
//! # Loop safety
//!
//! On reaching the loop point the register cache is reset, so the first in-body
//! write to each register is kept (never dropped as "same as before the loop").
//! The loop body then re-establishes every register it touches from its own kept
//! writes, so a wrap that carries the tail's register state back to the loop point
//! -- which is exactly what the engine does at the seam, without resetting the
//! chip -- lands on the same state the optimiser simulated. Stripping inside the
//! loop is therefore safe for both the file-format loop (loop offset -> EOF) and
//! the editor's `[loop_point, loop_end)` seam playback.
//!
//! # Delay merging
//!
//! Dropping a write between two delays leaves them adjacent; a run of adjacent
//! delays is summed and re-encoded compactly. The total is conserved exactly, so
//! playback timing is untouched (and the engine's frame clock carries its
//! fractional remainder, making the sum of two delays render frame-for-frame the
//! same as the merged delay). A lone delay keeps its original bytes, and a run is
//! only re-encoded when that actually saves bytes -- so an already-optimal stream
//! is reproduced verbatim.
//!
//! A single simulation pass is the fixpoint: dropping a same-value write does not
//! change the simulated state, so stripping decisions never invalidate each other
//! (`vgm_cmp` iterates only because its passes interact with encoding sizes). The
//! tests assert idempotence rather than looping.

use crate::song::{DroInstruction, Song, SongData};
use crate::vgm::VgmData;
use crate::vgm::data::command;

/// The largest wait a single `0x61` command can express.
const MAX_WAIT: u64 = 0xFFFF;

/// The number of register files the optimiser tracks: the low file and the high
/// file. For a dual OPL2 the high file is the second chip; for an OPL3 it is
/// port 1. A song is only ever one of those, so keying on the decoded [`Bank`]
/// (which the reader already mapped each write opcode onto) never conflates them.
///
/// [`Bank`]: crate::song::Bank
const FILE_COUNT: usize = 2;
const REGISTER_COUNT: usize = 256;

/// The result of optimising a VGM song: the rebuilt command stream, the loop
/// markers remapped onto it, and what was saved (for the status line).
///
/// [`optimize`] returns `None` when there is nothing to do, so an `OptimizeOutcome`
/// always represents a genuine reduction.
#[derive(Debug, Clone)]
pub struct OptimizeOutcome {
    /// The rebuilt command stream.
    pub data: VgmData,
    /// The loop point, as an instruction index into [`Self::data`].
    pub loop_point: Option<usize>,
    /// The exclusive loop end, as an instruction index into [`Self::data`].
    pub loop_end: Option<usize>,
    /// How many commands the stream lost (stripped writes plus merged-away delays).
    pub commands_removed: usize,
    /// How many bytes shorter the rebuilt stream is.
    pub bytes_saved: usize,
}

impl OptimizeOutcome {
    /// Installs this outcome into `song`, replacing its stream and loop markers and
    /// refreshing the derived length. `song` must be the VGM the outcome came from.
    pub fn install(self, song: &mut Song) {
        song.replace_vgm_stream(self.data, self.loop_point, self.loop_end);
    }
}

/// Optimises a VGM song: strips redundant register writes and merges the delays
/// left behind.
///
/// Returns `None` when the song is a DRO, or when nothing shrinks -- stripping
/// only removes bytes and merging never adds any, so an unchanged byte length
/// means the stream was already optimal, and the caller should leave it alone.
#[must_use]
pub fn optimize(song: &Song) -> Option<OptimizeOutcome> {
    // Only a VGM song has the OPL command stream this operates on.
    if !song.is_vgm() {
        return None;
    }
    let original_commands = song.len();
    let original_bytes = song.data().raw().len();

    // Phase 1: drop redundant register writes, letting `delete_instructions` slide
    // the loop markers past them exactly as a manual trim would.
    let redundant = redundant_indices(song);
    let mut work = song.clone();
    work.delete_instructions(&redundant);

    // Phase 2: merge the adjacent delays now left behind, remapping the (already
    // slid) loop markers onto the rebuilt stream.
    let (loop_point, loop_end) = work
        .vgm_meta()
        .map_or((None, None), |meta| (meta.loop_point, meta.loop_end));
    let SongData::Vgm(stream) = work.data() else {
        unreachable!("a VGM song's data is always a VGM stream");
    };
    let rebuilt = merge_delays(stream, loop_point, loop_end);

    let new_bytes = rebuilt.data.raw().len();
    if new_bytes >= original_bytes {
        return None;
    }
    Some(OptimizeOutcome {
        commands_removed: original_commands - rebuilt.data.len(),
        bytes_saved: original_bytes - new_bytes,
        data: rebuilt.data,
        loop_point: rebuilt.loop_point,
        loop_end: rebuilt.loop_end,
    })
}

/// The indices of the register writes that can be dropped without changing the
/// rendered output, in ascending order.
///
/// Only register writes are ever returned; delays are handled by the merge pass.
/// Returns an empty vector for a DRO song. See the module docs for the rule and
/// the loop-safety argument.
#[must_use]
pub fn redundant_indices(song: &Song) -> Vec<usize> {
    let Some(meta) = song.vgm_meta() else {
        return Vec::new();
    };
    let loop_point = meta.loop_point;
    let mut state = OplState::new();
    let mut redundant = Vec::new();

    for index in 0..song.len() {
        // Loop safety: forget every cached value before deciding the loop point, so
        // the first in-body write to each register is kept. See the module docs.
        if loop_point == Some(index) {
            state.reset();
        }
        if let Some(DroInstruction::Register { reg, value, bank }) = song.instruction(index) {
            // Every VGM write carries a bank; the reader has already routed each
            // opcode to the right file (chip 2 / port 1 -> the high file).
            let file = bank.map_or(0, |bank| usize::from(bank.index()));
            if state.is_cached(file, reg, value) {
                redundant.push(index);
            }
            state.record(file, reg, value);
        }
    }
    redundant
}

/// The OPL register cache the strip pass simulates: one 256-entry file per chip
/// side, each entry the register's last written value or `None` if never written
/// since load or the last loop-point reset.
struct OplState {
    files: [[Option<u8>; REGISTER_COUNT]; FILE_COUNT],
}

impl OplState {
    fn new() -> Self {
        Self {
            files: [[None; REGISTER_COUNT]; FILE_COUNT],
        }
    }

    /// Forgets every cached value, so the next write to any register is treated as
    /// a first write and kept.
    fn reset(&mut self) {
        self.files = [[None; REGISTER_COUNT]; FILE_COUNT];
    }

    /// Whether `reg` on `file` already holds `value` -- i.e. writing it would be a
    /// no-op the optimiser can drop.
    fn is_cached(&self, file: usize, reg: u8, value: u8) -> bool {
        self.files[file][usize::from(reg)] == Some(value)
    }

    fn record(&mut self, file: usize, reg: u8, value: u8) {
        self.files[file][usize::from(reg)] = Some(value);
    }
}

/// A stream rebuilt by the merge pass, with its loop markers remapped.
struct RebuiltStream {
    data: VgmData,
    loop_point: Option<usize>,
    loop_end: Option<usize>,
}

/// Rebuilds `stream`, merging runs of adjacent delays into one optimally encoded
/// sequence and remapping the loop markers onto the result.
///
/// The loop point and loop end are merge barriers: a run never spans either index,
/// so both stay on a command boundary, and each is resolved to the output command
/// its input command became -- exactly as [`filter_vgm`](crate::convert::filter_vgm)
/// resolves them. Register writes and lone delays are copied byte for byte, so a
/// stream with no mergeable run is reproduced verbatim.
fn merge_delays(
    stream: &VgmData,
    loop_point: Option<usize>,
    loop_end: Option<usize>,
) -> RebuiltStream {
    let mut out = Vec::with_capacity(stream.raw().len());
    let mut new_count = 0usize;
    let mut new_loop_point = None;
    let mut new_loop_end = None;
    // The indices of the adjacent delays awaiting a flush.
    let mut run: Vec<usize> = Vec::new();

    for index in 0..stream.len() {
        // Resolve the loop markers before emitting, flushing first so the marker
        // lands on the boundary between the run before it and the command at it.
        if loop_point == Some(index) {
            flush_run(stream, &mut run, &mut out, &mut new_count);
            new_loop_point = Some(new_count);
        }
        if loop_end == Some(index) {
            flush_run(stream, &mut run, &mut out, &mut new_count);
            new_loop_end = Some(new_count);
        }
        match stream.get(index).expect("index < len") {
            DroInstruction::DelaySamples { .. } => run.push(index),
            DroInstruction::Register { .. } => {
                flush_run(stream, &mut run, &mut out, &mut new_count);
                out.extend_from_slice(stream.raw_instruction(index).expect("index < len"));
                new_count += 1;
            }
            DroInstruction::BankSwitch(_) | DroInstruction::DelayMs { .. } => {
                unreachable!("a VGM stream has neither bank switches nor millisecond delays")
            }
        }
    }
    flush_run(stream, &mut run, &mut out, &mut new_count);

    // A marker at `len` means "the end of the stream", which the walk never reaches.
    if loop_point == Some(stream.len()) {
        new_loop_point = Some(new_count);
    }
    if loop_end == Some(stream.len()) {
        new_loop_end = Some(new_count);
    }

    RebuiltStream {
        data: VgmData::new(out).expect("every command emitted here is one the indexer knows"),
        loop_point: new_loop_point,
        loop_end: new_loop_end,
    }
}

/// Emits the pending delay run and clears it.
///
/// A lone delay is copied verbatim (the byte-exact invariant); a run of several is
/// summed and re-encoded, unless the encoding would be longer than the originals,
/// in which case they too are copied verbatim. Either way the sample total is
/// conserved exactly.
fn flush_run(stream: &VgmData, run: &mut Vec<usize>, out: &mut Vec<u8>, new_count: &mut usize) {
    match run.as_slice() {
        [] => {}
        [only] => {
            out.extend_from_slice(stream.raw_instruction(*only).expect("index < len"));
            *new_count += 1;
        }
        indices => {
            let total: u64 = indices
                .iter()
                .map(|&i| u64::from(delay_at(stream, i)))
                .sum();
            let original_len: usize = indices
                .iter()
                .map(|&i| stream.raw_instruction(i).expect("index < len").len())
                .sum();
            let (encoded, commands) = encode_wait(total);
            if encoded.len() <= original_len {
                out.extend_from_slice(&encoded);
                *new_count += commands;
            } else {
                for &i in indices {
                    out.extend_from_slice(stream.raw_instruction(i).expect("index < len"));
                    *new_count += 1;
                }
            }
        }
    }
    run.clear();
}

/// The sample count of the delay at `index`, or `0` if it is not a delay.
fn delay_at(stream: &VgmData, index: usize) -> u32 {
    stream
        .get(index)
        .map_or(0, |instruction| instruction.delay_samples())
}

/// Encodes a wait of `samples` samples as the byte sequence and command count of a
/// compact VGM wait: full `0x61` chunks for the bulk, then the shortest single
/// command for the remainder (`0x7n` for 1..=16, `0x62`/`0x63` for their exact
/// counts, else one more `0x61`).
///
/// The emitted commands always sum to exactly `samples`.
fn encode_wait(mut samples: u64) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut commands = 0usize;

    while samples > MAX_WAIT {
        out.push(command::WAIT);
        out.extend_from_slice(&(MAX_WAIT as u16).to_le_bytes());
        samples -= MAX_WAIT;
        commands += 1;
    }

    let remainder = samples as u32; // now 0..=65535
    if remainder == 0 {
        // A run summing to zero samples merges to nothing.
    } else if (1..=16).contains(&remainder) {
        out.push(command::SHORT_WAIT_BASE + (remainder as u8 - 1));
        commands += 1;
    } else if remainder == command::SAMPLES_60TH {
        out.push(command::WAIT_60TH);
        commands += 1;
    } else if remainder == command::SAMPLES_50TH {
        out.push(command::WAIT_50TH);
        commands += 1;
    } else {
        out.push(command::WAIT);
        out.extend_from_slice(&(remainder as u16).to_le_bytes());
        commands += 1;
    }

    (out, commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Bank, DelayKind, OplType};
    use crate::vgm::VgmMeta;
    use crate::vgm::io::synthesise_header;

    /// Builds a VGM song around a raw command stream, with no loop.
    fn vgm(bytes: Vec<u8>, opl_type: OplType) -> Song {
        Song::vgm(
            "t.vgm".to_owned(),
            0x151,
            VgmData::new(bytes).unwrap(),
            opl_type,
            VgmMeta::new(synthesise_header()),
        )
    }

    /// As [`vgm`], with a loop point and (optional) loop end set.
    fn looping_vgm(
        bytes: Vec<u8>,
        opl_type: OplType,
        loop_point: usize,
        loop_end: Option<usize>,
    ) -> Song {
        let mut song = vgm(bytes, opl_type);
        let meta = song.vgm_meta_mut().unwrap();
        meta.loop_point = Some(loop_point);
        meta.loop_end = loop_end;
        song
    }

    fn low(reg: u8, value: u8) -> [u8; 3] {
        [command::YM3812, reg, value]
    }

    // -- the strip rules ---------------------------------------------------

    #[test]
    fn the_first_write_to_a_register_is_kept() {
        let song = vgm([low(0x20, 0x01), low(0x21, 0x02)].concat(), OplType::Opl2);
        assert_eq!(redundant_indices(&song), Vec::<usize>::new());
    }

    #[test]
    fn a_same_value_rewrite_is_dropped() {
        // Write 0x20 = 0x01 twice, with a different register between; the second
        // 0x20 = 0x01 is redundant.
        let song = vgm(
            [low(0x20, 0x01), low(0x21, 0x02), low(0x20, 0x01)].concat(),
            OplType::Opl2,
        );
        assert_eq!(redundant_indices(&song), vec![2]);
    }

    #[test]
    fn a_changed_value_is_kept_and_reprimes_the_cache() {
        // 0x20 goes 0x01 -> 0x02 -> 0x02: only the last is redundant.
        let song = vgm(
            [low(0x20, 0x01), low(0x20, 0x02), low(0x20, 0x02)].concat(),
            OplType::Opl2,
        );
        assert_eq!(redundant_indices(&song), vec![2]);
    }

    #[test]
    fn the_two_dual_opl2_chips_are_tracked_separately() {
        // Chip 1 (0x5A) and chip 2 (0xAA) each hold their own 0x20. Writing the
        // same value to each is two first writes, not a redundant rewrite.
        let bytes = [
            [command::YM3812, 0x20, 0x01],
            [command::YM3812_CHIP_2, 0x20, 0x01],
            [command::YM3812, 0x20, 0x01],        // redundant on chip 1
            [command::YM3812_CHIP_2, 0x20, 0x01], // redundant on chip 2
        ]
        .concat();
        let song = vgm(bytes, OplType::DualOpl2);
        assert_eq!(redundant_indices(&song), vec![2, 3]);
    }

    #[test]
    fn the_two_opl3_ports_are_tracked_separately() {
        let bytes = [
            [command::YMF262_PORT_0, 0x20, 0x01],
            [command::YMF262_PORT_1, 0x20, 0x01],
            [command::YMF262_PORT_0, 0x20, 0x01], // redundant on port 0
            [command::YMF262_PORT_1, 0x20, 0x01], // redundant on port 1
        ]
        .concat();
        let song = vgm(bytes, OplType::Opl3);
        assert_eq!(redundant_indices(&song), vec![2, 3]);
    }

    #[test]
    fn the_loop_point_resets_the_cache_so_the_first_in_body_write_is_kept() {
        // 0: 0x40 = 0x10 (pre-loop)
        // 1: 0x40 = 0x10 (loop point -- same value, but kept because of the reset)
        // 2: 0x40 = 0x10 (now redundant against index 1)
        let bytes = [low(0x40, 0x10), low(0x40, 0x10), low(0x40, 0x10)].concat();
        let song = looping_vgm(bytes, OplType::Opl2, 1, None);
        assert_eq!(
            redundant_indices(&song),
            vec![2],
            "the loop-point write must survive; only the third is redundant"
        );
    }

    #[test]
    fn without_a_loop_the_same_run_strips_both_rewrites() {
        // The same three writes as above but with no loop: indices 1 and 2 are both
        // redundant. This is the control for the loop-reset test.
        let bytes = [low(0x40, 0x10), low(0x40, 0x10), low(0x40, 0x10)].concat();
        let song = vgm(bytes, OplType::Opl2);
        assert_eq!(redundant_indices(&song), vec![1, 2]);
    }

    #[test]
    fn a_dro_song_has_no_redundant_indices() {
        use crate::song::DroDataV1;
        let dro = Song::dro_v1(
            "t.dro".to_owned(),
            DroDataV1::new(vec![0x20, 0x01, 0x20, 0x01]).unwrap(),
            0,
            OplType::Opl2,
        );
        assert_eq!(redundant_indices(&dro), Vec::<usize>::new());
        assert!(optimize(&dro).is_none());
    }

    // -- the whole optimise pass -------------------------------------------

    #[test]
    fn optimising_drops_a_redundant_write_and_merges_the_delays() {
        // write, delay 100, redundant write, delay 200, write.
        let bytes = [
            &low(0x20, 0x01)[..],
            &[command::WAIT, 0x64, 0x00], // 100 samples
            &low(0x20, 0x01),             // redundant
            &[command::WAIT, 0xC8, 0x00], // 200 samples
            &low(0x21, 0x02),
        ]
        .concat();
        let song = vgm(bytes, OplType::Opl2);
        let outcome = optimize(&song).expect("there is a redundant write to drop");

        // The redundant write went, and the two 100/200 delays merged into one 300.
        assert_eq!(outcome.commands_removed, 2);
        assert_eq!(outcome.data.len(), 3);
        let kinds: Vec<DroInstruction> = (0..outcome.data.len())
            .map(|i| outcome.data.get(i).unwrap())
            .collect();
        assert_eq!(
            kinds,
            vec![
                DroInstruction::Register {
                    reg: 0x20,
                    value: 0x01,
                    bank: Some(Bank::Low)
                },
                DroInstruction::DelaySamples {
                    kind: DelayKind::Long,
                    samples: 300
                },
                DroInstruction::Register {
                    reg: 0x21,
                    value: 0x02,
                    bank: Some(Bank::Low)
                },
            ]
        );
    }

    #[test]
    fn an_already_optimal_stream_is_left_alone() {
        let bytes = [
            &low(0x20, 0x01)[..],
            &[command::WAIT, 0x64, 0x00],
            &low(0x21, 0x02),
        ]
        .concat();
        let song = vgm(bytes, OplType::Opl2);
        assert!(optimize(&song).is_none());
    }

    #[test]
    fn optimising_is_idempotent() {
        let bytes = [
            &low(0x20, 0x01)[..],
            &[command::WAIT, 0x64, 0x00],
            &low(0x20, 0x01), // redundant
            &low(0x20, 0x01), // redundant
            &[command::WAIT, 0xC8, 0x00],
            &low(0x21, 0x02),
        ]
        .concat();
        let mut song = vgm(bytes, OplType::Opl2);
        let first = optimize(&song).expect("there is work to do");
        first.install(&mut song);
        assert!(
            optimize(&song).is_none(),
            "a second pass must find nothing left to do"
        );
    }

    #[test]
    fn the_total_delay_is_conserved() {
        let bytes = [
            &low(0x20, 0x01)[..],
            &[command::WAIT, 0x64, 0x00], // 100
            &low(0x20, 0x01),             // redundant
            &[command::WAIT, 0xC8, 0x00], // 200
            &[command::WAIT_60TH],        // 735
            &low(0x21, 0x02),
        ]
        .concat();
        let song = vgm(bytes, OplType::Opl2);
        let before = song.total_delay_samples();
        let mut optimised = song.clone();
        optimize(&song).unwrap().install(&mut optimised);
        assert_eq!(optimised.total_delay_samples(), before);
    }

    // -- loop remapping ----------------------------------------------------

    #[test]
    fn stripping_before_the_loop_slides_the_markers() {
        // 0: redundant-making write, 1: write, 2: same write (redundant),
        // 3: delay (loop point), 4: write.
        let bytes = [
            &low(0x20, 0x01)[..],
            &low(0x21, 0x02),
            &low(0x20, 0x01), // redundant -> dropped, everything after slides by 1
            &[command::WAIT, 0x64, 0x00],
            &low(0x22, 0x03),
        ]
        .concat();
        let song = looping_vgm(bytes, OplType::Opl2, 3, None);
        let outcome = optimize(&song).expect("index 2 is redundant");
        assert_eq!(
            outcome.loop_point,
            Some(2),
            "the loop point followed the deletion of the earlier write"
        );
    }

    #[test]
    fn a_loop_marker_never_merges_across_its_boundary() {
        // Two delays flank the loop point; without the barrier they would merge and
        // swallow it.
        // 0: write, 1: delay 100, 2: delay 200 (loop point), 3: write.
        let bytes = [
            &low(0x20, 0x01)[..],
            &[command::WAIT, 0x64, 0x00], // 100
            &[command::WAIT, 0xC8, 0x00], // 200, loop point
            &low(0x21, 0x02),
        ]
        .concat();
        let song = looping_vgm(bytes, OplType::Opl2, 2, None);
        // No redundant writes, but the two delays before/at the loop cannot merge,
        // so nothing shrinks.
        assert!(
            optimize(&song).is_none(),
            "the barrier keeps the two delays apart, so there is nothing to merge"
        );
        // The barrier holds even when a redundant write elsewhere forces a rebuild.
        let bytes = [
            &low(0x20, 0x01)[..],
            &low(0x20, 0x01),             // redundant, forces the rebuild
            &[command::WAIT, 0x64, 0x00], // 100
            &[command::WAIT, 0xC8, 0x00], // 200, loop point (index 3)
            &low(0x21, 0x02),
        ]
        .concat();
        let song = looping_vgm(bytes, OplType::Opl2, 3, None);
        let outcome = optimize(&song).expect("index 1 is redundant");
        // Surviving stream: write, delay100, delay200, write. The loop point still
        // sits on its own delay command (index 2), not merged with the 100 before.
        assert_eq!(outcome.loop_point, Some(2));
        assert_eq!(
            outcome.data.get(1).unwrap().delay_samples(),
            100,
            "the delay before the loop stays separate"
        );
        assert_eq!(
            outcome.data.get(2).unwrap().delay_samples(),
            200,
            "the loop-point delay is not merged into the one before it"
        );
    }

    #[test]
    fn the_loop_region_between_the_markers_still_merges() {
        // A redundant write *inside* the loop body (repeating a value already set
        // after the loop-point reset) is stripped, and the delays it separated
        // merge -- while both markers stay on command boundaries.
        // 0: write 0x20=0x05 (pre-loop)
        // 1: write 0x20=0x05 (loop point; kept, first after the reset)
        // 2: delay 100
        // 3: write 0x20=0x05 (redundant against index 1 -> dropped)
        // 4: delay 200
        // 5: write 0x21=0x02 (loop end)
        // 6: delay 10
        let bytes = [
            &low(0x20, 0x05)[..],
            &low(0x20, 0x05),
            &[command::WAIT, 0x64, 0x00], // 100
            &low(0x20, 0x05),             // redundant (index 3)
            &[command::WAIT, 0xC8, 0x00], // 200
            &low(0x21, 0x02),             // loop end (index 5)
            &[command::WAIT, 0x0A, 0x00], // 10
        ]
        .concat();
        let song = looping_vgm(bytes, OplType::Opl2, 1, Some(5));
        assert_eq!(redundant_indices(&song), vec![3]);

        let outcome = optimize(&song).expect("index 3 is redundant");
        // Surviving: write, write(loop point), delay300 (merged), write(loop end),
        // delay10.
        assert_eq!(outcome.loop_point, Some(1));
        assert_eq!(outcome.loop_end, Some(3));
        assert_eq!(outcome.data.get(2).unwrap().delay_samples(), 300);
    }

    // -- the wait encoder --------------------------------------------------

    #[test]
    fn encode_wait_conserves_the_total_and_picks_short_forms() {
        let total = |samples| -> u64 {
            let (bytes, _) = encode_wait(samples);
            let data = VgmData::new(bytes).unwrap();
            (0..data.len()).map(|i| u64::from(delay_at(&data, i))).sum()
        };
        // Short single-command forms.
        assert_eq!(encode_wait(0), (vec![], 0));
        assert_eq!(encode_wait(16), (vec![command::SHORT_WAIT_BASE + 15], 1));
        assert_eq!(encode_wait(735), (vec![command::WAIT_60TH], 1));
        assert_eq!(encode_wait(882), (vec![command::WAIT_50TH], 1));
        // A plain 0x61 for an awkward remainder.
        assert_eq!(encode_wait(300), (vec![command::WAIT, 0x2C, 0x01], 1));
        // Bulk chunks plus a short remainder.
        let (bytes, commands) = encode_wait(65_535 + 5);
        assert_eq!(commands, 2);
        assert_eq!(bytes.len(), 3 + 1);
        for samples in [0, 1, 16, 17, 735, 882, 1000, 65_535, 65_536, 200_000] {
            assert_eq!(total(samples), samples, "sample total for {samples}");
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests {
    use super::*;
    use crate::song::OplType;
    use crate::vgm::VgmMeta;
    use crate::vgm::io::synthesise_header;
    use proptest::prelude::*;

    /// A random VGM command: a low/high register write, or a wait of some length.
    fn command_bytes() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            (any::<u8>(), any::<u8>()).prop_map(|(reg, value)| vec![command::YM3812, reg, value]),
            (any::<u8>(), any::<u8>()).prop_map(|(reg, value)| vec![
                command::YM3812_CHIP_2,
                reg,
                value
            ]),
            any::<u16>().prop_map(|samples| {
                let mut command = vec![command::WAIT];
                command.extend_from_slice(&samples.to_le_bytes());
                command
            }),
            (0u8..16).prop_map(|n| vec![command::SHORT_WAIT_BASE + n]),
        ]
    }

    fn random_stream() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(command_bytes(), 0..40).prop_map(|commands| commands.concat())
    }

    proptest! {
        /// However the stream is stripped and merged, the total delay is conserved
        /// exactly and a second pass finds nothing left to do.
        #[test]
        fn optimise_conserves_delay_and_is_idempotent(bytes in random_stream()) {
            let mut song = Song::vgm(
                "t.vgm".to_owned(),
                0x151,
                VgmData::new(bytes).unwrap(),
                OplType::DualOpl2,
                VgmMeta::new(synthesise_header()),
            );
            let before = song.total_delay_samples();
            if let Some(outcome) = optimize(&song) {
                prop_assert!(outcome.data.raw().len() < song.data().raw().len());
                outcome.install(&mut song);
                prop_assert_eq!(song.total_delay_samples(), before);
                prop_assert!(optimize(&song).is_none(), "a second pass must be a no-op");
            }
        }

        /// The merge pass in isolation conserves every sample and never grows the
        /// stream -- so it can only ever help.
        #[test]
        fn merge_conserves_delay_and_never_grows(bytes in random_stream()) {
            let stream = VgmData::new(bytes).unwrap();
            let before: u64 = (0..stream.len()).map(|i| u64::from(delay_at(&stream, i))).sum();
            let rebuilt = merge_delays(&stream, None, None);
            let after: u64 = (0..rebuilt.data.len())
                .map(|i| u64::from(delay_at(&rebuilt.data, i)))
                .sum();
            prop_assert_eq!(before, after, "the merge changed the total delay");
            prop_assert!(rebuilt.data.raw().len() <= stream.raw().len());
        }

        /// With random loop markers, the merged stream conserves the loop region's
        /// sample total and keeps both markers on real command boundaries.
        #[test]
        fn merge_preserves_the_loop_region(
            bytes in random_stream(),
            a in 0usize..40,
            b in 0usize..40,
        ) {
            let stream = VgmData::new(bytes).unwrap();
            let len = stream.len();
            prop_assume!(len > 0);
            let loop_point = a % (len + 1);
            let loop_end = loop_point + (b % (len + 1 - loop_point));
            let region = |data: &VgmData, start: usize, end: usize| -> u64 {
                (start..end.min(data.len())).map(|i| u64::from(delay_at(data, i))).sum()
            };
            let before = region(&stream, loop_point, loop_end);
            let rebuilt = merge_delays(&stream, Some(loop_point), Some(loop_end));
            let (new_start, new_end) = (rebuilt.loop_point.unwrap(), rebuilt.loop_end.unwrap());
            prop_assert!(new_start <= rebuilt.data.len());
            prop_assert!(new_end <= rebuilt.data.len());
            prop_assert!(new_start <= new_end);
            prop_assert_eq!(region(&rebuilt.data, new_start, new_end), before);
        }
    }
}
