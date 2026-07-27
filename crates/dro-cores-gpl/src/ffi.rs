//! The C ABI of the pinned upstream cores, and the only `unsafe` in this crate.
//!
//! Declared by hand rather than bindgen, and with **no struct mirrored**: the
//! state is allocated by a size the C reports, so an upstream that adds a field
//! changes a number rather than silently outgrowing a Rust twin of itself. The
//! same reasoning as `dro-cores-nuked`'s, and the same shape.

use std::ffi::c_void;

unsafe extern "C" {
    // Ours (shim/layout.c), so the size comes from the compiler.
    fn drotrim_opll_sizeof() -> usize;
    fn drotrim_opll_alignof() -> usize;

    fn OPLL_Reset(chip: *mut c_void, chip_type: u32);
    fn OPLL_Clock(chip: *mut c_void, buffer: *mut i32);
    fn OPLL_Write(chip: *mut c_void, port: u32, data: u8);
}

/// Upstream's `opll_type_ym2413`: the Yamaha part a VGM means by "YM2413".
const OPLL_TYPE_YM2413: u32 = 0x00;
/// Upstream's `opll_type_ds1001`: Konami's VRC VII, which a VGM signals with
/// bit 31 of the clock.
const OPLL_TYPE_DS1001: u32 = 0x01;

/// Zeroed bytes sized for the upstream chip struct.
///
/// `u64`-backed for eight-byte alignment; the constructor asserts rather than
/// assumes, because a silently under-aligned struct is undefined behaviour that
/// usually looks like it works.
struct OpaqueChip {
    storage: Box<[u64]>,
}

impl OpaqueChip {
    fn new(size: usize, align: usize) -> Self {
        assert!(
            align <= align_of::<u64>(),
            "the upstream wants {align}-byte alignment; this can promise {}",
            align_of::<u64>()
        );
        Self {
            storage: vec![0u64; size.div_ceil(size_of::<u64>()).max(1)].into_boxed_slice(),
        }
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.storage.as_mut_ptr().cast()
    }
}

impl std::fmt::Debug for OpaqueChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueChip")
            .field("bytes", &(self.storage.len() * size_of::<u64>()))
            .finish()
    }
}

// SAFETY: plain zeroed memory owned solely by this value, and the upstream
// keeps no global mutable state reachable through it -- `chip_type` is a field
// of the struct here, not a `static` as Nuked-OPN2's is.
unsafe impl Send for OpaqueChip {}

/// A Nuked-OPLL chip.
#[derive(Debug)]
pub(crate) struct OpllChip {
    state: OpaqueChip,
}

impl OpllChip {
    pub(crate) fn new() -> Self {
        // SAFETY: both shims return a compile-time constant and touch nothing.
        let (size, align) = unsafe { (drotrim_opll_sizeof(), drotrim_opll_alignof()) };
        Self {
            state: OpaqueChip::new(size, align),
        }
    }

    /// Re-initialises as a YM2413, or as Konami's VRC VII variant.
    pub(crate) fn reset(&mut self, vrc7: bool) {
        let kind = if vrc7 {
            OPLL_TYPE_DS1001
        } else {
            OPLL_TYPE_YM2413
        };
        // SAFETY: the block is sized by the C's own `sizeof(opll_t)`, so the
        // memset inside `OPLL_Reset` stays within the allocation.
        unsafe { OPLL_Reset(self.state.as_ptr(), kind) }
    }

    /// Presents `data` on `port` (0 selects a register, 1 its value).
    pub(crate) fn write(&mut self, port: u32, data: u8) {
        // SAFETY: as above; the call writes only inside the chip block.
        unsafe { OPLL_Write(self.state.as_ptr(), port, data) }
    }

    /// Advances one internal cycle and returns the melody and rhythm outputs.
    ///
    /// The chip has two DACs, time-multiplexed across its rotation, so a sample
    /// is the whole rotation of both summed.
    pub(crate) fn clock(&mut self) -> (i32, i32) {
        let mut out = [0i32; 2];
        // SAFETY: upstream writes exactly two i32s through `buffer`, and `out`
        // is two i32s. The pointer is not retained past the call.
        unsafe { OPLL_Clock(self.state.as_ptr(), out.as_mut_ptr()) }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zero size would mean the shim did not link and every core would be
    /// writing into a one-word allocation.
    #[test]
    fn the_shim_reports_a_real_size() {
        // SAFETY: both return compile-time constants.
        let (size, align) = unsafe { (drotrim_opll_sizeof(), drotrim_opll_alignof()) };
        assert!(size > 128, "opll_t came back as {size} bytes");
        assert!(align <= align_of::<u64>(), "{align}");
    }
}
