# Built-in optimiser as the primary VGM compressor

**Branch:** `optimizer-investigation` (this programme) · **Status:** parts 1 + 2
implemented; part 3 deferred.

## Why

An investigation (see *Findings* below) established that the in-house
`VgmFile::optimize()` can stand in for the external `vgm_cmp` C tool — it reduces
as well, and **both tools currently corrupt YM2612 audio**, so "own the
optimiser" is the correct direction. This programme makes the built-in the
primary path, keeps the external tools as a *fallback* for chips the built-in
does not yet safely cover, and adds a Settings choice — while **deferring** the
per-chip expansion that would let the tools be retired entirely.

## Findings (empirical, 500-file corpus render comparison)

- **Size:** built-in shrank 413 files at 19.6%; `vgm_cmp` 358 at 19.7% — equal,
  built-in touches more files.
- **The built-in has a real, shipping bug:** it drops "redundant" same-value
  writes to YM2612 frequency registers `0xA0`–`0xA6`, but on the OPN family those
  are a **latch pair** — an `0xA4`–`0xA6` write latches the high byte/block, and
  the *following* `0xA0`–`0xA2` low-byte write *commits* both. Dropping a
  low-byte write whose value is unchanged, when the high byte was re-latched
  since, loses the frequency change → wrong pitch. 25/500 files, all YM2612,
  audible (peaks to ~41000). The existing `optimize_corpus` gate renders OPL only
  (Nuked-OPL3), so it never saw this. Root cause: `chip_state::latch_rule` treats
  every YM2612 register except `0x28`/`0x2A` as an independent pure latch
  ([chip_state.rs:237](../../crates/vgms-core/src/chip_state.rs)).
- **`vgm_cmp` is idempotent** (twice == once on 0/500) but **corrupts 25/500
  pure-YM2612 files on the first pass** — its own handler bugs (OPN2 prescaler,
  OPL2 WSG toggle, SAA1099 routed through the OPLL handler).
- **The engine renders byte-exact under write removal:** `VgmEngine` applies
  writes immediately at wait-boundaries (no write-spreading buffer), so an
  optimiser that only drops truly-redundant writes renders *identically* to the
  original — which is why an unoptimised-vs-optimised render diff is a real state
  change, not a phase artifact, and is the correct safety oracle.

## Corpus

The canonical VGMRips corpus lives at
`F:\GameMusic\VGM\VGMRips_all_of_them_2025-10-17`. Point
`VGMSTUDIO_VGMRIPS_CORPUS` there for the parity gate and the investigation
harness.

## Decisions

- **D-opt-1 — the parity gate compares *unoptimised* against *optimised*.** For
  every corpus file, render the original and the optimiser's output through the
  engine and require them **byte-identical**, tallied per chip. This is the
  arbiter of which chips the built-in may safely optimise.
- **D-opt-2 — the built-in is *self-safe*.** It only drops writes for chips whose
  rules the gate confirms. **YM2612's rule fails the gate, so for the interim it
  is disabled** — the built-in drops nothing for YM2612, which falls back to
  `vgm_cmp`. Proper OPN latch modelling is part 3a. OPL stays (proven by
  `optimize_corpus`); YM2413 is kept only if the gate passes it.
- **D-opt-3 — fall back to the external tools per file.** A file whose every chip
  is in the built-in's safe set is optimised by the built-in alone (no child
  processes). A file carrying any chip the built-in does not cover falls back to
  the external tools (`vgm_cmp` / `optdac` / `vgm_sro`) plus the built-in's
  chip-agnostic delay-merge, which is always safe. True per-chip routing is
  unnecessary once part 3 completes the coverage and the tools retire.
- **D-opt-4 — the vgm_cmp YM2612 corruption is a known, pre-existing interim
  limitation.** Disabling the built-in's buggy rule *removes* the built-in's
  contribution to it; `vgm_cmp`'s own first-pass YM2612 bug remains until part 3a
  gives the built-in correct OPN handling and moves YM2612 off the tool.

## Stages

### Stage 1 — the parity gate + built-in self-safety *(part of part 1)*

- **s1-1** Promote the investigation harness into a permanent, per-chip parity
  gate: `optimize_corpus`-style, but rendering **all chips** through `VgmEngine`
  (byte-exact under write removal) and comparing original vs built-in output.
  Ignored by default, driven by `VGMSTUDIO_VGMRIPS_CORPUS`; prints a per-chip
  pass/fail tally so a newly-unsafe chip is named.
- **s1-2** Make the built-in self-safe: disable the YM2612 `latch_rule` (it fails
  s1-1), documenting the OPN commit-latch reason inline; confirm OPL passes;
  keep YM2413 iff the gate passes it. Close the shipping YM2612 corruption.

**Exit:** the gate is green for every chip the built-in still claims a rule for.

### Stage 2 — the optimiser router / fallback *(the rest of part 1)*

- **s2-1** Add `built_in_covers(chip) -> bool` (the gate-verified safe set).
- **s2-2** Restructure `optimize_vgm_with` (or a wrapper): a fully-covered file →
  built-in only; otherwise → the external tools fallback + the built-in's safe
  delay-merge. Keep the existing OPL bypass as the covered-file case it already
  is. No file is worse off than today, and fully-covered files stop spawning
  child processes.

**Exit:** `optimize_parity` (full pipeline render parity) green; the router picks
built-in for covered files and tools for the rest.

### Stage 3 — the Settings choice *(part 2)*

- **s3-1** Config: an `optimizer` enum — `Auto` (the router), `BuiltInOnly` (never
  spawn the external tools — for the web and minimal-dependency builds, accepting
  only the delay-merge on uncovered chips), `Tools` (force the external tools, the
  old behaviour and an A/B control). Default `Auto`.
- **s3-2** Thread the choice through `Options` into the pack service, the CLI
  `optimize` subcommand, and the editor's optimise action.
- **s3-3** A selector in the Settings dialog, and its row in the Help dialog's
  table (per the repo convention that a shortcut/setting change updates Help).

**Exit:** the choice is settable, honoured on every optimise path, and covered by
a UI test.

### Deferred — Part 3: complete the built-in's chip coverage

Expand `latch_rule` / the built-in to each chip the fallback still needs, each
admitted only when Stage 1's gate passes it, in priority order from the size gap:

- **3a — the `vgm_cmp` chips**, YM2612 first (model the OPN `0xA4→0xA0` commit
  latch the way `vgm_cmp`'s look-ahead does, recovering full YM2612 compression
  and moving Mega Drive off the buggy tool), then SN76489 (mind the noise-shifter
  reset), the OPN2/OPM/OPNA/OPL variants, etc.
- **3b — the PCM compressors:** `optdac` (YM2612 DAC-run collapse) first, then the
  sample-ROM trims (`vgm_sro`, per-chip behind the gate as
  `which_chips_the_sample_rom_trim_is_safe_for` already vets).

When coverage is complete the external tools — and the `vgm_cmp` YM2612 bug — are
retired.

## Gating discipline

Every increment must keep, at the corpus:
- **Stage 1's gate green** — the built-in never changes audio on a chip it claims.
- **`optimize_parity` green** — the router's output renders identically to the
  original.
- **`optimize_corpus` green** — the OPL byte-exact + idempotence proofs.

and `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test
--workspace` at every commit.
