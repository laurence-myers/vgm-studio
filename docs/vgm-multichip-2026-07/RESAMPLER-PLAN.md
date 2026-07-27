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

**Corrected after measuring.** This section first claimed the worst case was
the NES APU at 1.789 MHz (ratio 40.6, ~1300 taps, "a few percent of one core").
Both halves were wrong. The NES core averages 32 CPU cycles into each sample
and so presents 55.9 kHz, and the HuC6280 divides by 64 — that ratio does not
exist. And when measured, the hypothetical 40:1 ran at **1.1× realtime**: one
voice would have eaten a core.

The real worst case is the SN76489 and AY8910 at 223721 → 44100 (ratio 5.07):
183 taps, measured at **14.6× realtime** once the inner loop was written to
walk the kernel rather than recompute its index per tap. The guard test is
pinned there. The escape hatches — a half-band pre-decimation cascade, or
blip-style band-limited step synthesis inside individual cores — stay unbuilt,
because nothing now needs them.

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

**rs-t3 · The parity harness was to be the acceptance bar, and cannot be.**
Recorded here because the reasoning took two attempts to get right.

The plan was: re-point the scorecard at 44100 and require the SN76489 to move
decisively toward its native-rate score. Run, and it went 0.5848 → 0.6310 —
nowhere near. The bar was ill-founded. VGMPlay's `ResamplingMode` offers linear
interpolation, nearest-neighbour, or a mixture; there is no band-limited
option, and with `ChipSmplMode = 3` it runs an SN76489 at 223721 Hz and
linearly interpolates down to 44100 — the exact fault this branch removes. At
44100 the reference is now the aliased one, and scoring well against it would
mean having its artefacts back.

The second attempt: take the reference's *native* render, put it through our
filter, and require our 44100 render to agree with it as closely as the two
agree at native rate. Also unsound — a filter removes content from both sides,
so where two cores agree above 19.8 kHz and differ below, filtering strips the
agreeing part and correlation falls for reasons that have nothing to do with
the filter being right. Measured losses ran −0.007 to +0.345 over six files.

So the acceptance is:
1. **`resample.rs`'s own tests**, which measure the filter directly against
   signals whose answers are known by construction. That is the real evidence
   and it is falsifiable.
2. **The native-rate scorecard must not move** — the resampler is out of that
   path, so any change there is leakage.
3. Left open: the **synthetic probes** of PARITY-PLAN §2, never built. A
   written-by-us VGM playing one high tone gives a render that must have energy
   at exactly one frequency, and an aliasing player one that has energy at a
   second, computable frequency. That would close it against the reference
   properly.
The OPL control group re-run seals it: unchanged at native rate.

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

## Postscript: the full-size control run

The native-rate scorecard at twelve files per chip — the control rs-t3 asked
for — matches the old 44100 lerp table almost chip for chip (SN76489 0.5855
native against 0.5844; full table in `parity/SCORECARD.md`). Which settles it:
**the resampler was never a material factor in the parity scores.** The header
of this plan, written from a ratio table with wrong rates and a two-file
native experiment, said otherwise; both inputs are corrected in place above and
in SCORECARD.md.

What the branch delivered is exactly what `resample.rs`'s own tests measure —
aliasing to below −114 dB, alignment, exact DC, 8–15× realtime — an audio
fidelity fix for playback and WAV export. The parity gaps it was hoped to close
belong to the cores, and the per-chip investigations inherit them.
