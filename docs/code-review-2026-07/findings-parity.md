# Findings: Python feature-parity auditor (returned complete)

### [parity-1] `audio.buffer_size` is parsed, validated and shown in Settings, but does nothing
- Severity: Medium | Confidence: High
- Python: src/drotrimmer/dro_config.py:67-71 reads buffer_size (shipped drotrim.ini documents it); dro_player.py:393, 262, 337-344 use it as the OPL render/PyAudio chunk size (user-tunable latency/stutter).
- Rust: crates/dro-core/src/config.rs:203-204 parses, 171-172 validates; crates/dro-ui/src/dialogs/settings.rs:71-77 offers an editable "Buffer size" field with no caveat — but crates/dro-audio-native/src/lib.rs:94 opens the stream with cpal::BufferSize::Default and no other consumer exists (workspace grep). Zero effect.
- Note: buffer-size-agnostic engine is deliberate, but the ini comments + Settings dialog present a dead knob. Wire into StreamConfig or drop/grey the field.

### [parity-2] Table row numbers are now hexadecimal while Goto, Find and status messages stay decimal
- Severity: Medium | Confidence: High
- Python: dtgui/tables.py:81-82 renders Pos. zero-filled decimal, matching decimal Goto spinner and decimal statuses (wxapp.py:508-521, 546-548) — decimal end to end.
- Rust: crates/dro-ui/src/widgets/table.rs:63 renders `{index:04X}` (hex), but app.rs:1567-1583 goto_submitted parses decimal only, and app.rs:1596-1601 reports find results in decimal. Reading "0064" and typing 64 into Goto lands on a different row; no base hint anywhere.
- Note: hex Pos. arrived with the FT2 theme (likely intentional aesthetic) — but the decimal-in/hex-out mismatch is a degradation Python did not have.

### [parity-3] No application/window icon
- Severity: Low | Confidence: High
- Python: dtgui/containers.py:69-83 sets window icon from exe resource or dt.ico; setup.py:121 embeds dt.ico.
- Rust: drotrim.rs:35-39 viewport has title/size/maximize but no .with_icon(); no build.rs, no winres/embed-resource/IconData anywhere under crates/. src/dt.ico exists in-repo, unused. Default window/taskbar/exe icon shown.

### [parity-4] `--version` flag lost from dro_player and dro_split
- Severity: Low | Confidence: High
- Python: dro_player.py:769-770 and dro_split.py:151-152 optparse with version=g_app_version (drotrim too).
- Rust: dro_player.rs:28-31 and dro_split.rs:14-17 declare #[command(name, about)] without version → no -V/--version; only drotrim.rs:17 has #[command(version)]. (Python dro2to1 never had it — at parity.)

### [parity-5] CLI progress and skip feedback lost in dro_split and dro_player --render
- Severity: Low | Confidence: High
- Python: dro_split.py:100-142 printed "Skipping bank X, channel YY", live "MM:SS / MM:SS" while each channel rendered, "Finished rendering bank X, channel YY" (+ per-drum lines); dro_player.py:811-826 same live progress during --render.
- Rust: split.rs:66-67 silently continues over unused channels; split() renders everything before returning; dro_split.rs:53-64 prints nothing between header and final "Wrote ..." lines — multi-channel render of a long song is silent for its whole duration. dro_player.rs:69-95 prints only the final "Rendered ..." line (live playback DOES keep its progress line).

### [parity-6] DRO Info can no longer edit VGM songs (deliberate safety change, recorded)
- Severity: Low | Confidence: High
- Python: DROInfoDialog Edit/Save worked for any song incl. VGM (dialogs.py:154-258 no gate; wxapp.py:738-742 undoable).
- Rust: app.rs:929-937 `edit_allowed = dro_info_edit_enabled && !song.is_vgm()` → VGM DRO Info always view-only.
- Note: in-code comment justifies (VGM length derived from sample delays; chip re-type corrupts header clocks) — almost certainly deliberate, but not on the documented-divergence list.

### [parity-7] "Description (all register options)" column demoted to a hover tooltip (deliberate)
- Severity: Low | Confidence: High
- Python: tables.py:63-64, 95-97 — sixth always-visible column.
- Rust: table.rs:28, 83-94 — five columns; all-options text only as on_hover_text. Information survives but can't be scanned column-wise. Code comments show deliberate compression.

#### Verified present (spot list):
- All Python menu items + shortcuts (Open/Save/SaveAs/Exit/Undo/Redo(+Ctrl+Shift+Z)/Goto/Find/DROInfo/EditTag/EditVGMMeta/ConvertToVGM/Delete/Help/About); undo/redo disabled-when-empty, now with descriptions.
- Keyboard: Del+Backspace delete; Left/Right delay nav; Space toggle; text fields swallow shortcuts; modals block.
- Buttons: Delete/Play/Stop/Tail with Python's exact tail-label formatting.
- Load flow: auto-trim + box (DRO-only, non-undoable), mismatch box with v1/v2 advice, statuses, "Please open a DRO file first." gate, failed load keeps song.
- Dialogs: Goto (Enter submits, exact statuses); Find Register (tokens, BANK only DRO v1, hex range, exact statuses, empty=silent); DRO Info (fields, Edit→Save gate, statuses, undoable with Python's ms_length-revert bug fixed); GD3 (11 fields); VGM metadata (Save actually works now — Python's Save was bound to the wrong ID and never fired).
- Waveform: 1s debounce, progressive, peak auto-scale, click-to-seek, hover snap (+time tooltip), start indicator, cursor, dimmed pre-start.
- Position panel: pos/len ms + samples, 44.1k vs rendering-rate dropdown, round-half-up rescale (true frames — documented fix).
- Table: virtual rows, five columns w/ detailed analysis, no "?" phase, multi-select, delete reselects slid-in row.
- drotrim.ini: all seven Python keys, configparser-compatible semantics, identical defaults (pinned); chip_write_delay honored everywhere; maximize_window, tail_length, frequency/bit_depth honored.
- Shell: title "DRO Trimmer v{version}", 800×600, DnD (superset incl. folders), CLI file arg + --version, VGZ read/write, v1 char-vs-word quirk, v1 waveform-select hack, Convert statuses + undo wipe.
- CLI dro_player: -r/--render "{input}.wav" appended-extension, pretty_string header, live MM:SS progress incl. chip-write-delay.
- CLI dro2to1: default "{stem}_1.{ext}", refuses overwrite.
- CLI dro_split: -d capture with Python's codemap-overflow error text, -i percussion (names, .14. numbering, per-bank fix), unused channels skipped, naming, OPL2 low-bank only.
- Channel soloing in GUI as planned: per-bank melodic + percussion mute/solo, number keys 1-9, high bank hidden for OPL2.
