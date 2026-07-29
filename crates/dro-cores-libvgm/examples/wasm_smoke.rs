// SPDX-License-Identifier: GPL-2.0-or-later
//! The wasm smoke: proves libvgm's C actually links and *runs* on
//! `wasm32-unknown-unknown`, not merely that it compiles.
//!
//! Built as a `cdylib` so the final link resolves every symbol the C objects
//! reference -- an rlib check would happily skip that -- and exercised from
//! node with `scratchpad`-style loader JS (see the spike notes in
//! `docs/vgm-multichip-2026-07/LIBVGM-PLAN.md` §6). Two exports, chosen for
//! what they prove:
//!
//! - [`smoke_sn76489`]: construction, register writes, render -- the shim's
//!   allocator and the write path end to end.
//! - [`smoke_ym2203_ssg`]: a *linked* SSG child -- the heaviest user of the
//!   allocator family (`calloc` for the link table, a second device start)
//!   and the per-link resampler, audible through the mix.
//!
//! Each returns the rendered peak amplitude: > 1000 means the chip genuinely
//! sounded, 0 means silence, negative values are wiring failures.

use dro_core::vgm::ChipKind;

/// Builds `chip`'s libvgm default from a fresh registry and renders `writes`
/// through it for a second, returning the peak sample.
fn peak_after(chip: ChipKind, writes: &[(u8, u16, u16)]) -> i32 {
    let mut registry = dro_synth::CoreRegistry::new();
    dro_cores_libvgm::register(&mut registry);
    let Some(mut core) = registry.build(chip, None) else {
        return -1;
    };
    core.reset(3_993_600, false);
    core.configure(&dro_core::vgm::ChipSettings::default());
    for &(port, addr, data) in writes {
        core.write(port, addr, data);
    }
    let mut out = vec![0i32; 8192];
    core.render(&mut out);
    out.iter()
        .copied()
        .map(i32::saturating_abs)
        .max()
        .unwrap_or(-2)
}

/// SN76489: latch a tone period, open the volume, listen.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_sn76489() -> i32 {
    peak_after(
        ChipKind::Sn76489,
        &[(0, 0, 0x8E), (0, 0, 0x02), (0, 0, 0x90)],
    )
}

/// YM2203: sound the *linked* SSG child -- channel A tone through the OPN's
/// own register file.
#[unsafe(no_mangle)]
pub extern "C" fn smoke_ym2203_ssg() -> i32 {
    peak_after(
        ChipKind::Ym2203,
        &[(0, 0x00, 0x50), (0, 0x07, 0x3E), (0, 0x08, 0x0F)],
    )
}

// No `main`: the `crate-type = ["cdylib"]` in Cargo.toml makes this a
// library target, and the exports above are its whole surface. The native
// equivalents of these numbers live in `src/chip.rs`'s own sound tests.
