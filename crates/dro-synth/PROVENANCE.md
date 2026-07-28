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
| `cores/nes_apu.rs` | NES APU | clean-room | The NESdev documentation: channel layout, the frame sequencer's step boundaries, the length/noise/DMC tables, and the two non-linear mixer formulas | n/a — no code read | MIT OR Apache-2.0 | n/a. Both mixer tables are *regenerated from the formulas in a test*, because they must be integer tables — [`ChipCore`] forbids output that could differ across targets, and floating point in the hot loop is how that promise breaks. One deliberate departure from hardware, documented at `reset`: the channels start **enabled**. Not modelled: the FDS add-on. |
| `cores/ay8910.rs` | AY-3-8910, YM2149 — **and the SSG section of every OPN chip** | clean-room | The datasheets: three 12-bit tone counters, a 17-bit noise register tapped at bits 0 and 3, the shared envelope's four shape bits, and the 1.5 dB DAC curve | n/a — no code read | MIT OR Apache-2.0 | n/a. The 32-level curve is regenerated from "1.5 dB a step" in a test. Written to be *reused* by the OPN cores rather than to serve one chip — `write_register`/`tick`/`output` are `pub` for that -- the OPN cores are in another crate. Not modelled: the I/O ports, the VGM `0x31` stereo mask. |
| `cores/huc6280.rs` | HuC6280 (PC Engine) | clean-room | The documented register interface: six 32-entry wavetables, banked registers behind a channel-select, per-channel stereo attenuation, DDA, and noise on channels 5-6 | n/a — no code read | MIT OR Apache-2.0 | n/a. Attenuation curve regenerated from 1.5 dB a step, as above. Not modelled: the LFO on channels 1-2, the timer/IRQ registers. |
| `cores/okim.rs` | OKIM6295, OKIM6258 | clean-room | The documented Dialogic/OKI ADPCM algorithm (the 49-entry step table, the index deltas, the twelve-bit accumulator), the OKIM6295's ROM table of contents and its two-write command protocol | n/a — no code read | MIT OR Apache-2.0 | n/a. Both chips share one decoder, because they share a codec; only the source of the nibbles differs. Step and volume tables regenerated in tests. Both banking schemes modelled (the `$0F` latch and the NMK112's per-quarter banks), and the OKIM6258's header-flag divider honoured. Not modelled: the OKIM6258's 3-bit mode, the OKIM6295's mid-stream clock retune. |
| `cores/gb_dmg.rs` | Game Boy DMG | clean-room | The Pan Docs: the four channels, the 512 Hz frame sequencer, per-channel stereo routing, and the wave channel's shift-based volume | n/a — no code read | MIT OR Apache-2.0 | n/a. **The plan allowed for SameBoy's APU as a submodule instead**; that C is written against SameBoy's whole `GB_gameboy_t`, so carving it out would have meant editing the upstream — which the sourcing policy forbids, and which would have to be redone on every pull. Not modelled: wave-RAM access quirks while running, the CGB registers. |

| `dro-cores-nuked` → `cqm.rs` | YM3812 (OPL2), YMF262 (OPL3) — as the Creative CQM clone | **submodule + `cc`** | `vendor/upstream/nuked-cqm` (`nukeykt/Nuked-CQM`), `cqm.c` + `cqm.h` | `274a4c463ab2f8e193b1c1192f9d4e0d02df521a` | LGPL-2.1-or-later | **Compiled unmodified.** Freestanding C but for `#include <string.h>`, which `shim/string.h` answers. Its own `writebuf` ring matches Nuked-OPL3's, so `PlayerEngine`'s write spacing suits it unchanged. |
| `dro-cores-nuked` → `opn2.rs` | YM2612, YM3438 | **submodule + `cc`** | `vendor/upstream/nuked-opn2` (`nukeykt/Nuked-OPN2`), `ym3438.c` + `ym3438.h` | `335747d78cb0abbc3b55b004e62dad9763140115` | LGPL-2.1-or-later | **Compiled unmodified.** Two upstream properties are handled on our side rather than patched — see below. |
| `dro-cores-nuked` → `opm.rs` | YM2151, YM2164 | **submodule + `cc`** | `vendor/upstream/nuked-opm` (`nukeykt/Nuked-OPM`), `opm.c` + `opm.h` | `23ea53bb442b3f761ded3cd8a27399dd46db34fc` | LGPL-2.1-or-later | **Compiled unmodified.** Same designer, same shape as OPN2: cycle-level clocking (32 cycles to a sample, `clock / 64`) and latched writes. No global chip-type — the YM2164 variant is a flag on this chip's own reset — so no lock. Its write pacing is stricter; see below. |
| `dro-cores-nuked` → `opn.rs` | YM2203, YM2608, YM2610 | **assembled**, not a new core | Nuked-OPN2 for the FM (the family shares one engine; the YM2612's ladder DAC is the odd one out, so CMOS mode is selected always) + this project's `Ay8910` for the SSG | as Nuked-OPN2 above | LGPL-2.1-or-later | **The ADPCM is not modelled** — see below. Also simplified: the YM2203's programmable prescaler is assumed at its default, and the SSG clock is taken as clock/4 for all three. |
| `dro-cores-gpl` → `opll.rs` | YM2413 (OPLL), Konami VRC VII | **submodule + `cc`** | `vendor/upstream/nuked-opll` (`nukeykt/Nuked-OPLL`), `opll.c` + `opll.h` | `1269cf5a783b65583b50fa2464d08be75830aaa0` | **GPL-2.0-or-later** | **Compiled unmodified.** The first GPL core, and what stands up `dro-cores-gpl`. Two DACs (melody and rhythm) multiplexed across an 18-cycle rotation, so a sample is that whole rotation of both summed. `chip_type` is a *field* here, not the global Nuked-OPN2 keeps, so no lock. Its summed output carries a standing DC offset, removed by the same integer blocker the NES core uses. |
| `dro-cores-gpl` → `psg.rs` | SN76489 (Sega VDP flavour) | **submodule + `cc`** | `vendor/upstream/nuked-psg` (`nukeykt/Nuked-PSG`), `ympsg.c` + `ympsg.h` | `d15a168c676f4669e23660be9225b34ad7c1764e` | **GPL-2.0-or-later** | **Compiled unmodified.** A picker *alternative* -- the clean-room SN76489 stays the default, because the die trace is of the one part inside the Sega VDPs and ignores the header's feedback/width fields. One sample per 16 internal clocks, matching the clean-room rate. Upstream sums its DAC in `float`; the build passes `-ffp-contract=off` so the arithmetic is plain IEEE and identical across targets, and the single float-to-int crossing happens in the wrapper. Output scaled to the clean-room core's calibrated level (one channel ~4096 vs 4000) so the picker changes texture, not volume. |
| `dro-cores-gpl` → `lle_opm.rs` | YM2151, YM2164 -- **the LLE tier's first core** | **submodule + `cc`** | `vendor/upstream/ym2151-lle` (`nukeykt/YM2151-LLE`), `fmopm.c` + `fmopm.h` | `efa722f342119b69457e0e02a007449c5baac698` | **GPL-2.0-or-later** | **Compiled unmodified.** A die simulation from the decap, not a behavioural model: the API is the package's pins, so the wrapper drives the bus electrically (CS/WR asserted across master clocks, IC held low to reset) and decodes the YM3012 serial DAC stream itself -- 13 bits at half the master clock, offset-binary mantissa then exponent, framed backwards from each S/H falling edge. The framing was pinned with a pin probe (the idle word decodes to exactly zero) and is regression-tested. `realtime: false`: render/oracle only, never the default playback core. Pin access happens in `shim/lle_opm.c` against the upstream's own header, keeping the no-struct-mirroring rule. |
| `dro-cores-gpl` → `lle_opn2.rs` | YM2612 (die), YM3438 rendered the same -- a stated approximation | **submodule + `cc`** | `vendor/upstream/ym2608-lle` (`nukeykt/YM2608-LLE`), `fmopna_2612.c` compiling `fmopna_impl.c` under its chip macro | `7a2aca7b6830b96e48e3a4e1a40d15525993fa60` | **GPL-2.0-or-later** | **Compiled unmodified.** The OPNA decap builds three chips behind per-chip macros; only the 2612 die is wrapped so far. No serial DAC on this one: the nine-bit ladder time-multiplexes the channels on two parallel pins, asymmetry included, and the wrapper sums a sample period of them (the pins change every two master clocks -- measured via the oracle's level column, not derived). Oracle result on arrival: **Nuked-OPN2 vs the die, 0.9848 median (n=4)** -- the shipping core is die-accurate, so the open 0.904-vs-VGMPlay row is the reference driver's, not ours. The same submodule carries `fmopna_rom.h`, the YM2608's decapped internal rhythm ROM -- the "unshippable" 2608 rhythm gap has a shippable GPL-tier route when the 2608 die (or a ROM hand-off to the clean-room core) is wrapped. |

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


### A VGM is a log, not a machine

The NES core starts with every channel enabled, which hardware does not: at
power-on `$4015` is zero and a channel stays mute until something enables it.
But a VGM is a *register log*, and a ripper may start it after the driver's
initialisation has already run. Rips that never write `$4015` at all are not
rare — `Lemmings (NES)` is one — and from hardware's power-on state they play
in complete silence.

The corpus made the difference visible: 10 of 12 sampled NES files audible
before, 12 of 12 after. Worth remembering for the chips still to come — a core
that is right about the hardware can still be wrong about the format, and only
real files show which.

### The OPN family's ADPCM gap

The YM2608 and YM2610 each carry an ADPCM-A rhythm section and an ADPCM-B
sample channel. Neither is here, so a Neo Geo rip plays its FM and SSG with the
drums missing. That is a real gap, stated rather than hidden, and it shows in
the corpus: YM2203 and YM2608 come back 12/12 audible while the YM2610 manages
9/12, the quiet three being short ADPCM-led cues (`01 IPL`, `04 Stage Start`).

`Playability::Partial` says "this chip has no core" for a whole chip; there is
no vocabulary yet for "most of one", and inventing it is the honest follow-up
if the drums matter more than the next chip does.

**Why assembled rather than ported.** The plan's fallback was a port of MAME's
fmopn, because `nukeykt/Nuked-OPNB` was expected to mature. It has not: version
0.0, a header that declares `fm_ar` and `fm_ks` twice so it does not compile, no
reset function, no output function at all, and no SSG or ADPCM — 649 lines
against Nuked-OPM's 2,200. Meanwhile the FM half needs no port, because the
YM2612 *is* an OPN and its core is already shipped and byte-tested. A port
would earn its keep on the ADPCM, which is exactly where the gap is.

### Write pacing, and why it is measured per core

Every Nuke.YKT core here latches a register write and applies it only when the
chip's rotation reaches that register's slot. How much room that needs is *not*
the same across chips, and getting it wrong is silent — every write is accepted,
nothing errors, and some fraction of them never land:

| Core | Rotation | Pacing used |
|---|---|---|
| Nuked-OPN2 (YM2612, and the OPN family's FM) | 24 cycles | address, value, then the rest of the rotation — one register per output sample |
| Nuked-OPM (YM2151) | 32 cycles | address, **a whole rotation**, value, **a whole rotation** — one register per two samples |

All three cores drive one `WriteQueue` (`src/write_queue.rs`) parameterised by
those two figures, so the shape is shared and only the numbers differ. The
numbers are the part that must be measured.

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
| `dro-cores-nuked` | `opl3.cqm`, `ym2612.nuked`, `ym2151.nuked`, `ym2203/2608/2610.nuked` | LGPL-2.1-or-later. `dro-synth` is permissive; the app links this. |
| `dro-cores-gpl` | `ym2413.nuked` — Nuked-OPLL; `sn76489.nuked-psg` — Nuked-PSG; `ym2151.lle` — YM2151-LLE, `ym2612.lle` — YM2612-LLE (render/oracle only) | **GPL-2.0-or-later.** Neither `dro-synth` nor `dro-cores-nuked` may carry it without becoming something else. |
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

## Output levels, measured (2026-07-28)

pt-6's promise was that the `OUTPUT_GAIN` constants stop being guesses. The
measurement is the reference-parity scorecard's per-chip **level** column —
our RMS over VGMPlay 0.52's, twelve single-chip corpus files per chip, both
sides rendered at the chip's own native rate so no resampler touches either —
plus, for the YM2612, an independent in-mix least-squares fit over seven Mega
Drive rips (coefficient on our FM-solo render: 3.2–4.3, median ≈4.0,
residuals 0.24–0.70).

Applied:

| Core | measured level | correction | now |
|---|---|---|---|
| YM2612 (`dro-cores-nuked/opn2.rs`) | 0.227 (n=12); in-mix fit ≈4.0× | ×4.2 | `OUTPUT_GAIN = 21` |
| YM2151 (`dro-cores-nuked/opm.rs`) | 0.500 (n=12) | ×2 | `OUTPUT_GAIN = 2` |
| YM2203 (`dro-cores-nuked/opn.rs`) | 0.497 (n=12) | ×2, whole mix | `output_scale() = 2`; 0.994 verified (n=12) |
| YM2610 (`dro-cores-nuked/opn.rs`) | 0.318 post-ADPCM (n=12) | ×3, whole mix | `output_scale() = 3`; 0.955 verified (n=12) |

The OPN family's corrections are **whole-mix** — one integer on the summed
frame per chip kind, FM, SSG and ADPCM together — because the per-section
balance inside the mix is not separately measurable from single-chip corpus
files, and scaling the sections in step at least cannot disturb it. The
YM2608 stays at ×1 deliberately: its measured 0.641 is depressed by the
unshippable rhythm mask ROM, so a scale fitted to it would overshoot every
file that leans on FM. Correlations were unchanged by the pass (a whole-mix
scalar cannot move a normalised correlation), which the verification run
confirmed.

The in-mix fit's PSG coefficients came back negative and were discarded, for a
reason worth keeping: the fit renders at the YM2612's native rate, where the
reference's PSG passes through its own linear resampler and aliases, and a
decorrelated component's least-squares coefficient collapses. The PSG's answer
comes from its own single-chip row instead — SN76489 level 0.984, already
matched within 2%.

Measured and **deliberately not yet applied** (each carries a reason):

| Chip | level (n=12) | why deferred |
|---|---|---|
| YM2413 | 0.370 | shared-core correlation still open at 0.977; scale after |
| YM2608 | 0.641 | rhythm needs the chip's internal mask ROM (unshippable), which depresses the measurement itself; Delta-T modelled. Held at ×1 in the family balance pass for exactly that reason |
| AY8910 | 0.720 | type byte now read (GI parts get the coarse envelope); flags are emulator mixing options, deliberately unread; measure again |
| Game Boy DMG | 0.550 | correlation open at 0.295; scale after |
| NES APU | 0.756 | correlation open at 0.334; scale after |
| OKIM6295 | 0.497 | remeasure now the divider is right |
| HuC6280 | 0.52–0.67 | the two reference cores differ by 23% *from each other* |
| SN76489, OKIM6258 | 0.98 | matched; nothing to do |

A level correction on a chip whose content or correlation is still wrong would
bake the fault into the constant; these wait their turn.
