# Verified, per-track optimisation

**Branch:** `optimizer-rework-2026-08` · **Status:** stages 0–4 **complete**.
Stage 0 landed the deterministic cores (`1655060`, `8319882`); stages 1–4 landed
the render-verify seam, the per-track UI, the Settings switches and the
speculative hold-backs. Corpus numbers in the *Addendum* below.

## Why

Today the optimiser pipeline (`optdac` → `vgm_sro` → `vgm_cmp` → built-in) runs
inside the pack export, on every song, on every export, with no runtime check
that the output still plays the same audio. The safety story lives entirely in
offline corpus tests (`optimize_parity`, the built-in gate) plus blanket
hold-backs for the chips the tools are known to hurt (`vgm_sro` on QSound /
K053260 / SegaPCM; `vgm_cmp` on SAA1099). That means:

- a tool bug on a chip the corpus under-samples ships silently;
- the hold-backs deny *every* file of a chip because *some* files corrupt;
- the cost of optimisation is paid repeatedly (each export), invisibly, and
  cannot be verified without leaving the app.

This programme inverts the model: **optimisation becomes an explicit, per-track
action whose output is verified by rendering** — the original and the optimised
file are rendered through the real engine and must produce identical samples
before the smaller file is accepted. The export-time pass remains as the bulk
convenience, unchanged. The render gate is the same oracle the corpus tests
already trust: `VgmEngine` applies writes immediately at wait-boundaries, so a
sample difference is a real state change, never a phase artifact
(`docs/optimizer-2026-08/PLAN.md`, *Findings*).

The owner's original prompt asked for four things, addressed as follows:
render-and-compare (stage 1), explicit per-track triggering (stage 2),
DAC-before-`vgm_cmp` ordering (already true in the tools path — see D-orw-6),
and user-selectable stages (stage 3).

## Stage 0 — deterministic cores *(complete)*

The render gate was impossible for NES APU and Game Boy DMG files: those libvgm
cores called C `rand()` (NES noise/triangle phase at reset, SameBoy's DMG
phantom wave-RAM reads mid-render), so the same file rendered differently
twice. Fixed by redirecting every core's `rand`/`srand` to one deterministic
thread-local LCG, reseeded at each chip reset
(`crates/vgms-cores-libvgm/src/rng.rs`, force-included `shim/rand_shim.h` —
**not** `-Drand=`, which trips MSVC's dllimport). Re-evaluated over the corpus:
NES 60/60, GB 60/60, strided all-chip sweep — 0 non-deterministic files, 0
audio changes. The corpus tests' render-twice determinism baseline was removed
as dead code. Every chip is now judgeable by a render diff.

## Decisions

- **D-orw-1 — verification compares samples in memory, not files on disk.**
  No temporary WAVs: two renders are equal iff their sample streams are equal,
  and streaming the comparison allows an early bail at the first differing
  sample. Comparison is byte-exact on interleaved stereo `i16` at one fixed
  rate — no tolerance, ever (a tolerance is how corruption sneaks through).
- **D-orw-2 — the two sides render on separate threads, never interleaved on
  one.** Stage 0 made the RNG stream *thread-local*, reseeded at chip reset. Two
  engines alternating on a single thread would draw from one shared stream and
  desynchronise each other — false positives by construction. Each side gets
  its own thread (its own stream, identically seeded at engine construction);
  chunks meet at a bounded channel for comparison. This also caps memory at a
  few chunks instead of two whole songs. A regression test pins the rule.
- **D-orw-3 — coverage is the intro plus one full loop pass** (the whole song
  when unlooped), not a fixed 8 seconds: a dropped write can matter only on the
  second pass through the loop, when chip state differs from the first
  approach. Guard the pathological header (absurd `total_samples`) with a
  ceiling of 30 minutes of audio per side, logged when it bites — no silent
  caps.
- **D-orw-4 — a failed verification keeps the original bytes and names the
  stages.** Never fatal, mirroring the pipeline's own `StageOutcome` ethos: the
  user sees "kept original: render differed after vgm_cmp", and the file is
  untouched. (Per-stage bisection — re-run with stages disabled to attribute
  blame — is deferred; it multiplies renders.)
- **D-orw-5 — per-track optimise writes back in place, after the gate.** The
  precedent is the pack screenshot optimise (`PackService::optimize` →
  `image_optimized` writes the smaller PNG to its path). The render gate is
  precisely the safety net that makes rewriting the user's rip folder
  defensible; without it, in-place would be reckless. Export-time optimise
  keeps its current semantics (zip only, source untouched, ungated, fast) —
  re-running the pipeline over an already-optimised file is its idempotent
  second pass, so the two paths compose.
- **D-orw-6 — DAC ordering is already correct in the tools path; the built-in
  is where it would go wrong.** `optdac` runs first, per the wiki order the
  pipeline module documents. But the *built-in* pass runs **last**, and under
  `BuiltInOnly` no DAC cleanup happens at all. If a built-in DAC-run collapse
  is ever written (the file-level sequel to the playback-side `push_collapsing`
  of `55256b9`), it must be inserted as a pre-`vgm_cmp` stage, not appended to
  the built-in finisher. Recorded here; deferred with part 3.
- **D-orw-7 — verification is native-first.** The engine compiles to wasm, but
  doubling render time inside the pack worker is a real cost and the web pack
  path has no per-track action yet. The web export stays ungated for now.
- **D-orw-8 — the gate turns hold-backs into try-and-verify, per file.**
  `vgm_sro` on QSound corrupted 12 of 23 corpus files — the other 11 shrank
  safely and are today denied anyway. Under the gate, a held-back stage may run
  *speculatively in the verified path only*: keep the result iff the render
  matches. Blanket denials remain in the unverified (export) path.

## Stages

### Stage 1 — the render-verify seam *(complete)*

- **s1-1** `vgms_synth::verify`: `renders_identically(original: &VgmFile,
  candidate: &VgmFile, opts) -> Verdict`, the runtime twin of
  `optimize_parity.rs`'s render-and-diff. Two threads, chunk-lockstep over a
  bounded channel, first-difference early bail, D-orw-3 coverage. `Verdict`
  carries `Identical` or `DiffersAt { sample, of }`.
- **s1-2** Tests: identical-file smoke; a mutated file (one register value
  changed mid-song) is caught; a file differing only past the loop's first
  pass is caught (the D-orw-3 case); the D-orw-2 regression (two engines on
  one thread would false-positive — assert the seam's threading holds).
- **s1-3** A verified wrapper `optimize_verified(bytes, options, tools) ->
  VerifiedOptimized`, composing `optimize_vgm_with` + the s1-1 gate + D-orw-4's
  keep-original fallback. It lives in the layer that already links both halves
  (`vgms-ui`'s optimize module / the pack service in `vgms-app`), **not** in
  `vgms-vgmtools`: that crate is GPL-2.0-or-later (it embeds the GPL tools) and
  its Cargo.toml records that `vgms-core`/`vgms-synth` stay MIT/Apache and
  never meet it — the licence wall, not taste, fixes where this code goes.

**Exit:** the seam is unit-tested, and a corpus spot-check through
`optimize_verified` accepts what `optimize_parity` accepts and rejects a
seeded corruption.

### Stage 2 — explicit per-track optimisation *(complete)*

- **s2-1** `PackService` grows the song twin of the image path:
  `optimize_song(name, bytes, options)` + a poll, running pipeline + verify off
  the UI thread (native crates, hence `PackService` not `TaskService`).
  Progress narration reuses the `PackScanProgress` status-bar pattern.
- **s2-2** Pack mode UI: an Optimize action per track (and an all-tracks
  sweep), result written back in place per D-orw-5, with the log lines
  `optimize_song_logged` already produces surfaced in the pack log. A savings
  column ("−19.6%" / "optimal" / "kept: render differed") beside Peak, carried
  on `TrackEntry` so the table stays cheap per frame.
- **s2-3** The editor's Edit > Optimize becomes verified too: same wrapper,
  result reported in the status line (`app_status_optimized` gains the
  verified/kept-original arm). VGM-only refusal for DROs stays.
- **s2-4** Help dialog: new actions land in its tables in the same change
  (house rule).

**Exit:** a track optimised from pack mode is smaller on disk, verified, and
survives a rescan; a corrupting stage (forced via a test `Tools` impl) leaves
the file untouched and says why.

### Stage 3 — choosing what runs *(complete)*

- **s3-1** Surface the switches that already exist but are hard-coded `true`
  from the UI: `Options { sample_roms, dac_runs }`, plus the existing
  `OptimizerChoice`. One "Optimization" group (Settings > Output, where the
  Optimizer combo already lives): the combo as-is, plus "Trim sample ROMs
  (vgm_sro)" and "Collapse DAC runs (optdac)" checkboxes, persisted in config,
  honoured by both the per-track path and the export.
- **s3-2** Thread the two booleans through `PackJobRequest` (+ web codec u8/bool
  tags and the worker, as `OptimizerChoice` was threaded in part 2 of the
  previous programme) so export and per-track honour the same choice.

**Exit:** unchecking a stage provably skips it (its `Skipped` line in the pack
log), settings round-trip, UI snapshot updated once.

### Stage 4 — put the gate to work *(complete)*

- **s4-1** Try-and-verify the hold-backs (D-orw-8) in the verified path:
  `vgm_sro` on QSound / K053260 / SegaPCM, `vgm_cmp` on SAA1099. The pipeline
  gains a "speculative" stage mode the verified wrapper enables; the unverified
  path keeps today's denials. Measure on the corpus: how many previously-denied
  files now shrink safely.
- **s4-2** The `vgm_cmp` first-pass YM2612 corruption (~5% of files) is now
  caught per file instead of tolerated: confirm with a corpus run that the
  verified path keeps originals exactly where `optimize_parity` flags
  differences, and close the "known interim limitation" note in
  `docs/optimizer-2026-08/PLAN.md` (D-opt-4) with a pointer here.

**Exit:** corpus numbers in this doc's addendum: files recovered by s4-1,
corruptions caught by s4-2, wall-clock cost of a verified all-tracks sweep on a
representative pack.

## Addendum — what shipped

The four stages landed as these commits on the branch:

- **Stage 1** — `vgms_synth::verify` (`renders_identically` / `Verdict`, two
  threads over a bounded channel, intro + one extra loop pass, 30-min ceiling)
  and the render-gated wrapper `vgms_ui::optimize::optimize_verified`
  (`VerifiedOutcome`), with the licence wall keeping the composition out of
  `vgms-vgmtools`.
- **Stage 2** — `PackService::optimize_song` off the UI thread; a per-track
  **Optimize** row action and an **Optimize All** serial sweep (native-only,
  D-orw-7), written back in place with a reversible transaction; a savings
  column on the Tracks table (`PackState::optimize_results`, keyed by file
  name); Edit > Optimize verified (`OptimizeVgmOutcome`); the two gestures in
  the Help dialog.
- **Stage 3** — `AppConfig::optimize_sample_roms` / `optimize_dac_runs` (both
  default on), a Settings **Tool stages** group, threaded through the per-track
  path, the editor, the pack export (`PackJobRequest` + `build_pack_zip`) and
  the web codec/worker.
- **Stage 4** — `Options::speculative`, set by `optimize_verified` only, lifting
  the per-chip hold-backs under the render gate while the bottomless-ROM hang
  guard stands.

### Corpus measurement

`vgms-app/tests/optimize_verified.rs ::
the_verified_path_recovers_holdbacks_and_catches_corruptions` runs the verified
(speculative) path — forced `optimizer: Tools` — over a corpus subtree and
tallies what the render gate then does; the `optimize_parity` suite is the
non-speculative baseline. Measured over `F:/…/VGMRips_all_of_them_2025-10-17`
(release, 2026-08-24), by subtree:

| subtree (chips)                       | files | accepted | unchanged | held-back tried→kept / recovered | YM2612 caught | safe wrongly kept | wall  |
|---------------------------------------|------:|---------:|----------:|:--------------------------------:|--------------:|------------------:|------:|
| root, first 150 (OPL / SN / YM2151)   |   150 |       51 |        97 |             2 → 2 / 0            |             0 |                 0 | 170 s |
| `MegaDrive` (YM2612 + SN76489)        |   120 |        3 |         5 |               — / —              |       **112** |                 0 | 122 s |
| `Arcade/Capcom` (CPS1: YM2151+OKI)    |   120 |       40 |        80 |               — / —              |             0 |                 0 | 109 s |
| `Arcade/Capcom/AvP` (CPS2: QSound)    |    26 |        0 |        24 |             2 → 2 / 0            |             0 |                 0 |  16 s |
| `Arcade/SegaSys` (SegaPCM + FM)       |   120 |        3 |       111 |             6 → 6 / 0            |             0 |                 0 | 494 s |

- **s4-2 — corruptions caught.** On the Mega Drive subtree the gate kept **112
  of 120** YM2612 files back: the tools' first-pass OPN2 corruption, caught per
  file instead of shipped. This is not a loop-coverage artifact — the
  independent `optimize_parity` (8 s, no loop) run over the *same* 120 files
  fails its "renders identically" assertion too. The verified path is in fact
  *faster* there (122 s vs 341 s) because it early-bails at the first differing
  sample where parity renders the full window. The non-YM2612 arcade subtree
  (Capcom CPS1, YM2151 + OKIM6295) is the control: **0** kept back, 40 optimised
  cleanly — the tools are trusted there and the gate agrees.
- **s4-1 — hold-backs recovered.** Across the sampled subtrees the speculative
  path *tried* a held-back stage (`vgm_sro` on QSound / SegaPCM) on **10** files
  and the gate kept the original on all 10 — every trim that actually shrank a
  file also changed its render. **0 recovered** here: on this corpus the
  hold-backs were justified, and the speculative mode's worth is that it now
  *decides per file* (try, verify, keep-or-reject) rather than denying blind. A
  QSound / SegaPCM pack where the trim shrinks safely would recover there; none
  of the sampled packs was one. The `optimize_parity ::
  which_chips_the_sample_rom_trim_is_safe_for` table remains the place to widen
  the allowlist from evidence.
- **The one hard invariant** the test asserts held on every subtree: a file with
  no held-back chip and no YM2612 — no stage the tools are known to get wrong —
  is **never** kept back (0 across all 536 files), so the safe majority
  optimises exactly as before.
- **Wall-clock.** Per file the verified optimise averages ~1 s (the tool child
  processes plus a render only when the pass shrinks); a corrupt YM2612 file is
  cheap (early-bail), a long clean-shrinking SegaPCM track is the dear end (the
  SegaSys 494 s / 120 is those). A representative pack of 20–40 tracks verifies
  in tens of seconds — most tracks are already optimal and never render at all.

## Deferred

- **The built-in DAC stage** (D-orw-6's pre-`vgm_cmp` placement) — belongs to
  the previous programme's part 3 (widening built-in coverage), not started
  without the owner.
- **Web verification** (D-orw-7) and a web per-track action.
- **Per-stage blame bisection** on a failed verification (D-orw-4).
- **Export reusing per-track results** (skip already-verified tracks) — only
  worth it if the export's second pass ever measures as slow.

## Gating discipline

Per commit: `cargo fmt --all` → `cargo clippy --workspace --all-targets -- -D
warnings` → tests for the touched crates (wasm clippy for `vgms-ui`/`vgms-web`
changes). At the corpus, per stage: `optimize_parity`, the built-in gate, and
`optimize_corpus` all green. UI snapshots regenerate only where a change means
to move pixels.
