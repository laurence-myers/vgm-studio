# RESAMPLER-PLAN — a decimation filter for `VgmEngine`, measured into place

> Written 2026-07-27, the day the parity scorecard attributed most of its red
> to one function. `Voice::next_frame` in `dro-synth/src/vgm_engine.rs`
> interpolates linearly between point-sampled source frames — no anti-alias
> filter — so a chip rendering at 223721 Hz reaches a 44100 Hz output with
> everything above 22 kHz folded back into the audible band. Measured cost:
> SN76489 correlates 0.5848 against VGMPlay at 44100 and 0.9958 at its own
> rate; YM2612 0.9538 against 0.9949. This is audible in normal playback and
> WAV export for every non-OPL chip; the OPL path renders at its own rate and
> is untouched.

## 0 · What "accurate" means here, exactly

The original systems put the chip's DAC through analogue output stages and a
speaker; no 44100 Hz stream is that. The defensible definition — and the one
the reference player's best cores implement — is:

**the output must be what an ideal band-limited capture of the chip's output
pin would record at the output rate.**

That is a mathematical statement, not a taste: band-limit the native-rate
signal to the output Nyquist, then sample. Everything below ~20 kHz survives
untouched; everything above must not reappear as aliases. Two consequences:

- **Aliasing is the entire fault class.** Square waves and PCM steps are rich
  above 22 kHz; today all of it lands back in-band as inharmonic tones. A
  proper decimator removes it. Nothing else about the sound may change:
  passband flat, DC preserved, pitch exact.
- **Per-system analogue colour (RC filters, amp rolloff) is out of scope** —
  deliberately. It varies per console revision, the reference does not model
  it either, and it belongs to a future "tone" feature, not to correctness.
  The plan's accuracy bar is the ideal capture, which is also the only bar a
  correlation against VGMPlay can verify.

Why VGMPlay disagrees with our lerp today, for the record: its non-FM cores
(e.g. the Maxim SN76489) step the chip at native rate and **average** the
output over each output sample — a boxcar decimator. Crude, but a real filter.
A windowed-sinc decimator is strictly better than the reference on this axis;
the residual A/B difference will be the reference's own boxcar rolloff.

## 1 · The design

One algorithm, all ratios: a **polyphase windowed-sinc resampler**, kernel
scaled to the lower of the two Nyquists. This is the textbook decimator
(speexdsp/libsamplerate shape), hand-rolled in the house style — no new
dependency, every constant derived in a test, wasm-clean.

### The kernel

- Prototype: `h(t) = sinc(t) · kaiser(β, t/ZC)` with **ZC = 16** zero
  crossings per side and **β ≈ 10** — stopband rejection beyond −90 dB,
  passband ripple far below audibility.
- Cutoff at **0.45 × min(native, output)** rate: for decimation the sinc is
  stretched by the ratio so its cutoff sits below the *output* Nyquist; for
  upsampling (YM2203 at 41667 → 44100, OKIM6295 at ~8 kHz) it sits below the
  *source* Nyquist and the same code interpolates.
- Stored once as a `static` table: the half-kernel sampled at **256 points per
  zero-crossing interval** with linear interpolation between entries —
  interpolation error below −100 dB, table ~16 KB of `f32`, shared by every
  voice at every ratio because scaling happens at index time.

### The inner loop

Per output frame: `y = Σ x[n−k] · h((k − phase) / ratio) / norm` over the
`⌈2·ZC·max(1, ratio)⌉` source frames the stretched kernel spans.

- **Phase** advances by the exact rational step the current fixed-point code
  already uses (64-bit accumulator, `FRAC_BITS = 32` now — 16 bits of phase
  is too coarse for a 1280-tap kernel).
- **`norm` is the actual tap sum at this phase**, computed alongside the dot
  product. This pins DC gain to exactly 1.0 at every phase; a fixed norm
  would modulate amplitude at the beat between the rates, which is precisely
  the kind of artefact this programme has learned to expect to be real.
- Accumulate in `f64`, kernel in `f32`, in/out stays `i32`: with ~1300 taps
  the accumulation error budget is comfortable and the cores' contract does
  not change.
- History is a power-of-two ring sized at init from the ratio. Pull-one-frame
  stays the discipline: the ring is filled by pulling the core exactly as
  `next_frame` does today, so **chunk invariance is structural**, not tested
  into existence.

### Cost, bounded up front

Worst case is the NES APU at 1.789 MHz → 44100 (ratio 40.6): ~1300 taps × 2
channels × 44100 ≈ 115 M multiply-adds/s. Fine in release (a few percent of
one core); the plan includes a realtime-factor guard test so a regression is
a red test, not a stutter report. If profiling ever demands it, the escape
hatches are (a) a half-band pre-decimation cascade or (b) blip-style
band-limited step synthesis inside individual cores — both explicitly *not*
now, because one code path that is provably right beats two that are fast.

### What it replaces, and where state lives

`Voice { step, position, prev, next }` becomes `Voice { resampler }` with the
ring, phase and ratio inside a new `dro-synth/src/resample.rs`. The reset
points are exactly today's: construction, `rewind`, and `seek_to_row` — the
same places `core.reset` + `core.configure` already run, so a rewound engine
is the same machine it was. When `native == output` the resampler is an exact
identity (asserted bit-for-bit), so OPL-adjacent paths that happen to match
rates lose nothing.

## 2 · Verification — the part that makes it *accurate* rather than *plausible*

Three layers, cheapest first. The metrics need no FFT crate: a hand-rolled
Goertzel (one filter per probed frequency, ~10 lines, self-tested) measures
single tones exactly, which is all these tests need.

**rs-t1 · Unit tests against constructed signals** (in `resample.rs`):

| Test | Signal | Assertion |
|---|---|---|
| Passband flatness | sines at 1 k / 10 k / 15 kHz through 223721→44100 | amplitude within 0.1 dB of input |
| Alias rejection | sine at 30 kHz (inaudible at source, folds to 14.1 kHz) | Goertzel at 14.1 kHz ≤ −80 dB |
| The actual fault | 1 kHz square at 223721→44100 | every non-harmonic tone ≤ −60 dB; the lerp scores ~−20 dB here, so the test fails loudly on the old code |
| DC exactness | constant input | identical constant out, every ratio |
| Pitch exactness | sine in, detune_cents against ideal | 0.0 cents — reuse the parity metric |
| Identity | ratio 1.0 | bit-identical passthrough |
| Upsampling | 41667→44100, 8000→44100 | flat passband, no images above source Nyquist |
| Chunk invariance | 128-frame pulls vs 4096 | identical output |
| Determinism | twice through | identical output |

**rs-t2 · Engine-level**: the existing `VgmEngine` suite must stay green
(routing, seeks, loop seams — none of it may notice the swap), plus the
corpus audibility run (13 chips × 12 files) to prove nothing went quiet.

**rs-t3 · The parity harness is the acceptance bar** — this is why it was
built first. Two runs:
1. **Scorecard at 44100** (temporarily re-pointed): SN76489 must move
   decisively toward its native-rate 0.9958. It will not reach it exactly —
   the residual is the reference's boxcar versus our sinc, an explainable,
   frequency-shaped difference — so the bar is ≥ 0.95 with the residual
   written down in SCORECARD.md.
2. **Scorecard at native rates**: must not move at all (the resampler is out
   of that path). This is the control: if it shifts, the change leaked
   somewhere it should not be.

The OPL control group re-run seals it: unchanged at native rate, and the
`lag_drift` diagnostic must show no drift introduced at 44100.

## 3 · Steps

| Step | Contents | Acceptance |
|---|---|---|
| rs-1 | `resample.rs`: kernel table, phase accumulator, ring, identity path; rs-t1 tests | Unit suite green; the square-wave test demonstrably fails against the old lerp |
| rs-2 | Swap into `Voice`; reset wiring at rewind/seek; rs-t2 | Engine suite + corpus audibility green |
| rs-3 | rs-t3 parity runs; update SCORECARD.md before/after; realtime-factor guard for the NES-APU worst case | 44100 scorecard moves as predicted; native unchanged; realtime factor recorded |
| rs-4 | Unblocks: re-run pt-6's balance fit (a Mega Drive file mixes 53 kHz and 224 kHz chips, so the fit was measuring this bug); then freeze the pt-4/pt-5 thresholds the resampler was blocking | pt-6 numbers recorded in PROVENANCE.md; thresholds frozen with reasons |

## 4 · Explicitly out of scope

- **Per-system analogue filter modelling** — a future, optional tone layer.
- **blip-style synthesis in cores** — an optimisation to reach for only if
  the realtime guard ever fails.
- **The three near-silent PCM chips** (OKIM6258/6295, HuC6280) — a different
  fault; no filter fixes rendering nothing. Tracked separately (task #16).
- **YM2413's 0.956** — barely moves at native rate, so it is not this bug
  either.
