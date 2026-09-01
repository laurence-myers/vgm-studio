# Every register chip in the built-in optimiser (part 3a)

**Branch:** `builtin-optimizer-coverage-2026-09` · **Date:** 2026-09-01 ·
**Answers:** the open items in `docs/optimizer-review-2026-08/REVIEW.md`, and the
part 3a deferral in `docs/optimizer-2026-08/PLAN.md`.

## What changed

The built-in optimiser used to have redundancy rules for five chips — the OPL
family and the YM2413 — and handed every other file to `vgm_cmp`. It now
classifies **all 42 chips the VGM format defines**, register by register, in a
new module: `crates/vgms-core/src/redundancy.rs`.

`vgms_core::chip_state` keeps its restore/diff job; the rules and the
`redundant_indices` walk moved out of it, and the dead `VgmFile::unoptimized_chips`
(§5.1 of the review) is gone.

## The rule model

A write is dropped when its **cell** already holds its **value**. The work was in
deciding what a cell is, because it is not always "this address on this port":

| Shape | Chips | Why |
|---|---|---|
| Plain `(port, address)` | most | the address *is* the state |
| **Chip-wide latch group** | YM2612 / 2203 / 2608 / 2610 | `0xA4`-`0xA6` write one F-number latch that the whole chip shares (`OPN->ST.fn_h`), `0xAC`-`0xAE` another. Cell = the group; value = port + address + byte, so only a verbatim repeat of the *last* write to that latch counts. `0xA0`-`0xA2`, the commit, is never dropped. |
| **Channel-indirected** | RF5C68/164, HuC6280, MultiPCM | registers `0x00`-`0x06` / `0x02`-`0x07` / the data port address whichever channel or slot a select register last named, so the selection is in the cell |
| **Page-indirected** | ES5505/ES5506 | the page register is `0x0D` on one part and `0x0F` on the other and the stream does not say which; both bytes go into every cell, so a write to either forces the next through |
| **Two-byte protocol** | SN76489 | the register travels in the data. A latch byte is droppable only when the next byte to the chip is another latch, so the register select a continuation byte depends on always survives |
| **Forget the chip** | QSound `0xE3` | the write picks an update routine and may reset the part, so no earlier cell can be trusted |

`chip_cmp.c` was the primary source for which registers trigger. Where upstream
is known wrong or switched off, this is **stricter, never looser**: the SAA1099's
envelope registers (upstream's missing `break` routes them through the YM2413's
rules), the Game Boy (upstream's handler is disabled), the YM3812 waveform flush
that never runs, the YM2608 prescaler compare that never assigns.

## The gate, and the thing it could not see

`the_builtin_optimizer_never_changes_audio` renders every corpus file before and
after and requires byte-identical samples. Six rounds:

| Round | Files changing audio | What it caught |
|---|---|---|
| 1 | 40 / 400, 11 chip configurations | the OPN latch keyed per *port* |
| 2 | 8 / 400, 3 configurations | the Game Boy mixer pair; the YM2151's and YM2612's write pacing |
| 3 | 3 / 400, 1 configuration | the SN76489's write pacing |
| 4 | 2 / 800, 1 configuration | the OPL adapter's write spreading (on the *oldest* rule, untouched by this work) |
| 5 | 4 / 1500, 4 configurations | a stale shadow register on the SN76489; the MultiPCM's sample select |
| 6 | **0 / 1500** | — |

### The OPN latch is chip-wide, not per-port

The first round failed on the YM2608, YM2610 and YM2612 alike. libvgm's
`fmopn.c` holds the F-number latch as `OPN->ST.fn_h` — **on the chip, not the
port** — so a `0xA4` on port 1 overwrites what a `0xA4` on port 0 latched, which
is exactly what a Mega Drive or PC-88 driver does every frame. Keying the group
by port dropped the second of two port-0 latches with a port-1 latch between
them. Fixed by making the group cell port-independent; the YM2608 (12 files),
YM2610 (22) and YM2610B (4) went green immediately.

### A kept write still changes the register

The fifth round found the flaw in the *model*, not a chip. A verdict was either
"drop it if the cell holds this" or "keep it" — and keeping recorded nothing. On
the SN76489 the same latch byte is droppable or not depending on what *follows*
it (a continuation byte needs the register select it carries), so a kept latch
left the shadow register holding a nibble the chip no longer had; the next
repeat was then dropped against a stale cache, at peak 29792.

The verdict grew a third arm, `KeepAndRecord`, and every SN76489 write now
records what it left in its half of the register. The invariant is worth stating
because it is easy to lose: **a cell that any write can latch must be updated by
every write that reaches it.** Only chips whose keep/latch split depends on
something other than `(port, address)` can break it — the SN76489 and the
MultiPCM are the two.

### The MultiPCM's sample select is a loader, not a latch

Writing slot register 1 selects a sample, and loading one writes registers 6 and
7 from the sample's own header. So a repeat is not redundant — it undoes
whatever the driver put in 6 or 7 since. Registers 1, 4, 6 and 7 are now kept;
the panpot, pitch and level beside them still dedupe. `vgm_cmp` has the same
handler, commented out, which is how it came to be attempted here at all.

### The Game Boy has no pure latches at all

Eight Game Boy files failed with a rule that looked conservative already. The
blame tool (below) named `NR50`: 1918 dropped master-volume writes, audible from
sample 143390. SameBoy — the core this app ships for the DMG — forces a sample
update on all four channels on **every** write to `NR50`/`NR51`, and models
write-time behaviour on essentially every other audio register too (the `NRx2`
"zombie mode" envelope glitch, `NRx1`'s length reload, `NR43`'s counter
alignment, `NR30`'s wave-RAM corruption through a *random* index).

So the Game Boy's classification is **keep every write** — the same answer
`vgm_cmp` reached by switching its handler off, now with the reason measured and
written down.

### Write pacing makes the write *count* audible on four chips

`register_common_cores` promotes exactly four chips to a Nuked core — the
YM2612, YM2151, YM2413 and SN76489 — and **all four wrappers pace their
writes**: a `vgms_synth::WriteQueue` for the three FM parts, a byte queue with
its own settle for the PSG die trace. Each releases one write per settling
period, because that is what the real chip's busy flag does. Remove a write from
a zero-delay burst and every write behind it reaches the chip *earlier*. So an
optimiser that dropped nothing but genuinely redundant writes still renders
differently, and the gate's premise — "the engine renders byte-exact under write
removal, so a difference is a dropped write that mattered" — does not hold for
those four.

That the promoted-to-Nuked set and the write-paced set are the *same four chips*
is not a coincidence: pacing is what a die-level trace needs and an
adapter-tier core does not model.

**The OPL family spreads writes too**, and had done all along — `OplCoreAdapter`
sends ordinary playback through Nuked-OPL3's `OPL3_WriteRegBuffered` and reserves
the immediate `write_reg` for a seek's replay. That is why two YM3812 files
failed the fourth round on the *oldest* rule in the table, which this work did
not touch: the wider 800-file stride simply reached them for the first time. The
gate's own doc comment had asserted the opposite ("`VgmEngine` applies writes
immediately at wait-boundaries, no write-spreading buffer") since it was written.

`VgmEngine::set_immediate_writes` makes the assertion true instead of merely
claimed: it routes every write down the path the seek replay already uses. Off
for playback, on for the gate. With it, the same two files go from 114 of 118
register classes "guilty" to **0 of 118**.

This was measured rather than assumed. `crates/vgms-app/tests/opn_write_pacing.rs`
renders the same 24 optimised YM2612 files through both cores:

```
24 files: Nuked-OPN2 differs on 22, libvgm (immediate writes) on 0
```

The gate therefore renders those four chips through libvgm's immediate-write
cores (`gate_cores`). That is not a softening: a dropped write that really
mattered changes an immediate-write core's output too, which is how the OPN
latch bug was caught on the YM2608 and YM2610, and the Game Boy's mixer pair on
SameBoy — none of those cores are paced.

## Routing

With every chip covered, `built_in_covers_all` is true for every readable file,
so gating the *whole* tool stage on it would have silently retired `optdac` and
`vgm_sro` — which do work the built-in does not do at all. The bypass is now
`vgm_cmp`'s alone; the other two are gated on whether the file has anything for
them (a YM2612; a `0x67` ROM-image block, the new `has_rom_image` fact).

| Choice | optdac | vgm_sro | vgm_cmp | built-in |
|---|---|---|---|---|
| `Auto` | if YM2612 | if ROM image | only if a chip is uncovered | always |
| `BuiltInOnly` | no | no | no | always |
| `Tools` | if YM2612 | if ROM image | always | always |

"A chip is uncovered" is now only reachable two ways: a header that declares no
chip at all, and a header this app cannot read (whose chips are therefore
unknown). Both keep the old fallback deliberately.

`vgm_cmp`'s SAA1099 hold-back survives for `Tools`, the A/B control; under `Auto`
the built-in's own SAA1099 rule takes the file first. The export log's
"not optimized" line now fires only when `vgm_cmp` actually ran, and names it —
claiming a chip was left alone when the built-in had just optimised it was the
one thing the old wording could not say honestly.

## Tools added

- `crates/vgms-app/tests/optimizer_blame.rs` — the gate says *which file*; this
  says *which register*. It groups the writes the optimiser would drop by the
  register they land on and removes one group at a time, so a group whose removal
  alone changes the render names a wrong rule. This is D-orw-4 at write
  granularity, and it found both remaining bugs in one run each.
- `crates/vgms-app/tests/opn_write_pacing.rs` — the write-pacing measurement
  above, kept as the evidence for `gate_cores`.
- `VgmEngine::set_immediate_writes` — one production line of behaviour, off by
  default, so a verification render can ask about chip *state* rather than write
  *delivery*.

## What the corpus actually exercised

The 1500-file stride reached 86 chip configurations covering **38 of the 42
chips**. The four it missed were then swept individually with
`VGMSTUDIO_CHIP_FILTER`, which scans the corpus end to end for one chip:

| Chip | Result |
|---|---|
| POKEY | **30 files, 0 changed** — measured |
| SCSP | no files in the corpus (no Saturn tree in this mirror) |
| Mikey | no files in the corpus (no Lynx tree) |
| ES5505/ES5506 | no files in the corpus |

So three chips are **classified but unmeasurable here**, and their rules are
reasoned rather than checked. The SCSP's is the boldest — it dedupes everything
but each slot's control word and the common block, where `vgm_cmp` dedupes
nothing at all — followed by the ES5505's page model. On the interactive paths
the per-file render gate still stands behind them; on the unverified paths they
are the three to doubt first if a file ever comes back wrong. The module says
so at the top, so the next person does not have to rediscover it.

## Known consequences and what is still open

- **The interactive render gate still rejects write-paced chips.** `Edit >
  Optimize` and the per-track pack optimise call
  `vgms_synth::renders_identically`, which renders through the *configured*
  cores — Nuked for the YM2612, YM2151, YM2413 and SN76489. On those four the
  optimised file is correct but renders a settling period ahead of itself, so
  the gate keeps the original. This is not a regression (`vgm_cmp`'s output was
  rejected on 112 of 120 of the same files, and the YM2413 and SN76489 have been
  in this position since their rules shipped), but it does mean the new YM2612
  and SN76489 coverage delivers on the export and CLI paths and not in the
  editor. The pieces to unlock it now exist — `VgmEngine::set_immediate_writes`
  plus the `gate_cores` substitution — but using them in `renders_identically`
  would mean the gate no longer promises "what you hear will not change" under
  the user's chosen core. **That is a product decision, so it is left for the
  owner**; `gate_cores` is the map of which chips it would touch, and the OPL
  family joins them through the adapter's buffer.
- **The web path is still ungated** (D-orw-7). The web editor's `Edit > Optimize`
  runs `file.optimize()` with no render check. The risk profile is unchanged in
  kind — the CLI and bulk-export paths are ungated too — but it now reaches 42
  chips rather than five.
- **Part 3b is still deferred**: a built-in DAC-run collapser and the per-chip
  sample-ROM trims. They are now the *only* reason the vgmtools binaries ship.
- Chips whose classification is deliberately near-total keep, and could be
  narrowed by someone with the hardware knowledge: Game Boy (all), SCSP,
  K051649's frequency port, C352's address registers, Mikey's timer registers.
