# PROVENANCE — where every core came from

One row per core and per port: what it was read from, at which upstream
revision, under what license, and what was changed locally. This is the
document libvgm is missing, and the reason a user can be told exactly what is
running when they pick a core in Settings.

**The rule this file enforces:** nothing enters `vgms-synth` that is not
permissively licensed. `vgms-synth` is `MIT OR Apache-2.0` so it can be reused
without copyleft obligations, which means its cores are either clean-room
(written from documented behaviour) or ported from MIT/BSD/ISC/zlib sources
with the upstream notice retained verbatim in the ported file. Copyleft cores
— the Nuked family, the GPL-2 and LLE tiers — live in **provider crates the
application depends on** (`vgms-cores-nuked`, `vgms-cores-gpl`), never here. See
`licenses/README.md` for the split.

**The one copyleft dependency is optional.** `nuked-opl3` (LGPL-2.1-or-later)
is behind the default-on `nuked-opl` feature, so:

```bash
cargo build -p vgms-synth --no-default-features
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
   cannot be consumed directly. A ported file carries the upstream notice and
   names the revision it matches.

   *Amended 2026-07-28.* This tier used to say C++ could not be consumed here;
   `clang++` is now in the toolchain and `vgms-cores-ymfm` compiles C++ directly,
   so that only holds for `wasm32-unknown-unknown`, which has no C++ standard
   library. It also used to route libvgm here, on the grounds that compiling its
   BSD-tagged files drags in untagged framework headers. **That finding stands
   and is unresolved** — libvgm publishes no licence grant at all (see
   `crates/vgms-cores-libvgm/src/lib.rs`) — but the project owner has directed
   that libvgm be integrated as a submodule under tier 1 regardless, so the
   engineering no longer waits on it. What remains open is *release*: shipping a
   binary containing that object code is the redistribution the missing grant
   does not cover. `docs/vgm-multichip-2026-07/LIBVGM-PLAN.md` lv-0 tracks it.

## Cores

**The clean-room tier was culled on 2026-07-29**, by the owner's decision:
every core this crate wrote itself -- twenty-seven modules, from the SN76489
that opened the programme to the uPD7759 that closed its tail -- was deleted,
with no survivors, in favour of the reused emulators below. The per-core rows
that stood here (what each was read from, its stated gaps, the corpus lessons)
are preserved in this file's git history at `b070f41^`, and the measurements
that justified the cull in `docs/vgm-multichip-2026-07/parity/SCORECARD.md`.
The unregistered Y8950 assembly went with them, as did the OPN assembly
(`vgms-cores-nuked`'s `opn.rs`) whose SSG half was the clean-room AY8910 --
libvgm's OPN family, complete with ADPCM, is what replaced it.

| Core | Chips | Tier | Read from | Upstream revision | License | Local deltas |
|---|---|---|---|---|---|---|
| `vgms-cores-nuked` → `cqm.rs` | YM3812 (OPL2), YMF262 (OPL3) — as the Creative CQM clone | **submodule + `cc`** | `vendor/upstream/nuked-cqm` (`nukeykt/Nuked-CQM`), `cqm.c` + `cqm.h` | `274a4c463ab2f8e193b1c1192f9d4e0d02df521a` | LGPL-2.1-or-later | **Compiled unmodified.** Freestanding C but for `#include <string.h>`, which `shim/string.h` answers. Its own `writebuf` ring matches Nuked-OPL3's, so `PlayerEngine`'s write spacing suits it unchanged. |
| `vgms-cores-nuked` → `opn2.rs` | YM2612, YM3438 | **submodule + `cc`** | `vendor/upstream/nuked-opn2` (`nukeykt/Nuked-OPN2`), `ym3438.c` + `ym3438.h` | `335747d78cb0abbc3b55b004e62dad9763140115` | LGPL-2.1-or-later | **Compiled unmodified.** Two upstream properties are handled on our side rather than patched — see below. |
| `vgms-cores-nuked` → `opm.rs` | YM2151, YM2164 | **submodule + `cc`** | `vendor/upstream/nuked-opm` (`nukeykt/Nuked-OPM`), `opm.c` + `opm.h` | `23ea53bb442b3f761ded3cd8a27399dd46db34fc` | LGPL-2.1-or-later | **Compiled unmodified.** Same designer, same shape as OPN2: cycle-level clocking (32 cycles to a sample, `clock / 64`) and latched writes. No global chip-type — the YM2164 variant is a flag on this chip's own reset — so no lock. Its write pacing is stricter; see below. |
| `vgms-cores-gpl` → `opll.rs` | YM2413 (OPLL), Konami VRC VII | **submodule + `cc`** | `vendor/upstream/nuked-opll` (`nukeykt/Nuked-OPLL`), `opll.c` + `opll.h` | `1269cf5a783b65583b50fa2464d08be75830aaa0` | **GPL-2.0-or-later** | **Compiled unmodified.** The first GPL core, and what stands up `vgms-cores-gpl`. Two DACs (melody and rhythm) multiplexed across an 18-cycle rotation, so a sample is that whole rotation of both summed. `chip_type` is a *field* here, not the global Nuked-OPN2 keeps, so no lock. Its summed output carries a standing DC offset, removed by the same integer blocker the NES core uses. |
| `vgms-cores-gpl` → `psg.rs` | SN76489 (Sega VDP flavour) | **submodule + `cc`** | `vendor/upstream/nuked-psg` (`nukeykt/Nuked-PSG`), `ympsg.c` + `ympsg.h` | `d15a168c676f4669e23660be9225b34ad7c1764e` | **GPL-2.0-or-later** | **Compiled unmodified.** A picker *alternative* -- libvgm's SN76489 is the default (the 2026-07-29 redirect), and the die trace is of the one part inside the Sega VDPs, ignoring the header's feedback/width fields. One sample per 16 internal clocks, matching the clean-room rate. Upstream sums its DAC in `float`; the build passes `-ffp-contract=off` so the arithmetic is plain IEEE and identical across targets, and the single float-to-int crossing happens in the wrapper. Output scaled to the clean-room core's calibrated level (one channel ~4096 vs 4000) so the picker changes texture, not volume. |
| `vgms-cores-gpl` → `lle_opm.rs` | YM2151, YM2164 -- **the LLE tier's first core** | **submodule + `cc`** | `vendor/upstream/ym2151-lle` (`nukeykt/YM2151-LLE`), `fmopm.c` + `fmopm.h` | `efa722f342119b69457e0e02a007449c5baac698` | **GPL-2.0-or-later** | **Compiled unmodified.** A die simulation from the decap, not a behavioural model: the API is the package's pins, so the wrapper drives the bus electrically (CS/WR asserted across master clocks, IC held low to reset) and decodes the YM3012 serial DAC stream itself -- 13 bits at half the master clock, offset-binary mantissa then exponent, framed backwards from each S/H falling edge. The framing was pinned with a pin probe (the idle word decodes to exactly zero) and is regression-tested. `realtime: false`: render/oracle only, never the default playback core. Pin access happens in `shim/lle_opm.c` against the upstream's own header, keeping the no-struct-mirroring rule. |
| `vgms-cores-gpl` → `lle_opn2.rs` | YM2612 (die), YM3438 rendered the same -- a stated approximation | **submodule + `cc`** | `vendor/upstream/ym2608-lle` (`nukeykt/YM2608-LLE`), `fmopna_2612.c` compiling `fmopna_impl.c` under its chip macro | `7a2aca7b6830b96e48e3a4e1a40d15525993fa60` | **GPL-2.0-or-later** | **Compiled unmodified.** The OPNA decap builds three chips behind per-chip macros; only the 2612 die is wrapped so far. No serial DAC on this one: the nine-bit ladder time-multiplexes the channels on two parallel pins, asymmetry included, and the wrapper sums a sample period of them (the pins change every two master clocks -- measured via the oracle's level column, not derived). Oracle result on arrival: **Nuked-OPN2 vs the die, 0.9848 median (n=4)** -- the shipping core is die-accurate, so the open 0.904-vs-VGMPlay row is the reference driver's, not ours. The same submodule carries `fmopna_rom.h`, the YM2608's decapped internal rhythm ROM -- the "unshippable" 2608 rhythm gap has a shippable GPL-tier route when the 2608 die (or a ROM hand-off to the clean-room core) is wrapped. |
| `vgms-cores-gpl` → `lle_opna.rs` | YM2608 (die) | **submodule + `cc`** | as above: `fmopna_2608.c` compiling `fmopna_impl.c` under its chip macro, **`fmopna_rom.h` included -- the decapped internal rhythm mask ROM** | `7a2aca7b6830b96e48e3a4e1a40d15525993fa60` | **GPL-2.0-or-later** | **Compiled unmodified.** The first core in this project that plays a YM2608's drums: the rhythm test keys a bass drum with no sample block loaded and it sounds, from the ROM read off the die. The wrapper serves the Delta-T DRAM (RAS/CAS multiplexed nine-bit bus, reads and the die's own writes) and taps the serial DAC -- this package clocks its serial line at the *master* rate with no trailing bit, unlike the OPM's half-rate-trailing-one, pinned by the same idle-decodes-to-zero probe. **Not yet on the oracle bench**: the serial frame's structure when Delta-T is active (dac_damode regates SH1 and multiplexes FM/ADPCM words) is unpinned, and a trial row measured the harness rather than the emulation. The 2610 configuration of this upstream does not compile at any commit (checked 2026-07-28) and waits for upstream. |
| `vgms-cores-libvgm` | **the default core for every non-OPL chip** -- 32 chips over 38 devices, the OPN family's linked SSGs and the OPL4's linked FM included, plus per-device alternative cores (MAME, EMU2413/2149, Gens, SameBoy, NSFPlay, superctr, Valley Bell's SAA1099) as picker rows | **submodule + `cc`** | `vendor/upstream/libvgm` (`ValleyBell/libvgm`): `emu/SoundEmu.c` + `logging.c` + `panning.c` and the per-device core sources `build.rs` names -- never its player, resampler or DAC-stream code, which this project implements itself | `867223e7c33d63de115d1ab955f784c44f19040a` | **no grant published** -- see `crates/vgms-cores-libvgm/src/lib.rs`'s licence note; the crate's own wrapper is GPL-2.0-or-later, and the owner directed integration to proceed (2026-07-29) with release-time risk acknowledged | **Compiled unmodified.** Ours around it: the per-chip write table (`WriteRule`, each rule transcribed from a `Cmd_*` handler in upstream's player and pinned by a byte-level test), the linked-device start/mix (upstream's `SetupLinkedDevices` + link callback + `GetChipVolume` link column), the per-chip config and clock transcriptions from `vgmplayer.cpp`'s device switch, and the layout guards (`shim/layout.c`). **wasm-capable** (spiked 2026-07-29): `-ffreestanding` + `shim/wasm-libc/` headers + `src/wasm_libc.rs` symbols (allocator over Rust's, `libm` math, fixed-seed `rand`) + printf stubs; the smoke module links import-free and both smoke chips sound under node. |

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

A lesson the culled clean-room NES core taught, kept because it generalises:
at power-on `$4015` is zero and a channel stays mute until something enables
it, but a VGM is a *register log*, and a ripper may start it after the
driver's initialisation has already run. Rips that never write `$4015` at all
are not rare — `Lemmings (NES)` is one — and from hardware's power-on state
they play in complete silence. The corpus made the difference visible: 10 of
12 sampled NES files audible with the hardware behaviour, 12 of 12 with the
log's. A core that is right about the hardware can still be wrong about the
format, and only real files show which. (libvgm's cores play the format;
NSFPlay's default option bits are applied to match the reference player.)

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
| `vendor/nuked-opl3` (optional dependency, `nuked-opl` feature) | YM3812 (OPL2), YMF262 (OPL3) | vendored Rust port (legacy) | The `nuked-opl3` crate 0.1.0, itself a Rust port of Nuke.YKT's Nuked-OPL3 | crates.io 0.1.0 | LGPL-2.1-or-later | Two defect fixes and one pan-law change in `src/core.rs`, all documented in `vendor/nuked-opl3/README.vgm-studio.md`; both fixes are upstream-PR material. Shipped, byte-tested against the C reference (`c-parity`), wasm-clean. **The one legacy vendored core** — upstream is quiet, so the no-vendoring policy above does not apply retroactively. |

## Where a core is declared

Every core has a `CoreInfo` row in the registry (`vgms-synth/src/registry.rs`,
or a provider crate's `register`), and that row is what a user sees: the
Settings picker lists its label and license, and the About box credits it. The
row must agree with this file on label, license and upstream — the two
disagreeing means one of them is lying to somebody.

Registered outside `vgms-synth`, and why:

| Provider | Registers | Why not here |
|---|---|---|
| `vgms-cores-libvgm` | `<slug>.libvgm` for every chip it serves -- **registered first, so these are the defaults**, except the YM2612/YM2151/YM2413, which the app promotes back to Nuked (the owner's 2026-07-29 exceptions) -- plus `<slug>.libvgm-<core>` alternative rows | GPL-tier wrapper around an upstream with no published grant; `vgms-synth` is permissive. |
| `vgms-cores-nuked` | `opl3.cqm`, `ym2612.nuked`, `ym2151.nuked` | LGPL-2.1-or-later. `vgms-synth` is permissive; the app links this. |
| `vgms-cores-gpl` | `ym2413.nuked` — Nuked-OPLL; `sn76489.nuked-psg` — Nuked-PSG; `ym2151.lle` — YM2151-LLE, `ym2612.lle` — YM2612-LLE, `ym2608.lle` — YM2608-LLE (render/oracle only) | **GPL-2.0-or-later.** Neither `vgms-synth` nor `vgms-cores-nuked` may carry it without becoming something else. |
| `vgms-retrowave` | `opl3.retrowave` — the RetroWave OPL3 board | Native-only (serial ports). The web build never registers it, so its Settings dialog does not offer hardware it could never reach. |

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
| `serialport` | `vgms-retrowave` | MPL-2.0 | Native only; notice carried in the About dialog. |
| Px437 IBM VGA font trace | `vgms-ui/assets/fonts` | see `LICENSE-Px437-IBM-VGA.txt` beside it | A faithful trace of the IBM VGA ROM font. |
| `opl3-rs` (Nuked-OPL3 C bindings) | `vgms-synth`, `c-parity` feature | LGPL-2.1-or-later | **Never shipped.** Off by default, needs a C compiler and libclang, and cannot target wasm. A parity oracle only. |

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
| YM2612 (`vgms-cores-nuked/opn2.rs`) | 0.227 (n=12); in-mix fit ≈4.0× | ×4.2 | `OUTPUT_GAIN = 21` |
| YM2151 (`vgms-cores-nuked/opm.rs`) | 0.500 (n=12) | ×2 | `OUTPUT_GAIN = 2` |
| YM2203 (`vgms-cores-nuked/opn.rs`) | 0.497 (n=12) | ×2, whole mix | `output_scale() = 2`; 0.994 verified (n=12) |
| YM2610 (`vgms-cores-nuked/opn.rs`) | 0.318 post-ADPCM (n=12) | ×3, whole mix | `output_scale() = 3`; 0.955 verified (n=12) |

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
