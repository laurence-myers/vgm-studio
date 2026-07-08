//! Single-pass multi-range deletion and re-insertion, shared by every encoding.
//!
//! The Python deleted one contiguous range at a time -- `O(k*n)` for `k` ranges --
//! and re-inserted one instruction at a time. These are `O(n)` regardless of how
//! fragmented the selection is, and are exact inverses of each other.

use core::ops::Range;

use crate::util::condense_ranges;

/// A restored instruction: its logical index, and its raw bytes.
pub type InsertEntry = (usize, Box<[u8]>);

/// Turns an arbitrary selection into the byte ranges to remove, or `None` if
/// nothing survives the bounds filter.
///
/// Sorts and de-duplicates defensively: the Python `DRODataV1.insert_multiple`
/// (and its copy in `VGMData`) silently produced garbage when handed an unsorted
/// list, and the only reason that never fired was that the wx list control
/// happened to yield indices in ascending order.
pub(crate) fn byte_ranges_to_delete(
    indices: &[usize],
    len: usize,
    byte_offset: impl Fn(usize) -> usize,
) -> Option<Vec<Range<usize>>> {
    let mut sorted: Vec<usize> = indices.iter().copied().filter(|&i| i < len).collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return None;
    }
    Some(
        condense_ranges(&sorted)
            .into_iter()
            .map(|range| byte_offset(*range.start())..byte_offset(*range.end() + 1))
            .collect(),
    )
}

/// Removes `byte_ranges` (ascending, disjoint) from `data` in one forward pass.
pub(crate) fn splice_out(data: &mut Vec<u8>, byte_ranges: &[Range<usize>]) {
    let Some(first) = byte_ranges.first() else {
        return;
    };
    let mut write = first.start;
    for (i, range) in byte_ranges.iter().enumerate() {
        let next_start = byte_ranges.get(i + 1).map_or(data.len(), |next| next.start);
        let survivors = range.end..next_start;
        let count = survivors.len();
        data.copy_within(survivors, write);
        write += count;
    }
    data.truncate(write);
}

/// Rebuilds `data` with `entries` re-inserted at their logical indices, in one
/// forward pass and one allocation.
pub(crate) fn splice_in(
    data: &[u8],
    entries: &[InsertEntry],
    byte_offset: impl Fn(usize) -> usize,
) -> Vec<u8> {
    debug_assert!(
        entries.windows(2).all(|w| w[0].0 < w[1].0),
        "insert entries must be sorted ascending by index and de-duplicated"
    );

    let extra: usize = entries.iter().map(|(_, bytes)| bytes.len()).sum();
    let mut out = Vec::with_capacity(data.len() + extra);

    // `emitted` counts instructions already written to `out`, so the next entry
    // needs `target - emitted` of the surviving instructions copied before it.
    let mut emitted = 0usize;
    let mut src_logical = 0usize;
    let mut src_byte = 0usize;

    for (target, bytes) in entries {
        let need = target
            .checked_sub(emitted)
            .expect("insert entries must be sorted ascending");
        let end_byte = byte_offset(src_logical + need);
        out.extend_from_slice(&data[src_byte..end_byte]);
        src_logical += need;
        src_byte = end_byte;
        emitted += need;

        out.extend_from_slice(bytes);
        emitted += 1;
    }
    out.extend_from_slice(&data[src_byte..]);
    out
}
