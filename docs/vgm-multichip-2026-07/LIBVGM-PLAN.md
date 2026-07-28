# LIBVGM-PLAN — libvgm as the primary core source

> **CONDITIONAL PLAN. Read this first.**
>
> This document plans on the **assumption, supplied by the project owner on
> 2026-07-28, that libvgm is GPL-licensed.** That assumption is *not* met by
> the repository as it stands: there is no `LICENSE` or `COPYING` file at
> libvgm's root, GitHub's licence API returns 404, `Compiling.txt` is silent,
> and `EmuStructs.h`, `SoundEmu.h`, `2612intf.c` and `qsound_ctr.c` all carry
> no licence tag (some MAME-derived cores, such as `multipcm.c`, do retain
> `// license:BSD-3-Clause`). See [CORES-REUSE-PLAN.md](CORES-REUSE-PLAN.md) §1
> for the audit.
>
> **What must become true before step lv-1 may start:** an explicit,
> repository-wide grant that covers the *framework* (`EmuStructs.h`,
> `SoundEmu.h`, `EmuHelper.h`, `snddef.h`), not merely per-file tags on some
> cores. A LICENSE file upstream, or written confirmation from Valley Bell,
> would satisfy it. The cheap first move is to open an issue asking for one.
>
> Everything below is engineering-ready and waits only on that.

## 1 · Why libvgm would be primary rather than one more provider

Two facts, both verified 2026-07-28, are what make this different in kind from
the ymfm and vgsound_emu integrations:

**One API covers every chip.** `SndEmu_Start(DEV_ID, const DEV_GEN_CFG*,
DEV_INFO*)` returns a `DEV_DEF` carrying `Start`/`Stop`/`Reset`/`Update`
function pointers plus an array of width-typed register writers. There is no
per-chip C++ to wrap, no per-chip class to instantiate — the same twenty lines
of Rust drive a QSound and a SAA1099. Compare ymfm, where every chip is a
distinct C++ template needing a shim entry, and MAME, where every device is a
node in a framework we would have to reimplement.

**The coverage is total.** `SoundDevs.h` defines **50 device IDs**, including
every chip our corpus contains and, critically, the five we cannot play at all
today: **SCSP, ES5506, YMF271, POKEY and Mikey**. Adopting libvgm would take
full corpus playability from 99.07% to effectively 100%, and would close the
tail that CORES-REUSE-PLAN could only leave open.

And the framework is small: the whole of `emu/` is **five `.c` files**
(`SoundEmu.c`, `logging.c`, `panning.c`, plus `Resampler.c` and
`dac_control.c` we do not need) and eleven headers, over a `cores/`
subdirectory. This is a library designed to be embedded, unlike MAME.

A third advantage is structural: **libvgm ships several emulation cores per
chip**, selected through `DEV_GEN_CFG.emuCore` with the four-character codes in
`EmuCores.h` (`FCC_MAME`, `FCC_NUKE`, `FCC_GPGX`, `FCC_EMU_`, …). That maps
directly onto the per-chip core picker cr-2 already built — one binding can
publish *several* registry entries per chip, and the user chooses. We would get
the picker's whole reason for existing populated almost for free.

## 2 · Where it sits, and what happens to what we have

Under the assumption, libvgm is GPL, so it cannot live in `dro-synth` and
cannot be a dependency of anything permissive.

| Tier | Crate | Source | Job |
|---|---|---|---|
| **Accuracy (primary)** | **`dro-cores-libvgm`** *(new, GPL-2.0-or-later)* | libvgm submodule | The default core for nearly every chip in the GPL app build. |
| Extreme accuracy | `dro-cores-gpl` | Nuked-OPLL, LLE dies | Unchanged. The dies stay the oracle tier. |
| LGPL | `dro-cores-nuked` | Nuked submodules | Unchanged; still the OPL3 path. |
| **Permissive** | `dro-cores-ymfm`, later `dro-cores-pcm` | ymfm, vgsound_emu | **Not wasted.** They are what a permissive consumer of `dro-synth` gets, and what the app falls back to when the GPL crate is excluded. |
| **wasm + fallback** | `dro-synth` | our clean-room cores | Unchanged — unless §6's spike succeeds. |

A **new crate** rather than folding into `dro-cores-gpl`: libvgm is a large C
build with its own failure modes, and keeping it separate means a broken pin
cannot take the LLE dies down with it. Both are GPL-2.0-or-later, so the app
links both.

**ru-1's ymfm work keeps its value.** It is the permissive Yamaha tier, the
second opinion on every Yamaha row, and — since it is already integrated — the
control group for judging libvgm's own numbers.

## 3 · The binding — one wrapper, a per-chip table

The Rust side is a single `LibVgmChip` implementing `ChipCore`, holding a
`DEV_INFO` and the function pointers pulled from its `DEV_DEF`.

**Construction.** `DEV_GEN_CFG { emuCore, srMode, clock, flags, smplRate }`.
We set `srMode` to **native** and let our own resampler do the rate conversion,
exactly as every other core does — libvgm's `Resampler.c` is therefore not
compiled.

**Rendering.** `DEVFUNC_UPDATE(void* info, UINT32 samples, DEV_SMPL** outputs)`
writes **planar** `INT32` channels; `ChipCore::render` wants interleaved. Two
persistent scratch buffers and an interleave loop, allocated once at reset.

**Writing is the only per-chip work, and it is a table, not code.** libvgm
exposes writers typed by address and data width (`DEVRW_A8D8`, `A16D8`,
`A8D16`, …), fetched with `SndEmu_GetDeviceFunc(devDef, RWF_REGISTER | RWF_WRITE,
DEVRW_A8D8, 0, &ptr)`. Per chip we record which writer to fetch and how our
`(port, addr, data)` folds into its arguments.

**The impedance mismatch to watch.** Our stream decoder *normalises* several
chips on the way in — `0xC4` QSound rewritten so `addr` is the register and
`data` the 16-bit value, `0xE1` C352 read big-endian, `0xC1`/`0xC2`/`0xC3`
routed to port 1 so RAM pokes cannot collide with registers. libvgm's cores
expect the **raw VGM conventions** those fixes were written to hide. So the
per-chip table must *invert* our normalisation for exactly those chips. This is
the single most likely source of "it links, it runs, it is silent" bugs, and
each entry needs a unit test asserting the bytes that reach the writer.

**ROM and RAM.** libvgm takes sample ROM through its memory write types
(`DEVRW_MEMSIZE` to declare the image, then block writes), which is the same
shape as our `load_rom(block_type, total_size, start, data)` and `write_ram`.
The `banks::block_owner` table we already built is what routes them.

**Linked devices.** `DEV_DEF::LinkDevice` and `SndEmu_FreeDevLinkData` exist for
parts that attach to others. The OPN family's SSG and the YM2610's ADPCM are
the cases to check at lv-3.

## 4 · Risks, and the one that is not obvious

**Symbol collision with our own Nuked submodules.** libvgm *bundles* Nuked
cores (`FCC_NUKE` in `EmuCores.h`), and we already link Nuked-OPN2, OPM, OPLL
and PSG through `dro-cores-nuked` and `dro-cores-gpl`. Linking both into one
binary risks duplicate definitions of `OPN2_Reset` and friends. libvgm gates its
cores behind per-core defines (`2612intf.c` shows `#ifdef EC_YM2612_GPGX`), so
the fix is to compile libvgm with the duplicated cores disabled and let our
existing submodules serve those chips. **This must be settled at lv-1, not
discovered at link time in lv-4.**

**The OPL invariant still holds.** libvgm has YM3812, YMF262 and Y8950 cores,
and registering them must not make OPL generically buildable — OPL documents
route through `PlayerEngine`. `has_core` versus `can_build` (CORES-PLAN §2) is
unchanged, and Y8950 still needs its routing audit before it can claim an OPL
document.

**Our parity reference shares libvgm's lineage.** VGMPlay and libvgm are both
Valley Bell's, and libvgm is described upstream as the modular rewrite of
VGMPlay's components. So a libvgm core measured against VGMPlay should score
*very* high — near 1.0 where the core lineage is identical. Two consequences:
it is a strong validation signal at lv-2 (a low score means our binding is
wrong, not that the cores differ), and the frozen thresholds in
`parity/mod.rs` would need **re-freezing upward**, turning the scorecard from
"different implementations differ" into a genuine regression detector. That is
a significant gain and it deserves its own step.

**Allocator and logging.** libvgm uses `malloc`/`free` and has a logging
callback (`SetLogCB`). Native is free; the wasm spike in §6 is where this
matters.

**API drift.** libvgm is actively maintained (commits through June 2026), which
is the point — but its public API does evolve. Pinning a commit and bumping
deliberately is the same discipline the Nuked submodules already use.

## 5 · Step list

| Step | Work |
|---|---|
| **lv-0** | **The licence gate.** Obtain the explicit repository-wide grant described at the top. Nothing below starts without it. |
| **lv-1** | Submodule + `dro-cores-libvgm` crate + `build.rs` compiling `SoundEmu.c`, `logging.c`, `panning.c` and **one** core, with the Nuked-collision policy decided and encoded. A test that `SndEmu_Start` returns a working device. **The PoC gate.** |
| **lv-2** | The generic `LibVgmChip: ChipCore` — construction, native rate, planar-to-interleaved render, `Stop` on drop. One chip end to end, with a parity row. Expect a very high score (§4); a low one means the binding is wrong. |
| **lv-3** | The per-chip write table, including the inversions of our own normalisation (QSound, C352, the RF5C68 port-1 convention). A unit test per entry asserting the bytes that reach libvgm. ROM/RAM delivery through `block_owner`. Linked devices. |
| **lv-4** | Roll out across the chips we already play. Each takes the default only if it beats the frozen clean-room row — the CORES-REUSE-PLAN §7 gate, unchanged. |
| **lv-5** | **The five new chips: SCSP, ES5506, YMF271, POKEY, Mikey.** New `ChipKind`s, header decode, corpus playability re-measured — the step that closes the tail to ~100%. |
| **lv-6** | Publish libvgm's alternative cores per chip (`FCC_MAME`, `FCC_NUKE`, …) as picker entries. Cheap, and it populates the picker cr-2 built. |
| **lv-7** | **Re-freeze the parity thresholds upward** now that reference and core share lineage, and rewrite the SCORECARD chronicle to say what the board now measures. |
| **lv-8** | Sweep: About credits, `PROVENANCE.md`, the GPL notice in the About box, docs, and CORES-REUSE-PLAN's §5 resolved. |

## 6 · The wasm question, which libvgm may answer better than ymfm

libvgm is **C**, not C++. The blocker that keeps ymfm off the web —
`wasm32-unknown-unknown` has no C++ standard library — does not apply. The
Nuked cores already prove the route: `-ffreestanding` plus our own
`shim/string.h`, with `compiler_builtins` supplying the `mem*` family.

What libvgm needs beyond that is an **allocator** (it uses `malloc`/`free`
freely) and stubs for its logging path. A small `shim/alloc.c` forwarding to
Rust's allocator would likely suffice.

If that works, the consequence is large: **the GPL web build could carry
accurate cores**, and the clean-room tier would fall back to being purely the
permissive fallback rather than the web tier. Worth one timeboxed spike at
**lv-1a**, immediately after the PoC gate, because the answer changes how much
further investment the clean-room cores deserve.

## 7 · What this does not change

- The licence split, registry, picker, wasm rules and acceptance gates from
  CORES-PLAN §§1, 2, 4, 6.
- The clean-room cores keep their demoted job (CORES-REUSE-PLAN §6): fallback,
  and the wasm tier unless §6 succeeds. They are still not accuracy
  investments.
- The scorecard remains the arbiter. No core takes a default it has not earned
  against the frozen row it replaces.
