# vgms-vgmtools: where the code came from, and why it is not ours

## What is here

Three programs from [vgmtools](https://github.com/vgmrips/vgmtools), built from
the pinned submodule at `vendor/upstream/vgmtools` and shipped inside the
`vgmstudio` binary:

| Tool | What it does |
|---|---|
| `vgm_cmp` | Drops chip writes that change nothing, on per-chip rules for ~30 chips |
| `vgm_sro` | Strips sample-ROM regions no register write can reach |
| `optdac` | Collapses runs of 128+ identical YM2612 DAC writes |
| `vgm_ptch` | Edits the header; used here to strip chips the stream never writes to |

Plus `shim/zlib.h` and `shim/zshim.c`, which are ours: about a hundred lines
serving the seven `gz*` calls the tools make from `FILE*`, so no C compression
library enters the build. Everything is stored, never deflated -- `.vgz` is
flate2's job in Rust, above this layer.

`vgm_ptch` is the odd one out in three ways, each found rather than assumed: it
patches its file **in place** with no output argument (`vgm_ptch.c:283`), so its
caller works on a copy; it writes through `gzwrite` and asks `gzdirect`, where
the others use plain `fwrite`; and its `-StripList` pauses mid-listing on a bare
`_getch()` (`vgm_ptch.c:200`) that `MSYSTEM` does not disarm. Nothing here
invokes that command, but it is why the runner's deadline is not optional.

## Licence

vgmtools is **GPL-2.0** and ships a `LICENSE` saying so -- unlike libvgm, where
the grant is unresolved and this workspace had to guess conservatively. This
crate is therefore GPL-2.0-or-later, and only the copyleft half of the
workspace links it: `vgms-ui` (Edit > Optimize) and `vgms-app` (the CLI and
pack export). `vgms-core` and `vgms-synth` stay MIT OR Apache-2.0 and never
depend on it.

Because the executables are built into the distributed binary, the About box
credits them and points at the upstream source. That is the licence obligation,
not a courtesy.

**The submodule is never edited.** Upgrading is
`git -C vendor/upstream/vgmtools pull`, a pin bump, and a re-run of the golden
tests plus the corpus render parity.

## Why bind rather than re-implement

`vgms_core::redundancy` explains the discipline: a chip earns a redundancy rule
by being checked, because a register that *triggers* on write rather than
latching makes the generic "same value, drop it" rule audibly wrong, and the
failure is silent -- the file gets smaller and plays wrong.

`chip_cmp.c` is two decades of exactly that checking, for about thirty chips:
key-ons that re-attack, counters that reload, addresses the chip itself moves
during playback, masked compares, forward lookahead to prove a write is a dead
no-op. It is the source `vgms_core::redundancy` was written *from*, chip by
chip and stricter wherever upstream is known to be wrong, and it is still what
the `Tools` setting runs as the A/B control.

`chip_srom.c` is twenty-six more chip models, for the ROM trim, and has no
in-house peer at all. Re-spelling it in Rust would mean re-deriving every one
of those judgements about which ROM bytes a write history can reach, and
getting one wrong is inaudible in a test and audible in a pack.

So we run the originals, and equivalence stops being something to verify.

## Why child processes

Measured, not assumed -- the full argument is ot-1 in
`docs/vgm-multichip-2026-07/OPTIMIZER-PLAN.md`. The in-process binding was
built and worked, and re-entrancy measured clean. It was rejected because:

- `chip_srom.c` has 50 `realloc` sites and one `free` (line 650) that releases
  only the array holding the pointers, and `InitAllChips` *zeroes* those
  pointers (line 596) rather than freeing them. Every `vgm_sro` run orphans its
  whole sample-ROM set -- correct-because-we-exit code.
- `for (rom_mask = 1; rom_mask < ROMSize; rom_mask *= 2)` (chip_srom.c:3268)
  runs on a `UINT32`: a ROM size above `0x80000000`, read verbatim from a data
  block, wraps the mask to zero and spins forever.

A process boundary contains both, and removes work rather than adding it: no
symbol renaming, no `llvm-objcopy` (the build stays MSVC-only), and each tool
compiled exactly as upstream builds it.

## Traps worth not rediscovering

- **`DblClickWait` waits on `_getch()`** whenever `argv[0][1] == ':'`
  (`common.h:118`) -- every absolute path, which is how a spawned child sees
  itself. The runner sets `MSYSTEM=MSYS`, using upstream's own early return.
- **Exit codes are not all failures.** All three use 0 and 1; `vgm_sro` adds 2
  ("No chips with Sample-ROM used!", most files) and 9 (RF5C memory writes or
  `0x68` PCM RAM writes, which it declines). Both leave the file valid.
- **`vgm_cmp.c:537` is missing a `break`**, so `case 0xBD` (SAA1099) falls into
  `case 0x51` and SAA1099 writes are judged by the YM2413's rules. The
  SAA1099's `0x18`/`0x19` reload an envelope, so a repeated write is a
  retrigger. Held back in `pipeline.rs`.
- **A truncated file is nondeterministic**, even across processes: the tools
  `malloc` what the header claims and ignore `gzread`'s return, so the tail is
  uninitialised heap. `VgmFile::write` recomputes the EOF offset, which keeps
  this out of reach -- but any measurement here must use well-formed files.
- **The tools refuse gzip** (our shim's doing, deliberately). `.vgz` is
  unpacked and repacked by flate2 in Rust, well above this layer.

## Chips held back, and why

In `pipeline.rs`. Each is a decision against a smaller file, and each has a
reason on it:

- **SAA1099**, from `vgm_cmp` -- the missing `break` above.
- **QSound**, from `vgm_sro` -- measured. Running the trim alone over 1200
  corpus files, it fired on QSound and nothing else, and changed what 12 of
  those 23 files play. Whether that blames the trim or our own handling of the
  several smaller `0x67` blocks it splits one into is still open; the
  discriminating experiment (play a trimmed file through VGMPlay) is named in
  `vgm-studio/tests/optimize_parity.rs`.
- **K053260** and **SegaPCM**, from `vgm_sro` -- upstream's own wiki:
  *"It will still incorrectly strip K053260 PCM roms"*, and *"SegaPCM support
  isn't 100% safe. That means there may be samples stripped off despite them
  being used."*

Everything else is **unmeasured rather than cleared**: the trim never fired on
another chip in this corpus. `which_chips_the_sample_rom_trim_is_safe_for` is
the instrument that would produce evidence.
