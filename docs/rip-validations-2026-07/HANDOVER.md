# HANDOVER — Submission-readiness validations (plan complete, implementation not started)

Written 2026-07-22 for a fresh Claude session on the `rust` branch of
`I:\Code\Python\dro-trimmer`. The rip-mode code is moving quickly (bulk tag and
the volume scan landed this week) — re-verify every file:line reference before
leaning on it. Companion plans: `docs/vgm-cmp-2026-07/`, `docs/vgm-lpfnd-2026-07/`,
`docs/vgm-vol-2026-07/`, `docs/vgm-sptd-2026-07/`.

## 1 · The feature

Grow rip mode's export checks into a full VGMRips submission checklist, shown
**live** rather than only at export time. The wiki's submission requirements
that the app currently never checks: GD3 tags complete on every track,
tags consistent with the pack `.txt`, hyphen-separated release dates, update
history present, and loops verified. Today `RipState::validations()` covers
only file-level shape (game name, readable songs, screenshot, `NN Title`
numbering).

UI vision (the "nice UI" requirement):

- A **Submission checklist** section in the rip view: grouped, live-updating
  list — each unresolved item is one clickable line; clicking navigates to the
  fix (meta-field issues scroll/focus the form field; per-track issues open
  that track's quick-edit dialog). Groups with nothing wrong collapse to a
  single ✓ line, so a clean pack reads as five ticks.
- A **status glyph column** in the track table (✓ / ⚠) with a hover tooltip
  listing that track's specific problems; click opens quick-edit.
- **Fix-assists** where the fix is mechanical: a "Convert to hyphens" button
  on date-format warnings that rewrites `1994/03/01` → `1994-03-01` in the
  pack meta and every track's GD3 as one undoable batch.
- The export gate keeps its current shape (hard errors block via
  `Alert::error`, soft warnings get the "Export anyway?" confirm →
  `Action::ConfirmExportZip`) — it just gains the new checks, and in practice
  fires less because problems were visible all along.

## 2 · Decisions to confirm at kickoff

1. **Severity tiers.** Recommend three: `errors` (block export — unchanged),
   `warnings` (confirm dialog — most new checks land here), and a new
   `notes` tier (shown in the checklist panel, never in the export dialog) —
   for genuinely-optional things like non-looping tracks. Extends
   `RipValidations` with a `notes: Vec<String>` field.
2. **Are missing loops a warning or a note?** Jingles legitimately never
   loop. Recommend a *note* listing loopless tracks ("verify these are meant
   to play once"), no duration heuristics.
3. **Date rule.** Accept `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (all-digit,
   hyphen-separated) for both the pack meta and GD3 tags; anything else —
   slashes, dots, free text — warns. (GD3's own 1.00 spec shows slashes, but
   the VGMRips rerip guide explicitly converts slashes to hyphens; the wiki
   convention wins for packs.)
4. **The consistency anchor is the pack meta** (the `.txt` *is* the project):
   GD3 fields that differ from it are flagged per track, not the reverse.
5. **Author matching.** Track authors vary legitimately (that is why bulk tag
   has per-track selection). Recommend a *note* when the union of GD3 authors
   (comma/`&`-split, trimmed) differs from the meta's `Music author:` set —
   never a warning.

## 3 · Domain facts

### 3.1 The checks (wiki-derived, full list)

Pack-level (all soft warnings unless marked):
- P1 `creator` (Package created by / ripper) empty.
- P2 `release_date` empty or failing the §2.3 date rule.
- P3 `music_authors` empty.
- P4 `history` empty (the wiki requires update notes with a submission).
- P5 screenshot missing — exists today, keep.
- P6 game name empty — exists today, hard error, keep.

Per-track GD3 (soft; reported as "01 Intro: missing Track Author"):
- T1 no GD3 tag at all.
- T2 `track_name_en` empty.
- T3 each of `game_name_en`, `system_name_en`, `track_author_en`,
  `release_date`, `creator` empty.
- T4 `release_date` failing the date rule.
- T5 file-name title (`NN Title.ext` minus prefix) differs from
  `track_name_en` — quick-edit derives names from titles, so drift means an
  external rename or stale tag.

Cross-consistency vs the pack meta (soft; offenders listed):
- C1 `game_name_en` ≠ meta.game_name.
- C2 `system_name_en` ≠ meta.system.
- C3 `creator` ≠ meta.creator.
- C4 `release_date` ≠ meta.release_date.
- C5 (note tier) author-set mismatch per §2.5.

Loops (note tier): L1 tracks with no loop point (§2.2).

File-level (existing, keep): unreadable tracks, `NN Title` shape, duplicate /
non-contiguous numbering.

### 3.2 This codebase (the load-bearing specifics)

- `RipState::validations()` (`dro-ui/src/rip.rs`, currently ~:234) returns
  `RipValidations { errors, warnings }`; consumed only by `export_rip_zip`
  (`app.rs`, ~:2100): errors → `Alert::error`, warnings → `Alert::confirm`
  with `Action::ConfirmExportZip`.
- Tag access per track: `RipTrack::song()` → `vgm_meta().tag` (rip.rs);
  loop presence: `TrackEntry::loop_samples` (dro-core/src/rip.rs:96).
- The pack meta struct is `RipMeta` (dro-core/src/rip.rs:49).
- Title-from-filename logic to reuse for T5:
  `title_from_file_name` (dialogs/track_edit.rs:143) — move/share it rather
  than duplicating (it is currently private to the dialog).
- Batch rewrite precedent for the date fix-assist: `bulk_tag_submitted` /
  `apply_rip_modifiers` (app.rs) — build per-track bytes, one
  `RipTransaction` of `Write` mutations, undo for free; serialisation via
  `retagged_bytes` (rip.rs).
- Click-to-fix plumbing: opening a track's quick-edit is
  `Action::OpenTrackQuickEdit(index)`; scrolling the meta form needs an egui
  `scroll_to`/`request_focus` on the target `TextEdit` (the rip view already
  runs inside one `ScrollArea`, rip.rs `show()`).
- Track table columns: `rip.rs::track_table` (a Peak column was added by the
  volume feature — the glyph column follows the same pattern).
- The wasm-clean home for pure checks: `dro-core/src/rip.rs` beside
  `generate_description`/`parse_description` (the checks need only `RipMeta`
  + per-track facts; define a slim `TrackFacts { file_name, tag:
  Option<Gd3Tag>, loops: bool, readable: bool }` input so dro-core stays
  UI-free and the logic is table-testable).

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

### val-1 · dro-core: the readiness rules

`rip::readiness(meta: &RipMeta, tracks: &[TrackFacts]) -> Readiness`
implementing §3.1 with the three tiers (§2.1) and the shared date-rule fn
(`is_pack_date(s)`), plus `title_from_file_name` moved into dro-core and
re-exported for the dialog. Table-driven unit tests per rule (empty/dirty
values, date accept/reject table incl. `1994`, `1994-03`, `1994/03/01`,
`March 1994`; C1–C4 offenders listed by file name; L1 note only when a
readable track lacks a loop).

### val-2 · wire into RipState + export

`RipValidations` gains `notes`; `validations()` builds `TrackFacts` and
merges `readiness()` with the existing file-level checks. Export flow
unchanged except notes never gate. Tests: the existing validation gui tests
extended — errors still block, new warnings reach the confirm dialog, notes
do not.

### val-3 · the checklist panel + glyph column

Rip view: "Submission checklist" section (grouped, ✓-collapsed groups,
clickable items emitting the navigation actions) between the meta form and
the track table; ⚠/✓ glyph column in `track_table` with tooltip + click →
`OpenTrackQuickEdit`. Meta-field focus: stash a `focus_field:
Option<MetaField>` on `RipState`, honoured by the form next frame
(`request_focus` + `scroll_to_rect`). GUI tests: a dirty fixture renders the
expected items; clicking a track item opens quick-edit on the right track;
clicking a meta item focuses the field (assert via egui memory focus id).
Snapshots: rip view (clean and dirty fixtures).

### val-4 · date fix-assist + polish

"Convert dates to hyphens" button shown beside P2/T4/C4 warnings when the
offending values are slash-separated digits: rewrites meta.release_date and
every affected track's GD3 date as one `RipTransaction` batch (skip
unchanged), status totals. GUI test: mixed-format fixture converges to
hyphens in one undoable step. Update `TODO.md` + the `vgmrips-pack-gaps`
memory (item 6 → DONE) when it lands.

## 6 · Where everything lives

| Concern | Path |
| --- | --- |
| New rules + date fn + TrackFacts | `crates/dro-core/src/rip.rs` |
| RipValidations / validations() | `crates/dro-ui/src/rip.rs` |
| Export gate | `crates/dro-ui/src/app.rs` (`export_rip_zip`) |
| Checklist panel + glyph column | `crates/dro-ui/src/rip.rs` (`show`, `track_table`) |
| Quick-edit open / batch rewrite | `app.rs` (`OpenTrackQuickEdit`, `bulk_tag_submitted` precedent) |
| Title-from-filename (move) | `dialogs/track_edit.rs:143` → dro-core |

## 7 · Sources

- VGMRips wiki, "R(er)ipping for the Out-of-Element Contributor" — submission
  requirements digest (fetched 2026-07-20): complete/consistent GD3 tags,
  hyphen-separated dates, update notes, verified loops, ordered playlist.
- Existing checks and their consumption: `RipState::validations` and
  `export_rip_zip` as cited in §3.2.
