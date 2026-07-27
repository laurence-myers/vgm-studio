# PROVENANCE — where every core came from

One row per core and per port: what it was read from, at which upstream
revision, under what license, and what was changed locally. This is the
document libvgm is missing, and the reason a user can be told exactly what is
running when they pick a core in Settings.

**The rule this file enforces:** nothing enters `dro-synth` that is not
permissively licensed. `dro-synth` is `MIT OR Apache-2.0` so it can be reused
without copyleft obligations, which means its cores are either clean-room
(written from documented behaviour) or ported from MIT/BSD/ISC/zlib sources
with the upstream notice retained verbatim in the ported file. Copyleft cores
— the Nuked family, the GPL-2 and LLE tiers — live in **provider crates the
application depends on** (`dro-cores-nuked`, `dro-cores-gpl`), never here. See
`licenses/README.md` for the split.

**The one copyleft dependency is optional.** `nuked-opl3` (LGPL-2.1-or-later)
is behind the default-on `nuked-opl` feature, so:

```bash
cargo build -p dro-synth --no-default-features
```

gives a build with no copyleft in it at all. OPL documents still load, edit,
seek, split and render in that build — all of that is file-format logic, not
emulation — they simply produce silence, via `opl::SilentOpl`. The registry
registers no OPL core there, so the UI reports the silence rather than implying
sound. The app enables the feature, as every release build does.

## Sourcing tiers

How a core gets here, most preferred first — the policy is *avoid vendoring*:

1. **Git submodule + `cc`.** Maintained C upstreams are pinned to a commit
   under `vendor/upstream/<name>` and compiled **unmodified**; glue (allocator
   shims, no-libc stubs) is written on our side. Upgrading is
   `git -C vendor/upstream/<x> pull`, a pin bump and a corpus re-run — never a
   re-port.
2. **crates.io dependency.** Where a maintained Rust crate of the right
   license exists. Same non-vendored property, cargo-native.
3. **Rust port with a provenance header.** Last resort, for upstreams that
   cannot be consumed directly: C++ sources (no C++ toolchain here, and C++ to
   `wasm32-unknown-unknown` needs a runtime we do not ship), and libvgm's
   BSD-tagged C files, which `#include` untagged framework headers — compiling
   those would drag unlicensed code into the build, so the tagged file's logic
   is ported and cited instead. A ported file carries the upstream notice and
   names the revision it matches.

## Cores

| Core | Chips | Tier | Read from | Upstream revision | License | Local deltas |
|---|---|---|---|---|---|---|
| `cores/sn76489.rs` | SN76489 (+ Sega VDP variant) | clean-room | Documented behaviour: the latch/data protocol, ten-bit periods, the 16-bit LFSR tapped at bits 0 and 3, four-bit attenuation at 2 dB a step | n/a — no code read | MIT OR Apache-2.0 | n/a. Every constant is *derived in a test* rather than transcribed: the volume table is recomputed from "2 dB a step, last step off", and pitch is counted in rising edges against `clock / (32 × period)`. Not modelled: the Game Gear stereo register, T6W28 split addressing. |
| `dro-cores-nuked` → `cqm.rs` | YM3812 (OPL2), YMF262 (OPL3) — as the Creative CQM clone | **submodule + `cc`** | `vendor/upstream/nuked-cqm` (`nukeykt/Nuked-CQM`), `cqm.c` + `cqm.h` | `274a4c463ab2f8e193b1c1192f9d4e0d02df521a` | LGPL-2.1-or-later | **Compiled unmodified.** Freestanding C but for `#include <string.h>`, which `shim/string.h` answers. Its own `writebuf` ring matches Nuked-OPL3's, so `PlayerEngine`'s write spacing suits it unchanged. |
| `dro-cores-nuked` → `opn2.rs` | YM2612, YM3438 | **submodule + `cc`** | `vendor/upstream/nuked-opn2` (`nukeykt/Nuked-OPN2`), `ym3438.c` + `ym3438.h` | `335747d78cb0abbc3b55b004e62dad9763140115` | LGPL-2.1-or-later | **Compiled unmodified.** Two upstream properties are handled on our side rather than patched — see below. |
| `dro-cores-nuked` → `opm.rs` | YM2151, YM2164 | **submodule + `cc`** | `vendor/upstream/nuked-opm` (`nukeykt/Nuked-OPM`), `opm.c` + `opm.h` | `23ea53bb442b3f761ded3cd8a27399dd46db34fc` | LGPL-2.1-or-later | **Compiled unmodified.** Same designer, same shape as OPN2: cycle-level clocking (32 cycles to a sample, `clock / 64`) and latched writes. No global chip-type — the YM2164 variant is a flag on this chip's own reset — so no lock. Its write pacing is stricter; see below. |

### The two Nuked-OPN2 properties worth knowing

Both are upstream's design, not defects, and both would be silent bugs if
missed:

1. **A register write needs a whole rotation.** `OPN2_Write` only raises a
   pending flag; the register lands when the chip's 24-cycle rotation reaches
   *that register's slot* (`if (op_offset[slot] == (chip->address & 0x107))`),
   and the pending data is discarded the moment the next address arrives. So
   the wrapper queues writes and hands over **one register per output sample** —
   which is also the real chip's rate, since a YM2612 raises its busy flag for
   about a rotation after each write. Draining faster looks like it works (every
   write is accepted) and silently loses most of them: the symptom is a note
   that never starts. Nuked-CQM has a write buffer of its own; this one does
   not, so the buffer is ours.
2. **`OPN2_SetChipType` writes a file-scope `static`, not a chip field.** Two
   instances of different variants therefore share one setting, and only
   `OPN2_Clock` reads it (for the YM2612's discrete DAC ladder, which the CMOS
   YM3438 lacks) plus `OPN2_Read`, which nothing here calls. The wrapper holds a
   process-wide lock for the duration of each render call and sets the type
   inside it, so a YM2612 and a YM3438 in one file — or on two threads — each
   render as themselves. One acquisition per render, not per clock.

### Write pacing, and why it is measured per core

Every Nuke.YKT core here latches a register write and applies it only when the
chip's rotation reaches that register's slot. How much room that needs is *not*
the same across chips, and getting it wrong is silent — every write is accepted,
nothing errors, and some fraction of them never land:

| Core | Rotation | Pacing used |
|---|---|---|
| Nuked-OPN2 (YM2612) | 24 cycles | address, value, then the rest of the rotation — one register per output sample |
| Nuked-OPM (YM2151) | 32 cycles | address, **a whole rotation**, value, **a whole rotation** — one register per two samples |

The OPM figure was measured, and the measurement is the reason it is not a
guess: spacing writes 1, 2, 3 or 6 cycles apart produces total silence, while 4
gives full amplitude, 8 a quarter and 16 a half. That sequence is not monotonic
because those numbers are **phases**, not durations — each lands some registers
on their slot and misses others. So a spacing that happens to work for one patch
is no evidence at all, and only a full rotation each way is defensible. Any new
core in this family gets the same treatment: find the rotation, give each half
of the handover its own, and pin it with a burst-of-writes test.

**Not a struct mirror.** Every core's state is allocated by a size the C reports
(`shim/layout.c`, ours) and never declared in Rust; see `src/opaque.rs`. A
`#[repr(C)]` twin is fine until an upstream adds a field, at which point the
twin is too small and the C writes past it — which on a submodule that exists to
be pulled is a question of when, not whether.
| `vendor/nuked-opl3` (optional dependency, `nuked-opl` feature) | YM3812 (OPL2), YMF262 (OPL3) | vendored Rust port (legacy) | The `nuked-opl3` crate 0.1.0, itself a Rust port of Nuke.YKT's Nuked-OPL3 | crates.io 0.1.0 | LGPL-2.1-or-later | Two defect fixes and one pan-law change in `src/core.rs`, all documented in `vendor/nuked-opl3/README.dro-trimmer.md`; both fixes are upstream-PR material. Shipped, byte-tested against the C reference (`c-parity`), wasm-clean. **The one legacy vendored core** — upstream is quiet, so the no-vendoring policy above does not apply retroactively. |

## Where a core is declared

Every core has a `CoreInfo` row in the registry (`dro-synth/src/registry.rs`,
or a provider crate's `register`), and that row is what a user sees: the
Settings picker lists its label and license, and the About box credits it. The
row must agree with this file on label, license and upstream — the two
disagreeing means one of them is lying to somebody.

Registered outside `dro-synth`, and why:

| Provider | Registers | Why not here |
|---|---|---|
| `dro-cores-nuked` | `opl3.cqm` — Nuked-CQM | LGPL-2.1-or-later. `dro-synth` is permissive; the app links this. |
| `dro-retrowave` | `opl3.retrowave` — the RetroWave OPL3 board | Native-only (serial ports). The web build never registers it, so its Settings dialog does not offer hardware it could never reach. |

## Upgrading a submodule core

The whole reason these are submodules rather than ports:

```bash
git -C vendor/upstream/nuked-cqm pull
```

then commit the new pin, re-run the crate's tests, and A/B against VGMPlay. No
re-port, no merge against local edits — there are none. A fresh clone needs
`git submodule update --init --recursive` first; the build fails with that
instruction rather than a missing-file error if it is skipped.

**What was proven at cr-3**, and what the rest of the nukeykt family inherits:
the upstream C compiles to `wasm32-unknown-unknown` directly through clang, so
these cores reach the web build too. Verified by the object's own contents —
`llvm-nm` on the wasm build's `cqm.o` lists `CQM_Reset`, `CQM_GenerateStream`
and the rest, and `file` calls it a WebAssembly binary. The only thing standing
in the way was the libc include, which `shim/string.h` answers.

**And the buffered-write question the plan raised**: `PlayerEngine` spaces
queued register writes a couple of samples apart because Nuked-OPL3 resolves
key-on/off edges at sample-generation time. Upstream CQM has its own `writebuf`
ring with the same two-sample delay, so the spacing suits it unchanged — and
`buffered_writes_retrigger_where_immediate_ones_collapse` in `cqm.rs` asserts
that in *behaviour* rather than by reading the header, so an upstream change
that broke it would be caught.

## Non-core third-party code

| What | Where | License | Note |
|---|---|---|---|
| `serialport` | `dro-retrowave` | MPL-2.0 | Native only; notice carried in the About dialog. |
| Px437 IBM VGA font trace | `dro-ui/assets/fonts` | see `LICENSE-Px437-IBM-VGA.txt` beside it | A faithful trace of the IBM VGA ROM font. |
| `opl3-rs` (Nuked-OPL3 C bindings) | `dro-synth`, `c-parity` feature | LGPL-2.1-or-later | **Never shipped.** Off by default, needs a C compiler and libclang, and cannot target wasm. A parity oracle only. |

## Oracles — read, compared against, never linked

Programs run separately to check a core, whose code does **not** enter any
binary here. Their licenses are therefore irrelevant to distribution, which is
the whole point of keeping them at arm's length.

| Oracle | Used for | Why never linked |
|---|---|---|
| VGMPlay | The A/B listening test that is the real acceptance bar for any core | Reference playback; a person does this |
| Mesen2, BlastEm | NES, Mega Drive behaviour | GPL-3-only — incompatible with the GPL-2-or-later cores in the same binary |
| Genesis Plus GX | Mega Drive behaviour | Non-commercial clause: a further restriction the GPL does not permit |
| openMSX | Y8950 / YMF278B behaviour | Reference only |
| Mednafen | VSU behaviour | Reference only |
| MAME's QSound DSP16 LLE | — | Needs the DL-1425 ROM, which is not redistributable. Out of scope entirely. |

## Adding a row

When a core lands, its row goes in **the same commit**. A row needs:

- the tier it came in under, and the file or submodule it lives in;
- the exact upstream revision (a commit SHA for a submodule, a version for a
  crate, "n/a — no code read" for clean-room);
- the SPDX license expression, matching the crate it lives in;
- local deltas, or `n/a`. "Compiled unmodified" is a delta worth stating.

The core's `CoreInfo` entry in the registry must agree with this row on label,
license and upstream — that entry is what the About dialog shows a user, and
the two disagreeing means one of them is lying.
