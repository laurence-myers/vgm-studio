# Crop to Marked Region / Delete Marked Region — as shipped (2026-07-23)

Both edits from [PLAN.md](PLAN.md) are implemented on the `rust` branch and the
workspace is green (366 dro-core tests, 327 dro-ui, clippy and fmt clean).

Edit menu, under the loop-marker group, enabled only once the markers mark
something out:

- **Crop to Marked Region** — keeps `[start, end)`, drops everything else.
- **Delete Marked Region** — drops `[start, end)`, keeps everything else.

Both work on VGM, DRO v1 and DRO v2 — they edit the stream, not VGM metadata, so
unlike Apply Loop to Metadata they are not VGM-gated. Both are undoable.

## What was built

| Piece | Where |
| --- | --- |
| State fold + patch emitter | `crates/dro-core/src/state_patch.rs` (new) |
| Crop / delete operations | `crates/dro-core/src/crop.rs` (new) |
| `Song::replace_data` | `crates/dro-core/src/song.rs` |
| `ReplaceStream` undo command | `crates/dro-core/src/undo.rs` |
| `crop_to_markers` / `delete_marked_region` | `crates/dro-ui/src/editor.rs` |
| Actions, dispatch, menu items | `crates/dro-ui/src/{action,app,menus}.rs` |

### One mechanism, as planned

A state patch is the diff between two `OplState` folds, and both edits are that
diff placed at the edge the cut leaves:

- Crop prepends the diff from a **blank** chip to the state at `start`.
- Delete splices the diff from the state at `start` to the state at `end` into
  the seam.
- "Trim the intro" is the second with `start == 0`, where the diff from blank
  degenerates to the full state replay — the case the user asked for.

`split_songs::append_state_prelude` became the blank-`from` case of the shared
emitter, so its corpus test and proptests carry over unchanged as guards.

### Refinement over the plan

The patch writes **only registers whose value actually changed** between the two
folds, not every register the `to` fold holds. From blank that is every one of
them (so split is byte-for-byte unchanged), but across a seam it matters: a
register set before the cut and never touched inside it is left alone rather than
rewritten, which for a key-on register would retrigger a note that was already
sounding. Sustained notes still retrigger where the state genuinely changed —
the accepted trade, same as split's.

Because the `to` fold always covers a superset of `from`'s writes, there is never
an "un-write" case to express, which is just as well: an OPL register cannot be
returned to "never written".

### Loop metadata

Remapped inside the core functions (the wholesale rebuild invalidates the
incremental slide rule), by the rules `set_vgm_metadata` stores a loop by, so a
remapped loop and a typed one cannot mean different things. A loop point sitting
*before* a cropped region maps to `Some(0)` — the region now is the song, and
looping through the prelude re-establishes the state the body opens on. The
editor then re-derives the markers via `RangeMarkers::from_song`, as Optimize
already does.

### Timing

Patches carry no delay, so survivors keep their milliseconds exactly; a DRO's
header length shrinks by precisely what was dropped, and a VGM's derived length
is recomputed by the rebuild.

## Deviations from the plan

1. **`Song::replace_data` landed in step 2, not step 3** — `CropOutcome::install`
   needed it, so step 3 is the `ReplaceStream` command alone. `replace_vgm_stream`
   now delegates to it rather than duplicating the work.
2. **The shared primitive lives in `state_patch.rs`, not inside `crop.rs`** —
   `split_songs` uses it too, so a module about cropping was the wrong home.
3. **No row context menu item.** The plan cited `app.rs:1231` as a context menu;
   that is actually the `[`/`]` keyboard handler, and this UI has no context
   menus at all. The Edit menu is the only surface, matching every other marker
   action.
4. **No new keyboard shortcuts**, as planned — the menu is the discovery path.

## Testing

- `state_patch`: diff semantics (unchanged registers skipped, last-value-wins,
  files independent) and DRO v1 bank-switch emission including the entry/exit
  bank cases.
- `crop`: exact byte-level expectations on a layered fixture, exhaustive
  start/end sweeps asserting survivors resume on the folded state, all three
  formats, the loop-remap matrix, timing, and no-op guards.
- Proptests over random streams and regions: survivors open on exactly the state
  a fold of the original reaches, through a write/read round trip, and only the
  cut region's delays are gone.
- `undo`: apply/revert exactness for all three formats, DRO header length, loop
  markers, and interleaving with the other commands.
- Editor and GUI tests: counts, marker reset, selection clearing, dirty/revision
  bookkeeping, menu enablement, status text, and loop re-arming over the new
  stream.

## Not done / possible follow-ups

- `OptimizeVgm` could now be refitted onto `ReplaceStream` (the plan called this
  optional; it remains its own command).
- Deleting a region that covers the whole song is allowed in the core but
  declined by the editor, since the markers-are-full case gates both edits. Select
  All + Delete already covers that intent.
