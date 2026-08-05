//! Automatic loop-point discovery.
//!
//! A song captured from a game usually loops several times before the recording
//! is stopped, so the command stream contains a block of writes that repeats
//! verbatim later on. Finding that repeat is finding the loop: the block at
//! `loop_point` recurring at `loop_end` means `[loop_point, loop_end)` is one
//! loop body, and the editor's loop markers can be set straight to it.
//!
//! The search is the behaviour of vgmtools' `vgmlpfnd`, re-implemented from its
//! description rather than its (GPL) source:
//!
//! - **Delays are ignored.** Matching compares register writes and bank
//!   switches only, so a body and its repeat match even when the delays between
//!   them were captured with slightly different timing. The delay-stripped
//!   sequence is searched, and a parallel table maps each kept command back to
//!   its real instruction index, so the reported markers land on real command
//!   boundaries.
//! - **A match must be at least `min_len_commands` long**, in delay-stripped
//!   commands -- short coincidental repeats are not loops.
//! - **Quality flags** rank the candidates: `e` (the repeat runs to the end of
//!   the song, the strongest evidence of a loop), `f` (the source block ends
//!   before the copy begins -- a clean "body then repeat" shape), and `!` (both).
//!
//! Where `vgmlpfnd` brute-forces every offset, this buckets window starts by a
//! rolling hash first, so candidate pairs are found in roughly linear time and
//! then verified exactly. The search takes an `is_cancelled` callback and emits
//! candidates as it finds them, so it can run behind the UI's background-task
//! machinery without blocking.

use std::collections::HashMap;

use crate::Song;
use crate::song::{Bank, Instruction};

/// FNV-64's prime, used as the rolling hash base. Any odd base gives a
/// position-independent hash, so equal blocks always collide (collisions are
/// verified away exactly); the choice only affects how many false collisions
/// the verification has to reject.
const HASH_BASE: u64 = 1_099_511_628_211;

/// A bucket holding more window starts than this is a degenerate block that
/// repeats absurdly often (a symptom of too small a `min_len_commands`); pairing
/// them all up would be quadratic, so such a bucket is skipped with a warning
/// rather than allowed to stall the search. Real music never comes close.
const MAX_BUCKET_POSITIONS: usize = 4096;

/// A discovered loop candidate: the block of writes at `loop_end` repeats the
/// block at `loop_point`, so `[loop_point, loop_end)` is one loop body.
///
/// Both indices are real instruction indices (the app's exclusive-end model),
/// snapped to the first non-delay command of each block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// The instruction the loop restarts at -- the first command of the body.
    pub loop_point: usize,
    /// The exclusive instruction index where the body ends and the repeat
    /// begins.
    pub loop_end: usize,
    /// How many delay-stripped commands matched: the confidence in the loop, and
    /// the primary ranking signal after the quality flags.
    pub match_len: usize,
    /// The matched repeat runs to the end of the song (`e`). The strongest sign
    /// of a real loop, since the song's tail is then a replay of an earlier
    /// section.
    pub ends_at_eof: bool,
    /// The source block ends at or before the copy begins (`f`): a clean
    /// "body then repeat" shape, with no overlap between the two.
    pub clean_repeat: bool,
}

impl Candidate {
    /// A sortable quality score: both flags (3) beats `e` alone (2) beats `f`
    /// alone (1) beats neither (0). `e` outranks `f` because a repeat that runs
    /// to the end of the song is the stronger evidence of a loop.
    #[must_use]
    pub fn quality_rank(self) -> u8 {
        u8::from(self.ends_at_eof) * 2 + u8::from(self.clean_repeat)
    }

    /// `vgmlpfnd`'s flag notation for the results table: `!` (both), `e`, `f`,
    /// or `-` (neither).
    #[must_use]
    pub fn quality_label(self) -> &'static str {
        match (self.ends_at_eof, self.clean_repeat) {
            (true, true) => "!",
            (true, false) => "e",
            (false, true) => "f",
            (false, false) => "-",
        }
    }
}

/// Packs a non-delay instruction into a comparable key, or `None` for a delay.
///
/// Delays return `None` so the search skips them entirely -- a body and its
/// repeat match regardless of how their delays were encoded. Register writes
/// compare on bank, register and value; bank switches (DRO v1) compare on the
/// selected bank. The tag bits keep the two kinds from ever colliding.
fn normalize(instruction: Instruction) -> Option<u32> {
    match instruction {
        Instruction::Register { reg, value, bank } => {
            let bank_bit = u32::from(matches!(bank, Some(Bank::High)));
            Some((1 << 24) | (bank_bit << 16) | (u32::from(reg) << 8) | u32::from(value))
        }
        Instruction::BankSwitch(bank) => Some((2 << 24) | u32::from(bank.index())),
        Instruction::DelayMs { .. } | Instruction::DelaySamples { .. } => None,
    }
}

/// Searches `song` for loop candidates at least `min_len_commands` delay-stripped
/// commands long, calling `emit` with each as it is found and stopping promptly
/// once `is_cancelled` returns `true`.
///
/// Candidates arrive unranked and in no particular order; pass the collected set
/// through [`rank`] for display. Each distinct loop is emitted once -- the search
/// reports only the *start* of each maximal matching run, so a body that repeats
/// does not surface as one candidate per offset within it.
pub fn find_loops(
    song: &Song,
    min_len_commands: usize,
    emit: &mut dyn FnMut(Candidate),
    is_cancelled: &dyn Fn() -> bool,
) {
    // The delay-stripped command keys, and the real instruction index of each.
    let mut keys: Vec<u32> = Vec::with_capacity(song.len());
    let mut real: Vec<usize> = Vec::with_capacity(song.len());
    for (index, instruction) in song.data().iter().enumerate() {
        if let Some(key) = normalize(instruction) {
            keys.push(key);
            real.push(index);
        }
    }
    search(&keys, &real, min_len_commands, emit, is_cancelled);
}

/// The same search over a VGM stream of any chip's commands.
///
/// Everything but the key-building is shared: a loop is a block of commands
/// that recurs, and what a command *means* never enters into it. Waits are
/// stripped for the same reason as in the OPL path -- a body and its repeat
/// should match through timing jitter -- and every other command is keyed by a
/// hash of its own bytes, which needs no per-chip knowledge at all.
///
/// The hash can collide, and the verification compares keys rather than bytes,
/// so a collision could surface a candidate that is not really a repeat. At 32
/// bits over the block lengths this searches that is vanishingly unlikely, and
/// the cost is one spurious row in a list the user auditions before applying.
pub fn find_loops_in_stream(
    stream: &crate::vgm::VgmStream,
    min_len_commands: usize,
    emit: &mut dyn FnMut(Candidate),
    is_cancelled: &dyn Fn() -> bool,
) {
    let mut keys: Vec<u32> = Vec::with_capacity(stream.len());
    let mut real: Vec<usize> = Vec::with_capacity(stream.len());
    for index in 0..stream.len() {
        if matches!(stream.get(index), Some(crate::vgm::VgmCommand::Wait(_))) {
            continue;
        }
        let Some(bytes) = stream.raw_command(index) else {
            continue;
        };
        keys.push(hash_command(bytes));
        real.push(index);
    }
    search(&keys, &real, min_len_commands, emit, is_cancelled);
}

/// FNV-1a over a command's bytes: a stable, chip-blind identity for it.
fn hash_command(bytes: &[u8]) -> u32 {
    let mut hash = 0x811C_9DC5u32;
    for &byte in bytes {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The search proper, over pre-built keys and the real row each came from.
fn search(
    keys: &[u32],
    real: &[usize],
    min_len_commands: usize,
    emit: &mut dyn FnMut(Candidate),
    is_cancelled: &dyn Fn() -> bool,
) {
    let window = min_len_commands.max(1);
    let count = keys.len();
    // A pair of distinct `window`-length blocks needs at least `window + 1`
    // commands.
    if count < window + 1 || is_cancelled() {
        return;
    }

    // Bucket every window start by a rolling hash of its `window` keys, so only
    // blocks that (probably) match are ever compared.
    let starts = count - window + 1;
    let mut power = 1u64; // HASH_BASE^(window - 1)
    for _ in 1..window {
        power = power.wrapping_mul(HASH_BASE);
    }
    let mut hash = 0u64;
    for &key in &keys[..window] {
        hash = hash.wrapping_mul(HASH_BASE).wrapping_add(u64::from(key));
    }
    let mut buckets: HashMap<u64, Vec<u32>> = HashMap::new();
    buckets.entry(hash).or_default().push(0);
    for start in 1..starts {
        if start.is_multiple_of(8192) && is_cancelled() {
            return;
        }
        let outgoing = u64::from(keys[start - 1]).wrapping_mul(power);
        hash = hash
            .wrapping_sub(outgoing)
            .wrapping_mul(HASH_BASE)
            .wrapping_add(u64::from(keys[start + window - 1]));
        buckets.entry(hash).or_default().push(start as u32);
    }

    for positions in buckets.values() {
        if positions.len() < 2 {
            continue;
        }
        if positions.len() > MAX_BUCKET_POSITIONS {
            log::warn!(
                "loop search skipping a block that repeats {} times; the minimum match length \
                 of {window} commands is too small for this song",
                positions.len()
            );
            continue;
        }
        // `positions` is ascending: starts were pushed in increasing order.
        for (nth, &src_pos) in positions.iter().enumerate() {
            if is_cancelled() {
                return;
            }
            let i = src_pos as usize;
            for &copy_pos in &positions[nth + 1..] {
                let j = copy_pos as usize; // j > i
                // Only the start of a maximal matching run. If the commands just
                // before both blocks already match, this pair is the interior of
                // a longer match that will be (or was) reported at its true
                // start -- this is what collapses the many offsets of one repeat
                // down to a single candidate.
                if i > 0 && keys[i - 1] == keys[j - 1] {
                    continue;
                }
                // The hash can collide; verify the window exactly.
                if keys[i..i + window] != keys[j..j + window] {
                    continue;
                }
                // Extend the match forward, past the window, to its full length.
                let mut length = window;
                while j + length < count && keys[i + length] == keys[j + length] {
                    length += 1;
                }
                emit(Candidate {
                    loop_point: real[i],
                    loop_end: real[j],
                    match_len: length,
                    ends_at_eof: j + length == count,
                    clean_repeat: i + length <= j,
                });
            }
        }
    }
}

/// Sorts candidates best-first: by quality flags, then match length, then
/// position for a stable order. The top row is the search's best guess at the
/// song's loop.
pub fn rank(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.quality_rank()
            .cmp(&a.quality_rank())
            .then_with(|| b.match_len.cmp(&a.match_len))
            .then_with(|| a.loop_point.cmp(&b.loop_point))
            .then_with(|| a.loop_end.cmp(&b.loop_end))
    });
}

/// Runs [`find_loops`] to completion (no cancellation) and returns the ranked
/// results. The convenient entry point for tests and one-shot searches; the UI
/// uses [`find_loops`] directly so it can stream and cancel.
#[must_use]
pub fn find_loops_ranked(song: &Song, min_len_commands: usize) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    find_loops(
        song,
        min_len_commands,
        &mut |candidate| candidates.push(candidate),
        &|| false,
    );
    rank(&mut candidates);
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{DroDataV1, OplType};

    /// A DRO v1 register write, `reg value` (registers are all >= 0x20 here, so
    /// none collides with a v1 delay/bank opcode).
    fn reg(register: u8, value: u8) -> [u8; 2] {
        [register, value]
    }

    /// A DRO v1 long delay of `ms` milliseconds, `0x01 lo hi` (waits `word + 1`).
    /// The exact value never matters -- the search strips delays -- so this only
    /// has to be one instruction, like the register writes it sits between.
    fn wait(ms: u16) -> [u8; 3] {
        let word = ms.saturating_sub(1);
        [0x01, word as u8, (word >> 8) as u8]
    }

    fn dro_song(stream: Vec<u8>) -> Song {
        let data = DroDataV1::new(stream).expect("valid DRO v1 stream");
        Song::dro_v1("loop.dro".to_owned(), data, 0, OplType::Opl2)
    }

    /// Four distinct register writes with delays between them: one loop body.
    /// The delays vary by call so a repeat can be built with different timing.
    fn body(gap_a: u16, gap_b: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&reg(0xA0, 0x11));
        bytes.extend_from_slice(&wait(gap_a));
        bytes.extend_from_slice(&reg(0xB0, 0x22));
        bytes.extend_from_slice(&reg(0xA0, 0x33));
        bytes.extend_from_slice(&wait(gap_b));
        bytes.extend_from_slice(&reg(0xC0, 0x44));
        bytes
    }

    #[test]
    fn a_block_that_repeats_once_is_found_with_both_flags() {
        // intro (2 writes), then the body twice.
        let mut stream = Vec::new();
        stream.extend_from_slice(&reg(0x20, 0x01));
        stream.extend_from_slice(&wait(100));
        stream.extend_from_slice(&reg(0x40, 0x10));
        stream.extend_from_slice(&body(50, 70));
        stream.extend_from_slice(&body(50, 70));
        let song = dro_song(stream);

        let candidates = find_loops_ranked(&song, 4);
        assert_eq!(
            candidates.len(),
            1,
            "one clean loop, not a swarm of offsets"
        );
        let found = candidates[0];
        // Real indices: intro is instructions 0..3 (write, delay, write); the
        // first body's writes are at 3, 5, 6, 8; the second body's start at 9.
        assert_eq!(found.loop_point, 3);
        assert_eq!(found.loop_end, 9);
        assert_eq!(found.match_len, 4, "all four body commands matched");
        assert!(found.ends_at_eof, "the repeat runs to the end of the song");
        assert!(
            found.clean_repeat,
            "the body ends exactly where the repeat begins"
        );
        assert_eq!(found.quality_label(), "!");
    }

    #[test]
    fn a_repeat_matches_even_when_its_delays_differ() {
        // The two bodies have identical writes but wildly different delays; the
        // search strips delays, so it still matches.
        let mut stream = Vec::new();
        stream.extend_from_slice(&reg(0x20, 0x01));
        stream.extend_from_slice(&wait(100));
        stream.extend_from_slice(&reg(0x40, 0x10));
        stream.extend_from_slice(&body(50, 70));
        stream.extend_from_slice(&body(9000, 3));
        let song = dro_song(stream);

        let candidates = find_loops_ranked(&song, 4);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].loop_point, 3);
        assert_eq!(candidates[0].loop_end, 9);
    }

    #[test]
    fn a_repeat_shorter_than_the_minimum_is_suppressed() {
        // The body is four commands; asking for five finds nothing.
        let mut stream = Vec::new();
        stream.extend_from_slice(&reg(0x20, 0x01));
        stream.extend_from_slice(&wait(100));
        stream.extend_from_slice(&reg(0x40, 0x10));
        stream.extend_from_slice(&body(50, 70));
        stream.extend_from_slice(&body(50, 70));
        let song = dro_song(stream);

        assert!(find_loops_ranked(&song, 5).is_empty());
    }

    #[test]
    fn a_song_with_no_repeat_finds_nothing() {
        let mut stream = Vec::new();
        for n in 0..20u8 {
            stream.extend_from_slice(&reg(0x20 + n, n));
            stream.extend_from_slice(&wait(10));
        }
        let song = dro_song(stream);
        assert!(find_loops_ranked(&song, 3).is_empty());
    }

    #[test]
    fn a_thrice_repeated_body_dedups_to_two_candidates() {
        // body body body, no intro. The maximal-run filter collapses the offsets
        // within each body, leaving exactly the fundamental loop (body 1 -> body
        // 2) and the double-length loop (body 1 -> body 3).
        let mut stream = Vec::new();
        for _ in 0..3 {
            stream.extend_from_slice(&body(50, 70));
        }
        let song = dro_song(stream);

        let candidates = find_loops_ranked(&song, 4);
        assert_eq!(candidates.len(), 2, "no per-offset duplicates");

        // Each body is six instructions (four writes, two delays); the fundamental
        // loop runs from body 1's start (0) to body 2's start (6).
        let fundamental = candidates
            .iter()
            .find(|c| c.loop_point == 0 && c.loop_end == 6)
            .expect("the fundamental one-body loop is present");
        assert!(fundamental.ends_at_eof);
        // The other candidate is the two-body loop, ending where body 3 begins.
        assert!(
            candidates
                .iter()
                .any(|c| c.loop_point == 0 && c.loop_end == 12)
        );
    }

    #[test]
    fn a_cancelled_search_emits_nothing() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&body(50, 70));
        stream.extend_from_slice(&body(50, 70));
        let song = dro_song(stream);

        let mut emitted = Vec::new();
        find_loops(&song, 4, &mut |c| emitted.push(c), &|| true);
        assert!(
            emitted.is_empty(),
            "an always-cancelled search finds nothing"
        );
    }

    #[test]
    fn rank_orders_by_quality_then_length() {
        let none = Candidate {
            loop_point: 0,
            loop_end: 10,
            match_len: 5,
            ends_at_eof: false,
            clean_repeat: false,
        };
        let both = Candidate {
            loop_point: 1,
            loop_end: 11,
            match_len: 3,
            ends_at_eof: true,
            clean_repeat: true,
        };
        let eof_only = Candidate {
            loop_point: 2,
            loop_end: 12,
            match_len: 9,
            ends_at_eof: true,
            clean_repeat: false,
        };
        let mut candidates = vec![none, both, eof_only];
        rank(&mut candidates);
        assert_eq!(candidates, vec![both, eof_only, none]);
    }

    #[test]
    fn longer_matches_outrank_shorter_ones_at_equal_quality() {
        let short = Candidate {
            loop_point: 0,
            loop_end: 5,
            match_len: 4,
            ends_at_eof: true,
            clean_repeat: true,
        };
        let long = Candidate {
            loop_point: 9,
            loop_end: 20,
            match_len: 40,
            ends_at_eof: true,
            clean_repeat: true,
        };
        let mut candidates = vec![short, long];
        rank(&mut candidates);
        assert_eq!(candidates[0], long);
    }
}
