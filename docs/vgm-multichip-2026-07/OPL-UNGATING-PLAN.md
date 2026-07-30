# Any-chip VGM: un-gate the OPL-only features

> **Status: COMPLETE (2026-07-30).** All 13 steps shipped on `vgm-multichip`,
> one commit each (`34e78e8`..`cdab9eb`), workspace green, wasm clean. Every
> feature below now works for any-chip VGMs: CLI play/render/split, per-chip
> mute/solo + pan panels, clickable waveform, documented instruction
> descriptions, Find Register + delay navigation. Delay wording unified.
>
> **Deviations from the plan as written:**
> - **Step 7 pan probe.** Pan is driven through the `DEV_DEF.set_panning` field
>   directly (populated on every core that has it), not the `RWF_CHN_PAN`
>   rw-funcs probe the plan floated — simpler, and both point at the same
>   function. The i16 convention is libvgm's `Panning.c` `±0x100` (centre 0),
>   confirmed against upstream.
> - **Step 7 default-core reality.** libvgm's *default* SN76489 core is the
>   Maxim core (which pans), not the `-mame` alternative — so `default_core_pans`
>   includes the SN76489, and the flag is keyed to what each chip's default
>   libvgm core actually does: SN76489 (Maxim), AY8910 (EMU2149), NES APU
>   (NSFPlay), YM2413 (EMU2413), plus the built-in Nuked-OPL3's stereo-ext.
> - **Test doubles.** Both `ToneStub`s (vgms-synth `testing.rs` and vgms-ui
>   `chip_output.rs`) had to learn the mute mask so the split's solo renders
>   actually differ.
> - **Two `style:` commits** carry rustfmt churn separately from the feature
>   steps.

## Context

On `vgm-multichip`, the model is already generic (OPL = a projection of `VgmFile`, not a kind) and libvgm gives every chip a core — but most *features* still route through `Editor::song()` / `require_song()` (vgms-ui/src/app.rs:3789) or the legacy closed OPL reader, so a Mega Drive rip gets no waveform interaction, no channel controls, no find, no split, and the CLI refuses it outright ("No OPL2 or OPL3 data detected."). This plan generalises: CLI play/render/split, per-chip channel mute/solo + pan, waveform interaction, documented instruction descriptions, Find Register (per-chip), delay navigation — and unifies "delay"/"wait" wording.

**User decisions (fixed):**
1. **"Delay" everywhere** user-facing; internal identifiers (`VgmCommand::Wait` etc.) not mass-renamed.
2. **Chip docs, common chips first:** SN76489/T6W28, YM2612, YM2413, AY8910, GB DMG, NES APU, YM2151, YM2203, YM2608, YM2610, HuC6280, SegaPCM. Others keep the generic one-liner fallback.
3. **Channel panel:** every chip gets mute/solo. Pan controls **hidden** (not greyed) when the active core/output can't pan (RetroWave, Nuked-CQM, ymfm — pan-capable: nuked-opl3 stereo-ext + libvgm's SN76489/Maxim, EMU2413, EMU2149, NES APU cores, the only ones implementing `RWF_CHN_PAN`). Chip switching via `theme::tabs::strip` (like Editor/Pack tabs, app.rs:481-493), **always shown** even for one chip.

**Key facts (verified):** libvgm `set_mute_mask` is bound (vgms-cores-libvgm/src/ffi.rs:309) but never called; ~all 53 cores support it. `RWF_CHN_PAN = 0x92` (`EmuStructs.h:73`), probe-able via the rw-funcs pattern at chip.rs:473. `LibVgmChip::reset()` restarts the device, so masks are **lost on every seek** — must be reapplied. OPL VGMs never reach the chip-rows branch (`shows_chip_rows()` false when a projection exists), so OPL parity holds by construction; pinned by projection.rs:388+ and projection_corpus.rs.

## Steps (each independently committable, green tests)

Dependencies: 1, 2 independent early wins. 3/4/5 = vgms-core infra. 6←4, 7←6, 8←6, 9←4+6+7+8, 10←3, 11←5, 12←3+5, 13←2+4+6.

### 1. Terminology: "wait" → "delay" in user-facing strings
- `vgms-core/src/vgm/stream.rs:811,812,838`: `"wait {n}"` → `"delay {n}"`, `"YM2612 DAC, delay {n}"`, `"override delay …"`. Update pins at `vgm/file.rs:1381,1667` (+ comment :1309). Existing "delay" strings (help.rs, app.rs:3561, analysis.rs:134-136) already comply.
- `docs_src/Home.wiki` + regenerate `docs/readme.html`. Grep string literals for stray `"wait "`.
- Tests: one `describe` pin per wait shape (0x61/0x62/0x63/0x7n/0x8n/0x64).

### 2. CLI `play`/`render` accept any-chip VGMs
- `vgm-studio/src/lib.rs`: `enum LoadedSong { Opl(Song), Vgm(Box<VgmFile>) }` + `read_any_song_from_path()`: VGM bytes → `vgms_core::vgm::file::read`; `file.to_song()` yields projection → `Opl` (byte-identical to today), else `Vgm`. `audio_source()`, `total_ms()`, `pretty_string()` helpers.
- `cli/play.rs`: `NativeAudio` already dispatches `AudioSource::Vgm`; `--retrowave` + non-OPL → clean error. Use `playability()` to warn about coreless chips.
- `cli/render.rs`: Vgm arm → `render_vgm_wav_cancellable` (vgms-synth/src/wav.rs:279), resampling from config.
- Tests: in-memory SN76489 VGM → `Vgm`; OPL fixture → `Opl` parity vs old reader; corpus render smoke (non-silent for playable files).

### 3. vgms-core: wait prefix sum + timeline queries
- `VgmStream`: new `wait_prefix: Vec<u64>` built in `parse()` (reindex re-parses). `samples_before(i)`, O(1) `total_samples()`/`samples_from(i)`, `index_at_pct(pct)`, `seek_index_for_samples(t)` — copy `partition_point` boundary semantics from `song.rs:478-529`.
- `VgmFile`: `ms_offset_at(index)`, `stream_total_ms()` (stream-derived, not the lying header), `index_and_ms_offset_at_pct(pct)`.
- `VgmEngine::seek_to_ms` (vgm_engine.rs:337) drops its O(n) walk.
- Tests: prefix vs old summation property test; pct↔index boundary round-trips; edit-then-requery.

### 4. vgms-core: per-chip channel table
- New `vgms-core/src/vgm/channels.rs` (MIT-clean, datasheet facts): `ChannelInfo { name, short }`, `channels_of(kind, variant) -> &'static [ChannelInfo]`. Canonical order = the mute-mask bit contract (bit i = entry i); document it.
- Documented names for the 12 chips (SN76489: Tone 1-3 + Noise; YM2612: FM 1-6 + DAC; YM2413: FM 1-9 + 5 rhythm; YM2608/2610: FM + ADPCM-A/B + SSG; NES: +FDS on variant; …); the rest get "Ch 1..N" with counts cross-checked against libvgm mute-mask widths (counts only — no tables copied into MIT code).
- Tests: every ChipKind non-empty, ≤32 channels, 12 documented chips pinned.

### 5. vgms-core: chip-docs registry + generic changed-bit analyzer
- New `vgms-core/src/chip_docs/` (one module per chip, datasheet-cited): `RegisterDoc { name, fields: &[BitField] }`, `register_doc(chip, port, addr)`, `documented_registers(chip) -> &[(port, addr, name)]`, `address_width(chip)`.
- `ChipAnalyzer`: replay cursor over `BTreeMap<Cell, u16>` (promote `Cell` from chip_state.rs:43); `row(stream, index) -> Option<Cow<str>>` — documented chip → bit-diff text (lift the diff-formatting helper from `analysis.rs:126+`), else `None` (caller falls back to `stream.describe`). SN76489: per-instance latch decode (its `documented_registers` is empty → find dialog offers "any write").
- OPL: `register_doc` **delegates to regdata.rs** — no string duplication, no OPL behaviour change; equality test pins delegation.
- Tests: representative pins (YM2612 0x28 key, DMG NR52, NES $4015, SN76489 latch); same-value write → "(no changes)" parity with OPL wording; OPL delegation equality.

### 6. vgms-synth: ChipCore mute/pan API + VgmEngine + CoreInfo flag + WAV mix
- `ChipCore` (chip.rs:24), default impls (wasm + providers keep compiling): `set_channel_mutes(u32)`, `set_channel_pans(&[i16])`, `supports_pan() -> bool { false }`.
- New `ChipMuting`/`ChipPanning` (Vec of `{ kind, instance, muted: u32 / pans: Vec<i16> }`).
- `VgmEngine::set_muting/set_panning`: store as fields, apply to matching voices (`Voice::accepts`), **reapply after every `rewind()`/seek reset** (the mask_replay analogue — the reset-loss bug the tests must catch).
- `CoreInfo` gains `channel_pan: bool` (false everywhere initially); `CoreRegistry::pan_capable(kind)` reads the resolved choice's flag.
- `render_vgm_wav_cancellable` takes `VgmRenderMix { boost, muting, panning }` (old boost-only signature = thin wrapper).
- Tests via `RecordingChip` (grows recorded mutes): mask hits the right instance only; **seek persistence**; all-muted WAV render.

### 7. vgms-cores-libvgm: implement mute + pan
- `LibVgmChip` stores `mute_mask`/`pans`; `set_channel_mutes` calls `(*dev_def).set_mute_mask` (same unsafe shape as `set_option_bits`, chip.rs:799-807); `reset()`/`start()` reapply both.
- Pan: probe `RWF_CHN_PAN (0x92) | RWF_WRITE, DEVRW_ALL` via the `Writers::fetch` pattern → `DevFuncPanAll`; `supports_pan()` = probe found one. Verify the i16 pan range against upstream `emu/Panning.h` / vgmtest.c (±0x80?) before fixing the convention.
- Per-chip `mute_bits()` remap canonical→core order (identity default). **OPN caveat:** YM2203/2608/2610 SSG channels live on the linked EMU2149 child — split the mask between parent and child dev (store the child's dev_def/data at link time).
- `channel_pan: true` on exactly: SN76489 Maxim row, YM2413 EMU row, AY8910 EMU row, NES APU rows; vgms-cores-nuked sets it on `opl3.nuked` only (not CQM); ymfm/gpl/retrowave stay false.
- Tests (native): mute changes rendered output; mute survives reset; pan probe succeeds for each `channel_pan: true` row (catches upstream drift); YM2203 SSG mute reaches the child.

### 8. Audio service plumbing
- vgms-audio-native: `Command::SetChipMuting/SetChipPanning`; generic engine arm forwards (lib.rs:414-424 keeps OPL arm untouched — the two systems stay parallel).
- `AudioService` (vgms-ui/src/platform.rs:167): `set_chip_muting/set_chip_panning` default no-ops (RetroWave, tests, web stub).
- `services/audio.rs`: store + re-send on stream rebuild, same pattern as existing muting/panning (:149-166).
- Tests: Vgm arm forwards / Opl arm no-ops; reload re-sends.

### 9. Chip panels UI: tabs, generic mute/solo/pan, keyboard routing
- `chip_panels.rs`: `ChipControls::Generic { kind, instance, panel: GenericChannelPanel }`; `for_vgm` builds one entry per chip **instance** (dual → "SN76489", "SN76489 #2"); selector becomes `theme::tabs::strip`, **always drawn** (also single-chip and OPL/DRO docs).
- New `widgets/chip_channels.rs`: `GenericChannelPanel { audible: u32, pans: Vec<u8>, custom }` reusing `bevel::toggle` + `pan_knob`, mute/solo semantics copied from channels.rs; wrap rows of 9 for 16+ channel chips.
- Pan visibility per tab: OPL → `output_renders_samples() && resolved OPL core has channel_pan` (CQM/RetroWave now **hide**, was greyed — intended change; `ChannelPanel` switches grey→omit); generic → `registry().pan_capable(kind)`.
- `Action::ToggleChannel` (app.rs:2029) routes to the **selected tab**; new `Action::ChipMutingChanged/ChipPanningChanged`; `ensure_audio` (:4212) pushes chip muting/panning too.
- `dialogs/help.rs`: number-key rows reworded for "selected chip" (user rule: same change).
- Tests: per-instance entries for dual-chip file; selected-tab key routing (kittest: number key on SN76489 tab toggles SN76489, not OPL); pan absent for pan-false chip; re-baseline snapshots (`UPDATE_SNAPSHOTS=1`).

### 10. Waveform interaction for any-chip VGMs
- Buckets already render generically (tasks.rs:288-308) but `widgets/waveform.rs:82` discards them when `Editor::song()` is None.
- New `TimeSource<'a> { Song(&Song), Vgm(&VgmFile) }` in editor.rs with `total_ms`/`ms_offset_at`/`index_and_ms_offset_at_pct`; `Editor::timeline()`. `waveform::show` takes `Option<TimeSource>`.
- Rewire: app.rs:538 (show), :1483 (selection indicator), :1535 (end snap), :4234 (loop overlay gate `has_song()` → `timeline().is_some()`); position-panel length for VGMs uses `stream_total_ms` so cursor and wave agree. Click→seek already reaches `VgmEngine::seek_to_row`.
- Tests: kittest — non-OPL VGM draws buckets, click selects snapped row; `TimeSource` boundary agreement; OPL pixels unchanged (snapshots).

### 11. Editor rows from the chip-docs registry
- `vgms-ui/src/analysis.rs`: `AnalysisCache` grows a `ChipAnalyzer` beside the OPL one, same revision resets.
- `editor.rs:941-956` chip-rows branch: description = `analyzer.row(..)` else `stream.describe(index)`; hover = rendered `register_doc` block (format like `song.instruction_description`).
- Tests: YM2612 key-on row documented + hover non-empty; Pokey keeps the one-liner; OPL VGM rows unchanged (fixture pin).

### 12. Find Register + delay navigation, generic
- `vgms-core`: `VgmFindTarget { AnyDelay, Write { kind, instance: Option<u8>, addr: Option<u16> } }`; `VgmStream::find_next(start, target, backwards)` mirroring `Song::find_next_instruction` semantics.
- `Action::FindRegister { query: FindQuery, backwards }` where `FindQuery { Dro(String), Vgm(VgmFindTarget) }`.
- `FindRegDialog::for_vgm`: chip dropdown from `header.chips()` (expanded per instance for duals), register dropdown from `documented_registers(kind)` + free hex masked to `address_width(kind)` + delay token; "any write" for latch chips. DRO path untouched.
- Gates: `delay_navigate` (app.rs:3552) + `find_register` (:3590) → `require_document()` and dispatch song/vgm; ArrowLeft/Right on VGM uses `AnyDelay`. Reword :3793 to "This needs an OPL song." for the remaining genuinely-OPL features.
- Tests: stream find_next (delay/write/instance/backwards/none); kittest two-chip dialog find; ArrowRight on VGM; DRO find regression.

### 13. Generic per-channel split + CLI split + gates (closes CLI workstream)
- `vgms-synth/src/split.rs`: `split_vgm_cancellable(file, VgmSplitOptions, on_skip, on_progress, keep_going)` — for each voiced chip instance × channel, solo via `ChipMuting`, render WAV (Step 6 mix), track peak; below epsilon → skip + report. Pre-filter: one stream walk of written `ChipTarget`s skips never-written chips without rendering. Names: `{name}.{chip-slug}{#2}.{NN}-{short}.wav`. WAV-only for generic VGMs (per-chip write-gated song output out of scope); OPL keeps its key-on path + DRO/VGM capture untouched.
- `tasks.rs:67`: `TaskRequest::Split { source: SplitTaskSource }` with Opl/Vgm arms; gates `can_split_channels` (app.rs:4364), `Action::OpenSplit` (:1605), `begin_split` (:4003) → `has_song() || (vgm && renderable)`; SplitDialog hides format/percussion options for generic VGMs.
- `cli/split.rs`: `read_any_song_from_path`; Vgm arm runs the generic split with skip reporting.
- Tests: synthetic SN76489 (tone on ch1) → one WAV + 3 skips; cancellation emits nothing; kittest split on non-OPL VGM; OPL split regression; CLI corpus smoke.

## Verification

- Per step: `cargo test` workspace; `cargo check --target wasm32-unknown-unknown -p vgms-core -p vgms-synth`.
- OPL no-regression: projection parity tests (vgms-core/src/vgm/projection.rs:388+, vgm-studio/tests/projection_corpus.rs) stay green untouched; steps that touch OPL-adjacent code (1, 2, 9, 13) call out their pins above.
- Corpus runs after 2, 7, 13 (`vgm-studio/tests/*corpus*.rs`, env-var corpus dirs; memory: VGMSTUDIO_REF_CONFIG must be absolute).
- Snapshots re-baselined after 1, 9, 13 via `UPDATE_SNAPSHOTS=1`, diffs eyeballed.
- End-to-end: load a Mega Drive VGM in the GUI — chip tabs with working mute/solo, waveform click-seek, documented YM2612 rows, Find Register scoped to its chips, ArrowRight jumps delays; `vgmstudio play/render/split` on the same file.
