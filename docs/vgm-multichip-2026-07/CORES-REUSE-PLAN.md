# CORES-REUSE-PLAN — reuse upstream emulators; our clean-rooms become the fallback tier

> Written 2026-07-28 on branch `cores-reuse`, off `vgm-cores` at `847fe51`.
> Re-scopes [CORES-PLAN.md](CORES-PLAN.md) §3 and §5 after the user's call:
> *"I don't want this project to be a complete reimplementation of chip
> emulators — it will be difficult to get it to full accuracy. Reuse the
> existing emulators as much as possible, most permissive version in the
> regular crate, more accurate GPL version from the GPL crate."*
> Everything else in CORES-PLAN — the licence split, the registry, the core
> picker, the wasm rules, the acceptance gates — stands unchanged.

## 0 · Why this exists

The cores programme met its **coverage** goal and missed its **accuracy** one.
99.07% of the corpus plays; 36 of 39 chips sound. But the parity scorecard
frozen at `5f2fe45` reads, for the clean-room tier:

- Six cores are structurally wrong (QSound 0.046, MultiPCM 0.034,
  YMF278B 0.075 with half the chip missing, RF5C164 0.025, uPD7759 0.010,
  PWM 0.016).
- Five more sit near zero where the metric itself is suspect
  (WonderSwan, VSU, SAA1099, ES5503, X1-010 — the HuC6280 phase precedent).
- Ten open investigations are recorded in threshold reasons, each a research
  project. The RF5C164 one alone took a corpus probe and a new engine feature.

Every one of those is **ours to fix, forever, alone**. The same period showed
the alternative: three times on the YM2608 die the defect turned out to be our
*integration harness*, not emulation — and integration is a cost we pay whether
the core behind it is ours or upstream's. Reuse moves the emulation cost to
someone else's maintained repository and leaves us only the cost we cannot
avoid.

**The re-scope in one line:** upstream cores become the accuracy tier; our
clean-room cores stop being accuracy candidates and become the **wasm and
fallback tier**, which is a real job they are already good enough for.

## 1 · The licensing audit — the finding that shapes everything

Reuse is a licensing question before it is an engineering one. What was
verified on 2026-07-28, by fetching the actual files:

| Upstream | Licence | Evidence | Verdict |
|---|---|---|---|
| **ymfm** | **BSD-3-Clause** | Explicit header in `src/ymfm.h`: "2021, Aaron Giles" | **Clear — permissive tier** |
| **vgsound_emu** | **zlib** | Root `LICENSE`, "(C) 2022-present cam900 and contributors" — **covers the shared framework too**, which is precisely what libvgm lacks. Its zlib variant adds "you must notify your modifications": trivially satisfied, since we never modify a submodule. Active home is **GitLab** (`gitlab.com/cam900/vgsound_emu`); the GitHub repo is an archived v1 mirror | **Clear — permissive tier** |
| **emu2413 / emu8950** | **MIT** | Stated repo-wide | **Clear — permissive tier** |
| **Nuked-\*** (in tree) | LGPL-2.1 / GPL-2 | Explicit | Clear — copyleft tiers, unchanged |
| **libvgm** | **NONE FOUND** | No `LICENSE`/`COPYING` at root; GitHub licence API 404s; `Compiling.txt` silent; `EmuStructs.h`, `SoundEmu.h`, `2612intf.c`, `qsound_ctr.c` all carry **no licence tag**; only some MAME-derived files (e.g. `multipcm.c`) retain `// license:BSD-3-Clause` | **BLOCKED — see §5** |

**Why libvgm being unlicensed matters.** Code published without a licence grant
is, by default, all rights reserved. A git submodule is fine on its own — we
redistribute nothing, the user fetches from upstream. But **shipping a compiled
binary containing that object code is redistribution of a derivative work**, and
that is what dro-trimmer releases do. This is not a copyleft-compatibility
question that the GPL crate solves; there is no grant to be compatible with.

CORES-PLAN §3 already said as much ("libvgm's BSD-tagged C files `#include`
Valley Bell's *untagged* framework headers — compiling them drags unlicensed
code into the build"). That assessment was correct and is hereby re-affirmed
rather than overturned.

**This is a factual finding, not legal advice.** The project owner makes the
risk call; §5 lays out the options.

## 2 · The tiers, restated

Unchanged in shape from CORES-PLAN §1 — what changes is *who populates them*.

| Crate | Licence | Populated by | Job |
|---|---|---|---|
| `dro-synth` | MIT OR Apache-2.0 | **only the two clean-room cores that cleared 0.90: K053260 and C140** (§6) | Permissive fallback. No longer a complete wasm tier — see §6's cost note. |
| `dro-cores-ymfm` | MIT OR Apache-2.0 (wrapping BSD-3 ymfm) | ymfm submodule | Yamaha accuracy tier — **conditional on the ru-2 parity gate**; removed entirely if it does not clear. |
| `dro-cores-pcm` *(only if libvgm's licence gate fails)* | MIT OR Apache-2.0 (wrapping zlib/MIT upstreams) | vgsound_emu | Contingency PCM tier. |
| `dro-cores-nuked` | LGPL-2.1 | Nuked submodules | Cycle-accurate OPL/OPN2/OPM/PSG. Unchanged. |
| `dro-cores-gpl` | GPL-2.0-or-later | Nuked-OPLL, the LLE dies | Extreme accuracy + the decapped ROMs. Unchanged. |

Note what this does to the user's brief: because **ymfm is BSD-3, the accuracy
tier for every Yamaha chip lands in a *permissive* crate**, not the GPL one.
That is better than the brief asked for. The GPL crate keeps its existing job —
the LLE dies and the decapped 2608 rhythm ROM — and gains nothing new.

## 3 · Sourcing, per chip

> **Amended 2026-07-28 by the owner's three decisions:** OPL2/OPL3 is out of
> scope for reuse and keeps Nuked-OPL3 / Nuked-CQM / RetroWave (§6); the
> sub-0.90 clean-room cores are deleted *before* libvgm rather than kept as a
> fallback (§6); and ymfm survives only where already integrated **and**
> measured good (§8, ru-2). The table below therefore describes the
> **destination**, and between ru-0 and ru-3 most of these chips have **no
> core at all** — that gap is the deliberate cost recorded in §6.

Accuracy default first. **P** = permissive, **N** = LGPL, **G** = GPL,
**CR** = our clean-room (fallback/wasm everywhere it appears).

| Chip | New accuracy default | Today | Why the change |
|---|---|---|---|
| YM2608 | **ymfm** (P) | CR 0.601 | Rhythm + Delta-T + SSG, one maintained core |
| YM2610 / 2610B | **ymfm** (P) | CR 0.769 | ADPCM-A/B properly |
| YM2203 | **ymfm** (P) | CR 0.640 | — |
| **Y8950** | **ymfm** (P) | **unregistered** | Unblocks the parked core without the OPL-routing audit |
| **YMF278B** | **ymfm** (P) | CR 0.075 | We model the wave side only; ymfm has the whole chip |
| YM2413 | ymfm (P) — alt to Nuked-OPLL | Nuked-OPLL (G) | Gives dro-synth-only builds a real OPLL |
| YM2151 | ymfm (P) — alt | Nuked-OPM (N) 0.999 | Nuked stays default; ymfm is a permissive-build option |
| YM2612 / 3438 | ymfm (P) — alt | Nuked-OPN2 (N) 0.985 | as above |
| **OPL2 / OPL3 (YM3812, YMF262, YM3526)** | **no change — out of scope for reuse** | Nuked-OPL3 (default), Nuked-CQM, RetroWave | Owner's decision: these three options stay and nothing is added. `PlayerEngine` is the DRO *editing* engine, not merely a playback core |
| YM2149 / AY8910 | ymfm SSG (P) — alt | CR 0.597 | — |
| ES5505/06, ES5504 | **vgsound_emu** (P) | none / CR | Also unblocks ES5505, currently unplayable |
| K053260 | vgsound_emu (P) | CR 0.990 | Only if it beats us — we are already good here |
| K051649 (SCC) | vgsound_emu (P) | CR, unmeasurable | — |
| X1-010 | **vgsound_emu** (P) | CR 0.029 | — |
| OKIM6295 | vgsound_emu (P) | CR 0.676 | — |
| QSound, MultiPCM, uPD7759, RF5C68/164, C140/C219, C352, K054539, YMZ280B, SegaPCM, GA20, PWM, WonderSwan, VSU, SAA1099, ES5503, SCSP, POKEY, YMF271 | **unresolved — §5** | CR, mostly weak | libvgm was the answer; it is blocked |

Chips where our clean-room already scores well (K053260 0.990, C140 0.974,
YM2151 via Nuked 0.999) are **not** swapped on principle. The scorecard
arbitrates: a swap ships only if it measures better.

## 4 · The ymfm provider crate — design

The first and largest piece, and the one with no licensing question.

**Layout.** `vendor/upstream/ymfm` as a git submodule pinned to a commit
(policy unchanged from CORES-PLAN decision 5: never edit the submodule; upgrade
is `git pull` + pin bump + corpus re-run). `crates/dro-cores-ymfm/` holds
`build.rs`, `shim/ymfm_c.cpp`, and the Rust wrappers.

**The C++ problem, and why a shim.** ymfm is C++14 and Rust cannot call C++
directly, so `shim/ymfm_c.cpp` exposes an `extern "C"` surface over an opaque
handle. This is the same shape as the LLE shims already in `dro-cores-gpl`, so
the pattern is known. `cc::Build::new().cpp(true).std("c++14")`; `clang++` is
already in the PATH prelude.

**The interface object.** ymfm chips take a `ymfm_interface&` supplying timers,
IRQ and external memory. `examples/vgmrender/vgmrender.cpp` upstream is the
reference implementation and the shim follows it: a `vgm_chip`-alike that
subclasses `ymfm_interface`, owns the chip, and services `ymfm_external_read`
from data we hand it.

**The C surface** (one binding, every chip — mirroring what makes this cheap):

```c
ymfm_handle* ymfm_create(int kind, uint32_t clock);
void ymfm_destroy(ymfm_handle*);
void ymfm_reset(ymfm_handle*);
uint32_t ymfm_sample_rate(ymfm_handle*);
void ymfm_write(ymfm_handle*, uint32_t addr, uint8_t data);
void ymfm_generate(ymfm_handle*, int32_t* out, uint32_t frames);
void ymfm_load_data(ymfm_handle*, int access_class, uint32_t offset,
                    const uint8_t* data, uint32_t len);
```

`ymfm_load_data` is how `ACCESS_ADPCM_A` / `ACCESS_ADPCM_B` / `ACCESS_PCM` ROM
blocks arrive — which is exactly what our `ChipCore::load_rom` already receives
from `banks::block_owner`. The mapping is direct.

**The 2608 rhythm ROM, honestly.** ymfm does *not* ship the YM2608's internal
rhythm samples; it asks for them through `ymfm_external_read(ACCESS_ADPCM_A)`.
So ymfm alone does not fix the drums — it fixes everything *around* them. The
decapped ROM we already have lives in a GPL submodule, so feeding it to ymfm
produces a GPL-licensed combination: therefore **`dro-cores-gpl` gains an
optional "ymfm + rhythm ROM" registration**, and the permissive
`dro-cores-ymfm` registers a 2608 whose drums are silent, stated. This is the
existing tier discipline applied to a new case, not a new rule.

**Write pacing.** Trap 1 from `PROVENANCE.md` — nukeykt cores latch writes to
rotation slots and silently drop mistimed ones. ymfm is a different design
(clock-independent, `generate()` advances it), so the existing `WriteQueue`
spacing numbers do **not** transfer. Each family gets its pacing established by
the same procedure the table documents, and a silent core is the expected first
symptom if this is got wrong.

**wasm.** ymfm needs a C++ standard library; `wasm32-unknown-unknown` has none.
Upstream `libymfm.wasm` uses wasi-sdk against `wasm32-wasi`, which is a
different target from ours. So **ymfm is native-only for now** and the registry
simply will not list it on web — exactly the mechanism CORES-PLAN §4 specifies,
no stubs. This is precisely why the clean-room tier keeps a job.

## 5 · The libvgm question — gated, with options

> **Update 2026-07-28:** the owner asked for a plan that *assumes* option 2
> succeeds. [LIBVGM-PLAN.md](LIBVGM-PLAN.md) is that plan — libvgm as the
> **primary** core source, not one provider among several, because one API
> covers all 50 of its device IDs and five of them are chips we cannot play
> at all. It is engineering-ready and gated at step lv-0 on the grant below
> actually existing. The audit in §1 stands unchanged until then.

libvgm would have answered the entire PCM tail in one integration. It is blocked
on §1. The options, for the owner's decision:

1. **Per-file harvest (recommended if the tail matters).** Compile only files
   carrying an explicit `license:BSD-3-Clause` tag, and replace the untagged
   framework (`EmuStructs.h`, `SoundEmu.h`, `EmuHelper.h`) with headers we write
   ourselves from the interface those files require. We have done exactly this
   before — `dro-cores-nuked/shim/string.h` — and it is legally clean because
   we redistribute only tagged code plus our own. Cost: a per-file audit and our
   own framework, so the "one integration" saving is partly lost, but it is
   still far cheaper than reimplementing each chip.
2. **Ask upstream.** Open an issue asking Valley Bell to add a LICENSE. Costs
   nothing, may resolve everything, but is outside our control and not a plan.
3. **Oracle only.** Keep libvgm/VGMPlay as the reference player it already is —
   never linked, never shipped. Zero risk, zero accuracy gain.
4. **Source-only tier.** An opt-in cargo feature, off by default, that no
   release binary enables. Defensible but complicated, and it splits the
   product.
5. **Route the tail through MAME instead.** MAME's sound devices carry explicit
   per-file BSD-3 tags. The cost is MAME's device framework (`device_t`,
   `sound_stream`), which is heavy to satisfy — this is why CORES-PLAN chose
   porting. Worth re-evaluating per chip now that `clang++` is available.

**Recommendation: (2) immediately and cheaply, (1) for the chips that matter
most, (3) meanwhile.** Nothing about §4 depends on this decision, which is why
ymfm goes first.

## 6 · The cull — what happens to the clean-room cores

> **Owner's decision, 2026-07-28:** delete every clean-room core scoring
> **below 0.90** on the frozen scorecard, and do it **before** libvgm is
> implemented rather than after.

**Two survive**, measured against VGMPlay at `bee82d7`:

| Kept | Score | Note |
|---|---|---|
| **K053260** | 0.9895 (n=6) | The cleanest tail row on the board. |
| **C140** | 0.9740 (n=12) | Keeps its stated C219 gap: that variant is approximated as linear addressing and is audibly silent on NA-1/NA-2 rips. |

**Twenty-eight modules go.** Twenty-five measured below the bar — YM2610
0.769, K054539 0.748, OKIM6295 0.676, YMZ280B 0.664, YM2203 0.640, YM2608
0.601, AY8910 0.597, SN76489 0.586, OKIM6258 0.557, C352 0.456, RF5C68 0.432,
NES APU 0.334, Game Boy DMG 0.295, YMF278B 0.075, HuC6280 0.075, VSU 0.074,
QSound 0.046, MultiPCM 0.034, SAA1099 0.031, X1-010 0.029, RF5C164 0.025,
WonderSwan 0.022, PWM 0.016, ES5503 0.015, uPD7759 0.010 — plus **SegaPCM,
K051649 and GA20**, which have no single-chip corpus file and therefore cannot
demonstrate 0.90, and **Y8950**, which was never registered.

Two notes on the edges, recorded so the rule is applied knowingly rather than
blindly. **HuC6280** is not demonstrably bad — correlation is meaningless for
it (VGMPlay's own two cores score -0.19 against each other) and its envelope
agreement is 0.97 — but it cannot *prove* 0.90 by the agreed metric, and
libvgm carries `DEVID_C6280`, so it goes with the rest. **YM2203/2608/2610**
are labelled clean-room but are hybrids: cr-8 assembled them from Nuked-OPN2
for the FM and the clean-room AY for the SSG. Culling them removes that
assembly, and ymfm (§8, ru-2) is what must replace it.

**The scorecard rows for culled chips are deleted with their cores**, since a
threshold guards a core that no longer exists. The SCORECARD chronicle stays —
it is the *evidence* for the cull and must outlive the code.

**What this costs, stated plainly.** Between the cull and libvgm landing, the
app plays only OPL2/OPL3, YM2413, YM2612/YM3438, YM2151, C140, K053260, and
SN76489 via Nuked-PSG — plus whatever ymfm keeps at ru-2. Full corpus
playability drops from 99.07% to a fraction of that, and **the web build loses
every non-OPL chip**, because the clean-room cores were the only wasm-capable
ones. This is a deliberate, bounded interval, not a regression to be
discovered: `rust` must not take this state (see §8, ru-0's note), and
`scratchpad/coverage.py` should re-measure so the number is known rather than
assumed.

**The OPL invariant is untouched, and OPL is now explicitly out of scope for
reuse.** Per the owner: OPL2/OPL3 keep **Nuked-OPL3 (default, the vendored
Rust port), Nuked-CQM, and RetroWave** — and nothing else. libvgm and ymfm do
**not** supply OPL cores here. `PlayerEngine` carries the buffered-write
spacing, muting and panning that the DRO editor depends on, so this is the
editing engine, not merely a playback core. `has_core` vs `can_build`
(CORES-PLAN §2) still holds.

## 7 · Acceptance — how a swap is proven

Unchanged from CORES-PLAN §6, with one addition: **a reused core must beat the
clean-room row it replaces on the frozen scorecard**, or it does not take the
default. Per core:

1. Unit tests in the wrapper (register write reaches the chip; reset clears;
   ROM load lands) — the wrapper is ours, so it is tested like our code.
2. `tests/core_audio.rs` corpus audibility, 12 files.
3. `reference_parity.rs` row vs VGMPlay, compared against the frozen
   clean-room number.
4. For chips with an LLE die, `oracle_lle.rs` as the third witness.
5. `PROVENANCE.md` row: upstream, commit pin, licence, what our shim adds.

## 8 · Step list

| Step | Work |
|---|---|
| **ru-0** | **The cull (§6), first.** Delete the 28 sub-0.90 clean-room modules, their registry entries, their `ChipKind` core mappings and their parity threshold rows; regrow the settings snapshot; re-measure coverage. Keep K053260 and C140. **Method: subtract from the green tree, never reconstruct one** — every VGM decode fix (`0xC1`-`0xC3` port routing, `0xE1` and `0xC5`-`0xC8` big-endian, `0xC4` QSound, `banks::block_owner`) lives *inside* a clean-room core commit, so it survives only by not being touched. `rust` must not take this state; it stays at the `vgm-multichip` foundation until coverage is restored. |
| **ru-1** | *(DONE, `c0fb1ea`)* ymfm submodule + `dro-cores-ymfm` + the PoC gate: all six OPN-family chips build, link, take writes and sound. |
| **ru-2** | **The ymfm parity gate.** Per the owner: ymfm survives *only* where it is already integrated **and** scores well. Complete the shim surface and write pacing, register the OPN family, and measure. Chips that score below the bar are dropped from the registry; if none clear it, `dro-cores-ymfm` is removed entirely and libvgm carries the Yamaha family alone. **No further ymfm investment (Y8950, YMF278B, the rhythm-ROM variant) until this gate reports.** |
| **ru-3** | libvgm, per [LIBVGM-PLAN.md](LIBVGM-PLAN.md) lv-0..lv-8. This is where coverage comes back. |
| **ru-4** | vgsound_emu (C++11, zlib, from GitLab) — **only if libvgm's lv-0 licence gate fails**, or for chips libvgm serves worse. Its V2 list: AY-3-8910, SN76489, ES5504/5505/5506, GA20, HuC6280, K005289, K007232, K053260, MMC5, MSM5205/6585, MSM6295, Namco 163, C140/C219, NDS, NES APU, SCC, SM8521, VRC VI, X1-010. |
| **ru-5** | Merge to `rust` **only once playable-chip coverage meets or beats the pre-cull 99.07%**. |
| **ru-6** | Sweep: About credits, `PROVENANCE.md`, docs, Settings picker copy, the CORES-PLAN §5 table superseded. |

Steps are independently shippable and each ends green (fmt, clippy, suite,
`--no-default-features`, wasm check).

## 9 · Risks, named

- **ymfm's timer/IRQ model** is richer than anything we drive today. If the
  shim's timer service is wrong, chips will sound *nearly* right, which is the
  worst failure mode. `vgmrender.cpp` is the reference; the corpus is the test.
- **Write pacing** (§4) — a silent core is the expected symptom, and the
  existing numbers do not transfer.
- **Binary size and build time** grow with a C++ dependency; both are measured
  at ru-1, not assumed.
- **wasm regression** — the web build must keep working with zero ymfm. The
  `--no-default-features` and wasm checks in every step's green gate are what
  catch this.
- **Scope creep back into emulation.** The discipline of §6 is the whole point:
  when a reused core is 0.9 and our clean-room was 0.03, the remaining 0.1 is
  *upstream's* to close, not ours.
