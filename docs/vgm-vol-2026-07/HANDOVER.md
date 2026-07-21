# HANDOVER — Volume measurement (`vgm_vol` equivalent; plan complete, implementation not started)

Written 2026-07-21 for a fresh Claude session on the `rust` branch of
`I:\Code\Python\dro-trimmer`. Verify file:line references before leaning on
them. Companion plans: `docs/vgm-cmp-2026-07/` (optimizer),
`docs/vgm-lpfnd-2026-07/` (loop finder), `docs/vgm-sptd-2026-07/` (splitter).

## 1 · The feature

Measure a song's true peak level by rendering it internally through dro-synth,
then put the number to work three ways:

1. **Suggest the VGM header volume modifier** (the pack-QA step `vgm_vol`
   exists for — VGMRips wants consistent loudness across a pack).
2. **Auto-set the playback boost** (user-requested): a quiet song gets its
   boost dialed to exactly the factor that brings its peak to full scale,
   instead of the user nudging the stepper by ear.
3. **Rip mode: scan the whole pack**, show a per-track peak column, and offer
   one-click album normalization via each track's volume modifier.

`vgm_vol` measures WAVs the user had to render first; rendering internally
removes that whole step and is sample-exact.

UI vision (the "nice UI" requirement): a **Measure** button inside the VGM
Metadata dialog that fills the volume-modifier field live (with a peak/dBFS
readout), a **"Set boost to match"** button beside the boost stepper, and in
rip mode a **Scan Volumes** button that fills a Peak column in the track table
plus an **Apply suggested modifiers** action that rewrites all tracks as one
undoable batch.

## 2 · Decisions to confirm at kickoff

1. **Album vs track normalization default in rip mode** — recommend album
   mode (`vgm_vol`'s `MaxLvlAlbum`): scale every track by the same factor so
   the loudest track peaks at full scale, preserving the game's relative
   levels. Per-track normalization offered as a checkbox.
2. **Whether playback honors the header volume modifier** — it currently does
   NOT (§3.3). Recommend leaving playback semantics alone this feature (boost
   is the playback lever; the modifier is metadata for other players), and
   noting a follow-up if the user wants the engine to apply it.
3. **Boost auto-set rounding** — boost range is 1.0..=16.0 (config.rs:166);
   recommend clamping the exact factor into range and NOT quantizing to the
   stepper's increments (the stepper displays whatever value it is given).
4. Licensing: same Route A/B framing as `vgm-cmp` §2.2.1; Route B trivially —
   the formula below is arithmetic, nothing to transcribe.

## 3 · Domain facts (verified 2026-07-21)

### 3.1 How `vgm_vol` behaves (behavioural digest from vgmtools source)

- Input: WAVs (or an M3U whose entries it retargets to `.wav`).
- Measurement: peak of |16-bit samples| (`ReadWAVFile`, `MaxLvl`); clipping
  flagged at `MaxLvl >= 0x7FFF` with advice to re-render at half volume.
- Output formula (`PrintVolMod`):
  `Factor = 0x8000 / MaxLvl * RecVolume`;
  `VolMod = floor(log2(Factor) * 0x20)` — i.e. **32 steps per doubling**.
- Album mode tracks the max across all files so one factor serves the pack.

### 3.2 The VGM header field this feeds

`volume_modifier` byte at 0x7C (v1.60+; stored in this app's `VgmMeta`,
data.rs:344, editable in the VGM Metadata dialog, written verbatim by
`vgm/io.rs` write()). Spec semantics: players scale output by
`2^(value/0x20)`, where the byte is signed-ish: `0x00..=0xC0` = 0..192
(gain), `0xC1..=0xFF` = −63..−1 (attenuation). The suggestion must encode
negative results into that split range and clamp to it; keep the raw-u8
storage untouched.

### 3.3 This codebase (the load-bearing specifics)

- **Rendering for measurement**: `dro_synth::wav::render_wav_cancellable`
  (wav.rs:88) renders the mixed song with progress + cancel — but allocates
  the whole buffer. Prefer a new streaming
  `measure_peak(song, rate, is_cancelled, progress) -> Option<Peak>` in
  dro-synth that drives `PlayerEngine::render` (engine.rs:552) chunkwise and
  tracks `max |sample|` — no allocation, same cancel/progress shape as the
  waveform task. `Peak { max_level: i16, clipped: bool }`.
- **Playback boost**: `BoostLimiter` (dro-synth/src/limiter.rs) — live
  playback only, renders/exports are never boosted (limiter.rs:11). Boost is
  `Action::SetBoost { value, persist }` (action.rs:129), persisted to
  drotrim.ini `[audio] boost` via `AppConfig` (dro-core/src/config.rs:19,
  valid 1.0..=16.0, :166). Auto-set = push one SetBoost with
  `value = clamp(0x8000 / peak, 1.0, 16.0)`, persist=true.
- **Playback does not apply `volume_modifier`**: no reference to it anywhere
  in dro-synth (verified by grep 2026-07-21) — §2.2.2.
- **Task service**: `TaskKind`/`run_task` (dro-ui/src/tasks.rs) for the
  editor-side scan (cancel-on-resubmit, progress snapshots). Rip-mode batch:
  drive one track at a time from the app (queue in `DroApp`, status
  "Scanning 3/12…"), reusing the same task kind — tracks are already parsed
  `Arc<Song>`s (`RipTrack::song`).
- **Rip-mode batch rewrite**: header-only edits re-serialise via the same
  path as bulk tagging — build per-track bytes with the new modifier and run
  one `RipTransaction` of `Write` mutations (see `bulk_tag_submitted`,
  app.rs; undo/redo comes free). `retagged_bytes` (rip.rs:635) is the
  serialisation precedent (clone song, tweak, `write_song`).
- **UI seams**: VGM Metadata dialog (`dialogs/vgm_metadata.rs`) owns the
  volume-modifier field; the boost stepper is `widgets/boost_stepper.rs`;
  the rip track table is `rip.rs::track_table` (add a Peak column fed from a
  `HashMap<file_name, Peak>` on `RipState`).

## 4 · Environment & workflow

PATH prelude before any cargo call:
```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```
Gates per step: `cargo test --workspace`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo fmt --all`; snapshots via
`UPDATE_SNAPSHOTS=1 cargo test -p dro-ui`. Commit per step; autonomous.

## 5 · The plan

### vol-1 · dro-synth: streaming peak measurement

`measure_peak` as in §3.3 (chunked engine render, running max, progress %,
cancellation). Unit tests: a full-scale synthetic song reports ~0x7FFF and
`clipped`; a −6 dB song reports half; cancellation returns None; unity-boost
independence (measurement never goes through `BoostLimiter`).

### vol-2 · dro-core: the suggestion math

Pure helpers (`dro-core/src/volume.rs` or alongside `VgmMeta`):
`suggest_volume_modifier(peak: i16, album_peak: Option<i16>) -> u8` encoding
the §3.1 formula into the §3.2 split-range byte (clamped), plus
`boost_for_peak(peak) -> f32` (clamped 1.0..=16.0). Table-driven unit tests
pinning the exact bytes for known peaks (full scale → 0x00; half scale →
0x20; quarter → 0x40; over-unity attenuation cases → 0xC1..0xFF end).

### vol-3 · editor integration

`TaskKind::VolumeScan` in tasks.rs. In the VGM Metadata dialog: a Measure
button + live peak/dBFS readout + "fill modifier" once the scan lands; beside
the boost stepper (transport row): "Match Volume" button issuing the scan
then `SetBoost { value: boost_for_peak, persist: true }` with a status line
("Peak −8.2 dBFS; boost set to 2.57"). Progress + cancel like the WAV render.
GUI tests: measure fills the field; match-volume sets and persists boost
(assert config store write); DRO songs allowed too (boost path) with the
modifier button VGM-gated. Snapshot of the touched dialog.

### vol-4 · rip mode: scan + normalize

`RipState.peaks` map + Peak column (dBFS, red when clipped) + "Scan Volumes"
button driving the sequential per-track scan (skip unreadable tracks). Then
"Apply suggested modifiers": album-mode factors (§2.2.1) → per-track new
volume_modifier → one undoable `RipTransaction` batch of header rewrites,
skipping tracks whose byte would not change (the `bulk_tag_submitted`
pattern). GUI tests: scan fills the column; apply writes only changed tracks
and lands one undo step; export unaffected otherwise. Snapshots: rip view.

### vol-5 · docs + memory

Update `TODO.md`; mark item "vgm_vol" done in the `vgmrips-pack-gaps` memory
with the commit hash; note the §2.2.2 follow-up (engine honoring the
modifier) there if the user wants it.

## 6 · Where everything lives

| Concern | Path |
| --- | --- |
| New peak scan | `crates/dro-synth/src/` (new fn beside wav.rs) |
| Suggestion math | `crates/dro-core/src/volume.rs` (create) |
| VolMod storage / dialog | `crates/dro-core/src/vgm/data.rs:344`, `dro-ui/src/dialogs/vgm_metadata.rs` |
| Boost action/config/limiter | `action.rs:129`, `dro-core/src/config.rs:19`, `dro-synth/src/limiter.rs` |
| Task service | `crates/dro-ui/src/tasks.rs` |
| Rip table / batch rewrite | `crates/dro-ui/src/rip.rs`, `app.rs` (`bulk_tag_submitted` precedent) |

## 7 · Sources

- vgmtools `vgm_vol.c` (GPL-2.0) — formula digest fetched 2026-07-21:
  https://github.com/vgmrips/vgmtools (Route B).
- VGM spec v1.72, header 0x7C Volume Modifier semantics:
  https://vgmrips.net/wiki/VGM_Specification
