# HANDOVER — DRO Trimmer review remediation (implementation)

**For:** a fresh Claude Code session picking up implementation of the review fixes.
**From:** the review + planning session, 2026-07-19.
**Repo:** `I:\Code\Python\dro-trimmer` · **branch `rust`** (main/master is the Python original — do not touch Python except as a parity oracle).
**Status:** review + plan complete and delivered; **implementation not yet started.** All user decisions are made — you are cleared to begin at Batch 0 → A, following the workflow rules in §3.

> Everything you need is in this doc. Deeper detail (only if required) is in the **same folder** (`docs/code-review-2026-07/`): `remediation-plan.md` (the full per-issue plan — READ THIS SECOND), `review-report.md`, and `findings-{core,vgmrip,synth,uishell,uiwidget,native,ux,parity}.md`. A durable summary also lives in the maintainer's Claude project memory (`code-review-2026-07.md`).

---

## 1 · Environment & commands (do this before any cargo call)

Rust/LLVM are Scoop-installed at **User** scope; a long-running agent process does **not** inherit them. Prepend this to **every** PowerShell tool call that runs cargo/rustc/clippy:

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

- rustc/cargo 1.97.0 (msvc), clippy, rustfmt; MSVC `link.exe` auto-discovered (no vcvars needed). `wasm32-unknown-unknown` + clang/wasm-ld present.
- Do **not** use the 8.3 short path `C:\Users\LDAWG~1\...` with `Set-Location` (PS filesystem provider rejects it — use `C:\Users\L Dawg\...`).
- Build: `cargo build` · Test all: `cargo test` · UI only: `cargo test -p dro-ui` · Lint: `cargo clippy --workspace --all-targets` (**currently zero warnings — keep it that way; workspace lints deny `unsafe_code`, warn `clippy::all`**).
- **Snapshot baselines** (egui_kittest, `crates/dro-ui/tests/snapshots/*.png`): machine-specific (DX12 WARP adapter). If a change touches a themed surface, regenerate on THIS machine:
  `$env:UPDATE_SNAPSHOTS='1'; cargo test -p dro-ui; Remove-Item Env:\UPDATE_SNAPSHOTS`
  Only `theme_showcase` (2 baselines: clone-dark, ft2-classic) guards the whole theme surface; the other ~6 snapshot tests use the default theme. CI (`.github/workflows/rust.yaml`, windows-latest) only verifies, never regenerates.

## 2 · Workspace map (7 crates + vendored chip)

```
crates/
  dro-core/     pure model+formats, WASM-CLEAN. song, undo, analysis, io/{dro,mod}, vgm/{io,data},
                convert, rip (VGMRips .txt generate/parse), config, regdata, util. NO egui/audio/native deps.
  dro-synth/    pull playback engine over the chip, WASM-CLEAN. engine (PlayerEngine, FrameClock,
                RecordingChip test mock), opl (OplChip trait), capture, wav, waveform, limiter, Muting/Panning.
  dro-audio-native/  cpal stream + rtrb SPSC command queue (NEVER wasm). runs PlayerEngine in the callback.
  dro-ui/       egui 0.35 app AS A LIBRARY. app.rs (DroApp), editor.rs (headless Editor), action.rs (Action enum),
                platform.rs (service traits), tasks.rs, selection.rs, menus.rs, alert.rs, analysis.rs (AnalysisCache),
                widgets/{table,channels,pan_knob,peak_meter,waveform,position_panel}, dialogs/{settings,vgm_metadata,
                dro_info,gd3_tag,track_edit,find_reg,goto,mod}, theme/{palette,bevel,style,mod}, test_support.rs,
                app_gui_tests.rs, test_song.rs, theme_showcase.rs.
  dro-trimmer/  NATIVE shell. bin/{drotrim(GUI), dro_player, dro_split, dro2to1}; services/{file,audio,task,rip,config};
                split.rs, rip_zip.rs, lib.rs ("Shared logic for the native binaries").
  dro-synth-worklet/  wasm worklet stub (doc-only). dro-web/  wasm shell stub (doc-only). Both are correct placeholders.
vendor/nuked-opl3/   THIRD-PARTY OPL3 chip (patched). NEVER edit or review. patch target in root Cargo.toml.
src/                 the Python original (MIT). Parity oracle ONLY — never modify.
```

**Architecture in one paragraph:** widgets *emit* `Action` enum values while the frame draws; `DroApp::update_impl` collects them into a `Vec<Action>` and processes them after drawing via `handle_action`. All platform work (files, audio, background tasks, rip export) is behind **polled, never-awaited** service traits in `platform.rs`/`tasks.rs`; native impls live in `dro-trimmer/src/services/`. The headless `Editor` owns song+undo+selection+analysis and a monotonic `revision: u64` that keys audio/waveform staleness. `snapshot()` deep-clones the song (`Arc<Song>`) so background work never aliases the editable copy.

## 3 · Workflow rules (from the user; non-negotiable)

- **One step at a time. Report what changed + how it diverged from plan, then ASK before starting the next batch.** Do not batch-land multiple steps silently.
- **Do NOT commit unless explicitly asked.** No pushing. If you must branch, branch from `rust`.
- **Write tests alongside each fix.** A fix without a test that would have caught the bug is incomplete.
- Idiomatic, simple Rust — not a transliteration. Match the surrounding comment density (this maintainer comments deliberately; read comments before "fixing" a documented tradeoff).
- The Python under `src/` stays in place (parity oracle) — never delete it.

## 4 · GLOBAL CONSTRAINTS every fix must preserve

1. **Byte-parity (golden tests are the guard):** VGM writer preserves the source header; VGZ gzip is byte-identical native vs wasm (relies on the `flate2` `rust_backend` pin + `zip = deflate-flate2` only — verify with `cargo tree -p dro-trimmer -i flate2 -e features` must show `rust_backend`, no `zlib*`); rip `.txt`/`.m3u` byte-match the VGMRips template; DRO→VGM rounding is fixture-locked (`tests/lsl3_score_up.*`). Touching `vgm/io.rs`, `rip.rs`, `convert.rs`, or gzip **must** keep golden tests green.
2. **Real-time audio:** the cpal callback (`dro-audio-native/src/lib.rs`) must stay **alloc-free and lock-free**; the engine renders byte-identically to `golden_opl.rs`/`c_parity.rs`. M8/M9 fixes must keep those green and add no lock/alloc.
3. **wasm-clean core:** `dro-core`/`dro-synth` must not gain native-only deps. Icon, rename, `buffer_size`, CLI-progress fixes live in native crates only.
4. **`nuked-opl3` dev tuning stays:** root `Cargo.toml` has `[profile.dev.package.nuked-opl3] overflow-checks=false, opt-level=3` — required (a debug build without opt-level renders the waveform ~realtime; without overflow-checks=false it panics on 20 legit registers). Do not remove.
5. **Snapshot regeneration** (§1) for any themed-surface change.

## 5 · LOCKED DECISIONS (all forks resolved — no more user input needed to start)

- **H2 = FULL unsaved-changes protection:** dirty watermark on `Editor` + intercept window-close + confirm-prompts on File>Open, opening a rip track, and Exit.
- **parity-2 = KEEP the hex Pos. column:** make Goto parse hex + Find already accepts `0x` + print Pos. in hex in find/status messages, **AND make the hex-ness explicit in the label** (e.g. header `"Pos (hex)"` and/or a `(hex)` hint on the Goto field) so "0064" vs typed "64" can't confuse.
- **parity-1 = WIRE `buffer_size`** into the cpal stream (see §7 Batch F for the guarded-range detail); it also sizes M9's scratch preallocation.
- **Dialog fold = FIRST in Batch B**, as a pure green refactor, THEN the dialog bug fixes land in the folded code.
- **Report-only items that are NOT bugs — do nothing except (optionally) add to a divergence doc:** DRO Info view-only for VGMs (safety-justified), the 6th "all register options" column → hover tooltip, Stop leaving the readout at the pause point.

## 6 · Batch order (each step independently green + tested; STOP and report after each)

- **Batch 0 — test infra:** fallible `FakeAudioService` (gate for A).
- **Batch A — audio/rip coupling:** H3 + M2 + M7 + ux-18 + ux-13.
- **Batch B — dialogs:** fold FIRST (uiwidget-4 + uiwidget-5), THEN H1 + M10 + ux-8 + uiwidget-10.
- **H2 — unsaved-changes protection** (its own step; largest).
- **Batch C — keyboard:** M3 + ux-14 + ux-12.
- **Batch D — file/format safety:** M1/ux-9 + M5/ux-2.
- **Batch E — config apply:** M4/ux-15 + ux-16.
- **Batch F — engine real-time:** M8 + M9 + parity-1 (buffer wiring).
- **Batch G — polish:** uishell-7 + ux-11 + ux-17 + ux-19 + uiwidget-11 + vgmrip-5 + doc fixes.
- **Batch H — perf:** uiwidget-3 + uiwidget-9 + native-5.
- **Batch I — parity:** parity-4 (trivial) + parity-2 + parity-5 + parity-3 + document parity-6/7.
- **Batch J — remaining folds:** rip field-table (vgmrip-2), word-wrappers (vgmrip-3/4), VGM double-walk (vgmrip-1), boost-stepper (uishell-6), dead-API prune, small folds, ThreadTaskService single-slot (native-3).

## 7 · Per-batch implementation notes (anchors + exact change shape)

> Line numbers are **as of commit 2cb3d0f** and WILL drift once you edit — anchor by function name; treat lines as hints. All `app.rs` refs are `crates/dro-ui/src/app.rs`.

### Batch 0 — fallible FakeAudioService  `crates/dro-ui/src/test_support.rs` (~line 120)
`FakeAudioService::load` currently returns `Ok(())` unconditionally — this is why H3/preview-error paths are untested. Add fields like `fail_next_load: bool` / `fail_next_play: bool` (or a small mode enum) with setters, and have `load`/`play` consume them to return `Err("…")`. Keep the existing infallible default so current tests are unaffected. This unblocks the Batch A tests.

### Batch A — audio/rip coupling (all in `app.rs` + one `services/audio.rs` fact)
Key fact: `NativeAudioService::load` (`services/audio.rs:73-82`) does `self.unload()` **first**, then `self.audio = Some(NativeAudio::new(...)?)` — so a **failed load leaves the service cleanly empty** (`audio:None`), and the editor's prior stream is already gone. `ensure_audio` (`app.rs:~1687`) short-circuits when `audio_revision == Some(editor.revision())`.
- **H3** `preview_track` (`app.rs:~1334`): move `self.audio_revision = None;` to the **top** (before the `load` call). On the `play`-failure branch also `self.audio.unload();` and leave `rip.preview = None`. Test (needs Batch 0): fail preview-load → editor `Play` reloads (no "No song is loaded" wedge); fail preview-play → editor `Play` reloads the EDITOR song, not the rip track.
- **M2** `select_tab` (`app.rs:~1241`): when leaving the Editor tab, `self.audio.unload(); self.audio_revision = None;` (mirror `open_folder`'s documented rule at `app.rs:~1229`). Test: switch tab mid-play → `!audio.is_playing()`.
- **ux-13** (same `select_tab`): entering Rip, call `self.close_song_dialogs()` (already closes find_reg/dro_info/gd3_tag/vgm_metadata) + close Goto, mirroring the menu gating (`menus.rs` disables those on the rip tab).
- **M7** `load_file` (`app.rs:~1126`): add at the top `self.stop_preview(); self.active_tab = AppTab::Editor;` (covers menu Open, drop, CLI initial load; idempotent with `open_track_in_editor` which already sets the tab). Test: on Rip tab, load a song → tab flips to Editor, no stranded ▶.
- **ux-18** (Low): make same-folder `refresh_files` (`dro-ui/src/rip.rs:~124`) preserve a playing preview by re-matching by file name instead of nulling `preview` — fold into the H1(b) "identity by name" change.

### Batch B — dialogs (FOLD FIRST, then fixes)
Dialog shape today: each dialog is a struct with `show(&mut self, ctx, palette, area: Rect, actions: &mut Vec<Action>) -> bool` (returns false when closed); `DRO Info` is the exception — it's a modal, signature has no `area`. `Dialogs` struct + `retain()` + `show_all` live in `dialogs/mod.rs`. Every modeless dialog repeats this boilerplate: `let mut open=true; let mut close=false; egui::Window::new(TITLE).open(&mut open).resizable(false).collapsible(false).constrain_to(area).show(ctx, |ui| { <grid body>; ui.add_space(8.0); ui.with_layout(right_to_left(Center), |ui| { spacing.x=10.0; if bevel::button(ui,palette,"Close").clicked(){close=true;} if bevel::button(ui,palette,"Save").clicked() && self.save(actions) {close=true;} }); }); open && !close`.
- **Fold (uiwidget-4):** extract a helper — e.g. `dialog_frame(ctx, title, area, |ui| body) -> FrameVerdict` + a `footer(ui, palette, buttons) -> Verdict` — so each dialog keeps only its grid body + `save()`. Do it as a pure refactor: all dialog tests + snapshots stay green (regenerate snapshots only if pixels move — they shouldn't). `save()` returning `bool` (false = stay open, error already queued) is the existing contract (see `settings.rs:save`).
- **Fold (uiwidget-5):** `gd3_tag.rs` and `track_edit.rs` share an identical `const LABELS: [&str; GD3_FIELD_COUNT]` (11 entries) + the field-grid loop (index 10 = Notes → multiline `.desired_rows(4)`, else singleline, both `.desired_width(250.0)`). Extract one `LABELS` + `gd3_fields(ui, palette, &mut [String; GD3_FIELD_COUNT])`; `track_edit` prepends its "File name" row.
- **H1** (High) `track_edit.rs` + `quick_edit_submitted` (`app.rs:~1416`): the dialog stores only `index: usize`; a rescan reorders the name-sorted `rip.tracks` while the modeless dialog is open → Save rewrites/renames the wrong file. Fix: store the **original file name** in `TrackEditDialog`; in `quick_edit_submitted` re-resolve the track by that name (alert+abort if gone). Also close `dialogs.track_edit` on any track-list refresh (add to `refresh_files` next to the `preview=None` reset, and to a rescan-close step; today `close_rip_dialogs` only runs on a *different* folder). Test: open quick-edit on track C, trigger a reordering refresh, Save → the file that changes is C's (or dialog closed).
- **M10** `track_edit`/`quick_edit_submitted`: validate the new name on Save — non-empty, ends `.vgm`/`.vgz`, no collision with another track; on failure alert + keep the dialog open (the folded `save()->bool` makes this one line). Prevents a typo'd extension silently dropping the track from the pack (`classify` in `dro-ui/src/rip.rs:~292` accepts only `.vgm`/`.vgz` as Song).
- **ux-8 + uiwidget-10** `vgm_metadata.rs` + `set_vgm_metadata` (`editor.rs:~236`): dialog captures `song_len` at open and validates against it (stale after deletes behind the modeless window); `set_vgm_metadata` clamps against live len with only a `log::warn` and silently drops an out-of-range loop point. Fix: re-validate at save against live length and surface the drop as an alert/status; recompute the read-only "Loop length (samples)" display from the currently-typed loop point (capture a samples-before handle at open).

### H2 — full unsaved-changes protection (own step)
Pattern to reuse (already in the code): `Alert::confirm(title, message, Action)` stores `confirm: Some(Box<Action>)`; `alert::show_front` (`alert.rs:~50`) on OK pops and pushes the carried `Action`. Existing examples: `Action::OpenRipFolder` → checks `rip_is_dirty()` → `Alert::confirm(..., Action::ConfirmOpenRipFolder)`; `ConfirmOpenRipFolder` → `files.pick_folder()`. Same for `CloseRip`/`ConfirmCloseRip`, `RipExportZip`/`ConfirmExportZip`.
- **Dirty watermark:** add `saved_revision: Option<u64>` to `Editor` (`editor.rs:~29`); `is_dirty()` = `has_song() && Some(revision) != saved_revision`. Set `saved_revision = Some(revision)` on load (`load`), convert (`convert_to_vgm`), and on a successful Song save (in `handle_save_outcome`'s `SavePurpose::Song` arm, `app.rs:~613`). Revision is monotonic (never reused) → equality compare means redo-back-to-saved reads clean.
- **Close intercept:** in `update_impl`, read `ctx.input(|i| i.viewport().close_requested())`; if `editor.is_dirty() || rip_is_dirty()` and not a confirmed quit, `ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose)` and queue `Alert::confirm("Discard unsaved changes?", …, Action::ConfirmExit)`. Add `Action::ConfirmExit` → set a `quitting: bool` field + `send_viewport_cmd(ViewportCommand::Close)`; guard the intercept with `!self.quitting`. (Works with the custom `eframe::App::ui` shell — no version-specific hook needed.)
- **Open/track-open guards:** add `pending_load: Option<PickedFile>`; where `poll_picked`→`load_file` (and `open_track_in_editor`) would replace a dirty editor song, stash the file + queue `Alert::confirm(…, Action::ConfirmDiscardAndLoad)`; the confirm handler takes `pending_load` and calls `load_file`. `Action::Exit` gets the same dirty check → `ConfirmExit`.
- Tests: dirty editor + `Exit` → confirm appears, app not closed; confirm → closes. Same for Open. Watch: the CLI initial load and non-dirty paths must not prompt.

### Batch C — keyboard  (`app.rs` `gather_key_input` ~705, `alert.rs`)
- **M3** (Confidence Med — spike the egui API first): `ctx.egui_wants_keyboard_input()` is true for ANY focused widget (`memory.focused().is_some()`), so one Tab press onto a chrome button disables every app shortcut and makes Space "click" the focused button (e.g. "Del." → deletes). The editor view has **no text inputs** (all text is in dialog windows), so replace that broad gate with an explicit "a text-bearing dialog is open" check (you already hold `self.dialogs`; the modal `dro_info` is already special-cased). Additionally consume Tab/Shift+Tab on the editor view so focus can't land on chrome. Add a test: Tab then Space still plays (doesn't delete).
- **ux-14/uishell-4:** in the plain-key block require `!modifiers.command && !modifiers.alt` (keep Shift for `SelectionMove`); add Shift+1..9 → channels 9..17 (high bank; `toggle_channel` already supports 0..18, keys stop at Num9).
- **ux-12:** in `alert::show_front`, Enter = OK for info alerts and (matching wx) confirm boxes; focus OK on open.

### Batch D — file/format safety  (`dro-trimmer/src/services/file.rs`, `app.rs`)
- **M1/ux-9** `rename_in_place` (`file.rs:~170`): `dest == from` is case-sensitive and `dest.exists()` matches the same file on NTFS → case-only renames ("01 intro"→"01 Intro") fail with "already exists". Fix: if `dest.exists()` but `dest` is the *same* file as `from` (case-insensitive filename compare in the same dir), rename via a temp name so NTFS updates the case. Also reorder `quick_edit_submitted` so the in-place byte rewrite (in the target format) happens **after** the rename succeeds (fixes the "surviving .vgm holds .vgz bytes" variant). Pure-fs → unit-test in a temp dir.
- **M5/ux-2:** Save As offers VGM filters for a DRO (`FILTERS`, `file.rs:13-18`); bytes are serialized in the song's own format before the dialog, and only vgz↔vgm *compression* is fixed up (`record_saved`, `editor.rs:~132`), so a DRO saved as `.vgm` is verbatim DRO bytes the app can't reopen. Fix: after the save dialog returns a name, if the chosen extension's format ≠ the song's format, alert ("Save As can't change format — use Convert to VGM") and abort. Polish: narrow `save_filters` (`file.rs:~184`) for a song to its own format.

### Batch E — config apply  (`app.rs` `apply_settings` ~1604, `dialogs/settings.rs`)
- **M4/ux-15:** `SettingsDialog` snapshots the whole `AppConfig` at open and `save()` rebuilds from it, so a boost changed via the transport while the modeless dialog is open gets reverted on Save. Fix handler-side (keep the dialog dumb): in `apply_settings`, `config.audio.boost = self.config.audio.boost;` before installing (preserve the live-changed field). `AppConfig` is `Copy`.
- **ux-16:** don't `position.set_frequency(new)` while a stream is live — keep the panel on `audio.output_rate()` (as `ensure_audio` already does) and adopt the configured rate on reload; otherwise the samples readout mixes new-rate length with old-rate live frames.

### Batch F — engine real-time  (`dro-synth/src/engine.rs`, `dro-audio-native/src/lib.rs`)
- **M8** (Confidence Med): playback writes go through the chip's timestamped `write_reg_buffered` (`engine.rs:~508`, drained one entry/sample), but `set_muting`/`set_panning` use immediate `write_reg` (`engine.rs:~315`, `~341-345`) — a write burst that outlives a callback lets a queued key-on drain AFTER a mute's key-off (stuck note), or a queued `0xD0` drain after Custom panpots (clobber). First write a `RecordingChip` (test mock in `engine.rs`) write-order test reproducing both, THEN pick the fix that keeps that + `golden_opl`/`c_parity` green: either route control writes through `write_reg_buffered`, or add a chip-queue flush-to-now before the immediate writes. Offline renders set muting before render → golden bytes unaffected either way.
- **M9:** the callback allocates its `scratch: Vec<i16>` on first run (`lib.rs:~247`) — pre-size it at stream build; keep the in-callback resize as a never-hit fallback.
- **parity-1 (WIRE buffer_size):** `build_stream` currently sets `config.buffer_size = cpal::BufferSize::Default` (`lib.rs:~94`). Set `Fixed(audio_config.buffer_size)` **guarded** by the device's `SupportedStreamConfigRange::buffer_size()` (`SupportedBufferSize::Range{min,max}`): clamp into range, and if the host rejects `Fixed` (WASAPI can) fall back to `Default` + `log::warn!`. Preallocate M9's scratch to `buffer_size * 2` i16. `AudioConfig::buffer_size` is `u32`, default 512, validated `!= 0` (`config.rs:20/35/171`); it's already threaded into `NativeAudio::new`. Buffer-size-agnostic engine → no golden-byte impact.

### Batches G–J — see `remediation-plan.md` §3/§4/§5 for the same anchor-level detail
Highlights: **ux-19** `scan_folder` (`file.rs:~150`) `return Err` on first unreadable file → change to skip-and-collect + warn (unreadable *songs* are already tolerated per-track). **uiwidget-3** cache `TrackEntry` on `RipTrack` at scan (currently recomputed per row per frame in `track_table`, `dro-ui/src/rip.rs:~576`; `TrackEntry::from_song` does full O(n) sample sums with no prefix cache). **parity-4** add `#[command(version)]` to `dro_player`/`dro_split` (trivial). **parity-3** icon: load `src/dt.ico` → RGBA `IconData` via `ViewportBuilder::with_icon` (`drotrim.rs:~35`) + embed exe icon (winres/embed-resource, native-only). **Dead-API prune** (grep-verified unused): `RipService::cancel`+`TaskService::cancel` (trait-level), `Selection::len`, `PlayerEngine::{sample_rate,muting,panning}`, `render_waveform_cancellable` export, `io::dro::sum_delay_ms`(+test), `util::to_timestr` pub.

## 8 · Code-fact cheat-sheet (so you don't re-look-up)

- **Action processing:** `handle_action` (`app.rs:~884`) is one big `match`; add new variants to `action.rs` `Action` enum (~19-137) + a handler arm. `AppTab::{Editor,Rip}` in `action.rs`.
- **DroApp fields** (`app.rs:~117`): `editor, files, audio, tasks, rip_service, config_store, config, status, alerts:VecDeque<Alert>, dialogs:Dialogs, rip:Option<RipState>, active_tab, pending_saves:VecDeque<SavePurpose>, waveform, peak_meter, position, channels, scroll_to, last_first_selected, audio_revision:Option<u64>, was_playing, pending_open`.
- **SavePurpose** (`app.rs:~103`): `Song, RipDoc, TrackRewrite, ImageOptimised, ExportZip`. Push one before EVERY `files.save`; `poll_saved` pops front (FIFO correlation). 6 save sites — keep the invariant if you add saves.
- **Editor** (`editor.rs`): `song:Option<Song>, path:Option<PathBuf>, undo:UndoController<Song>, selection:Selection, analysis:AnalysisCache, revision:u64`. Mutating methods bump `revision` + `analysis.invalidate()` (load/delete/undo/redo/convert; `update_header` bumps revision only — analysis is a pure fn of the instruction stream). `snapshot()` = `song.clone().map(Arc::new)`.
- **Services** (`platform.rs`): `FileService{pick_open,open_path,poll_picked,save,poll_saved,pick_folder,open_folder_path,poll_folder,rename,poll_renamed}`; `AudioService{load,unload,play,pause,seek_ms,seek_pos,rewind,set_muting,set_panning,set_boost,is_playing,is_finished,position,take_peaks,output_rate}`; `RipService{submit,poll,is_busy,cancel,optimize,poll_optimized,today}`; `TaskService{submit(debounce),cancel,poll,is_busy,shutdown}`. Native impls: `dro-trimmer/src/services/`.
- **Native save contract** (`services/file.rs`): `save()` produces EXACTLY ONE `SaveOutcome` per call on every branch (InPlace/Dialog-picked → `write_outcome`; Dialog-dismissed → `Cancelled`); `poll_saved` pops oldest-first. Don't break this — the app correlates outcomes to `pending_saves` by order.
- **eframe launch** (`bin/drotrim.rs`): `run_native("drotrim", NativeOptions{viewport}, |cc| { theme::install; …; DroApp::new(files,audio,tasks,rip,config_store, None) })`. `DroApp` impls `eframe::App::ui` (not `update`) + `on_exit` (unload audio + tasks.shutdown). ViewportBuilder has title/inner_size(800×600)/maximized/drag_and_drop.
- **Config** (`dro-core/src/config.rs`): `AppConfig{audio:AudioConfig, ui:UiConfig}` is `Copy`. `AudioConfig{frequency,bit_depth,buffer_size,chip_write_delay,boost}`; `UiConfig{theme:ThemeChoice, tail_length, maximize_window, dro_info_edit_enabled}`. Load precedence: exe-dir then cwd (cwd only if not shadowed). 7 ini keys, configparser-compatible. `ThemeChoice::{CloneDark, Ft2Classic}`, `ThemeChoice::ALL` drives the showcase test.
- **Table hex Pos.** (`widgets/table.rs:~63`): `format!("{index:04X}")` — this is parity-2's column. Goto parses decimal (`app.rs` `goto_submitted:~1567`), Find via `FindTarget` FromStr (`dro-core song/instruction.rs`, accepts `0x..`).
- **Engine control vs playback writes:** `set_muting`/`set_panning` = immediate `write_reg`; playback = `write_reg_buffered`. `RecordingChip` mock + `with_chip` enable write-order tests without audio.
- **Test hooks that already exist** (`app_gui_tests.rs`, mounted as a child module of `app` so it reads private fields; `test_support.rs` fakes; `test_song.rs` fixtures). Pinned today: play reuse/reload-skip, delete+undo, goto, modal gating, muting keys, full panning suite, boost persist, Ctrl+S in-place save, rip open/tabs/dirty/save-docs/preview/quick-edit/optimize/export/confirm/failure, 5 snapshots. NOT pinned (your new tests): both H3 failure paths, tab-switch-stops-playback, Tab-focus, modifier+plain keys, vgz-flip re-save, rename flow, quick-edit-after-reorder.

## 9 · First actions for the new session

1. Read `remediation-plan.md` (full per-issue detail) — it's the authority; this handover is the orientation layer.
2. Confirm the toolchain works: run `cargo clippy --workspace --all-targets` with the §1 prelude → expect **zero warnings** (baseline). If it errors on toolchain, re-check §1.
3. Start **Batch 0** (fallible `FakeAudioService`), then **Batch A**. Implement, add tests, run `cargo test` (+ `-p dro-ui`), report what changed and any divergence from plan, and **STOP for confirmation before Batch B** (per §3).
4. Do NOT commit unless the user asks.

## 10 · Definition of done (per step)

- `cargo build` + `cargo test` (and `cargo test -p dro-ui`) green; `cargo clippy --workspace --all-targets` still zero warnings.
- A new test exists that fails without the fix and passes with it.
- Snapshots regenerated (§1) only if a themed surface changed, and the diff is intentional.
- Byte-parity/real-time/wasm constraints (§4) unviolated (golden + c_parity green).
- You reported the change + divergences and paused for confirmation.
