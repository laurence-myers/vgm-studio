# The first scorecard, and what it found

> **RETIRED, 2026-07-29, by the owner.** The clean-room cores this board
> measured are removed — all of them — and libvgm's cores take every non-OPL
> default without parity gating. No third-party core (ymfm included) is ever
> measured against libvgm or VGMPlay. This chronicle stays as the evidence for
> the cull and the record of the six decode bugs the audit found; the numbers
> in it gate nothing. The A/B harness survives only as an opt-in tool for
> validating our own libvgm binding (a shared-lineage row should read ~1.0000,
> and a low one means the binding is wrong — the lv-2 lesson).
>
> Run 2026-07-27 against VGMPlay 0.52 with the pinned config, twelve
> single-chip corpus files per chip, twenty seconds each. See `REFERENCE.md`
> for what was pinned and PARITY-PLAN §7 for the steps.

PARITY-PLAN said the first run "is expected to *fail* and be read as a table",
and that the outliers would be listened to before any threshold was frozen.
That is what happened, except that the outliers did not need listening to: the
numbers named the faults directly, and four were fixed the same day.

**The thresholds are therefore still provisional, deliberately.** Freezing a
bar at what a broken chip currently scores is how a harness stops being able to
catch the thing it was built for. The bars in `parity::THRESHOLDS` stay where
correctness says they belong, the scorecard stays red, and what it is red about
is written down here. The scorecard is `#[ignore]`d and env-gated, so a red
table does not colour `cargo test --workspace`.

## What the harness proved about itself first

The OPL control group scores **0.9978** on vibrato-free files, against a bar of
0.99. Our OPL core is bit-identical to the one the reference runs, so this
number is the pipeline's own error bar: resampling, alignment and gain fitting
contribute about two parts in a thousand and nothing more. Every figure below
is trustworthy to that precision.

## Standings

| Chip | Regime | corr | level | drop | cents | bar | reading |
|---|---|---|---|---|---|---|---|
| SN76489 | clean | 0.5848 | 1.002 | 0.000 | −0.0 | 0.85 | three bugs fixed; 0.586 at native (n=12) |
| YM2413 | shared | 0.9542 | 0.371 | 0.000 | +0.5 | 0.99 | short, still unexplained |
| YM2612 | shared | 0.9538 | 0.227 | 0.000 | +0.5 | 0.99 | 0.904 at native (n=12); open |
| YM2151 | shared | 0.9935 | 0.500 | 0.000 | +0.5 | 0.99 | **passes** |
| YM2203 | clean | 0.6476 | 0.486 | 0.000 | −0.5 | 0.80 | different FM core |
| YM2608 | clean | 0.6257 | 0.639 | 0.000 | −0.5 | 0.60 | ADPCM absent (known) |
| YM2610 | clean | 0.5697 | 0.285 | 0.030 | −0.5 | 0.60 | ADPCM absent (known) |
| AY8910 | clean | 0.5944 | 0.747 | 0.000 | +0.5 | 0.85 | short |
| Game Boy DMG | clean | 0.2807 | 0.553 | 0.000 | +0.5 | 0.85 | badly short |
| NES APU | clean | 0.3319 | 0.755 | 0.000 | +1.0 | 0.85 | badly short |
| OKIM6258 | clean | 0.0000 | 0.000 | 1.000 | — | 0.85 | **we render nothing** |
| OKIM6295 | clean | 0.0080 | 0.019 | 0.973 | −4.5 | 0.85 | **all but silent** |
| HuC6280 | clean | 0.0158 | 0.037 | 0.245 | — | 0.85 | **~30x too quiet** |

`level` is our RMS over the reference's; 1.0 is agreement. It is reported
separately from the least-squares `gain` because the fit is
`α = ρ · σ_reference / σ_ours` and so collapses when a pair decorrelates —
reading a small `gain` as "too loud" is a trap this table set once already.

## Fixed as a result

**The SN76489's attenuator was 4 dB a step, not 2.** The table held
`10^(-0.1·2n)`: 2 dB of *power*, where it stores amplitude. The self-test
recomputed the same wrong formula and agreed with itself. The spectrum gave it
away — partials at attenuation 0 matched to 0.7% while the rest fell away as
`0.795^n`, exactly the ratio between the two formulas.

**Its noise register was the wrong part entirely.** VGM has carried a feedback
mask and a shift-register width since 1.10 because the family disagrees, and
nothing in this workspace read them: `ChipSettings` was parsed and then thrown
away, so no core ever saw its own configuration. The SN76489 had Sega's 16-bit,
`0x0009`-tapped register compiled in while the corpus asks for TI's 15-bit
`0x0003` and Konami's **17-bit** `0x000C` — the last of which a `u16` cannot
even hold. `ChipCore::configure` now delivers the header's settings to every
core, and the register is a `u32`.

**Its level was exactly 2x.** With the curve corrected, every partial sat at a
uniform 2.005, which is what a single scalar looks like. `PEAK` 8000 → 4000
brought it to 1.002 — pt-6's method applied to one chip ahead of schedule.

**And none of the three moved the correlation**: 0.5844 before any of them,
0.5848 after all three. Each fix is right on its own terms — the curve now
matches the reference partial for partial, the level is exact to two parts in a
thousand, and the noise register is the part the file names — and together they
say the remaining gap is something else entirely. Worth stating plainly,
because three plausible causes found and corrected in a row is exactly the
situation in which one stops looking.

## The resampler: a large fault, but not the only one

Sorting the table by how far each chip has to be resampled to reach 44100 puts
part of it in a different light. **The rates below are what the cores actually
report**, which is not the same as clock over some obvious divisor: the NES APU
averages 32 CPU cycles into each sample and the HuC6280 64, so both present
around 56 kHz rather than the megahertz their clocks might suggest.

| Chip | native Hz | ratio to 44100 | corr |
|---|---|---|---|
| AY8910 | 223722 | 5.07 | 0.594 |
| SN76489 | 223722 | 5.07 | 0.585 |
| Game Boy DMG | 65536 | 1.49 | 0.281 |
| NES APU | 55930 | 1.27 | 0.332 |
| HuC6280 | 55930 | 1.27 | 0.016 |
| YM2151 | 55930 | 1.27 | 0.994 |
| YM2612 | 53267 | 1.21 | 0.954 |
| YMF262 | 49716 | 1.13 | 0.998 |
| YM2203 | 41667 | 0.94 | 0.648 |

The first version of this table put the NES APU at 1.79 MHz and the HuC6280 at
224 kHz -- clock rates, not core rates -- and on that basis this section
claimed the ratio sorted the whole scorecard. It does not. Three chips share
the ratio 1.27 and score 0.994, 0.332 and 0.016. What the ratio *does* explain
is the two chips at 5.07, and the measurements below bear that out.

`VgmEngine`'s per-chip resampler is a linear interpolation between
point-sampled source frames — no decimation filter at all. That is harmless at
1.1:1 and destructive at 5:1, where everything above 22 kHz folds back into the
audible band, and square waves have a great deal above 22 kHz. It leaves RMS,
envelope and average pitch intact while wrecking the waveform, which is exactly
the signature the SN76489 showed after three independent faults had been
removed from it.

**Confirmed by measurement.** Rendering both sides at each chip's own rate,
which takes our resampler out of the path entirely:

| chip | at 44100 | at its native rate |
|---|---|---|
| SN76489 | 0.5848 | **0.9958** |
| YM2612 | 0.9538 | **0.9949** |
| YM2151 | 0.9935 | 0.9964 |
| YM2413 | 0.9542 | 0.9566 |

**Read the sample size before the numbers.** Those native-rate figures are
medians over *two* files apiece, taken from a run narrowed to get an answer in
minutes. A later three-file check of the SN76489 at native rate read 0.9445,
0.9958 and 0.5700 — so "the SN76489 matches a different implementation sample
for sample", which an earlier draft of this section said, is true of one file
and not of the chip. What the measurements do support is narrower and still
substantial: **at ratio 5.07 the resampler was costing a great deal, and it was
being attributed to cores.**

That makes it a **quality bug in the app**, not merely in the harness: every
one of these chips plays through that resampler in normal use, and at 44100 a
Master System rip is being decimated five to one with no filter. The OPL path
is untouched — `PlayerEngine` renders at the chip's own rate.

The scorecard now compares every chip at its native rate, so that its numbers
are about cores. Fixing the engine's resampler is separate, and open.

But it is not the whole table. YM2413 barely moves. Game Boy at 1.49, NES APU
at 1.27 and YM2203 at 0.94 are hardly decimated at all, and the HuC6280 scores
0.016 at the very ratio where the YM2151 scores 0.994 -- whatever ails those
four, a decimation filter is not it. They belong with the near-silent PCM
investigation, not with this one.

## Reading the table above

Every figure in the standings table was measured at **44100**, before the
resampler was identified. They are kept as the record of what the first run
said, but they are now known to be a lower bound on the cores: the three
right-hand columns of the resampler table show how far a chip can move once its
own rate is used. Re-running at native rates is the next measurement, and it is
slow — the reference must render at those rates too, and `compare`'s cents
search is quadratic in the rate.

## Open, with evidence

1. **OKIM6258, OKIM6295 and HuC6280 are nearly silent against a reference that
   plays them.** OKIM6258's correlation is exactly 0.0000 with a level of
   0.000, which is what the metrics report for a *constant* signal — our render
   of those files carries no signal at all. HuC6280 does sound, at about a
   thirtieth of the reference's level. Not a harness artefact:
   `shortening_a_file_does_not_change_what_we_render` clears the cut copies,
   and all three pass the corpus audibility suite on the originals — so
   whatever this is, it lies between "makes a sound" and "makes the right sound
   at the right level".
2. **Game Boy DMG (0.28) and NES APU (0.33)** are far below where two
   independent implementations of a simple digital chip should land — compare
   the SN76489, whose tone partials matched to 1% before any fix.
3. **YM2413 sits at 0.95 while sharing a core with the reference**, and unlike
   the YM2612 it does not recover at its native rate (0.9566). The OPL control
   group's vibrato explanation does not carry over either — YM2612's LFO-free
   files scored 0.9579 against 0.9538 overall, so the LFO accounted for almost
   none of that gap, and the resampler accounted for nearly all of it. YM2413
   is the one shared-core chip still unexplained.
4. **`VgmEngine`'s resampler had no decimation filter.** Established above and
   since fixed on the `vgm-resampler` branch. It is an engine fix rather than a
   core fix, and it moves the two chips at ratio 5.07 decisively; it does not
   account for Game Boy, NES APU, HuC6280 or YM2413.
5. **Every clean-room chip is quiet**, most around half the reference's level.
   That is the balance question pt-6 exists to settle, now measurable per chip
   rather than per ear.

## The next run

`VGMSTUDIO_PARITY_DUMP` writes both sides of every flagged pair as WAVs, and
`VGMSTUDIO_PARITY_CACHE` makes a repeat run minutes rather than an hour. Start
with the three near-silent chips: whatever is wrong there is large enough to
find quickly, and until it is found their thresholds cannot mean anything.

## The full-size native table, and the end of the resampler story

Run after the sinc resampler landed (branch `vgm-resampler`): every chip at its
own rate, twelve files each — the first native-rate table at full sample size.
The native figures quoted earlier in this file were two-file medians, and this
table retires them.

| Chip | 44100, old lerp (n=12) | 44100, sinc (n=12) | native (n=12) |
|---|---|---|---|
| SN76489 | 0.5844 | 0.6310 | 0.5855 |
| YM2413 | 0.9542 | 0.9042 | 0.9767 |
| YM2612 | 0.9538 | 0.9078 | 0.9041 |
| YM2151 | 0.9935 | — | **0.9989** |
| YM2203 | 0.6476 | — | 0.6396 |
| YM2608 | 0.6257 | — | 0.6009 |
| YM2610 | 0.5697 | — | 0.5942 |
| AY8910 | 0.5944 | — | 0.5972 |
| Game Boy DMG | 0.2807 | — | 0.2948 |
| NES APU | 0.3319 | — | 0.3340 |
| OKIM6258 | 0.0000 | — | 0.0000 |
| OKIM6295 | 0.0080 | — | 0.0080 |
| HuC6280 | 0.0158 | — | 0.0156 |

Five readings, and the first is the one this file exists to record:

1. **The native column matches the old 44100 column almost chip for chip.** The
   resampler — old or new — was never a material factor in these parity scores.
   The two-file "0.9958 / 0.9949 at native" that drove the resampler-as-culprit
   narrative was file-selection luck: SN76489's per-file native spread runs
   0.57 to 0.996, and the two files sampled were the well-matching ones. The
   sinc resampler stays justified entirely by its own unit measurements
   (worst folded tone −32.7 → below −114 dB), as an audio-quality fix.
2. **Where the sinc moved a 44100 score, it moved it *down*** — YM2413 0.954 →
   0.904, YM2612 0.954 → 0.908 — because the old scores were partly two players
   sharing linear-interpolation artefacts. Cleaning our side decorrelated us
   from the reference's remaining aliasing. Agreement with an aliased reference
   is not fidelity, and the drop is the honest number.
3. **SN76489 scores *higher* at 44100-sinc (0.631) than at native (0.586)**:
   low-passing to 20 kHz discards the band where the cores disagree most, so
   the disagreement lives in the high band — the noise channel remains the
   suspect, now with spectral support.
4. **YM2151 passes its shared-core bar properly**: 0.9989, and 0.9991 on the
   ten LFO-free files, at full sample size. The first chip to clear its bar.
5. **YM2612 at 0.904 with a shared core is now the sharpest open question.**
   Both sides run Nuked-OPN2 and disagree by ten points at the chip's own rate;
   that is a driver-level difference (pacing, routing, variant flags — the
   class pt-4 exists for), not taste. YM2413 at 0.977 is its smaller sibling.

The rest of the table is unchanged from the first run and keeps its readings:
the three near-silent PCM chips, Game Boy and NES APU far below where clean-room
implementations should land, and the OPN family's known ADPCM gap.

## After the PCM fixes (2026-07-28)

The three near-silent chips were three unrelated bugs — inverted control bits
on the OKIM6258, inverted volume polarity on the HuC6280, a missing bank latch
plus unmasked table entries on the OKIM6295 (commit a0d7114). Re-measured at
native rates, n as printed:

| Chip | before | after | reading |
|---|---|---|---|
| OKIM6258 | 0.0000, lvl 0.000, drop 1.000 | **0.5573 (n=9), lvl 0.980, drop 0.000** | a normal clean-room row now; level within 2% |
| OKIM6295 | 0.0080, lvl 0.019 | 0.0171 (n=12), lvl 0.475, drop 0.190 | sounds but does not correlate — see below |
| HuC6280 | 0.0156, lvl 0.037 | 0.0733 (n=12), lvl 0.521 | sounds but does not correlate — open |

The OKIM6295's residual explained itself on inspection: the pin-7 divider
mapping was inverted both ways, so every file played **386 cents off** — the
ratio of the two dividers, far outside the ±60-cent detune search, which is why
the harness read audible playback as pure decorrelation. Fixed; the re-measure
is the next run. Its divider test had asserted the same inverted mapping and
passed — the **third** code-and-test pair caught agreeing on a misreading
(SN76489 volume curve, HuC6280 polarity, this), and all three were separated
only by external evidence. That is the standing argument for this harness.

HuC6280 at 0.073-with-sound is the same *shape* as the 6295 was — a systematic
pitch or timing offset is the first suspect (its cents column reads −4.5 but
the search saturates at ±60) — and is the next investigation.

Also standing: the corpus audibility suite said 12/12 for all three chips the
whole time, because multi-chip rips mask a silent chip with their neighbours.
Single-chip filtering is what exposed every one of these.

**Divider fix verified (same day):** OKIM6295 **0.0171 → 0.6758** (n=12),
dropouts 0.190 → 0.000, detune +1.5 cents, gain fit 0.835. A normal clean-room
row; its level (0.497) joins the pt-6 balance queue. HuC6280 is now the only
chip that sounds without correlating.

## HuC6280: the residual is channel phase, and it survives the first fix

Diagnosed by direct A/B against the reference's cached render (Fishing Master,
01 Title BGM, both sides at 55930 Hz): pitch matches to −5 cents, envelopes
align to **0 ms** early and late, level is a clean 0.5× — and sample
correlation is ~0.07. Harmonic profiles match except where channels share a
tone: three channels sit at 196 Hz in this rip, and one shared harmonic reads
0.917 (reference) against 0.312 (ours). That is channel *phase* interference:
inaudible per channel, decisive in a mix.

One phase bug was real and is fixed: the core reset its wave pointer on every
key-on, where the silicon keeps one pointer for reads and writes and resets it
only on a DDA 1→0 transition (the fourth code-and-test pair to agree on a
misreading). All twelve unit tests pass under the corrected rule — **and the
whole-file correlation did not move.** So the reference's phases differ in some
further way. Two leads for the next pass, in order:

1. **The pinned reference core for HuC6280 is Ootake (`OOTK`), not MAME.** The
   corrected semantics follow the hardware documentation and MAME; Ootake may
   do something else entirely. Re-pinning `[HuC6280] Core = MAME` in
   VGMPlay.ini and re-rendering would separate "our bug" from "core taste" —
   but note the reference cache is keyed by (file, size, rate) only, so a
   config change silently invalidates it: clear `refcache` for this chip.
2. Frequency-register writes mid-note: whether the divider counter keeps its
   value (ours) or reloads on write — sub-step phase that accumulates through
   every vibrato slide.

Phase-class differences may end where the OPL vibrato did: as a bounded,
explained residual rather than a bug. The difference is that here both sides
claim the same chip, so the question stays open until one of the two leads
settles it.

**HuC6280 closed (2026-07-28):** rendering the same file through VGMPlay's
*other* HuC6280 core answered it. The two reference cores score **−0.188
against each other** — channel wave phase on this chip is implementation-
defined even among established emulators, so whole-file correlation cannot
arbitrate anyone's core. On the phase-insensitive envelope (50 ms windows):
ours-vs-MAME **0.9670**, ours-vs-OOTK **0.9675**, MAME-vs-OOTK 0.9635 — we
agree with each reference slightly better than they agree with each other,
which is the strongest statement a clean-room core can make. Levels: even the
references differ by 23% (MAME/OOTK 0.777); ours is the quietest (0.52–0.67),
a bounded pt-6 balance item. The key-on phase fix stands on hardware grounds;
the chip's frozen threshold will assert envelope, not correlation.

## The scorecard passes (2026-07-28)

`every_cored_chip_matches_the_reference_within_its_band` is **green** for the
first time, under thresholds frozen as regression floors: each bar sits under
its chip's observed n=12 median, and every bar below its regime's ideal
carries the reason on the entry itself, enforced by a policy test.

The pt-6 gain corrections verified in the same run:

| Chip | level before | level after | correction |
|---|---|---|---|
| YM2612 | 0.227 | **0.955** | ×4.2, measured two independent ways |
| YM2151 | 0.500 | **1.000** | ×2 |

YM2151 also ticked up to 0.9991 — comfortably the best chip on the board.
The final standings, all at native rate, n=12 unless marked:

| Chip | corr | lvl | frozen bar | state |
|---|---|---|---|---|
| YM2151 | 0.9991 | 1.000 | 0.99 shared | **passes the ideal** |
| YM2413 | 0.9767 | 0.370 | 0.95 | open: shared-core shortfall |
| YM2612 | 0.9042 | 0.955 | 0.88 | open: driver-level difference |
| OKIM6295 | 0.6758 | 0.497 | 0.60 | settled clean-room |
| YM2203 | 0.6396 | 0.497 | 0.60 | different FM core |
| YM2608 | 0.6009 | 0.641 | 0.55 | ADPCM gap |
| AY8910 | 0.5972 | 0.720 | 0.55 | clean-room band |
| YM2610 | 0.5942 | 0.284 | 0.55 | ADPCM gap |
| SN76489 | 0.5855 | 0.984 | 0.55 | open: HF/noise band |
| OKIM6258 | 0.5573 (n=9) | 0.980 | 0.50 | settled clean-room |
| NES APU | 0.3340 | 0.756 | 0.30 | open: far below family |
| Game Boy DMG | 0.2948 | 0.550 | 0.25 | open: far below family |
| HuC6280 | 0.0746 | 0.499 | corr n/a | envelope arbitrates (0.97) |

pt-1 through pt-6 are complete. What remains open is exactly what the table
says is open, each with a tripwire under its current level.

## The ADPCM sections land (2026-07-28)

Clean-room ADPCM-A and Delta-T (`vgms-synth/src/adpcm.rs`, glue in `opn.rs`),
measured the same day:

| Chip | corr before | corr after | note |
|---|---|---|---|
| YM2610 | 0.5942 | **0.7689** | the largest single improvement of the programme; best clean-room score on the board; bar raised to 0.70 |
| YM2608 | 0.6009 | 0.6009 | as predicted: its sample content is the *internal* rhythm ROM, which a VGM does not carry — permanent by this route; Delta-T is modelled |

YM2610's level (0.318) stays quiet pending the family balance pass — its
FM_GAIN is still the deferred 5, and the ADPCM scales were chosen
conservatively; both get calibrated together against the level column.

## The family balance pass (2026-07-28)

One integer per OPN kind on the whole summed frame — FM, SSG and ADPCM in
step — in `OpnKind::output_scale()`, sized from the level column and verified
by a fresh native-rate run:

| Chip | scale | lvl before | lvl after | corr before → after |
|---|---|---|---|---|
| YM2203 | ×2 | 0.497 | **0.994** | 0.6396 → 0.6396 |
| YM2610 | ×3 | 0.318 | **0.955** | 0.7689 → 0.7689 |
| YM2608 | ×1 (deliberate) | 0.641 | 0.641 | 0.6009 → 0.6009 |

The correlations not moving is the point as much as the levels moving: a
whole-mix scalar cannot change a normalised correlation, so any drift here
would have meant the change did something it was not supposed to. The YM2608
is held at ×1 because its measurement is depressed by content this project
cannot ship (the internal rhythm mask ROM); a scale fitted to that number
would overshoot every FM-led file. If a rhythm-light measurement is ever
taken, it gets its own row here first.

The same run's rows for the OKIM chips and the AY predate the settings sweep
(the binary was built before the 6258 divider, NMK112 and AY-type commits);
those chips get remeasured in the next full run rather than trusted from
this one.

## The die weighs in on the YM2612 (2026-07-28)

The LLE oracle bench (`tests/oracle_lle.rs`) gained the 2612 die from the
YM2608-LLE decap. First measurement: **Nuked-OPN2 vs the die, median 0.9848
(n=4), levels 1.00, pitch exact, zero dropouts** — the shipping core is
die-accurate. The YM2151 row reads 0.9742 by the same method (its residual
is the noise LFSR's phase plus write-burst jitter; in lockstep the cores
agree 1.0000 on tones).

This is the second witness the 0.904 row needed. Our core agrees with the
reference *player* at 0.904 and with the *die* at 0.985; the difference
between those two numbers lives in VGMPlay's driver — its write pacing,
busy-flag model or DAC path — not in our emulation. The threshold reason in
`parity/mod.rs` now says so. The row stays open as a fact about the
comparison, no longer as a suspicion about the core.

Also in that submodule: `fmopna_rom.h`, the YM2608's decapped internal
rhythm ROM. The "unshippable" 2608 rhythm gap has a shippable GPL-tier
route — wrap the 2608 die, or hand the ROM to the clean-room ADPCM-A
section through the registry — when that work is scheduled.

## The 2608 die arrives with the drums (2026-07-28)

The 2608 die is wrapped (`vgms-cores-gpl/src/lle_opna.rs`): the DRAM bus
served pin by pin, and — the point — **the decapped internal rhythm mask
ROM plays**. A bass-drum key-on with no sample block loaded sounds, from
silicon this project could never ship by any other route. Two packages, two
serial framings: this die clocks its serial line at the master rate with no
trailing bit, where the OPM's ran at half rate trailing by one — each
pinned by its own idle-decodes-to-zero probe.

It is deliberately **not on the oracle bench yet**. A trial row read
corr 0.07–0.42 with the die far too quiet — the serial frame's structure
while Delta-T is active (dac_damode regates SH1 and multiplexes the FM and
ADPCM words onto one pin) is unpinned, so the number measured the harness,
not the emulation, and a number that indicts the wrong party is worse than
no number. The bench comment in `tests/oracle_lle.rs` records exactly what
to pin before the row returns.

The 2610 die — the original target, for our weakest OPN core — does not
compile upstream in its configuration (unguarded 2608-only GPIO writes at
the pin; a different error one commit back). It waits for upstream, not us.

## The 2608 die reaches the bench (2026-07-28, same day, later)

Two harness gaps found and pinned by probe: this package's serial line is
**bit-clock gated** (`o_s` is a real pin; sampling at master rate smeared
every word, which was the whisper-quiet FM), and its mantissa is **two's
complement** where the OPM DAC's was offset binary (the idle word said so).
With both fixed the trial 0.13 became **0.4883 median (n=4)**, above its
tripwire bar, pitch exact — and the row is on the bench.

Still owed: the die reads 2–11× quieter than our core file by file, with
the FM channel-slot accumulation, the SSG scale and the Delta-T DA
time-slots the open suspects. The row's comment carries the list; the
number is a floor under a harness still being tightened, not a verdict on
either emulation yet.

## The tail gets its numbers (2026-07-28)

Twenty rows entered the board at once — every chip the two tail tiers
shipped. The cold run (the one that also caught the WonderSwan crash) and
a fixed-binary rerun from the same cache agreed on every row neither
build touched, so the numbers are stable; the floors in `parity/mod.rs`
now sit just under the observed medians, majors-style. Three chips —
SegaPCM, K051649, GA20 — have **no single-chip corpus file at all**, so
their bars stay tripwires with that as the recorded reason.

The measurement did its job before it was even frozen. It caught four
healthy-but-quiet cores (level 0.12–0.39), and the gain fixes it dictated
brought **C140 to 0.974 / level 0.96** and **K053260 to 0.990 /
level 0.97** — the cleanest tail rows on the board, K054539 (0.748) and
YMZ280B behind them. It also caught **RF5C164 rendering nothing at all**:
the Mega CD rips upload sample RAM through `0x68` PCM RAM writes, which
the engine dropped wholesale. Probing Dark Wizard settled the semantics
(one type-`0x02` stream bank, then thousands of copies at *absolute*
chip addresses — the first sixteen fill all sixteen 4 KiB pages), and
with the path implemented the row went from silence to level 1.00. Its
waveform still disagrees (corr 0.025, +20 cents), which the row's reason
carries as the open investigation.

One trade is recorded rather than hidden: YMZ280B's x8 gain fix brought
its level from 0.12 to 0.81 but cost correlation (0.773 quiet, 0.664
loud, with dropout windows appearing) — clipping is the suspect and a
rebalance is owed.

The structurally-wrong cluster — QSound (0.046, +14.5 cents), MultiPCM
(0.034), YMF278B (0.075, the FM half unrouted), uPD7759 and PWM (one
single-chip file each), and the wavetable/square family (WonderSwan,
VSU, SAA1099, ES5503, X1-010) — is frozen at tripwire floors with its
suspects named per row. The wavetable family's near-zero correlations
carry the HuC6280 precedent: phase is implementation-defined, and the
reference's own two HuC6280 cores score -0.19 against each other, so
whole-file correlation may be measuring the metric there, not the cores.

## The 2608 die stops being quiet (2026-07-28, later still)

The "die reads 2–11× quieter than our core" defect was never the die. It
was the last of the harness, and the correction is the same lesson this
tier keeps teaching: **the DAC word's format is per package, and only the
silicon can be asked.** The wrapper had been decoding the OPM's
floating-point word — three-bit exponent, offset-binary mantissa, read
backwards from the falling edge. This package does not send one. Its
shifter loads the summed 18-bit accumulator as a **16-bit linear word**,
sign inverted into the top bit, and shifts it out **LSB first** — the
YM3016's input format, where the OPM's YM3012 took the float. The die's
own shifter line (`ac_shifter[0] = ac_shifter[1] >> 1`) says so, and the
idle die's single set bit two clocks before the strobe falls — positive
zero's inverted sign — pinned the alignment.

A second inversion came with it: **SH1 frames the left word here**, not
the right. Accumulator 1 collects the pan bits the register map calls
left, and in Delta-T DA mode the die gates `o_sh1` with the left enable.
The OPM's wiring had suggested the opposite, and copying it had been
swapping the channels on every render.

**0.4883 → 0.5829 median (n=4), levels 0.79–1.05.** The quietness is
gone; the row's bar rises from its 0.20 tripwire to 0.50. What is left
*should* be the genuine article — the drums our clean-room core cannot
play — but the per-file spread (0.31 to 0.72, envelope agreement 0.08 to
0.40) is wider than one fixed missing section ought to produce, so the
row does not yet claim the remainder is all rhythm. The sibling rows are
unmoved (OPM 0.9742, OPN2 0.9848), which is the check that says this was
the 2608 wrapper and nothing shared.

---

## 2026-07-28 — libvgm arrives, and the SN76489's open question closes

`LIBVGM-PLAN` lv-2 needed one chip measured end to end through the new
`LibVgmChip` binding. The chip was the SN76489, and the run answers a question
this chronicle had left open rather than merely adding a row.

**The measurement**, all twelve files, one pinned reference (`VGMPlay 0.52`,
`docs/.../parity/VGMPlay.ini`), each core rendered at its own native rate:

| our core | corr | level | gain dB | drop | cents |
|---|---|---|---|---|---|
| clean-room (the default) | 0.5855 | 0.984 | +0.580 | 0.000 | −0.0 |
| libvgm, `FCC_MAME` — *the reference runs `MAXM`* | 0.5353 | 1.006 | −0.068 | 0.000 | −0.0 |
| **libvgm, `FCC_MAXM` — the core the reference runs** | **1.0000** | 1.000 | 1.000 | 0.000 | −0.0 |

**1.0000 over twelve files.** Not "high": the metric does not resolve a
difference. `LIBVGM-PLAN` §4 predicted exactly this — reference and core share
a lineage, so a shared-core comparison should be near-exact and *a low score
would mean our binding was wrong*. That prediction is now a passed test rather
than an expectation, and it validates the whole lv-2 path at once: config
mapping, native rate, write fold, planar-to-interleaved render. Any one of them
wrong and 1.0000 is unreachable.

**What it says about the clean-room row.** That row's threshold has carried
"HF/noise-band disagreement under investigation; tones match to 1%" since the
freeze. There is no longer anything to investigate: with the reference's own
core on our side the agreement is exact, so the missing 0.41 is the clean-room
core differing from Maxim's, and nothing about noise bands, our engine, or the
resampler. The earlier `0.9958` native-rate figure — two files, flagged here as
too small a sample — was seeing the same thing from one file's worth of
evidence.

**Three ways to get a meaningless number, all met on the way.** Each produced a
plausible score, which is why they are written down:

1. **No `VGMSTUDIO_REF_CONFIG`.** The pinned ini is what the harness rewrites the
   render rate into, so without it the reference rendered at its own 44100
   while we rendered at 223721. Every core scored ≈0.006 — including the
   clean-room one, whose frozen row is 0.586. A baseline that fails to
   reproduce its own frozen number is the tell, and it is worth checking first.
2. **Core mismatch.** `emuCore: 0` takes libvgm's *default* (`FCC_MAME`) while
   the pinned ini selects `MAXM`. 0.5353 — a number about two emulators
   disagreeing, dressed as a number about our binding. This ini's own header
   warns about precisely this and it still happened.
3. **The challenger measured at the incumbent's rate.** `native_rate_of` asked
   the registry *default* for the rate while rendering through the challenger,
   putting our resampler back in the middle of the one measurement built to
   exclude it. Cost: −3.5 cents and 0.11 of correlation. Fixed — it now asks
   the core under test.

Two harness affordances came out of it, both of which lv-4 needs per chip:
`VGMSTUDIO_PARITY_CORE` (measure a provider that is not the default yet) and
`VGMSTUDIO_PARITY_CHIPS` (measure one chip instead of thirty-nine, an hour into a
minute).

**A property worth knowing before the next chip.** Maxim's SN76489 *ignores*
`srMode` and renders at whatever `smplRate` asks for — upstream's `EmuStructs.h`
warns that some cores do. So a libvgm chip's rate does not necessarily follow
its clock, unlike ymfm's. Pinned by a test, because code written on the obvious
assumption would look correct and drift pitch.

**The row does not change hands here.** `CORES-REUSE-PLAN` §7 gives the default
to the core that beats the frozen row, and 1.0000 beats 0.5855 — but taking
defaults is lv-4's step, chip by chip, and libvgm is registered as a selectable
alternative until then.

## 2026-07-28 — lv-3: the write table, and where the remaining gap actually is

The per-chip write table landed with eighteen chips. A spot check of three,
each exercising a *different* fold, against the same pinned reference:

| chip | rule exercised | libvgm corr | clean-room row | level |
|---|---|---|---|---|
| YMZ280B | `RegisterLatch` (the address/data pair) | **1.0000** (n=12) | 0.664 | 0.842 |
| C140 | `MemoryPortHigh` (16-bit offset split across our port/addr) | **1.0000** (n=12) | 0.974 | 0.749 |
| K053260 | `Register` (straight `write8`) | **1.0000** (n=6) | 0.990 | 0.505 |

Three folds, three exact agreements. Taken with the SN76489's, that is four of
the seven rules confirmed against the reference rather than only reasoned about.

**And it isolates what is left.** Correlation is level-invariant, so 1.0000
with a level of 0.505 says the waveform is identical and the *gain* is not.
That is not a defect in the binding — it is that `010b189` calibrated our output
scaling per chip against the **clean-room** cores, and a libvgm core has its own
raw scale. So every chip libvgm takes at lv-4 needs its level re-fitted, and the
harness already reports the number to fit against. Worth stating plainly because
the opposite mistake is easy: reading a 0.505 level as "the core is wrong" and
going looking for an emulation bug that is not there.

**Two things the table's own tests caught**, both of which would have been
silent in a corpus run:

- **SegaPCM exposes its 16-bit-address writer under `RWF_REGISTER`, not
  `RWF_MEMORY`.** Fetching only the memory space found nothing and every one of
  its writes was dropped. libvgm's player has the same split — it fetches that
  field from `RWF_MEMORY` for some chips and `RWF_REGISTER` for others — so the
  space is per chip, not per width, and we now try both.
- A device named in a spec but missing from `build.rs`'s `ENABLED` starts
  nothing and is *silently silent*. Two lists that cannot see each other now
  have a test that walks one against the other.

## 2026-07-28 — a core's level belongs to the core

The reused cores correlated at 1.0000 and read 0.50–0.85 on level, which is
not a contradiction: correlation is scale-invariant, so the two numbers were
saying "the waveform is identical and the gain is not".

**It is not the file's volume data.** Both candidates were checked and both
ruled out. The header's volume modifier is already excluded by the sample
filter, so it is zero across every measured file. The extra header's per-chip
volumes were *named* in that filter's comment but never actually checked — now
they are, and re-measuring K053260, C140 and YMZ280B afterwards gave identical
correlations, levels and sample counts. No file in those samples carried one.

**It is the reference's own volume chain**, which we do not implement:
VGMPlay's built-in `_CHIP_VOLUME[]` table, its hardcoded per-core patches
(K051649 ×8/5, C140 ×2/3), `_PB_VOL_AMNT[]` and `NormalizeOverallVolume()`.
Our engine applies none of it. What it had instead was four hand-fitted
constants baked *inside* clean-room cores at `010b189` — `cores/k053260.rs`
carries a literal `* 11 >> 3`, a ×5.5 derived from one scorecard reading.

That works exactly as long as a chip has one core, and stopped working the
moment a second arrived. So the calibration moves to where it belongs:
`CoreInfo::level`, 8.8 fixed point, applied by the registry when it builds a
core. Unity means no wrapper at all, so nothing uncalibrated pays for the
mechanism, and the existing clean-room constants are untouched.

| chip | before | after |
|---|---|---|
| YMZ280B | corr 1.0000, lvl 0.842, gain 1.185 | corr 1.0000, **lvl 0.997, gain 1.001** |
| C140 | corr 1.0000, lvl 0.749, gain 1.297 | corr 1.0000, **lvl 0.971, gain 1.001** |
| K053260 | corr 1.0000, lvl 0.505, gain 1.930 | corr 1.0000, **lvl 0.968, gain 1.000** |

Residual fitted gain is unity to three decimals, and correlation did not move —
which is the check that says a gain was corrected and nothing else was.

**A level here is a measurement.** It is the harness's own least-squares gain,
and it only means anything for a core whose correlation is high enough that one
scalar describes the whole difference. The other fourteen libvgm rows stay at
unity, honestly uncalibrated, until lv-4 measures them.

The principled fix is still to transcribe VGMPlay's volume chain, which would
let all four of `010b189`'s magic numbers go and would calibrate the chips that
were never fitted at all. This does not block it; it is the same field, filled
in per core instead of per chip.

## The 16-bit write range marks its second chip in the address (2026-07-29)

Two decode faults in one match arm, found by reading `0xC5`-`0xC8` against
upstream's `Cmd_Ofs16_Data8` rather than against the handover note that
described the range. Neither is a core's fault, and both were sitting under
rows this scorecard had filed as emulation gaps.

**1. The second-chip flag was read from the wrong byte.** `0xC0`-`0xC8` mark a
second instance in bit 15 of the 16-bit address — the bit the `0x7FFF` mask
exists to clear — so which *byte* carries it follows the byte order: byte 2 for
`0xC0`-`0xC3`'s little-endian address, byte 1 for `0xC5`-`0xC8`'s big-endian
one. Upstream writes that rule twice, once per convention (`Cmd_SegaPCM_Mem`
tests `fData[0x02]`, `Cmd_Ofs16_Data8` tests `fData[0x01]`). This decoder read
byte 2 for the whole range while masking byte 1 out of the address, so the two
halves of its rule disagreed, and every big-endian write whose *low* address
byte had bit 7 set was retargeted to a second chip. The engine builds voices
only for declared instances, so those writes were dropped: **43.7% of the
corpus's 11.0M X1-010 writes, 25.2% of its 64,736 WonderSwan RAM pokes, 0.56%
of its 4.2M VSU writes.**

Not one corpus file declares a dual SCSP, WonderSwan, VSU or X1-010 — and none
declares an SCSP at all — which is exactly why this survived: it looks like a
dual-chip bug, and the dual-chip path is the one part of the range no rip in
the corpus exercises.

**2. The WonderSwan's wave RAM was never written.** `0xC6` is a *memory* write
against `0xBC`'s registers, the same collision the port-1 convention was
invented for at `0xC1`/`0xC2` — and `cores::WonderSwan` has always documented
port 1 as its memory path. The decoder never set it. Every wave-RAM poke in the
corpus's 266 WonderSwan rips was read as a register write, `wave_ram` kept its
initial contents, and all four wavetable channels rendered a constant.

A/B against the pinned reference, same build and the same twelve files per
chip, the decoder line the only difference:

| chip | before | flag byte fixed | + `0xC6` on port 1 |
|---|---|---|---|
| X1-010 | corr 0.0290, lvl 0.674 | corr 0.0290, **lvl 0.939** | unchanged |
| VSU | corr 0.0735, lvl 0.464 | corr 0.0735, lvl 0.468 | unchanged |
| WonderSwan | corr 0.0224, lvl 0.221, drop 0.057 | unchanged | **corr 0.8949, lvl 0.498, drop 0.000** |

The X1-010's correlation does not move because what was missing was half its
envelope and wave RAM — that is level, not phase — and its row keeps its 0.02
floor and its envelope-walk suspect. The WonderSwan's row does not: **0.022 to
0.8949** retires "wavetable phase is implementation-defined (the HuC6280
precedent)" as its explanation, and its floor is re-frozen at 0.85. Level 0.50
is still owed there.

Two lessons, both familiar. A row whose *stated* suspect is untestable —
"phase is implementation-defined" — is a row that stops being investigated;
the HuC6280 precedent is real but it makes a comfortable place to file
anything. And the harness measures four numbers for a reason: correlation
alone would have shown nothing at all for the X1-010 here, and level alone
nothing for the WonderSwan.

## The MultiPCM's bank select reached no core at all (2026-07-29)

The row above filed MultiPCM's 0.034 as "the ROM-header envelopes are
unmodelled and a structural gap is under investigation". The structural gap was
`0xC3`, and it is the same class of fault as the two above it: a decode read
against the handover note rather than against upstream.

**The spec's field names are wrong, and the decoder believed them.** `0xC3 cc
bbaa` is documented as "write set bank offset aabb to channel cc", and `cc` is
not a channel. Upstream reads it as `chipID << 7 | bankmask` (`Cmd_YMW_Bank`
takes `fData[0x01] & 0x03`), and the corpus settles it: across all 72,481 files
`0xC3` appears **296 times in 175 files**, and `cc` is only ever `0x01`, `0x02`
or `0x03` — one bit per 512 KiB bank. A 28-voice chip whose channel number
never exceeds three is not naming a channel. `bbaa` is a little-endian offset
in 64 KiB units; its high byte is zero in all 296 (which is why upstream can
ignore it and say so — no YMW258 ROM over 16 MB).

The decoder assembled `addr = (bb << 8) | cc` and `data = aa`, the shape the
rest of the `0xC0`-`0xC8` range takes. Information-preserving, and meaningless:

- **the clean-room core** reads `addr << 16` as the bank, so a file saying
  `0x10_0000` banked to `0x1003_0000` — every banked sample thrown past the end
  of the ROM, and this core kills a voice whose fetch misses;
- **the libvgm binding** had no bank rule at all (`WriteRule::Register`), so
  `write8(cc, aa)` landed on the *register file*, where `cc` of 1 and 2 are the
  slot and register selects. 96 of the 296 commands silently retargeted
  whichever write came next.

The fix is a port. `addr` and `data` are two fields and the command has two
operands, but neither is an address, so the bank select takes a *third* space
of the chip beside the register-versus-memory pair the port already separates
(`stream::BANK_PORT`), and the operands go back to meaning what the command
says: mask in `addr`, offset in `data`. `WriteRule::MultiPcmBank` then does
what `Cmd_YMW_Bank` does — register `0x10` for Model 1's 1 MB window,
`0x11`/`0x12` for Multi 32's two 512 KiB halves — and the clean-room core grows
the second bank latch it never had. Its power-on pair is chosen to reproduce
the single `0x18_0000` bank it held before, so the unbanked rips it was
calibrated against fetch from exactly where they used to.

A/B against the pinned reference, same build, the same twelve files, the decode
the only difference. 175 of the 224 single-chip MultiPCM files in the corpus
carry a `0xC3`, so the sample is not sparse in them:

| core | before | after |
|---|---|---|
| clean-room (default) | corr 0.0343, lvl 0.413, **drop 0.111** | corr 0.0338, lvl **0.491**, **drop 0.030** |
| libvgm (`VGMSTUDIO_PARITY_CORE=libvgm`) | corr 0.0780, lvl 2.104, +36.0 cents | corr 0.0780, lvl 2.141, +36.0 cents |

The X1-010 lesson again, and more sharply: **correlation did not move at all**,
and would have reported this fix as a no-op. Level and dropout are what a bank
select can change — a voice fetching from the wrong megabyte is silent, not
out of phase — and dropout falling by a factor of nearly four is the honest
measure of it. The row keeps its 0.03 floor; what it no longer keeps is the
"structural gap" clause, because the structure is now right and the remaining
distance is the unmodelled ROM-header envelopes on their own.

Two things this leaves open, both new and neither this fix's:

- **libvgm's YMW258 runs +36.0 cents sharp against the reference's**, at level
  2.14, and its correlation is *double* the clean-room core's despite that. The
  reference plays a MAME YMW258 too, so a detune that large is a rate or clock
  question in our binding, not an emulation difference. That is lv-4's to
  settle before this chip's default can move.
- the clean-room core's banking is now MAME's rule (bit 20 selects banked, bit
  19 chooses the half) but its *power-on* state is still a departure from it:
  MAME does not bank at all until a bank command arrives, where this banks from
  reset. The 129 corpus files that never send a `0xC3` are what that default
  was fitted to, and re-deriving it is a separate measurement.

## The full-range audit, and three more decode-class bugs (2026-07-29)

The `0xC3` find prompted the question it should have: what else reads
differently from upstream? Every `Cmd_*` handler in libvgm's
`vgmplayer_cmdhandler.cpp` was read against its arm of `decode()` (and against
the write rules and cores downstream of it), and every suspected divergence
was then *counted* over the corpus — a Python walk driven by
`vgmstudio-chip-index.tsv`, so only the files declaring the chip are opened.
Three real bugs, all fixed; a tail of inert divergences, recorded.

**1. `0xB2` (PWM) is `Cmd_Ofs4_Data12`, not `Cmd_Ofs8_Data8`** (`a837273`).
Nibble register in bits 6-4, twelve-bit big-endian value across the low nibble
and the second byte — the layout `cores::Pwm::write` has documented since it
was written, and the decoder never delivered: it read the range's usual
`aa dd`, so "register 2, value 0x1xx" (`ad` = 0x21) became "register 0x21",
which the core's nibble mask reads as the *cycle* register, and every value
lost its top four bits. 229.0M of the corpus's 229.3M PWM writes (180 of 184
files) carried a non-zero value nibble; 54.6M landed on the cycle register.
The n=1 parity row does not reward it (0.0156 → 0.0169, lvl 0.338 → 0.328):
the remaining suspect is the core's stated fixed-rate approximation, which no
operand fix can reach, and the row's reason now says so.

**2. `0x31` is `Cmd_AY_Stereo`, not a write to AY register 1** (`abb78a1`).
Six-bit stereo mask to a dedicated per-core function; bit 6 retargets it at a
YM2203's SSG, bit 7 is the second chip. We wrote the raw byte to the first
AY8910's register 1 — channel A's coarse period — detuning a live voice on
each of the corpus's 100 masks (35 files; bit 6 set in 30, bit 7 in 50). The
mask now rides `stream::STEREO_PORT`, and the fix needed *two core guards*
beyond the decode: `cores::Ay8910` ignored `port` entirely, and `OpnCore`
folds `port & 1`, either of which would have eaten the mask as a register
write one address over. Rows unmoved and expected to be (AY 0.597, YM2203
0.935): a 35-in-11,177 population is invisible to a twelve-file sample.

**3. The SN76489 core read the Game Gear stereo mask as a latch byte.** The
decoder and libvgm agree (`SN76496_W_GGST`): `0x4F` is address 1. The
clean-room core dropped `addr`, so the mask fell into `write_byte`, where the
overwhelmingly common `0xFF` parses as "noise volume, attenuation 15" — the
noise channel died on the spot, and the byte corrupted the latch for the data
byte after it. This is the *largest population of the three*: 13,344 masks in
8,067 of 11,845 SN76489 files (68%), all but one with bit 7 set. And yet the
row did not move (0.5855 vs the frozen 0.586): the mask is an init-time write
and real drivers re-program every volume immediately after, so the mute was a
transient the correlation window barely weighs. The probed worst case — a mask
with no volume write behind it — silenced the channel entirely.

The lesson this section exists for: **none of the three was visible in the
parity table.** One hid behind an n=1 row, one behind sampling odds, one
behind a transient. The operand *counts* are what found and sized all three —
the corpus is a better witness to a decode bug than the renderer is, because a
decode bug's damage is conditional on the driver while its operand layout is
not.

Divergences audited and left alone, with reasons: `0xE1` (C352) misses
upstream's `& 0x7FFF`/chip-id read of bit 15 — bit never set in 36.1M
commands, and the write is dropped either way today; `0xC1`/`0xC2` read a
second-chip bit that `Cmd_RF5C_Mem` hard-codes to zero — never set in 1.04M
commands; `0xD6` splits a 16-bit datum across `addr`/`data` — no ES5506 core
exists on either side of it; `0xB4`'s FDS remap and `0xB8`'s pin-7 strip guard
features no core models. Self-consistent conventions that would bite only if a
libvgm row were added for the chip: WonderSwan registers 0-based here versus
`0x80`-based upstream; the SAA1099's address/data latch order; `0x68` RAM
addresses treated as absolute where upstream ORs in the RF5C bank register.

## 2026-08-01 — the core picker was also a volume control

Reported from listening, not from the table: *"the volume between cores for the
same chip is quite different — libvgm is a lot quieter than Nuked-\*, which
causes issues when the VGM contains different chips."* The rip named was Sonic 3
& Knuckles' Hydrocity Zone Act 2 — a YM2612 and an SN76489, and switching the
YM2612 to libvgm drops its FM ~6 dB while the PSG stays put.

**It is real, and it is the three chips whose default is a Nuked core**
(`install_cores` promotes `ym2612.nuked`, `ym2151.nuked`, `ym2413.nuked`; libvgm
is the default everywhere else, so on every other chip a "core change" is one
libvgm core for another). Measured two independent ways — the reference harness
at native rate (n=12), and a direct core-against-core render at 44100 (n=8):

| chip | default `lvl` vs reference | libvgm `lvl` vs reference | ratio | direct (n=8) |
|---|---|---|---|---|
| YM2612 | 0.955 | 0.466 | **0.488** | 0.4877 [0.4858..0.5018] |
| YM2151 | 1.000 | 0.498 | **0.498** | 0.4973 [0.4663..0.4998] |
| YM2413 | 0.370 | 0.246 | **0.665** | 0.7375 [0.7053..0.9281] |

The YM2612 and YM2151 agree to three decimals across the two methods and barely
scatter across files: one scalar describes the whole difference, which is
exactly the condition `CoreInfo::level` states for being usable at all.

**Read `lvl`, never `gain`.** The libvgm YM2612 reads `lvl 0.466 gain 1.766` on
the same twelve files — the least-squares fit is `α = ρ · σ_ref / σ_ours`, so it
collapses for any decorrelated pair, and two *different* emulators for one chip
are the pair most likely to decorrelate. `ChannelScore` has warned about this
since the SN76489's first row; this is the second time it has mattered.

**The corrections**, in `vgms-cores-libvgm`'s spec table:

| row | level | anchored to |
|---|---|---|
| `ym2612.libvgm` | 525 | the reference (its default is there already) |
| `ym2612.libvgm-gens` | 516 | " |
| `ym2151.libvgm` | 514 | " |
| `ym2413.libvgm` | 385 | **the chip's default, not the reference** |
| `ay8910.libvgm-mame` | 399 | its own default row (0.6415 [0.5886..0.6938]) |

The YM2413 is the one that could not be anchored to the reference: the chip's
own default sits at 0.370 of VGMPlay's, the shortfall this chronicle has carried
as open since 2026-07-28. Calibrating the alternate to the reference would have
left it 2.7x above the chip's own default — the same complaint, an octave
louder. **If that open item is ever settled, this row's number moves with it.**

**Left alone, and why.** A row whose ratio scatters across files is not
describable by one scalar, and a fitted constant there would be a guess wearing
a measurement's clothes:

| row | ratio | spread | reading |
|---|---|---|---|
| `nesapu.libvgm-mame` | 1.262 | 1.97x | differs in more than level |
| `gameboydmg.libvgm-mame` | 1.148 | 1.74x | " |
| `qsound.libvgm-mame` | 1.029 | 1.56x | within band anyway |
| `ym2413.libvgm-mame` | 0.779 | 1.51x | scatters; EMU2413 vs MAME vs Nuked, three ways |
| `sn76489.nuked-psg` | 0.895 | 1.49x | 10.5%, at the band's edge, scattered |
| `huc6280.libvgm-mame` | 0.932 | 1.41x | within band |
| `saa1099.libvgm-mame` | 1.000 | 1.25x | within band |
| `rf5c68/164.libvgm-gens` | 0.994 | 1.02x | within band |
| `sn76489.libvgm-mame` | 1.021 | 1.05x | within band |

**The regression guard.** `every_core_for_a_chip_agrees_on_its_level` in
`reference_parity` is the measurement above, kept: every chip with more than one
realtime core, each core's RMS over the default's, failing a row that is more
than 10% out *and* consistent enough for a scalar to fix. It needs the corpus
and no reference player. A scattered row prints its numbers and does not fail —
saying so out loud, so the list above cannot quietly become a list of tolerated
faults.

**What this did not touch.** libvgm's own per-chip volume table
(`VGMPlayer::_CHIP_VOLUME`) is still not applied by our binding — VGMPlay halves
the SN76489 and doubles the YM2413 relative to everything else, and we do
neither. That is a *cross-chip* balance question about every libvgm row at once,
not the *same-chip* one reported here, and it belongs with the YM2413's open
shortfall rather than in a fix for the core picker.

## 2026-08-01, later — the volume model itself, and a silent chip

Three more listening reports, each of which turned out to be a different layer
of the same subject:

**1. Lemmings (FM Towns) played silence.** An RF5C68-only rip: one 34 KiB
type-`0xC0` RAM block, channels starting at 0x7400. Our binding looped RAM
images through the byte-wide memory writer — whose window (this is the CPU's
own view of the chip) masks offsets to 4 KiB (`rf5c68_mem_w`:
`offset &= 0x0FFF`) — so the whole image folded onto one window while the
channels fetch *absolute* addresses. The play cursor advanced normally through
empty RAM. Diagnosed by dumping the C state byte by byte after every layer of
the stack checked out individually; the tell was a sentinel that read back
through the window but not through the raw data pointer. Fix: RAM images go
through each core's `DEVRW_BLOCK` writer (absolute, as upstream's
`Cmd_DataBlock` does), with the RF5C bank register tracked binding-side and
OR'd into every RAM-write address (upstream's `Cmd_RF5C_Reg` bank patch +
`DoRAMOfsPatches`). This also fixes the same folding on the `0x68` copies.

**2. Black Knight 2000 clipped hard.** Its rips are YM2612+YM2151+PWM, and one
carries volume modifier `0xC1` (0.25x). Two findings:

* The header volume modifier was already honoured by the volume lever for the
  editor — but the **pack preview** only read it off OPL sources, so a non-OPL
  VGM previewed at whatever the lever last was. Fixed.
* The real damage was **cross-chip balance**: VGMPlay normalises every file's
  loudness from its declared chip set (`EstimateOverallVolume` /
  `NormalizeOverallVolume` — double every chip volume while the weighted sum
  is ≤ 0x180, halve while > 0x300). Our per-core levels are calibrated against
  *single-chip* reference renders, which fold that normalisation in — so a
  three-chip mix summed voices each sitting at its normalised-up solo level,
  clipped the 16-bit mix, and no output lever could undo it.

  The fix is `vgms_synth::balance`: VGMPlay's `_CHIP_VOLUME` + `_PB_VOL_AMNT`
  tables and normalisation, expressed as a per-voice **ratio**
  `V_eff·N(set) / (V·N({chip}))` — exactly 1.0 for every single-instance
  single-chip file, so no calibration and no parity row moves. Black Knight's
  set puts all three voices at 1/2 (peak 32768→16036, unclipped); Hydrocity's
  keeps the FM at unity and drops the PSG to half, which *is* VGMPlay's Mega
  Drive tilt. Dual declarations halve per instance (T6W28 excepted), the
  v1.70 extra header's per-chip volumes override the table (Black Knight
  carries entries), and the estimate counts chips this build cannot play,
  because the reference's does.

**3. 500GP (C352) was ~4x hot.** The unmeasured `c352.libvgm` row, measured at
last: the files that correlate at 1.0000 read lvl 4.0000 exactly — VGMPlay's
`_CHIP_VOLUME[C352]` (0x40). Level set to 64; the mix stops clipping. Rows for
other never-measured chips still sit at unity; the C352 measurement is the
template for closing them chip by chip.

**And channel muting stopped being OPL-only.** The report: mute selectors did
nothing for other chips. Engine, UI and service wiring all proved correct —
the gap was the three chips whose default core is Nuked (`ym2612.nuked`,
`ym2151.nuked`, `ym2413.nuked`), which had no mute and so shipped disabled
toggles. Now:

| core | mute | how |
|---|---|---|
| `ym2612.nuked` | **yes** | render-gate on the 24-cycle rotation (order 2,6/DAC,4,1,5,3, from libvgm's copy); DAC-enable sniffed off register 0x2B |
| `ym2413.nuked` | **yes** | render-gate on the 18-cycle rotation's melody/rhythm pair, map transcribed from libvgm's `nukedopll_update` |
| `ym2151.nuked` | no — cannot | its DAC accumulates all eight channels inside the chip; the tooltip points at the libvgm core, which mutes |

Both gates key on a binding-side mirror of the chip's private cycle counter
(zeroed by reset, +1 per clock, untouched by writes), pinned by tests that mute
the playing channel and an idle one.

## 2026-08-01, later still — the muting report, re-verified end to end

A follow-up report said muting and panning still did nothing on Hydrocity
(SN76489+YM2612), on every core. Rather than assume, every layer was measured
against that exact file **in the current tree**:

| layer | evidence |
|---|---|
| engine + real cores | both chips masked → RMS 0.0/0.0; YM2612 masked → PSG remains; SN hard-left + FM masked → right channel 0.0 |
| the real `NativeAudio` (cpal stream, command ring, callback) | its own peak meter: 0.0356 → 0.0000 after a full mask |
| the clicked toggle | new GUI test `clicking_a_chip_channel_toggle_pushes_the_mask` |

All passed — **and the conclusion drawn from them was wrong.** "Every layer" was
three layers of four. The reader should skip to the next section: the fault was
real, in the one seam not on that list, and blaming the user's binary was a
misreading of a gap in the evidence.

**Whole-chip Mute/Solo landed with it**, as the report suggested — the right
instrument for exactly this A/B. Each chip tab of a multi-chip file gets a
Mute toggle and a Solo toggle (solo mutes every other chip; again restores).
Backed by a new engine guarantee: a mask covering every channel silences the
voice *in the engine*, whatever the core can do -- so chip Mute/Solo work even
on cores with no per-channel mute (Nuked-OPM included), pinned by
`a_full_mask_silences_a_voice_whose_core_cannot_mute`. The Help dialog lists
both gestures.

## 2026-08-01 — the seam nobody tested: a defaulted trait method

The muting report was right and the section above was wrong. A screen recording
settled it: the channel toggles visibly un-lit while the peak meter never
moved. The UI was sending, the engine was applying, and the two were not
connected.

**`SwitchingAudioService` — the only `AudioService` the desktop binary ever
builds** (`vgmstudio.rs:119`) — forwards eighteen methods to the active
backend, including `set_muting` and `set_panning`. It never defined
`set_chip_muting` or `set_chip_panning`, and those two were the only live
controls the trait gave a `{}` **default body** (`platform.rs:417,420`). So the
wrapper silently inherited two no-ops, and every any-chip mute and pan died
between a UI that sent them and an engine that would have applied them.

The A/B, same file, same config, same call order, same process — only the
wrapper differs:

| service | peak before | after a full mask |
|---|---|---|
| `NativeAudioService` (what every probe used) | L 0.90607 | **L 0.00000** |
| `SwitchingAudioService` (what the app uses) | L 0.90607 | **L 0.89005** |

**Why three green layers proved nothing.** The engine probes constructed
`VgmEngine` directly; the "real audio service" probe constructed `NativeAudio`
directly; the GUI test used `FakeAudioService`. Each end was real and each
passed. Nothing exercised the wrapper in between, and a defaulted trait method
is invisible exactly there — it is not a wrong line of code, it is an absent
one, and absent code has no line to review.

**The fix is the trait, not the wrapper.** Forwarding the two methods repairs
today's bug; *removing their defaults* is what stops the next one. They are
required methods now, so a backend must either forward them or write the empty
body and its reason — `RetroWaveAudioService` does the latter (an OPL3 board;
`load` refuses anything else). `the_switching_service_forwards_the_any_chip_controls`
pins the forwarding behaviourally; the compiler pins the class.

**Measured on the way, and worth keeping** (release, real cores, real file):

| core | per-channel mute | note |
|---|---|---|
| `sn76489.libvgm` (default) | **works**, to exactly 0 | pan works too: hard-left L 22.3M / R 0 |
| `sn76489.libvgm-mame` | works | no pan (`supports_pan=false`) |
| `sn76489.nuked-psg` | none, honestly declared | registry says `channel_mute=false`; UI greys it |
| `ym2612.nuked` (default) | **works** (0.0845; the residual is the idle DAC-ladder floor) | the render gate added earlier is real, verified not assumed |
| `ym2612.libvgm` (level 525) | works, to 0 | `Leveled`-wrapped, and the wrapper forwards |
| `ym2612.libvgm-gens` (level 516) | works, to 0 | also wrapped |
| `ym2612.lle` | none | `channel_mute=false`, `realtime=false` — never the transport's core anyway |

A mask also survives libvgm's device restart on `configure`, and survives a
rewind (`start()` re-applies both).

**Panning is not broken, but it is near-undiscoverable**, and the report is
fair. Two things hide it, both by design: the pan knobs are not drawn at all
until the small **Custom** icon under the channel row is pressed
(`show_pans = pan_supported && self.custom`), and pressing Custom is itself
inaudible because every knob defaults to centre. On Hydrocity there is a third:
the SN76489 is ~6% of the mix (peak 0.0597 against the YM2612's 0.9315), so
panning it moves the meter by ~1% unless the YM2612 is muted first — with it
muted, the pan is total (L 0.08438 / R 0.00000, and the exact mirror).

## 2026-08-11 — Cameltry names the YM2203, the fourth half-level libvgm row

Reported from listening: on Cameltry (Taito B System, YM2203+OKIM6295) *"the
YM2203 chip is too quiet compared to the OKIM6295"* — and so it was, by
exactly the factor this chronicle has now seen four times. The balance model
was checked first and is innocent: the file is a plain v1.61 header (no extra
header, no volume modifier), legacy VGMPlay's arithmetic for the set is
YM2203 0x100 + OKI 0x200 (PB 0x200) = 0x300, no normalisation, and our
`voice_gain` reproduces its tilt exactly (now pinned as
`the_cameltry_pair_keeps_the_references_tilt`). The fault was the anchor
under the ratio: `ym2203.libvgm` had sat at `LEVEL_UNITY` since the libvgm
cores took the defaults, *unmeasured* — the only measured YM2203 rows in this
file were the retired clean-room core's.

Measured against the reference at native rate (n=12), including the fmopn
siblings the same suspicion covered, none of which had a threshold row:

| chip | corr | lvl | verdict |
|---|---|---|---|
| YM2203 | 0.9999 | **0.508** | half the reference — the 2026-08-01 YM2612/YM2151 story again |
| YM2608 | 0.9983 | 1.000 | nothing to fix (and libvgm carries the rhythm ROM: the clean-room era's 0.60 "ADPCM gap" is gone) |
| YM2610 | 1.0000 | 1.000 | nothing to fix |
| OKIM6295 | 1.0000 | 1.000 | nothing to fix — the complaint's other half was never guilty |

**The corrections:** `ym2203.libvgm` level **504** (256/0.508; re-measured
lvl 0.984, corr unmoved at 0.9999), and YM2203-LLE level **499** to keep the
die swap level-neutral — the 2026-08-09 LLE audit measured the die at 1.01
*against the then-miscalibrated default*, so its unity inherited the same
half-level and moves by the same factor (256 / (1.01 × 0.508)).

**All four chips now hold permanent `shared(...)` rows in
`parity::THRESHOLDS`** — every one measured at or above the 0.99 shared-
lineage ideal. Their absence is what let a 6 dB anchor error ship silently:
`every_cored_chip` prints "no threshold — not compared" and moves on. The
lesson mirrors lv-2's: a shared-lineage row that has never been read is not
"probably 1.0", it is unmeasured.

## 2026-08-12 — the level sweep: every remaining chip and core

The Cameltry lesson, applied to the whole roster: every buildable chip
without a threshold row was measured against the reference (native rate,
n=12 strided single-chip files, the scorecard harness under a temporary
lenient bar), plus `every_core_for_a_chip_agrees_on_its_level` across every
realtime alternate. The YM2203's "unmeasured unity anchor" turned out to be
the rule, not the exception.

**The mechanism, named.** For a single-chip file the reference plays a chip
at `_CHIP_VOLUME[chip] × 2^shift` — the power-of-two loudness normalisation
`EstimateOverallVolume` applies to the one-chip set. That staged net is what
every calibration anchors to, and it retro-predicts every previously
measured row exactly (RF5C68's 0.364 is its 0xB0-doubled-twice staging;
YMZ280B's 0.844 its 0x98 × 1.1875). An unmeasured unity anchor is therefore
wrong by precisely that staging factor whenever the raw cores agree — which,
for shared-lineage rows, they do.

**Corrected, each verified at lvl ≈ 1.000 against the reference after:**

| chip | corr | lvl before | new level | note |
|---|---|---|---|---|
| HuC6280 (Ootake) | 1.0000 | 0.500 | 512 | MAME alternate ×2 with it (1.047 relative) |
| X1-010 | 1.0000 | 0.500 | 512 | |
| YMF271 | 1.0000 | 0.500 | 512 | |
| QSound (superctr) | 1.0000 | 0.500 | 512 | MAME alternate ×2 with it (0.995 relative) |
| VSU | 1.0000 | 0.500 | 512 | |
| uPD7759 | 1.0000 (n=1) | 0.448 | 572 | one corpus file; staging derivation co-signs (2.234) |
| MultiPCM | 0.9999 | **1.998** | 128 | the one row too LOUD (0x40 staged ×2 = 0.5) |
| K054539 | 0.9987 | 0.500 | 512 | |
| WonderSwan | 0.9888 | 0.500 | 512 | |
| OKIM6258 | 0.9766 | 1.170 | 219 | 17% hot |
| SAA1099 (VB) | 0.8471 | 0.500 | 512 | gain 1.878 concurs; noise-phase band |
| SN76489 (Nuked-PSG, default) | 0.3581 | **0.247** | 1036 | see below |
| POKEY | (output rate) | 0.501 | 512 | native rate impractical: MAME renders at its 1.79 MHz clock and the pitch search is quadratic in rate |
| RF5C164 (both rows) | 0.2605 | **2.649** | 703 → unity | see below |

Also ×4 with their chip's default: `sn76489.libvgm` and `.libvgm-mame`
(medians 1.10/1.12 of Nuked-PSG, scatter 1.55× left as the deliberate
residual). Verified clean, no change: YMF278B 1.000, YMZ280B 0.997, C352
1.000, C140 0.971, K053260 0.968, RF5C68 0.999, Y8950 0.998, NES APU 0.978,
YM3526 1.009 (levels only — see the open findings).

**The SN76489 was the loudest wrong anchor on the board**: the default
(Nuked-PSG, the owner's promotion) played at a *quarter* of the reference's
level — the ×2 staging (0x80 doubled twice) on top of the raw-half scale the
2026-07 survey had already noted for libvgm's SN. Every Master System rip
and every Mega Drive PSG sat 12 dB under the reference. The correlation band
(0.36, the inherited noise/HF item) means the constant is pinned two ways
rather than one: lvl 0.247 measured, 4.0 derived, 1.2% apart. Post-fix it
reads lvl 0.998.

**The RF5C164 was the 2026-08-08 fix overshooting**: the 68's 703 was copied
onto the 164 rows, but the staging that produced the 68's 0.364 (0xB0 ×
2.75) is not the 164's (0x80 × 1.0 — VGMPlay's own tables). Measured 2.649×
hot, reverted to unity, re-measured lvl 1.006.

**Threshold rows added** — shared bars for HuC6280, X1-010, K054539,
YMF271, QSound, VSU, C352, RF5C68, K053260, C140, YMZ280B, YMF278B,
MultiPCM, uPD7759; known-gap bars under the observed scores for WonderSwan,
OKIM6258, SAA1099, Y8950, SN76489, and two OPEN rows (below). Nothing
buildable prints "no threshold" silently wrong again.

**Open findings, not level-shaped, each with a follow-up task chip:**

- **YM3526**: corr 0.0312 with a systematic **−24 cents** (level exactly
  right at 1.009) — the AY-class detune signature, on the OPL-adapter
  projection path whose YM3812/YMF262/Y8950 siblings show no detune.
- **RF5C164**: corr 0.2605 with **fit gain −0.963 at lvl 1.006** — a
  polarity-inversion signature against the reference, on the same device
  whose 68 flavour reads 1.0000 (legacy VGMPlay runs Gens' PCM core for the
  164; ours may not be the core it compares against).
- **ES5503**: corr 0.0022 — the comparison itself is broken (channel-count
  configuration is the first suspect); lvl 3.398 suggests the staging (×4 at
  0x40) will need applying once the pair correlates, but not before.

**Reported and left alone:** Game Boy DMG (different core family: SameBoy vs
the legacy player's; median 1.336, scatter 1.7×), PWM (one corpus file, corr
0.017), NES APU (corr 0.45 but lvl 0.978 — nothing to fix by this method),
and the scattered alternates the agreement run named (SN76489 pair residual,
YM2413 pair with silent files in range, GB/NES MAME rows, Y8950-CQM).

## The ES5503 was never decorrelated — it was playing 11× too fast (2026-08-12)

The sweep's corr 0.0022 row is closed: **0.9944 at lvl 1.005 (n=12)**. The
channel-count suspect was innocent (`configure_es5503` matches upstream's
`DEVID_ES5503` case byte for byte, as do the 0xD5 write fold and the 0xE1
RAM-block routing). The fault was a *rate* the harness's own vocabulary had
no word for: the ES5503's output rate is **dynamic** — `clock / 8 /
(oscillators + 2)`, re-derived on every oscillator-enable write (register
0xE1) and announced through libvgm's `SetSampleRateChangeCallback`, which
the adapter never registered. `native_rate()` reported the reset rate (one
oscillator: `clock/24` ≈ 298 kHz on a IIgs) for the whole file, while every
real rip enables all 32 oscillators (~26 kHz), so the engine consumed
source frames ~11× too fast. Unrelated waveforms, exactly as measured.

Two seams, both now under test: the adapter routes the callback into an
atomic rate slot (`the_es5503_rate_follows_the_oscillator_enable_register`),
and the engine's `Voice::follow_rate` rebuilds the resampler when a core's
rate moves after a write or a rewind
(`a_rate_change_after_a_write_rebuilds_the_resampler`). Eight vendored
cores fire the callback (OKIM6258/6295, MSM5205/5232, ES5503, AY8910,
BSMT2000, ICS2115); the previously-measured rows among them keep their
scores — a core that never fires it is a no-op in the new path.

With the comparison sound, the level followed the staging table as the
sweep predicted: **64** (`_CHIP_VOLUME` 0x40 = 0.25; measured 0.252, within
1%). The first correlated run read 0.9912 at lvl 3.976; applying the level
lifted corr to 0.9944 — at 4× hot the 16-bit render had been clipping.

**Residual, on the threshold row as a known gap:** a flat offset, median
−6.5 cents (per-file −7.0 to −0.0). Our rate arithmetic matches upstream
exactly, so the suspect is the reference's older core revision — the MAME
ES5503's loop-phase handling changed in v2.1 ("no longer go out of tune"),
and legacy VGMPlay predates the current core. Untested; the bar sits at
0.98/10.0 cents under the observed score.

**Unanchorable — no single-chip corpus files:** SegaPCM, K051649, GA20,
SCSP, Mikey (the last also unplayable by the legacy reference). Their
staging derivations (SegaPCM 3.0, K051649 2.0, GA20 2.5, SCSP 4.0) are
recorded here as *predictions*, deliberately not applied: the SN76489 shows
raw scales are not always shared, so an unmeasurable derivation stays a
prediction. If single-chip rips of these ever land in the corpus, measure
first.

## 2026-08-12 — YM3526: the OPL adapter now projects the header clock

The level sweep's first OPEN row closes: **corr 0.0312 → 0.7533** (n=12),
cents −0.0, lvl 0.999, drop 0.000.

The "−24-cent" reading was an artefact of the saturated ±60-cent search; the
real offsets were −192 and +306 cents. The `OplCoreAdapter` reset its chip
at the standard crystal's 49716 Hz and reported that as the native rate
whatever the header said — and *every one* of the twelve sampled single-chip
YM3526 rips is an arcade board at 4 MHz (Terra Cresta, Galivan, Dangar,
eight in all) or 3 MHz (DECO8, Renegade, four), none at 3.579545 MHz. The
reference honours those clocks (fmopl runs at `clock / 72`), so every pair
compared a chip against a transposition of itself, outside the detune
search: audible playback scored as pure decorrelation, the OKIM6295 divider
lesson over again. The siblings never showed it because their sampled rips
are standard-crystal (the YMF262's twelve entirely so; the YM3812's row is
pinned unmoved by the control run below either way).

The fix (`opl_adapter.rs`): the chip still renders at 49716 Hz — every
`OplChip` assumes the standard crystal, and at that rate its internal
resampler is an identity pass — but a `projected` adapter reports
`clock / 72` (OPL2 generation) or `clock / 288` (YMF262), rounded, as its
`native_rate`, so the engine's Voice resampler repitches the whole render by
`clock / standard`: exactly what the different crystal does to real silicon,
envelopes and vibrato included. Standard clocks still land exactly on 49716,
keeping the identity bypass. The RetroWave hardware host keeps the pinned
un-projected adapter (a real board cannot be repitched, and its write
timing wants the identity pass), and the LLE die sims already clocked
correctly (`clock / CLOCKS_PER_SAMPLE`).

**Siblings re-measured, unmoved to four decimals** (YM3812 verified against
a stashed pre-change control run): YM3812 0.9771 / lvl 1.005 / −0.0 cents,
Y8950 0.8287 / lvl 0.998 / −0.5 cents, YMF262 0.9898 / lvl 1.003 / −0.0
cents. The YM3812 and YMF262 medians sit a shade under their shared-core
0.99 bar with vibrato files in the sample — a pre-existing red the control
run confirms predates this change, not a product of it.

The residual 0.75 band is the cross-core one: VGMPlay 0.52 offers no core
choice for the YM3526 (no `Core =` line exists for it; it always plays MAME
fmopl) against our Nuked-OPL3 compat mode — the Y8950's situation (0.83),
minus its ADPCM half. The threshold row now holds 0.70 with that reason.

## 2026-08-12 — YM3812/YMF262: the sub-bar medians are free-running state

The clock-projection work left the pre-existing red on the two shared-core
OPL rows (YM3812 0.9771, YMF262 0.9898, both under `shared()`'s 0.99) as an
open question, with one oddity: the YM3812's "LFO off" subset scored
*lower* (0.9552) than its full median, which read as a fault the vibrato
story could not explain. Per-file attribution (a new
`VGMSTUDIO_PARITY_FILES=1` switch prints every rip's row) resolved it:

| YM3812 rip | corr | vib share | the rest of the class |
|---|---|---|---|
| Battlantis 01/02 (3 MHz) | 0.8304 / 0.5463 | 0.78 / 0.64 | vibrato-heavy (now in tune: +0.5 cents) |
| Lychnis 11 | 0.9056 | 0.22 | + rhythm mode (3689 writes) |
| Space Chase 03 | 0.9390 | 0.06 | + deep tremolo (DAM), AM on 13% of ops |
| **Simpsons 24** | **0.9499** | **0.00** | **rhythm mode (320 writes), AM 2 ops** |
| **Fury of the Furries 09** | **0.9552** | **0.00** | **rhythm mode (1640 writes), deep DAM+DVB** |
| Flashback 16 | 0.9957 | 0.00 | rhythm only briefly (17 writes) |
| the rest | 0.977–0.989 | 0.04–0.38 | |

The oddity was the statistic, not the chip: `modulation_share` reads only
the vibrato bit (0x40 of the operator's 0x20–0x35 byte), but vibrato is one
member of the class of **state that free-runs from reset** — the shared
vibrato/tremolo LFO, and rhythm mode's noise LFSR. The two "LFO off" low
scorers are exactly the two heavy rhythm rips: the noise phase starts
wherever each player's reset left it, precisely the mechanism already on
file for the SAA1099 ("the two noise generators' phase") and the SN76489's
noise band. The YMF262 side is the same story dominated by vibrato alone
(Giten Misty: vib 0.79 → 0.5892; steady files 0.9978–0.9988), rhythm-free.

**No driver fault** (option c ruled out): every sub-0.99 rip is accounted
for by the class, levels sit at 0.999–1.045, cents at 0.0–1.0, and the rips
that touch the class most lightly (brief rhythm, one-percent AM) hold
0.9957–0.9988 — the control group's pipeline band.

**Taken: (b) with the statistic fixed.** The scorecard's steady-subset
filter now tests the whole class (`touches_free_running_state`: AM|VIB bits,
plus 0xBD bit 5 on the OPL rows), so "steady" again means "a shared core can
be near-identical here"; the per-file line prints `vib` honestly plus a
`free-running` marker. The `shared()` rows for YM3812/YMF262 become
known-gap rows — floors 0.95/0.97 under the observed 0.9771/0.9898 — because
a strided-sample median that includes free-running rips can never stably
clear 0.99, and which rips those are is the sample's business, not the
driver's. The near-identity claim lives where it is provable: the control
group (0.9978 asserted on vibrato-free files) and the steady-subset line.
Option (a) — asserting on the steady subset — was declined: the subset can
be empty for a sample, and an assertion that quietly judges nothing is the
"no threshold" hole again. The confirming run proved the point immediately:
under the whole-class filter, *all 24* sampled OPL rips carry the
free-running marker — a strided OPL sample with a genuinely steady file in
it is the exception, not the rule.
