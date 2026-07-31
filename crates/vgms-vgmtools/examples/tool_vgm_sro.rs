// SPDX-License-Identifier: GPL-2.0-or-later
//! `vgm_sro` as a standalone `.wasm` module.
//!
//! The sample-ROM trimmer, in its own linear memory. Same host ABI as
//! `tool_vgm_cmp`: `reserve_input` -> `run()` -> `output_ptr`/`output_len` +
//! `log_ptr`/`log_len`. This is the tool that can spin `chip_srom.c`'s `UINT32`
//! ROM-size mask forever, so the caller guards it (ow-5) and hosts it in a
//! terminable worker (ow-6).

vgms_vgmtools::wasm_tool!(vgm_sro_main, "vgmtools_wasm_sro");
