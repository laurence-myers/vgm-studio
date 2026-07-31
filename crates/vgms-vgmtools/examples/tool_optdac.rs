// SPDX-License-Identifier: GPL-2.0-or-later
//! `optdac` as a standalone `.wasm` module.
//!
//! Collapses long runs of identical YM2612 DAC writes, in its own linear
//! memory. Same host ABI as `tool_vgm_cmp`: `reserve_input` -> `run()` ->
//! `output_ptr`/`output_len` + `log_ptr`/`log_len`.

vgms_vgmtools::wasm_tool!(optdac_main, "vgmtools_wasm_dac");
