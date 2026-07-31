# Un-gate the remaining OPL-only features (all VGMs get everything; DRO keeps DRO)

## Context

The 13-step un-gating programme (complete 2026-07-30, `OPL-UNGATING-PLAN.md`) opened
most features to any-chip VGMs, but a cluster survived: everything built on **peak
measurement** (`measure_peak` only exists for the OPL `PlayerEngine`), the **Render
to WAV dialog's** gate, and a few stragglers — two of which are **live panics** for
non-OPL VGMs (Play Tail; Save As, which is *every* save on the web target since
`PickedFile::path` is always `None` there). Branch: `web-target` (has all prior
un-gating work; wasm must keep building). Implemented on branch `ungate-remaining-opl`.

**Features being un-gated** (all "incidental" — generic operations routed through `Editor::song()`):

| Feature | Gate today | Symptom |
|---|---|---|
| Match (volume lever) | `submit_volume_scan` app.rs:1082 via `editor.snapshot()` | "This needs an OPL song." |
| Measure (VGM Metadata dialog) | same path, app.rs:1071 | dead button in an otherwise generic dialog |
| Pack Scan Volumes → Apply/Album | app.rs:2990 `PackTrack::playable_song()` (= OPL projection) | Peak column "-" forever; levelling greyed |
| Render to WAV… dialog | app.rs:1607 `require_song()` | menu shows (any-chip `can_render`), click refuses |
| Play Tail | app.rs:3432 `song().expect("gated")` after `require_playable` | **panic** |
| Save As / pathless Save | app.rs:2264 `song().expect("gated").name` after `require_document` | **panic** (always on web) |
| Header volume modifier at load | app.rs:1035 `song_modifier_boost()` | non-OPL VGM opens at 1.0× |
| Position length after edits | app.rs:3908/3919, 3671, 4261 | stale length for non-OPL after crop/delete |
| Pack hardware Presets row | only OPL-2/Dual OPL-2/OPL-3 exist | user chose: **add console presets** |
| Stale wording | CLI help, PACK_OPT_TIP, comments | says "OPL" about now-generic operations |

**Deliberate gates that STAY** (verified, do not touch): CLI `split --song` OPL-only
(cli/split.rs:76); vgmtools pipeline OPL bypass (pipeline.rs:173); RetroWave OPL3-only
(services/retrowave.rs:69, cli/play.rs:55); DRO Info / Convert to VGM / Convert to
DRO v1 (DRO-specific, menu-gated `is_dro`); SplitDialog hiding OPL options for generic VGMs.

**Key existing pieces to reuse**: `audio_source()` app.rs:4236 (dual-arm snapshot:
`AudioSource::Opl(snapshot)` else `Arc<VgmFile>` clone); `VgmEngine` (vgm_engine.rs —
same `render`/`position` pull contract as `PlayerEngine`; fresh engine has
`loop_config: None` so one pass terminates, mirror `render_vgm_wav_mixed_cancellable`
wav.rs:349); `do_play_seam` app.rs:3446 (correct dual-arm shape);
`editor.timeline()`/`TimeSource::total_ms`; `preview_source()` pack.rs:133 (the
"renderable" track predicate); `suggested_modifier_transaction` pack.rs:452 +
`PackTrack::revolumed` pack.rs:195 (already generic); `volume.rs` maths (all generic);
`VgmFile::vgm_meta()` fills `volume_modifier` from the header (file.rs:289) so generic
header reads agree with the projection.

**Cross-cutting rules** (every step = one conventional commit, green):
- `cargo test --workspace`; `cargo check --target wasm32-unknown-unknown -p vgms-core -p vgms-synth -p vgms-ui -p vgms-web -p vgms-synth-worklet`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all --check`
- **wasm worker codec**: any `TaskRequest` shape change must land in the same commit as `crates/vgms-web/src/codec.rs` (VolumeScan = tag 4, PackVolumeScan = tag 5; helpers `write_audio_source`/`read_audio_source`, `write_resample`/`read_resample` already exist).
- Steps 1–11 must leave kittest PNG baselines untouched (investigate any diff, don't regenerate). Only step 12 regenerates (`UPDATE_SNAPSHOTS=1 cargo test -p vgms-ui`).

## Steps

### 1. `fix(ui): suggest the document's own name when saving a non-OPL VGM`
- `editor.rs`: add `pub fn document_name(&self) -> Option<&str>` — vgm name first, else dro name.
- `app.rs`: Save As / pathless Save uses `document_name()` for the suggested name.
- Tests: `document_name_names_either_kind_of_document`; `save_as_offers_the_documents_own_name_for_a_non_opl_vgm`; `saving_a_pathless_non_opl_vgm_falls_through_to_the_dialog`.

### 2. `fix(ui): play tail reads the timeline, not the OPL song`
- `do_play_tail`: `editor.timeline().map(|t| t.total_ms())` instead of `song().expect`.
- Test `play_tail_seeks_near_the_end_of_a_non_opl_vgm`.

### 3. `fix(ui): keep the position length current for non-OPL documents`
- Four sites → `editor.timeline()`-based (`after_edit`, `apply_settings`, `ensure_audio`).
- `widgets/position_panel.rs`: add `pub fn length_ms(&self) -> u32`.
- Test `editing_a_non_opl_vgm_keeps_the_position_length_current`.

### 4. `fix(ui): honour the header volume modifier when loading a non-OPL VGM`
- `song_modifier_boost`: read `VgmHeader::volume_modifier` for a non-OPL VGM.
- Load path: run the lock-gated boost block in both arms.
- Test `a_non_opl_vgms_header_modifier_sets_the_load_volume`.

### 5. `feat(ui): offer Render to WAV for any renderable document`
- `require_renderable`; gate `OpenRenderWav` on it; `can_render` empty-editor escape.
- `dialogs/render_wav.rs`: generic mode hides the OPL-only channel toggle/pan rows.
- Tests: `render_to_wav_dialog_opens_for_a_non_opl_vgm`, `an_empty_editor_still_offers_render_to_wav`.

### 6. `feat(synth): measure a VGM's peak through the generic engine`
- `peak.rs`: `measure_vgm_peak[_cancellable]` via `VgmEngine`; export from `lib.rs`.
- Tests: peak equals the WAV render's peak; cancellation; shorthand equivalence.

### 7. `refactor(ui,web): carry volume scans as AudioSource pairs` (behavior-neutral)
- `tasks.rs`: `VolumeScan`/`PackVolumeScan` carry `AudioSource` + `resampling`; `measure_source` helper.
- `app.rs` constructors shape-only. `codec.rs` tags 4/5 re-encoded — same commit (wasm).
- Tests: updated scans + VGM-arm scan + codec round-trips.

### 8. `feat(ui): un-gate Match and Measure for renderable VGMs`
- `submit_volume_scan` uses `require_renderable` + `audio_source`.
- Tests: `match_volume_measures_a_non_opl_vgm`, `measuring_the_modifier_fills_for_a_non_opl_vgm`, `match_volume_on_a_coreless_vgm_reports_nothing_to_play`.

### 9. `feat(ui): pack volume scan covers renderable non-OPL tracks`
- `scan_pack_volumes` filters via `preview_source`; delete `PackTrack::playable_song`.
- Test `pack_scan_measures_non_opl_tracks_too`.

### 10. `docs: refresh wording that says optimise/scan/split are OPL-only`
- CLI help, `PACK_OPT_TIP`, `pack_zip.rs`/`pack.rs`/`app.rs`/`menus.rs` comments. No behavior change.

### 11. `refactor: drop the dead editable capability and revolumed_bytes`
- Remove `DocCapabilities.editable` and `pack::revolumed_bytes` (+ its test).

### 12. `feat(pack): console hardware presets` (user-approved)
- `vgms-core/src/pack.rs`: `CONSOLE_PRESETS` (Mega Drive, Master System, NES, Game Boy, PC Engine, Neo Geo, X68000, PC-98).
- `vgms-ui/src/pack.rs`: second preset row. Verify empty-`os` handling. Regenerate snapshots.

## Verification
- Per step: the four CI commands above.
- Steps 1–2 turn today's panics into passing tests; step 5's dialog test covers the gate the existing render test side-steps; step 8's coreless test pins the refusal.
- End-to-end sanity after step 9: non-OPL VGM → Match / Measure / Render / Play Tail / Save As all work; non-OPL pack → Scan + Apply/Album work.
