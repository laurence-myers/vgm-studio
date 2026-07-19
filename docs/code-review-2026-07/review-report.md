# DRO Trimmer — Rust branch code review

**Date:** 2026-07-19 · **Branch:** `rust` @ 2cb3d0f · **Scope:** all Rust crates (~24k lines; `vendor/nuked-opl3` excluded); Python consulted only as a parity oracle. **Report-only — no code was changed.**

**Method:** `cargo clippy --workspace --all-targets` baseline (zero warnings), my own close read of the UI shell (`app.rs`, `editor.rs`, `action.rs`, `platform.rs`, `tasks.rs`, `alert.rs`, `selection.rs`, `menus.rs`, `table.rs`), then eight parallel review agents (dro-core ×2, dro-synth+audio, dro-ui shell, dro-ui rip/dialogs/widgets, native shell, a cross-cutting UX-bug hunt, and a Python-parity audit), followed by first-hand verification of every High/Medium finding against source. The three top UI bugs were each found independently by two or three agents, which is strong signal.

## Verdict

The codebase is in excellent shape: clippy-clean, heavily and accurately commented, byte-parity constraints respected everywhere they matter, and an architecture whose layers all earn their keep — the headless `Editor`, the emit-`Action`-then-process frame loop, and the polled service traits are the right shape, and none of the reviews found a structural change worth making. **No rewrite or re-architecture is recommended.** What the review did find: a cluster of real interaction bugs concentrated where *rip mode and the editor share the audio service and file system* (the newest seams), one systemic gap (no unsaved-changes protection), a set of genuine duplication folds, and a short parity punch-list.

Totals: **3 High bugs, 10 Medium bugs/UX defects, ~16 Low items, ~20 simplification/duplication opportunities, 5 parity gaps** (2 of them deliberate-but-unrecorded).

---

## 1 · Bugs — High

### H1. Quick-edit dialog can rewrite and rename the WRONG track's file
`dialogs/track_edit.rs:28` stores only a bare `index`; Save emits it verbatim and `quick_edit_submitted` (`app.rs:1416`) resolves `rip.tracks.get(index)` at submit time. The track list is name-sorted and rescans *while the modeless dialog is open* (returning to the Rip tab rescans, `app.rs:1250-1257`; rename/rewrite/optimise outcomes rescan too), and nothing closes the dialog on rescan (`refresh_files` clears `preview` but not `track_edit`; `close_song_dialogs` doesn't include it). Sequence: rename track B so it sorts elsewhere → open Edit… on track C → the pending rescan reorders the list → Save rewrites another file's bytes with C's GD3 tag and renames it. The in-place rewrite happens even when the follow-up rename fails.
**Fix direction:** close `dialogs.track_edit` whenever the track list refreshes, or bind the dialog to the opened file name and re-resolve (and alert if gone) at submit.

### H2. No unsaved-changes protection for the editor song (and Exit bypasses even the rip prompt)
`Editor` has no dirty state at all; `Action::Exit` sends viewport Close unconditionally (`app.rs:892`), no `close_requested` interception exists anywhere, `load_file` replaces the song without a check, and opening a rip track loads over the editor silently. Python had the same hole (`wxapp.py:660-665`), so this is not a regression — but the port *added* dirty prompts for rip metadata (Open Rip Folder / Close Rip), so the cheap-to-retype half is protected while actual song edits (the app's purpose) are not, and File > Exit / the window X discard a dirty rip without hitting that prompt either.
**Fix direction:** a saved-revision watermark on `Editor` (revision at last successful save), route Exit/Open/track-open through the same confirm-alert pattern the rip already uses, and intercept window close.

### H3. A failed rip preview wedges editor playback — or makes it play the preview track
`preview_track` (`app.rs:1349-1366`) returns early on `audio.load` / `audio.play` failure *before* the `self.audio_revision = None` at the end of the success path. `NativeAudioService::load` unloads the old stream before trying the new one (`services/audio.rs:76`), so after a load failure the revision still matches while the service is empty → every editor Play short-circuits in `ensure_audio` and errors "No song is loaded into the audio output." until some edit bumps the revision. If load succeeded and `play` failed, the service holds the *rip track* while the revision claims the editor song → the editor's next Play plays the rip track under the editor UI (`rip.preview` was never set, so `stop_preview` can't clean up). Found independently by three reviewers; neither path is covered by tests (the fake audio service's `load` is infallible).
**Fix direction:** invalidate `audio_revision` unconditionally at the top of `preview_track` — the editor's snapshot is destroyed the moment any load is attempted.

## 2 · Bugs — Medium

- **M1. Case-only track renames always fail on Windows.** `rename_in_place` (`services/file.rs:170-180`): `dest == from` is case-sensitive `Path` equality, then `dest.exists()` matches the same file on NTFS → "already exists". Fixing capitalisation is a core VGMRips workflow on the maintainer's own platform. Related: the in-place rewrite serialises bytes *per the new name* before the rename outcome is known (`app.rs:1434-1447`), so a failed `.vgm`→`.vgz` rename leaves gzipped bytes under `.vgm`.
- **M2. Editor playback keeps playing under the Rip tab with every control hidden.** `select_tab` only stops *previews*; `open_folder` one function away documents and enforces "the editor's audio must not keep playing under the rip view" with `audio.unload()`. On the rip tab the transport is hidden and Space is inert (`app.rs:721-731`), so nothing can stop it; returning after song end leaves the readout frozen (the end-snap is editor-tab-gated).
- **M3. One Tab press silently kills the entire keyboard interface.** `gather_key_input` bails on `ctx.egui_wants_keyboard_input()`, which is true for *any* focused widget (egui 0.35 `memory.focused().is_some()`), and Tab focuses buttons by default while the bevel widgets paint no focus cue. After one Tab: all shortcuts incl. Ctrl+S dead, arrows move the invisible focus ring instead of the selection, and Space "clicks" the focused widget — if that's "Del.", Space *deletes the selection*. The comment describes text-field protection; the gate is far broader. **Fix direction:** let modifier shortcuts through unless a text edit is focused, and/or paint focus.
- **M4. Settings Save reverts a boost changed while the dialog is open.** The dialog snapshots the whole config at open expressly "so fields the dialog does not expose (e.g. `audio.boost`) are preserved" — but the dialog is modeless, the transport stays interactive, and Save rebuilds from the stale snapshot (`settings.rs:152-155`), silently clobbering the meanwhile-persisted boost (config/ini revert; the live stream keeps the boost until next reload — audible/visible disagreement). **Fix direction:** merge unexposed fields from the *current* config at apply time.
- **M5. Save As across formats writes unconverted bytes.** Bytes are serialised in the song's own format before the dialog; the filter list for songs includes "VGM files (*.vgm;*.vgz)" (`services/file.rs:13-18,193`); only the vgz↔vgm *compression* flip is fixed up post-save. A DRO saved as `song.vgm` is verbatim DRO bytes the app itself then refuses to reopen. Python had the same trap, but the port makes it more inviting (real VGM support, Convert to VGM one menu over). **Fix direction:** compare the picked extension with the song format — warn, refuse, or offer the existing conversion.
- **M6. Dropping a folder bypasses the dirty-rip confirm; dotted folder names are silently dropped.** `handle_drops` has no `rip_is_dirty()` check (menu path prompts), and its `extension().is_none()` folder heuristic drops "Game v1.2" entirely (extension `Some("2")`) with zero feedback. The service already routes by `is_dir()` — the UI-side heuristic is both wrong and redundant.
- **M7. File > Open works on the Rip tab but loads invisibly and strands the preview.** The menu item is always enabled; `load_file` neither switches to the Editor tab nor clears `rip.preview`, and because it unloads audio, `is_finished()` stays false so the ■→▶ cleanup never runs; meanwhile Ctrl+O is dead on that tab (only Save/Help are consumed). Status says "Successfully opened…" while the visible view doesn't change.
- **M8. Live mute/pan chip writes can be overtaken by still-queued song writes** (confidence: Medium). Playback writes go through the chip's timestamped write buffer (drained one entry per generated sample); `set_muting`/`set_panning` use immediate `write_reg` (`engine.rs:315,341-345` vs `:508`). A write burst that outlives a callback lets an already-queued key-on drain *after* the mute's key-off (note rings stuck — later writes to that register are gated), and a queued `0xD0` write can land after Custom panpots (the 9adc07-class clobber resurfacing through the queue window). **Fix direction:** route control writes through the same buffered channel, or flush the chip queue first. Offline renders are unaffected (muting set before render).
- **M9. The cpal callback allocates.** `let mut scratch: Vec<i16> = Vec::new()` grows inside the callback on first run (and on any larger buffer), contradicting the module's "Nothing locks in the audio path" (malloc may lock). Pre-size at stream build.
- **M10. Quick-edit rename validates nothing.** Renaming "01 A.vgm" to "01 A" (or a typo'd extension) succeeds end-to-end and the track silently vanishes from the list, the .m3u, and the export zip (`classify` → Other); `*.txt` even makes it a description-file candidate. The one field that renames a file on disk is the only unvalidated input in the app.

## 3 · Bugs — Low

- Failed `.txt` save still clears the rip dirty flag and reports "Saved …txt and …m3u" when the following `.m3u` save lands (`app.rs:630-647`; Failed outcomes skip purpose handling — the purpose was already popped).
- VGM metadata dialog validates the loop point against the length captured at open; edits behind the modeless window make Save silently accept a now-out-of-range loop that `set_vgm_metadata` then drops with only a `log::warn` (`vgm_metadata.rs:41,119`; `editor.rs:250-256`). Its read-only "Loop length" readout likewise shows the pre-edit value while you type a new loop point.
- "Building rip zip..." / "Optimising X..." status lines survive job failure indefinitely (failures raise alerts but never rewrite `self.status`).
- Song-bound modeless dialogs (Goto, Find Register, GD3, VGM metadata) stay open *and functional* on the Rip tab, mutating the hidden song — while the equivalent menu items are deliberately greyed out there ("so they cannot edit an unseen song", `menus.rs:40-42`).
- Optimising a screenshot stops a running track preview as a side effect of the post-save rescan (`ImageOptimised` → rescan → same-folder branch → `stop_preview`).
- One unreadable file (e.g. a locked .png) aborts the whole rip-folder open (`scan_folder` returns Err on first failed read), while unparseable *songs* are already tolerated per-track as "unreadable" rows — inconsistent policy.
- After a Settings frequency change mid-playback, the position panel mixes new-rate totals with old-rate live frames until the next reload (position can exceed length).
- Seek replays the whole instruction prefix synchronously inside the audio callback (`engine.rs:434-446`); fine for DROs, a deadline risk for 100k+-write VGMs.
- `vgm::io::write()` indexes the header before its only length guard runs — a short header panics instead of erroring. Unreachable today (reader enforces the minimum; the one `VgmMeta::new` caller synthesises 0x80 bytes), but the guard is in the wrong place.
- Doc-vs-code drift (all confirmed): `sum_delay_ms`'s comment names two callers that don't use it; `WaveformBucketer::push` says overflow frames are "ignored" but they fold into the last bucket; `Muting`'s doc attributes soloing to the CLI player, which deliberately doesn't have it.

## 4 · UX papercuts (not code defects)

- **Hex vs decimal mismatch (the biggest one):** the table's Pos. column is now `{index:04X}` while Goto parses decimal only and Find/status messages print decimal — reading "0064" and typing 64 lands on a different row, with no base hint anywhere. Python was decimal end-to-end; the hex look arrived with the FT2 theme. Either accept hex input in Goto (and print hex in statuses) or show decimal.
- Enter does not confirm alert boxes (Esc works); wx MessageDialog confirmed with Enter — keyboard regression, and confirm-boxes are keyboard-unconfirmable short of Tab-focusing OK.
- Digit mute shortcuts fire with Ctrl/Alt held (every other shortcut requires exact modifiers), and OPL3 high-bank channels 10–18 have no keyboard toggles at all (keys stop at 9; `toggle_channel` supports 0..18).
- Wrong-extension and multi-file drops are ignored with zero feedback; the adjacent comment claims junk files alert, which is only true for extensionless ones. wx gave visible drag-time refusal; a status-line note is the egui-available equivalent.
- Invalid-input alerts are inconsistently titled ("Invalid settings" vs "Error"+different wording); the Theme hover "Takes effect immediately" reads as live-preview but means "on Save".
- No application/window icon: `dt.ico` sits unused in the repo; the viewport builder never calls `.with_icon()` and there's no `build.rs`/resource embedding — default exe/taskbar icon.

## 5 · Architecture, simplification, duplication

**Overall:** the layering (dro-core pure model / dro-synth pull engine / dro-ui headless app / dro-trimmer native shell) is clean and each abstraction was checked for pulling its weight — the engine's dual genericity (song container + chip) has live instantiations on both axes, the service traits are the documented web-port seam, splice.rs's three callers justify it, and the enum-over-trait song dispatch is a documented choice whose ~30-line glue residue is acceptable. The recommendations below are targeted folds, not redesign.

**Worth doing (best value first):**
1. **Dialog scaffolding helper** — six modeless dialogs repeat ~20 lines each of identical window-frame + Close/Save footer + open/close verdict plumbing (`settings/vgm_metadata/gd3_tag/track_edit/goto/find_reg`). One helper leaves each dialog holding only its grid body and save logic.
2. **Share the GD3 form** — `track_edit.rs` duplicates `gd3_tag.rs`'s 11-entry LABELS table and field-grid loop wholesale (its own header admits it's "the GD3 dialog plus a File name field"). One LABELS const + one `gd3_fields(ui, …)` helper.
3. **Rip description field table** — `generate_description` and `parse_description` hand-maintain the same ten labelled fields in two lockstep lists (a third echo in the UI form). One ordered (label, aliases, accessor) table driving both; output bytes unchanged, pinned by golden tests.
4. **Two word-wrappers in `rip.rs`** — `push_wrapped_block` and `wrap_value` are the same greedy algorithm parameterised differently; unify on `(first_width, continuation_width)` (byte-locked by three golden tests). Plus the twice-encoded right-aligned time-row helper.
5. **VGM read path walks the command stream twice** — `read_commands` validates opcode sizes/truncation, then `VgmData::build_offsets` immediately re-walks identically. Let one walker serve both (read path only; cannot affect written bytes).
6. **Boost stepper → widget module** — the densest widget code in `app.rs` (~90 lines: nested RTL layout, scoped visuals, DragValue formatter/parser) is fully self-contained and matches the existing `widgets/pan_knob.rs` shape. With the waveform row too, `update_impl` drops from ~337 to ~230 lines of pure panel scaffolding. Further extraction is *not* recommended.
7. **Dead API prune** (all grep-verified): `RipService::cancel` + `TaskService::cancel` (trait level; native impls can keep them inherent), `Selection::len` (zero callers anywhere), `PlayerEngine::{sample_rate, muting, panning}` getters, `render_waveform_cancellable` export (+ `total_output_frames`'s unused genericity), `io::dro::sum_delay_ms` (dead with a misleading doc), `util::to_timestr` pub, `preset_for`/`music_hardware_suggestion` double entry point.
8. **v1 opcode constants** — `convert.rs` re-encodes the DRO v1 opcode table (incl. the `reg < 0x05` escape threshold) as magic numbers because `mod v1_opcode` is `pub(super)`; widen to `pub(crate)` and use the names.
9. **Small folds:** shared `read_song_from_path` for the three CLI bins' identical preamble (that's what lib.rs is for); one `Position::from_frames` constructor for the elapsed-ms formula duplicated across dro-synth/dro-audio-native; Find Register token list sourced from dro-core instead of a second hardcoded copy (app silently drops parse failures of its own strings); the dual-OPL2 hard-L/R pan image built in one place instead of two; `RegisterUsage::percussion` as `BTreeSet` (it's a `BTreeMap<u16, bool>` that only stores `true`).
10. **ThreadTaskService** — per-kind HashMap machinery with a *global* generation counter: correct only while `TaskKind` has one variant, and a silent-results trap the moment a second kind is added. Either make the generation per-kind or collapse to the single-slot shape `NativeRipService` uses.

**Performance (all in draw paths or hot paths):**
- The rip track table recomputes `TrackEntry::from_song` — full instruction-stream sample sums with no prefix cache — for every track, every frame (`rip.rs:576`), and the rip view repaints continuously during preview. Cache per track at scan time.
- The screenshot preview clones the full PNG `Vec<u8>` into a fresh `Arc` every frame (`rip.rs:676-682`) just for the loader to hit its by-URI cache; hold `Arc<[u8]>` once per scan.
- `dro_split` clones each rendered multi-MB WAV buffer only to unify match-arm types.
- (M9's callback allocation belongs here too.)

**Explicitly checked and fine as-is** (so these aren't re-litigated): the delete path's triple sanitisation (defence-in-depth; middle pass could go with a documented precondition), the v1/VGM variable-length glue, undo command shapes (two commands, genuinely different state), the bank-tracking loop idiom, the worker-thread spawn parallel (documented), the two sample clocks (`SampleClock` is byte-locked, `FrameClock` general — unifying would couple them for ~3 lines), config.rs's field symmetry (a table can't carry its per-field ini comments), tests/common's FrameClock copy (worth one "independent oracle" comment), and the wasm placeholder crates (zero-weight, correctly staged).

## 6 · Feature parity vs Python

Broad parity is **confirmed**: every menu item and shortcut, the load-warning flow, all six dialogs (with Python's VGM-metadata Save actually *working* now — the original's button was bound to the wrong ID), waveform behaviours incl. the 1 s debounce, position-panel semantics, all seven `drotrim.ini` keys with configparser-compatible parsing and identical defaults, window title, drag-and-drop (superset), and the three CLI bins' flags and naming schemes. Gaps:

1. **`audio.buffer_size` is a dead knob** (Medium): parsed, validated, editable in Settings — and consumed by nothing; the stream opens with `cpal::BufferSize::Default`. Python used it as the render/PyAudio chunk size (latency tuning). Wire it into the `StreamConfig` or remove/grey the field.
2. `--version` lost from `dro_player` and `dro_split` (only `drotrim` has `#[command(version)]`).
3. CLI progress/skip feedback lost: `dro_split` renders every channel silently (Python printed skip lines, live MM:SS per channel, and completion lines), same for `dro_player --render` (live playback kept its progress line).
4. No app icon (see UX above).
5. Deliberate but unrecorded divergences, now noted: DRO Info is view-only for VGMs (safety-justified in a comment), and the sixth "all register options" column became a hover tooltip.

## 7 · Test-coverage gaps worth closing (highlighted by multiple reviewers)

The kittest suite is strong (playback reuse, dialogs, rip flows, panning regressions, five snapshots), but none of the confirmed High/Medium bugs is on a pinned path: the fake audio service's `load` is infallible (hides H3), no test switches tabs during playback (M2), Tab-focus behaviour, modifier+plain-key handling, the vgz-flip re-save, PlayTail/Rewind/waveform-click handlers, the FIFO save contract beyond back-to-back in-place saves, and the rename flow are all unpinned. `tests/rip_flow.rs` is happy-path only.

---

### Where the raw material lives
Per-agent findings files (full evidence chains) are in this folder (`docs/code-review-2026-07/`): `findings-{core,vgmrip,synth,uishell,uiwidget,native,ux,parity}.md`. Every High/Medium item above was re-verified against source before inclusion; agent-reported severities were kept except where a fuller trace justified promotion (the quick-edit index bug).
