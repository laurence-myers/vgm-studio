# Findings: dro-core vgm/convert/rip reviewer (returned complete)

### [vgmrip-1] VGM read path walks and validates the command stream twice with duplicated mid-command checks
- Severity: Medium | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/vgm/io.rs:340-357 (`read_commands`) and crates/dro-core/src/vgm/data.rs:58-78 (`VgmData::build_offsets`)
- Evidence: `read_uncompressed` does `VgmData::new(read_commands(&bytes[data_offset..])?)?` (io.rs:270). `read_commands` walks: `let size = VgmData::command_size(bytes[offset])?;` + truncation check; then `VgmData::new` immediately re-walks identical bytes in `build_offsets` with the same opcode-size lookup and truncation check, slightly different error strings. Only logic unique to `read_commands`: truncating at `command::END` (+ missing-marker warning). `build_offsets` must exist independently (rebuilds offsets after `delete_many`/`insert_many`), but the file-read pass duplicates it.
- Suggestion: one walker serves both — e.g. a `VgmData` constructor that consumes up to an optional `0x66` and reports whether the marker was seen, or `read_commands` returns the offsets it already computed. Read path only; cannot affect written bytes.

### [vgmrip-2] Description header field list is hand-maintained twice (generate vs parse), echoed a third time in the UI form
- Severity: Medium | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/rip.rs:270-284 (generate), rip.rs:344-359 (parse); label echo at crates/dro-ui/src/rip.rs:432-471 (form)
- Evidence: `generate_description` hand-lists ten fields with printed labels/grouping (`push_field(&mut lines, "Game name:", ...)` ...) while `parse_description` hand-matches the same ten labels lower-cased (`"game name" => &mut meta.game_name, ...`, with `"music author" | "music authors"` alias). Adding/renaming a field means editing both in lockstep (+ display-only dro-ui labels). Round-trip test only catches drift for fields its fixture populates.
- Suggestion: one ordered table of (printed label, parse aliases, group break, accessor) driving both. Output bytes unchanged.

### [vgmrip-3] Two parallel greedy word-wrap implementations in rip.rs
- Severity: Medium | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/rip.rs:413-462 (`push_wrapped_block`) and rip.rs:466-500 (`wrap_value`)
- Evidence: both iterate `split_whitespace`, accumulate `Vec<char>` to width, flush, hard-split overlong words (`split_off(width)`). Identical line breaks on shared cases; genuine differences are parameterisation only (first-line vs continuation width + prefix + per-line byte-exact pass-through in `push_wrapped_block`; single width + always-normalise in `wrap_value`). (`break_title` rip.rs:554-578 is a genuinely different vgm_stat algorithm — not part of this.)
- Suggestion: one core wrapper `(first_width, continuation_width)`; `wrap_value` passes equal widths; pass-through/prefix stay at `push_wrapped_block`'s call site. Byte-locked output pinned by three golden tests, so unification is verifiable.

### [vgmrip-4] Right-aligned time-block row assembled identically in two places
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/rip.rs:514-525 (`push_track_rows` final row) and rip.rs:542-549 (`push_total_row`)
- Evidence: both do `format!("{...:>5} {...:>6}")` then pad-to-LINE_WIDTH-and-trim (align-to-column-47 rule the layout comment calls load-bearing) — encoded twice.
- Suggestion: shared `push_aligned_row(lines, head, block)`; identical strings out.

### [vgmrip-5] `write()` indexes a short header before `put_chip_clocks`' length guard can fire (panic vs error inconsistency)
- Severity: Low | Category: Bug | Confidence: High (path verified; reachability caveat)
- Location: crates/dro-core/src/vgm/io.rs:140-156 vs io.rs:441-446; entry point crates/dro-core/src/vgm/data.rs:295 (`VgmMeta::new`)
- Evidence: `write()` performs five unchecked `put_u32`s (0x04-0x23) and, after guarded `put_chip_clocks(...)?`, three unchecked stores at 0x7C/0x7E/0x7F. The only length check lives in `put_chip_clocks` (`len < MINIMUM_HEADER_SIZE => Err`), but a header < 0x24 bytes panics on the earlier `put_u32`s before that guard runs. Reachability today: reader enforces `data_offset >= MINIMUM_HEADER_SIZE`; sole workspace caller of public `VgmMeta::new` is convert.rs:95 with the 0x80-byte `synthesise_header()` — panic path unreachable in-workspace, exposed only to future/external callers.
- Suggestion: hoist one `MINIMUM_HEADER_SIZE` check to the top of `write()` (graceful `Error::File` everywhere) or document the `VgmMeta::new` precondition. No output-byte impact.

### [vgmrip-6] `preset_for` + `music_hardware_suggestion`: two public entry points for one lookup, one non-test caller between them
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/rip.rs:213-219, rip.rs:222-225; sole external caller crates/dro-ui/src/rip.rs:330
- Evidence: `music_hardware_suggestion` is `preset_for(opl).music_hardware` verbatim; its only non-test caller is dro-ui `prefilled()`. `preset_for`'s only non-test caller is `music_hardware_suggestion` (else test-only). UI otherwise uses `PRESETS` directly.
- Suggestion: collapse to one entry point. Cosmetic API surface only.

### [vgmrip-7] Reader honours a GD3 tag placed before the data, but header preservation + writer assume it is after
- Severity: Low | Category: Bug | Confidence: Low
- Location: crates/dro-core/src/vgm/io.rs:220-223, io.rs:269, io.rs:158-162
- Evidence: if a nonstandard file put its GD3 between header fields and `data_offset`, tag bytes get captured inside the verbatim `header` blob and preserved forever while `write()` appends a live copy at the end and repoints 0x14 — one-time file growth with stale tag bytes. Every real file puts GD3 after the data; may be purely theoretical.
- What would confirm: a real VGM with `gd3_offset < data_offset`. If none exist, fine as-is.

#### Checked and fine:
- Header layout: one `mod offset` table (io.rs:35-50) shared by reader/writer; implicit offsets agree and are spec-frozen; test raw hex offsets are deliberate independent spec checks.
- GD3 transcoding: UTF-16 encode only in `write_gd3_tag` (io.rs:418), decode only in `parse_gd3_tag` (io.rs:387-394); `fields()`/`from_fields()` pinned by round-trip test.
- Sample clocks: `SampleClock` (convert.rs:26-41, carry seeded 500, byte-locked, fixed 44100/1000) vs dro-synth `FrameClock` (engine.rs:46-79, general-rate, reset-for-seek) — intentionally separate; unifying couples a byte-locked converter to a playback utility for ~3 lines. `util::smp_to_ms` and dro-ui `rescale_to_44100` are further distinct documented rounding rules, not duplicates.
- dro-ui/src/rip.rs contains no wrapping/reflow logic (calls dro-core's generate/parse; own code is classification/prefill/validation/drawing).
- Error enum: two variants, both constructed widely; stringly payloads are the documented crate-wide design.
- GD3 12-field tolerance: one place (io.rs:396-405), one call site, own test.
- Deferred-gap rejection paths single-site, no dead branches beyond vgmrip-5; `data_offset > len` check (io.rs:263) is live.
- `ByteReader` bounds-checks every seek/take — truncated headers fail as `Error::File`, never panic.
- convert.rs: 0x61 chunking repeats opcode correctly (regression-tested); `write_command` mapping exhaustively tested; zero-wait corner unreachable (parsed DRO delays >= 1 ms).
- rip.rs formulas (vgm_play_count, format_track_time +22050 rounding, num_width, NO_LOOP dash, hour squeeze) match vgm_stat and are pinned; `break_title` cannot recurse infinitely.
- `dro_to_vgm` and `dro2_to_dro1` both have live non-test callers.
- tests/wasm_roundtrip.rs covers DRO/VGM/VGZ round trips, conversion oracle, 32-bit usize wrap on wasm32.
