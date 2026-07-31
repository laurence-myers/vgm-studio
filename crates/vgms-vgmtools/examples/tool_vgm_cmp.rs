// SPDX-License-Identifier: GPL-2.0-or-later
//! `vgm_cmp` as a standalone `.wasm` module.
//!
//! Built as a `cdylib` for `wasm32-unknown-unknown`, this is the write-dedup
//! optimiser with its own linear memory. The host reserves the input with
//! `reserve_input`, calls `run()`, and reads the result back through
//! `output_ptr`/`output_len` (0 length == the file was left unchanged) and the
//! failure tail through `log_ptr`/`log_len`. On non-wasm targets the macro
//! expands to nothing, so this example is an empty, harmless cdylib.

vgms_vgmtools::wasm_tool!(vgm_cmp_main, "vgmtools_wasm_cmp");
