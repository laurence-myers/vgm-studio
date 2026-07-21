# HANDOVER — VGM optimizer (`vgm_cmp` equivalent; plan complete, implementation not started)

Written 2026-07-21 for a fresh Claude session on the `rust` branch of
`I:\Code\Python\dro-trimmer`. Everything below was verified against the code and
the vgmtools sources on that date; re-verify file:line references before leaning
on them.

## 1 · The feature

Strip audibly-redundant chip writes from OPL VGMs and merge the delays left
behind, shrinking files the way VGMRips' `vgm_cmp` does. This is the last step
of the VGMRips pack pipeline (wiki: "Optimizing VGMs") with no equivalent in the
app — see the survey in the `vgmrips-pack-gaps` memory: every other pipeline
step (trim, loop, tag, gzip, screenshot, docs, zip) is covered.

Scope: the three OPL layouts the app reads (`OplType`: Opl2, DualOpl2, Opl3).
Non-OPL commands (data blocks, DAC streams, PCM) do not exist in this app's
stream model, so `vgm_cmp`'s kept-verbatim classes are out of scope until the
any-chip work (`docs/vgm-multichip-2026-07/HANDOVER.md`, phase mc-4) lands.

Non-goals: `vgm_smp1`-style dropping of 1-sample delays (changes timing; the
optimizer must be timing-exact), and byte-parity with `vgm_cmp` output (see
§2.2.1 — behavioural parity with a render-equality proof is the goal).

## 2 · Decisions

### 2.1 Locked by the user (do not re-litigate)

- The project will move to GPL: approved 2026-07-20 in the multichip context
  (multichip handover §2.1), recommended GPL-2.0-or-later. **Not yet executed**
  — the workspace `Cargo.toml` still says `license = "LGPL-2.1-or-later"`.

### 2.2 Recommended (confirm with the user at kickoff)

1. **Licensing route.** vgmtools is GPL-2.0. Two clean options:
   - **Route B (recommended): independent implementation.** Derive the strip
     rules from chip facts (§3.3) and the VGM spec; treat `vgm_cmp` as a
     behavioural reference (its documentation and observed outputs), never
     transcribe its code. Keeps LGPL-2.1 for now; the render-parity test
     (§5, cmp-1) is the correctness net, which is stronger than matching
     another tool's bytes anyway.
   - **Route A: execute the approved GPL-2.0-or-later relicense first**, then
     port `chip_cmp.c` rules directly. Choose this only if exact `vgm_cmp`
     behaviour matching turns out to matter.
2. **Placement:** `crates/dro-core/src/optimize.rs` — pure, wasm-clean, no I/O,
   like `rip.rs`. Public API `optimize(song: &Song) -> Option<OptimizeOutcome>`
   returning the new `VgmData`, remapped loop indices, and counts for the UI
   (`None`/unchanged when the song is a DRO or nothing shrinks).
3. **Integration defaults:** rip export checkbox "Optimize on export" default
   ON (mirrors the gzip toggle; `vgm_cmp` is expected practice for packs), and
   an undoable editor action for one-song use. CLI subcommand optional (cmp-5).
4. **Editor undo:** one `OptimizeVgm` command implementing
   `UndoableCommand<Song>` (`dro-core/src/undo.rs:16`) that stores the whole
   before/after `VgmData` + loop indices. Songs are small; snapshot simplicity
   beats replaying two phases of edits.

## 3 · Domain facts (verified 2026-07-21)

### 3.1 How `vgm_cmp` works (behavioural digest from vgmtools source)

- `main()` re-runs `CompressVGMData()` until a pass stops shrinking the file.
- Each pass walks the command stream; per chip-write it calls a chip-state
  simulator (`ym3812_write()` etc.) returning whether the write changes state.
  Unchanged ⇒ command dropped.
- **Loop safety:** on reaching the header loop offset it calls
  `ResetAllChips()` — every register cache forgets its value — so nothing
  inside the loop body is stripped based on pre-loop state. The new loop
  offset is captured as the output position where that input position landed.
- Delays: dropped commands' delays accumulate; the pending total is flushed
  (optimally encoded) before the next kept command and at the loop offset.
- Header EOF/loop offsets recomputed for the output; GD3 carried over.
- Dual-chip commands (`0xAA` style) remap to the same simulator keyed by chip
  instance.

### 3.2 OPL rule table (what the simulator must model)

State: a 256-byte register file per (chip instance, port):
- Opl2: one file. DualOpl2: two files — `0x5A` = chip 1, `0xAA` = chip 2
  (`Bank::High` in this codebase, data.rs:177 — do NOT confuse with OPL3
  port 1, which shares that bank flag; split on `song.opl_type`).
- Opl3: two files — `0x5E` port 0, `0x5F` port 1. Port matters: port 1 reg
  0x04 is the 4-op connection select and 0x05 the NEW bit (audible state),
  while port 0 reg 0x04 is the timer/IRQ control (inaudible, §3.3).

Keep/drop per write, in order:
1. First-ever write to a (file, register) after load or loop-reset: **keep**
   (never assume power-on defaults; `vgm_cmp` does the same via `RegFirst`).
2. Same value as cached: **drop** — except the always-droppable /
   never-droppable classes below.
3. Timer registers (port-0 0x02/0x03/0x04 on OPL2 and OPL3, both chips of
   DualOpl2): inaudible in playback. Minimum safe rule: treat same-value like
   any register (rule 2). Route A may adopt `vgm_cmp`'s stronger "0x04
   flag-clear writes always droppable"; Route B should start conservative and
   let the corpus (cmp-5) say whether the extra bytes matter.
4. Everything else (0x01 test/WSE, 0x08, 0x20–0xF5 operator/channel regs,
   0xBD rhythm): rule 2 applies. Same-value rewrites do not retrigger
   envelopes or drums (§3.3), so dropping them is inaudible.

### 3.3 Chip facts that make rule 2 safe (Route B's independent basis)

OPL registers are level-sensitive latches: a write only matters if it changes
the latched value. Key-on (0xB0–0xB8 bit 5, 0xBD bits 0–4) retriggers only on
a 0→1 *transition* — rewriting an already-set bit with the same value is
silent. Timers (0x02–0x04) drive IRQs no VGM player uses for audio. These are
YM3812/YMF262 datasheet facts, independently derivable from any OPL emulator's
write path (e.g. the vendored nuked-opl3 in `dro-synth`).

### 3.4 Loop-safety proof sketch (why reset-at-loop-point suffices)

After the state reset at `loop_point`, the first in-body touch of every
register is kept (rule 1). So the loop body re-establishes every register it
depends on from its own kept writes: on wrap (end → `loop_point`), whatever
the registers hold, the body's kept writes recreate the same state sequence
the optimizer simulated. This covers both the file-format loop (loop offset →
EOF, what foreign players do) and the app's `[loop_point, loop_end)` seam
playback, which deliberately does not reset the chip at the seam (loop-points
feature decision).

### 3.5 This codebase (the load-bearing specifics)

- `VgmData` (`dro-core/src/vgm/data.rs`): raw bytes + per-command offset
  table. Only seven opcodes exist: the four OPL writes (3 bytes each) and
  waits `0x61`/`0x62`/`0x63`/`0x7n`. `get(index)` decodes to
  `DroInstruction::Register { reg, value, bank }` /
  `DelaySamples { kind, samples }`.
- `Song::delete_instructions` (`dro-core/src/song.rs:595`) already slides
  `loop_point`/`loop_end` past deletions (`slide_index_past_deletion`) — a
  strip pass expressed as "delete these indices" inherits loop remapping.
- `loop_num_samples` is derived, never stored (`song.rs:445`), and the writer
  (`vgm/io.rs:111`) recomputes every header field from the stream — the
  optimizer never touches headers.
- Byte-exact unedited round-trips are a project invariant (`vgm/io.rs` module
  doc). The optimizer must be identity when nothing is strippable, and must
  leave untouched delays' encodings verbatim (only *merged* runs re-encode).
- Undo: command pattern, `UndoableCommand<Song>` + `UndoController`
  (`dro-core/src/undo.rs`); `DeleteInstructions` is the precedent; the editor
  drives it in `delete_selection` (`dro-ui/src/editor.rs:190`).
- Rip export: `RipState::export_request` (`dro-ui/src/rip.rs`) ships
  `track.bytes` verbatim into `build_rip_zip`
  (`dro-trimmer/src/rip_zip.rs:30`), which gzips songs per the toggle — the
  natural place to optimize (native thread, bytes-in/bytes-out).
- Render for parity tests: `dro_synth::wav::render_wav(song, rate, depth)`
  (`dro-synth/src/wav.rs:52`); looped rendering via `PlayerEngine` +
  `LoopConfig` (see `engine.rs` tests, e.g. `render_to_end`).
- Corpus: `F:\GameMusic\VGM\{YM3812,YMF262}` — 233 real packs, 188 readable
  by the current reader (45 known-unreadable, multichip handover §1).

### 3.6 Why one strip pass suffices (vs `vgm_cmp`'s fixpoint loop)

Dropping a same-value write does not change the simulated register state, so
stripping decisions never invalidate each other; one exact-simulation pass is
already the fixpoint. (`vgm_cmp` iterates because its passes interact with
encoding sizes.) Assert idempotence in tests rather than looping.

## 4 · Environment & workflow rules

### 4.1 PATH prelude (required before ANY cargo/rustc call)

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

### 4.2 Working rules

- Work autonomously; commit per cmp-step with conventional messages
  (`feat(dro-core): …`), `Co-Authored-By: Claude <noreply@anthropic.com>`.
- Gate every step on: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- GUI snapshots: regenerate with `UPDATE_SNAPSHOTS=1 cargo test -p dro-ui`;
  new/changed PNGs under `crates/dro-ui/tests/snapshots/` are committed.
- dro-core must stay wasm-clean: `cargo check -p dro-core --target
  wasm32-unknown-unknown` if you add dependencies (you should not need any).

## 5 · The plan

### cmp-1 · dro-core: the strip pass

`optimize.rs`: an `OplState` (per-`OplType` register files, first-write flags)
plus `redundant_indices(song) -> Vec<usize>` implementing §3.2 with the state
reset *before* processing index `loop_point`. Apply via
`Song::delete_instructions`. Tests: per-rule units (first write kept; dup
dropped; dup kept when it is the first after the loop point; dual-OPL2 chips
tracked separately; OPL3 port 1 tracked separately); idempotence; and the
**render-parity property**: `render_wav(original) == render_wav(optimized)`
byte-for-byte on `tests/lsl3_score_up.vgm` and a synthetic percussion fixture,
plus a looped-render comparison across the seam (PlayerEngine + LoopConfig)
on a fixture with a loop point.

### cmp-2 · dro-core: delay merging

After stripping, runs of adjacent `DelaySamples` merge into one optimally
encoded sequence (greedy: 0x61 chunks of 65535, then 0x62/0x63 when exact,
then 0x7n) — **total samples conserved exactly**, verified against
`total_delay_samples()`. Merge barriers at `loop_point` and `loop_end`: a run
never merges across either index, so both stay on command boundaries with
their meaning intact; remap them by construction of the rebuilt stream.
Lone/untouched delays keep their original bytes (§3.5 invariant). Implement as
a rebuild into a fresh `VgmData` returning the new loop indices. Property
tests: sample-total conservation on random streams; barrier preservation;
no-op streams return byte-identical data.

### cmp-3 · editor integration

`Action::OptimizeVgm` + an `OptimizeVgm: UndoableCommand<Song>` snapshot
command (§2.2.4). Edit-menu item "Optimize VGM" gated VGM-only (exactly like
"Edit Tag" — see `edit_menu_items` tests in `app_gui_tests.rs:2231`). Status
line reports commands removed and bytes saved; markers re-derived from the
remapped loop (`RangeMarkers::from_song` after the command, as
`save_vgm_metadata` does). GUI tests: menu gating, undo/redo restores the
exact prior bytes, marker consistency; snapshot only if the menu screenshot
changes.

### cmp-4 · rip-mode export integration

`gzip_on_export`-style flag `optimize_on_export` (default ON) on `RipState`,
checkbox beside the gzip toggle, carried in `RipJobRequest`. In
`rip_zip::process_entry`: for song entries, `read_song` → optimize → write →
then gzip as now; on any parse/optimize error fall back to the original bytes
and log it (same never-fatal posture as the PNG path, `rip_zip.rs:28`). Log
lines like the gzip ones: `01 Intro.vgm: 40 -> 31 KB (optimized), -> .vgz …`.
Tests: service-level (an optimizable fixture shrinks; a DRO-less folder
unchanged; error fallback) and a GUI test that the checkbox reaches the job
request. Snapshots: rip view header changes → regenerate.

### cmp-5 · corpus validation (+ optional CLI)

Script or test-harness pass over the 188 readable corpus packs: optimize every
track, assert render-parity on a sampled subset, report aggregate size
reduction (expect the OPL ballpark of `vgm_cmp`, roughly 30–60% on DOSBox
logs; document actuals). Fix any rule found wanting. Optionally add a
`drotrim optimize <in> [out]` subcommand beside the existing
play/render/convert/split set. Update `TODO.md` and the `vgmrips-pack-gaps`
memory (item 1 done) at the end.

## 6 · Where everything lives

| Concern | Path |
| --- | --- |
| New optimizer | `crates/dro-core/src/optimize.rs` (create) |
| Stream model | `crates/dro-core/src/vgm/data.rs` |
| Loop-index sliding on delete | `crates/dro-core/src/song.rs:595` |
| Header writer (recomputes all fields) | `crates/dro-core/src/vgm/io.rs:111` |
| Undo command pattern | `crates/dro-core/src/undo.rs` |
| Editor actions / menu gating | `crates/dro-ui/src/editor.rs`, `app.rs`, `menus.rs` |
| Rip export UI + job | `crates/dro-ui/src/rip.rs`, `platform.rs` |
| Zip/gzip pipeline | `crates/dro-trimmer/src/rip_zip.rs` |
| Render for parity tests | `crates/dro-synth/src/wav.rs`, `engine.rs` |
| Fixtures | `tests/lsl3_score_up.vgm`, corpus at `F:\GameMusic\VGM\` |

## 7 · Sources

- vgmtools (GPL-2.0): https://github.com/vgmrips/vgmtools — `vgm_cmp.c`
  (pass structure, `ResetAllChips` at loop offset, delay flushing),
  `chip_cmp.c` (per-chip rules; OPL family confirmed present). Fetched
  2026-07-21. Route B: consult behaviour, do not transcribe code.
- VGM spec v1.72 (wait commands, loop offset semantics):
  https://vgmrips.net/wiki/VGM_Specification
- OPL programming facts: YM3812/YMF262 datasheets; the vendored nuked-opl3
  write path in `dro-synth` (authoritative for what a write actually does).
- Prior handover in this style: `docs/vgm-multichip-2026-07/HANDOVER.md`
  (multichip plan; its mc-4 is where non-OPL commands would join this
  optimizer's world).
