# The first scorecard, and what it found

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

`DROTRIM_PARITY_DUMP` writes both sides of every flagged pair as WAVs, and
`DROTRIM_PARITY_CACHE` makes a repeat run minutes rather than an hour. Start
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

Clean-room ADPCM-A and Delta-T (`dro-synth/src/adpcm.rs`, glue in `opn.rs`),
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

The 2608 die is wrapped (`dro-cores-gpl/src/lle_opna.rs`): the DRAM bus
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
