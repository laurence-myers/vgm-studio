# Findings: dro-ui rip/dialogs/widgets/theme reviewer (returned complete)

### [uiwidget-1] Quick-edit dialog's stale track index can rewrite and rename the WRONG file after a rescan reorders the list
- Severity: High | Category: Bug | Confidence: High
- Location: crates/dro-ui/src/dialogs/track_edit.rs:28,102-104; crates/dro-ui/src/app.rs:1416-1447 (quick_edit_submitted), app.rs:1241-1258 (select_tab rescan), app.rs:558-565 + 648-654 (rename/save outcomes rescan), app.rs:1637-1642 (close_song_dialogs omits track_edit), crates/dro-ui/src/rip.rs:124-129 (refresh_files), crates/dro-ui/src/platform.rs:35-37 (folder sorted by name)
- Evidence: Dialog stores only `index: usize`. Submit does `rip.tracks.get(index)` with no identity check, then rewrites bytes in place at that track's path and renames it. PickedFolder is name-sorted, so a rename changes order on rescan. Rescans fire while the modeless dialog is open (select_tab(Rip) re-scans; rename/TrackRewrite/ImageOptimised outcomes rescan). Nothing closes the dialog on rescan: refresh_files resets preview=None but not dialogs.track_edit; select_tab never calls close_rip_dialogs; close_song_dialogs closes only find_reg/dro_info/gd3_tag/vgm_metadata. Concrete sequence: tracks [01 A, 02 B, 03 C]; (1) quick-edit "02 B" → rename "10 B" → Save; rename outcome triggers async rescan; (2) before it lands, open Edit… on "03 C" (index 2); (3) rescan delivers [01 A, 03 C, 10 B] while dialog open — index 2 now "10 B.vgm"; (4) Save: 10 B.vgm rewritten with 03 C's GD3 and renamed to 03 C.vgm (rename collides → alert, but wrong file's contents already rewritten). Tab-round-trip variant gives an arbitrarily wide window.
- Suggestion: close dialogs.track_edit wherever the track list refreshes (mirroring preview=None), or carry the opened file name and re-locate by name on submit, alerting if gone.

### [uiwidget-2] Settings Save clobbers boost changes made while the dialog is open (contradicts its own "preserved" comment)
- Severity: Medium | Category: Bug | Confidence: High
- Location: crates/dro-ui/src/dialogs/settings.rs:12-15,30,152-155; crates/dro-ui/src/app.rs:383-395,433-436, 1083-1092 (SetBoost), 1604-1631 (apply_settings)
- Evidence: settings.rs keeps `original: AppConfig` captured at open ("so fields the dialog does not expose (e.g. audio.boost) are preserved rather than silently reset") and save starts `let mut config = self.original;`. Dialog is modeless, transport boost stays interactive; SetBoost mutates self.config.audio.boost + persists. Later dialog Save rebuilds from the stale open-time original, persists + installs it — silently reverting boost set while dialog open (and audio config differs → revision invalidated → next play reloads at reverted level). Exactly the "silently reset" the comment set out to avoid.
- Suggestion: merge the dialog's edited fields into the app's CURRENT config at apply time (ApplySettings handler), not an open-time snapshot diff.

### [uiwidget-3] Rip track table recomputes TrackEntry (full instruction-stream sums) per track, every frame
- Severity: Medium | Category: Simplify (perf) | Confidence: High
- Location: crates/dro-ui/src/rip.rs:576 (TrackEntry::from_song in per-row draw); crates/dro-core/src/rip.rs:113-129; crates/dro-core/src/song.rs:414-426,435-438
- Evidence: track_table builds all rows every frame (explicit for-loop, .vscroll(false), not virtual) and calls TrackEntry::from_song per row → total_delay_samples() = samples_before(len()) — full data-iter fold with no prefix cache (unlike O(1) total_delay_ms) — plus loop_num_samples() two more passes. Multi-track pack of 100k+-row captures = millions of adds per repaint; rip view repaints continuously during preview playback. Data only changes on rescan; from_folder already parses every song.
- Suggestion: compute TrackEntry (or display strings) once per track at scan/refresh; cache on RipTrack.

### [uiwidget-4] Six modeless dialogs repeat the same window-frame + Close/Save-row + open/close plumbing
- Severity: Medium | Category: Duplication | Confidence: High
- Location: settings.rs:50-57,136-147; vgm_metadata.rs:53-60,97-108; gd3_tag.rs:47-54,79-93; track_edit.rs:55-62,95-111; goto.rs:29-36,46-57; find_reg.rs:50-57,80-99
- Evidence: every modeless dialog repeats verbatim: `let mut open = true; let mut close = false; egui::Window::new(…).open(&mut open).resizable(false).collapsible(false).constrain_to(area).show(ctx, |ui| { …; ui.add_space(8.0); right-to-left footer { spacing 10.0; Close → close=true; Save → save(actions) && close=true } }); open && !close`. Three differ only in title; footer same shape in all six (goto/find_reg substitute separator + Go/Find). ~20 lines × 6 of structural plumbing.
- Suggestion: shared helper (window frame + footer-row builder returning open/close verdict); each dialog keeps only grid body + save logic.

### [uiwidget-5] track_edit.rs duplicates gd3_tag.rs's LABELS table and field-grid loop wholesale
- Severity: Medium | Category: Duplication | Confidence: High
- Location: gd3_tag.rs:10-22,31-37,59-77; track_edit.rs:12-24,36-40,75-93
- Evidence: both declare identical `const LABELS: [&str; GD3_FIELD_COUNT]` and identical render loop (notes multiline .desired_rows(4) else singleline, both .desired_width(250.0)), same constructor seeding. track_edit's header says it "is the GD3 dialog plus a leading 'File name' field". A GD3 field/label change must be made twice.
- Suggestion: one LABELS const + shared `gd3_fields(ui, palette, &mut [String; GD3_FIELD_COUNT])`; track_edit prepends its file-name row.

### [uiwidget-6] Quick-edit rename accepts any file name; an extension typo silently drops the track from the pack
- Severity: Medium | Category: UX | Confidence: High
- Location: track_edit.rs:67-73,101-108 (no validation); app.rs:1416-1447; crates/dro-ui/src/rip.rs:292-303 (classify: only .vgm/.vgz Song, .txt Doc); crates/dro-core/src/io/mod.rs:38-46 (write_song succeeds for any non-.vgz name)
- Evidence: dialog emits QuickEditSubmitted with whatever's typed — no emptiness/extension/duplicate check. "01 A.vgm" → "01 A" (or "01 A.wgm") succeeds end-to-end; classify then returns Other → track vanishes from track list, .m3u, and export zip with no warning (file still in folder). Renaming to *.txt even makes it a description-file candidate (choose_description falls back to texts.first()). Every numeric dialog validates; the one field that renames a file on disk validates nothing.
- Suggestion: validate on Save: non-empty, .vgm/.vgz, no collision; alert and keep dialog open.

### [uiwidget-7] Find Register token list is hardcoded in the dialog AND in dro-core's FindTarget parser
- Severity: Low | Category: Duplication | Confidence: High
- Location: find_reg.rs:24-33; crates/dro-core/src/song/instruction.rs:205-212 (FromStr), 78-83 (DelayKind::token)
- Evidence: dialog builds ["DLYS","DLYL","DALL"] + conditional "BANK" + hex as owned strings; app.rs:1591 round-trips through target.parse::<FindTarget>() and silently drops parse failure. Spellings exist independently in dialog + FromStr (+ DelayKind::token). Renamed/added token in core leaves dialog stale or silently ignored.
- Suggestion: expose token list from dro-core, or store FindTarget values in the dialog and format for display.

### [uiwidget-8] Dual-OPL2 hard-L/R pan image constructed independently in two places in channels.rs
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-ui/src/widgets/channels.rs:72-76 and 165-170
- Evidence: default_pans_for DualOpl2 arm and panning()'s inline rebuild are identical ([PAN_LEFT;18], [9..].fill(PAN_RIGHT)). Divergence path set_opl_type(DualOpl2, None) unreachable (sole caller app.rs:1107 runs after DRO Info edit which requires a loaded song).
- Suggestion: one `fn dual_opl2_image() -> [u8;18]` (or reuse self.default_pans).

### [uiwidget-9] Screenshot preview clones the full PNG byte buffer every frame
- Severity: Low | Category: Simplify (perf) | Confidence: High
- Location: crates/dro-ui/src/rip.rs:676-682
- Evidence: `egui::Image::from_bytes(uri, image.bytes.clone())` per image per repaint; converts Vec<u8> into egui Bytes = fresh Arc<[u8]> + copy each frame, only for the loader to hit its by-URI cache. (URI cache-busting sound; copy pure waste.)
- Suggestion: hold screenshot bytes as Arc<[u8]>/Bytes converted once per scan.

### [uiwidget-10] VGM metadata dialog's derived "Loop length" readout goes stale while editing the loop point
- Severity: Low | Category: UX | Confidence: High
- Location: vgm_metadata.rs:20-21,34-37,74-81
- Evidence: loop_samples_display computed once in new(), shown disabled every frame. Typing a different loop start (or clearing) leaves the readout showing the OLD loop point's value until reopen — misrepresents the pending edit one row above, precisely while deciding what to save.
- Suggestion: recompute display from the currently-typed loop point (capture samples-before table at open), or label as pre-edit value.

### [uiwidget-11] Invalid-input alerts titled inconsistently across numeric dialogs; Theme hover overpromises
- Severity: Low | Category: UX | Confidence: Medium
- Location: settings.rs:163-167,179-183 ("Invalid settings"); vgm_metadata.rs:121-127,139-144 ("Error"); dro_info.rs:131-135 ("Error"); settings.rs:111 (hover)
- Evidence: same failure mode (unparseable numeric, dialog stays open) yields different titles/wordings. Theme row hovers "Takes effect immediately" but dropdown does nothing until Save (apply_palette only in apply_settings, app.rs:1610-1612) — suggests a live preview that never happens.
- Suggestion: standardise invalid-input wording; reword hover ("Applied on Save; no restart needed").

#### Checked and fine:
- dialogs/mod.rs lifecycle (retain + show_all) minimal, single-sited; area constraint + DRO Info modal exemption documented and consistent.
- channels.rs banks NOT two strip copies (pan_row/channel_toggle/percussion_toggle shared; residual asymmetry justified). Solo/mute precedence unit-tested (isolate, re-solo restores, solo moves, drums solo, pans survive solo).
- Auto-pan doesn't duplicate dro-synth pan law (auto_pan_image produces pan bytes only; constant-power law in engine.rs:175-183). OPL3 "Original" capture delegates to dro_core::initial_channel_pans.
- bevel.rs is the single source of rectangular bevels (paint_bevel: waveform.rs:66,108; peak_meter.rs:104; buttons via bevel::button/button_sized). pan_knob hand-paints circular arcs (rect helpers can't express), documented.
- All 34 palette roles consumed in non-test code (least-used data_hover styles row hover app.rs:474).
- rip.rs refresh_files reuses from_folder; edited-meta+dirty across refresh tested. Title/length/description/m3u delegated to dro-core (uiwidget-3 is caching, not correctness). Unreadable-track row fills all six columns.
- position_panel.rs two-entry rate picker mirrors documented Python; rescale_to_44100/ms_to_frames tested.
- pan_knob drag model (raw value in widget memory so centre detent doesn't trap drag), snap band, 270° sweep, readout unit-tested; reports as slider for accessibility/kittest.
- peak_meter attack/release/hold + zone splits tested; is_active keeps repaint alive through decay.
- find_reg BANK gating to actual DRO v1 documented improvement; empty-selection silent no-op documented both ends.
- settings.rs validates via AppConfig::validate before emitting; starting from open-time config right for common case (uiwidget-2 covers the concurrent-edit hole).
- waveform.rs denominates in measured total_delay_ms; min/max-bucket + tooltip divergences documented deliberate.
