# The pinned reference, and what it took to make it one

PARITY-PLAN §1 asks for the reference version, its config and its hash to be
recorded, "so a re-run years later compares against the same thing". This is
that record, plus the four behaviours that had to be discovered by experiment
before the reference could be driven at all — none of which are in its manual,
and every one of which would have quietly corrupted the comparison rather than
failing it.

## What is pinned

| Thing | Value |
|---|---|
| Player | VGMPlay 0.52 (Valley Bell), Windows build |
| Executable | `VGMPlay64.exe`, 948224 bytes, SHA-256 `106c0306…a72d3d73f` |
| (32-bit, unused) | `VGMPlay.exe`, SHA-256 `8b137e8f…b13227aa` |
| Its zlib | `zlib1.dll`, SHA-256 `4a2561f5…16a1e670` |
| Config | `VGMPlay.ini` in this directory, SHA-256 `39852341…1a1345bf` |

The binary itself is **not** in the tree — it is the user's own copy, named by
`VGMSTUDIO_REF_PLAYER`. Only the config is checked in, and the hashes above are
what identify the build that produced the numbers in the harness.

Set up:

```bash
VGMSTUDIO_REF_PLAYER=/path/to/VGMPlay64.exe
VGMSTUDIO_REF_CONFIG=docs/vgm-multichip-2026-07/parity/VGMPlay.ini
VGMSTUDIO_VGMRIPS_CORPUS=/path/to/vgmrips
VGMSTUDIO_PARITY_CACHE=/some/scratch   # optional; renders are deterministic
VGMSTUDIO_PARITY_DUMP=/some/scratch    # optional; writes both sides of a flagged pair
```

## The four discoveries

**1. It reads its ini from the executable's directory, not the working one.**
A pinned config placed where the process is started has no effect at all. The
run does not fail — it silently uses whatever settings the installation
happens to hold, which is precisely the unreproducibility that pinning exists
to prevent. The runner therefore *stages* the player: it copies the executable,
its zlib and the pinned config into the work directory and runs that copy. The
user's installed configuration is never read and never modified.

**2. An empty `LogPath` means "beside the input file".** Not "the current
directory", which is the natural reading and the one the first implementation
took. The result was an eight-megabyte WAV deposited in the corpus directory
the harness was reading from. `LogPath` is now rewritten to the work directory
on the way into the staged config.

**3. Every `Core =` is empty in the stock ini, and empty means the vendor
default** — AdLibEmu for YMF262, Genesis Plus GX for YM2612. A "shared-core"
comparison against those is a comparison of two unrelated emulators that
attributes the difference to us. The pinned config names a core for every chip
we emulate: `NUKE` for the five we share upstream with, and the stock default
written out explicitly for the clean-room ones so that a future VGMPlay's
changed default cannot move the reference without anyone noticing. This was
worth about five points of correlation on the OPL control group.

**4. Determinism is a property of the chip, not of the player.** The first
determinism check drew a YMF262+YMZ280B rip and the reference disagreed with
itself across two runs on 0.9% of samples, at full scale — the signature of a
PCM chip reading sample memory it was never given. The check is now made once
per chip, on the same single-chip files the comparison uses. All fourteen chips
we emulate pass, PCM included; the flaky one is a chip we do not emulate.

## Rendering at the chip's rate

Both sides render at the **chip's native rate**, which is derived from the clock
in each file's header and so is asked per file (`ChipCore::native_rate`, and
`SampleRate` rewritten into the staged config by `Reference::at_rate`).

This is not a refinement. At 44100 the OPL control group — whose core is proven
bit-identical to the one the reference runs — scored **0.836**, and its
alignment slid three or four frames between the start of a file and its end.
That is two resamplers disagreeing, and nothing about it is visible in the
headline correlation: `metrics::lag_drift` exists to tell an even, phase-blind
resampler difference (alignment unmoved) from a rate difference (each window
aligns, the file does not). At the native rate the same files score 0.985–0.997.

## The residual, and why it is not ours to close

What is left on the control group is the OPL3 **vibrato LFO's phase**. Files
that never switch vibrato on score 0.998 and 0.999; a file where 46% of
operator writes set the vibrato bit scores 0.590. The spectrum shows both
renders putting the same energy in the same partials, with each partial's
instantaneous frequency wobbling on a different schedule — the LFO free-runs
from chip reset and the two sides start it at different points relative to the
music. Average pitch is identical to 0.0 cents, so nothing is out of tune; the
waveforms are simply not the same waveform, and no amount of pipeline work will
make them so.

The control group therefore asserts on vibrato-free files, where "the same core
driven the same way" is actually true, and still scores and prints the rest.
The same effect should be expected on any chip with a free-running LFO —
YM2151's PMS/AMS and YM2612's LFO are the obvious next ones.
