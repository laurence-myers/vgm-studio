# Findings: UX behavior-bug hunter (returned complete; 19 findings)

### [ux-1] No unsaved-changes protection anywhere: Exit, window X, File > Open, opening a rip track all discard editor edits (Exit also discards a dirty rip) without a prompt
- Severity: High | Category: Bug | Confidence: High
- Editor has no dirty flag at all (editor.rs:29-43). Action::Exit → viewport Close unconditional (app.rs:892); no close_requested/CancelClose anywhere in crates/. load_file (app.rs:1126) replaces without check; open_track_in_editor calls it. Contrast OpenRipFolder/CloseRip which DO prompt via rip_is_dirty() — a check Exit/X never runs.
- Python parity: identical hole (wxapp.py:660-665 Destroy(); __load_file replaces) — NOT a regression, but the port added dirty prompting for rip metadata (cheap to retype) while song edits (the app's purpose) have zero protection, and the rip's own prompt is bypassed by Exit/X. No comment marks deliberate.
- Suggestion: saved-revision watermark in Editor; intercept close_requested(); route Exit/Open/track-open through the confirm-alert pattern the rip already uses.

### [ux-2] Save As across formats writes unconverted bytes under the new extension; the app then can't reopen its own file
- Severity: Medium | Category: Bug | Confidence: High
- Bytes serialised in the song's own format before the dialog (app.rs:1178); native save filters offer the full set incl. VGM (file.rs:184-194, FILTERS :13-18). Only post-save fixup is vgz↔vgm recompression (record_saved true only for is_vgm && vgz-flip, editor.rs:132-143). DRO saved as .vgm → verbatim DRO bytes; reopen dispatches by extension (io/mod.rs:21-31) → "Failed to load file". No warning at any point.
- Python parity: same trap — but undocumented in the port, and more inviting now (real VGM support + Convert to VGM one menu over).
- Suggestion: compare picked extension with the song's format on Save As — refuse/warn or offer conversion.

### [ux-3] Dropping a folder bypasses the dirty-rip confirm; folders with a dot in the name are silently ignored
- Severity: Medium | Category: Bug | Confidence: High
- handle_drops native path has no rip_is_dirty() check → dropped folder B replaces dirty project A instantly (File > Open Rip Folder prompts). Dotted name: Path::extension() for "Game v1.2" = Some("2") → gate drops before open_path (app.rs:694-702). open_folder different-folder branch replaces self.rip unconditionally (app.rs:1220-1226).
- Suggestion: route dropped paths through the same dirty-confirm; decide folder-vs-file by is_dir() in the service.

### [ux-4] Failed rip preview leaves audio_revision stale: editor Play wedges — or plays the rip track
- = uishell-1 / native-2 (three independent confirmations). Clear audio_revision first thing in preview_track.

### [ux-5] Switching to the Rip tab leaves the editor song playing with no way to stop it from that tab
- = uishell-2. Also: returning after song end → readout/cursor frozen at switch moment (position updates gated on editor tab; was_playing consumed).

### [ux-6] One Tab press focuses a widget and silently kills the whole keyboard interface; Space then activates whatever is focused (e.g. "Del.")
- = uishell-3, plus: arrows then move the focus ring instead of the selection; Space "clicks" the focused widget — if that's "Del.", Space DELETES the selection instead of playing. wx parity: wx's list was focusable with a native cue; here the table isn't focusable so Tab-focus only lands on chrome where every app key dies.

### [ux-7] File > Open on the Rip tab loads the song invisibly (tab doesn't switch) and can strand the preview button on ■
- Severity: Medium | Category: Bug | Confidence: High
- Menus always enable Open (menus.rs:45); load_file never sets active_tab (contrast open_track_in_editor app.rs:1396) and never clears rip.preview, but calls audio.unload() (:1141). With audio unloaded is_finished() is false (services/audio.rs:186-188) so the playback_tick cleanup never fires → row shows ■ indefinitely. Ctrl+O dead on rip tab (only SAVE/HELP consumed) while the menu item works.
- Suggestion: load_file should stop_preview() and switch to Editor tab (or disable Open on rip tab like Save).

### [ux-8] VGM metadata dialog validates the loop point against a stale length; the stored value is then silently discarded
- Severity: Low | Category: Bug | Confidence: High
- Dialog captures song_len at open (vgm_metadata.rs:41), validates against it (:119) — stale after deletes behind the modeless window; set_vgm_metadata clamps against live length with log::warn only (editor.rs:250-256). User saves loop 800 into a 500-row song: accepted, closed, silently no loop in the file.
- Suggestion: re-validate at save against live length; surface the clamp as alert/status.

### [ux-9] Case-only track renames always fail on Windows; failed rename after extension change leaves compression mismatched
- = native-1, plus: rewrite bytes are serialised per the NEW name unconditionally BEFORE the rename outcome is known (app.rs:1434-1447; format keyed off new name in retagged_bytes rip.rs:490-497) — vgm→vgz rename that fails (target exists) leaves the surviving .vgm holding gzipped bytes (reopens by content sniff here, but ships nonstandard).
- Suggestion: case-insensitive same-file → two-step rename via temp name; rewrite bytes in target format only after rename outcome known.

### [ux-10] Quick-edit dialog binds a track by index; rescan while open can retarget or no-op the Save
- = uiwidget-1 (which traces the wrong-file-rewrite consequence and rates High).

### [ux-11] "Building rip zip..." / "Optimising ..." status lines outlive failure
- Severity: Low | Category: Bug | Confidence: High
- Status set at submit (app.rs:1330, 1461); failures arm only alerts (app.rs:591-593, 599-601) — nothing rewrites self.status. After dismissing the failure alert the status still claims work in progress indefinitely.
- Suggestion: replace status in the same match arms that raise the failure alerts.

### [ux-12] Enter does not confirm alert boxes (wx MessageDialog's default-button Enter is gone)
- Severity: Low | Category: UX | Confidence: High
- alert::show_front reacts only to clicks and modal.should_close() (Esc/backdrop) (alert.rs:62-90); no Enter handling, no default-button focus. Keyboard-only confirm requires Tab onto OK. wx confirmed with Enter — keyboard regression.
- Suggestion: Enter = OK for info alerts (focus OK on open); decide deliberately for confirm boxes.

### [ux-13] Modeless song dialogs stay open and functional on the Rip tab, editing the hidden song the menus deliberately disable
- Severity: Low | Category: Bug | Confidence: High
- dialogs.show_all runs every frame regardless of tab (app.rs:528); select_tab closes nothing; only track_edit is rip-bound; dialog actions not tab-gated in handle_action. Find Next moves the hidden selection; GD3 Save mutates the hidden song — while the Edit menu equivalents are greyed out there "so they cannot edit an unseen song" (menus.rs:40-42).
- Suggestion: close or disable song-bound modeless dialogs on entering the Rip tab, mirroring menu gating.

### [ux-14] Digit mute shortcuts fire with Ctrl/Alt held; OPL3 high-bank channels 10-18 have no keyboard toggles
- = uishell-4 + the 10-18 gap (toggle_channel covers 0..18; keys stop at Num9). Python n/a (muting is new).
- Suggestion: require no-modifiers; add Shift+1-9 for the high bank.

### [ux-15] Settings Save silently reverts a boost changed while the dialog was open
- = uiwidget-2. Extra detail: the live stream keeps boost 3 until next reload while config/ini snap back to 1 — visible+audible disagreement.

### [ux-16] After a frequency change in Settings, the position panel mixes new-rate lengths with old-rate live frames until the next reload
- Severity: Low | Category: Bug | Confidence: Medium
- apply_settings immediately set_frequency(new)+set_length_ms while reload is lazy; live updates keep raw frames from the old-rate stream (app.rs:1619-1626; position_panel.rs:48-56, 65-68) → samples readout can exceed the total; ms/samples columns disagree until next Play.
- Suggestion: keep the panel on audio.output_rate() while a stream is loaded (as ensure_audio already does); adopt configured rate on reload.

### [ux-17] Dropped files with the wrong extension (and multi-file drops) are ignored with zero feedback
- Severity: Low | Category: UX (debatable) | Confidence: High
- len != 1 → return (documented "as in Python"); dotted non-song files vanish silently. Adjacent comment "A junk file surfaces the usual 'bad format' alert" only true for extensionless files. wx refused at drag time (visible cursor) and alerted on accepted junk — silent post-drop ignore is strictly less feedback than wx gave.
- Suggestion: status line for ignored drops (post-drop feedback is the only cue egui allows).

### [ux-18] Optimising a screenshot stops a running track preview as a rescan side effect
- Severity: Low | Category: Bug | Confidence: High
- ImageOptimised outcome → rescan_rip_folder → open_folder same-folder branch → stop_preview() before refresh_files (app.rs:648-654, 1491-1495, 1213-1219; refresh_files also nulls preview rip.rs:128). Music stops for no visible reason.
- Suggestion: keep preview across in-place refresh (re-match by file name), or skip stop_preview on same-folder path.

### [ux-19] One unreadable file aborts the whole rip-folder open
- Severity: Low | Category: UX | Confidence: High
- scan_folder returns Err for the whole scan on first failed fs::read (file.rs:150-157), while unparseable SONGS are tolerated per-track and shown inline as "unreadable" (rip.rs:28-34). One locked .png blocks the whole project.
- Suggestion: skip-and-report unreadable files like unparseable songs.

#### Checked and fine (UX hunter):
- Save As vgz↔vgm recompression: exactly one re-save, terminates (second outcome compares equal vgz-ness).
- Save-outcome FIFO routing matches the service's strict one-outcome-per-save order (pinned).
- Playback state machine: edit-while-playing pauses via after_edit; next Play reloads + resumes from selection; waveform click seeks live while playing, primes selection while paused; Stop leaves readout at pause point (documented deliberate); song-end snaps exactly.
- Mute/pan/boost while paused not lost: native service stores + flushes all three before play() — applied before first audible frame.
- Rip preview happy path: preview uses track's own panning + clears mutes (pinned), tab switches stop preview, editor Play after successful preview reloads, quick-editing a previewed track stops preview first, closing rip stops it.
- Modality: alerts + DRO Info true Modals; app keys blocked under both; queued load warnings in order (pinned); alert stacks above dialogs.
- Goto validated against live len at submit — safe across loads. Song-bound dialogs close on load and Convert.
- DRO Info: VGM view-only documented; Save keeps dialog open as in Python; header edits pause audio + refresh pan policy.
- Export zip vs Save Package Files generate from the same live meta; zip playlist renames .vgm→.vgz deliberately (pinned); dirty docs auto-save on export.
- Rename to a genuinely different existing name correctly fails + alerts.
- Zero-VGM folder opens empty; export blocked with clear error; unreadable tracks inline without buttons; double-click surfaces real parse error.
- Window title static "DRO Trimmer v{version}" = Python parity (though filename/dirty marker a natural add alongside ux-1).
- Waveform hover/click clamps; boost DragValue clamps + persists once; digit keys map to correct banks (pinned).
- Task/rip services: debounce supersede-by-generation, stale drops, repaint notifier race closed, busy indicators clear.
