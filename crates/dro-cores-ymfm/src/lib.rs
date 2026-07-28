// SPDX-License-Identifier: MIT OR Apache-2.0
//! Yamaha cores from [ymfm](https://github.com/aaronsgiles/ymfm), the
//! BSD-3-Clause library Aaron Giles wrote and MAME adopted.
//!
//! This crate is the first piece of the reuse re-scope
//! (`docs/vgm-multichip-2026-07/CORES-REUSE-PLAN.md`): upstream emulators
//! become the accuracy tier, and the clean-room cores in `dro-synth` become
//! the wasm-and-fallback tier. ymfm covers the whole Yamaha family in one
//! maintained dependency -- including the three chips our own cores are
//! weakest on (YM2608, YM2610, YMF278B) and one they cannot register at all
//! (Y8950).
//!
//! Because ymfm is permissive, this provider imposes nothing on consumers:
//! unlike `dro-cores-nuked` (LGPL) and `dro-cores-gpl` (GPL), the accuracy
//! tier for these chips is `MIT OR Apache-2.0`.
//!
//! **Native only.** ymfm is C++ and `wasm32-unknown-unknown` has no C++
//! standard library, so the app excludes this crate on web and the registry
//! lists what exists -- the mechanism CORES-PLAN §4 already specifies.

mod ffi;

pub use ffi::{Kind, YmfmChip};
