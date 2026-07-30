# Any-chip VGM optimization, by binding vgmtools

> **Status: IN PROGRESS.** Approved 2026-07-30, implemented on branch
> `vgmtools-binding` off `vgm-multichip`. Steps ot-1..ot-8, one commit each.

## Context

Optimization today covers three chips. `chip_state::latch_rule`
(`crates/dro-core/src/chip_state.rs:232`) knows the OPL family, the YM2612 and
the YM2413; every other chip is deliberately untouched, on the rule written
into its doc comment -- *chips earn a rule by being checked, not by being
present*. That was the right call for a clean-room table maintained by hand:
a register that **triggers** on write rather than latching makes the generic
"same value, drop it" rule audibly wrong, and the failure is silent -- the
file gets smaller and plays wrong.

vgmtools has spent two decades accumulating exactly that table. `vgm_cmp`
covers ~30 chips, each with hand-tuned rules for the registers that must never
be dropped (key-ons, counter reloads, phrase triggers, chip-mutated address
registers), the ones that need masked compares, and the ones that need
forward-lookahead to prove a write is a dead no-op. `vgm_sro` strips unused
bytes out of sample ROMs by *executing* each chip's address registers through
~26 cut-down decoders (`chip_srom.c`) -- typical rips fall to 5-10% of their
raw size. These are the last vgmtools relevant to pack preparation that we
lack.

## The decision: bind, do not re-implement

**User decisions (2026-07-30):**

1. **Bind the original GPL C code.** The same posture as the 2026-07-29 libvgm
   redirect: upstream is the source of truth, and the accumulated per-chip
   knowledge is the whole value. This also dissolves the question the plan
   started as -- "do we have functional equivalence with `vgm_cmp`?" -- because
   equivalence stops being something to verify and becomes something that
   holds by construction.
2. **Scope:** `vgm_cmp` (all-chip write dedup) + `vgm_sro` (sample-ROM trim) +
   `optdac` (YM2612 DAC run cleanup).
   **Out of scope:** `optvgmrf` (niche RAM coalescing), `dacopt` (raw ->
   DAC-stream conversion; an authoring tool, not a pack tool), `opt_oki`
   (upstream marks it alpha, *"not for public use"*, and it needs per-rip
   source edits), `vgm_dbc` (bit-packing -- packs gzip anyway, the same
   argument that deferred the mc-10 header shrink), `vgm_tt` (user: never).
3. **Full `vgm_cmp` parity** in dedup depth -- free, now that we run the real
   thing rather than a re-spelling of it.

### Where our optimizer stood against `vgm_cmp`

Recorded because it is the baseline the binding replaces, and because the
agreements are load-bearing invariants the composed pipeline must preserve.
Where the two overlap (OPL family, YM2612, YM2413) they already agree on
everything that matters: the first write to a register is always kept (neither
assumes power-on values), dedup state resets at the loop point so the loop
body re-establishes itself, dual-chip instances are tracked separately,
dropping a write never changes timing, and both drop OPL `0x04` flag-clears.
Our differences were all *conservative*: we keep the YM2612's `0x28`/`0x2A`
and the YM2413's key registers that `vgm_cmp` partially dedupes, and we skip
its masked compares and lookahead drops -- slightly less shrink, never more
risk. Our delay re-encoder is the one place we are strictly stronger: it is
provably byte-minimal (`optimize.rs:382`), `vgm_cmp`'s is not. The real gap
was coverage: ~30 chips against 3.

## Licensing fit (verified)

vgmtools is GPL-2.0. The binding lives in a new leaf crate `dro-vgmtools`
(GPL-2.0-or-later). Both call sites are already GPL-2.0-or-later -- `dro-ui`
(Edit menu) and `dro-trimmer` (CLI, pack export) -- so both may link it
directly and no crate changes licence. `dro-core` and `dro-synth` stay
MIT OR Apache-2.0 and keep the existing pure-Rust optimizer, which remains
the wasm-clean path (`dro-web` and `dro-synth-worklet` never see this crate).

## Architecture

- **Submodule** `vendor/upstream/vgmtools`, pinned. Never edited -- house rule.
- **New crate `crates/dro-vgmtools`**, shaped like `dro-cores-libvgm`: a
  `build.rs` compiling the needed C with `cc` + clang. Per-tool wrapper `.c`
  files (`#define main vgm_cmp_main` then `#include "vgm_cmp.c"`) so upstream
  stays pristine. The tools are standalone programs, not a library: each
  defines its own `VGMHead`, `OpenVGMFile`, and friends, so symbol collision
  between them is the spike's problem to solve -- preferred route is one
  object per tool with `llvm-objcopy --prefix-symbols=`, fallback is one
  staticlib per tool, last resort is subprocess invocation of built exes.
- **I/O model:** in-process `<tool>_main(argc, argv)` over temp files (the
  tools are file-path driven). `stdin` is EOF under the GUI, so `getchar()`
  prompt paths return immediately; the spike audits whether `exit()` is
  reachable on malformed input and shims it if so.
- **zlib** via `libz-sys` (static): the tools gz-read/write. We always hand
  them uncompressed temp files; `.vgz` stays ours.
- **Safe API** (`dro-vgmtools/src/lib.rs`): `optimize_writes(&[u8])`,
  `trim_sample_roms`, `clean_dac_runs`, each returning
  `ToolOutcome = Smaller(Vec<u8>) | Unchanged | Failed(String)` -- mirroring
  each tool's own "only write if smaller" gate. Plus `passthrough_chips()`,
  the chips `vgm_cmp` copies verbatim (MultiPCM, K053260, PWM, GA20, SAA1099,
  ...), so the export log can stay honest about what was left alone.
- **Composed pipeline** `optimize_vgm(bytes) -> ToolOutcome`:
  1. `clean_dac_runs`, only when the header declares a YM2612,
  2. `optimize_writes` (`vgm_cmp`),
  3. `trim_sample_roms` (`vgm_sro`),
  4. finish with the existing `VgmFile::optimize`
     (`crates/dro-core/src/vgm/file.rs:589`) -- its redundancy pass is
     subsumed but harmless, and its byte-minimal delay re-encoder out-spells
     `vgm_cmp`'s delay writer.
  **A wholly-OPL file bypasses the C entirely** and keeps the current
  pure-Rust path, so the 3933-file byte-pins in `projection_corpus`'s
  `compare_optimised` stay meaningful and untouched.
- Each tool relocates the loop offset, GD3 and EOF itself; we re-read the
  result through `VgmFile::read`, so header state is re-derived and undo stays
  the existing whole-file `ReplaceVgm` swap.

## Steps (one commit each)

**ot-1 -- the spike (GATE).** Like cr-3's CQM proof-of-concept. Add the
submodule; compile `vgm_cmp`+`chip_cmp`, `vgm_sro`+`chip_srom`, `optdac`
through wrappers; solve symbol isolation; run each in-process on a fixture and
byte-compare against the upstream exe on the same input. Audit `exit()` and
prompt paths. Record the chosen route in the crate's `lib.rs` header note.
**GO/NO-GO:** if in-process proves unreasonable, the crate wraps subprocesses
of exes built by `build.rs` -- the API is unchanged and the rest of the plan
is unaffected.

**ot-2 -- the crate.** `dro-vgmtools` with the safe API above, temp staging,
and captured (not leaked) stdout. Golden tests: fixture in, pinned bytes out,
goldens generated once from the upstream exes and committed.

**ot-3 -- the composed pipeline, and honesty.** `optimize_vgm` with the
wholly-OPL bypass. Tests assert delay totals conserved (`VgmStream`'s wait
prefix, before and after) and idempotence (a second run returns `Unchanged`).
`unoptimised_chips` reporting switches to `vgm_cmp`'s passthrough list when
the pipeline is in use.

**ot-4 -- the CLI.** `drotrim optimize`
(`crates/dro-trimmer/src/cli/optimize.rs`) runs the pipeline, reports
per-stage savings, and gains `--no-rom-trim` / `--no-dac-clean` (write dedup
is always on). `.vgz` output re-gzips as today.

**ot-5 -- pack export.** `optimize_song` / `process_entry`
(`crates/dro-trimmer/src/pack_zip.rs:132`) route through the pipeline under
the one existing "Optimize VGMs on export" toggle, with per-stage log lines.
Never fatal: a `Failed` stage passes the file through verbatim and logs, as
today.

**ot-6 -- the Edit menu.** `Action::OptimizeVgm` (`dro-ui/src/app.rs:1858` ->
`editor.rs`'s `optimize_vgm_document`) uses the pipeline through a direct
`dro-ui` -> `dro-vgmtools` dependency (both GPL). The status line reports the
stage breakdown; undo is unchanged; a kittest covers the non-OPL path; any UI
string change regrows the settings snapshots (`UPDATE_SNAPSHOTS=1`).

**ot-7 -- corpus verification** (`#[ignore]`, `DROTRIM_CORPUS`). Per chip in
the chip index, sample N files, run the pipeline, and assert: delay totals
conserved, idempotence, and **`VgmEngine` render byte-parity** -- render the
original and the optimized file through the deterministic engine and require
identical samples. That last one is the audio gate for both `vgm_cmp`'s drops
and `vgm_sro`'s ROM trims: a wrong usage decoder changes the render, and
parity catches it. Re-run the OPL pins to prove the bypass. Report aggregate
size reduction per chip.

**ot-8 -- docs, credits, memory.** A PROVENANCE-style note in `dro-vgmtools`
(GPL binding, upstream commit pin, why there is no clean-room version); About
dialog credits; update `HANDOVER.md`'s "adjacent tools" note; update the
`vgmrips-pack-gaps` memory.

## Verification

- ot-1's golden byte-comparison against the upstream exes -- the binding *is*
  the exe, so this is the equivalence proof.
- ot-7's corpus render-parity through `VgmEngine`; no external reference
  needed, the engine is deterministic.
- Existing suites stay green: `projection_corpus`'s `compare_optimised`
  byte-pins (proving the OPL bypass), `optimize_parity.rs`, the `pack_zip`
  tests, and workspace fmt/clippy including the wasm target -- `dro-vgmtools`
  is desktop-only and no dependency of `dro-web` or `dro-synth-worklet`, so
  `cargo check --target wasm32-unknown-unknown` is unaffected.

## Risks and notes

- **`exit()` and prompts in tool error paths.** The spike audits them; the
  subprocess fallback is ready. Inputs are files we serialized ourselves, so
  the malformed-input paths are cold.
- **`vgm_cmp` re-spells delays** on non-OPL files through its own writer; our
  final `dro-core` pass re-minimizes them, and the shrink gates leave
  no-net-win files untouched.
- **Bytes after the end marker** are dropped by our `VgmFile::optimize` and by
  the C tools alike. Pre-existing and consistent; ot-3 pins it as behaviour
  rather than calling it a fix.
- **MSVC vs clang:** `cc` must pick clang, as `dro-cores-libvgm` does -- the
  same PATH-prelude trap recorded in the toolchain memory.
- **`vgm_sro` splits one ROM block into several smaller `0x67` blocks**, with
  the declared total size preserved. Our `banks.rs` ROM path and the libvgm
  cores already handle multi-part ROM loads (each block carries its start
  offset); ot-7's render parity is the proof.
