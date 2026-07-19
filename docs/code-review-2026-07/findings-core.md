# Findings: dro-core song/undo/io/analysis/config reviewer (returned complete)

### [core-1] `io::dro::sum_delay_ms` is dead code, and its doc comment names two callers that do not use it
- Severity: Medium | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/io/dro.rs:245-254 (definition); only use is its own test at io/dro.rs:569-572
- Evidence: Doc says "Exposed because `dro2to1` and the v1 writer both need it, and because it makes the 'header length disagrees with the data' warning a subtraction." All three claims stale: v1 writer uses prefix sum (io/dro.rs:145 `song.total_delay_ms()`), `dro2_to_dro1` (convert.rs:118-179) never calls it, mismatch warning lives at dro-ui/src/editor.rs:105 as `song.ms_length != song.total_delay_ms()`. Workspace grep hits only io/dro.rs:250, 569, 571 (test-only).
- Suggestion: delete function + test (prefix-sum tests pin the same total), or fix the comment if a future caller is planned.

### [core-2] DRO v1 opcode values duplicated as bare literals in convert.rs because the named constants are `pub(super)`
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/song/dro_data.rs:12-20 (`v1_opcode`: SHORT_DELAY 0x00, LONG_DELAY 0x01, BANK_LOW 0x02, BANK_HIGH 0x03, ESCAPE 0x04) vs crates/dro-core/src/convert.rs:137, 146, 156, 161-162
- Evidence: `dro2_to_dro1` re-encodes the whole v1 opcode table (incl. escape threshold `reg < 0x05`) as magic numbers; cannot reuse decoder constants because `mod v1_opcode` is `pub(super)` (visible only inside `song`). Round-trip test contains drift risk, but format knowledge exists in two unlinked places.
- Suggestion: widen `v1_opcode` to `pub(crate)`, use named constants (ESCAPE doubles as threshold) in convert.rs.

### [core-3] The delete path sorts/dedups/bounds-filters the same selection at three layers
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/undo.rs:169-179 + 194; crates/dro-core/src/song.rs:571-578; crates/dro-core/src/song/splice.rs:26-29
- Evidence: Only production chain is `DeleteInstructions::apply` → `Song::delete_instructions` → `SongData::delete_many` → `byte_ranges_to_delete` (undo.rs:213 is sole caller). Constructor sort required (undo capture order); splice's copy required (`delete_many` is `pub`); the middle pass in `Song::delete_instructions` re-sanitises already-sorted/unique/bounds-retained input — three full passes per delete.
- Suggestion: make sorted/unique/in-range a documented precondition of `pub(crate) delete_instructions` and drop the middle pass — or consciously keep as defence-in-depth with a comment.

### [core-4] ~30 lines of variable-length-encoding glue duplicated between `DroDataV1` and `VgmData`
- Severity: Low | Category: Duplication | Confidence: High
- Location: crates/dro-core/src/song/dro_data.rs:121-130, 161-167, 169-187 vs crates/dro-core/src/vgm/data.rs:115-121, 153-159, 172-190
- Evidence: Parallel glue: `byte_offset` (same three-arm match, same panic shape), `raw_instruction` (bounds + offset..offset), `delete_many` (`byte_ranges_to_delete` + `splice_out` + rebuild map), `insert_many`. Only the index-map rebuild differs. Enum-over-trait dispatch is documented deliberate (song.rs:24-26); heavy algorithm already shared in splice.rs; this residue is the cost of that choice.
- Suggestion: only fold (small internal trait: `byte_offset` + `reindex`, provided methods) if a fourth encoding appears; defensible as-is.

### [core-5] `UndoController::len` / `is_empty` / `applied` have no callers outside dro-core's own tests
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/undo.rs:49-64
- Evidence: Grep: appear only in undo.rs tests (319-370, 411-412, 599). dro-ui editor consumes new/reset/execute/undo/redo/can_undo/can_redo/undo_description/redo_description only. Generic `<T>` itself is fine (lets the ported Python state-machine test use a `Log` target).
- Suggestion: accessors let the ported state-machine test pin buffer-vs-applied counts — legitimate to keep; else demote to `#[cfg(test)]`.

### [core-6] `RegisterUsage::percussion` is a `BTreeMap<u16, bool>` that only ever stores `true`
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/analysis.rs:239, 265, 283-286
- Evidence: Single writer `usage.percussion.insert(perc_key, true);` (265); reader `.get(&key).copied().unwrap_or(false)` (284). A map whose values are always true is a set.
- Suggestion: `BTreeSet<u16>` with insert/contains; removes the `unwrap_or(false)`.

### [core-7] `util::to_timestr` is public API with a single caller, in the same file
- Severity: Low | Category: Simplify | Confidence: High
- Location: crates/dro-core/src/util.rs:42-46; sole caller util.rs:39 (`ms_to_timestr`)
- Evidence: Precise grep across crates finds only definition + internal call. The one plausible external consumer, rip.rs `format_track_time`, documents NOT using this family (rip.rs:164).
- Suggestion: inline into `ms_to_timestr` or demote from `pub`.

### [core-8] Bank-tracking loop idiom repeated across whole-song analysis passes
- Severity: Low | Category: Duplication | Confidence: High (exists) / Medium (folding pays off)
- Location: crates/dro-core/src/analysis.rs:255-257, 310-312; cross-crate crates/dro-synth/src/capture.rs:188-200; persistent-state variants analysis.rs:134-136 + engine.rs:472-474 (cannot share); test oracle analysis.rs:389-391 deliberately independent
- Evidence: Each whole-song pass opens `let mut bank = Bank::Low;` + `if let Some(selected) = instruction.selected_bank() { bank = selected; }` in a data iter loop. Two dro-core loops + capture.rs share the local-variable shape; cursor/engine keep bank as struct state so an adapter can't serve them.
- Suggestion: `iter_with_bank()` adapter on SongData yielding `(Bank, DroInstruction)` would serve 2-3 sites; marginal — reasonable to leave.

#### Checked and fine:
- undo.rs commands: only two, genuinely different capture/restore (DeleteInstructions: bytes + ms_length + loop point; UpdateHeader: 2-tuple) — no collapsible boilerplate. Sentinel-free `applied` counter sound (verified vs ported state-machine test). VGM interplay correct: `revert` restores ms_length AFTER insert's rebuild clobbers it; loop point restored verbatim.
- analysis vs song/regdata: only shared logic is the two `regdata::register_kind`/`register_description` compositions; opposite bank precedence documented both sides (song.rs:647-649, analysis.rs:214-221), pinned by tests. Test `reference_rows` re-implementation is a documented independent oracle.
- io/dro.rs reader vs writer: v1/v2 genuinely different header layouts; MAGIC/VERSION/WRITE_CHAR_OPL/MAX_DRO_DATA_BYTES shared; v1 recompute-total vs v2 verbatim ms_length is a documented format discrepancy pinned by tests. No foldable copy-paste.
- Delay prefix: total_delay_ms O(1), ms_offset_at O(1), seek_index_for_ms / index_and_ms_offset_at_pct O(log n), verified vs verbatim Python-scan ports. No path recomputes what the prefix provides. Sample-domain queries (samples_before, total_delay_samples, loop_num_samples) are O(n) but all callers cold (VGM save, load cross-check, rip-entry construction, VgmMetadataDialog::new — dialog open, not per-frame). `samples_before` pub with only internal callers — noted, not a finding.
- config.rs: 9-field read/write symmetry real but table/macro wouldn't pay (per-field comments in to_ini_string; lookup/parse/parse_bool already factor mechanics). All-or-nothing fallback is documented Python parity. ThemeChoice round-trip tested; ALL consumed by showcase test. ConfigStore: two real impls (IniConfigStore, MemoryConfigStore); dro-web placeholder.
- util.rs: smp_to_ms + VGM_SAMPLE_RATE used by song prefix + dro-synth; ms_to_timestr by waveform hover + dro_player; condense_ranges by splice. Integer smp_to_ms verified vs Python float formula over wide range.
- splice.rs: three production callers (DroDataV1, DroDataV2, VgmData); delete/insert inverse proptested from all three. Keep.
- lib.rs re-exports: all consumed cross-crate except VgmData/VgmMeta/DelayKind which appear in re-exported public signatures (structural, not dead). Bank::register_offset live; pretty_string live; RegisterAnalyzer::analyze_all used by dro-ui tests as documented.
- dro_data.rs correctness: v1 truncated-tail strict-vs-truncating split matches rationale; v2 load-time codemap validation; >4 GiB guard; wasm32 checked_mul; ByteReader universal bounds checks — all present and tested.
- regdata.rs: kind-keyed tables can't drift (documented vs Python string-keyed risk); masks non-overlapping/non-zero by tests; 145-key parity test pins the port.
- error.rs: minimal two-variant enum, both used.
