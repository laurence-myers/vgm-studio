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

> **Open caveat until cr-2.** `dro-synth` still depends on `nuked-opl3`
> (LGPL-2.1-or-later) unconditionally, so a build of it today carries that
> obligation regardless of this crate's own license expression. cr-2 makes it
> an optional, default-on `nuked-opl` feature so `--no-default-features`
> yields a genuinely permissive build. Until then this crate's permissive
> claim covers its own source only, and the row below is listed for honesty
> rather than because it belongs here.

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
| `vendor/nuked-opl3` (dependency) | YM3812 (OPL2), YMF262 (OPL3) | vendored Rust port (legacy) | The `nuked-opl3` crate 0.1.0, itself a Rust port of Nuke.YKT's Nuked-OPL3 | crates.io 0.1.0 | LGPL-2.1-or-later | Two defect fixes and one pan-law change in `src/core.rs`, all documented in `vendor/nuked-opl3/README.dro-trimmer.md`; both fixes are upstream-PR material. Shipped, byte-tested against the C reference (`c-parity`), wasm-clean. **The one legacy vendored core** — upstream is quiet, so the no-vendoring policy above does not apply retroactively. |

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
