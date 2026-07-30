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
- **New crate `crates/dro-vgmtools`**: a `build.rs` that builds each tool as
  its own **executable**, exactly as upstream's CMake does (`vgm_cmp` =
  `vgm_cmp.c` + `chip_cmp.c`, `vgm_sro` = `vgm_sro.c` + `chip_srom.c`,
  `optdac` = `optdac.c`). No wrappers, no renaming: the tools are standalone
  programs and this treats them as such.
- **I/O model:** each call spawns the tool as a child process over temp files,
  **with a timeout**, and reads the result back. The process boundary is the
  feature -- see ot-1 for the four upstream hazards it contains. The child
  gets `MSYSTEM=MSYS` so `DblClickWait` returns instead of waiting on a key.
- **Distribution:** the built exes are embedded in the binary with
  `include_bytes!` and unpacked to a cache directory on first use, so the app
  still ships as one file.
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

**ot-1 -- the spike (GATE). DONE: the answer is SUBPROCESS.** The in-process
route was built end to end and then rejected on evidence. What it cost and
what it found is worth keeping, because all of it is upstream behaviour that
the subprocess route now contains rather than solves:

- **In-process is buildable.** Each tool gathers into one TU with no name
  conflicts; `llvm-objcopy --redefine-syms` (`--keep-global-symbol` is *not*
  supported for COFF) isolates 68/50/17 symbols per tool, and all three link
  and run. So the rejection is not "it did not work".
- **It is also correct, for well-formed files.** Calling a tool once per
  process and calling it 60 times in one process gave byte-identical results:
  `vgm_cmp` 0 divergences over 60 files, `vgm_sro` 0 over 80. The re-entrancy
  worry did not materialise.
- **What decided it was robustness, not correctness.** `chip_srom.c` has 50
  `realloc` sites and exactly one `free` (line 650), which releases only the
  array holding the pointers -- and `InitAllChips` *zeroes* those pointers
  (line 596) rather than freeing them. Every `vgm_sro` run orphans its whole
  sample-ROM set. That is correct-because-we-exit code, and a pack export over
  a hundred arcade tracks is exactly the shape that would turn it into
  hundreds of unreclaimable megabytes inside a long-lived GUI.
- **And a file can hang the process for good.**
  `for (rom_mask = 1; rom_mask < ROMSize; rom_mask *= 2);` (chip_srom.c:3268)
  runs on a `UINT32`: a ROM size above `0x80000000`, read verbatim out of a
  data block, wraps the mask to 0 and spins forever. In-process that is an
  unkillable GUI freeze; as a child it is a timeout and a log line. The same
  goes for the unchecked `malloc` of a header-controlled `lngEOFOffset`
  (vgm_cmp.c:249) and the unchecked `fopen` in `WriteVGMFile` -- an
  access violation that would take the user's unsaved work with it.

A process boundary answers all four at once, and it *removes* work rather than
adding it: no symbol renaming, no `llvm-objcopy` (so no new toolchain
requirement -- the build stays MSVC-only), no one-TU trick, and each tool
compiled exactly the way upstream builds it. Equivalence with `vgm_cmp` stops
being a test result and becomes an identity: we run the program.

Two upstream behaviours the runner must still handle, both found here:
- **`DblClickWait` blocks on `_getch()`** whenever `argv[0][1] == ':'`
  (common.h:118), which is every absolute path -- it hung the reference exe
  for two minutes during the spike. The runner sets **`MSYSTEM=MSYS`** in the
  child environment, using upstream's own early return (common.h:121-128),
  which is robust however the path arrives.
- **A truncated file is nondeterministic.** The tools `malloc` what the header
  claims and ignore `gzread`'s return, so the tail is uninitialised heap; the
  same truncated input gave different results in different processes. The
  binding only ever writes complete files it serialised itself, and
  `VgmFile::write` recomputes the EOF offset, so this stays out of reach --
  but it is why the re-entrancy measurement above insisted on well-formed
  inputs.

**ot-2 -- the crate.** `dro-vgmtools` with the safe API above: temp staging,
captured (not leaked) stdout, `MSYSTEM=MSYS`, a per-call timeout, and the
embedded-exe unpacking. Golden tests pin fixture-in/bytes-out so a submodule
pin bump that changes behaviour shows up as a failing test rather than as
quietly different output.

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

- Equivalence with `vgm_cmp` needs no test: the binding runs the program,
  built from the pinned source the way upstream builds it.
- ot-7's corpus render-parity through `VgmEngine`; no external reference
  needed, the engine is deterministic.
- Existing suites stay green: `projection_corpus`'s `compare_optimised`
  byte-pins (proving the OPL bypass), `optimize_parity.rs`, the `pack_zip`
  tests, and workspace fmt/clippy including the wasm target -- `dro-vgmtools`
  is desktop-only and no dependency of `dro-web` or `dro-synth-worklet`, so
  `cargo check --target wasm32-unknown-unknown` is unaffected.

## Risks and notes

- **A tool that hangs, leaks or faults** takes only its own child process with
  it. Each of those is a real upstream behaviour (ot-1), not a hypothetical,
  and the timeout is what turns the unkillable one into a failed file.
- **`vgm_cmp` re-spells delays** on non-OPL files through its own writer; our
  final `dro-core` pass re-minimizes them, and the shrink gates leave
  no-net-win files untouched.
- **Bytes after the end marker** are dropped by our `VgmFile::optimize` and by
  the C tools alike. Pre-existing and consistent; ot-3 pins it as behaviour
  rather than calling it a fix.
- **zlib is shimmed away.** The tools use exactly four zlib calls
  (`gzopen`/`gzread`/`gzseek`/`gzclose`), all read-only; output goes through
  plain `fopen`/`fwrite`. `shim/zlib.h` + `shim/zshim.c` serve those from
  `FILE*`, so no C compression library enters the build. The shim *refuses*
  gzip input rather than reading it, which is correct here: `.vgz` is
  unpacked and repacked by flate2 in Rust, and gzip reaching this layer would
  mean a caller skipped that.
- **`vgm_sro` splits one ROM block into several smaller `0x67` blocks**, with
  the declared total size preserved. Our `banks.rs` ROM path and the libvgm
  cores already handle multi-part ROM loads (each block carries its start
  offset); ot-7's render parity is the proof.
