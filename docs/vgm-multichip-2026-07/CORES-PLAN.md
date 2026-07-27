# CORES-PLAN — every chip playable, the most accurate core selectable per chip

> Written 2026-07-27, from the licensing analysis session. Supersedes the
> mc-8/mc-9 sketch in `HANDOVER.md` (the "corpus-ordered core waves") with a
> concrete programme: a **license split**, a **multi-core registry**, an
> **upstream-tracking policy that avoids vendoring**, and a per-chip **core
> picker in Settings**. Everything else in the handover stands.

## 0 · Decisions locked by the user (2026-07-27, this session)

1. **dro-trimmer (the app) goes GPL-2.0-or-later.** Accuracy wins: the app may
   link GPL-2 and LGPL cores. The 2026-07-20 "GPL approved" decision is hereby
   spent.
2. **dro-synth becomes permissive** (MIT OR Apache-2.0) — a reusable,
   clearly-licensed alternative to libvgm whose consumers owe no source. Its
   own cores are permissive-sourced only (clean-room, or ported from
   MIT/BSD/ISC/zlib code with notices retained). Copyleft cores live in
   separate provider crates the *app* links, never in dro-synth.
3. **The user picks the core per chip, at runtime, in Settings** — the mc-7
   per-chip output widget grows into a core picker. Cores are data (a
   registry), not compile-time facts.
4. **wasm32-unknown-unknown stays first-class.** A core either compiles to
   wasm or is cleanly absent there (the registry simply doesn't list it; the
   UI follows the registry).
5. **Avoid vendoring where possible.** Actively-maintained upstream cores are
   consumed as **git submodules pinned to a commit** and compiled as-is;
   pulling upstream fixes is `git -C vendor/upstream/<x> pull` + a pin bump,
   never a re-port. Rust ports are reserved for sources that cannot be
   consumed directly (C++ upstreams, license-entangled files) — see §3.
6. **Nuked-CQM ships as a selectable OPL3 core** (Creative CQM — the OPL3
   clone in SB16 Vibra / AWE64 cards; LGPL-2.1). An authenticity flavour
   beside Nuked, not a replacement.
7. **No GPL-3-only code in any shipped binary** (Mesen2, BlastEm). They are
   A/B oracles run as separate programs. Non-commercial-clause code stays
   excluded too (a further restriction under GPL — §2.1.9 of the handover).
8. **The full VGMRips corpus is available** at
   `F:\GameMusic\VGM\VGMRips_all_of_them_2025-10-17` — 72,481 .vgm/.vgz files
   in 13 system folders (Arcade, Computers, GameBoy, MegaDrive, Misc, NES,
   NeoGeo, NeoGeoPocket, Other, Pinball, SegaPico, TurboGrafx, WonderSwan).
   It is organised **by system, not by chip** — cr-2 builds a chip index.

## 1 · The license change (cr-1)

Today the whole workspace is `LGPL-2.1-or-later` via `workspace.package`. The
split:

| Crates | New license | Why |
|---|---|---|
| `dro-core`, `dro-synth` | **MIT OR Apache-2.0** | The reusable pair — the VGM/DRO file model and the engine + permissive cores. All own clean-room code today (the vgmtools equivalents were Route-B precisely to keep options open; this is the payoff). dro-synth's public API exposes dro-core types, so the permissive goal forces both. |
| `dro-trimmer`, `dro-ui`, `dro-audio-native`, `dro-retrowave`, `dro-web`, `dro-synth-worklet` | **GPL-2.0-or-later** | The app and its glue. Links whatever gives the best sound. |
| `vendor/nuked-opl3` | LGPL-2.1-or-later (unchanged — upstream's terms) | Becomes an **optional, default-on** dependency of dro-synth (`nuked-opl` feature). A permissive consumer builds `--no-default-features`; the crate's own license expression stays clean because optional deps convey nothing until enabled. |
| New provider crates (§2) | Per contents: `dro-cores-nuked` LGPL-2.1-or-later, `dro-cores-gpl` GPL-2.0-or-later | Leaf crates; only the app depends on them. |

Mechanics, all in cr-1:

- Replace the workspace `license` key with per-crate `license` fields.
- `licenses/` directory at the repo root with the full GPL-2.0, LGPL-2.1,
  MIT, Apache-2.0 texts; `docs/LICENSE.txt`, README, and the About dialog
  updated to describe the split. The combined desktop/web binary is
  **GPL-2.0-or-later** and its About box must say so.
- About dialog gains a **core credits panel** fed from the registry (§2):
  every registered core reports name, upstream URL, authors, license. This is
  the runtime face of the provenance policy and grows automatically (same
  pattern as the mc-7 widget rows).
- `PROVENANCE.md` in dro-synth: one row per core/port — read-source, upstream
  commit, license, local deltas. The document libvgm is missing.
- SPDX header comments on each crate's `lib.rs`/`main.rs` (not every file —
  the Cargo `license` field is authoritative; ported files carry their
  upstream notices verbatim, which BSD-3/MIT require).
- Sanity gate: `git shortlog -sn` to confirm sole authorship before
  relicensing own code (expected: yes).

## 2 · The core registry (cr-2)

`core_for(ChipKind) -> Option<Box<dyn ChipCore>>` (chip.rs:99) is a
hard-coded match. It becomes a **registry**:

```text
CoreInfo {
    id: &'static str,          // "sn76489.native", "opl3.nuked", "opl3.cqm", ...
    chip: ChipKind,            // one entry per (core, chip) it serves
    label: &'static str,       // Settings row text: "Nuked-CQM (SB16 Vibra)"
    license: &'static str,     // "LGPL-2.1-or-later" — feeds About credits
    upstream: &'static str,    // URL — feeds About credits
    realtime: bool,            // false = offline render + oracle only (LLE tier)
    make: fn() -> Box<dyn ChipCore>,
}
```

- dro-synth owns `CoreRegistry` and registers its **built-in permissive
  cores** in `CoreRegistry::with_builtins()`. Provider crates export a plain
  `register(&mut CoreRegistry)` function the app calls at startup — explicit
  and deterministic, no link-time magic, wasm-safe. Dependency direction:
  providers depend on dro-synth's trait; dro-synth names no provider.
- Priority = registration order per chip; first registered is the default.
  The app registers accuracy-tier providers *before* built-ins so Nuked-class
  cores win by default where present.
- Config: **`audio.core.<chip_slug>` = core id** (`audio.core.opl3 = cqm`).
  This is the migration mc-7 deferred ("generalises when a second chip has
  somewhere else to go" — that moment is now): `audio.output_backend =
  RetroWave` migrates to `audio.core.opl3 = retrowave`, `Emulated` to
  `nuked`. A configured id that no longer exists falls back to priority
  order, logged, never fatal (the web build genuinely lacks some ids — §4).
- `playability` (chip.rs:108) and `DocCapabilities` read the registry filtered
  to `realtime` cores available on this build; `renderable` accepts
  non-realtime ones too.
- **The OPL path is special and stays special.** OPL cores implement
  `OplChip` (register policy, muting, panning, RetroWave live there); the
  Settings OPL row lists OplChip-shaped entries (Nuked, CQM, RetroWave
  hardware) — one selector, mixed emulated/hardware, exactly the row mc-7
  already drew. Generic chips list `ChipCore` entries. A core that serves
  OPL VGMs *and* DROs (CQM does — it is an OPL3) plugs in at `OplChip`, so
  every OPL consumer (player, render, worklet, split) gets it for free.
- Also in cr-2: the **corpus chip index**. A small tool walks
  `DROTRIM_VGMRIPS_CORPUS` (new env var, set to the F: path) with the
  existing header reader and caches `chip -> [files]` (JSON, target/ or
  alongside the corpus). Per-core tests draw N files per chip from it; the
  system folders stop mattering.

Settings UI (same step): each `widgets/chip_output.rs` row becomes a core
selector fed by the registry — label, license shown small (the user should
see "GPL-2.0" next to an LLE core), hardware entries where they exist. The
closing "chips with none" line stays. Snapshot tests per the established
kittest discipline; a registry-coverage test replaces the current
rows-cover-the-table test. Core switching applies on next `load()` — the
`SwitchingAudioService` precedent, no live-swap machinery.

## 3 · Sourcing policy — submodules first, ports second

Three tiers, replacing the old vendor-everything assumption:

1. **Git submodule + `cc`** (preferred) — for actively-maintained C cores:
   the nukeykt family (CQM, OPN2, OPM, OPLL, PSG, the LLE series), ESFMu,
   SameBoy's APU if its C carves out cleanly. Submodules live under
   `vendor/upstream/<name>` pinned to a commit; the provider crate's
   `build.rs` compiles the needed files **unmodified**. Glue (allocators,
   no-libc shims) is written on our side, never patched into the submodule —
   so `git pull` + pin bump + corpus re-run is the whole upgrade.
2. **crates.io dependency** — where a maintained Rust crate of the right
   license exists (candidates to evaluate at the relevant step: `emu2413`,
   Ayumi ports). Same non-vendored property, cargo-native.
3. **Rust port with provenance header** (last resort, lives in dro-synth) —
   for upstreams that cannot be consumed directly: **C++ sources** (MAME
   devices, ymfm, SAASound — no C++ toolchain in this workspace, and C++ to
   wasm32-unknown-unknown needs a runtime we don't ship), and **libvgm's
   BSD-tagged C files**, which `#include` Valley Bell's *untagged* framework
   headers — compiling them drags unlicensed code into the build, so we port
   the tagged file's logic instead and cite it in `PROVENANCE.md`. These
   upstreams are mature and near-frozen; the tracking cost is a note in the
   port header saying which upstream revision it matches.

The existing hand-ported Rust `vendor/nuked-opl3` **stays** — shipped,
byte-tested, wasm-clean; upstream is quiet. It is the one legacy vendored
core, documented as such.

## 4 · wasm rules

- Every provider crate must either compile to wasm32-unknown-unknown or be
  excluded by `[target.'cfg(...)'.dependencies]` in the app; the registry on
  web simply lacks those entries and Settings shows what exists. No stubs.
- The nukeykt C cores are freestanding C — clang can target
  wasm32-unknown-unknown directly (the toolchain is already in the PATH
  prelude; rust-synth-emulation proved the route with Nuked-OPN2). **cr-3 is
  the proof-of-concept gate**: if Nuked-CQM won't compile/link/play in
  dro-web, the fallback per §3 is a Rust hand-port (accepting vendoring for
  that core) — decided then, not silently.
- `realtime: false` cores (LLE tier) are expected to miss wasm real-time
  budgets even when they compile; they are render/oracle cores everywhere,
  live-playback candidates only after a native benchmark says otherwise.

## 5 · Which core, per chip (the target state)

Accuracy default first; picker alternatives after. P = permissive enough for
dro-synth; N = dro-cores-nuked (LGPL); G = dro-cores-gpl (GPL-2).

| Chip | Default core (source, tier) | Alternatives in the picker |
|---|---|---|
| OPL2/OPL3 (+ dual OPL2) | Nuked-OPL3 — vendored Rust port, shipped | **Nuked-CQM (N, submodule)** · RetroWave hardware · later ESFMu (N), YMF262-LLE / YM3812-LLE (G, render-only) |
| YM2413 | Nuked-OPLL (G, submodule) | emu2413 (P — crate or port; the dro-synth-only build's default) |
| SN76489 | our clean-room core (P, shipped) | Nuked-PSG (G, submodule; SMS/MD VDP flavour) |
| YM2612 / YM3438 | Nuked-OPN2 (N, submodule) | YM2608-LLE / YMF276-LLE (G, render-only) |
| YM2151 | Nuked-OPM (N, submodule) | YM2151-LLE (G, render-only) |
| YM2203 / 2608 / 2610(B) | MAME fmopn logic → Rust port (G) — the pragmatic accuracy route now the app is GPL | ymfm-informed permissive rewrite (P, later, for dro-synth); YM2608-LLE / YM2203-LLE (G, oracles); Nuked-OPNB when it leaves WIP (N) |
| AY8910 / YM2149 | Ayumi (P — MIT, crate/port per §3) | MAME ay8910 port (P) |
| Game Boy DMG | SameBoy APU (P — MIT; submodule if its C carves out, else port) | gb_mame port (P) |
| NES APU | clean-room from NESdev docs (P) | oracles: Mesen2, NSFPlay (never linked) |
| HuC6280 | c6280_mame port (P) | — |
| Y8950 / YMF278B(PCM half) | ymfm-informed Rust (P) + datasheets | openMSX as oracle |
| SAA1099 | SAASound-informed port (P — BSD-3, C++ upstream) | saa1099_vb as behaviour cross-check |
| Pokey | Altirra-informed (G) or MAME port (P) — decide at step | — |
| WonderSwan | ares-informed clean-room (P — ares is ISC) | MAME port (P) |
| VSU | ares-informed clean-room (P) | Mednafen as oracle |
| Mikey | laoo core (P — MIT; submodule) | — |
| QSound | qsound_hle port (P — BSD-3) | MAME DSP16 LLE needs a non-redistributable ROM — out of scope |
| ES5503 / ES5505/06 | vgsound_emu (P — zlib) / MAME ports | — |
| PCM long tail (SegaPCM, RF5C68/164, MultiPCM, uPD7759, OKIM6258/6295, K051649/K054539/K053260, C140/C219, C352, YMZ280B, X1-010, GA20, SCSP) | MAME logic → Rust ports (P), batched by family | — |

Rules that ride along: when a chip is studied for its core, add its
`chip_cmp` trigger rules to the optimiser's table in the same step (the
standing note from the handover), and revisit the cautious YM2612/YM2413
exclusions once a render oracle exists.

## 6 · Acceptance, per core

1. **Corpus walk** — every indexed file for the chip loads, seeks, renders
   its full bounded window without panic; non-silence asserted where the
   stream writes key-ons. Extends `engine_corpus.rs`, driven by the chip
   index, `DROTRIM_VGMRIPS_CORPUS` gated like the existing corpus.
2. **A/B listening vs VGMPlay** — unchanged: a person does it; a core is
   unverified until someone has listened.
3. **LLE oracle diff** where one exists (OPL3, OPL2, OPN family, OPM, PSG):
   offline render, automated comparison. This is the new capability the GPL
   move buys — the acceptance bar becomes mechanical for the chips that
   matter most. Not CI; a documented `xtask`-style run.

## 7 · Step list

Confirm before starting each step (§4.2 of the handover); commit per step;
workspace green including the wasm check build.

| Step | Contents |
|---|---|
| cr-1 | The license split (§1): per-crate licenses, `licenses/`, README/About/`docs/LICENSE.txt`, About core-credits panel, `PROVENANCE.md`, authorship check. No behaviour change. |
| cr-2 | Registry + config migration + Settings core picker (§2); corpus chip index tool; `nuked-opl` feature-gating in dro-synth. Registry entries: existing SN76489 + OPL path. |
| cr-3 | **Submodule infrastructure + Nuked-CQM** (§3, §4): `vendor/upstream/`, `dro-cores-nuked` crate, `build.rs` + clang-to-wasm proof, CQM as an `OplChip`, OPL row entry beside Nuked/RetroWave. Check the PlayerEngine's Nuked-specific buffered-write spacing against CQM's semantics. The PoC gate for the whole submodule approach. |
| cr-4 | YM2612/YM3438 via Nuked-OPN2 submodule. MegaDrive folder becomes fully audible (PSG + FM). Optimiser: revisit the 0x2A/0x28 exclusions against an LLE render. |
| cr-5 | YM2151 via Nuked-OPM (N, submodule). |
| cr-6 | NES APU (clean-room from NESdev docs) + Game Boy DMG (SameBoy APU). NES + GameBoy folders audible. |
| cr-7 | **AY8910 / YM2149** (P, clean-room) + HuC6280. The AY is *also the SSG section of every OPN chip*, so it is cr-8's foundation as well as a chip in its own right. |
| cr-8 | OPN family (YM2203/2608/2610): fmopn-logic port (G) reusing cr-7's SSG. NeoGeo folder audible. The biggest single step, and the one that **stands up `dro-cores-gpl`**; LLE oracles first. |
| cr-9 | YM2413: Nuked-OPLL (G) + emu2413 (P). OKIM6295 + OKIM6258 alongside — the two biggest of the PCM chips, and both arcade. |
| cr-10 | PCM long tail, batched (P), plus WonderSwan, VSU and SAA1099. Arcade folder converges. |
| cr-11 | LLE tier as render-only cores + the oracle `xtask` (§6.3); Nuked-PSG; ESFMu if wanted. |
| cr-12 | Sweep: registry-vs-chip-table coverage test, About credits complete, `PROVENANCE.md` complete, docs. |

> **Nuked-OPNB is not usable, checked 2026-07-27.** §5 hoped for it "when it
> leaves WIP"; it has not. `nukeykt/Nuked-OPNB` (and `Nuked-OPNA`, which is the
> same repository) is version 0.0: its header declares `fm_ar` and `fm_ks`
> twice, so it does not compile; there is no reset function, no output function
> at all — `OPNB_Clock` takes no buffer — and no SSG or ADPCM. 649 lines against
> Nuked-OPM's 2,200. So the OPN family really does need the port route, and if
> Nuked-OPNB ever lands it is LGPL, which puts it in `dro-cores-nuked` rather
> than the GPL crate.
>
> That is why **cr-7 and cr-8 swapped**: the AY8910 is the SSG section of every
> OPN chip, so building it first makes the big step smaller rather than merely
> reordering it.
>
> **Reordered 2026-07-27 by corpus weight**, on the user's call, once cr-2's
> index made §7.1's numbers available. The old order ran YM2413 at cr-5, YM2151
> at cr-6 and NES APU at cr-7; measured, YM2413 is 1.8% of the corpus while
> YM2151 is 13.9% and NES APU 9.9%, so each early step now buys as much audible
> corpus as it can. The steps are independent, so this cost nothing but the
> numbering. One consequence worth noting: the first `dro-cores-gpl` content
> moves from cr-5 (Nuked-OPLL) to cr-7 (the fmopn port), so the GPL provider
> crate is stood up by the OPN family rather than ahead of it.

Corpus-weight note: MegaDrive + NES + GameBoy + Arcade dominate the 72k
files, which is why cr-4..cr-8 front-load those systems. Re-derive the exact
counts from the cr-2 index rather than trusting this sentence.

### 7.1 · What the corpus actually says (measured 2026-07-27, cr-2's index)

72,481 files, 39 of the 42 chips present (no SCSP, ES5505 or Mikey), 18 files
whose header would not read. Share is of all files, and sums past 100% because
a multi-chip rip counts once per chip.

| Chip | Files | Share | Plan step |
|---|---:|---:|---|
| YM2612 | 14,622 | 20.2% | cr-4 |
| SN76489 | 11,845 | 16.3% | **done** |
| **YM2151** | **10,069** | **13.9%** | cr-6 |
| **NES APU** | **7,191** | **9.9%** | cr-7 |
| YM2203 | 7,020 | 9.7% | cr-8 |
| YM2610 | 4,449 | 6.1% | cr-8 |
| AY8910 | 4,228 | 5.8% | cr-6 |
| HuC6280 | 4,178 | 5.8% | cr-9 |
| YM3812 | 3,934 | 5.4% | **done** |
| Game Boy DMG | 3,827 | 5.3% | cr-7 |
| OKIM6295 | 3,465 | 4.8% | cr-10 |
| OKIM6258 | 2,900 | 4.0% | cr-10 |
| YM2608 | 2,408 | 3.3% | cr-8 |
| QSound | 1,699 | 2.3% | cr-10 |
| **YM2413** | **1,284** | **1.8%** | **cr-5** |
| YMF262 | 1,003 | 1.4% | **done** |

Tail below 1%: C352 1,271 · K054539 872 · RF5C68 705 · C140 639 · K051649 512 ·
YMF278B 494 · RF5C164 419 · YMF271 376 · K053260 366 · YMZ280B 354 · VSU 353 ·
Y8950 268 · WonderSwan 266 · YM3526 247 · uPD7759 240 · X1-010 229 · MultiPCM
224 · Sega PCM 215 · PWM 184 · ES5503 145 · GA20 136 · SAA1099 116 · POKEY 30.

**Two places the measured order disagrees with §7, for the user to rule on:**

1. **YM2413 is cr-5 but is only 1.8% of the corpus** — below YM2151, NES APU,
   YM2203, YM2610, AY8910, HuC6280, Game Boy and both OKIM chips. It was
   presumably placed early because it is a small chip with a permissive core
   available (emu2413), which is a fine reason to do it *cheaply*, but a poor
   reason to do it *fifth*.
2. **YM2151 (13.9%, third overall) sits at cr-6 and NES APU (9.9%, fourth) at
   cr-7**, behind YM2413. Swapping YM2413 later and pulling those two forward
   would make each early step buy more audible corpus.

`OKIM6295` and `OKIM6258` are also worth noting: together 6,365 files, ranked
11th and 12th, but filed in the cr-10 "PCM long tail" batch.

Nothing has been reordered — the step order is the user's call. Re-run
`cargo test -p dro-trimmer --release --test chip_index -- --ignored --nocapture`
with `DROTRIM_VGMRIPS_CORPUS` set to regenerate this table.

## 7.2 · What actually shipped (branch `vgm-cores`)

| Step | Commit | Contents |
|---|---|---|
| cr-1 | `d5330f3` | The licence split: per-crate licences, `licenses/`, About core-credits panel, `PROVENANCE.md` |
| cr-2 | `039e193` | The core registry, `audio.core.<chip>` config + migration, Settings core picker, corpus chip index |
| cr-3 | `e2f4433` | Nuked-CQM by submodule — **the clang-to-wasm gate, passed** |
| cr-4 | `be731dd` | YM2612 / YM3438 (Nuked-OPN2) |
| cr-5 | `8c406a2` | YM2151 / YM2164 (Nuked-OPM) |
| cr-6 | `a425a64` | NES APU + Game Boy DMG, clean-room |
| cr-7 | `e85c330`, `c0fedf5` | AY-3-8910 + HuC6280, clean-room |
| cr-8 | `f829818` | YM2203 / YM2608 / YM2610, assembled from OPN2's FM and the AY's SSG |
| cr-9 | `aa2142e` | YM2413 (Nuked-OPLL) — **stands up `dro-cores-gpl`** |
| cr-10 | `83d3cbe` | OKIM6295 + OKIM6258, clean-room; Settings output list made scrollable |

**Thirteen chips play**, covering the great majority of the corpus by weight.

### Still open

- **cr-10's tail**: 26 chips, none above 1.8% — C352, K054539, RF5C68, C140,
  K051649, YMF278B, RF5C164, YMF271, K053260, YMZ280B, VSU, WonderSwan,
  uPD7759, X1-010, MultiPCM, SegaPCM, PWM, ES5503, GA20, SAA1099, POKEY, SCSP,
  ES5505, Mikey, Y8950, YM3526.
- **The OPN family's ADPCM** (§ `PROVENANCE.md`). YM2610 corpus audibility is
  9/12 against 12/12 for its relatives, and the gap is the drums. This is the
  one place a MAME-fmopn port would earn its keep, and it is worth more than
  several tail chips.
- **cr-11** entirely: the LLE tier as render-only cores, the oracle `xtask`,
  Nuked-PSG, ESFMu.
- **cr-12's remaining sweep**: the registry coverage tests landed with cr-10;
  the docs pass and a final `PROVENANCE.md` audit have not.
- **The A/B against VGMPlay, for every core.** Nothing here has been listened
  to. The per-core output gains — the FM-to-PSG and FM-to-SSG balances
  especially — are arithmetic that sounds plausible, not judgements anyone has
  made with their ears. §6.2 has always said a core is unverified until someone
  does this, and that remains true of all thirteen.

## 8 · Out of scope, recorded so nobody wonders

- Mesen2/BlastEm code in-binary (GPL-3 — decision 7). Oracle use only.
- MAME's QSound DSP16 LLE (needs the DL-1425 ROM; not redistributable).
- YM2414/OPZ (no VGM header slot through v1.72).
- Genesis Plus GX (non-commercial clause — unchanged from §2.1.9).
- Live-swap of a core mid-playback (next `load()` is the contract).
- The T6W28 / Game Gear stereo extensions of our SN76489 (tracked in the
  core's own notes; unrelated to this plan's structure).
