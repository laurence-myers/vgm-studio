# Crop to Marked Region / Delete Marked Region — plan (2026-07)

Two new undoable stream edits driven by the loop markers (`RangeMarkers`, the
half-open `[start, end)` instruction range in `crates/dro-ui/src/markers.rs`):

- **Crop to Marked Region** — keep only `[start, end)`, deleting everything
  before and after. When `start > 0`, a state-replay prelude is prepended so the
  kept body opens on exactly the chip state the original stream had reached at
  `start`.
- **Delete Marked Region** — remove `[start, end)`. A state-diff patch is
  inserted at the seam so the instructions after `end` still see the register
  state they were authored against.

Both work on all three formats (VGM, DRO v1, DRO v2) — these are stream edits,
not VGM metadata, so unlike Apply Loop they are not VGM-gated.

The markers module's own doc comment anticipated this: "the loop region today,
and whatever a crop/trim comes to need tomorrow."

## Why one mechanism covers both

The chip-state problem is the same in both operations, and it is the diff of two
register-state folds:

- Crop's prelude is the patch from the **blank** state to the state at `start`
  — i.e. every touched register's last write over `[0, start)`.
- Delete's seam patch is the patch from the state at `start` (what the chip
  holds when playback reaches the seam) to the state at `end` (what the
  post-seam instructions expect).
- "Delete instructions from the start of the song" is just Delete Marked Region
  with `start == 0`: the diff from blank is the full state replay.

The fold and the "emit each register's last write, verbatim bytes, low file
before high, with DRO v1 bank switches" emitter already exist in
`split_songs::append_state_prelude` (`crates/dro-core/src/split_songs.rs`),
backed by `OplState` (`crates/dro-core/src/opl_state.rs`). Step 1 extracts and
generalises that; the splitter becomes a caller of the shared code, so its
existing tests (including the real-capture corpus test and the proptests) keep
guarding the emitter.

Because `B` is folded over a superset of `A`'s writes, every register `A` holds
is also held by `B` — the diff is only ever "B differs from A" or "B has it, A
does not"; there is no "unset a register" case to worry about.

### DRO v1 banks

v1 register writes carry no bank; the current bank is a chip-side latch moved by
the bank-switch opcodes. So a patch must:

- start from the bank that is current at the point it is spliced in (for the
  seam patch that is the bank current at `start`, since the pre-seam body left
  the chip there; for the crop prelude it is `Bank::Low`, a fresh stream's
  default),
- emit its own switches between the low and high groups of writes, and
- end by switching to the bank current at the *resume* point (`start` for crop,
  `end` for delete), so the following body's bank-less writes land where they
  did originally.

This is the same dance `append_state_prelude` already does; the generalisation
adds the configurable entry/exit banks.

### Deliberate behavioural choices

- **Key-on bits replay verbatim.** A note sounding across the crop start (or
  across the deleted region's seam) is retriggered at that point. This is the
  same choice `materialise` made for split pieces: correct for sustained notes,
  and the only honest option — the alternative (masking key-on) would silence
  notes the original had playing. Accepted, consistent with split.
- **Patches add zero delay.** The kept material's timing is untouched; a DRO's
  `ms_length` shrinks by exactly the delays removed.
- No confirmation dialog: both edits are fully undoable.

## Core: `crates/dro-core/src/crop.rs`

```rust
pub struct CropOutcome {
    pub data: SongData,          // rebuilt stream, same variant as the source
    pub ms_length: u32,          // recomputed header value (DRO); VGM derives its own
    pub loop_point: Option<usize>,  // remapped, VGM only
    pub loop_end: Option<usize>,
    pub patch_writes: usize,     // for the status-bar message
}

pub fn crop_to_region(song: &Song, start: usize, end: usize) -> Option<CropOutcome>;
pub fn delete_region(song: &Song, start: usize, end: usize) -> Option<CropOutcome>;
```

Both return `None` for a degenerate request (no-op range: full-song crop,
empty region, out-of-range). Streams are rebuilt wholesale by concatenating
`raw_instruction` bytes, exactly as `materialise` does:

- crop: `patch(blank → state@start)` ++ `raw[start..end]`
- delete: `raw[0..start]` ++ `patch(state@start → state@end)` ++ `raw[end..len]`

The fold walks with `OplState` while tracking the v1 current bank and each
(file, register)'s **last-write source index**, so the patch re-uses the
source's encoding byte for byte, whatever the format (VGM opcode routing, v2
codemap codes, v1 bank-less pairs).

### Loop metadata remap (VGM)

Handled inside the core functions, since the rebuild invalidates
`move_loop_markers_past_deletion`'s incremental arithmetic. Same spirit as the
slide rule (land on the next surviving instruction; `None` end means "song
end"):

Crop, with a patch of `k` instructions:
- `loop_point < start` → `Some(0)` (its target is gone; the region now *is* the
  song, and looping through the prelude re-establishes crop-start state, which
  is what that loop meant).
- `start <= loop_point < end` → `Some(loop_point - start + k)`.
- `loop_point >= end` → `None`, and log the same "no longer loops" warning
  `move_loop_markers_past_deletion` emits.
- `loop_end` maps the same way, then drops to `None` unless it still bounds a
  real region (`> loop_point`, `< new len`) — the `set_vgm_metadata` rules.

Delete region `[start, end)` with a seam patch of `k` instructions:
- `index < start` → unchanged.
- `start <= index < end` → `Some(start)` (the seam; the patch is a timeless
  prefix of the surviving tail).
- `index >= end` → `index - (end - start) + k`.

DRO: no loop metadata; `ms_length` = the sum of kept instructions' delays,
computed from the existing delay prefix (`ms_offset_at` arithmetic), so the
header stays honest the way `DeleteInstructions` keeps it.

## Undo: a general stream-snapshot command

`DeleteInstructions` cannot express this edit (it inserts new instructions),
and composing delete+insert with loop-meta shifting is fiddlier than it is
worth. `OptimizeVgm` set the precedent: for a wholesale rebuild, snapshot the
stream before/after. Add to `crates/dro-core/src/undo.rs`:

```rust
pub struct ReplaceStream {
    description: &'static str,   // "Crop to Marked Region" / "Delete Marked Region"
    after:  (SongData, u32, Option<usize>, Option<usize>),  // data, ms_length, loop point/end
    before: Option<...>,         // captured on apply
}
```

backed by a new `pub(crate) fn Song::replace_data(data, ms_length, loop_point,
loop_end)` — the `replace_vgm_stream` shape generalised to any `SongData`
variant (debug-assert the variant matches; refresh the delay prefix; a VGM
ignores the passed `ms_length` since it derives its own, a DRO takes it).
Streams are small; two clones per edit is the accepted OptimizeVgm trade.
(Optional later cleanup, out of scope: refit `OptimizeVgm` onto
`ReplaceStream`.)

Undo restores the entire before-state exactly — `song == original` is the test.

## Editor: `crates/dro-ui/src/editor.rs`

Mirroring `optimize_vgm`:

```rust
pub fn crop_to_markers(&mut self) -> Option<usize>       // kept-instruction count
pub fn delete_marked_region(&mut self) -> Option<usize>  // removed-instruction count
```

- Bail (`None`) when no song, or `markers.is_full(len)`.
- Run the core fn, `undo.execute(Box::new(ReplaceStream::...))`.
- `self.markers = RangeMarkers::from_song(song)` — after a crop of the loop
  region the remapped loop is `0..None`, so the markers come back full; after a
  region delete they follow the remapped loop, exactly as Optimize does.
- `selection.clear()`, `analysis.invalidate()`, `revision += 1` (the audio
  snapshot and waveform re-key on revision, so playback picks up the new
  stream the same way a delete does).

## UI wiring

- `Action::CropToMarkers` and `Action::DeleteMarkedRegion` in
  `crates/dro-ui/src/action.rs`, in the "Loop points" group.
- `app.rs` dispatch → the editor methods, with status messages
  ("Cropped to N instructions." / "Deleted M instructions from the marked
  region."), plus the same post-edit housekeeping the delete path runs
  (loop-overlay/loop-config refresh — follow `Action::DeleteSelection`'s
  handler).
- `menus.rs` Edit menu: two items at the end of the marker section (after
  "Apply Loop to Metadata", before the Delete separator), enabled when a song
  is loaded **and** the markers are not full — `RangeMarkers::is_full` is
  already the "is this worth showing?" predicate.
- Row context menu (the marker items around `app.rs:1231`): the same pair.
- No new keyboard shortcuts initially; no confirmation dialogs.

## Tests

Core (`crop.rs`), reusing/moving split's `state_over` / `state_after_writes`
fold helpers into shared `#[cfg(test)]` support:

- Crop: the folded state at the cropped song's start equals
  `state_over(original, start)` — VGM, DRO v1 (across bank switches), DRO v2;
  asserted through a write/read round trip so the patch's opcodes decode back
  to the right banks (split's pattern).
- Delete: the fold of the whole edited stream equals the fold of the whole
  original (end-state preserved); the fold up to the seam patch's last write
  equals `state_over(original, end)`; a v1 stream's post-seam writes land in
  the bank they did originally.
- `start == 0` delete (the "trim the intro" case) gets an explicit test: the
  patch is the full state replay.
- Timing: `ms_length` / `total_delay_*` shrink by exactly the removed delays;
  patches add none.
- Loop remap matrix: loop point before/inside/after each boundary, end
  before/inside/after, end-at-song-end stays `None`.
- Proptests mirroring split's: random streams and random regions hold the
  state invariants and round-trip through write/read.
- Undo exactness for all three formats: apply → revert → `song == original`;
  redo re-applies byte-identically.

GUI (`app_gui_tests.rs`): the menu items are disabled while the markers are
full; crop resets the markers to full and bumps the revision/dirty flag; undo
restores the pre-crop song and re-clamps markers; status text lands.

## Steps (one commit each, workspace green after each)

1. **Extract the patch emitter.** Move the state fold + last-write emitter out
   of `split_songs` into `crop.rs` (or a shared module), generalised to
   from→to states with entry/exit banks; `append_state_prelude` becomes a
   call with a blank `from`. Split's tests unchanged and green.
2. **Core operations.** `crop_to_region` / `delete_region`, loop remap,
   `ms_length` arithmetic; unit tests + proptests.
3. **Undo.** `Song::replace_data` + `ReplaceStream`; exactness tests.
4. **Editor methods** + headless tests.
5. **UI.** Actions, dispatch, Edit menu + context menu items, GUI tests;
   IMPLEMENTED.md note in this folder.

## Decisions taken (revisit here if they grate)

- Named "Crop to **Marked Region**" / "Delete **Marked Region**", not "Loop":
  the markers double as the loop region, but on a DRO there is no loop to
  speak of and the crop use-case is exactly the DRO one.
- Key-on retrigger at the crop start / seam (see above) — matches split.
- A `loop_point` that sat *before* the cropped region maps to `Some(0)` rather
  than dropping the loop.
- Selection-based variants are out of scope: Delete Instruction(s) already
  covers arbitrary row deletion (without state patching); the marked region is
  the contiguous-range story.
