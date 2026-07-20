# HANDOVER — Loop points feature (plan complete, implementation not started)

**For:** a fresh Claude Code session implementing the loop-points feature.
**From:** the planning session, 2026-07-20.
**Repo:** `I:\Code\Python\dro-trimmer` · **branch `rust`** (main/master is the Python original — parity oracle only, never modify `src/`).
**Status:** **lp-1, lp-2 and lp-3 are implemented, tested and committed** (see §0).
The backend is done; lp-4 starts the user-visible surface. All user decisions are
made (§2). Follow the workflow rules in §4.

---

## 0 · Progress (updated 2026-07-20)

| Step | State | Commit |
|------|-------|--------|
| lp-1 | **done** — `VgmMeta::loop_end`, derivation, reader materialisation, deletion sliding, undo restore, 11 tests | `a015398` |
| lp-2 | **done** — `LoopConfig`/`LoopCount`, wrap in `render`, 12 tests | `0ee16a9` |
| lp-3 | **done** — `SetLoop` command, `loop_iteration` atomic, `AudioService::set_loop` | `9e738d9` |
| lp-4 | not started — `RangeMarkers`, actions, gestures | |
| lp-5 | not started — palette, waveform markers, transport, snapshots | |
| lp-6 | not started — dialog loop-end field, apply action, docs | |

Workspace after lp-3: `cargo test --workspace` green (dro-core 224, dro-synth 70,
dro-ui 151, dro-trimmer 31, + integration suites); `cargo clippy --workspace
--all-targets` zero warnings.

### What changed versus the plan below (read before lp-4)

1. **`Position` gained only `loop_iteration`, not the `song_frames`/`song_ms`
   pair §5.2 called for.** `frames_rendered` turned out to be a *song position*
   already — `seek_to_pos` resets it — not a monotonic count of frames sent to
   the device, and nothing consumes it as the latter. So a wrap simply rewinds
   `frames_rendered` to `LoopConfig::start_frames`, and the readout, cursor and
   `elapsed_ms` all wrap for free. lp-5 should read `Position::loop_iteration`
   for the "loop 2/5" display and otherwise needs no new position plumbing.
2. **`LoopCount::Times(n)` means the region is heard `n` times in total**
   (player convention), so it jumps back `n - 1` times; `Times(0)` and
   `Times(1)` both mean "no repeat". The count applies **from now**: both
   `set_loop` and any seek restart the tally rather than crediting repeats
   already played.
3. **A region containing no delays is refused at wrap time.** It renders no
   audio, so looping it would spin inside `render` forever without ever filling
   the caller's buffer — a hang, not a glitch. The engine logs a warning, drops
   the loop and plays on. `set_loop` likewise refuses an empty or out-of-range
   region outright. lp-4's UI should not be able to produce either, but the
   engine no longer depends on that.
4. **`dro-synth` gained a `log` dependency** (workspace dep, wasm-clean) for
   that warning.
5. **Loop-end index normalisation.** Zero-delay commands share a timestamp, so a
   header loop length can match several indices; the reader takes the *first*.
   The file round-trips byte-for-byte either way, but `loop_end` may come back
   as a lower index than the one that was saved. lp-6's dialog should not treat
   that as data loss.
6. **`resolve_loop_end` rejects a zero-length header loop** (a loop offset with
   a zero length is self-contradictory) and falls back to looping to the end.

### Pre-existing breakage you will trip over

`cargo fmt --all --check` is **red on code nobody in this feature touched** —
7 files (`dro-core/src/rip.rs`, `dro-ui/src/{action,app,rip}.rs`,
`dro-ui/src/theme/bevel.rs`, `dro-ui/src/widgets/{channels,pan_knob,table}.rs`).
`rust-toolchain.toml` floats on `channel = "stable"`, and rustfmt 1.9.0
(2026-07-07) changed its wrapping heuristics, so files formatted by the previous
rustfmt now fail. Confirmed pre-existing by formatting HEAD's copy of an
untouched file. **lp-1..lp-3 therefore formatted only their own files** rather
than bundle an unrelated 7-file reformat into a feature commit. Worth its own
`style: reformat under rustfmt 1.9` commit before lp-5, since lp-5 edits several
of those files and would otherwise mix the reformat into a feature diff.

---

## 1 · The feature

The user's requirements, verbatim in spirit:

1. Select a loop **start** point (including the very start of the song)
2. Select a loop **end** point (including the very end of the song)
3. Play looped
4. Play unlooped
5. Set the **number of loops** during playback
6. **Save the loop points to the VGM metadata**
7. A **visual indicator** of the loop points

Standing constraint: a future **crop/trim** feature should be able to reuse the
start/end selection machinery — keep the range concept generic.

Context: this closes the `TODO.md` bullet under `## VGM` → "Support for header
features → Loop points → What is still missing is *playback*". The feature also
serves rip mode (VGMRips submission prep): auditioning whether a loop seam is
clean is the point of loop playback there.

## 2 · User decisions (all made — do not re-litigate)

1. **Loop end is a persisted, first-class field.** The UX lets the user set a
   loop end point, and the VGM header's `loop # samples` field is written as
   `end − start` (derived, never stored). This required promoting `loop_end`
   into the data model — see §5.1. The user accepted the spec caveat (§3).
2. **A Loop toggle changes Play's behaviour** — no separate "Play looped" /
   "Play unlooped" buttons. The toggle (and loop count) are live-changeable
   during playback.
3. **Finite loop count plays the tail.** After the final pass reaches the end
   marker, playback continues through `[end, len)` and finishes at EOF.
   (This is the simpler engine too: the wrap check only fires while wraps
   remain.)
4. **A "Play Seam" button is in scope**: seek to `tail_length` ms before the
   *end marker*, loop playback forced on — the loop-audition twin of Play Tail.

## 3 · Domain facts you must know (verified against spec + code)

- **VGM has no loop-end field.** The header stores a loop *offset* (0x1C, where
  playback jumps back to) and `loop # samples` (0x20). The spec defines the
  latter as the wait total between the loop point and the **end of the file**,
  and real players (vgmplay, libvgm) jump back on hitting the end-of-data
  command `0x66` — they do not stop at `loop start + length`.
- **Consequence (accepted by the user):** a saved file whose `loop_end` sits
  before EOF is internally unusual per spec — other players will audibly loop
  the full tail; the shorter length only affects their duration/fade math. In
  the intended workflow a finished rip has its tail trimmed so `loop_end` *is*
  EOF and the written value equals the spec's definition. Until then the
  shorter length is a faithful record of intent that dro-trimmer itself honours
  in playback and preserves across saves. Surface a status-bar note when
  applying a pre-EOF end ("trim the tail to make this the real end").
- **Existing model** (all on branch `rust`, as of commit `2951871`):
  - `VgmMeta::loop_point: Option<usize>` — an *instruction index*, not a byte
    offset (`crates/dro-core/src/vgm/data.rs`, ~line 322).
  - `Song::loop_num_samples()` (`crates/dro-core/src/song.rs`, ~line 451)
    *derives* the length as `total − samples_before(loop_point)`; the header
    copy is never stored, so trims can't leave it stale.
  - The writer recomputes the byte offset and derived length
    (`crates/dro-core/src/vgm/io.rs`, ~line 134–165). Zero offset = no loop.
  - The reader resolves the byte offset to an index and validates it
    (`resolve_loop_point`); a header length disagreeing with the derived value
    is **warned about and discarded** ("trusting the stream", io.rs ~line 308).
  - Deletions slide the loop point via `Song::move_loop_point_past_deletion`
    (`song.rs`, ~line 614): partition_point over the sorted deleted indices;
    deleting the loop instruction lands the loop on its successor; deleting
    everything at/after it drops the loop.
  - `Song::delay_samples_prefix()` (public; the VGM metadata dialog uses it)
    gives cumulative samples before each instruction.

## 4 · Environment & workflow rules

### 4.1 PATH prelude (required before ANY cargo/rustc call)

Rust/LLVM are Scoop-installed at **User** scope; agent processes do not inherit
them. Prepend to every PowerShell tool call that runs cargo:

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

### 4.2 Working rules

- **Confirm with the user before starting each numbered step in §6.** That is
  the established rhythm for this port; do not batch ahead silently.
- Keep the workspace green after every step: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets` (**zero warnings** — lints deny
  `unsafe_code`, warn `clippy::all`), `cargo fmt --all --check`.
- Everything in `dro-core` / `dro-synth` / `dro-ui` must stay **wasm-clean**
  (no `std::fs`, no cpal, no threads). Native-only code goes in
  `dro-audio-native` / `dro-trimmer`.
- GUI snapshot tests (egui_kittest): after visual changes, regenerate PNGs with
  `UPDATE_SNAPSHOTS=1` and eyeball the diffs. New palette fields must appear in
  the per-theme showcase (`theme_showcase.rs`) so theming stays guarded.
- Commit style: conventional-commit-ish, scoped, e.g.
  `feat(dro-core): persist an explicit VGM loop end (lp-1)`. One commit per
  coherent step; reference this doc's step ids (lp-1 … lp-6).
- The web crates (`dro-web`, `dro-synth-worklet`) are stubs — ignore them, but
  keep the engine changes inside `PlayerEngine` so the future worklet inherits
  looping for free.

## 5 · The plan

### 5.1 · dro-core: `loop_end` in the model (step lp-1)

- Add `loop_end: Option<usize>` to `VgmMeta` — **exclusive** instruction index;
  `None` means end-of-data (today's behaviour, and the value for every file
  whose header length matches the to-EOF derivation).
- `Song::loop_num_samples()` becomes
  `samples_before(effective_end) − samples_before(loop_point)` where
  `effective_end = loop_end.unwrap_or(len)`.
- **Writer:** shape unchanged — it already writes the derived length, so it
  automatically emits `end − start` once the derivation respects `loop_end`.
- **Reader:** where it currently warns-and-discards a mismatched header length:
  if the header length is *shorter* than the to-EOF derivation and lands
  **exactly on an instruction boundary** after the loop point, materialise it
  as `loop_end` (search the samples prefix for
  `samples_before(i) − samples_before(start) == header_length`). This makes
  dro-trimmer's own files round-trip byte-for-byte instead of being
  "corrected" on re-save. Longer-than-EOF or mid-command lengths keep the
  existing warn-and-fall-back-to-EOF path.
- **Deletion sliding:** extend `move_loop_point_past_deletion` to slide
  `loop_end` with the same partition_point rule. Rules: deleting instructions
  before it slides it left; deleting everything from `loop_end` onward
  collapses it to `None` (EOF); it must never end up ≤ the (slid) loop point —
  if it would, drop it to `None`. Maintain the invariant
  `loop_point < loop_end ≤ len` whenever both are `Some`.
- Tests: round-trip a file with an explicit shorter loop length (byte-for-byte
  both ways); boundary/mid-command/longer-than-EOF header lengths; each
  deletion rule; the existing loop tests must pass untouched (they are the
  `loop_end: None` cases).

### 5.2 · dro-synth: engine looping (step lp-2)

Add to `PlayerEngine` (`crates/dro-synth/src/engine.rs`):

```rust
pub enum LoopCount { Infinite, Times(u32) }   // Times(n): region heard n times total
pub struct LoopConfig {
    pub start: usize,        // instruction index to jump back to
    pub end: usize,          // exclusive; == song.len() means "to EOF"
    pub count: LoopCount,
    pub start_frames: u64,   // precomputed frame position of `start` — see below
}
```

- In `render`'s instruction-stepping branch (the `else if self.pos <
  self.song().len()` arm, ~line 411): when `pos` reaches `end` with wraps
  remaining, set `self.pos = start` and decrement — **no chip reset, no
  replay**. Carrying chip state across the seam is deliberate: it is what a
  real VGM player does, and hearing seam discontinuities is the point of loop
  audition. When wraps are exhausted, `pos` simply runs past `end` into the
  tail and the song finishes at EOF (decision §2.3).
- `FrameClock` carry: leave it accumulating across the seam (sub-frame
  precision; resetting it would drift the total).
- **Position reporting:** `frames_rendered` stays monotonic (meter, total
  listening time). Add `song_frames: u64` (set to `config.start_frames` at
  each wrap, advanced in lockstep otherwise) and `loop_iteration: u32`.
  Extend `Position` with song-relative ms + iteration; existing callers keep
  compiling (add fields, keep `from_frames` semantics for them).
- `start_frames` is **precomputed by the caller** (UI side:
  `delay_samples_prefix()[start]` converted to output frames as
  `samples × rate / 44100` in u64 math, ms analog for DRO) so the audio
  callback never walks the song. The engine treats it as opaque.
- `is_finished()`: false while wraps remain; `Infinite` never finishes.
- Playback starting past `end` (Play honours the selected row) never wraps —
  plays out unlooped, by construction.
- `set_loop(Option<LoopConfig>)` applies live; a mid-delay change takes effect
  at the next boundary check (fine).
- Tests (use `RecordingChip`): a wrap writes **no** reset/replay registers;
  `Times(n)` renders exactly intro + n×region + tail frames (build on the
  FrameClock arithmetic like `total_frames_match_the_song_length`);
  chunk-size invariance across a wrap (extend
  `output_is_independent_of_the_pull_size`); seek during looped playback;
  `is_finished` truth table (unlooped / Times / Infinite); wrap when
  `end == len` vs `end < len`; config change mid-playback.

### 5.3 · dro-audio-native + AudioService (step lp-3)

- `Command::SetLoop(Option<LoopConfig>)` over the existing rtrb queue
  (`crates/dro-audio-native/src/lib.rs`; `LoopConfig` is `Copy`-friendly).
  Requirement 5 ("set count during playback") = resend the config.
- `SharedState` gains `song_frames: AtomicU64`, `loop_iteration: AtomicU32`;
  `NativeAudio::position()` folds them into `Position`.
- `AudioService` trait (`crates/dro-ui/src/platform.rs`, ~line 126) gains
  `fn set_loop(&mut self, config: Option<LoopConfig>)`. Update the test/mock
  implementations in dro-ui (`test_support.rs`).

### 5.4 · dro-ui state: RangeMarkers + actions (step lp-4)

- New `RangeMarkers { start: usize, end: usize }` — **session state owned by
  the app** (like `Selection`), *not* the Song: a marker click must not
  silently mutate file data; persisting is the explicit apply action
  (requirement 6). Named neutrally — this is the future crop selection.
  Invariant `start < end ≤ len`; defaults `0 .. len` (satisfying "including
  the very start/end"). On load, initialise from
  `VgmMeta::{loop_point, loop_end}` when present.
- Edit tracking: on deletion, slide both markers with the **same shared
  helper** extracted from `move_loop_point_past_deletion` (do not fork the
  rule); on undo/redo, clamp to the new length as `Selection::truncate_to`
  does.
- New `Action` variants: `SetLoopStart(usize)`, `SetLoopEnd(usize)`,
  `ClearLoopMarkers`, `ToggleLoopPlayback`, `SetLoopCount(LoopCount)`,
  `ApplyLoopToMetadata`, `PlaySeam`.
- Gestures (all three): waveform Shift+click = set start / Ctrl+Shift+click =
  set end at the snapped instruction (the click-snap machinery exists in
  `widgets/waveform.rs::show`); table keys `[` / `]` = start/end from the
  focused row; transport "Set start"/"Set end" buttons from the selection.
- Loop playback state (enabled + count) lives beside the markers; any change
  while playing sends `set_loop` immediately. DRO songs: loop playback works
  (whole-song defaults); `ApplyLoopToMetadata` is VGM-only, disabled with a
  "Convert to VGM first" tooltip.

### 5.5 · Visuals + transport (step lp-5)

- Palette (`theme/palette.rs`): add `wf_loop` (marker lines/flags) and
  `wf_loop_region` (translucent tint), for **all** themes; extend the theme
  showcase.
- Waveform: draw start/end markers as vertical lines with small inward-pointing
  triangular flags at the top (DAW-style, distinct from the plain start/cursor
  lines); tint `[start, end)` while loop playback is enabled. When a marker
  differs from the saved `VgmMeta` value, render its flag hollow/outlined —
  the "unsaved" cue that motivates the explicit apply.
- Transport row: Play / Stop / Play Tail / **Play Seam** / **Loop toggle** /
  **count control** (∞, 1, 2, 3…; default ∞). Play Seam = seek to
  `config.ui.tail_length` ms before the *end marker* with looping on (mirror
  `do_play_tail`, ~app.rs line 1948).
- `playback_tick` (~app.rs line 927): guard the end-of-song snap (~line 951) so
  a looping stream never triggers it; drive `waveform.cursor_ms` from the new
  song-relative ms so the cursor visibly wraps; position panel shows
  "loop 2/5" (or ∞) while looping.
- Regenerate kittest snapshots (`UPDATE_SNAPSHOTS=1`); add GUI tests for the
  new transport controls and marker painting.

### 5.6 · Metadata apply + dialog (step lp-6)

- `ApplyLoopToMetadata` routes through the `Editor::set_vgm_metadata` path
  (extended for `loop_end`) — **non-undoable**, matching the existing dialog
  convention. Same out-of-range drop guard (the modeless dialog / stale-marker
  race). Writes both markers; `start == 0 && end == len` still writes a loop
  (loop_point `Some(0)` is valid); "no loop" = explicit clear.
- VGM Metadata dialog (`dialogs/vgm_metadata.rs`): add a "Loop end
  (instruction)" field (empty = EOF); the read-only length readout derives
  from the typed pair via the captured samples prefix; validate
  `start < end ≤ len`.
- When applying an `end < len`: status-bar note that other players will loop
  the full tail until the tail is trimmed (§3).
- Known quirk, unchanged by design: metadata edits do not mark the song dirty
  (`Editor::is_dirty` deliberately ignores them, matching the Python), so an
  applied-but-unsaved loop won't trigger the discard prompt. Noted as a
  possible follow-up — do not fix it as part of this feature.
- Docs: tick the TODO.md loop-playback bullet; update user docs if touched.

## 6 · Step sequence (confirm with the user before each)

| Step | Scope | Landable alone? |
|------|-------|-----------------|
| lp-1 | dro-core: `loop_end` model + reader/writer + deletion sliding + tests | yes |
| lp-2 | dro-synth: `LoopConfig`, wrap logic, `Position` extension + tests | yes |
| lp-3 | dro-audio-native + `AudioService::set_loop` + mocks | yes |
| lp-4 | dro-ui: `RangeMarkers`, actions, gestures, edit clamping | yes |
| lp-5 | visuals: palette, waveform markers, transport (Loop/count/Play Seam), snapshots | yes |
| lp-6 | metadata: apply action, dialog loop-end field, notes, docs/TODO | yes |

Manual acceptance at the end: load a real looping VGM from a rip project,
audition the seam with Loop ∞ — it must sound identical to vgmplay/in-game;
set markers, apply, save, reopen — markers restore; `end < len` file
round-trips byte-for-byte.

## 7 · Where everything lives (orientation)

| Concern | File |
|---------|------|
| VGM header read/write, loop offset ↔ index | `crates/dro-core/src/vgm/io.rs` |
| `VgmMeta` (loop_point, loop_base/modifier) | `crates/dro-core/src/vgm/data.rs` |
| `Song`: prefix sums, `loop_num_samples`, deletion sliding | `crates/dro-core/src/song.rs` |
| Undo command pattern | `crates/dro-core/src/undo.rs` |
| Pull engine (`render`, `seek_to_pos`, `Position`, `FrameClock`) | `crates/dro-synth/src/engine.rs` |
| cpal callback, rtrb `Command` queue, `SharedState` atomics | `crates/dro-audio-native/src/lib.rs` |
| `AudioService` trait | `crates/dro-ui/src/platform.rs` |
| Action enum (UI → app) | `crates/dro-ui/src/action.rs` |
| App shell: transport, `playback_tick`, `do_play`/`do_play_tail` | `crates/dro-ui/src/app.rs` |
| Headless editor (undo, `set_vgm_metadata`, dirty) | `crates/dro-ui/src/editor.rs` |
| Row selection model (the clamping precedent) | `crates/dro-ui/src/selection.rs` |
| Waveform painting + click snap | `crates/dro-ui/src/widgets/waveform.rs` |
| Position readout panel | `crates/dro-ui/src/widgets/position_panel.rs` |
| Themes/palette (`wf_*` colours) | `crates/dro-ui/src/theme/palette.rs` |
| VGM metadata dialog | `crates/dro-ui/src/dialogs/vgm_metadata.rs` |

Line numbers cited in §3/§5 are as of commit `2951871` — re-locate by symbol
name if the files have moved on.
