// SPDX-License-Identifier: GPL-2.0-or-later
//! A deterministic `rand`/`srand` for the vendored cores, on every target.
//!
//! Several libvgm cores reach for C's `rand()`: the NES APU randomises its
//! noise LFSR and triangle phase at reset (`np_nes_dmc.c`), SameBoy's DMG
//! simulates the unpredictable wave-RAM address bus of its "bugged read"
//! (`sameboy_apu.c`), and adlibemu, the HuC6280 and the SCSP dither likewise.
//! Linked against a platform CRT, those draws make a render a function of the
//! process's RNG history rather than of the file alone -- two renders of one
//! file differ. That breaks [`ChipCore`](vgms_synth::chip::ChipCore)'s
//! determinism promise, and forces the optimiser's render-parity gate to skip
//! every file carrying such a chip (it cannot tell a dropped-write regression
//! from the core's own reset noise).
//!
//! `build.rs` redirects the cores' `rand`/`srand` to the symbols below on all
//! targets (via the force-included `shim/rand_shim.h`), so the C never reaches
//! a CRT `rand`. The state is **thread-local**, so two engines rendering
//! concurrently on different threads never share a stream; and it is reseeded
//! to a fixed
//! constant whenever a chip resets ([`seed_deterministic`], called from
//! `LibVgmChip::reset`), so a given file renders identically every time. A
//! reset happens at engine construction and at every seek, never mid-render, so
//! the shared per-thread stream that a render draws from is left untouched --
//! which is what lets the parity gate see a dropped write desynchronise it.
//!
//! Before this, `wasm32` supplied its own deterministic `rand` (there is no CRT
//! to borrow) while native quietly linked the platform's. This module unifies
//! the two: one deterministic implementation, redirected on both.

// `pub` documents that these are an exported ABI surface, which
// `unreachable_pub` cannot see.
#![allow(unreachable_pub)]

use std::cell::Cell;
use std::ffi::c_int;

/// The baseline every render starts from -- glibc's classic `drand48` seed,
/// chosen only because it is a well-mixed constant, not for any property of it.
const SEED: u64 = 0x5DEECE66D;

thread_local! {
    /// This thread's LCG state. One stream per thread, so concurrent renders
    /// never draw from each other.
    static STATE: Cell<u64> = const { Cell::new(SEED) };
}

/// One step of the LCG, returning a value in `0..=0x7FFF_FFFF` (`RAND_MAX`).
fn next() -> c_int {
    STATE.with(|state| {
        // Numerical Recipes' LCG constants; the high bits are the good ones,
        // which is why the return shifts them down rather than masking the low.
        let stepped = state
            .get()
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state.set(stepped);
        ((stepped >> 33) & 0x7FFF_FFFF) as c_int
    })
}

/// C ABI `rand`, redirected here from the vendored cores by `build.rs`.
#[unsafe(no_mangle)]
pub extern "C" fn vgms_libvgm_rand() -> c_int {
    next()
}

/// C ABI `srand`, redirected here from the vendored cores by `build.rs`. No
/// core calls it, but the redirect names it, so it must exist.
#[unsafe(no_mangle)]
pub extern "C" fn vgms_libvgm_srand(seed: u32) {
    STATE.with(|state| state.set(u64::from(seed)));
}

/// Resets this thread's RNG to the fixed baseline, so a render is a pure
/// function of the file. Called from every chip reset -- setup and seek, never
/// mid-render.
pub(crate) fn seed_deterministic() {
    STATE.with(|state| state.set(SEED));
}
