//! Delay merging and wait encoding for the VGM optimiser.
//!
//! [`VgmFile::optimize`](crate::VgmFile) drops audibly-redundant register writes
//! (a write whose value equals the register's cached value is inaudible, so
//! dropping it cannot change the rendered output -- the rule and its loop-safety
//! argument live with [`chip_state::redundant_indices`](crate::chip_state)) and
//! is left with runs of adjacent delays. This module merges those runs.
//!
//! # Delay merging
//!
//! Dropping a write between two delays leaves them adjacent; a run of adjacent
//! delays is summed and re-encoded compactly. The total is conserved exactly, so
//! playback timing is untouched (and the engine's frame clock carries its
//! fractional remainder, making the sum of two delays render frame-for-frame the
//! same as the merged delay). A lone delay keeps its original bytes, and a run is
//! only re-encoded when that actually saves bytes -- so an already-optimal stream
//! is reproduced verbatim. The loop point and end are merge barriers, so each
//! stays on a command boundary.

use crate::vgm::data::command;
use crate::vgm::stream::{VgmCommand, VgmStream};

/// The largest wait a single `0x61` command can express.
const MAX_WAIT: u64 = 0xFFFF;

/// Merges runs of adjacent waits in a stream of any chip's commands.
///
/// The second half of [`VgmFile::optimize`](crate::VgmFile): dropping
/// audibly-redundant register writes (via
/// [`chip_state::redundant_indices`](crate::chip_state)) leaves delays sitting
/// next to each other, and two waits in a row cost more bytes than one wait of
/// their sum.
///
/// Returns the rebuilt bytes (end marker included) and where the loop point
/// and the deliberately-short loop end landed in them. Both are merge barriers
/// -- a run never spans either -- so each stays on a command boundary. Without
/// the end barrier the boundary could vanish into a merged wait: the header's
/// sample count would survive, but the *row* it re-derives from would not, and
/// the next edit's `loop_end_index` would find nothing and widen the loop to
/// the tail.
///
/// A `0x8n` DAC write is *not* a wait for this purpose even though it waits:
/// it writes a sample first, so folding it into a neighbouring run would drop
/// the sample. It is copied verbatim like any other command.
#[must_use]
pub(crate) fn merge_stream_delays(
    stream: &VgmStream,
    loop_at: Option<usize>,
    loop_end: Option<usize>,
) -> (Vec<u8>, Option<usize>, Option<usize>) {
    let mut out: Vec<u8> = Vec::with_capacity(stream.raw().len());
    let mut new_loop = None;
    let mut new_loop_end = None;
    let mut run: Vec<usize> = Vec::new();

    let flush = |run: &mut Vec<usize>, out: &mut Vec<u8>| {
        let bytes_of = |index: usize| stream.raw_command(index).unwrap_or_default();
        match run.as_slice() {
            [] => {}
            // A lone delay is copied verbatim, which is what keeps a stream
            // with nothing to merge byte-identical.
            [only] => out.extend_from_slice(bytes_of(*only)),
            indices => {
                let total: u64 = indices
                    .iter()
                    .map(|&i| u64::from(stream.wait_samples(i)))
                    .sum();
                let original: usize = indices.iter().map(|&i| bytes_of(i).len()).sum();
                let (encoded, _) = encode_wait(total);
                if encoded.len() <= original {
                    out.extend_from_slice(&encoded);
                } else {
                    for &i in indices {
                        out.extend_from_slice(bytes_of(i));
                    }
                }
            }
        }
        run.clear();
    };

    for index in 0..stream.len() {
        if loop_at == Some(index) {
            flush(&mut run, &mut out);
            new_loop = Some(out.len());
        }
        if loop_end == Some(index) {
            flush(&mut run, &mut out);
            new_loop_end = Some(out.len());
        }
        if matches!(stream.get(index), Some(VgmCommand::Wait(_))) {
            run.push(index);
        } else {
            flush(&mut run, &mut out);
            out.extend_from_slice(stream.raw_command(index).unwrap_or_default());
        }
    }
    flush(&mut run, &mut out);
    if loop_at == Some(stream.len()) {
        new_loop = Some(out.len());
    }
    if loop_end == Some(stream.len()) {
        new_loop_end = Some(out.len());
    }

    out.push(command::END);
    (out, new_loop, new_loop_end)
}

/// The single-byte VGM wait commands, as `(samples, opcode)`: `0x7n` for 1..=16,
/// `0x62` for 735, `0x63` for 882.
fn short_waits() -> Vec<(u64, u8)> {
    let mut waits: Vec<(u64, u8)> = (0..16u8)
        .map(|n| (u64::from(n) + 1, command::SHORT_WAIT_BASE + n))
        .collect();
    waits.push((u64::from(command::SAMPLES_60TH), command::WAIT_60TH));
    waits.push((u64::from(command::SAMPLES_50TH), command::WAIT_50TH));
    waits
}

/// Encodes a wait of `samples` samples as the *shortest* VGM byte sequence, with
/// its command count. The emitted commands always sum to exactly `samples`.
///
/// The bulk goes in full `0x61` chunks (three bytes, up to 65535 samples each --
/// far the most byte-efficient); a "tail" of at most two single-byte commands
/// (`0x62`/`0x63`/`0x7n`) then shaves the last chunk when it lands on a value they
/// can hit. Two is the useful maximum: three single-byte commands cost the same
/// three bytes as one `0x61`, which covers far more, so no optimal encoding needs
/// more than two. Enumerating every such tail and taking the fewest bytes is
/// therefore exactly minimal -- and it captures the cases the old greedy pass
/// missed, e.g. 32 → `0x7F 0x7F` (two bytes, not a three-byte `0x61`), 1470 →
/// `0x62 0x62`, and the borrow 67004 → `0x61(65534) 0x62 0x62` (five bytes, where
/// a full `0x61(65535)` chunk would leave an un-shavable 1469).
fn encode_wait(samples: u64) -> (Vec<u8>, usize) {
    let shorts = short_waits();

    // Candidate tails: nothing, one single-byte command, or two.
    let mut tails: Vec<(u64, Vec<u8>)> = vec![(0, Vec::new())];
    for &(value, op) in &shorts {
        tails.push((value, vec![op]));
    }
    for &(v1, op1) in &shorts {
        for &(v2, op2) in &shorts {
            tails.push((v1 + v2, vec![op1, op2]));
        }
    }

    // The rest of `samples` after the tail is covered by `0x61` chunks. Pick the
    // tail giving the fewest bytes, then the fewest commands.
    let (tail_value, tail_ops) = tails
        .iter()
        .filter(|(value, _)| *value <= samples)
        .min_by_key(|(value, ops)| {
            let chunks = (samples - value).div_ceil(MAX_WAIT) as usize;
            (ops.len() + 3 * chunks, ops.len() + chunks)
        })
        .expect("the empty tail is always a candidate");

    // The bulk goes in `0x61` chunks (the shared emitter), then the chosen tail.
    let mut out = Vec::new();
    let mut commands = crate::vgm::data::append_wait(&mut out, samples - tail_value);
    out.extend_from_slice(tail_ops);
    commands += tail_ops.len();
    (out, commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    // -- the wait encoder --------------------------------------------------

    #[test]
    fn encode_wait_conserves_the_total_and_picks_short_forms() {
        // The samples an encoding carries, decoding its wait commands by hand so
        // the check leans on nothing but the byte forms.
        let total = |samples| -> u64 {
            let (bytes, _) = encode_wait(samples);
            let mut sum = 0u64;
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    command::WAIT => {
                        sum += u64::from(u16::from_le_bytes([bytes[i + 1], bytes[i + 2]]));
                        i += 3;
                    }
                    command::WAIT_60TH => {
                        sum += u64::from(command::SAMPLES_60TH);
                        i += 1;
                    }
                    command::WAIT_50TH => {
                        sum += u64::from(command::SAMPLES_50TH);
                        i += 1;
                    }
                    // The only remaining forms encode_wait emits are the 0x7n
                    // single-byte waits of 1..=16 samples.
                    op => {
                        sum += u64::from(op - command::SHORT_WAIT_BASE) + 1;
                        i += 1;
                    }
                }
            }
            sum
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

        // Two single-byte commands beat a three-byte 0x61 where they can hit the
        // value: 32 = 16 + 16, 1470 = 735 + 735, 1764 = 882 + 882.
        for &(samples, len) in &[(32u64, 2usize), (1470, 2), (1617, 2), (1764, 2)] {
            assert_eq!(encode_wait(samples).0.len(), len, "for {samples} samples");
            assert_eq!(total(samples), samples);
        }
        // The borrow: a full 0x61(65535) chunk would leave 1469 -- un-shavable, so
        // three bytes. Shaving the chunk to 65534 leaves 1470 = 0x62 0x62, so the
        // whole thing is five bytes, not six.
        let (bytes, commands) = encode_wait(65_535 + 1469);
        assert_eq!(bytes.len(), 5);
        assert_eq!(commands, 3);
        assert_eq!(total(65_535 + 1469), 65_535 + 1469);

        for samples in [0, 1, 16, 17, 735, 882, 1000, 65_535, 65_536, 200_000] {
            assert_eq!(total(samples), samples, "sample total for {samples}");
        }
    }

    /// The encoder is provably minimal for values needing at most one `0x61`
    /// (`<= 2000`): its byte count matches the best over every "at most two
    /// single-byte commands, then one `0x61` for the rest" split, which is the
    /// whole optimum (three single-byte commands never beat a `0x61`).
    #[test]
    fn encode_wait_is_byte_minimal_for_small_waits() {
        use std::collections::HashMap;
        let shorts = short_waits();
        // The fewest single-byte commands to sum to each reachable value (<= 2 of
        // them; more can never help).
        let mut by_short: HashMap<u64, usize> = HashMap::from([(0, 0)]);
        for &(v, _) in &shorts {
            by_short.entry(v).or_insert(1);
        }
        for &(v1, _) in &shorts {
            for &(v2, _) in &shorts {
                by_short.entry(v1 + v2).or_insert(2);
            }
        }
        for samples in 0..=2000u64 {
            let reference = by_short
                .iter()
                .filter(|&(&value, _)| value <= samples)
                .map(|(&value, &count)| count + if value == samples { 0 } else { 3 })
                .min()
                .expect("the zero tail always qualifies");
            assert_eq!(
                encode_wait(samples).0.len(),
                reference,
                "not minimal for {samples} samples"
            );
        }
    }
}
