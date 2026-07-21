# HANDOVER — Loop finder (`vgmlpfnd` equivalent; plan complete, implementation not started)

Written 2026-07-21 for a fresh Claude session on the `rust` branch of
`I:\Code\Python\dro-trimmer`. Verify file:line references before leaning on them.
Companion plans: `docs/vgm-cmp-2026-07/HANDOVER.md` (optimizer),
`docs/vgm-vol-2026-07/`, `docs/vgm-sptd-2026-07/`.

## 1 · The feature

Find loop-point candidates automatically: search the command stream for a block
of writes that repeats later in the song, rank the matches, and let the user
audition and apply them with the loop machinery that already exists. Today
loops are found entirely by ear; this turns the fiddliest part of ripping into
"listen to the top three suggestions, click Apply".

UI vision (the "nice UI" requirement): a modeless **Find Loop** dialog (Edit
menu) with a Search/Cancel button and progress bar, a minimum-match-length
control, and a results table — columns Loop start, Loop end, Length, Quality.
Clicking a row sets the editor's loop markers (waveform highlights instantly);
**Audition** = markers + `Action::PlaySeam` (hear the seam on its own);
**Apply** = markers + `Action::ApplyLoopToMetadata`. Every one of those actions
already exists (`action.rs` Loop points group) — the dialog only orchestrates.

## 2 · Decisions to confirm at kickoff

1. **Licensing route** — vgmtools is GPL-2.0, the workspace is still
   LGPL-2.1-or-later. Same Route A/B framing as the optimizer handover
   (`vgm-cmp` §2.2.1); Route B (independent implementation, behaviour-level
   reference only) is again recommended — block matching is textbook, nothing
   needs transcribing.
2. **Format gating** — recommend: dialog available for VGM *and* DRO (markers
   and loop playback are format-agnostic), with the Apply button VGM-gated
   exactly like the "Apply Loop to Metadata" menu item (`app_gui_tests.rs`
   `edit_menu_items`).
3. **Search algorithm** — recommend rolling-hash + verification (§3.2) over
   vgmlpfnd's brute force; confirm the default minimum match length (§3.1).

## 3 · Domain facts (verified 2026-07-21)

### 3.1 How `vgmlpfnd` behaves (behavioural digest from vgmtools source)

- Compares command byte + operand value (`CompareVGMCommand`), **skipping all
  delay commands** (`IgnoredCmd`) — a match is a musical repetition regardless
  of timing-encoding differences.
- Brute force: outer cursor stepped by `STEP_SIZE` (1), inner extension while
  commands match. O(n³)-ish, hence its "searching takes considerable time"
  warning.
- A match is reported only at `MIN_EQU_SIZE` (default 0x400 = 1024 commands).
- Quality flags: `f` = source block ends before the copy begins (a clean
  "loop body then repeat" shape); `e` = the match runs to EOF; `!` = both —
  the ideal candidate.
- Output is positions only; the user manually feeds them to a trimmer. Our
  version closes that gap with Apply.

### 3.2 Adaptation to this codebase

- Interpretation: a match of block A (at `src`) recurring at `copy` means the
  region `[src, copy)` is the loop body — **loop_point = src, loop_end =
  copy** in this app's exclusive-index model (`VgmMeta`, data.rs:323). The
  `e`-flagged case (match extends to EOF) is the strongest evidence.
- Match key: normalize each non-delay instruction to `(opcode, reg, value)`
  via `VgmData::get` / `raw_instruction` (data.rs:162) and search over the
  delay-stripped sequence, carrying a parallel index map back to real
  instruction indices (delays must be *skipped for matching* but the reported
  loop indices land on real command boundaries; snap loop_point to the first
  non-delay instruction of the match).
- Performance: streams here are 10⁴–10⁵ commands. A rolling hash (or suffix
  automaton) over the normalized sequence finds candidate pairs in ~O(n log n)
  with exact verification after; even so, run it through the existing
  background-task machinery so the UI never blocks.
- Ranking: sort by (quality flags desc, match length desc); deduplicate
  overlapping candidates sharing an end position (vgmlpfnd's `EndPosArr`
  trick) so the table shows distinct musical loops, not 50 offsets of one.

### 3.3 This codebase (the load-bearing specifics)

- Background tasks: `TaskService` (`dro-ui/src/tasks.rs`) — tasks keyed by
  `TaskKind`, cancel-on-resubmit, progressive snapshots (see
  `render_waveform_progressive` handling, tasks.rs:130). Add
  `TaskKind::LoopSearch` emitting `TaskResult::LoopCandidates` snapshots as
  matches are found; the pure search fn lives in dro-core (wasm-clean),
  `run_task` calls it with the `is_cancelled` callback.
- Loop markers: `RangeMarkers` (`dro-ui/src/markers.rs`), set via
  `Action::SetLoopStart/SetLoopEnd`; seam audition `Action::PlaySeam`; apply
  `Action::ApplyLoopToMetadata` (editor.rs:389 `apply_loop_to_metadata`).
- Dialog chrome: `dialogs/mod.rs` (`dialog_window`, `dialog_footer`, the
  `Dialogs` registry + `retain` loop); precedent for a results-table dialog:
  the rip track table (`rip.rs::track_table`, egui_extras `TableBuilder`).
- Waveform reacts to markers automatically (revision/marker driven).
- Fixture with a real loop: any corpus pack track with a loop
  (`F:\GameMusic\VGM\{YM3812,YMF262}`); synthesize small fixtures in tests by
  concatenating a block twice with `VgmData` bytes.

## 4 · Environment & workflow

PATH prelude before any cargo call:
```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```
Gates per step: `cargo test --workspace`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo fmt --all`; GUI snapshots via
`UPDATE_SNAPSHOTS=1 cargo test -p dro-ui`. Commit per step, conventional
messages, work autonomously.

## 5 · The plan

### lf-1 · dro-core: the search

`crates/dro-core/src/loopfind.rs`: normalized-sequence builder (delay-skipping,
index map), rolling-hash candidate generation, exact verification, flag
computation (`f`/`e`), end-position dedup, ranking. API:
`find_loops(song, min_len_commands, emit: &mut dyn FnMut(Candidate), is_cancelled)`
— emit-as-found so the UI can stream results. Tests: synthetic
block-repeats-once (found, correct indices, `f`+`e` flags); delay-encoding
differences between body and repeat still match; below-min matches suppressed;
overlap dedup; cancellation stops cleanly; wasm-clean build.

### lf-2 · task wiring

`TaskKind::LoopSearch` + `TaskResult::LoopCandidates(Vec<Candidate>)`
snapshots in tasks.rs `run_task`, cancel-on-resubmit semantics identical to
the waveform task. Tests mirror `a_cancelled_task_produces_nothing`.

### lf-3 · the Find Loop dialog

`dialogs/find_loop.rs` + `Dialogs.find_loop` slot + Edit-menu item "Find
Loop…" (both-formats gating per §2.2; keyboard-suppression comes free from
`Dialogs::any_open`). Search button submits the task with the chosen minimum
length (entered in **seconds**, converted via `delay_samples_prefix`);
progress bar from streamed snapshots; table rows → `SetLoopStart/SetLoopEnd`;
Audition → + `PlaySeam`; Apply → + `ApplyLoopToMetadata` (button enabled for
VGMs only). Row selection state lives in the dialog. GUI tests: gating; a
seeded fake-task result renders rows; clicking a row moves the markers;
Apply writes metadata (assert via `vgm_meta().loop_point`). Snapshot
`find_loop_dialog`.

### lf-4 · polish + corpus sanity

Run against a handful of corpus tracks whose loops are already tagged; the
known loop should appear in the top candidates (add a dev-note table of
results to the PR/commit message). Tune default min length from what that
shows (start at ~2 s of commands). Update `TODO.md` + the `vgmrips-pack-gaps`
memory (item 4 → DONE) when it lands.

## 6 · Where everything lives

| Concern | Path |
| --- | --- |
| New search | `crates/dro-core/src/loopfind.rs` (create) |
| Stream model / index map | `crates/dro-core/src/vgm/data.rs` |
| Loop indices + prefix sums | `crates/dro-core/src/song.rs` (`delay_samples_prefix`, :433) |
| Task service | `crates/dro-ui/src/tasks.rs` |
| Markers / seam / apply actions | `crates/dro-ui/src/action.rs`, `editor.rs`, `markers.rs` |
| New dialog | `crates/dro-ui/src/dialogs/find_loop.rs` (create) + `dialogs/mod.rs` registry |
| Menu + gating precedent | `crates/dro-ui/src/menus.rs`, `app_gui_tests.rs:2231` |

## 7 · Sources

- vgmtools `vgmlpfnd.c` (GPL-2.0) — behaviour digest fetched 2026-07-21:
  https://github.com/vgmrips/vgmtools (Route B: do not transcribe code).
- Loop semantics in this app: `docs/vgm-cmp-2026-07/HANDOVER.md` §3.4 (seam
  playback does not reset the chip; loop_point/loop_end are instruction
  indices, exclusive end).
