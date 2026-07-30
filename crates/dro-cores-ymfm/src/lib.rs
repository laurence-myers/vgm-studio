// SPDX-License-Identifier: MIT OR Apache-2.0
//! Yamaha cores from [ymfm](https://github.com/aaronsgiles/ymfm), the
//! BSD-3-Clause library Aaron Giles wrote and MAME adopted.
//!
//! The accuracy tier for the Yamaha family, covered by one maintained
//! dependency -- including the chips our own cores are weakest on (YM2608,
//! YM2610, YMF278B) and one they cannot register at all (Y8950). Being
//! permissive, this provider imposes nothing on consumers: unlike
//! `dro-cores-nuked` (LGPL) and `dro-cores-gpl` (GPL), it is `MIT OR Apache-2.0`.
//!
//! Native only: ymfm is C++ and `wasm32-unknown-unknown` has no C++ standard
//! library, so the app excludes this crate on web.

mod ffi;

pub use ffi::{Kind, YmfmChip};
