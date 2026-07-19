# Findings: dro-ui shell reviewer (returned complete)

### [uishell-1] preview_track error paths leave audio_revision pointing at a stream the audio service no longer holds
- Severity: High | Category: Bug | Confidence: High
- Location: crates/dro-ui/src/app.rs:1349-1366 (preview_track), app.rs:1687-1698 (ensure_audio), crates/dro-trimmer/src/services/audio.rs:73-82 (load unloads first)
- Evidence: preview_track returns early on failure without touching audio_revision (cleared only on full success). NativeAudioService::load begins with self.unload() before NativeAudio::new (which can fail: NoDevice, UnsupportedFormat, cpal errors). Two broken sequences from "editor song played, then user on rip tab" (audio_revision == Some(editor.revision())):
  1. Preview LOAD fails (device unplugged): editor's stream already dropped by unload(), revision still matches → every subsequent editor Play takes ensure_audio's short-circuit, skips reload, play() errors "No song is loaded into the audio output." — broken until an edit/settings change clears the revision.
  2. Preview load succeeds, PLAY fails: service holds the rip track's song, rip.preview never set (stop_preview no-op), stale revision → editor's next Play skips reload and plays the rip track under the editor's table/waveform.
  Neither path pinned (FakeAudioService::load infallible, test_support.rs:120-125).
- Suggestion: invalidate audio_revision unconditionally at the top of preview_track (before the load attempt). ensure_audio's own failure path is safe by contrast (revision only increments; stale Some(old) can never match again).

### [uishell-2] Switching to the rip tab leaves editor playback running with every transport control hidden
- Severity: Medium | Category: Bug | Confidence: High
- Location: crates/dro-ui/src/app.rs:1241-1258 (select_tab), app.rs:1229-1231 (contradicting comment in open_folder), app.rs:843-875 (playback_tick), app.rs:721-731 (rip-tab key gate)
- Evidence: open_folder documents "The editor's audio must not keep playing under the rip view." + audio.unload(). But select_tab (Editor → Rip with rip already open) only calls stop_preview() — a no-op when the playing stream is the editor song. On the rip tab the transport panel is hidden, playback_tick's rip branch only handles rip.preview, gather_key_input handles only Save/Help there — Space cannot stop it. Song plays on with no visible control until switching back or previewing (which replaces the stream). Code contradicts its own documented rule one function away.
- Suggestion: pause (or unload + clear audio_revision) in select_tab when leaving the editor tab, mirroring open_folder.

### [uishell-3] One Tab press invisibly focuses a widget and silently disables every keyboard shortcut
- Severity: Medium | Category: UX | Confidence: Medium (mechanism verified in egui source; real-world frequency uncertain)
- Location: crates/dro-ui/src/app.rs:715; egui-0.35.0 src/context.rs:2884-2886, src/memory/mod.rs:580-581, src/context.rs:1399-1406
- Evidence: ctx.egui_wants_keyboard_input() is a plain egui Context method: memory.focused().is_some() — true for ANY focused widget. Mouse-clicking a button does NOT grant egui focus (only TextEdit takes focus on click) so clicking transport buttons is harmless — but Tab/Shift+Tab move focus by default, and the bevel widgets never consult focus state (no `focus` anywhere in theme/), so the focused widget shows nothing. After one Tab press the entire gather_key_input body (Ctrl+S/Z/O, Space, Delete, arrows, digits) is skipped until a click elsewhere surrenders focus, while Space/Enter "click" the invisible focused widget (FAKE_PRIMARY_CLICKED). Comment at app.rs:712-714 describes text-field protection; the gate is much broader. Nothing in app_gui_tests.rs pins tabbed focus.
- Suggestion: keep plain-key block behind the broad gate, but let modifier shortcuts fall through unless ctx.text_edit_focused(); or paint focus in the bevel widgets so the state is visible.

### [uishell-4] Plain-key handlers fire with any modifier held
- Severity: Low | Category: UX | Confidence: High
- Location: crates/dro-ui/src/app.rs:767-807
- Evidence: the ctx.input block tests only key_pressed(...) → Ctrl+Space toggles playback, Ctrl+Delete deletes selection, Ctrl+1..9 toggle channels, Alt+Left (common back gesture) triggers PreviousDelay. No internal conflict today (consume_shortcut block runs first; no defined shortcuts use Space/Delete/arrows/digits) — over-eager handling rather than misrouting; unpinned.
- Suggestion: require no modifiers on the plain-key branch (Shift excepted where SelectionMove reads it).

### [uishell-5] Dead trait surface: RipService::cancel and TaskService::cancel have no production callers (Selection::len has none at all)
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-ui/src/platform.rs:257, crates/dro-ui/src/tasks.rs:59, crates/dro-ui/src/selection.rs:47-50; call sites crates/dro-trimmer/src/services/rip.rs:111, services/task.rs:133,177,297
- Evidence: only calls to either cancel are the native impls calling their OWN method from submit/shutdown (cancel-on-resubmit is an implementation detail) + one impl-level test. dro-ui never calls them; test fakes implement purely to satisfy the trait (TaskLog.cancelled recorded, never asserted). TaskService::shutdown IS used (app.rs:1742 on_exit) — keep. Selection::len: zero callers anywhere incl. tests. (Editor::is_empty is clippy-mandated since Editor::len is used.)
- Suggestion: drop cancel from both traits (keep as inherent on native impls) and delete Selection::len — or comment as reserved for web shells.

### [uishell-6] update_impl: extract the boost stepper (and optionally the waveform row) as widget modules
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-ui/src/app.rs:358-447 (boost stepper ~90 lines), app.rs:254-286 (waveform/rewind/peak-meter row ~33 lines)
- Evidence: update_impl ~337 lines. Boost stepper block fully self-contained (inputs: config.audio.boost + palette; output: Action::SetBoost) — exactly the shape of widgets/pan_knob.rs/peak_meter.rs, and the densest widget code in the file (nested RTL quirks, scoped visuals, DragValue formatter/parser). Extracting it (+ waveform row) leaves ~230 lines of pure panel scaffolding. Rest is fine inline (seams painting, dialog-area, menu/tab strip coupled to panel responses); whole-controls-panel extraction not worth churn.
- Suggestion: widgets/boost_stepper.rs with show(ui, palette, boost, &mut actions); stop there.

### [uishell-7] A failed description save still clears the rip dirty flag when the playlist save lands
- Severity: Low | Category: Bug | Confidence: High
- Location: crates/dro-ui/src/app.rs:630-647 (RipDoc arm), app.rs:663-665 (Failed arm)
- Evidence: save_rip_docs queues .txt then .m3u. If the .txt write fails, its outcome takes the generic Failed arm (alert only; purpose already popped at app.rs:570 so purpose-specific handling skipped); the succeeding .m3u outcome finds no more RipDoc purposes pending → sets rip.dirty = false and reports "Saved {stem}.txt and {stem}.m3u." even though the .txt failed — later Close Rip / Open Folder skips the unsaved-changes prompt.
- Suggestion: route Failed/Cancelled through the purpose too; only clear dirty/claim both files when the batch's last RipDoc outcome arrives with no earlier failure.

### [uishell-8] Dropping a folder whose name contains a dot is silently ignored
- Severity: Low | Category: UX | Confidence: High
- Location: crates/dro-ui/src/app.rs:694-702; crates/dro-trimmer/src/services/file.rs:42-46
- Evidence: native drop path forwards a path only `if is_song || path.extension().is_none()`; a rip folder named "Game v1.3" yields extension()==Some("3") → drop does nothing, no alert. Guard is redundant for folders anyway: NativeFileService::read already routes path.is_dir() into the folder scan, and a junk file forwarded surfaces the normal "Failed to load file" alert.
- Suggestion: forward any pathful drop; let the service's is_dir routing decide (dro-ui deliberately has no fs access; the check belongs there).

#### Checked and fine:
- pending_saves FIFO: all six files.save sites immediately preceded by exactly one push_back (app.rs:580/625/1195/1288/1435/1491); native save produces exactly one outcome per call incl. Cancelled; order pinned by test. vgz-flip re-save cannot loop (record_saved sets song.name first; second outcome compares equal names → false).
- Action enum: every variant has ≥1 emitter and non-empty handler (all 40+ grepped).
- update_header bumping revision without analysis.invalidate() correct (RowAnalysis pure function of instruction stream; opl_type/ms_length never enter it). set_gd3_tag/set_vgm_metadata not bumping revision sound (dro-synth reads neither).
- editor.rs touched() helper: tail repeats at exactly five sites but each pairs with different selection fixup; helper saves ~8 lines, absorbs none of the variation — not worth it.
- platform.rs: output_rate/is_finished/take_peaks/position/seek_ms/rename/poll_renamed all have production callers; only the two cancels + Selection::len dead. One-impl indirection is the documented web-port seam.
- table/selection: all multi-select semantics in selection.rs (headless, tested); table.rs maps modifiers + paints. Single scroll consumer. Three two-line select_only+scroll_to sites below helper threshold.
- stop_preview paths clear audio_revision whenever a preview existed; select_tab and open_track_in_editor both call it.
- DeleteInstructions tolerates out-of-range selection (retain < len) — RewindToStart-selects-row-0-empty-song cannot panic.
- Alert queue: one modal at a time; Esc/backdrop cancels confirm without running action; modal key-blocking pinned by test.
- apply_settings: audio_changed/waveform_changed computed against old config before assignment (AppConfig is Copy); lazy reload documented.
- Coverage: pinned — play reuse/reload-skip, delete+undo, goto, modal gating, muting keys, full panning suite, boost persist, Ctrl+S in-place + status, rip open/tabs/dirty/save-docs/preview/quick-edit/optimize/export/confirm/failure, five snapshots. Unpinned — both uishell-1 failure paths, uishell-2 tab-switch playback, tabbed focus, modifier+plain keys, vgz-flip re-save, PlayTail/RewindToStart/waveform-click handlers.
