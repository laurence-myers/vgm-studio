# DRO Trimmer — remediation plan for the 2026-07-19 review

Report-only planning. Nothing here is implemented. Fix *directions* with enough
specificity to act, a test hook, an effort/risk tag, and a batching order that
respects the "one step at a time, confirm before the next" workflow.

Effort: **S** ≤~20 lines/one fn · **M** a few fns / new small module / new test infra · **L** cross-cutting state + multiple sites + tests.
Risk: chance of regression or subtlety.

---

## 0 · Constraints every fix must respect (don't regress these)

- **Byte-parity is sacred.** VGM writer preserves the source header; VGZ gzip is byte-identical native-vs-wasm (flate2 `rust_backend` pin); rip `.txt`/`.m3u` byte-match the VGMRips template; DRO→VGM rounding is fixture-locked (`lsl3`). Any edit to `vgm/io.rs`, `rip.rs`, `convert.rs`, or gzip must keep the golden tests green — they are the safety net that makes the rip/convert *refactors* safe.
- **Real-time audio path.** The cpal callback must stay alloc-free and lock-free; the engine renders byte-identically to `golden_opl.rs` / `c_parity.rs`. The M8/M9 fixes must keep those green and add no lock/alloc.
- **wasm-clean split.** `dro-core`/`dro-synth` must not gain native-only deps. Icon, rename, buffer-size, and CLI-progress fixes live in native crates only.
- **Toolchain.** MSRV 1.97; egui/eframe/egui_kittest move in lockstep at 0.35. Any themed-surface change needs the two `theme_showcase` snapshot baselines regenerated on the maintainer's machine (`UPDATE_SNAPSHOTS=1`; DX12 WARP adapter).
- **Process.** Tests alongside each fix; don't commit unless asked; land one reviewable step at a time.

## 0.1 · Test infrastructure this plan adds first (prerequisite)

Several High/Medium bugs are invisible to the current suite because **`FakeAudioService::load` is infallible** (`test_support.rs:120`). Before the audio fixes:

- **Fallible fake audio** — add a switch to `FakeAudioService` so a test can make the next `load` and/or `play` return `Err`. Unblocks H3, and pins the preview error paths. **M / Low risk.**

That one addition is the gate for Batch A below.

---

## 1 · High-severity bugs

### H1 — Quick-edit can rewrite/rename the WRONG file  (rip index vs rescan)
Two-part fix, correctness first then defensive:
- **(a, correctness)** Bind `TrackEditDialog` to the file **name** it opened on, not just the index. In `quick_edit_submitted` (`app.rs:1416`), re-resolve the track by that name; if it's gone, alert and abort. A reorder can then never retarget the write. Store the original name in the dialog struct (`track_edit.rs:27`) — the index becomes a hint, the name the identity.
- **(b, defensive)** Close `dialogs.track_edit` wherever the track list refreshes — add it next to the existing `preview = None` reset in `refresh_files`, and to a "close rip-bound dialogs on rescan" step (today `close_rip_dialogs` only runs on a *different* folder).
- **Test:** kittest — open quick-edit on track C, trigger a same-folder refresh that reorders the list, Save; assert the file that changed is C's (or that the dialog closed). **Effort M · Risk Low.**

### H2 — No unsaved-changes protection (Exit/X/Open/track-open discard edits)
Biggest single fix; build it on the pattern the rip dirty-prompt already uses (`ConfirmOpenRipFolder`):
- **Dirty watermark on `Editor`.** Add `saved_revision: Option<u64>`; `is_dirty()` = `has_song() && Some(revision) != saved_revision`. Set it in the Song save-outcome success arm (`handle_save_outcome` → `SavePurpose::Song`) and on load/convert (fresh song = clean). Equality-compare (revision is monotonic, never reused) so redo back to the saved point reads clean.
- **Close interception.** In `update_impl`, read `ctx.input(|i| i.viewport().close_requested())`; if `editor.is_dirty() || rip_is_dirty()` and not already confirmed, `ctx.send_viewport_cmd(ViewportCommand::CancelClose)` and queue a confirm alert carrying a new `Action::ConfirmExit` that sets a `quitting` flag and re-sends `Close`. (Works with the custom `App::ui` shell in `drotrim.rs` — no eframe-version-specific hook needed.)
- **Open/track-open guards.** Add a `pending_load: Option<PickedFile>` slot; when `poll_picked`/`open_track_in_editor` would replace a dirty editor song, stash the file and queue a confirm alert with `Action::ConfirmDiscardAndLoad` — mirroring `ConfirmOpenRipFolder` exactly. `Action::Exit` gets the same dirty check.
- **Test:** kittest — dirty the editor, fire `Exit` → assert a confirm alert appears and the app didn't close; confirm → closes. Same for Open. **Effort L · Risk Med** (close-request wiring is the subtle part; add a focused test).
- Note: this is a *new* protection, not a Python regression fix — Python had the same hole. Worth doing because the port already protects the cheaper rip metadata.

### H3 — Failed rip preview wedges editor audio / plays the wrong song
- Move `self.audio_revision = None;` to the **top** of `preview_track` (`app.rs:1349`), before the `load` attempt — the editor's snapshot in the service is destroyed the moment `load` calls `unload()`, success or not.
- On the `play`-failure branch, also `self.audio.unload()` (or `stop_preview`-equivalent) and leave `rip.preview = None`, so the service doesn't hold a half-started preview.
- **Test:** with the fallible fake audio (0.1), fail preview `load`, then editor `Play` → assert `ensure_audio` reloads (no "No song is loaded" wedge); fail preview `play` → assert editor `Play` reloads the editor song, not the rip track. **Effort S · Risk Low.**

---

## 2 · Medium bugs

Grouped by root cause so related fixes land together.

### Cluster: rip/editor audio+tab coupling (do with H3)
- **M2 — editor audio plays under the Rip tab.** In `select_tab` (`app.rs:1241`), when leaving the Editor tab, `self.audio.unload(); self.audio_revision = None;` — mirror `open_folder`'s documented rule. **S · Low.** Test: switch tab mid-play → `!is_playing()`.
- **M7 — File>Open on Rip tab loads invisibly / strands the ▶.** At the top of `load_file`, `self.stop_preview(); self.active_tab = AppTab::Editor;` (covers menu, drop, and CLI paths; idempotent for `open_track_in_editor` which already sets the tab). **S · Low.** Test: on Rip tab, load a song → asserts tab flips to Editor and no stranded preview.
- **ux-18 — optimise stops a running preview** (Low, same cluster): make the same-folder `refresh_files` preserve a playing preview by re-matching the preview by file name instead of nulling it. Fold into the "refresh_files keeps identity by name" change H1(b) already touches. **M · Low.**

### Cluster: dialog staleness (do with H1)
- **ux-13 — song-bound modeless dialogs live on the Rip tab.** In `select_tab` entering Rip, call `close_song_dialogs()` + close Goto — mirror the menu gating. **S · Low.** (Same function as M2.)
- **ux-8 / uiwidget-10 — VGM metadata stale-length loop point + stale readout.** Re-validate the loop point at save against the *live* length and surface the drop as an alert/status instead of the silent `log::warn` in `set_vgm_metadata` (`editor.rs:250`); recompute the read-only "Loop length" display from the currently-typed loop point (capture a samples-before handle at open). **M · Low.**

### Keyboard (Cluster D)
- **M3 — one Tab press disables the whole keyboard.** Recommended two-pronged fix: (1) replace the `ctx.egui_wants_keyboard_input()` gate in `gather_key_input` with an explicit "a text-bearing dialog is open" check (the editor view has *no* text inputs — all text lives in dialog windows — so the current gate's only true purpose is dialog protection, and the false-positive that kills shortcuts after Tab disappears); (2) additionally consume Tab/Shift+Tab on the editor view (the app has no focus-traversal use) so a stray Tab can't focus a chrome button and let Space activate it. **M · Risk Med** — confirm the egui 0.35 focus/consume API with a quick spike; add a test that Tab-then-Space still plays rather than deletes.
- **ux-14 / uishell-4 — digits fire with modifiers; ch 10–18 unreachable.** In the plain-key block require `!modifiers.command && !modifiers.alt` (keep Shift for `SelectionMove`); add Shift+1..9 → channels 9..17 for the high bank. **S · Low.**
- **ux-12 — Enter doesn't confirm alerts.** In `alert::show_front`, Enter = OK for info alerts and (matching wx) for confirm boxes; focus OK on open. **S · Low.**

### File/format safety (Cluster E)
- **M1 / ux-9 — case-only rename fails on Windows.** In `rename_in_place` (`file.rs:170`), when `dest.exists()` but `dest` is the *same* file as `from` (case-insensitive filename compare in the same dir), rename via a temp name so NTFS updates the case; only then. Also reorder `quick_edit_submitted` so the in-place byte rewrite (in the target format) happens **after** the rename succeeds, not before — fixes the "surviving .vgm holds .vgz bytes" variant. **M · Med** (pure-fs, unit-testable in a temp dir — good coverage).
- **M5 / ux-2 — Save As across formats writes unconvertible bytes.** Correctness fix: after the save dialog returns a name, if the chosen extension's format ≠ the song's format, alert ("Save As can't change format — use Convert to VGM") and abort. Polish: narrow `save_filters` for a song to its own format so a DRO only offers `.dro`. **M · Low.**
- **M10 — quick-edit rename validates nothing.** Validate on Save (in `track_edit`/`quick_edit_submitted`): non-empty, ends in `.vgm`/`.vgz`, no collision with another track; alert + keep the dialog open otherwise (same "return false, stay open" shape as `settings.save`). **S–M · Low.** Best done after the dialog-scaffolding fold (§4) so "stay open on invalid" is shared.

### Settings / rate (Cluster: config apply)
- **M4 / ux-15 — Settings Save reverts a concurrently-changed boost.** Fix handler-side, keep the dialog dumb: in `apply_settings` (`app.rs:1604`), preserve the live-changed fields — `config.audio.boost = self.config.audio.boost;` before installing — or have the dialog only carry the fields it edits and merge onto the current config. **S · Low.**
- **ux-16 — position panel mixes new-rate length with old-rate live frames after a frequency change.** While a stream is loaded, keep the panel on `audio.output_rate()` (as `ensure_audio` already does) and adopt the configured rate only on reload; in `apply_settings`, don't `set_frequency(new)` if a stream is live. **S · Low.**

### Audio engine real-time (Cluster F)
- **M8 — live mute/pan writes can be overtaken by queued song writes** (Confidence Med). Two candidate fixes: (i) route `set_muting`/`set_panning` register writes through `write_reg_buffered` so ordering holds by construction, or (ii) drain the chip's write buffer to "now" before the immediate `write_reg`s (keeps instant semantics). Start by writing a `RecordingChip` write-order test that reproduces burst-then-mute → stuck key-on and burst-then-pan → clobber; then pick whichever fix keeps that test **and** `golden_opl`/`c_parity` green. **M · Risk Med** — the trickiest; may need a small `flush` entry point on the chip wrapper.
- **M9 — cpal callback allocates its scratch Vec.** Pre-size `scratch` at stream build to the max buffer (device buffer-size upper bound, or the wired `buffer_size` from parity-1, or a generous cap); keep the in-callback resize as a never-hit fallback. **S · Low.** Naturally pairs with parity-1.

---

## 3 · Low bugs & UX papercuts (batch as polish)

- **uishell-7** — failed `.txt` save still clears the rip dirty flag: route `Failed`/`Cancelled` through the popped purpose; only clear `dirty` when the batch's last `RipDoc` lands with no earlier failure (per-batch failure flag). **S–M.**
- **ux-11** — stale "Building…/Optimising…" status after failure: set `self.status` in the failure match arms. **S.**
- **ux-17** — silent ignored/multi drops: set a status line ("Unsupported file type" / "Drop a single file"). **S.**
- **ux-19** — one unreadable file aborts the whole folder open: change `scan_folder` to skip-and-collect errors and surface them as a warning (like the per-track "unreadable" rows), instead of `return Err` on first failure. **M.**
- **uiwidget-11** — standardize invalid-input alert titles/wording across dialogs; reword the Theme hover ("Applied on Save; no restart needed"). **S.**
- **vgm write short-header panic** (vgmrip-5) — hoist one `MINIMUM_HEADER_SIZE` check to the top of `write()` so a short header errors instead of panicking (unreachable today, but cheap correctness). **S.**
- **synth-3** — in-callback seek replays the whole prefix: **defer** unless large-VGM playback becomes a goal; note as a known limitation. If needed, bound per-callback replay across callbacks (position semantics unchanged). **M, deferred.**
- **Doc-vs-code fixes** (S, batch together): `sum_delay_ms` comment (it's being deleted anyway, §4), `WaveformBucketer::push` "ignored" vs folds-into-last-bucket, `Muting` doc "CLI player's soloing" → GUI channel panel.

---

## 4 · Simplification / duplication folds (quality-only track)

These don't fix bugs; do them to reduce drift risk. Sequence note *(DECIDED 2026-07-19)*: do the **dialog scaffolding + GD3-form** folds **FIRST** — as pure, behavior-preserving refactors that stay green (all existing dialog + snapshot tests pass unchanged) — **then** land the dialog bug fixes (H1/M10/ux-8/uiwidget-10) in the folded code, one place per fix.

**Higher value:**
- **Dialog scaffolding helper** (uiwidget-4): a `dialog_window(ctx, title, area) + footer(ui, palette, buttons) -> verdict` pair folds ~20 lines × 6 dialogs (settings/vgm_metadata/gd3_tag/track_edit/goto/find_reg) down to grid-body + save-logic. **M.** Enables M10's "stay open on invalid" in one place.
- **GD3 form share** (uiwidget-5): one `LABELS` const + `gd3_fields(ui, palette, &mut [String;11])`; `track_edit` prepends its File-name row. **S–M.** Do with H1.
- **Rip description field table** (vgmrip-2): one ordered `(label, parse-aliases, accessor)` table driving both `generate_description` and `parse_description`. Byte-locked by golden tests → safe. **M.**
- **Two word-wrappers + time-row** (vgmrip-3, vgmrip-4): unify on one `(first_width, continuation_width)` wrapper; shared `push_aligned_row`. Byte-locked. **M.**
- **VGM read double-walk** (vgmrip-1): one command-stream walker serves the file read and `build_offsets`. Read-path only — no output-byte impact. **M.**
- **Boost stepper → `widgets/boost_stepper.rs`** (uishell-6): extract ~90 lines; `update_impl` drops to ~230 lines of panel scaffolding. Stop there (the rest is fine inline). **M.** Touches a snapshot? No new visuals → baselines unchanged, but re-run to confirm.

**Dead-API prune** (one commit; all grep-verified unused): `RipService::cancel` + `TaskService::cancel` (keep inherent on native impls), `Selection::len`, `PlayerEngine::{sample_rate,muting,panning}`, `render_waveform_cancellable` export (+ `total_output_frames` genericity), `io::dro::sum_delay_ms` (+ test), `util::to_timestr` pub, collapse `preset_for`/`music_hardware_suggestion`. **S · Low.**

**Small folds:** widen `v1_opcode` to `pub(crate)` + use the names in `convert.rs` (core-2); `read_song_from_path` helper for the 3 CLI bins (native-4); one `Position::from_frames` for the elapsed-ms formula (synth-6); source the Find Register token list from dro-core (uiwidget-7); one `dual_opl2_image()` (uiwidget-8); `RegisterUsage::percussion` → `BTreeSet` (core-6); `ThreadTaskService` single-slot instead of per-kind HashMaps-with-global-generation (native-3); tests/common reuse the exported `FrameClock` (synth-8); `dro_split` write-without-clone (native-5). Each **S**.

**Perf:**
- **uiwidget-3** — cache `TrackEntry` (or its display strings) on `RipTrack` at scan/refresh instead of recomputing full sample-sums per row per frame. Biggest runtime win (rip view repaints during preview). **M.**
- **uiwidget-9** — hold screenshot bytes as `Arc<[u8]>`/`Bytes` converted once per scan, not cloned per frame. **S–M.**

**Judged fine — do NOT touch** (so they're not re-raised): the two sample clocks (byte-locked vs general), delete-path triple sanitisation (defensible defence-in-depth), undo command shapes, enum-over-trait song dispatch + its glue, worker-spawn parallel, config field symmetry, wasm placeholder crates.

---

## 5 · Feature parity gaps

- **parity-4 — `--version` on bins:** add `#[command(version)]` to `dro_player`/`dro_split`. **S · trivial.**
- **parity-1 — `buffer_size` dead knob → DECIDED (2026-07-19): WIRE IT.** In `build_stream`/`NativeAudio::new` (`dro-audio-native/src/lib.rs:94`) set `config.buffer_size = cpal::BufferSize::Fixed(audio_config.buffer_size)` instead of `Default`, **guarded by the device's supported range** (`Device::supported_output_configs()` → `SupportedStreamConfigRange::buffer_size()` gives a `SupportedBufferSize::Range { min, max }`): if the requested frame count is outside it — or the host rejects `Fixed` (WASAPI can) — clamp into range or fall back to `Default`, and `log::warn!`. Preallocate the callback `scratch` to `buffer_size * 2` i16 (this *is* M9's preallocation size, so M8/M9/parity-1 land together in Batch F). The engine is buffer-size-agnostic by design (its comment), so this changes only callback size/latency, never rendered bytes — golden tests unaffected. **M.**
- **parity-2 — hex Pos. vs decimal Goto/Find:** *DECIDED (2026-07-19): keep the hex Pos. column.* Make Goto parse hex, keep `0x` working in Find, print the Pos. in hex in find/status messages, **and make the hex-ness explicit in the label** — e.g. header "Pos (hex)" and/or a "(hex)" hint on the Goto field — so "0064" vs typed "64" can't confuse. **S–M.**
- **parity-5 — CLI progress/skip output:** re-add skip lines + live `MM:SS` progress to `dro_split` and `dro_player --render`. `split()` renders to completion before returning, so live progress needs a progress callback hook (like the waveform emit). **M.**
- **parity-3 — no app/window icon:** load `dt.ico` → RGBA `IconData` via `.with_icon()` for the window, and embed the exe icon (build.rs/winres/embed-resource, native-only). **M.**
- **parity-6/7 — deliberate** (DRO Info VGM view-only; sixth column → hover): no code change; add them to the documented-divergence list so they stop resurfacing in reviews. **S.**

---

## 6 · Recommended sequencing (each step independently green + testable)

1. **Test infra:** fallible `FakeAudioService`. *(gate for step 2)*
2. **Batch A — audio/rip coupling:** H3 + M2 + M7 + ux-18 (+ ux-13 in the same `select_tab`). One mental model, mostly `app.rs`.
3. **Batch B — dialog fold FIRST, then its bug fixes** *(DECIDED)*: land the dialog-scaffolding helper (uiwidget-4) + GD3-form share (uiwidget-5) as pure green refactors, **then** fix H1 + M10 + ux-8 + uiwidget-10 in the folded code.
4. **H2 — unsaved-changes protection.** Its own step (largest).
5. **Batch C — keyboard:** M3 + ux-14 + ux-12.
6. **Batch D — file/format safety:** M1/ux-9 + M5/ux-2.
7. **Batch E — config apply:** M4/ux-15 + ux-16.
8. **Batch F — engine real-time:** M8 + M9 (+ parity-1 buffer wiring). Careful; own step.
9. **Batch G — polish:** uishell-7, ux-11, ux-17, ux-19, uiwidget-11, vgmrip-5, doc fixes.
10. **Batch H — perf:** uiwidget-3, uiwidget-9, native-5.
11. **Batch I — parity:** parity-4 (trivial), parity-2, parity-5, parity-3, parity-6/7 docs.
12. **Batch J — remaining folds:** rip field-table, word-wrappers, VGM double-walk, boost-stepper, dead-API prune, small folds.

Highs (steps 2–4) first; folds (3, 12) placed to avoid double-refactoring; quality-only work (10–12) last.

## 7 · Decision points — status

**Resolved 2026-07-19 (user):**
1. **H2 scope → FULL** — dirty watermark + window-close interception + confirm-prompts on File>Open, opening a rip track, and Exit. (§1 H2 already describes this shape.)
2. **parity-2 → keep hex Pos.** — make Goto/Find hex-aware and label the column clearly as a hex position (see §5 parity-2).
3. **Overall → keep as a plan for now; do NOT implement yet.** No batch started.

**Resolved 2026-07-19 (round 2):**
4. **parity-1 → WIRE `buffer_size`** into the cpal stream, guarded by the device's supported range; it also sizes M9's scratch preallocation. Lands in Batch F with M8/M9. See §5 parity-1.
5. **Dialog fold → FIRST in Batch B**, as a pure green refactor, then the dialog bug fixes land in the folded code. See §6 step 3.

All forks now resolved. Nothing else awaits a user decision before implementation.

**Recommended first step when implementation is authorized:** Batch A (test-fake + H3/M2/M7/ux-13).
