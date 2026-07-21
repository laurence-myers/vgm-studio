# HANDOVER — Multi-song splitter (`vgm_sptd` equivalent; plan complete, implementation not started)

Written 2026-07-21 for a fresh Claude session on the `rust` branch of
`I:\Code\Python\dro-trimmer`. Verify file:line references before leaning on
them. Companion plans: `docs/vgm-cmp-2026-07/` (optimizer — shares the OPL
state tracker, see §3.3), `docs/vgm-lpfnd-2026-07/`, `docs/vgm-vol-2026-07/`.

## 1 · The feature

Split one long VGM capture — a whole sound-test session logged in one file —
into individual per-song files at the silence gaps, producing the numbered
`NN Title.vgm` set that rip mode expects. This is the missing entry path for
"I logged the entire soundtrack in one go".

UI vision (the "nice UI" requirement): **File ▸ Split Songs…** opens a dialog
with a gap-threshold control (in seconds) and a live boundary list — detection
is a single cheap pass, so dragging the threshold re-fills the table
instantly. Table: segment #, start time, length, an include checkbox per
segment (drop false positives without leaving the dialog), and a Preview
button that seeks playback to the segment start. Export picks an output
folder (the existing Split Channels picker flow), writes `NN <stem>.vgm`
files, and finishes with a status offer the user can act on: open the folder
as a rip project — one click from raw log to the rip-mode track table.

## 2 · Decisions to confirm at kickoff

1. **Chip-state replay at segment starts** — recommend ON always (§3.2):
   segments start with the register state captured from playing the stream to
   that point, prepended as writes. Sound-test songs usually re-init, but
   "usually" is not a guarantee, and vgm_sptd itself gets state safety from
   its shared `TrimVGMData`. Requires the `OplState` tracker planned in
   `vgm-cmp` lf the optimizer lands first, share it; otherwise build it here
   and let the optimizer reuse it (either order works, note the dependency).
2. **GD3 per piece** — recommend copying the source GD3 to every piece
   (titles then fixed up in rip quick-edit / bulk tag); vgm_sptd drops it.
3. **Default threshold** — vgm_sptd uses 0x8000 = 32768 samples ≈ 0.74 s.
   Recommend the same default, surfaced in seconds (0.75 s) with a sensible
   range (0.2–5 s).
4. Licensing: same Route A/B framing as `vgm-cmp` §2.2.1; Route B trivially —
   gap detection is one accumulator.

## 3 · Domain facts (verified 2026-07-21)

### 3.1 How `vgm_sptd` behaves (behavioural digest from vgmtools source)

- Accumulates delay samples across consecutive delay commands (`CmdDelay`),
  reset by any real (non-ignored) command; a split triggers when
  `CmdDelay >= SplitDelay` (default 0x8000).
- Pieces are produced by the shared `TrimVGMData(start, loop, end, …)` —
  the same machinery as `vgm_trim`, which re-establishes chip register state
  at the piece start; leading silence is trimmed (`VGMSmplStart =
  LastCmdDly`) and the trailing gap up to the boundary is kept.
- Naming `basename_NN.vgm` (`DIGIT_COUNT` default 2); empty pieces guarded
  (`if (TempLng > VGMSmplStart)`); GD3 not copied into pieces.

### 3.2 Adaptation to this codebase

- Detection is over instructions: sum `DelaySamples` across *consecutive*
  delay instructions (data.rs `DroInstruction::DelaySamples`); any register
  write resets the accumulator. A boundary is (first delay index of the gap,
  first non-delay index after it): the segment ends before the gap, and the
  next segment starts at the first real command after it — vgm_sptd's
  leading-silence trim falls out of that choice naturally. Keep the trailing
  gap out of pieces too (cleaner than vgm_sptd; each piece then ends at its
  last real command — confirm at kickoff if the trailing second should be
  kept for reverb-ish decay; recommend keeping ~one 0x61 max… simplest:
  keep no trailing gap, decay tails are register-silence anyway).
- Segment materialisation: for segment `[a, b)`, capture `OplState` by
  scanning instructions `[0, a)` (per §2.2.1), emit the minimal register
  writes that recreate it (registers actually touched, in ascending order,
  bank-aware), then the segment's instructions verbatim; wrap with
  `synthesise_header()` (vgm/io.rs:193) + the source song's `opl_type` and
  optional GD3 clone; `loop_point = None`. Serialise via the normal writer
  (header fields all recomputed there).
- Splitting is fast (no rendering), but files are written through the
  existing background split flow so the UI stays live and cancellable.

### 3.3 This codebase (the load-bearing specifics)

- **The precedent to mirror end-to-end** is Split Channels:
  `dialogs/split.rs` (options dialog) → `Action::SplitSubmitted` →
  `pick_output_folder` / `poll_output_folder` (platform.rs:98-110 in the fake;
  app.rs `split_into`) → `TaskKind`-keyed background job (tasks.rs:164
  `split_to_bytes`) → per-file `SaveRequest::InPlace` writes. Clone that
  pipeline for Split Songs (`TaskKind::SplitSongs`,
  `Action::SplitSongsSubmitted { threshold_samples, included: Vec<bool> }`).
- **Boundary detection lives in dro-core** (`split_songs.rs`, pure +
  wasm-clean): `detect_segments(song, threshold_samples) -> Vec<Segment>`
  (start/end indices + start-time/length in samples for the UI), and
  `materialise(song, &segment, state_replay: bool) -> Song`.
- **OplState tracker**: shared design with the optimizer plan
  (`docs/vgm-cmp-2026-07/HANDOVER.md` §3.2 — per-chip/per-port register
  files, `Bank::High` = second OPL2 chip under DualOpl2, OPL3 port 1
  otherwise). Whichever feature lands first owns
  `crates/dro-core/src/opl_state.rs`.
- **Preview**: seek-and-play exists (`Action::WaveformClicked` seeks; the
  transport supports play from position — see `do_play` + `seeks_ms` in the
  audio service). Preview = seek to segment start + play; stopping at the
  boundary is a nice-to-have (skip unless trivial).
- **Rip handoff**: after export, `files.open_folder_path(chosen_dir)`
  (app.rs `rescan_rip_folder` precedent) installs the folder as a rip
  project — gate behind a "Open as rip project" button on the completion
  status/alert.
- **Naming**: `track_file_name(number, title, ext)` (dro-core/src/rip.rs:253)
  already builds `NN Title.ext`; title = source stem (or GD3 track name)
  until quick-edit renames.

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

### spd-1 · dro-core: detection + OPL state capture

`split_songs.rs::detect_segments` (accumulator over consecutive delays,
threshold param, empty-segment guard) and `opl_state.rs` (register files +
`replay_writes()` emitter) unless the optimizer already created it. Tests:
gaps spanning several delay instructions found; gaps interrupted by a write
are not boundaries; threshold edge cases; segments exclude leading/trailing
gap; state capture equals a naive replay of all writes (property test vs a
direct register-file fold); wasm-clean.

### spd-2 · dro-core: segment materialisation

`materialise` per §3.2 (state prelude + verbatim body + synthesised header +
GD3 clone). Tests: piece round-trips through `read_song`; register state at
the first note of piece N equals the original stream's state at that
instruction (assert via the tracker); a piece from offset 0 gets no prelude;
GD3 copied; total piece durations sum to the original minus gaps.

### spd-3 · the Split Songs dialog + job

`dialogs/split_songs.rs` (threshold slider in seconds with live re-detect,
boundary table with include-checkboxes and Preview buttons, footer
Export/Close), File-menu item gated on a loaded song (both formats? VGM only
first — DRO segments would need the DRO writer path; recommend VGM-only
initially, note DRO as follow-up). `TaskKind::SplitSongs` job clones the
Split Channels save loop, naming via `track_file_name` with a running NN over
*included* segments. GUI tests: threshold changes re-fill the table; exclude
drops a file; export writes the expected names/count via the fake file
service; cancellation. Snapshot `split_songs_dialog`.

### spd-4 · rip handoff + polish

Completion alert with "Open as rip project" → `open_folder_path`; status
totals ("Wrote 12 songs"). Corpus sanity: run on one real multi-song log
(log one with DOSBox if none on disk; a synthetic concatenation of corpus
tracks with 1 s gaps otherwise) and listen to piece 2+ for state-replay
correctness. Update `TODO.md` + the `vgmrips-pack-gaps` memory when it lands.

## 6 · Where everything lives

| Concern | Path |
| --- | --- |
| Detection + materialise | `crates/dro-core/src/split_songs.rs` (create) |
| OPL state tracker (shared with optimizer) | `crates/dro-core/src/opl_state.rs` (create; see vgm-cmp plan) |
| Synthesised header / writer | `crates/dro-core/src/vgm/io.rs:193, :111` |
| Pipeline to mirror | `dialogs/split.rs`, `tasks.rs:164`, `app.rs` `split_into` |
| NN naming | `crates/dro-core/src/rip.rs:253` |
| Rip handoff | `app.rs` (`open_folder_path` / `rescan_rip_folder`) |

## 7 · Sources

- vgmtools `vgm_sptd.c` (GPL-2.0) — behaviour digest fetched 2026-07-21:
  https://github.com/vgmrips/vgmtools (Route B; note its pieces get state
  safety via the shared `TrimVGMData`, which is why §2.2.1 recommends replay).
- `docs/vgm-cmp-2026-07/HANDOVER.md` §3.2 — the OPL state model both plans
  share (per-chip/per-port register files; DualOpl2 vs Opl3 bank mapping).
