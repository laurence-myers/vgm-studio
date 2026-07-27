# PARITY-PLAN — the A/B against a reference player, made mechanical

> Written 2026-07-27, after cr-1..cr-10 shipped thirteen playable chips.
> CORES-PLAN §6.2 says a core is unverified until someone listens to it against
> VGMPlay, and that remains true — but *most of what listening would catch is
> measurable*, and every real bug this programme has shipped so far proves it:
> the flat AY notes (a pitch offset), the silent YM2203 FM (an envelope of
> zero), the untriggerable OKI voice 4 (a missing channel), the DC-blocker fixed
> points (a standing offset), the FM-to-PSG balance (a gain ratio). None of
> those needed ears. This plan turns the A/B into a harness that catches those
> classes automatically and turns the listening that remains from "audition
> thirteen chips" into "audition the five files the metrics flagged".

## 0 · The two regimes, and the control group

The comparison is not one problem but two, split by whether the reference runs
**the same upstream core** we do:

| Regime | Chips | What parity can mean |
|---|---|---|
| **Shared-core** | YM2612/YM3438 (Nuked-OPN2), YM2151 (Nuked-OPM), YM2413 (Nuked-OPLL), OPL2/OPL3 (Nuked-OPL3) — VGMPlay/libvgm offer all four Nuked cores | Near-exact: after gain normalisation and resampler tolerance, correlation ≥ ~0.99. A miss is a *driver* bug (write pacing, routing, variant flags), not a taste question. |
| **Clean-room** | SN76489, AY8910, NES APU, Game Boy, HuC6280, OKIM6295/6258, and the OPN family's SSG | Statistical: envelope, pitch, silence-agreement, per-channel stereo. Differences are expected; *systematic* differences are bugs. |

**OPL is the harness's own control group.** Our OPL core is proven
bit-identical to the C reference (`c-parity`), and VGMPlay runs that same C.
So an end-to-end OPL comparison measures only the *harness* — resampling, gain
fitting, alignment. Until OPL scores ≥ 0.99, the pipeline is what's broken,
not a chip. Build it first, calibrate on it, then point it at everything else.

## 1 · The reference player

Requirements: batch operation (file in, WAV out, no UI), deterministic output,
loop/fade control, and per-chip core selection pinnable in a config we check
in. Run as an **external process only** — the same stance as the Mesen2 and
BlastEm oracles, so its licence never touches our binary and even a GPL-3
reference would be fine.

Candidates, in order of preference:

1. **VGMPlay (Valley Bell, 0.40.x)** — the canonical player, WAV logging
   built in, per-chip core selection in `VGMPlay.ini` (including the Nuked
   family), loop count and fade time configurable. The name the acceptance bar
   has always used.
2. **libvgm's player** — the successor, same author, same core options; the
   sample player may need a small wrapper to batch-render, which if so lives
   as a *documented external tool*, never in our tree.
3. **MAME's `vgmplay` machine** (`mame vgmplay -wavwrite …`) — a genuinely
   independent implementation (ymfm-era cores), valuable later as a
   *second* opinion precisely because it shares no code with us; not the
   primary because its differences are legitimate.

**pt-1 verifies the invocation before anything is built on it** — the exact
flags are to be confirmed against the binary, not assumed. The pinned
reference version, its config file, and its SHA go in
`docs/vgm-multichip-2026-07/parity/` so a re-run years later compares against
the same thing.

> **Done, and it took more than flags.** `parity/REFERENCE.md` records what was
> pinned and the four behaviours that had to be found by experiment: VGMPlay
> reads its ini from its *own* directory (so the runner stages a private copy
> of the player), an empty `LogPath` writes the WAV *beside the input* (i.e.
> into the corpus), every `Core =` is empty and empty means the vendor default
> rather than Nuked, and determinism turns out to be a property of the chip
> rather than of the player. Every one of those would have corrupted the
> comparison silently rather than failing it.

Environment, following the corpus-test convention:

```text
DROTRIM_VGMRIPS_CORPUS  — exists already; the file source
DROTRIM_REF_PLAYER      — path to the reference executable
DROTRIM_REF_CONFIG      — the pinned settings file, staged beside a private
                          copy of the player; parity/VGMPlay.ini
DROTRIM_REF_ARGS        — optional; extra arguments before the input path
DROTRIM_PARITY_CACHE    — optional; reference WAVs are deterministic per
                          (file, rate, ref version, config), so cache them
DROTRIM_PARITY_DUMP     — optional; writes both sides of any flagged pair as
                          WAVs, which is what makes "listen to the outliers"
                          an instruction someone can actually follow
```

## 2 · Test material

Three tiers, each answering a different question:

1. **Synthetic probes** — we can *write* VGMs (the writer and the test-VGM
   builder in `vgm_engine.rs` already exist). One file per chip: a single
   sustained note at a known pitch, a volume ramp, a silence-note-silence
   gate. These give the sharpest pitch and gain measurements with no musical
   content in the way, and they are tiny and checked in.
2. **Single-chip corpus files** — drawn per chip via `ChipIndex::sample`
   (exists), filtered to files whose header declares exactly one chip, so a
   global gain fit is legitimate. ~12 per chip, 30 s each, loop count 1, no
   fade, both sides at 44100 Hz.
3. **Multi-chip corpus files** — Mega Drive (2612+PSG) and PC-88 (2203) rips,
   for the balance fit in §4. Only meaningful once tiers 1–2 pass.

## 3 · The comparison pipeline

All in a new `crates/dro-trimmer/src/parity.rs` (pure functions, hand-rolled,
unit-tested against synthetic signals — the house discipline of deriving every
number in a test applies to the metrics themselves). `hound` reads the WAVs;
no new runtime dependency, and autocorrelation covers pitch so no FFT crate is
needed unless spectral comparison earns its way in later.

Per file pair, after decoding:

1. **Trim** to the common prefix minus a 0.5 s tail (the reference may fade or
   stop at the loop point differently).
2. **High-pass** at ~20 Hz before comparing — DC policy differs by design —
   but **report DC separately**: a standing offset is the DC-blocker bug
   class and must not be filtered into invisibility.
3. **Gain-fit** a single scalar per channel by least squares
   (`α = Σxy/Σx²`). For shared-core chips α *is* the measured gain
   correction, worth reporting on its own.
4. Metrics, per channel:
   - **Correlation** at zero lag, with a ±5 ms lag search to absorb
     off-by-a-buffer alignment. The headline number.
   - **Envelope**: RMS in 50 ms windows, correlation plus mean relative
     error over windows where the reference is non-silent. Catches missing
     voices, wrong decays, the ADPCM gap.
   - **Pitch**: autocorrelation-based dominant period per window on sustained
     segments, difference in **cents**. The AY bug (period+1) was ~27 cents
     at period 64 — comfortably above a 5-cent threshold and inaudible to
     many listeners, which is exactly why this beats ears.
   - **Silence agreement**: windows the reference holds silent where we are
     loud (phantom sound) and the converse (dropouts), as a rate.
   - **Stereo**: all of the above per channel, plus an L/R-swap check —
     the Game Boy NR51 and HuC6280 balance tests depend on orientation.

Thresholds live in one table in the harness, per chip, **calibrated then
frozen**: the first run is a scorecard, the outliers get listened to (this is
where targeted listening replaces exhaustive listening), and the observed
passing band becomes the assertion. Known gaps are expected-fail entries with
a reason string — YM2610's missing ADPCM is the standing example — printed,
never silently skipped.

## 4 · The balance fit: gains stop being guesses

Every `OUTPUT_GAIN` in the cores is flagged "a balance is a listening
question". This harness answers it with arithmetic instead:

For a two-chip file, render the reference mix `R`, and our engine twice with
one core withheld each time (`VgmEngine::with_cores` — the same decomposition
`core_audio.rs` already uses): `A` = chip A solo, `B` = chip B solo. Solve

```text
min ‖ a·A + b·B − R ‖²
```

The ratio `a/b` is how far our balance is from the reference's, per file;
the median over many files is the correction. Applied, the constants become
*measured against VGMPlay r<N>'s mix, residual X%* — recorded in
`PROVENANCE.md`, replacing the standing caveat. The FM-to-PSG and FM-to-SSG
balances are the first two customers.

## 5 · Known complications, stated up front

- **The volume modifier and extra-header chip volumes.** Verified 2026-07-27:
  our engine applies *neither*; VGMPlay applies both. Until that gap is
  closed (or the reference is configured to ignore them, if it can be), the
  harness must either restrict to files where both are absent/neutral —
  cheap to filter via the header we already parse — or expect a systematic,
  explainable gain difference. Restricting is the right first move; the gap
  itself becomes a tracked engine item the harness will then *measure*.
- **Resampler smear.** Different resamplers blur transients differently;
  correlation and envelope are robust to it, sample-wise L2 is not — which is
  why L2 is not a metric.

  > **Underestimated.** Correlation is *not* robust to it: at 44100 the OPL
  > control group scored 0.836 on a core proven bit-identical to the
  > reference's. Both sides now render at the **chip's native rate**, asked per
  > file because it follows the header's clock, and the same files score
  > 0.985–0.997. `metrics::lag_drift` was added to tell the two failure modes
  > apart — an even resampler difference leaves the alignment where it was, a
  > rate difference slides it as the file plays.

- **Free-running LFOs are chip state, not pipeline error.** The OPL3's vibrato
  LFO runs from chip reset, and our side and the reference's start it at
  different points relative to the music. Files that never enable vibrato score
  0.998–0.999; one that enables it on 46% of its operator writes scores 0.590,
  with identical average pitch, level and envelope. The control group therefore
  asserts on vibrato-free files and reports the rest. Expect the same on
  YM2151's PMS/AMS and YM2612's LFO.
- **Reference determinism** is asserted, not assumed: pt-1 renders one file
  twice and requires byte-identical output before anything else is built.
- **Nuked-CQM has no reference** — VGMPlay does not ship it. It stays
  listening-only (or hardware-vs-emulation, which is its whole point).
- **Variants need explicit coverage**: YM3438 vs YM2612, YM2164, VRC VII,
  dual-chip second instances — at least one corpus file each in the sample.

## 6 · Harness form and workflow

`crates/dro-trimmer/tests/reference_parity.rs`, shaped exactly like
`core_audio.rs`: `#[ignore]`, env-gated, prints a per-chip table, asserts the
frozen thresholds. Not CI — the documented run the plan's §6.3 always
intended, executed:

- after any submodule pin bump (the whole point of the submodule policy),
- after any core or gain change,
- before a release.

The printed table diff is the review artifact. This same machinery is what
cr-11's LLE oracles slot into later — an LLE render is just another
"reference exe", so building this pays twice.

## 7 · Steps

| Step | Contents | Acceptance |
|---|---|---|
| pt-1 | Choose and pin the reference: verify batch invocation, loop/fade control, per-chip core config; check in the config + version record; determinism check | The same file renders byte-identically twice |
| pt-2 | `parity.rs` metrics with self-tests: correlation, gain fit, envelope, cents-accurate pitch on synthetic sines (detune a sine 10 cents, measure 10±1) | Metric unit tests green |
| pt-3 | Harness + the **OPL control group** | OPL correlation ≥ 0.99 after gain fit, or the pipeline is fixed until it is |
| pt-4 | Shared-core chips (YM2612, YM2151, YM2413), strict thresholds; variant files included | Calibrated, frozen, green |
| pt-5 | Clean-room chips: scorecard run, targeted listening on flagged outliers, freeze; known-gaps list started (YM2610 ADPCM) | Thresholds frozen; every expected-fail has a reason |

> **pt-4/pt-5 status: the scorecard runs and has been read; the thresholds are
> deliberately *not* frozen.** `parity/SCORECARD.md` is the table. YM2151
> passes. Three real faults were found and fixed in the SN76489 alone, and the
> outliers needed no listening — the numbers named them. But freezing a bar at
> what a chip currently scores, when three chips are all but silent and the
> leading suspect is our own resampler, would retire the harness from the job
> it was built for. The bars stay where correctness says they belong and the
> table stays red until the open findings in `SCORECARD.md` are closed.
| pt-6 | Balance fit (§4) on multi-chip files; update `OUTPUT_GAIN`s; `PROVENANCE.md` records the fit; re-run the corpus audibility suite | Gains documented as measured; residuals reported |

## 8 · What this still cannot replace

A correlation of 0.93 on a clean-room chip does not say *which* side is more
faithful to the silicon — the reference's MAME-derived cores are themselves
approximations. The harness proves we match the accepted player within a
frozen band and pins us there against regression; where we deliberately differ
(or want to beat the reference), the judgement is human, the LLE tier is the
higher court, and hardware is the supreme one. The win is that human ears now
get pointed at the residual, not at everything.
