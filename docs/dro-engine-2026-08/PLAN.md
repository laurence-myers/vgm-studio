# Retire `DroEngine`: one engine, one sound

**Branch:** `dro-engine-2026-08` (proposed; branch after `dro-arm-2026-08` merges)
· **Status:** planned, not started.
· **Backlog origin:** [DIVERGENCE.md §7 item 1](../dro-arm-2026-08/DIVERGENCE.md).

## Why

Live playback is already unified: every document — DRO included — plays through
`VgmEngine` over a projected VGM (`vgms-audio-native`, the web worklet, and the
RetroWave pump all do this, "ou-2"). `DroEngine` survives in exactly four
offline pipelines: the DRO WAV render, the DRO peak scan, the DRO waveform, and
the CLI `render` DRO arm.

That split is the **hear-vs-export gap**: a DRO is exported by different code
than plays it, so a render can not sound like what the user heard. Retiring
`DroEngine` closes the gap, deletes a whole second engine and its register
policy, removes the loaded `DelaySamples`/ms-clock trap, and lets the DRO
offline pipelines honour the resampling mode and the Settings core choice like
everything else already does.

It also makes the deferred [`dro-arm` Stage 5](../dro-arm-2026-08/PLAN.md) moot:
once `DroEngine` goes, what remains in `engine.rs` *is* the shared clock module
that stage wanted to extract.

## Findings (verified in code; the v1 one empirically)

### The production surface is tiny

Four `DroEngine` constructions, all inside `vgms-synth`:
[wav.rs:182](../../crates/vgms-synth/src/wav.rs) and
[:191](../../crates/vgms-synth/src/wav.rs),
[peak.rs:86](../../crates/vgms-synth/src/peak.rs),
[waveform.rs:196](../../crates/vgms-synth/src/waveform.rs). Four production
callers: three in the GUI task layer
([tasks.rs:282](../../crates/vgms-ui/src/tasks.rs) peak,
[:312](../../crates/vgms-ui/src/tasks.rs) waveform,
[:349](../../crates/vgms-ui/src/tasks.rs) wav — which the web worker also
executes via `run_task`) and one CLI
([cli/render.rs:74](../../crates/vgms-app/src/cli/render.rs)). Nothing else in
the workspace constructs or calls it.

### The DRO v1 waveform-select prime is a **live bug**, and it blocks this work

`DroEngine::reset_chip` writes `0x01 = 0x20` (waveform-select enable) when the
song is DRO v1 — [engine.rs:775-777](../../crates/vgms-synth/src/engine.rs),
pinned by `the_v1_waveform_select_hack_is_primed_on_reset`
([engine.rs:1203](../../crates/vgms-synth/src/engine.rs)). **Nothing on the VGM
path emits it**: `dro_to_vgm` iterates only the song's own instructions
([convert.rs:115-135](../../crates/vgms-core/src/convert.rs)) and
`OplCoreAdapter::reset` primes nothing
([opl_adapter.rs:152-162](../../crates/vgms-synth/src/opl_adapter.rs)).

Confirmed empirically — projecting a v1 song whose only write is `0x20 = 0x01`
yields exactly:

```
Write { Ym3812, addr: 0x20, data: 0x01 }
Wait(485)
```

No `addr: 0x01, data: 0x20`. So **today a DRO v1 file already loses WSE in live
playback** (native, web and hardware all project) while its offline render keeps
it. On OPL2 the waveform-select registers `0xE0..=0xF5` are ignored unless WSE
is set, so a v1 capture's non-sine timbres collapse to sine on playback and come
back on export — audible, not cosmetic.

This is the inverse of the gap the audit found, and it is **load-bearing for
this programme**: retire `DroEngine` without fixing it and every DRO v1 render
silently adopts the wrong behaviour. Fix it first, and the retirement becomes
behaviour-preserving.

### The two paths agree exactly only at 49716 Hz

`DroEngine` builds `DefaultOplChip::new(sample_rate)` — Nuked reset *at the
output rate*, using the chip's own internal resampler. `OplCoreAdapter` always
resets at `NATIVE_SAMPLE_RATE` (49716) and lets the engine's `Voice` resampler
convert ([opl_adapter.rs:152-157](../../crates/vgms-synth/src/opl_adapter.rs)).
The existing A/B (`a_dro_sounds_the_same_projected_through_the_vgm_engine`,
[opl_ab_parity.rs:83](../../crates/vgms-app/tests/opl_ab_parity.rs)) deliberately
renders at 49716 "so neither side's resampler enters the measurement" — so the
parity evidence we have covers the **ms→sample timing requantization only, not
the resampler difference**. At 44100/48000 the output will change.

### What moves, and what anchors

**Baselines that must be re-blessed:** `tests/render/{full,muted,boosted,panned,
combined}.wav` (blessed at 48 kHz under `DroEngine`, byte-compared, re-bless via
`UPDATE_RENDER_FIXTURES=1`); and the kittest PNGs that paint a real DRO
waveform — `loaded_tone_song.png`, `loop_overlay.png`, and the eight
`theme_showcase_*.png`.

**Anchors that do not move:** `golden_opl.rs` (a SHA-256 over a bare `NukedOpl3`
render at 49716) and `c_parity.rs` (Nuked vs the C reference) never touch
`DroEngine`. They stay the independent "is the emulator itself still right?"
check while the render fixtures are re-blessed — exactly the role
`golden_opl.rs`'s own header claims.

### Some of it is already dead

`render_dro_waveform_cancellable` has no callers outside its own two unit tests.
`DroEngine::set_loop` and the whole `loop_config`/`wraps_remaining` machinery
have no production caller at all — every offline pipeline runs one un-looped
pass. `render_dro_wav`, `render_dro_wav_mixed`, `measure_dro_peak` and
`render_dro_waveform` have **zero production callers**; they survive only as
test oracles.

### The vocabulary and core-choice couplings

The DRO render takes `RenderMix { Muting, Panning }` (the OPL register-gate
vocabulary) while the VGM peer takes `VgmRenderMix { ChipMuting, ChipPanning }`.
`RenderWavMix::Opl` is built at
[app/split.rs:26-38](../../crates/vgms-ui/src/app/split.rs) by *reverse-bridging*
the chip panel's generic values back into the OPL ones — even though the panel's
native output is already `chip_muting()`/`chip_panning()`.

Separately, the DRO render consults only the per-render override, never the
process-wide Settings core, and says so
([wav.rs:171-176](../../crates/vgms-synth/src/wav.rs)). The VGM path honours
both. Retirement fixes that asymmetry by construction.

## Decisions

- **D-de-1 — fix the v1 prime first, as its own shippable commit.** It is a
  live-playback bug on its own merits; ship it whether or not the retirement
  proceeds. Prime in the **playback/render projection**
  (`opl_song_to_vgm_file`), not in `dro_to_vgm`.
- **D-de-1b — `dro_to_vgm` stays byte-exact to `dro2vgm` (open question).**
  `dro_to_vgm`'s output is pinned byte-for-byte against the external `dro2vgm`
  fixture ([convert.rs:100](../../crates/vgms-core/src/convert.rs)), so adding a
  prime write there would break that pin — and `dro2vgm`'s own output presumably
  has the same omission. Recommendation: keep the pin, prime only in the
  playback projection, and treat "should *Convert to VGM* emit the prime, at the
  cost of reference parity?" as an **owner decision**, since it trades fidelity
  to the reference tool against musical correctness of converted files.
- **D-de-2 — accept that offline renders change at non-native rates; that is the
  point.** Exports start matching playback, honour `ResampleMode`, and honour
  the Settings core. Re-bless deliberately and list the diffs.
- **D-de-3 — freeze the reference before deleting the referencer.** Before
  `DroEngine` goes, bless a 49716 Hz `DroEngine` render of the DRO fixtures as a
  committed WAV baseline, so the post-retirement VGM-path render can still be
  A/B'd forever against it, on the parity harness's `rms_ratio` + correlation
  bars rather than byte equality (49716 is where the resampler drops out of the
  comparison).
- **D-de-4 — the "identical test-name set" acceptance rule does not apply.**
  This is a deletion, not a refactor. Replace it with: every deleted test is
  enumerated with the mechanism it tested named as gone, and every *portable*
  test (one asserting a property rather than `DroEngine`'s mechanics) is ported
  to the VGM path rather than dropped.
- **D-de-5 — collapse `RenderWavMix` to one arm.** Build `VgmRenderMix` straight
  from the panel's native `chip_muting()`/`chip_panning()`, deleting the
  reverse bridge from the render path. A down-payment on
  [DIVERGENCE §7 item 2](../dro-arm-2026-08/DIVERGENCE.md).
- **D-de-6 — keep `Muting`/`Panning` and `opl_chip_mix`.** Live playback and
  RetroWave still speak them. Retiring the vocabulary entirely is §7 item 2, a
  separate programme.

## Stages

One atomic commit per stage; the usual gates (`cargo fmt --all`, then native +
`wasm32-unknown-unknown` clippy `-D warnings`, then tests for the touched
crates). Stages 1–2 are independently shippable.

### Stage 1 — fix the DRO v1 waveform-select prime *(ships alone)*

Emit `0x01 = 0x20` at the head of `opl_song_to_vgm_file`'s projection when the
source is DRO v1. Tests: a projection-level test pinning the prime is present
for v1 and absent for v2 (the peer of `engine.rs:1203`), and a v1 case added to
`a_dro_sounds_the_same_projected_through_the_vgm_engine` — which should now pass
for v1 *because* both sides prime. Fixes live playback for every v1 capture on
native, web and hardware.

### Stage 2 — freeze the reference

Bless 49716 Hz `DroEngine` renders of the v1 and v2 fixtures as committed
baselines, with the A/B test that compares the projected VGM-path render against
them on the parity bars. This is the safety net every later stage leans on.

### Stage 3 — move the GUI task pipelines

Point [tasks.rs:282/312/349](../../crates/vgms-ui/src/tasks.rs) at the VGM path:
project the DRO once per task, then call `measure_vgm_peak_cancellable`,
`render_vgm_waveform_progressive` and `render_vgm_wav_mixed_cancellable`.
Re-bless the three PNG families. Watch for frame-count drift (below).

### Stage 4 — move the CLI render

[cli/render.rs:74](../../crates/vgms-app/src/cli/render.rs) → the same
`render_vgm_wav_cancellable` the VGM arm already uses, collapsing the match. The
DRO arm gains the resampling setting and the missing-core check it currently
skips — both audit findings, fixed for free.

### Stage 5 — collapse the mix vocabulary at the render seam

Per D-de-5: `RenderWavMix` loses its `Opl` arm;
[app/split.rs:26-38](../../crates/vgms-ui/src/app/split.rs) builds
`VgmRenderMix` directly. Delete `RenderMix` and the wire-codec half that carried
it.

### Stage 6 — delete `DroEngine` and its family

Remove `DroEngine`, the `render_dro_wav*` / `measure_dro_peak*` /
`render_dro_waveform*` families, `CoreRegistry::build_opl` /
`CoreInfo::build_opl`, `DefaultOplChip`'s engine-only role, and the dead loop
machinery. Re-bless `tests/render/*.wav` (or re-express those tests on the VGM
path — decide once Stage 3 shows how the numbers move). What is left of
`engine.rs` is the shared clock: rename it `clock.rs`, which retires the
`dro-arm` Stage 5 deferral.

### Stage 7 — restructure the orphaned tests

`opl_ab_parity.rs` loses its live reference (it becomes a comparison against
Stage 2's frozen baseline). `render_core_choice.rs` tests `build_opl` plumbing
that no longer exists and must be re-expressed against the VGM path's core
selection. Port the ~10 category-(b) property tests in `wav.rs`/`peak.rs`/
`waveform.rs`/`panning.rs`; delete the ~39 `engine.rs` mechanics tests with
their mechanism.

### Stage 8 — the docs sweep

`TERMINOLOGY.md` (the `DroEngine` entry and the "engine" collision both change
meaning — there is one engine again), tick
[DIVERGENCE §7 item 1](../dro-arm-2026-08/DIVERGENCE.md), and the stale comments
naming `DroEngine` in `vgms-cores-libvgm`, `vgms-cores-nuked`, `vgms-retrowave`,
`platform.rs` and `audio.rs`.

## Risks to measure, not assume

- **Frame-count drift.** `wav_export_of_the_fixture_is_the_right_length_and_audible`
  asserts *exactly* `2683 * 48 * 2` samples and
  `total_output_frames_matches_the_engine` asserts `ms_length * 48`. The
  projected path re-quantizes ms → samples(44100) → frames, so these may move by
  a frame or two. Measure in Stage 3; re-derive the expectation or allow a
  tolerance — do not silently loosen to "roughly".
- **The peak↔render invariant.** `peak_matches_the_wav_render_it_mirrors` asserts
  the scan equals the render's max sample exactly; both halves must move
  together in the same commit.
- **A persisted config value.** `match_volume_measures_the_peak_and_sets_the_volume`
  pins the ladder volume *derived from* the measured peak, so a peak shift can
  move a stored setting by a ladder step.
- **Corpus.** Run the corpus/parity gate before and after Stage 6
  (`VGMSTUDIO_VGMRIPS_CORPUS`); a whole-family render change deserves more than
  the fixtures.

## Acceptance

- No `DroEngine`, no `render_dro_*` / `measure_dro_peak*` family, one playback
  engine in the crate.
- Every re-blessed baseline's diff is explained in its commit message, and
  `golden_opl.rs` + `c_parity.rs` pass **untouched** throughout — proving the
  emulator did not change while the render pipeline did.
- A DRO and the same music as a VGM render through the same code, and a DRO's
  export matches its playback (the gap this programme exists to close).
- Every deleted test is accounted for per D-de-4.
