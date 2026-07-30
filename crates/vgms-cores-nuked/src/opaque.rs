//! Chip state allocated by a size the C side reports, never mirrored in Rust.
//!
//! A `#[repr(C)]` twin of an upstream `struct` becomes memory corruption the
//! moment the upstream adds a field, which a pulled submodule makes a question
//! of when, not whether. So nothing is mirrored: the C reports `sizeof` and
//! `alignof`, this allocates that many bytes, and Rust never looks inside. The
//! cost is that the state cannot be `Clone`d field-wise or printed usefully,
//! neither of which any core here wants.

use std::ffi::c_void;

/// The alignment this can guarantee, being backed by `u64`.
///
/// Every upstream here is plain C with no over-aligned members, so 8 is ample;
/// the constructor asserts rather than assumes, because a silently under-aligned
/// struct is undefined behaviour.
const GUARANTEED_ALIGN: usize = align_of::<u64>();

/// A block of zeroed bytes sized for one upstream chip struct.
pub(crate) struct OpaqueChip {
    /// `u64` rather than `u8` for the alignment; the length is the byte size
    /// rounded up.
    storage: Box<[u64]>,
}

impl OpaqueChip {
    /// Allocates `size` zeroed bytes, aligned to at least `align`.
    ///
    /// # Panics
    /// If the upstream wants an alignment stronger than `u64`'s -- which none
    /// does, and which would need a different backing type if one ever did.
    pub(crate) fn new(size: usize, align: usize) -> Self {
        assert!(
            align <= GUARANTEED_ALIGN,
            "an upstream chip struct wants {align}-byte alignment; \
             this is backed by u64 and can only promise {GUARANTEED_ALIGN}"
        );
        // Zeroed is the right initial state for every core here: each one's
        // reset function memsets before doing anything else, so this only has
        // to be *valid* to hand out a pointer to.
        let words = size.div_ceil(size_of::<u64>()).max(1);
        Self {
            storage: vec![0u64; words].into_boxed_slice(),
        }
    }

    /// A pointer to the state, for passing to the upstream's functions.
    pub(crate) fn as_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}

impl std::fmt::Debug for OpaqueChip {
    /// The bytes are the upstream's business, so the size is all there is to say.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueChip")
            .field("bytes", &(self.storage.len() * size_of::<u64>()))
            .finish()
    }
}

// SAFETY: the block is plain zeroed memory owned solely by this value, and the
// upstream cores keep no global mutable state reachable through it -- each
// chip's state lives inside its own struct. (Nuked-OPN2's `OPN2_SetChipType`
// is a global, but `opn2.rs` handles that as a correctness hazard, not an
// aliasing one.)
unsafe impl Send for OpaqueChip {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_big_enough_and_aligned() {
        // A size that is not a multiple of 8 must still fit.
        let mut chip = OpaqueChip::new(60, 8);
        assert!(chip.storage.len() * size_of::<u64>() >= 60);
        assert_eq!(chip.as_ptr() as usize % GUARANTEED_ALIGN, 0);

        // And a zero-size struct still yields a valid pointer rather than a
        // dangling one, since the C will be handed it regardless.
        let mut empty = OpaqueChip::new(0, 8);
        assert!(!empty.as_ptr().is_null());
    }

    #[test]
    fn the_block_starts_zeroed() {
        let chip = OpaqueChip::new(256, 8);
        assert!(chip.storage.iter().all(|&word| word == 0));
    }

    #[test]
    #[should_panic(expected = "wants 16-byte alignment")]
    fn an_over_aligned_struct_is_refused_rather_than_under_aligned() {
        let _ = OpaqueChip::new(64, 16);
    }
}
