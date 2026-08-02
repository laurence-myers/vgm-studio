# VGM Studio — full codebase review

**Date:** 2026-08-02 · **Tree:** `web-target` @ `df3d5cf`, clean · **Scope:** the whole workspace — 14 crates, ~97k lines of Rust, plus the web JS host, the vendored WASI shim, build tooling, CI and docs. Not a branch diff.

**Method.** A clippy baseline over `--workspace --all-targets` first, then two waves of review: twenty area reviewers each owning a slice of the tree, each one's findings handed to a second agent told to *refute* them, plus a completeness critic over the survivors that identified the coverage gaps the second wave then filled. 41 agents, ~6.7M tokens of reading. Findings the verifier could not sustain were dropped; where it narrowed a claim, the narrowing is recorded inline in the appendix. Every high-severity item below I also confirmed myself by reading the code.

**Result:** 233 findings survived verification — 13 high, 57 medium, 163 low.

---

## Verdict

This is a well-kept codebase. Clippy is **completely silent** across the workspace with `--all-targets` — no dead-code warnings, no idiom lints, nothing. The comments are unusually good: they explain *why*, they name the trap they guard, and repeatedly during this review an agent's "this looks wrong" died on contact with a comment three lines up that had already considered the objection. Licensing is enforced structurally rather than by policy. There is a real test culture — property tests, corpus tests, byte-parity oracles against a reference player, snapshot tests for themed UI.

So the findings are not lint-tier. They fall into three groups:

1. **A malformed-file crash class.** Three ways a downloaded `.vgm`/`.vgz` can hard-kill the app — a slice panic, an unbounded allocation, and an uncapped gunzip — plus a zip bomb and a stream-rate wedge. This is the group I did not expect to find, and it is the one I would fix first, because the threat model is real: users download rips from the internet and open them.
2. **Two independent "the safety net isn't armed" problems.** The parity scorecard passes green when the reference player fails to run at all, and CI never runs any of it anyway.
3. **Structural debt from three unfinished migrations.** DRO→VGM, clean-room cores→libvgm, and native→web each shipped their replacement without removing the predecessor. That's a defensible way to migrate, but the bill is now due: two document models, two state-restore stacks, two pack-zip builders, two core-registration lists, and a set of docs describing a Python project that was deleted months ago.

---

## 1. Correctness — fix these

### 1.1 A malformed data block panics the render thread
`crates/vgms-synth/src/decompress.rs:240`

`bits_out` comes straight from the file (`payload[5]`) and is only checked for zero. `width = ceil(bits_out/8)` can therefore be up to 32, while `value` is a `u32` — so `&value.to_le_bytes()[..width]` slices a 4-byte array with an end index of up to 32. Unconditional panic. The line above (`packed << bits_out.saturating_sub(bits_in)`) is a second trigger: a shift of up to 254 on a `u32`, which panics in debug and silently masks in release.

What makes this clearly an oversight rather than a judgement call: the `mask` computation eleven lines earlier *does* special-case `bits_out >= 32`. The wide case was considered for the mask and missed for the width.

A concrete file: a `0x67 0x66 0x40` block whose payload begins `00 08 00 00 00 28 08 00 00 00` + one data byte. This is reached from `vgm_engine::data_block` on ordinary playback and on offline render — i.e. **on the cpal audio callback natively, and inside the worklet on the web**. Neither has a `catch_unwind`.

*Fix:* reject `bits_out > 32` and `bits_in > 32` beside the existing zero check, next to the `an_unknown_scheme_is_refused` test.

### 1.2 Two unbounded allocations from attacker-controlled sizes
`crates/vgms-synth/src/decompress.rs:215` and `crates/vgms-core/src/vgm/io.rs:80` (duplicated at `vgm/file.rs:698`)

`Vec::with_capacity(uncompressed_size)` takes the block's declared size verbatim, with no relation to how much packed data the block carries — the loop breaks as soon as the bit reader runs dry, and the doc comment says so, which makes the capacity the only unbounded part. An eleven-byte payload can ask for 4 GiB. On wasm32 that exceeds the entire address space, so the web app aborts.

Separately, `.vgz` gunzip runs `read_to_end` with no ceiling and never consults the gzip trailer's ISIZE. Deflate reaches ~1000:1 on zeros. Through the `vgm::file` reader there is a further ~12× amplification, because `0x00` decodes as a one-byte command and each command costs 4 bytes in `offsets` plus 8 in `wait_prefix` — so a 1 MB `.vgz` becomes ~1 GB decompressed and ~12 GB of index. (The verifier established this amplification does *not* apply via `vgm::io`, which rejects the all-zero stream first; the missing cap is real on both paths.)

The codebase's own convention is already the fix: `Banks::read` caps exactly this pattern with `length.min(0x1_0000)`.

*Fix:* decompress through `Read::take(MAX_UNCOMPRESSED)` with a documented cap, in **one** shared helper — the two gunzip preambles are byte-identical today, so a cap added to one would silently miss the other.

Same class, lower severity: `vgms-pack-archive/src/lib.rs:60` reads every zip entry with `read_to_end` and never consults the declared size (the `zip` crate does not bound output, and CRC is verified only *after* the allocation); `vgms-synth/src/dac_stream.rs:251` stores an unvalidated `0x92` stream rate, so `0xFFFF_FFFF` plus a looping `0x93` costs ~97,000 emulated chip writes per output frame and never terminates.

### 1.3 Header pointer arithmetic overflows on wasm32 — where the codebase already knows better
`crates/vgms-core/src/vgm/header.rs:705` (and `:469`, `:586`, `:797`, `:829`, `:838`, plus `file.rs:783`)

Every relative header pointer is widened as `offset + relative as usize` from an untrusted `u32`. On 64-bit this cannot overflow and the later bounds check catches the nonsense value. On wasm32 `usize` is 32 bits, so a data-offset field of `FF FF FF FF` wraps — panicking in a debug build, and in release making `data_start` 0x33, which *passes* the bounds check. The same file then parses differently in the browser than it does natively.

`crates/vgms-core/src/io/dro.rs:186` guards exactly this hazard one module over, with a comment explaining the wasm32 reason and a `checked_mul`. The VGM header reader never got the same treatment.

### 1.4 An early-stopping loop is silently widened on every stream edit
`crates/vgms-core/src/vgm/file.rs:674`

`repatch_header` unconditionally rewrites the header's loop length as `stream.samples_from(index)` — the entire tail after the loop point. But a loop that deliberately stops early is a real thing this editor both reads and writes: `loop_end_index` (file.rs:303) documents it as "materialised so that saving does not silently widen it", and `set_loop_rows(start, Some(end))` creates one.

Every edit routed through `repatch_header` destroys it: `DeleteCommands`, `crop_to_region`, `delete_region`, `optimize`. Delete one unrelated command in the any-chip editor, save, and the loop now runs to the end of the file. The parallel `Song` path gets this right — `VgmMeta::loop_end` is slid through deletions and `io.rs`'s tests pin it — so two production readers disagree about a user-visible property. That is theme 3.1 in miniature.

Same root, second door: `audit::fix` (`vgm/audit.rs:157`) always recomputes the loop length as the full tail even when the findings it is fixing were only `TotalSamples` or `TrailingBytes`. `audit()` deliberately does *not* report a short loop as a problem — there is a test named `a_loop_that_deliberately_stops_early_is_not_a_finding` — so Edit ▸ Fix Header does strictly more than the dialog said it would.

*Fix:* capture `loop_end_index()` before the edit, slide it with the arithmetic the loop point already uses, and emit `samples_between(start, end)` when an end survives.

### 1.5 The primary audio backend never reports that the stream died
`crates/vgms-audio-native/src/lib.rs:561`

cpal's error callback is `|error| log::error!(…)` and nothing else. `SharedState` has no error slot and no stopped flag, so `NativeAudioService` cannot implement `AudioService::last_error` and falls through to the trait's `None` default.

Unplug the headphones mid-song and: the callback stops, `frames_rendered` freezes, `playing` stays `true`, and the transport shows a stationary cursor with no message. **Both sibling backends do this properly** — RetroWave keeps `error: Mutex<Option<String>>` *plus* a `stopped: AtomicBool` with a comment saying it exists "so the transport can leave the 'playing' state instead of showing a frozen cursor", and the web service records worklet faults. `app.rs:1654` already polls `last_error()` every tick and raises the alert. The whole path exists; only the primary backend is silent.

This is the same seam that produced a real shipped bug before (the defaulted `set_chip_muting`), which is an argument for making these trait methods required rather than defaulted.

### 1.6 Superseding a web audio node leaks it — still connected, still audible
`crates/vgms-web/src/services/audio.rs:170`

`load()` says "drop the old node so its handler stops updating our state" and then only does `inner.node = None`. Dropping the `web_sys` handle does neither thing:

- The node is still connected to `context.destination()`, and `worklet-processor.js:256` returns `true` unconditionally with no dispose command — so the old processor stays in the graph and keeps being called every quantum, holding its own multi-megabyte wasm instance with the previous song. Every song loaded in a session accumulates.
- `_on_message` is untouched, so the orphan keeps overwriting the state `load` just reset — and once `setup` replaces the closure, the orphan's port still points at the dropped one, so every state post throws "closure invoked after being dropped" forever.
- `pending` is not cleared (only `unload` clears it), so commands queued against the old song flush into the new one.
- The async `setup` carries no generation token, so two loads in flight race: the first to finish drains `pending` and starts sounding while the second silently becomes `inner.node` — every later pause then goes to the wrong node and **the audible one cannot be stopped**.

`unload()` does all of this correctly, so the asymmetry is plainly unintended. The native service already calls `self.unload()` at the top of `load` with the comment "two open output streams would play over each other".

*Fix:* call `unload()` at the top of `load()`, add a dispose command to the processor, and gate node installation on a generation counter.

### 1.7 Web task results survive the cancel that was supposed to drop them
`crates/vgms-web/src/services/task.rs:184`

The native `ThreadTaskService` tags every result with a generation and bumps it on cancel, precisely so a task that already finished has its result discarded — there is a test named `cancel_drops_an_already_queued_result`. The web service has no such filter: results go into one shared untagged vector and `poll` returns it wholesale; neither `cancel`, `terminate`, nor `submit`'s supersede touches it.

So: a VolumeScan finishes and queues its Peak between two frames; the user opens another song; `app.rs:2285` cancels VolumeScan with the comment *"its peak is the old song's, and landing late it would set this song's volume from it"* — and on the web that is exactly what happens. The same applies to a `Wav` result opening a save dialog for a song the user closed.

### 1.8 Song-bound state survives a song load
`crates/vgms-ui/src/app.rs:3943` and `:2281`

`close_song_dialogs` closes six dialogs and misses `find_loop` and `split_songs`, both of which hold a snapshot of the song they were opened on. `load_file` cancels `RenderWav`/`Split`/`VolumeScan` but not `TaskKind::LoopSearch`. So a Find Loop search started on song A keeps running over A's snapshot, and its candidates land in a dialog now sitting over song B; picking a row emits A's raw row indices, and Apply writes them into B's VGM header.

Honest narrowing from the verifier: the dialog is an `egui::Modal`, so the menu and keyboard shortcuts are blocked while it is up — the song can only be swapped underneath it by drag-and-drop. That makes it harder to hit than it first looks, but the guard is still missing and the load-time comment already states the rule this violates ("anything song-bound closes with the song").

Treat this as a symptom rather than a bug — see theme 3.4.

### 1.9 The parity harness can pass without measuring anything
`crates/vgms-app/tests/reference_parity.rs:635`

`every_cored_chip_matches_the_reference_within_its_band` discards reference failures with `let Ok(bytes) = … else { continue; };` twice, prints "nothing comparable" for a chip with no comparisons, and asserts only `failures.is_empty()`. **A run where nothing at all was compared reports PASS.** The trigger is documented, not hypothetical — this is the harness whose invocation is already known to skip every row silently when the reference config path is relative.

Two more staleness holes in the same harness (`parity/reference.rs:266`, `:290`): the reference player is staged only `if !to.exists()` into a work dir that persists across runs, so replacing the binary keeps running the old one while `describe()` reports the new one's size; and the render cache is keyed on input name/size/rate only, so changing the pinned `VGMPlay.ini` — which selects the reference's per-chip cores — serves WAVs rendered under the old configuration.

And underneath all of it: **CI never runs any of this.** `rust.yaml` runs `cargo test --workspace` with no `-- --ignored`, and all 17 tests in the parity and corpus suites are `#[ignore]`d. `crates/vgms-synth/tests/scratch_chip.rs` cannot pass at all — it builds cores from an ambient registry that its own test binary never installs, so every core comes back `None` and the render is silence.

*Fix:* make the scorecard fail on a reference error rather than `continue`, assert a minimum comparison count, and add a scheduled CI job that runs the ignored suites where a corpus is available.

### 1.10 Smaller correctness items
- `crates/vgms-cores-gpl/src/lle_opm.rs:153` — the YM2151-LLE stops asserting the `ym2164` variant pin after the first bus write, so a YM2164-flagged VGM is simulated as a plain YM2151 **in the crate whose entire purpose is being the accuracy oracle**. The Nuked wrapper has a test for this; the LLE one does not.
- `crates/vgms-core/src/vgm/stream.rs:339` — `wait_prefix` ignores the `0x64` wait-override while the playback engine honours it, so the timeline, cursor, seeks and the audit's total-samples check all disagree with what actually plays.
- `crates/vgms-core/src/chip_docs/opl.rs:253` — the Find dialog offers OPL3-only registers for YM3812/YM3526/Y8950, where they can never match; the invariant test only iterates the YMF262, leaving three of the family unguarded.
- `crates/vgms-app/src/services/file.rs:313` — case-only rename detection uses `eq_ignore_ascii_case`, so `01 étude.vgz` → `01 Étude.vgz` fails the check, hits `dest.exists()` (true on NTFS, which folds Unicode-wide) and errors as a clobber. Reachable from pack mode with European titles.
- `crates/vgms-pack-archive/src/lib.rs:121` — the case-only rename branch skips the collision check, so in a zip legitimately holding both `Song.VGM` and `song.vgm`, renaming one silently destroys the other. The doc promises "fails rather than overwrites".
- `crates/vgms-ui/src/widgets/chip_channels.rs:30` — `pan_to_i16` maps `0x00` to exactly hard-left but `0xFF` to 2/256 short of hard-right, while the readout says "R100".
- `web/task_worker.js:20` — an `init()` rejection throws out of the async handler as an unhandled rejection, so `postMessage("done")` never runs and that task kind reports busy for the rest of the session.
- `crates/vgms-ui/src/tasks.rs:456` — the loop search clones, ranks and emits the *entire* accumulated candidate list per candidate found: O(N² log N) work into an unbounded channel, of which the dialog displays only the last. Its sibling streaming task (the waveform render) throttles properly.

---

## 2. Two things that are quietly not protecting you

Worth separating from the bug list, because both are load-bearing safety machinery that reports success while doing nothing:

1. **The parity scorecard** (§1.9) — passes green with zero comparisons, and CI never invokes it regardless.
2. **The vendored WASI shim** (`web/wasi-shim/`) — imported by exactly one file, and every failure mode is deliberately non-fatal: a trap or missing hook becomes `ToolOutcome::Failed` and the pipeline moves on. The one e2e spec that exercises it asserts only that the download is a zip containing two `.vgz` names, which is equally true when every tool silently failed. Related: the shim's `debug` singleton **defaults to enabled** — `enable(undefined)` means *on* — and the only thing preventing per-syscall `console.log` in production is one prose comment in `pack_worker.js` saying `debug: false` "must be said out loud".

---

## 3. The five structural themes

### 3.1 The DRO→VGM migration is half-finished, and it is the root of most duplication
Two document models are both in production: `Song` (read by `vgm::io`) and `VgmFile` (read by `vgm::file`). Both implement container parsing, GD3 handling, loop resolution and byte-exact writing.

`projection.rs`'s own docs frame the parity tests as the gate for a switch-over — *"nothing switches over to the projection until this holds"* — and `projection_corpus.rs` pins the two readers "row for row, total for total, and byte for byte on the way out". The gate exists and passes. It is also `#[ignore]`d and needs a local corpus, so the honest instruction is: run it once, then throw the switch.

Everything downstream shows up separately in this review, and each looks like its own ticket until you line them up:

| Symptom | Where |
|---|---|
| Loop-widening bug (§1.4) — the two paths disagree | `vgm/file.rs:674` vs `vgm/io.rs` |
| Gunzip cap must be added twice (§1.2) | `vgm/io.rs:80`, `vgm/file.rs:698` |
| `opl_type_of` implemented twice, same precedence rule | `vgm/io.rs:261`, `vgm/projection.rs:44` |
| `loop_end_index` re-derives `resolve_loop_end` with a linear scan | `vgm/file.rs:303` vs `io.rs:313` |
| Two state-restore stacks (`StateFold` vs `ChipState`) | `state_patch.rs` vs `chip_state.rs` |
| Two `redundant_indices`, only one re-exported | `optimize.rs:149`, `chip_state.rs:261` |
| Four task-source enums of identical shape, six copies of the same match | `ui/tasks.rs:125`, `ui/app.rs` ×6 |
| The `dro` slot's documented DRO-only invariant, contradicted by its own load fallback | `ui/editor.rs:153` |
| Wait-chunking loop written independently four times | `convert.rs:69`, `split_songs.rs:301`, `file.rs:136`, `optimize.rs:408` |

**Recommendation:** one migration-completion programme, not eight refactors. Step one is small and provable: implement `vgm::io::read` as `file::read` + `to_song`, then delete `VgmData::read_from_stream`, `resolve_loop_point` and `resolve_loop_end`. §1.4 stops being *possible* rather than being fixed twice.

### 3.2 God-files

| File | Lines | Note |
|---|---|---|
| `vgms-ui/src/app_gui_tests.rs` | 7,399 | one test file; `act` is defined 6,000 lines after first use, under an unrelated section banner |
| `vgms-ui/src/app.rs` | 4,807 | eight separable responsibilities; every new feature lands here |
| `vgms-ui/src/pack.rs` | 3,845 | model + view + tests, and model code leaks past its own `-- view --` divider |
| `vgms-cores-libvgm/src/chip.rs` | 2,516 | pure spec tables and fold rules sit beside the unsafe FFI wrapper |
| `vgms-core/src/pack.rs` | 2,412 | module doc claims two concerns; the file holds six |

`app.rs` first, and its seams are already marked by the file's own section comments. Converting it to a directory module keeps private-field access working (submodules are descendants of `app`), so the split is mechanical — `app/pack_ops.rs` alone is ~1,180 lines with nothing to do with the editor.

### 3.3 Native and web have forked the same code four times
Line-for-line forks with hand-synchronised semantics and nothing pinning them:

- **`pack_zip`** (`vgms-app` vs `vgms-web`): both define `PackZipOutput`, `build_pack_zip`, `process_entry`, `gzip`, `has_extension`, `to_vgz_name` and the same test scaffolding, and have already drifted (`anyhow` vs `String`, a progress hook only on web). The web copy is the more general one and **already has the unifying seam** — a `SongOptimizer` trait. The native header's rationale ("the native-only crates live here") is half-stale: `zip` demonstrably builds for wasm32, so only `oxipng` justifies anything — and it justifies a hook, not a fork.
- **Core registration** (`vgms-app/src/lib.rs:33` vs `vgms-synth-worklet/src/player.rs:48`): same provider order, same three promotions, differing only by the native-only RetroWave line. A fourth promotion added in one place silently gives web users different default cores for the same file.
- **Case-only rename**, handled wrong in two different ways (§1.10), in two crates.
- **JS-error-to-string**, written twice in `vgms-web` with different fallbacks.

Also worth noting because it points the same way: both web workers do `const copy = bytes.slice(); postMessage(copy, [copy.buffer])` under a comment claiming zero-copy — but wasm-bindgen's glue already returned a standalone JS-heap copy, so this is a *second* full copy of a multi-megabyte payload, at exactly the moment the module doc says it wants to avoid one.

### 3.4 A missing dialog registry is causing a bug class, not a bug
The §1.8 bug, the ten-way duplicated `Cell<bool>` footer scaffold, `BulkTagDialog` hand-rolling chrome the shared scaffold provides, and the fifteen-slot three-way lockstep in `Dialogs` are one defect wearing four hats:

- `dialogs/mod.rs:219-299` — adding a dialog means touching the struct, the 15-arm `any_open()`, and the 15 `retain()` calls in `show_all()`, with nothing enforcing agreement. Forgetting `any_open()` is silent and dangerous: it is what suppresses editor keyboard shortcuts, so Space/Delete keep driving the editor underneath the new dialog.
- Ten modals each re-implement the same `Cell::new(false)` workaround (because `dialog_modal`'s body and footer closures cannot both borrow `self`), with a near-verbatim comment each — roughly 150 duplicated lines.

*Recommendation:* one macro listing the slots, generating the struct, `any_open()` and `show_all()`; and let the scaffold own footer plumbing. §1.8 becomes structurally impossible instead of fixed once.

### 3.5 Documentation and CI describe a project that no longer exists
The best effort-to-value ratio in this report — most of it is deletion.

- **`.github/workflows/check.yaml` runs on every push and fails on every push.** It does `pip install -r requirements.txt` against a file deleted in `5e9ece7`. Every push gets a red ✗ beside the green `rust.yaml`, which is how a team learns to ignore CI.
- **`build.yaml`** packages the app with `cd src; python setup.py`. `src/` now holds an `.ico` and an `.ini`. There is consequently **no working release workflow in the repo at all**.
- **`DEVELOPMENT.md`** — the primary onboarding doc — opens by saying the Python sources "stay put during the transition… Both suites run", then spends five sections telling the reader to install Python 3.13, pip-install deleted requirements, and run Black and mypy over removed directories. Its subcommand list names `convert` (not a subcommand) and omits `optimize` and `retrowave-probe` (which are). Its wasm-cleanliness command checks two crates where CI checks seven. And it never mentions the completed web target at all.
- **`licenses/README.md`** omits four crates, including `vgms-cores-libvgm` — whose own Cargo.toml records that libvgm ships **no licence grant at all**. That is precisely the fact this file exists to surface, in a project that treats licensing as load-bearing.
- **`TODO.md`** lines 180-252 describe the pre-libvgm world: "no implementations of the chips themselves, so today it renders silence", "the first chip is in: an SN76489" (clean-room, since deleted). About a third of the file misleads anyone planning work from it.
- Nine doc comments across `vgms-synth` and `vgms-core` — the *permissive, reusable* crates whose rustdoc outsiders are most likely to read — still attribute behaviour to the `dro_split`/`dro_player` binaries, one citing a flag spelling that no longer parses.

---

## 4. Dead code

Clippy finds none of this because it is all `pub` and therefore invisible to the lint. That is itself the finding: **`vgms-ui`'s entire widget tree is `pub` with no external consumer**, which is how dead palette roles survived there previously. Making `widgets` `pub(crate)` would let the compiler start doing this job.

| Item | Where | Note |
|---|---|---|
| `adpcm.rs` (whole module) | `vgms-synth/src/adpcm.rs` | 218 lines; its consumers were the clean-room YM2608/2610 cores, deleted in the libvgm pivot |
| `TrackEntry::from_song` | `vgms-core/src/pack.rs:117` | remnant of the pre-multichip pack flow |
| `retagged_bytes` | `vgms-ui/src/pack.rs:1723` | superseded by `PackTrack::retagged`; doc claims a caller it lost |
| `render_wav_muted` + `_with_progress` | `vgms-synth/src/wav.rs:114` | superseded by `RenderMix`; eight public entry points for one render loop |
| `Regime::CleanRoom` | `vgms-app/src/parity/mod.rs:316` | unconstructible since the clean-room cores went |
| `dominant_period`, `cents_error` | `vgms-app/src/parity/metrics.rs:307,380` | superseded by `detune_cents`, still credited in the module's headline table |
| `Threshold::max_envelope` | `vgms-app/src/parity/mod.rs:340` | always `None`, never read — a bar that reads as enforced |
| `WRITE_CHAR_OPL` | `vgms-core/src/io/dro.rs:22` | `const false` guarding an unreachable branch; a Python-port leftover |
| `Compression` re-export | `vgms-synth/src/lib.rs:40` | inert public API: importable, unobtainable |
| `reset_value`, `Device::port_name` | `vgms-retrowave/src/commands.rs:88`, `device.rs:215` | no callers; `port_name` forces every test to invent a placeholder for a value nothing reads |
| `autoplay` processorOption | `web/worklet-processor.js:91` | nothing has ever set it |
| unused `log` dep | `vgms-cores-nuked`, `vgms-cores-gpl` | zero `log::` calls in either |
| unused `vgms-core` dep | `vgms-cores-ymfm/Cargo.toml:16` | the crate's only cross-crate import is `vgms_synth` |
| `check.yaml`, `build.yaml` | `.github/workflows/` | see §3.5 |

Three that need a decision rather than a delete:

- **`vgms-cores-ymfm`** has zero dependents, exports no `register()`, and its C++ submodule is compiled by every `cargo test --workspace`. Its lib.rs claims to cover YM2608/YM2610/YMF278B/Y8950 while `build.rs` compiles only `ymfm_opn.cpp` — no YMF278B, no Y8950. But `CORES-REUSE-PLAN.md` records a deliberate 2026-07-29 decision to freeze it at PoC. **Nothing in the crate says it is frozen**, which is why two independent reviewers both flagged it for deletion. Either drop it from `members` or say "frozen PoC" in its lib.rs and move it out of default-members.
- **`RecordingChip`** (`vgms-synth/src/chip.rs:205`) looks dead, and three doc comments falsely claim the engine tests use it (they use seven bespoke stubs instead) — but `docs/render-split-2026-08/PLAN.md` names it as the test vehicle for the next programme. It needs the docs fixed and a rename (there are two different `RecordingChip` types in the crate), not deletion.
- **`tests/make_screenshot.py`** is referenced by nothing — no CI step, no doc — but it *generates* `tests/screenshot.png`, which five Rust sites embed. It is the fixture's provenance, so the fix is a pointer comment in the consumers (or a header in the fixture's README), not removal.

Test-only code shipping in the app crate is a related item: `vgms_app::parity` and `::corpus` (~2,200 lines) plus the `hound` dependency are `pub` modules of the crate that builds the shipped binary, consumed only by integration tests that CI never runs. A dev-only crate or a feature gate would be tidier.

---

## 5. Terminology and consistency — the cheap sweeps

1. **`optimise` vs `optimize`.** Not a dialect complaint — the spellings interleave *within one surface*. The Edit menu says "Optimize VGM" while the undo entry beside it says "Undo Optimise shot.png". In identifiers: `pub struct Optimised` is returned by `optimize_vgm_with`; `VgmFile::optimize` sits beside `unoptimised_chips` in one impl; `vgms-core::optimize` and `vgms-web::optimize_tools` vs `vgms-ui::optimise`. Majority is `optimize`; rename the minority. (Prose can stay British — it already is, consistently.)
2. **`Action::OptimizeImage`** contradicts a deliberate UI decision recorded three lines above it: the button says "Recompress" because "two different jobs must not share one word on the same screen" — and then the code gives them one word.
3. **Wrapped string literals with embedded space runs** — multi-line literals concatenated with their source indentation, so a user-facing `warn!` renders as `…puts          Nuked-OPN2…`. In `parity/mod.rs` ×3, `libvgm/chip.rs:772`, `cores-nuked/opn2.rs` ×3, `app_gui_tests.rs:2073`. One grep for `"  ` inside literals catches the set.
4. **Doc comments attached to the wrong function** — six confirmed, all reading as an insertion made above an existing doc without moving it. `rewind()`'s summary now documents `set_resample_mode` as restarting playback; `run_task`'s doc sits on `measure_source`; `mismatch_alert`'s sits on `waveform_action`.
5. **Stale comments that invert the code.** `palette.rs:622,674,726` describe the pre-complementary display inks — "Ice blue" above a gold literal, "Warm amber" above cyan — so anyone tuning a case from the comments picks the wrong hue family. `resample.rs:991` opens a test doc with the exact claim the test forty lines above debunks in bold.
6. **Two env vars for one concept** — four test suites read `VGMSTUDIO_CORPUS`, four read `VGMSTUDIO_VGMRIPS_CORPUS`; both mean "a tree of .vgm/.vgz files", nothing says they should differ, and setting one makes half the suite skip silently.
7. **Manifest hygiene.** The `zip`/`flate2` pins that protect VGZ byte parity — where feature drift silently flips `flate2` to `zlib-rs` and breaks native↔web byte parity — are hand-copied into three manifests instead of inherited. `vgms-synth-worklet` is the one intra-workspace dep bypassing `[workspace.dependencies]`. `vgms-vgmtools` is the one crate whose lib.rs lacks its SPDX header, and its `unsafe_code = "allow"` override is justified by a comment describing FFI the crate does not contain.
8. **`tools/build-web.ps1:61`** copies all of `web/` into the servable dist, including 27 MB of Playwright `node_modules` and stale `test-results`, then serves them over HTTP.

---

## 6. Not-Invented-Here — mostly a clean bill

I went looking for hand-rolled implementations of solved problems and found the ones that exist are deliberate, with rationales that still hold. Recording that here so it does not get re-litigated:

- `resample.rs`, `wav.rs`, `decompress.rs`, `limiter.rs` live in `vgms-synth`, which is `MIT OR Apache-2.0` on purpose and must stay dependency-light to be reusable. `rubato`/`hound`/`symphonia` would either bloat the permissive half or drag licences across the split.
- `config.rs` INI handling is hand-rolled, but it migrates legacy keys with a documented precedence order and round-trips an ini the user hand-edits. `serde` + an ini crate would not be less code once migrations are honoured.
- `web/wasi-shim` is vendored `browser_wasi_shim` with local modifications — the normal way to do that. (Its debug default is a real problem; see §2.)
- `vendor/nuked-opl3` is patched with upstream-PR-material fixes and a comment saying to drop it when a release carries them. Textbook.

The one genuine inverse: `vgms-pack-archive` copies the entire zip buffer (`Cursor::new(zip_bytes.to_vec())`) when `Cursor::new(zip_bytes)` over the borrowed slice already satisfies `Read + Seek` — doubling peak memory on a multi-MB pack for nothing.

---

## 7. What is working well

Stated plainly, because a list of problems misrepresents the codebase:

- **Comments carry the reasoning.** `banks.rs` documents that the `0x8n` path must not allocate; `optimize.rs` says which function retires when; `gather_key_input` explains why it deliberately does *not* use `egui_wants_keyboard_input`. Several findings died on contact with a comment that had already considered the objection.
- **The licence split is enforced structurally** — copyleft cores live in provider crates the permissive half cannot reach, and `--no-default-features` genuinely yields a copyleft-free build.
- **Parity is tested, not asserted** — a reference-player harness, a projection corpus pinning two readers byte-for-byte, an LLE oracle tier, golden renders. The §1.9 bugs matter *because* this machinery is load-bearing.
- **The optimiser took the harder, cleaner route** — implemented from chip facts rather than ported from vgmtools, which is what lets it live in the permissive crate, with render parity as the net.
- **The dual-target discipline is real.** `dro.rs` guards 32-bit `usize` overflow with a comment explaining wasm32; §1.3 is notable precisely because it is the exception.

---

## 8. Suggested order

1. **First (crash class):** §1.1 the decompress panic, §1.2 the two unbounded allocations, §1.3 the wasm32 overflows. These are small, local, and each wants one test with a malformed fixture. A fuzz target over `vgm::file::read` + `decompress` would cover the whole class permanently and is maybe an afternoon with `cargo-fuzz`.
2. **Next (silent wrongness):** §1.4 loop widening, §1.5 the audio error channel, §1.6 the web node leak, §1.7 the web result generation, §1.9 the green-when-empty scorecard.
3. **One afternoon, pure deletion, zero risk:** the two dead CI workflows, the §4 table, the doc rot in §3.5, the sweeps in §5. Highest value per hour in the report, and it makes everything after it easier to read.
4. **One focused programme:** finish the DRO→VGM migration (§3.1), starting by routing `vgm::io::read` through `file::read`. Deletes more than it adds, and removes the class §1.4 belongs to.
5. **As you touch them:** split `app.rs` (§3.2), unify `pack_zip` and core registration (§3.3), add the dialog registry (§3.4).

Two items need a decision from you before anyone can act: **`vgms-cores-ymfm`** (§4) and whether the ignored parity suites should get a scheduled CI job (§1.9).

---

## Appendix — every confirmed finding

Grouped by area, most severe first. Each was produced by a reviewer that read the files in full and then survived an adversarial verification pass; where the verifier narrowed a claim, its note is included so you can see what was walked back. Line numbers are as of `df3d5cf`.

### vgms-core — the VGM file-format layer

**[HIGH · bug-risk] `crates/vgms-core/src/vgm/file.rs:680`** — repatch_header silently widens an early-stop loop to the full tail on every stream edit

repatch_header always rewrites the header's `loop # samples` as `stream.samples_from(index)` (the full tail after the loop point). A file whose declared loop_samples is deliberately shorter than the tail -- the early-stop loop that VgmFile::loop_end_index (file.rs:303) documents as 'materialised so that saving does not silently widen it', and that this editor itself writes via set_loop_rows(start, Some(end)) -- loses that end on ANY edit that goes through repatch_header: DeleteCommands (undo.rs:296 -> delete_commands), crop_to_region, delete_region, optimize. The parallel Song path preserves the end (VgmMeta::loop_end is slid through deletes; io.rs tests 'deleting_before_the_loop_slides_both_markers' pin it), so the two production paths disagree. Deleting one unrelated command in the any-chip editor and saving persists a widened loop.

*Fix:* In delete_commands/rebuild, capture loop_end_index() before the edit, slide it with the same slide_index_past_deletion arithmetic as the loop point, and have repatch_header emit samples_between(start, end) when an end survives. Add a test mirroring io.rs's loop_end slide tests on the VgmFile path.

**[MEDIUM · bug-risk] `crates/vgms-core/src/vgm/audit.rs:157`** — audit::fix rewrites loop fields it never reported, destroying a deliberate early-stop loop

fix() unconditionally recomputes loop_samples as the full samples_from(loop_at) and calls set_loop, even when the findings were only TotalSamples, TrailingBytes or VersionUnderclaimed. audit() itself deliberately does NOT report a declared loop shorter than the tail (line 92 comment; test a_loop_that_deliberately_stops_early_is_not_a_finding), yet fixing any unrelated finding widens exactly that loop. The user confirms 'Song length: header says X' in the dialog and gets an undisclosed loop change too -- fix does more than audit promised.

*Fix:* Preserve the declared loop_samples when declared <= samples_from(loop_at); only clamp when it exceeds the tail (the LoopSamples finding). Add a test: wrong TOTAL_SAMPLES plus a deliberately short LOOP_NUM_SAMPLES, assert fix leaves the loop length alone.

**[MEDIUM · bug-risk] `crates/vgms-core/src/vgm/audit.rs:117`** — VersionUnderclaimed fires falsely for every pre-1.50 file because minimum_version bakes in the writer's FLOOR

minimum_version (version.rs:38) starts from FLOOR = 0x150, documented as 'the floor this app WRITES'. audit() reuses it as a read-side diagnosis, so any genuine v1.00-1.10 file (VgmHeader::parse accepts down to MINIMUM_VERSION 0x100, and old SMS rips exist) is reported as 'the file uses something from 1.50' -- untrue -- and Edit > Fix Header (vgms-ui/src/editor.rs:754) restamps it 1.50. This contradicts audit's own contract that findings are rare and real. No audit test covers a pre-1.50 file.

*Fix:* Compute the audit's 'needed' without FLOOR (max of chips_version/fields_version/commands_version only), or skip the VersionUnderclaimed check when declared < 0x150. Keep FLOOR for the conversion/write path it was written for.

**[MEDIUM · refactor] `crates/vgms-core/src/vgm/io.rs:197`** — Two parallel VGM document paths (Song via vgm::io, VgmFile via vgm::file) still both live in production

vgm::io::read/write (VgmData, resolve_loop_point, resolve_loop_end) and vgm::file::read/write (VgmStream, loop_index, loop_end_index) each implement container parsing, GD3 handling, loop resolution and byte-exact writing. Both are production: io/mod.rs read_song dispatches .vgm to vgm::io::read (used by vgms-app lib.rs:72 and the web codec), while read_any_song_from_path uses file::read + to_song. projection.rs's own docs frame the parity tests as the gate for a switch-over ('Nothing switches over to the projection until this holds'), and the projection corpus (vgms-app/tests/projection_corpus.rs) already pins row-for-row and byte-for-byte equality. Every semantics change must now be made twice; the early-stop-loop divergence (repatch_header finding) is the cost already realised.

*Fix:* Finish the documented unification: implement vgm::io::read as file::read + to_song (the corpus proves equality), then fold VgmData::read_from_stream, resolve_loop_point and resolve_loop_end away. The OPL-only writer can stay until Song stops being the OPL editor's model.

**[MEDIUM · bug-risk] `crates/vgms-core/src/vgm/stream.rs:339`** — wait_prefix ignores 0x64 OverrideWait while the playback engine honours it

command_wait counts 0x62 as 735 and 0x63 as 882 unconditionally, so total_samples, samples_before/from, seek_index_for_samples, index_at_pct, and the audit's TotalSamples comparison all use un-overridden values. vgms-synth/src/vgm_engine.rs:623-631 applies the override during playback (self.wait_60hz/wait_50hz). For a file using 0x64, the timeline, cursor, seeks, stream_total_ms and audit/fix all disagree with what actually plays, and fix() would stamp a 'corrected' total that is wrong for playback.

*Fix:* Apply overrides while building wait_prefix in parse() (the walk is already sequential, so tracking the two override registers is a few lines), or -- if the divergence is judged acceptable for so rare a command -- document it at command_wait and make the engine ignore overrides so the two agree.

**[LOW · bug-risk] `crates/vgms-core/src/vgm/version.rs:111`** — raw_opcode_version's two arms are unreachable: decode() never yields Raw for 0x31 or 0x40

commands_version matches on stream.get(), i.e. decode() output. decode has explicit arms for 0x31 (AY stereo mask -> VgmCommand::Write, stream.rs:378) and 0x40 (Mikey -> Write, stream.rs:507), so both fall into the `_ => 0` arm of commands_version and raw_opcode_version's 0x31=>1.71 / 0x40=>1.72 mappings never execute. Consequence: a 1.71 file using AY stereo masks computes minimum_version 1.51 (the AY8910 chip is a 1.51 field), so the planned normalise-headers operation would downgrade it and other players would then skip the 0x31 commands; the Mikey case is mostly masked because a declared Mikey clock pins 1.72 via chips_version. No test covers either opcode's version.

*Fix:* Match the decoded commands instead: a Write whose target.kind is Mikey => 0x172, a Write on the AY/YM2203 STEREO_PORT => 0x171; delete raw_opcode_version or keep it only for genuinely reserved opcodes. Add tests for both.

*Verifier narrowed this:* Core claim confirmed: decode has explicit arms for 0x31 (stream.rs:378) and 0x40 (stream.rs:507) producing VgmCommand::Write, so commands_version's Raw arm never sees them and raw_opcode_version's two mappings are unreachable; minimum_version genuinely computes 0x151 for an AY-stereo-mask file. Severity lowered to low: the stated harm (a normalise-headers downgrade) rests on can_downgrade_to, which has no production consumer yet; today's only live effect is a missed VersionUnderclaimed audit warning for rare 0x31-bearing under-stamped files.

**[LOW · consistency] `crates/vgms-core/src/vgm/io.rs:261`** — opl_type_of is implemented twice with the same precedence rule that must never diverge

io.rs's private fn opl_type_of (Result<OplType>) is a byte-for-byte duplicate of the pub projection::opl_type_of (Option<OplType>, projection.rs:44): same YM3812-beats-YMF262 precedence, same dual-bit handling, same dro2vgm caveat in the doc comment. The whole projection parity programme depends on the two readers agreeing about which OPL a header declares, yet the rule lives in two bodies with no enforcing mechanism.

*Fix:* Have io.rs call crate::vgm::projection::opl_type_of(&parsed).ok_or_else(|| Error::file("No OPL2 or OPL3 data detected.")) and delete its private copy.

*Verifier narrowed this:* The duplication is real and verbatim (io.rs:261-269 vs projection.rs:44-52, same precedence, same dro2vgm caveat), and the suggested fold is trivially safe. Severity lowered to low: 'no enforcing mechanism' overstates it -- assert_parity compares opl_type on the fixture corpus and projection_corpus pins the two readers file-for-file -- and the io.rs copy sits in the module the documented unification (finding 5) is slated to fold away.

**[LOW · complexity] `crates/vgms-core/src/vgm/file.rs:303`** — loop_end_index reimplements resolve_loop_end with a linear scan instead of the existing prefix

VgmFile::loop_end_index walks every command from the loop point accumulating waits (O(n) per call, and it is called from vgm_meta() on every to_song), while io.rs's resolve_loop_end (io.rs:313) answers the identical spec question with partition_point over a delay prefix, plus warnings the file.rs copy lacks. VgmStream already stores wait_prefix and exposes samples_before, so the binary-search form is available. Two hand-maintained implementations of one boundary rule (ties on zero-wait rows, mid-delay fallback) is exactly the lockstep hazard the parity tests exist to catch.

*Fix:* Rewrite loop_end_index over wait_prefix (partition_point on samples_before, verifying an exact boundary hit), matching resolve_loop_end's semantics, or share one helper on VgmStream that both readers call.

*Verifier narrowed this:* Duplication verified: loop_end_index (file.rs:303-323) is a linear walk of the same boundary rule resolve_loop_end (io.rs:313-343) answers via partition_point, minus the warnings, and VgmStream already carries wait_prefix. Severity lowered to low: the two implementations' agreement is actively pinned (projection.rs a_loop_that_ends_early_projects_identically plus vgm_meta equality in the parity tests), and the O(n) walk sits inside an already-O(n) to_song snapshot rebuild, so the hazard and cost are both smaller than 'medium' implies.

**[LOW · consistency] `crates/vgms-core/src/vgm/data.rs:150`** — VgmData and VgmStream give the same-named offset APIs different contracts

VgmData::byte_offset panics on an out-of-range index (data.rs:154) where VgmStream::byte_offset returns Option (stream.rs:681); VgmData::index_at_byte_offset rejects the end-of-stream offset (plain binary_search, data.rs:197) where VgmStream::index_at_byte_offset returns Some(len()) for the tail (stream.rs:694-696). Concretely, a loop pointer aimed exactly at the end marker is dropped with a warning by the Song reader (resolve_loop_point) but resolves to Some(len) in VgmFile::loop_index -- an edge the parity proptests never generate (loop_at is filtered to < commands.len()).

*Fix:* Align the two: make VgmData::byte_offset return Option, and pick one tail rule for index_at_byte_offset on both types (then extend the parity proptest to generate a loop at the end marker).

**[LOW · confusing] `crates/vgms-core/src/vgm/header.rs:387`** — ChipSettings doc says 'the playback engine to come' and 'nothing reads them' -- both now stale

The struct doc reads 'They exist for the playback engine to come; nothing in the metadata tier reads them.' The engine has arrived: vgms-cores-libvgm/src/chip.rs consumes ChipSettings throughout (configure_ay8910, configure_segapcm, configure_sn76496, etc.). A reader auditing which fields are live would be misled into treating these as speculative.

*Fix:* Reword to state the actual consumers: read here, applied by the core providers' configure hooks; the metadata tier itself still ignores them.

*Verifier narrowed this:* Half right. 'The playback engine to come' is genuinely stale -- vgms-cores-libvgm/src/chip.rs consumes ChipSettings throughout via the configure hooks (lines 1028, 1475-1629). But the finding misquotes the other half: the doc says 'nothing in the METADATA TIER reads them' (header.rs:387), which is scoped and still true, not an unqualified 'nothing reads them'. Scope reduced to the one stale clause; severity stays low.

**[LOW · consistency] `crates/vgms-core/src/vgm/mod.rs:18`** — ExtraClock is the only extra-header type not re-exported, and lib.rs exports a different subset again

vgm/mod.rs re-exports ExtraHeader and ExtraVolume but not ExtraClock, even though ExtraHeader::clocks is a public Vec<ExtraClock>; consumers must spell crate::vgm::header::ExtraClock. lib.rs (line 46) then re-exports ExtraHeader but neither ExtraVolume nor ExtraClock, so vgms-synth reaches for vgms_core::vgm::ExtraVolume by module path. Three different export sets for one family of types.

*Fix:* Re-export ExtraClock alongside its siblings in vgm/mod.rs and make the lib.rs list match (or drop the partial crate-root re-exports and standardise on the vgm:: path).

**[LOW · bug-risk] `crates/vgms-core/src/vgm/stream.rs:715`** — VgmStream::get hands decode the unbounded tail, so version-sized commands can read past their own end

get() calls decode(&self.data[start..]) rather than the already-computed raw_command slice. For the reserved range that is 2 bytes before v1.60, decode's 0x40 arm still reads byte(2) -- the FIRST BYTE OF THE NEXT COMMAND -- as the Mikey data operand, so describe()/find_next report a fabricated value for such rows in pre-1.60 files (the engine is protected only because routing drops writes to an undeclared Mikey, as the 0x40 comment notes).

*Fix:* Use self.raw_command(index) in get() (and pass the sized slice anywhere else decode is fed), so operand reads fall off the command's own end and yield the documented zero instead of a neighbour's byte.

**[LOW · terminology] `crates/vgms-core/src/vgm/io.rs:1`** — vgm::io's name and module doc claim the container layer, but it is the OPL-only Song serialisation

The header doc says 'The VGM container and its gzipped form, VGZ' with the byte-exact round-trip pitch -- a description that now equally (and more generally) describes vgm::file. In the same crate, `vgm::file::read` opens any VGM while `vgm::io::read` refuses non-OPL files, and projection.rs refers to this module as 'the old OPL-only reader'. A newcomer looking for the container lands in the wrong file; the sibling top-level crate::io module compounds the collision.

*Fix:* Rename the module (opl_song.rs / legacy.rs) or at minimum rewrite its module doc to say 'the OPL-subset Song reader/writer; the chip-agnostic container is vgm::file' -- whichever fits the planned unification.

*Checked and dismissed here:* file.rs is 2069 lines mixing the document model, region edits, and container serialisation; VgmFile::is_opl_only and VgmHeader::is_opl_only have no production callers and contradict opl_type_of; WRITE_CHAR_OPL is a const false guarding a permanently dead branch; u32_at/put_u32 are defined three times across the vgm serialisation files; The gzip decompress/compress wrappers are duplicated verbatim between file.rs and io.rs; can_downgrade_to and blockers have no callers outside their own tests.

### vgms-core — song model, undo, edits

**[LOW · refactor] `crates/vgms-core/src/state_patch.rs:38`** — Two parallel state-restore stacks (StateFold vs ChipState) with no agreement test between them

The crate carries two complete implementations of "carry chip state across a cut": the OPL-projection stack (opl_state.rs OplState + state_patch.rs StateFold/append_patch, driving crop.rs and split_songs.rs materialise) and the any-chip stack (chip_state.rs ChipState + vgm/file.rs region_bytes/extract_region, driving materialise_vgm and VgmFile edits). The optimiser halves of this duality are explicitly transitional and pinned: optimize.rs:257 says merge_stream_delays "will replace merge_delays when VgmData retires", and vgm/projection.rs has the_two_redundancy_passes/the_two_delay_mergers_agree_on_opl agreement tests. The state-restore pair has neither a pinning test (nothing asserts StateFold's prelude equals ChipState's restore on an OPL file) nor a retirement note, yet both must implement the same last-write-wins semantics; crop_to_region/delete_region exist only on the OPL side while extract_region exists only on the VgmFile side.

*Fix:* Add an agreement test pinning StateFold/append_patch against ChipState::restore_indices/changes_from on OPL streams (mirroring the two existing pinning tests), and record the retirement plan for the OPL-side fold in state_patch.rs's module docs so the duplication is visibly transitional rather than accidental.

*Verifier narrowed this:* Core premise overstated. An agreement test DOES exist: projection_corpus.rs::compare_split pins StateFold-materialise against ChipState-materialise_vgm (same final chip state + length) across the corpus, and its doc explains why byte-level pinning is impossible by design (emission order, zero-write handling) - so the suggested prelude-equals-restore pin is documented as wrong. The retirement plan IS recorded (HANDOVER.md: StateFold/crop.rs 'still serve the DRO path, the only caller left'; decision 14). The claimed asymmetry is false: VgmFile has crop_to_region (file.rs:403) and delete_region (file.rs:470). What survives, at low: the corpus pin is #[ignore]d rather than an always-on unit test like the two optimiser pins, and state_patch.rs module docs do not mention the transitional status.

**[LOW · terminology] `crates/vgms-core/src/optimize.rs:149`** — Two pub functions named redundant_indices in one crate; only the OPL one is re-exported at the root

optimize::redundant_indices(&Song) -> Vec<usize> (optimize.rs:149) and chip_state::redundant_indices(&VgmStream, Option<usize>) -> Vec<usize> (chip_state.rs:261) implement the same concept for the two document kinds, under the identical name with different signatures. lib.rs:37 re-exports only the optimize one, so vgms_core::redundant_indices silently means the OPL-song variant while the general variant must be path-qualified. Grep hits and doc links for the name land on both; callers in vgm/file.rs and vgm/projection.rs must fully qualify to disambiguate.

*Fix:* Rename one of the pair (e.g. chip_state::redundant_command_indices, or optimize::redundant_write_indices) or stop re-exporting either at the crate root so both are always module-qualified, matching how the two delete/undo command families are kept distinct (DeleteInstructions vs DeleteCommands).

*Verifier narrowed this:* Facts confirmed (optimize.rs:149, chip_state.rs:261, lib.rs:37), but medium overstates it: module-qualified same-name twins are this crate's established style (vgm::io::read vs vgm::file::read), every existing caller already path-qualifies, and a workspace grep shows no consumer of the ambiguous root re-export outside vgms-core. The residual issue is only that lib.rs re-exports the legacy OPL variant bare.

**[LOW · refactor] `crates/vgms-core/src/convert.rs:69`** — Four separate implementations of chunking a wait into 0x61 commands

The "emit N samples as one-or-more 0x61 commands capped at 65535" loop is written independently four times: convert.rs VgmStream::wait (lines 69-82), split_songs.rs append_delay's Vgm arm (lines 301-309), vgm/file.rs append_wait (lines 136-144), and the bulk-chunk loop inside optimize.rs encode_wait (lines 408-415). Three of them are byte-for-byte the same naive greedy chunker. A change to the chunking policy (e.g. adopting encode_wait's optimal tail everywhere) would have to touch all of them with nothing enforcing agreement.

*Fix:* Move one append_wait(bytes: &mut Vec<u8>, samples: u64) helper into vgm::data or util and call it from convert.rs, split_songs.rs and vgm/file.rs; optimize.rs's encode_wait can keep its optimal encoder but reuse the same chunk emitter for the bulk portion.

**[LOW · terminology] `crates/vgms-core/src/convert.rs:44`** — Private struct VgmStream in convert.rs shadows the crate's public vgm::stream::VgmStream

convert.rs:44 defines `struct VgmStream` (a write-side accumulator of output bytes) while the crate also has the public parsed-command type `vgm::stream::VgmStream`, re-exported at the crate root (lib.rs:46) and imported by the sibling module optimize.rs. Two unrelated types with the same name in one crate: a reader following `VgmStream` from optimize.rs or the docs into convert.rs meets a different type doing the opposite job (emitting rather than parsing).

*Fix:* Rename the convert.rs accumulator to something write-shaped, e.g. VgmEmitter or VgmOut, and mention in its doc comment that it feeds VgmData::new.

**[LOW · confusing] `crates/vgms-core/src/config.rs:490`** — output_backend migration comment sits above the resampling block it does not describe

The four-line comment at config.rs:490-493 ("`output_backend` is the OPL row's core choice under a legacy name... Applied *before* the `core.*` keys so an explicit new-style choice wins") is placed directly above `if let Some(value) = lookup(&ini, "audio", "resampling")` (line 494). The code it actually describes is the output_backend block twelve lines later (lines 505-508). A reader skimming apply_ini associates the migration rationale with the resampling setting, which has its own separate comment inside its block.

*Fix:* Move the comment down so it sits immediately above the `lookup(&ini, "audio", "output_backend")` block at line 505.

**[LOW · confusing] `crates/vgms-core/src/config.rs:891`** — Test doc comment about retrowave_port is attached to the renders_samples test

Lines 891-892 ("An unset port means \"find one\", which is different from a port literally named the empty string.") describe the behaviour verified by an_empty_retrowave_port_reads_back_as_no_port (line 904), but they are part of the doc comment on only_the_emulator_renders_samples_this_program_can_shape (line 896), whose own description follows on lines 893-894. Two tests' doc comments have been merged onto the wrong test, so the port test now has no explanation and the renders_samples test opens with an unrelated claim.

*Fix:* Move the first two comment lines down onto an_empty_retrowave_port_reads_back_as_no_port.

**[LOW · confusing] `crates/vgms-core/src/song/instruction.rs:35`** — Bank::from_bit doc says any non-zero value selects High; the code tests bit 0

The doc comment reads "The bank selected by bit 0 of `value`. Any non-zero value selects `High`." but the body is `if value & 1 == 0 { Low } else { High }`, so value 2 selects Low, contradicting the second sentence. No current caller is affected (the only call site passes `code >> 7`, always 0 or 1), but the doc invites a future caller to pass any non-zero flag and get the wrong bank.

*Fix:* Fix the second sentence to "Any value with bit 0 set selects High" (or debug_assert!(value <= 1) and say the argument must be 0 or 1).

**[LOW · consistency] `crates/vgms-core/src/split_songs.rs:394`** — piece_name and convert::replace_extension implement different rules for swapping a song-name extension

split_songs.rs piece_name (line 394) strips whatever follows the last '.' unconditionally (rsplit_once), while convert.rs replace_extension (line 327) and dro1_default_name (line 315) only treat a 3-4 character suffix as an extension and otherwise append. The same input diverges: "song.backup" becomes "song.vgm" through piece_name but "song.backup.vgm" through replace_extension. Both are private helpers for the same concept (naming a derived song), in two modules, with no shared code or cross-test.

*Fix:* Fold both into one helper in util (keeping convert.rs's 3-4 char plausibility rule, which piece_name's own "my.sound.test.vgm" test is compatible with) and use it from both modules.

**[LOW · idiom] `crates/vgms-core/src/volume.rs:173`** — FULL_SCALE constant exists but boost_for_peak and peak_dbfs repeat the 32_768.0 literal

volume.rs:28 defines FULL_SCALE: f64 = 32_768.0 with a doc comment explaining why 0x8000 (not 0x7FFF) is the reference, but boost_for_peak (line 173) and peak_dbfs (line 198) hard-code 32_768.0 again as f32 literals. The rationale in FULL_SCALE's comment (matching vgm_vol's 0 dBFS point) applies to all three uses; a future change to the reference level would have to find the two literals by eye.

*Fix:* Use `FULL_SCALE as f32` (or an f32 twin constant beside it) in boost_for_peak and peak_dbfs.

**[LOW · other] `crates/vgms-core/src/optimize.rs:382`** — encode_wait rebuilds the short-wait and ~360-entry tail tables on every call

encode_wait (line 382) calls short_waits() -- which heap-allocates an 18-entry Vec -- and then builds a fresh `tails` Vec of 1 + 18 + 18*18 = 343+ entries, per invocation. It is invoked once per multi-delay run in flush_run and in merge_stream_delays' flush closure, so optimising a large stripped VGM performs thousands of identical table constructions. Correct, but pure constant data recomputed in a loop.

*Fix:* Make short_waits a const array (the values are compile-time known) and either precompute the tail table once per merge pass or as a lazily-initialised static.

**[LOW · bug-risk] `crates/vgms-core/src/chip_state.rs:124`** — ChipState::is_empty ignores dac_stream and seek, so a non-trivial state can report empty

is_empty (chip_state.rs:123-126) checks only `latches` and `blocks`, but restore_indices() also emits dac_stream entries and the seek (lines 141-146). A state folded over a span containing only 0x90-0x95 DAC-stream setup or an 0xE0 seek returns is_empty() == true while restore_indices() is non-empty. len() similarly counts only latches but is documented as such ("How many cells are latched"); is_empty has no doc and ChipState is re-exported at the crate root (lib.rs:32), so an external caller using is_empty() to skip a restore would drop real state. Today the only caller is a test, so this is latent rather than live.

*Fix:* Either include dac_stream/seek in is_empty (self.latches.is_empty() && self.blocks.is_empty() && self.dac_stream.is_empty() && self.seek.is_none()) or document it as "no latches or blocks" and rename to match, keeping restore_indices().is_empty() as the authoritative emptiness check.

**[LOW · confusing] `crates/vgms-core/src/crop.rs:107`** — crop_to_region's inline comment says a pre-region index 'has lost its target' but the code maps it to Some(0)

The comment at crop.rs:106-107 describes the remap closure's three cases as "one before it has lost its target, and one after it is gone", yet the match arm at line 109 returns Some(0) for index < start -- the loop point is deliberately re-homed to the start of the cropped song (so the prelude replays on each wrap). "Lost its target" reads as None, which is the behaviour of the third arm only. The actual rationale for Some(0) lives solely in the test a_crop_remaps_a_loop_point_onto_the_kept_region (line 559).

*Fix:* Reword the comment to state the policy, e.g. "one before it re-homes to the start of the kept region (the region is now the whole song), and one at or past the end is gone".

*Checked and dismissed here:* merge_stream_delays returns the loop as a byte offset; its twin merge_delays returns command indices; state_patch.rs re-declares OplState's REGISTER_COUNT/FILE_COUNT and array shape without an enforcing link; The delete path sorts, dedups and bounds-filters the same index list three times.

### vgms-core — pack mode, chip docs, analysis

**[MEDIUM · dead-code] `crates/vgms-core/src/pack.rs:117`** — TrackEntry::from_song has no callers outside its own unit tests

A workspace-wide grep for `TrackEntry::from_song` finds only pack.rs's own tests (lines 1972, 1986) and a mention in docs/vgm-multichip-2026-07/HANDOVER.md. The pack UI builds every entry via TrackEntry::from_vgm_file (crates/vgms-ui/src/pack.rs:1156 read_track), and PackSong models tracks only as Vgm(Arc<VgmFile>) or Unreadable — there is no Song-based pack path anymore. from_song's doc claims its stream-derived timings "stay correct after trimming", a property nothing exercises. It appears to be a remnant of the pre-multichip pack flow.

*Fix:* Delete TrackEntry::from_song and its two test assertions (or, if it is meant as reusable library API for external consumers, say so in its doc comment so the next reviewer does not re-litigate it). Removing it also drops the now-unused `Song` half of the `use crate::song::{OplType, Song}` import.

**[MEDIUM · refactor] `crates/vgms-core/src/chip_docs/opn.rs:18`** — OPN FM register table duplicated verbatim between opn.rs and ym2612.rs with no pinning test

opn.rs lines 23-131 (LFO, TIMER_A_HIGH/LOW, TIMER_B, MODE_TIMER, KEY_ON_OFF, DT_MULTI, TOTAL_LEVEL, KS_AR, AM_DR, SR, SL_RR, SSG_EG, FREQ_LOW, FREQ_HIGH, CH3_FREQ_LOW, CH3_FREQ_HIGH, FB_ALGO, LR_AMS_PMS) and the fm() match at 402-423 are byte-identical to ym2612.rs lines 15-117 and its match arms 143-158 — opn.rs's own header even says "The FM section is the YM2612's layout exactly -- same operator ranges, same `addr & 3 == 3` holes". Unlike the OPL family (where the restated tables are pinned to regdata by tests.rs::the_opl_docs_mirror_regdata_exactly), nothing stops these two ~120-line copies drifting: a wording or mask fix in one silently misses the other.

*Fix:* Extract the shared FM constants + per-channel match into one place (e.g. an `opn_fm` submodule both files call, with the Ym2203 stereo gate as a parameter, since the YM2612 is itself an OPN2), or — if keeping the copies is preferred for readability — add a pinning test that walks 0x30..=0xB6 on both ports asserting opn::doc(chip,..) and ym2612::doc(..) return equal RegisterDocs.

**[MEDIUM · bug-risk] `crates/vgms-core/src/chip_docs/opl.rs:253`** — Find dropdown offers OPL3-only registers for YM3812/YM3526/Y8950 that can never match

documented_registers (mod.rs:92) returns the single opl::NOTABLE list for all four OPL chips, and NOTABLE includes (1, 0x04, "Four-Operator Enable") and (1, 0x05, "OPL3 Mode Enable"). For the two-operator chips register_doc correctly refuses these (opl::doc returns None for port==1 && chip != Ymf262; tests.rs:39 asserts it), and the stream decoder never produces a port-1 write for them (stream.rs ym_family: 0xA/0xB/0xC are all port 0). So the find dialog (vgms-ui/src/dialogs/find_reg.rs:212) lists two rows for a YM3812 that are wrong for the chip and whose search can never match anything. The pinning test notable_lists_exist_for_documented_chips only iterates K::Ymf262 for the OPL family, so the "every notable has a doc" invariant is silently violated for the other three. (Contrast NOTABLE_2608 shared with the YM2610, where the sharing is explicitly justified: both addresses resolve to real registers.)

*Fix:* Either split NOTABLE into a two-op list and an OPL3 list selected per chip in documented_registers, or make documented_registers filter entries through register_doc(chip, port, addr).is_some(). Extend the test loop to include Ym3812, Ym3526 and Y8950 so the invariant is enforced for the whole family.

**[MEDIUM · maintainability] `crates/vgms-core/src/pack.rs:5`** — pack.rs module doc describes two concerns; the 2412-line file now holds six

The module doc (lines 3-9) says the module owns "the two things that are pure data transforms -- generating and parsing the description, and generating the playlist". The file has since accreted: PNG header inspection (PngInfo + DISPLAY_MODES, lines 218-308), system presets (310-412), vgm_ren file naming (414-518), and the entire submission-readiness engine (lines 960-1371: Severity, ReadinessCategory, MetaField, ReadinessTarget, ReadinessItem, TrackFacts, readiness + five check functions). At 2412 lines it is the largest file in vgms-core, and the readiness half shares nothing with the description half except PackMeta.

*Fix:* Split the readiness checks (and plausibly PngInfo + the vgm_ren naming helpers) into submodules (pack/readiness.rs, pack/naming.rs) re-exported from pack, and refresh the module doc to name everything the module actually owns.

**[LOW · confusing] `crates/vgms-core/src/chip_docs/mod.rs:209`** — describe_sn76489's `target.port == 1` arm is unreachable and its comment names the wrong field

The method doc (line 202) says "Port 1 is the Game Gear stereo mask", and the code tests `target.port == 1 || addr == 1`. But the stream decoder encodes GG stereo as addr == 1 with port 0 for both instances (stream.rs:366 `0x4F => write(ChipTarget::first(Sn76489), 1, ..)` and 0x3F for the second chip); no producer of VgmCommand::Write ever sets port 1 on an Sn76489 target. The port half of the condition is dead and the comment actively points a reader at the wrong encoding.

*Fix:* Drop the `target.port == 1` test, keep `addr == 1`, and fix the comment to "addr 1 carries the Game Gear stereo mask (opcodes 0x4F/0x3F)". If the port test is meant as defence-in-depth, say so explicitly instead of stating it as the encoding.

**[LOW · consistency] `crates/vgms-core/src/chip_docs/opn.rs:148`** — The OPN SSG docs describe the same AY-3-8910 registers as ay8910.rs with different wording and inverted polarity

opn.rs's SSG section (lines 133-200) restates the AY-3-8910 register layout the OPN chips inherit (its own header says so), but with a different vocabulary from ay8910.rs: mixer bits are "Noise C off" / "Tone A off" (opn.rs:149-159) vs "Noise C enable" / "Tone A enable" (ay8910.rs:40-52) — opposite polarity labels for identical active-low bits; "I/O port B direction" vs "IO port B direction"; "Tone period (coarse 4 bits)" vs "Tone period (high 4 bits)"; "Envelope period (coarse 8 bits)" vs "(high 8 bits)". The instruction table will therefore describe the same hardware bit two different ways depending on which chip the write targets.

*Fix:* Pick one wording (the ay8910.rs one, which matches the register name's "active low" note) and either share the field arrays between the two modules (an `ssg_common` set of consts with role names, opn adding its "SSG:" register-name prefix) or add a small test pinning the two modules' field descriptions and masks to each other for registers 0x00-0x0F.

**[LOW · refactor] `crates/vgms-core/src/analysis.rs:146`** — The changed-fields diff/borrow/join logic exists twice: RegisterAnalyzer::describe_register and chip_docs::describe_changes

analysis.rs:160-186 (mask-diff closure, count-first-to-borrow for 0/1 fields, `join(" / ")` for more) is the same algorithm as chip_docs/mod.rs:237-266 describe_changes, whose doc even says "The Description wording both analysers share" — they share the wording but not the code. The bridge already exists and is test-pinned: opl::doc_for_kind maps every regdata::RegisterKind to an equivalent RegisterDoc, and tests.rs::the_opl_docs_mirror_regdata_exactly asserts the two are identical. Relatedly, kind_for (analysis.rs:213) re-implements the same high-bank-precedence lookup as opl::doc (opl.rs:239).

*Fix:* Have RegisterAnalyzer::describe_register delegate: widen opl::doc_for_kind's visibility to pub(crate) and call chip_docs::describe_changes(opl::doc_for_kind(kind), previous.map(u16::from), value.into()), deleting the duplicate diff logic. (The independent tests-side reference_rows oracle should stay independent.)

**[LOW · confusing] `crates/vgms-core/src/pack.rs:158`** — title_from_filename doc says "two-or-more digit number" but the code strips a single-digit prefix too

The doc comment (lines 157-158) promises to remove "the leading two-or-more digit number and its trailing space", but the implementation strips whenever `digits > 0` (line 165) — so "1 Foo.vgz" yields "Foo", which the comment says should not happen. No test pins the single-digit case, so it is unclear which behaviour is intended (the lenient one is plausibly right for hand-named folders; the VGMRips convention is two digits).

*Fix:* Decide the intent and align: either change the guard to `digits >= 2` to match the comment, or fix the comment to "one-or-more digits" and add a test case for "1 Foo.vgz" so the behaviour is pinned.

**[LOW · confusing] `crates/vgms-core/src/chip_docs/mod.rs:118`** — address_width claims QSound has 16-bit register addresses, contradicting its own rule and the decoder

address_width's doc says it "Follows the addressing the stream decoder produces", but decode's QSound arm (vgm/stream.rs:427-431) normalises `0xC4 mm ll rr` to addr = byte(3) — an 8-bit register — with the 16 bits going to *data*. Listing QSound in the 16-bit arm makes the find dialog offer a 4-digit hex box (find_reg.rs:221) where addresses above 0xFF can never match a decoded command. The table also has no test tying it to the decoder, so rows like this can drift silently as chips are added.

*Fix:* Move QSound to the 8-bit default, and add a comment (or a small test over representative decoded commands) documenting which opcode gives each 16-bit-listed chip its wide address, so the lockstep with decode is checkable.

**[LOW · other] `crates/vgms-core/src/analysis.rs:87`** — Replay catch-up builds and discards a description for every skipped row

RegisterAnalyzer::row's catch-up loop (lines 87-93) calls step(), which allocates a `format!("Delay: {ms} ms")` String for every delay and a joined String for every multi-field write, only for the RowAnalysis to be dropped — the comment even says "discarding descriptions". ChipAnalyzer::row (chip_docs/mod.rs:171-174) has the same shape via describe_changes. The UI's AnalysisCache memo (vgms-ui/src/analysis.rs) hides this in the steady state, but every backward jump past the memo (or after the 50k-row memo clears on a large VGM) replays the whole prefix with O(n) throwaway allocations per query.

*Fix:* Split step into a state-only apply (bank tracking + state-array/BTreeMap insert, no string building) used by the catch-up loop, and describe only the requested index. In describe_register the split is natural: recording the value is independent of composing the description.

### vgms-synth — playback and render engine

**[MEDIUM · dead-code] `crates/vgms-synth/src/adpcm.rs:1`** — adpcm.rs is a dead module: no code in the workspace uses AdpcmA or DeltaT

The module is declared `pub mod adpcm` in lib.rs but nothing re-exports or imports it. A workspace-wide grep for `AdpcmA|DeltaT|adpcm::` matches only adpcm.rs itself and vendor/ C sources; no provider crate (vgms-cores-libvgm/-gpl/-nuked/-ymfm), no engine code, and no test outside the module's own unit tests touches it. Its stated consumers ('the chip glue that owns them', i.e. the clean-room YM2608/YM2610 cores) were deleted when the cores programme replaced every clean-room core with libvgm, so this is a stale remnant of that pivot. 218 lines of codec plus tests are compiled and maintained for no caller.

*Fix:* Delete the module (and its PROVENANCE.md row if one exists), or, if it is being kept deliberately as a documented clean-room codec for a future core, say so in the module docs and file an issue reference so the next reviewer does not have to re-derive that it is intentionally dormant.

**[MEDIUM · refactor] `crates/vgms-synth/src/vgm_engine.rs:466`** — ~70 lines of loop/transport state machinery are duplicated verbatim between the two engines

PlayerEngine (engine.rs 543-617) and VgmEngine (vgm_engine.rs 440-504) each carry the identical trio of fields (loop_config, wraps_remaining, loops_done) and near-verbatim copies of set_loop (same empty/out-of-range filter), loop_config(), restart_loop_count(), wrap_to_loop_start() (including the same 'the loop region renders no audio; playing on without looping' warn string and the frames_rendered==start_frames spin guard), owes_a_wrap(), and the Position::looping construction. seek_to_ms's prefix-sum contract is also restated in both. A behaviour fix in one (e.g. the spin guard) must be hand-mirrored in the other with no mechanism enforcing it — the two log strings already have to be kept in sync by eye.

*Fix:* Extract a small LoopState (or Transport) struct owning {config, wraps_remaining, loops_done, frames_rendered} with set(config, len), restart(), wrap(&mut index) -> bool, owes_a_wrap(index) and position(rate, index); have both engines delegate. The engines themselves are deliberately separate (OPL register policy vs generic cores) — only this plumbing is shared.

**[MEDIUM · other] `crates/vgms-synth/src/vgm_engine.rs:667`** — DAC stream start copies the entire sample bank twice, on the audio thread

The 0x93 handler builds `self.banks.concatenated(..)` — a full copy of every block of the type, potentially megabytes for a Mega Drive/Mega CD rip — and then `DacStreams::start` (dac_stream.rs:210) does `stream.data = data.to_vec()`, a second full copy, even when the playing range [start, end) is a few kilobytes. The 0x95 fast form likewise copies the block (`map(<[u8]>::to_vec)`) and then start copies it again. These run inside run_until_wait, i.e. inside the audio callback during live playback, and 0x93/0x95 fire per sample trigger in stream-heavy files. banks.rs's byte_at explicitly documents that the 0x8n path 'must not allocate', so the crate already treats this path as hot; the copy itself is justified ('a later block must not change what a running stream is playing') but its size and duplication are not.

*Fix:* Store only the [start..end) slice in the stream (folding stream.start/position/end down to the slice), and cache the concatenated bank per type inside Banks (invalidated on push) or hand out Arc<[u8]> so a start shares rather than copies. Either change preserves the snapshot semantics the comment asks for.

**[LOW · dead-code] `crates/vgms-synth/src/chip.rs:205`** — pub RecordingChip is unused, and three doc comments claim the engine tests use it when they do not

`chip::RecordingChip` is only ever constructed in chip.rs's own smoke test (line 320). Its fields `mutes`, `pans`, `pan_capable` and the `at_rate` constructor are never read anywhere. The docs in vgm_engine.rs (lines 16-18, 'Routing, banks and timing are all testable against RecordingChip') and on `VgmEngine::with_cores` (lines 207-210, 'a RecordingChip answers those') are false: the vgm_engine tests define seven bespoke Arc<Mutex<Vec>>-logging stubs (Constant, Tap x4, Ramp, RomTap, RamTap) precisely because VgmEngine takes ownership of a Box<dyn ChipCore> with no accessor back, which RecordingChip's plain-field design cannot serve. Separately, engine.rs's test module defines its own unrelated `struct RecordingChip` (an OplChip recorder, line 873), so the crate has two different types under one name.

*Fix:* Either give RecordingChip a shared-log design (Arc<Mutex> internals or a chip accessor on VgmEngine) and fold the seven bespoke test stubs into it, or delete it and correct the three doc comments. Rename one of the two RecordingChip types either way.

*Verifier narrowed this:* The false-docs half is confirmed (vgm_engine.rs:16-18, 207-210, chip.rs:12 all claim RecordingChip tests the engine; the tests actually use bespoke Arc<Mutex> stubs because VgmEngine owns the Box<dyn ChipCore> with no accessor), as is the two-types-one-name collision. But the 'unused, consider deleting' half is undercut: docs/render-split-2026-08/PLAN.md pm-2 (committed 2026-08-02, the active next programme) names RecordingChip as the ChannelGate test vehicle, so it is deliberately-kept API with an imminent consumer. Scope narrows to fixing the three doc comments (or the shared-log redesign) and the rename; deletion is wrong. Severity drops to low.

**[LOW · bug-risk] `crates/vgms-synth/src/vgm_engine.rs:582`** — 0x64 wait override misfires on literal 0x61 waits of exactly 735/882 samples

vgms-core decodes 0x61 nnnn, 0x62 and 0x63 all into VgmCommand::Wait(samples) (stream.rs:522-524), so the opcode is lost. VgmEngine::execute then treats *any* Wait(735) as 'the 60 Hz wait' and substitutes self.wait_60hz. After a 0x64 override redefines the short waits, a file that also encodes a literal `0x61 DF 02` (735 samples — something vgm_cmp-style tools can emit) has that wait silently remapped to the overridden length, drifting the whole song. The failure needs a rare command (0x64) plus a plausible encoding, so it is latent, but the model genuinely cannot represent the distinction the spec makes.

*Fix:* Have the decoder keep the opcode distinct (e.g. VgmCommand::Wait60th / Wait50th variants, or a flag on Wait) and match on that in execute, so only genuine 0x62/0x63 commands are subject to the 0x64 override.

**[LOW · confusing] `crates/vgms-synth/src/vgm_engine.rs:326`** — rewind()'s doc summary is glued onto set_resample_mode's doc comment

The doc block on set_resample_mode opens with two unrelated summary lines: 'Restarts from the first command with every chip reset.' followed by 'Chooses how every voice is brought to the output rate.' The first sentence describes rewind() (line 341), which has no doc comment at all — the docs were evidently split apart during an edit and the summary landed on the wrong function. Rendered rustdoc for set_resample_mode now claims it restarts playback, which it does not.

*Fix:* Move 'Restarts from the first command with every chip reset.' down to be rewind()'s doc comment and leave set_resample_mode's doc starting at 'Chooses how every voice...'.

**[LOW · dead-code] `crates/vgms-synth/src/wav.rs:114`** — render_wav_muted and render_wav_muted_with_progress are superseded wrappers with no callers left

Since the RenderMix refactor, the muted render is expressible as render_wav_mixed / render_wav_cancellable with a RenderMix{muting,..}; split.rs (the original consumer) now calls render_wav_cancellable directly (split.rs:194). A workspace grep shows render_wav_muted and render_wav_muted_with_progress are called only from wav.rs's own tests. render_wav_boosted (without progress) is in the same position — only its _with_progress sibling has a real caller (vgms-app cli/render.rs:65). Eight public entry points for one render loop is API surface that each needs doc upkeep and a pass-through test.

*Fix:* Drop the muted pair (and consider render_wav_boosted) in favour of render_wav_mixed/render_wav_cancellable, or keep them only if the crate's external-library API is meant to stay wide — in which case move their equivalence tests next to the wrappers they pin and note the redundancy.

**[LOW · consistency] `crates/vgms-synth/src/wav.rs:272`** — render_vgm_wav masks an impossible cancellation with unwrap_or_default; the OPL twin uses expect

render_vgm_wav passes `&mut || true` for keep_going and then does `.map(|bytes| bytes.unwrap_or_default())`. The OPL counterpart render_uncancelled (line 201-205) handles the same impossible case with `.expect("a render that is never cancelled always completes")`. If the invariant ever broke, the OPL path would panic loudly while the VGM path would silently return an empty Vec, which a caller would write out as a zero-length/duff WAV — the same class of silent failure the codebase elsewhere goes out of its way to avoid.

*Fix:* Use the same expect(...) as render_uncancelled, or route render_vgm_wav through a shared render_vgm_uncancelled helper so the two families cannot diverge.

**[LOW · refactor] `crates/vgms-synth/src/peak.rs:131`** — The chunked pull loop is copy-pasted four times across peak.rs and waveform.rs

measure_peak_cancellable (peak.rs:80-107) and measure_vgm_peak_cancellable (peak.rs:131-157) are byte-for-byte the same loop over different engines (4096*2 buffer, keep_going between chunks, per-sample fold, break on short render); render_waveform_progressive (waveform.rs:175-210) and render_vgm_waveform_progressive (waveform.rs:221-266) duplicate the same loop again with the bucketer. wav.rs already solved this exact problem with write_render taking a `pull: &mut dyn FnMut(&mut [i16]) -> (usize, u64)` closure, so the crate contains both the pattern and its fix side by side. Relatedly, the two wav.rs pulls disagree on progress accounting — the OPL pull reads engine.position().frames_rendered while the VGM pull keeps a separate `rendered` accumulator (wav.rs:362-371), even though peak.rs uses position() for both engines.

*Fix:* Introduce one crate-private drive_render(pull, keep_going, on_chunk) helper in the write_render style and express the two peak scans and two waveform renders through it; while there, use position().frames_rendered for the VGM WAV progress like peak.rs does.

**[LOW · maintainability] `crates/vgms-synth/src/wav.rs:431`** — The synthetic-VGM test fixture builder is duplicated in five test modules

The identical put_u32 + 0x100-byte-header + eof-patch VGM builder appears as `vgm()` in vgm_engine.rs tests (line 873), `vgm_file()` in wav.rs tests (431) and waveform.rs tests (459), and inlined into peak.rs's sms_vgm (299-322) and split.rs's tone1_vgm (391-406). peak.rs and wav.rs additionally duplicate the whole sms_vgm stream byte-for-byte. The crate already has a cfg(test) shared-support module (testing.rs) that all five modules import for install_registry_with_stub, so the home for the builder exists.

*Fix:* Move one vgm(chips, stream) builder (and the sms fixture) into testing.rs and delete the five copies; a future header-layout change then touches one function instead of five.

**[LOW · confusing] `crates/vgms-synth/src/credits.rs:46`** — credits() merge comment says 'same id means same core' but the code merges on label

The comment above the merge (lines 44-45) claims rows are joined because 'Same id means same core', yet the find compares `credit.label == info.label`. Ids cannot be the merge key: they embed the chip slug (sn76489.libvgm vs huc6280.libvgm are the same libvgm core under different ids), so label is the only field shared across a multi-chip core's rows — the code is right and the comment describes a scheme that would not work. As written it also means two distinct cores that ever shipped the same label would be silently merged into one credit, which the comment obscures.

*Fix:* Fix the comment to say rows merge on label because ids are chip-prefixed, and note the resulting constraint (labels must be unique per core across providers) — or add a debug assertion that merged rows agree on authors/license/upstream so a label collision is caught.

**[LOW · terminology] `crates/vgms-synth/src/wav.rs:154`** — Docs still route users to the dead dro_player / dro_split binaries

render_wav_boosted's doc says the boost 'is opt-in through `dro_player --render --boost`', but dro_player no longer exists — the feature ships as a `vgmstudio render` flag (vgms-app cli/render.rs calls render_wav_boosted_with_progress). engine.rs:71 ('for `dro_split`'s channel isolation') and split.rs:55/373 similarly name dro_split, which is now `vgmstudio split`. The split.rs mentions arguably document a naming convention inherited from the old tool, but the wav.rs one tells a reader to invoke a binary that is gone.

*Fix:* Update wav.rs to name the current `vgmstudio render` invocation, and sweep engine.rs/split.rs to either use the current subcommand names or phrase the old ones explicitly as history ('the naming the old dro_split used').

**[LOW · maintainability] `crates/vgms-synth/src/balance.rs:46`** — CHIP_VOLUME/PB_VOL are fixed [u16; 0x2A] tables indexed by ChipKind::id with no explicit length guard

chip_volume() and estimate_contribution() index both tables with `chip.kind.id() as usize` unchecked. Adding a ChipKind past id 0x29 without growing both tables panics with an index-out-of-bounds at runtime. The unity-ratio test happens to iterate ChipKind::all() through voice_gain and so would panic in CI, but that guard is incidental — its assertion message is about gain ratios, and nothing states the lockstep requirement where the next chip-adder will look.

*Fix:* Add an explicit test (or const assertion if ChipKind exposes a COUNT) asserting CHIP_VOLUME.len() == PB_VOL.len() == ChipKind::all().len(), with a message pointing at VGMPlay's _CHIP_VOLUME/_PB_VOL_AMNT as the source for new rows.

*Verifier narrowed this:* Claim narrowed: the unchecked indexing and implicit lockstep requirement are real (balance.rs:76, 106), but there is no realistic runtime-panic exposure -- the finding itself concedes that a_single_chip_file_is_left_exactly_alone iterates ChipKind::all() through voice_gain, which indexes BOTH tables for every kind, so any table/enum mismatch fails CI deterministically as an index-out-of-bounds naming balance.rs. The actionable residue is only an explicit lockstep assertion with a message pointing new chip-adders at VGMPlay's tables, i.e. failure-message clarity, not the 'panics at runtime' framing.

**[LOW · confusing] `crates/vgms-synth/src/resample.rs:991`** — Two test comments in resample.rs flatly contradict each other about the worst ratio

the_worst_ratio_runs_faster_than_realtime's doc (lines 946-950) states, bolded, 'The worst ratio in this app is 5.07:1, not 40:1' and explains the NES APU presents 55.9 kHz. Forty lines later, the_tap_count_stays_bounded's doc (991-993) opens 'The worst ratio this app meets is the NES APU's 40:1' — the exact claim the first comment debunks, and one its own body then walks back ('No core presents a ratio like this today'). A reader landing on the second test alone budgets 1445 taps for a case that does not exist.

*Fix:* Reword the_tap_count_stays_bounded's summary to match its body, e.g. 'A hypothetical 40:1 ratio must stay affordable as a warning shot; the worst real ratio is the SN76489's 5.07:1.'

*Checked and dismissed here:* The shared Position::next_instruction field carries a VGM 'row', and the seek APIs disagree on the noun.

### vgms-ui — application shell and editor

**[HIGH · bug-risk] `crates/vgms-ui/src/app.rs:3943`** — close_song_dialogs leaves Find Loop and Split Songs dialogs open across a song load

close_song_dialogs (app.rs:3943-3950) closes find_reg, dro_info, gd3_tag, vgm_metadata, render_wav and split, but not dialogs.find_loop or dialogs.split_songs, both of which hold snapshots of the song they were opened on (FindLoopDialog holds a LoopSearchDoc; SplitSongsDialog holds a SplitSource). The function's own doc says only Goto and Settings are meant to survive, and load_file's comment explains exactly why stale dialogs are dangerous ('a stale Save silently corrupting it'). Concretely: open song A, Edit > Find Loop, Search, then open song B -- the dialog stays up listing A's candidates; clicking Apply emits SetLoopStart/SetLoopEnd with A's row indices plus ApplyLoopToMetadata, silently writing a wrong loop region into B (dialogs/find_loop.rs:193-210). Similarly a stale SplitSongsDialog's Export re-runs detection on B but applies A's per-segment include flags (which line up with different segments), and Preview seeks B at A's indices. Related: load_file cancels RenderWav/Split/VolumeScan tasks but not TaskKind::LoopSearch (app.rs:2281-2285), so a running search for A keeps streaming candidates into the stale dialog over B.

*Fix:* Add `self.dialogs.find_loop = None; self.dialogs.split_songs = None;` (and `self.tasks.cancel(TaskKind::LoopSearch);`) to close_song_dialogs / load_file, or restructure Dialogs so song-bound dialogs are one group closed en masse, making the next added dialog impossible to forget.

**[HIGH · refactor] `crates/vgms-ui/src/app.rs:254`** — app.rs is a 4807-line god-file spanning eight separable responsibilities

VgmStudioApp's impl mixes: (1) frame layout/chrome (update_impl, ~490-901); (2) service polling and save-outcome routing (poll_services/handle_save_outcome/handle_wav_result, ~905-1347); (3) input (handle_drops/intercept_close/gather_key_input, ~1349-1624); (4) the 530-line handle_action dispatch (~1727-2258); (5) song load/save/close workflows (~2262-2432); (6) the entire pack-mode file-op executor, undo stacks, screenshots and bulk edits (~2434-3614, ~1180 lines -- a file of its own by any measure); (7) the two split flows (SplitFlow/PendingSplit plus ~4221-4456); (8) playback/audio plumbing plus settings previews (~3616-4030, 4441-4618). The section markers ('-- pack mode --', '-- the workflows --') already name the seams. Every new feature lands here (wt-8 zip packs, split songs, previews all did), so merge friction and scroll cost keep compounding.

*Fix:* Convert to a directory module: app/mod.rs keeps the struct, new(), update_impl and the eframe::App impl; move each seam into a submodule holding an `impl VgmStudioApp` block -- app/pack_ops.rs (PackRun/PackRunKind/ScreenshotPick/PendingAdd + lines 2434-3614), app/split_flow.rs (SplitFlow/PendingSplit + begin_split..finish_split), app/audio.rs (do_play family, ensure_audio, push_muting/panning/loop_config, reload_audio_in_place, boost + volume scans), app/poll.rs (poll_services + SavePurpose + handle_save_outcome), app/input.rs, app/settings.rs (apply_settings + the three previews). Private-field access still works because submodules are descendants of `app`, and the gui-tests mount (#[path] child module) is unaffected. handle_action stays in mod.rs as one-line delegations.

**[MEDIUM · confusing] `crates/vgms-ui/src/editor.rs:720`** — replace_vgm_stream dispatches crop-vs-delete by comparing the undo description string

replace_stream takes the DRO edit as a function pointer `edit: fn(&Song, usize, usize) -> Option<CropOutcome>`, but the VGM arm (replace_vgm_stream, editor.rs:712-730) ignores it and re-derives which edit to run by string-comparing `description` against CROP_DESCRIPTION (`match description { CROP_DESCRIPTION => edited.crop_to_region(...), _ => edited.delete_region(...) }`). The const at line 23 exists solely to make this stringly-typed dispatch line up with the undo label. A third region edit added tomorrow silently falls into the `_ => delete_region` arm; renaming the undo label breaks the crop. One operation is thus selected two different ways in the two arms of the same function.

*Fix:* Introduce `enum RegionEdit { Crop, Delete }` carrying both the description and the per-representation implementations (or two small methods on it), and pass that through replace_stream/replace_vgm_stream instead of a fn pointer plus a magic string.

**[MEDIUM · confusing] `crates/vgms-ui/src/editor.rs:153`** — The `dro` slot's documented DRO-only invariant is contradicted by the load fallback and record_saved

The field doc says 'A DRO, the only format this app still holds as a decoded OPL stream' (editor.rs:152-153), and convert_to_vgm's comment claims the round trip 'keeps the DRO slot holding only DROs' (line 816). But Editor::load's fallback (lines 409-432) calls io::read_song whenever vgm::file::read errors, and io::read_song (vgms-core/src/io/mod.rs:21-32) happily reads .vgm/.vgz into a Song -- so any VGM the new whole-file reader rejects but the legacy OPL reader accepts lands in the `dro` slot as a VGM-typed Song. Code still alive for that case: record_saved's DRO arm computes was_vgz/is_vgz and returns `song.is_vgm() && was_vgz != is_vgz` (lines 521-530, duplicating the VGM arm), and save_bytes falls through to write_song's SongData::Vgm arms. Either the invariant is real (then those branches are dead and the fallback should not accept VGM extensions) or it is not (then the field name and both comments lie). Note the projection path is unrelated: snapshot()/song() legitimately hand out VGM-typed Songs, but those never enter `self.dro`.

*Fix:* Decide the invariant: either have the load fallback refuse .vgm/.vgz names (surface the vgm::file::read error instead) and delete the is_vgm/vgz handling from record_saved's DRO arm, or rename/re-document the field to admit it can hold a legacy-read VGM.

**[MEDIUM · refactor] `crates/vgms-ui/src/tasks.rs:125`** — Four near-identical Opl/Vgm source enums plus six copies of the same (snapshot, vgm) match in app.rs

tasks.rs declares WavSource (line 125), SplitSource (165) and LoopSearchSource (114) with the identical shape { Opl(Arc<Song>), Vgm(Arc<VgmFile>) }, alongside vgms_synth::AudioSource with the same two arms; SplitTaskSource differs only by attached options. Feeding them, app.rs repeats the same construction six times: `match (self.editor.snapshot(), self.editor.vgm()) { (Some(song), _) => ...Opl(song), (None, Some(file)) => ...Vgm(Arc::new(file.clone())), (None, None) => ... }` at lines 1876-1880 (OpenFindLoop), 3757-3766 (start_loop_search), 4179-4186 (render_to_wav), 4242-4249 (split_source), 4306-4315 (split_into) and 4481-4489 (audio_source). Each site re-clones the VgmFile into a fresh Arc, and each new task type will add a seventh copy of both the enum and the match.

*Fix:* Add one `Editor::doc_source() -> Option<AudioSource>` (or a shared DocSource enum with From conversions into the per-task request types) and express the task-specific enums in terms of it; keep per-task options in the request structs where they already live. Consider having Editor cache the Arc<VgmFile> so the six-fold `file.clone()` collapses to an Arc clone.

**[MEDIUM · terminology] `crates/vgms-ui/src/strings.rs:55`** — optimise vs optimize is mixed within the same UI surface and identifier space

Not a dialect complaint -- the two spellings are interleaved where users and maintainers see both at once. User-visible: the Edit menu says 'Optimize VGM' (menus.rs:359) and its statuses say 'Optimized: removed...' / 'Nothing to optimize' (strings.rs:90-92, 262-267), while the screenshot path says 'Optimise failed' (APP_ERR_OPTIMISE_TITLE, strings.rs:55), 'Optimising {name}...' (364-381), and lands an undo transaction labelled 'Optimise {name}' (app.rs:3374) -- so the Edit menu can read 'Undo Optimise shot.png' directly under an 'Optimize VGM' item. Identifiers mirror the split: module `optimise` and fn `optimised` (optimise.rs) vs Action::OptimizeVgm, Action::OptimizeImage, Editor::optimize_vgm, optimize_image.

*Fix:* Pick one spelling per audience (likely US 'optimize' to match the existing menu items and Action names), sweep strings.rs and the transaction labels in one commit, and rename the `optimise` module and its fns to match.

**[LOW · maintainability] `crates/vgms-ui/src/app.rs:4694`** — play_seam_label derives its text by string-stripping play_tail_label's prefix

play_seam_label (app.rs:4691-4697) computes `self.play_tail_label().trim_start_matches("Play last ")` to recover the '3 seconds' fragment, coupling itself to the exact wording of app_play_tail_label in strings.rs:448-450. strings.rs's whole charter is 'edit wording here rather than at call sites', but rewording 'Play last {value} second{plural}' (say to 'Play final...') silently makes the seam tooltip read 'Play the loop join: the last Play final 3 seconds of the region' -- no compile error, no test failure.

*Fix:* Have strings.rs expose the duration fragment itself (e.g. app_tail_duration(value, plural) -> String) and build both app_play_tail_label and app_play_seam_label from it; delete the trim_start_matches call.

*Verifier narrowed this:* The claim is fully accurate — play_seam_label (app.rs:4691-4697) does trim_start_matches("Play last ") against app_play_tail_label's exact wording — but the failure is one garbled tooltip with no functional impact; by this list's own calibration (finding 9's silent Save-Pack degradation is low) medium overstates it.

**[LOW · confusing] `crates/vgms-ui/src/app.rs:53`** — Three doc comments have drifted away from the code they describe

(1) app.rs:53-54: 'The DRO timing mismatch box; the v2 advice points at the Settings dialog...' sits fused onto waveform_action's doc block (lines 53-68); it belongs to mismatch_alert at line 92, which now has no doc at all. (2) tasks.rs:292-302: the doc for run_task ('Runs `request`, calling `emit`... the platform-independent half of every TaskService...') is attached to measure_source, with measure_source's own doc appended below it; run_task (line 325) is undocumented. (3) app.rs:1646-1657: playback_tick opens with a comment describing the peak-meter dt advance, but the first statements are the audio last_error check whose comment is buried at the end of that block. All three read as an insertion made above an existing doc without moving it.

*Fix:* Reattach each doc block to its function: move lines 53-54 onto mismatch_alert, split tasks.rs:292-299 back onto run_task, and reorder the playback_tick comments to sit above the code they describe.

**[LOW · confusing] `crates/vgms-ui/src/tasks.rs:4`** — tasks.rs module doc claims the waveform render is the only background task left

The module header ('The register analysis is vgms-core's synchronous replay cursor, so the waveform render is the only background task left') predates most of the file: TaskKind now has seven variants (RenderWaveform, RenderWav, Split, SplitSongs, VolumeScan, PackVolumeScan, LoopSearch), all defined thirty lines below the sentence that denies they exist. First-time readers get an actively wrong mental model of the crate's threading story.

*Fix:* Rewrite the paragraph: the task logic for all background work (renders, splits, scans, loop search) lives here, scheduling lives behind TaskService.

**[LOW · maintainability] `crates/vgms-ui/src/app.rs:199`** — The "vgms-zip-" token prefix is hard-coded independently in app.rs and platform.rs

platform.rs mints the token (`format!("vgms-zip-{}", self.next_id)`, line 247) and recognises it (archive_token, lines 360-365, `starts_with("vgms-zip-")`); app.rs re-implements the recognition from scratch in folder_is_archive (lines 199-203) with its own copy of the magic string, used by open_folder to stamp PackOrigin::MemoryZip. Changing the prefix in platform.rs compiles cleanly while every zip-opened pack silently loses its memory origin in the app -- the dirty-flag/Save-Pack behaviour then quietly degrades to Directory semantics.

*Fix:* Export a `pub const ZIP_TOKEN_PREFIX` (or reuse platform::archive_token) from platform.rs and have folder_is_archive call it, so there is exactly one definition of the token shape.

**[LOW · other] `crates/vgms-ui/src/app.rs:2460`** — start_pack_run clones every mutation's full file bytes to build the run queue

start_pack_run does `transaction.inverse.clone()` / `transaction.forward.clone()` (app.rs:2460-2464) because the PackRun keeps both the queue and the whole transaction. PackMutation::Write carries the complete file contents, so a bulk tag or album-levelling run over N tracks momentarily duplicates every rewritten VGM's bytes (the transaction already holds forward+inverse copies by design; this adds a third copy of one side). advance_pack_run then moves each Vec into files.save, so the clone exists purely to keep the transaction intact for the undo stack.

*Fix:* Store bytes in PackMutation as Arc<[u8]> (SaveRequest can take a Vec via Arc::unwrap_or_clone, or FileService can accept Arc), or drive the queue by index into the retained transaction instead of cloning the mutation list.

**[LOW · maintainability] `crates/vgms-ui/src/menus.rs:54`** — ALL_SHORTCUTS is a hand-maintained mirror of the shortcut consts with no completeness guard

The Help-dialog test guards one direction only: everything in ALL_SHORTCUTS must appear in the dialog tables (dialogs/help.rs:296-311). Nothing guards the other direction -- a new `pub const` shortcut added above (the file's own comment says bindings 'added above and left out of the dialog's tables fail a test') that is forgotten from ALL_SHORTCUTS ships silently undocumented, which is precisely the failure the list exists to prevent. The list currently repeats all 21 const names by hand.

*Fix:* Declare the consts and the list together with a small macro (`shortcuts! { OPEN = ..., SAVE = ..., }` emitting both the consts and ALL_SHORTCUTS), so omission becomes impossible; or at minimum add a comment-anchored count assertion.

**[LOW · complexity] `crates/vgms-ui/src/app.rs:1494`** — gather_key_input duplicates the shift-variant-first shortcut consumption in both tab branches

The pack-tab branch (app.rs:1490-1505) and the editor branch (1526-1555) each re-state the same REDO_ALT-before-UNDO-before-REDO sequence, with the same subtle ordering comment ('egui ignores a surplus Shift') repeated twice. The constraint must be preserved in both places by hand; the pack branch also omits consuming SAVE_AS before SAVE, so Ctrl+Shift+S on the pack tab falls through to plain SAVE (PackSaveDocs) via the surplus-Shift rule -- harmless today, but evidence the duplication already drifted.

*Fix:* Extract a helper `consume_undo_redo(input, actions)` (and one for save/save-as) used by both branches, so the ordering constraint lives once next to its explanation.

**[LOW · consistency] `crates/vgms-ui/src/app.rs:1982`** — OptimizeVgm hand-rolls the document gate its siblings get from require_document, with a divergent message

Action::OptimizeVgm (app.rs:1981-1989) checks `!self.editor.has_document()` and sets APP_STATUS_OPEN_SONG_FIRST ('Please open a song first.') where every neighbouring arm calls self.require_document(), whose message is APP_STATUS_OPEN_FILE_FIRST ('Please open a file first.'). OpenFindLoop (1883) and start_loop_search (3763) also use the 'song first' string for the same nothing-open situation. Two nearly identical strings answer one condition depending on which arm you hit, and the gate logic exists in two shapes.

*Fix:* Use require_document() in all three places and delete APP_STATUS_OPEN_SONG_FIRST, or fold the wording into one constant if the distinction is not intentional.

**[LOW · confusing] `crates/vgms-ui/src/strings.rs:25`** — The DRO-mismatch alert is assembled by splicing a mid-word prefix ('...t' + 'here was')

APP_MISMATCH_PREFIX_TRIMMED is 'Despite auto-trimming, t' and APP_MISMATCH_PREFIX_PLAIN is 'T' (strings.rs:25-26); app_mismatch_body (193-200) concatenates '{prefix}here was a mismatch...'. The sentence is split inside the word 'There', so neither constant is readable or greppable on its own ('Despite auto-trimming, there' finds nothing), and the module's promise that wording is editable 'here rather than at call sites' quietly depends on noticing the splice.

*Fix:* Store two complete sentences (or a bool-selected full prefix 'Despite auto-trimming, there was...' / 'There was...') and drop the mid-word concatenation.

**[LOW · complexity] `crates/vgms-ui/src/editor.rs:636`** — optimize_vgm is a trivial public wrapper over optimize_vgm_document, whose doc implies a second arm that does not exist

pub fn optimize_vgm (editor.rs:636-638) does nothing but call optimize_vgm_document (580), whose doc opens 'The VGM-held document's half of [`Self::optimize_vgm`]' -- mirroring the genuine two-arm pattern of delete_selection/delete_vgm_selection, but here there is no DRO half and never a dispatch. The indirection plus the 'half' phrasing sends a reader hunting for the other implementation.

*Fix:* Inline optimize_vgm_document into optimize_vgm (keeping the public doc), or reword the private fn's doc to say it is the whole implementation.

**[LOW · idiom] `crates/vgms-ui/src/editor.rs:1006`** — Editor::row_analysis is pub but has no caller outside editor.rs

row_analysis (editor.rs:1006) is only invoked by row_cells three lines down (1093); a workspace-wide grep finds no other use (the table goes through row_cells, tests through row_cells_for_test). As a pub method on the crate's central type it advertises an API surface nothing consumes, and its cache-borrowing contract ('the two arms are the same call...') is an internal detail.

*Fix:* Make it private (fn row_analysis) so the compiler starts reporting if it ever truly loses its caller.

**[LOW · other] `crates/vgms-ui/src/app.rs:2882`** — preview_track instantiates a UI ChannelPanel just to compute a track's default panning

The pack preview builds `ChannelPanel::for_song(song).panning()` (app.rs:2880-2882) -- constructing a full mute/pan widget state object solely to read the song-type-derived default Panning, inside a non-UI code path. The knowledge 'what is this song type's original panning' is a synth/domain fact (the widget itself derives it from the song), so the widget is acting as a data holder here.

*Fix:* Expose the default as a plain function (e.g. Panning::original_for(song) in vgms-synth, or a free fn beside ChannelPanel that both it and preview_track call) and drop the widget construction.

### vgms-ui — pack mode and dialogs

**[MEDIUM · refactor] `crates/vgms-ui/src/dialogs/mod.rs:79`** — Ten modal dialogs copy-paste the same Cell<bool> footer-click scaffold

Because dialog_modal takes separate body and footer closures that cannot both borrow self mutably, every modal re-implements the same workaround: std::cell::Cell::new(false) flags for close/save/apply, a near-verbatim comment ("The body borrows `self` mutably, so the footer reports clicks through cells..."), deferred handling after the call, and a return expression of the shape `open && !(close.get() || saved)`. This appears in dro_info.rs:51-140, find_loop.rs:221-303, gd3_tag.rs:47-80, render_wav.rs:55-123, screenshot_rename.rs:134-212, settings.rs:194-245, split.rs:45-104, split_songs.rs:100-169, track_edit.rs:76-131, vgm_metadata.rs:132-208 — roughly 150 duplicated lines plus ten copies of the same comment.

*Fix:* Let the scaffold own the footer plumbing: e.g. have dialog_modal hand the footer closure a small &mut FooterClicks (or return a struct {closed, clicks: ...} / take declarative button specs) so each dialog's show() shrinks to body + a match on which button fired. One place then owns the `open && !close` logic instead of ten variations.

**[MEDIUM · complexity] `crates/vgms-ui/src/pack.rs:1269`** — pack.rs is a 3845-line god-file, and model code leaks into its own '-- view --' section

The file holds three parts: the headless model (PackState, PackTrack, transactions; lines 1-1268), the egui view (from the '-- view --' divider at 1269), and ~1140 lines of tests (2706-3845). The crate's own convention separates these (editor.rs is the headless Editor; its views live under widgets/). Worse, the file violates its own divider: retagged_bytes (1723), gd3_index (1734), BulkTagOverlay (1752) and seed_from_meta (1794) are pure model code sandwiched between view functions deck/lamp_colour (1643-1718) and field/meta-form helpers (1819+). vgms-core/src/pack.rs (2412 lines) keeps the wasm-clean shared model separate, which makes the ui-side blur the only remaining mix.

*Fix:* Split into a pack/ module: pack/state.rs (PackState, PackTrack, PackTransaction, PackMutation), pack/tags.rs (BulkTagOverlay, seed_from_meta, gd3_index), pack/view.rs (show, deck, track_table, screenshots, checklist), with tests beside the code they cover. At minimum, move the four model items out of the view section.

**[MEDIUM · consistency] `crates/vgms-ui/src/dialogs/bulk_tag.rs:67`** — BulkTagDialog hand-rolls the modal chrome that dialog_modal_sized already provides

bulk_tag.rs builds egui::Modal directly (line 67) and re-implements everything the shared scaffold does: heading + separator_clipped (69-71), a max_height'd ScrollArea with frame_scroll_output (74-101), a pinned dialog_footer (106-113), and the `!close && !modal.should_close()` return (116). Its geometry (width = area.width()*0.9 min 720, body_height = area.height()*0.9 - 150) closely mirrors dialog_modal_sized's modal_width/max_height derivation from ctx.content_rect(). Being the odd one out also forces Dialogs::show_all (mod.rs:291) to thread the `area` rect to this one modal while every other modal ignores it.

*Fix:* Port BulkTagDialog onto dialog_modal_sized(ctx, id, title, palette, 720.0, body, footer); the scaffold already handles the always-scrolls case via its measured-height logic. The intro label can be the first line of the body. Then drop the `area` parameter from its show() and the special case in show_all.

**[MEDIUM · maintainability] `crates/vgms-ui/src/dialogs/mod.rs:252`** — Dialogs' 15 slots must be updated in lockstep across the struct, any_open() and show_all() with nothing enforcing it

Adding a dialog requires touching three places: the Dialogs struct (219-244), the 15-arm boolean chain in any_open() (252-268), and the 15 retain() calls in show_all() (283-299). Nothing enforces agreement. Forgetting any_open() is the dangerous miss: app.rs:463 uses it to suppress editor keyboard shortcuts while a dialog is open, so a forgotten entry means Space/Delete/digit keys keep driving the editor underneath the new dialog — a silent behavioural bug rather than a compile error.

*Fix:* Generate the struct, any_open() and show_all() from a single macro listing (field name, type, show-call shape), or restructure so both methods iterate one canonical list (e.g. a method returning [&dyn ...] slots). Even a declarative macro_rules! with one field list removes the three-way lockstep.

*Verifier narrowed this:* The three-way lockstep is real (struct 220-243, 15-arm any_open 253-267, 15 retains 283-299) and the silent-miss risk is genuine — but the cited consumer is wrong: app.rs:463 is the e2e test snapshot field; the shortcut-suppression gate is gather_key_input at app.rs:1464-1473, whose comment confirms it deliberately does not use a blanket egui_wants_keyboard_input gate, so a forgotten any_open entry does let Space/Delete drive the editor behind a new dialog. Severity stays medium; correct the citation.

**[MEDIUM · confusing] `crates/vgms-ui/src/dialogs/find_reg.rs:305`** — VgmFind::query's doc contradicts the code: invalid hex silently deadens Find, instead of falling back to the dropdown

The doc comment (294-295) says "the hex box wins when it holds a valid address, else the register dropdown". The code says otherwise: for a non-empty hex box, `u16::from_str_radix(digits, 16).ok()?` (line 305) propagates None out of query() entirely, never reaching the dropdown path. In show() (147-162), a None query makes Find Next / Find Previous do nothing when clicked — no alert, no status, no fallback. A user who types a stray character into the hex box gets two buttons that silently stop working.

*Fix:* Pick one behaviour and make doc and code agree. Best UX: treat unparseable hex as an explicit invalid state (disable the Find buttons with an on_disabled_hover_text, or tint the field), rather than either silently ignoring the click or surprising the user with a dropdown-based search they didn't select.

**[LOW · dead-code] `crates/vgms-ui/src/pack.rs:1723`** — retagged_bytes is unused outside its own unit test and its doc comment is stale

pub fn retagged_bytes(song: &Song, ...) at pack.rs:1723 claims "Used by the quick-edit dialog to rewrite a track without loading it into the editor", but a workspace-wide grep finds no callers except its own test (pack.rs:3282, :3300) and a historical mention in docs/vgm-multichip-2026-07/HANDOVER.md. The quick-edit path now goes through PackTrack::retagged (app.rs:3016 handles Action::QuickEditSubmitted; bulk tag uses it at app.rs:3155). The function also duplicates the write-by-extension logic that PackTrack::retagged/write_vgm already own.

*Fix:* Delete retagged_bytes and its test (the equivalent behaviour is covered by the PackTrack::retagged tests, e.g. retagging_a_track_for_other_chips_to_vgz_compresses_it), or if the Song-based path is still wanted somewhere, fix the doc comment and move it next to PackTrack::retagged.

*Verifier narrowed this:* Dead code confirmed: workspace-wide grep finds no callers of retagged_bytes outside its own test (pack.rs:3282/3300) and docs/vgm-multichip-2026-07/HANDOVER.md history; the quick-edit and bulk-tag paths use PackTrack::retagged (app.rs:3016, :3155), so the doc comment is stale. Severity lowered to low: it is a ~10-line self-contained helper plus one test, no behavioural risk; the stale doc is the main harm.

**[LOW · consistency] `crates/vgms-ui/src/dialogs/render_wav.rs:155`** — 'Caption toggles the checkbox' is implemented four different ways

The same concept exists as: render_wav::option_row (155-166, checkbox left + clickable label), settings::checkbox_row (617-625, clickable caption left + checkbox right, for grids), an inline copy in split.rs (72-84), and another inline copy in screenshot_rename.rs (179-195). option_row's own doc even says "as the Settings rows do" while laying the row out in the opposite order. Four sites re-derive the sense(click) + manual toggle dance.

*Fix:* One shared helper in dialogs/mod.rs (or theme) taking caption, hover and &mut bool, with a variant or flag for grid (caption-first) vs row (checkbox-first) layout; replace the two inline copies and the two private helpers.

**[LOW · consistency] `crates/vgms-ui/src/pack.rs:1981`** — Three hand-rolled disclosure triangles, and the checklist's folded glyph contradicts the stated CP437 rationale

Disclosure widgets are hand-built three times: pack.rs hardware_fields (1584-1618, clickable label with summary), pack.rs disclosure() for the checklist (1980-1996, frameless button with widget_info), and settings.rs output_tab "All chips" (310-328, clickable label with count). The first and third use U+25BA with comments explaining "CP437 triangles ... so the DOS face has the glyph rather than a box"; the checklist's disclosure() uses U+25B6 (line 1981), which is not a CP437 code point — by the file's own stated rationale it risks rendering as tofu, and at minimum the folded-glyph choice diverges across three implementations. The accessibility treatment also diverges: only disclosure() sets an explicit widget name.

*Fix:* One theme::disclosure(ui, palette, open, label, summary: Option<&str>) helper carrying the CP437 glyph choice and the explicit accessible name; use it at all three sites. Verify whether U+25B6 actually renders in the bundled VGA face and standardise on whichever glyph does.

**[LOW · complexity] `crates/vgms-ui/src/pack.rs:728`** — readiness_items() runs at least twice per frame, cloning every track's GD3 tag each time

In pack mode, show() computes readiness_items() when the Tracks (line 1321) or Checklist (1329) section draws, and deck() computes it again every frame via readiness_summary -> validations -> readiness_items (1649, 809-825). Each pass calls track_facts() (685-698) which clones each track's full Gd3Tag (11 Strings), plus doc_stem/silent_chips string building and the core readiness() checks. Additionally track_tools() calls has_convertible_dates() and has_tag_renames() per frame (1425, 1435), the latter running vgm_ren character rewriting over every tagged track's title per frame. This is the same class of per-row-per-frame waste the file itself fixed for is_playable (documented 9 ms/frame incident at lines 99-105), just one tier up.

*Fix:* Compute readiness_items once per frame at the top of show() and pass it to both the section body and deck() (deck already receives &mut PackState; pass the items or a summary alongside), or cache the Vec<ReadinessItem> keyed on a change counter bumped by edits/rescans.

**[LOW · complexity] `crates/vgms-ui/src/dialogs/find_loop.rs:107`** — FindLoopDialog stores duplicate copies of LoopSearchDoc facts under second names

FindLoopDialog holds doc: LoopSearchDoc yet also copies total_commands (line 107, mirroring doc.total_commands with the identical doc comment) and is_vgm (109, mirroring doc.can_store_loop — the same fact under two names), plus commands_per_sec which is a pure derivation of doc.total_commands/doc.total_secs. The copies exist so tests can poke them (searching_emits_a_command_count_from_the_seconds mutates commands_per_sec/total_commands directly), but nothing stops the copies drifting from doc if either is ever updated after construction.

*Fix:* Drop total_commands and is_vgm in favour of self.doc.total_commands / self.doc.can_store_loop (keeping one name for the store-a-loop fact). Keep commands_per_sec if the test seam is wanted, or have tests build a synthetic LoopSearchDoc instead.

**[LOW · refactor] `crates/vgms-ui/src/dialogs/split_songs.rs:262`** — fmt_time is duplicated between find_loop.rs and split_songs.rs

split_songs.rs:262-267 and find_loop.rs:398-403 both format an M:SS.s time with the identical floor/remainder logic; find_loop's ms version is exactly split_songs' fmt_time(native, 1000). This is a third and fourth time-formatting routine in the workspace next to vgms_core::util::ms_to_timestr (MM:SS) and vgms_core::pack::format_track_time (M:SS from samples), but these two are byte-identical in intent and format.

*Fix:* Move one fmt_time(native: u32, rate: u32) -> String into dialogs/mod.rs (or vgms_core::util beside ms_to_timestr) and call it from both dialogs, with find_loop passing rate = 1000.

**[LOW · consistency] `crates/vgms-ui/src/dialogs/settings.rs:556`** — Settings implements its preview-once lifecycle two different ways in one file

The skin preview (preview(), line 556) detects change by comparing against an opened_with snapshot the caller must capture at the top of show() (line 196) and pass back in, and needs no stored state. The cores and resampling previews (preview_cores 534, preview_resampling 252) instead store previewed_cores / previewed_resampling fields updated on emit. Both implement the identical audition-once / revert-on-close / silent-on-save contract (the doc comments cross-reference each other as "analogues"), but a reader must understand two mechanisms, and the skin one couples show()'s ordering (snapshot before body mutation) to correctness.

*Fix:* Unify on the stored previewed_* pattern (add a previewed_skin: Skin field), which removes the opened_with parameter threading and makes the three methods textually parallel — potentially one generic helper over (current, original, previewed) -> Option<Action>.

**[LOW · terminology] `crates/vgms-ui/src/pack.rs:2650`** — Action::OptimizeImage names the job the UI deliberately renamed 'Recompress'; optimise/optimize also split across identifiers

The comment at pack.rs:2643-2645 explains the button says "Recompress", not "Optimize", because the deck's Optimize pad is the vgm_cmp step and "two different jobs must not share one word on the same screen" — yet the code keeps exactly that collision: the button pushes Action::OptimizeImage (2650) and pack state uses optimize_on_export for the VGM job, so in code the one word still names both jobs. Separately, identifier spelling is split within the crate: module optimise.rs and crate::optimise::credit() (British) versus optimize_on_export, optimize_vgms, OptimizeImage, fn optimize_image (American).

*Fix:* Rename Action::OptimizeImage -> Action::RecompressImage (and app.rs optimize_image -> recompress_image) so code matches the UI's own distinction; pick one identifier spelling (the codebase is majority 'optimize' in identifiers, so renaming the optimise module is the smaller change).

**[LOW · idiom] `crates/vgms-ui/src/pack.rs:501`** — Hand-rolled singular/plural suffixes repeated at five sites in pack.rs

The `if n == 1 { "" } else { "s" }` pluralisation is written out at lines 501-503 (volume-modifier label), 569-572 (date conversion label), 625-628 (rename label), inside the count closure at 926, and 1953-1956 (collapsed checklist item count). Five copies of the same two-token idiom, four of them inside format! calls that read poorly.

*Fix:* One small helper, e.g. fn plural(n: usize, word: &str) -> String or fn s(n: usize) -> &'static str, in strings.rs (which already owns interpolating message helpers), used by all five sites.

**[LOW · idiom] `crates/vgms-ui/src/pack.rs:1475`** — The preset picker uses a String sentinel plus a re-search instead of acting on the click

meta_form's preset dropdown (1473-1500) creates `let mut picked = String::new()`, feeds it to selectable_value for every preset, then linearly re-searches PRESETS.chain(CONSOLE_PRESETS) by name to find which preset was picked. The selectable_value response already says whether that row was clicked; the sentinel + find round-trip allocates a String per preset per open frame and would silently break if two presets ever shared a name.

*Fix:* Inside show_ui, check `ui.selectable_label(false, preset.name).clicked()` per preset (or capture Option<&PackPreset> from the response) and apply the fields directly, removing the sentinel and the second lookup.

### vgms-ui — theme engine and widgets

**[MEDIUM · confusing] `crates/vgms-ui/src/theme/palette.rs:622`** — Stale colour comments in NAVY/CREAM/VERDIGRIS describe the pre-complementary-ink values

The display inks were deliberately flipped to complementary colours (tabs.rs module doc: 'gold on navy ... cyan on cream'), but the case-table comments still describe the old matching colours and now contradict the literals beside them. NAVY line 622: '// Ice blue, to sit with the navy plate.' precedes data_text 0xF2C766 (warm gold); line 627: 'Ice-blue wave on a deep navy screen' while wf_wave = data_text = gold. CREAM lines 674-675: 'Warm amber, matching the cream plate.' precedes data_text 0x74CFE6 (cyan); line 680: 'Warm amber wave ... cool cursor' is inverted twice (wave is cyan, cursor 0xF2C766 is warm amber). VERDIGRIS line 726: 'Pale mint, matching the patina plate.' precedes data_text 0xF0A878 (warm peach); line 731: 'Mint wave' likewise. Anyone tuning a case from these comments will pick the wrong hue family.

*Fix:* Rewrite the six comments to describe the complementary-ink scheme actually in the code (gold on navy, cyan on cream, peach on verdigris), or drop the per-colour prose and state the complementary rule once above the Bassoon section.

*Verifier narrowed this:* Every cited mismatch verified: NAVY 'Ice blue' over data_text 0xF2C766 (gold) and 'Ice-blue wave / warm cursor' inverted (wf_wave=data_text=gold, wf_cursor 0x7CD8E0 is the cool one); CREAM 'Warm amber' over 0x74CFE6 (cyan) with wave/cursor swapped; VERDIGRIS 'Pale mint'/'Mint wave' over 0xF0A878 (peach). compose() maps wf_wave=data_text (palette.rs:400) and tabs.rs:6-7 confirms the complementary flip was deliberate. Severity lowered to low: comments only, no functional effect, and a mis-tuned hue would be visible in the per-theme showcase snapshots.

**[MEDIUM · refactor] `crates/vgms-ui/src/widgets/pan_knob.rs:143`** — show() and show_spread() duplicate the drag-memory, detent, and reset-gesture logic

Lines 97-126 (pan) and 146-170 (spread) are the same interaction skeleton written twice: on drag_started insert_temp the seed, on dragged get_temp/apply delta/insert_temp/snap-through-detent/mark_changed, on drag_stopped remove the temp, then the identical (double_clicked || secondary_clicked) recentre block. Only the value type (u8 with SNAP_BAND vs f32 with SPREAD_SNAP), the delta sign convention, and the centre value differ. Two copies of subtle egui data_mut plumbing invite drift — e.g. a fix to the seed fallback or the detent escape in one knob silently not applied to the other.

*Fix:* Extract a helper like fn drag_raw(ui, response, seed: f32, apply: impl Fn(f32, Vec2) -> f32) -> Option<f32> plus a reset_on_click helper, leaving show/show_spread as thin adapters that convert to/from their value domain and snap.

**[MEDIUM · refactor] `crates/vgms-ui/src/widgets/channels.rs:366`** — The PanModeResponse application block is duplicated between the OPL and generic panels

channels.rs pan_row lines 366-388 and chip_channels.rs mode_controls lines 244-268 are the same ~20 lines: copy self.custom/self.spread into locals, call pan_controls::mode_controls, then apply the three flags identically (mode_toggled -> write back custom + changed; spread_changed -> self.set_spread(spread) + changed; reset -> changed |= self.reset_pans()). pan_controls.rs exists precisely so the two panels do not drift ('one design ... rather than a second implementation that drifts', its module doc), but only the *drawing* half was shared; the application half is copy-pasted. A related small drift already exists: under Original mode the OPL panel shows inert knobs via a throwaway copy of default_pans (channels.rs:352-356) while the generic panel passes the live pans array disabled (chip_channels.rs:206-215) — the same concept, two idioms.

*Fix:* Move the application into pan_controls — e.g. mode_controls takes &mut custom/&mut spread plus set_spread/reset callbacks and returns just 'changed', or define a tiny SpreadPanel trait (set_spread, reset_pans, set_custom) both panels implement — so a future mode gains behaviour in one place.

**[LOW · bug-risk] `crates/vgms-ui/src/theme/fonts.rs:73`** — install_cjk_fallback's already-registered guard can never fire on wasm, its only caller

The doc promises 'A no-op if a CJK fallback is already registered (a second fetch, or a native system font)'. The guard checks defs.font_data.contains_key(CJK_FAMILY_NAME) on a *freshly built* FontDefinitions from font_definitions(), which only contains a CJK entry when system_cjk_font() found one on disk — always None on wasm32 (line 107-110). The sole caller is the web shell (crates/vgms-web/src/runner.rs:77). So on the only platform that calls this function the guard is永 false: a second call with valid bytes re-inserts the font and rebuilds the whole font atlas via ctx.set_fonts, contradicting the comment. The guard would only trip on native, where nothing calls the function. Today only one fetch is spawned, so it is latent, but the documented idempotence is not real.

*Fix:* Track installation in the place that actually persists across calls — e.g. a static AtomicBool set on the first successful install, or read ctx.fonts to see whether the family already resolves — and fix or drop the misleading doc sentence.

**[LOW · refactor] `crates/vgms-ui/src/theme/bevel.rs:190`** — The four pad widgets repeat the allocate/widget_info/PadState/paint skeleton

button_impl (190-210), icon_button_sized (221-244), toggle_impl (294-317) and icon_toggle (320-341) each hand-roll the same frame: allocate_exact_size, widget_info, is_rect_visible, build a PadState, paint_pad, then paint content nudged by ink.offset. The momentary 'lit only while held' comment+logic is duplicated in the two buttons, and the toggle 'preview the outcome' comment+logic (lit: if held { !on } else { on }) is duplicated in the two toggles. A change to the press-preview behaviour or the accessibility reporting must be made in four (comment: eight) places.

*Fix:* Fold into two cores — pad_button(ui, palette, size, label, paint: impl FnOnce(&Painter, Rect, PadInk)) and pad_toggle(...) — with the text/icon variants as one-line wrappers supplying the content painter.

*Verifier narrowed this:* The allocate/PadState/paint_pad/content-nudge skeleton does repeat across the four widgets and the two lit-computation comments are each duplicated once, but the claim overstates the blast radius: interact_toggle (bevel.rs:257-274) already centralises toggle click handling AND accessibility reporting, so widget_info lives in three places (not four), a press-preview change touches two toggle sites, and momentary-lit two button sites. Real but thinner than described; severity stays low.

**[LOW · bug-risk] `crates/vgms-ui/src/widgets/chip_channels.rs:30`** — pan_to_i16 is asymmetric: 0xFF maps to +254, never reaching libvgm's +0x100 full right

(i16::from(byte) - 128) * 2 sends 0x00 to exactly -0x100 (hard left) but 0xFF to +254, 2/256 short of +0x100. Everything else in the pan pipeline scales the two sides independently around the 0x80 anchor — dot_angle (pan_knob.rs:67-74, '128 steps left, 127 right') and strings::pan_knob_readout both report 0xFF as R100 — so a knob the UI declares 'hard right' delivers a not-quite-hard-right position to libvgm's SetPanning, leaving a residue in the left channel under a constant-power law, asymmetrically with hard left.

*Fix:* Scale the right side by 256/127 like dot_angle does (Ordering::Greater => (byte - 128) * 256 / 127), or special-case 0xFF -> 0x100, and add a unit test asserting pan_to_i16(0x00) == -0x100 and pan_to_i16(0xFF) == 0x100.

**[LOW · confusing] `crates/vgms-ui/src/widgets/table.rs:3`** — Module doc says 'Six columns' but the table builds five; header height literal duplicated

The doc comment ('Six columns.') predates folding the 'all register options' column into the Description cell's hover text (the comment at lines 105-106 records the fold); the builder now defines five columns (lines 65-69). Additionally header_height = row_height + 2.0 is computed at line 59 for the scrollbar-frame math at line 135, but the .header() call at line 77 repeats the raw expression row_height + 2.0 instead of using the variable — the two must stay in lockstep or the hand-framed scrollbar channel (lines 135-141) misaligns with the real header.

*Fix:* Change the doc to 'Five columns' and pass header_height to .header() so the frame math cannot drift from the actual header.

**[LOW · consistency] `crates/vgms-ui/src/widgets/boost_stepper.rs:127`** — Hand-rolled dark-well restyle duplicates theme::style_dropdown; third variant in loop_stepper

boost_stepper.rs lines 125-135 loop over widgets.inactive/hovered/active setting weak_bg_fill/bg_fill to data_bg and fg_stroke to a data ink — the same body as theme::style_dropdown (theme/mod.rs:52-64) minus the 'open' state and with data_text instead of data_label. loop_stepper.rs lines 43-50 render the same 'value in a sunken dark well' concept a third way (manual rect_filled + paint_bevel + Label). Three implementations of one visual concept means a change to the well treatment (e.g. adding the sunken bevel the loop stepper has but the boost value lacks) touches scattered sites.

*Fix:* Add a shared helper in theme (e.g. style_data_field(ui, palette, ink: Color32) generalising style_dropdown) and use it in boost_stepper; consider a small well_label helper for the loop stepper's painted readout.

**[LOW · idiom] `crates/vgms-ui/src/widgets/mod.rs:1`** — The widgets module tree is fully pub but never consumed outside the crate

lib.rs exports pub mod widgets and each of the 13 submodules is pub, yet a workspace grep shows no external consumer: vgms-app and vgms-web reach vgms-ui only through theme::install/install_cjk_fallback, VgmStudioApp, and the platform/tasks traits. Public items here (PositionPanel accessors, ChipPanels internals, pan_controls constants, chip_output::plan, ...) therefore present an unintended API surface: rustc's dead_code lint cannot flag any of them if they fall out of use (which is exactly how the dead palette roles survived in theme), and semver-invisible internals read as supported API.

*Fix:* Make widgets pub(crate) (and let the compiler then report any genuinely unused pub items inside), keeping only what the shells actually import public.

*Checked and dismissed here:* Nine palette roles (button_* and bevel_border) have no production consumer; Bevel::Raised has no production call site.

### Test suites — GUI tests and web e2e

**[MEDIUM · refactor] `crates/vgms-ui/src/app_gui_tests.rs:1`** — 7,399-line single test file should be split into a test module directory

The whole headless GUI suite lives in one file with clear internal section banners (interaction, channel panning, render to WAV, split channels, split songs, snapshot tests, pack mode, loop points, find loop, format-specific menus, Optimize VGM). It is mounted via `#[path = "app_gui_tests.rs"] mod gui_tests;` at crates/vgms-ui/src/app.rs:4751 specifically so tests can read VgmStudioApp's private fields. Because private-field visibility extends to all descendants of `app`, the same trick works for a directory: `#[path = "app_gui_tests/mod.rs"]` with per-section child modules (harness+fixtures in mod.rs or a `support` module, then `panning.rs`, `pack.rs`, `loops.rs`, `snapshots.rs`, ...). At its current size, finding the right section, avoiding fixture duplication (see the put_u32 finding), and reviewing diffs are all measurably harder than they need to be.

*Fix:* Convert app_gui_tests.rs into app_gui_tests/mod.rs + one child module per existing section banner; keep build/build_sized, act, pack_section and the fixture builders in a shared support module. Purely mechanical; no visibility changes needed.

**[MEDIUM · complexity] `crates/vgms-ui/src/app_gui_tests.rs:2067`** — VGM-header fixture boilerplate (incl. `fn put_u32`) is hand-rolled six times

A nested `fn put_u32(bytes: &mut [u8], at: usize, value: u32)` is redefined verbatim six times (lines 2067, 2137, 2780, 3097, 3145, 3188), and around each copy the same VGM skeleton is rebuilt by hand: 0x100 zeroed header, `b"Vgm "`, version at 0x08, data offset at 0x34, chip clocks via clock_offset(), total samples at 0x18, stream append, EOF backfill at 0x04. The builders differ only in version, chip list, totals/loop fields and stream bytes (other_chip_vgm_bytes, generic_vgm_file, sms_vgm_file, the inline fixtures in a_non_opl_document_can_be_optimised and a_chip_the_built_in_pass_cannot_touch_is_optimised_in_the_editor, and non_opl_looping_vgm). Any future header-layout tweak (e.g. a new version) must be repeated six times.

*Fix:* One parameterised builder, e.g. `fn raw_vgm(version: u32, chips: &[(ChipKind, u32)], total: u32, loop_: Option<(u32, u32)>, stream: &[u8]) -> Vec<u8>`, with the six fixtures as thin wrappers. Kills all six put_u32 copies at the same time.

**[MEDIUM · maintainability] `crates/vgms-ui/src/app_gui_tests.rs:4206`** — Pack rename-failure path and pack redo are scriptable but never tested

FileLog.rename_outcomes is a VecDeque<Result<(), String>> (test_support.rs:53), yet every GUI test only ever pushes Ok(()) — `grep 'rename_outcomes.push_back(Err'` finds nothing. The app has real failed-rename handling (app.rs:332 defers the quick-edit rewrite until the rename lands "so a failed rename ...", app.rs:3084) and the reorder batch uses a temp-then-final dance that can strand `.vgmstudio*` temp names mid-batch — the single riskiest file operation in pack mode, with zero coverage of its failure/recovery behaviour. Similarly, redo_pack_edit (app.rs:2641, reachable via handle_action at app.rs:1845) is never driven: reordering_renumbers_files_and_is_undoable_and_redoable asserts `pack_redo.len() == 1` (line 4256) and stops, despite the test name promising "redoable".

*Fix:* Add a test that feeds an Err into rename_outcomes mid-reorder and pins what the user sees (status/alert) and what state the folder+undo stack are left in; extend the reorder test (or add one) to actually dispatch the pack Redo and assert the order returns.

**[LOW · consistency] `web/e2e/tests/pack.spec.js:12`** — pack.spec.js is documented as Chromium-only but runs (unfiltered) on Firefox too

The spec header and the comment at line 12 frame this file as the wt-7 "Chromium (File System Access)" proof, and web/e2e/README.md's Browser matrix says "Chromium gets the wt-7 File System Access pack proofs; Firefox is the non-Chromium proof for wt-8's zip packs". But playwright.config.js defines both projects over the same testDir with no testIgnore/grep, and pack.spec.js has no `test.skip(({ browserName }) => ...)` — so these specs also execute on Firefox, where the `__vgms_pick_dir` OPFS shim (pack_fs.js:63) plus the no-move() copy+delete fallback (pack_fs.js:185) make them pass. The executed matrix and the documented matrix disagree: a Firefox-only OPFS regression would surface in a spec labelled Chromium, and the suite silently runs ~double the pack tests the docs claim.

*Fix:* Pick one: scope the file to Chromium (per-project testIgnore in playwright.config.js, or a browserName skip at the top of the spec) to match the docs, or update the spec header and README to say the OPFS-shimmed pack path is deliberately proven on both engines.

*Verifier narrowed this:* Facts all verified: no testIgnore/grep in playwright.config.js, no browserName skip in pack.spec.js, README's matrix assigns the wt-7 pack proofs to Chromium, and the OPFS shim (crates/vgms-web/pack_fs.js:63, no-move fallback at 185) lets the specs pass on Firefox. But the consequence is extra passing coverage plus a stale label/doc — a Firefox OPFS regression would still surface, just in a mislabelled spec. Documentation-consistency issue with a one-line fix: low, not medium.

**[LOW · confusing] `crates/vgms-ui/src/app_gui_tests.rs:7`** — Header claims snapshot tests sit "at the bottom"; they are interleaved throughout

The module doc (lines 6-9) says "The snapshot tests at the bottom need the wgpu renderer", and a `-- snapshot tests --` section banner sits at line 1976. In reality snapshot tests are scattered across the back two-thirds of the file, interleaved with interaction tests: snapshot_settings_output_per_chip (1995) is followed by dozens of non-snapshot tests (2196 onward), and further snapshots appear at 2432, 2972, 3353-3390, 5716-5815, 6334-6436, 6902 and 7053. Both the header and the banner now mislead a reader looking for "the wgpu tests".

*Fix:* Either regroup the snapshot tests (easy if the file is split into modules — a snapshots.rs child) or reword the header/banner to say snapshot tests are placed beside the feature they render, marked by the `wgpu: true` build flag.

**[LOW · complexity] `crates/vgms-ui/src/app_gui_tests.rs:539`** — Three pan tests hand-roll the drag sequence that the later drag_by helper wraps

pan_knob_drag_sends_custom_panning_without_resending_muting (539-543), pan_knob_drag_up_pans_left_like_dragging_left (570-574) and spread_knob_spreads_the_pans_and_engages_custom (638-643) each spell out the identical press/run/hover/run/drop/run six-step dance that `drag_by` (line 2240) encapsulates; the grip drag at 4277-4282 re-rolls it once more with the same shape. The helper was evidently added after these tests and they were never folded onto it.

*Fix:* Replace the four hand-rolled sequences with drag_by(&mut harness, center, delta) and move drag_by up with the other harness helpers.

**[LOW · complexity] `crates/vgms-ui/src/app_gui_tests.rs:1024`** — The `for _ in 0..4 { harness.step(); }` inline-task settle idiom is repeated six times

Lines 1024, 1112, 1157, 1209, 3580 and 3606 all step exactly four frames to let an InlineTaskService result travel submit -> pending -> poll -> apply, and most repeat a variant of the same explanatory comment ("the inline scan finishes on submit, but its Peak lands in `pending` and is delivered by a later frame's poll..."). The magic count 4 and the rationale are maintained in six places; if the delivery pipeline ever gains a frame, six call sites break.

*Fix:* Add `fn settle_inline_tasks(harness: &mut Harness<VgmStudioApp>)` next to build(), carrying the comment (and the frame count) once.

**[LOW · confusing] `crates/vgms-ui/src/app_gui_tests.rs:6442`** — File-wide helpers `act` and `pack_section` are buried under the "loop points" banner

`act` (line 6442) is first used at line 208 and `pack_section` (line 6455) at line 3941, but both are defined 2,000-6,000 lines after first use, under the `-- loop points --` section banner (6439) that has nothing to do with them. pack_section additionally carries the crucial run-twice rationale (egui hit-tests against the previous frame's rects) that anyone writing a new pack test needs to find. Rust tolerates the ordering, but a reader hunting for what `act` does has no reason to look inside the loop-points section.

*Fix:* Move act, pack_section (and drag_by) up beside build/build_sized/empty_harness so all harness plumbing lives in one place.

**[LOW · refactor] `crates/vgms-ui/src/app_gui_tests.rs:63`** — build()'s two positional bools are opaque enough that a call site decodes them in a comment

`build(initial, inline_tasks, wgpu)` is called ~50 times as e.g. `build(Some(picked(&song)), true, false)`; the flags are unreadable at the call site, to the point that line 1016 carries "// build(initial, inline_tasks, wgpu): inline runs the scan synchronously" purely to decode the signature. The file already grows purpose-named wrappers where the pattern recurs (empty_harness, harness_with_song, tall_pack_harness), which confirms the raw signature is the wrong interface.

*Fix:* Take a small options struct (`HarnessOptions { inline_tasks: bool, wgpu: bool, size: Vec2 }` with Default) or add two more named constructors (inline_harness(song), snapshot_harness(song)) and keep raw build() private to them.

**[LOW · dead-code] `crates/vgms-ui/src/app_gui_tests.rs:1999`** — Redundant install_test_cores() call — build_sized() already installs on every harness

snapshot_settings_output_per_chip calls `crate::widgets::chip_output::install_test_cores()` at line 1999 with a three-line justification, immediately before `build(...)` — but every harness path runs through build_sized(), which makes the same call at line 81 with its own (newer, more general) justification. The local call and comment are leftovers from before the installation was centralised, and they invite the next author to cargo-cult the same redundant call into new snapshot tests.

*Fix:* Delete the call and comment at 1999; the build() call two lines below already guarantees the registry.

**[LOW · confusing] `crates/vgms-ui/src/app_gui_tests.rs:2073`** — Assertion message contains an accidental run of ten spaces mid-sentence

The guard inside other_chip_vgm_bytes reads: "the YM2610 now has a core, so this fixture no longer stands for a          document nothing can play -- pick a chip that still has none" — a string-wrap artifact left the indentation embedded in the literal, so the (deliberately loud, fixture-naming) failure message prints with a gap in the middle.

*Fix:* Rejoin the literal with a single space, or use concat!/a `\` line continuation as the other long messages in this file do.

**[LOW · confusing] `web/e2e/tests/helpers.js:44`** — seedPackFolder doc claims it generates the 1x1 PNG; that lives in pngBytes

The JSDoc says "A fresh valid 1x1 PNG is generated in-page so screenshot entries decode", but seedPackFolder only writes the byte arrays it is given; the PNG generation is the separate exported helper pngBytes(), which the caller (pack.spec.js openSeededPack, line 25) must invoke and pass in explicitly. Someone seeding a pack with a screenshot from this doc alone would expect the PNG to appear for free and get a non-decoding entry.

*Fix:* Move that sentence to pngBytes' doc (or reword: "pair with pngBytes() for a screenshot entry that decodes").

**[LOW · refactor] `web/e2e/tests/playback.spec.js:14`** — openFixture and the download-collection idiom are re-rolled per spec instead of shared

playback.spec.js lines 13-17 re-implement inline exactly the openFixture() flow that file-ops.spec.js defines at lines 11-18 (waitForEvent filechooser -> dispatch OpenFile -> setFiles -> poll hasDocument). Separately, the download-byte collection idiom (waitForEvent download -> createReadStream -> for-await chunk loop -> Buffer.concat) appears three times: file-ops.spec.js 33-42, pack.spec.js 78-88, zip-pack.spec.js 60-73. helpers.js is the established home for exactly this kind of flow (boot, dispatch, state, seedPackFolder) but these two never moved there.

*Fix:* Add `openFixture(page)` and `downloadBytes(page, trigger)` (returns a Buffer) to helpers.js and use them from all four specs.

**[LOW · bug-risk] `web/e2e/serve.mjs:40`** — Path-escape check uses startsWith without a trailing separator

`if (!filePath.startsWith(DIST))` admits any sibling directory sharing the prefix: with DIST = .../target/web-dist, a request normalising to .../target/web-dist-old/... or .../target/web-dist2/... passes the check. Traversal upward (../) is caught by normalize, but the prefix comparison is the textbook off-by-a-separator. Impact is small — a dependency-free test server bound to 127.0.0.1 serving build output — but the fix is one token.

*Fix:* Compare against the directory boundary: `if (filePath !== DIST && !filePath.startsWith(DIST + sep))` (import sep from node:path).

*Checked and dismissed here:* Test names mix 'optimised' and 'optimizing' for the same OptimizeVgm action.

### vgms-app — CLI, services, parity harness

**[MEDIUM · bug-risk] `crates/vgms-app/src/parity/reference.rs:266`** — stage() never refreshes an already-staged player, so an upgraded reference is silently ignored

stage() copies the player and its neighbours into work_dir/player only `if !to.exists()` (line 266-268), and the work dir (std::env::temp_dir()/vgmstudio-parity) persists across runs. Replacing the reference player binary at the same VGMSTUDIO_REF_PLAYER path therefore keeps running the OLD staged executable, while describe() (line 137-155) reads the size of the NEW source file -- the run record misrepresents what actually rendered. Only the pinned config is rewritten unconditionally (line 270-277). For a harness whose whole design is 'pin what the reference was', a silently stale executable is the exact failure it exists to prevent.

*Fix:* Refresh the staged copy when the source differs: compare file length and mtime before skipping (or always re-copy the executable, which is one file), and/or make describe() report the staged file it actually runs.

**[MEDIUM · bug-risk] `crates/vgms-app/src/parity/reference.rs:290`** — Render cache key omits the config, args and player identity, so config changes serve stale WAVs

cached_path (lines 290-298) keys a cached reference render by input name, size and rate only. The pinned VGMPlay.ini carries the per-chip core selection, loop count and fade -- 'everything that decides what the reference is' per this module's own docs (line 37-40) -- and VGMSTUDIO_REF_ARGS/the player binary are likewise invisible to the key. Switching the reference's YM2612 core in the pinned ini, or swapping player builds, silently serves WAVs rendered under the old configuration, and every downstream score is then measured against the wrong reference. The doc comment on cached_path only justifies name+size against corpus collisions, not against configuration changes.

*Fix:* Fold a digest of the pinned config text, the extra args and the player's size/mtime into the cache file name (or into a per-configuration cache subdirectory), so any change to what the reference is invalidates the cache.

**[MEDIUM · consistency] `crates/vgms-app/src/services/audio.rs:93`** — NativeAudioService never surfaces mid-song stream errors although AudioService::last_error exists for exactly that

AudioService::last_error's doc (vgms-ui/src/platform.rs:501-507) says it exists 'for faults that happen away from a call the app made -- a device unplugged mid-song'. The native backend has precisely that fault class: cpal's error callback in vgms-audio-native/src/lib.rs:561 only does `log::error!("audio output stream error: ...")`, and NativeAudioService inherits the defaulted `last_error -> None`. Meanwhile RetroWaveAudioService implements it (services/retrowave.rs:198-200) and surfaces its pump-thread errors to the UI. Same fault class, two treatments -- and this is the same seam where a defaulted method already shipped one real bug (the set_chip_muting story documented on the trait).

*Fix:* Have NativeAudio store the cpal error-callback message in a take-once slot (mirroring RetroWaveAudio::take_error) and implement last_error on NativeAudioService to drain it; or add a comment on the impl stating why native deliberately keeps the default.

**[LOW · refactor] `crates/vgms-app/src/lib.rs:16`** — Parity harness and corpus indexer (~2.2k lines + hound dep) ship in the app crate but are test-only

`pub mod parity` (lib.rs:16) and `pub mod corpus` (lib.rs:14) are consumed exclusively by integration tests (tests/reference_parity.rs, oracle_lle.rs, chip_index.rs, core_audio.rs, engine_corpus.rs) -- a workspace-wide grep finds no use from the GUI, CLI, or web. They are compiled into the shipping GPL binary's lib on every build, and `hound` (Cargo.toml:24, comment: "Reads the reference player's WAV output for the parity harness") is a hard dependency of the app purely for them. The area guidance explicitly asks whether this belongs in the shipping crate.

*Fix:* Move parity/ and corpus.rs into a dev-only crate (e.g. crates/vgms-parity) that vgms-app lists under [dev-dependencies]; the integration tests can import it directly and hound leaves the shipping dependency set. Alternatively gate both modules behind a `parity` feature with `required-features` on the relevant [[test]] targets.

*Verifier narrowed this:* Facts verified: parity/corpus are consumed only by vgms-app's integration tests, and hound exists solely for parity. But the severity overstates the cost: PARITY-PLAN.md section 3 deliberately planned the harness into crates/vgms-app/src/parity.rs, unreferenced lib code is stripped from the shipped exe by the linker, and hound is a tiny pure-Rust dep, so the real cost is dependency surface and build hygiene, not shipped-binary weight. Valid restructuring suggestion at low severity.

**[LOW · dead-code] `crates/vgms-app/src/parity/mod.rs:340`** — Threshold::max_envelope is always None and never read -- a bar that looks enforced but is not

Every THRESHOLDS row sets max_envelope: None (lines 361, 372, 390) and no code reads the field: the scorecard test (tests/reference_parity.rs, every_cored_chip_matches_the_reference_within_its_band) asserts only min_correlation, max_dropout and max_cents. Yet the field's doc (lines 333-340) presents it as 'the metric that arbitrates chips whose waveform phase is implementation-defined', citing the HuC6280. A reader of the threshold table reasonably believes envelope error is part of the bar; it is not.

*Fix:* Either wire max_envelope into the scorecard test's trouble checks (envelope_error is already computed per Score), or delete the field and move its HuC6280 rationale to PARITY-PLAN.md as future work.

*Verifier narrowed this:* Dead-field claim verified: workspace grep finds max_envelope only at its definition and three None assignments; the scorecard's trouble checks test only correlation/dropout/cents. But the 'looks enforced' framing is overstated: every row is None and the field doc says 'where a bar has been measured', so the table reads as no-envelope-bar-yet; the real trap (a future Some() silently unenforced) is latent. Low, not medium.

**[LOW · confusing] `crates/vgms-app/src/corpus.rs:219`** — cache_path's doc promises a target/ fallback that the code does not implement

The doc (lines 217-218) reads 'Where the cache lives: beside the corpus if that is writable, else under `target/`', but the body is unconditionally `root.join("vgmstudio-chip-index.tsv")`. No caller implements the fallback either (tests/chip_index.rs:28 uses it directly). On a read-only corpus root (e.g. a network share), save() fails, open_or_build logs a warning, and every run silently re-walks tens of thousands of headers -- the doc's promised escape hatch does not exist.

*Fix:* Either implement the fallback (probe writability, else derive a path under the workspace target/ or a temp dir) or fix the doc to state the cache only ever lives beside the corpus.

*Verifier narrowed this:* Doc/code mismatch verified: cache_path's doc promises a target/ fallback and the body is unconditionally root.join(...), with no caller implementing it. But this is ignored-test tooling whose worst case is a re-walk plus a log::warn on a read-only corpus; a one-line doc fix or small fallback resolves it. Low, not medium.

**[LOW · dead-code] `crates/vgms-app/src/parity/mod.rs:316`** — Regime::CleanRoom is never constructed since the clean-room cores were deleted

The comment at lines 377-379 says 'Only shared-lineage rows remain', and no THRESHOLDS entry (nor any other code in the workspace) constructs Regime::CleanRoom -- the clean-room cores programme was retired and libvgm (shared-lineage with VGMPlay) is now the default everywhere. The variant survives only in a test branch that can never fire (mod.rs:432) and in the module doc (lines 16-18), which still presents two live regimes in the present tense.

*Fix:* Delete the CleanRoom variant, its dead test branch, and rewrite the module-doc regime summary as history ('the clean-room regime applied while those cores existed'), or explicitly mark the variant as retained for future non-shared references.

**[LOW · dead-code] `crates/vgms-app/src/parity/metrics.rs:307`** — dominant_period and cents_error are unused outside their own unit tests

A workspace-wide grep shows `dominant_period` (line 307) and `cents_error` (line 380) are called only from metrics.rs's own #[cfg(test)] module. The live pitch path is `detune_cents`, whose doc (lines 408-420) explains why it superseded absolute-period comparison (per-frame resolution too coarse; polyphony). The module's headline table (line 21) still credits [`cents_error`] with catching the AY bug, pointing readers at a function nothing calls.

*Fix:* Delete both functions and their tests (detune_cents carries the cents unit itself), or keep dominant_period only if a planned metric needs it and say so; update the doc table row to name detune_cents.

**[LOW · bug-risk] `crates/vgms-app/src/services/pack.rs:137`** — NativePackService::cancel does not bump the generation, unlike ThreadTaskService's cancel

PackService::cancel (lines 137-141) only sets the cancel flag. A job that finished and queued its outcome just before a bare cancel() still passes poll()'s `generation == latest` filter and is delivered afterwards. ThreadTaskService::Slot::cancel (task.rs:70-77) bumps the generation for exactly this race and pins it with the test `cancel_drops_an_already_queued_result`. Today the UI only cancels via submit() (which does bump), so the gap is latent -- but the trait exposes cancel() and the sibling service documents the hazard the pack service ignores.

*Fix:* Add `self.generation += 1;` to NativePackService::cancel (and a test mirroring task.rs's), so the two services share one cancellation contract.

**[LOW · confusing] `crates/vgms-app/src/corpus.rs:113`** — ChipIndex::load repurposes `scanned` as a (chip, file) pair count, changing its meaning per provenance

build() counts `scanned` as files walked, including unreadable ones (lines 62-71). load() sets it to `by_chip.values().map(Vec::len).sum()` (line 113) -- the deduped per-chip entry count, where a multi-chip rip counts once per chip and unreadable files count zero. tests/chip_index.rs:40,47 prints 'over {scanned} files' and computes per-chip percentage shares against it, so a cache-served run reports inflated file totals and deflated shares compared to the walk that produced the identical index.

*Fix:* Persist the real scanned/unreadable counts in the cache's comment lines and parse them back in load(), or rename the loaded value (e.g. `entries`) and have scanned() return Option when the true count is unknown.

**[LOW · idiom] `crates/vgms-app/src/services/audio.rs:142`** — stream_live() plus eight `.expect("stream_live checked")` re-borrows instead of one Option accessor

Every live-forwarding method (seek_ms, seek_pos, rewind, set_muting, set_panning, set_chip_muting, set_chip_panning, set_boost, set_loop; lines 142-230) repeats the pattern `if self.stream_live() { self.audio.as_mut().expect("stream_live checked")... }`. The check and the unwrap are separated, so each site carries a panic message asserting a nonlocal invariant.

*Fix:* Add `fn live_audio(&mut self) -> Option<&mut NativeAudio> { if self.playing { self.audio.as_mut() } else { None } }` and rewrite each site as `if let Some(audio) = self.live_audio() { ... }`, removing all eight expects.

**[LOW · consistency] `crates/vgms-app/src/cli/render.rs:73`** — render and split rebuild the chip list inline instead of using LoadedSong::chips(), and skip its dedup

LoadedSong::chips() (lib.rs:125-135) exists, per its own doc, to produce 'what vgms_synth::playability wants to hear about', deduplicated -- and play.rs:94 uses it. But render.rs:73 and split.rs:82-89 re-derive `file.header.chips().iter().map(|chip| chip.kind).collect()` inline inside the Vgm match arm, without the dedup. Same concept, two spellings, one of them bypassing the helper written for it (chips() takes &self, so it can be called before the match consumes the song).

*Fix:* Call `song.chips()` before matching in render::run and split::run and pass that to warn_missing_cores, deleting both inline collections.

**[LOW · refactor] `crates/vgms-app/src/cli/play.rs:43`** — The --boost override+validate block is duplicated verbatim between play and render

play.rs:43-49 and render.rs:36-42 contain the identical five lines: assign args.boost into config.audio.boost, call config.validate(), and attach the same `invalid --boost {boost}` context, each with the same 'Reuse the config's 1..=16 range check' comment. A change to the boost validation story (range, message, which config field) must be made twice with nothing enforcing agreement.

*Fix:* Extract a helper (e.g. `fn apply_boost_override(config: &mut AppConfig, boost: Option<f32>) -> Result<()>` in cli/mod.rs or lib.rs) and call it from both subcommands.

*Verifier narrowed this:* The five-line block (assign, validate, identical context string, identical comment) is verbatim-duplicated between play.rs:43-49 and render.rs:36-42 as claimed. Scope narrowed: the 1..=16 range check itself is already centralized in config.validate(), so only the override/context plumbing can drift, not the range -- the 'range... must be made twice' part of the claim is wrong. Still a fair small extraction.

**[LOW · confusing] `crates/vgms-app/src/lib.rs:62`** — read_song_from_path's doc claims every subcommand uses it; none do anymore

The doc (lines 62-63) says 'Every subcommand opens its one input exactly this way', but all subcommands now open via read_any_song_from_path (play.rs:39, render.rs:32, split.rs:41); a workspace grep shows read_song_from_path's only remaining caller is the lib's own test `an_opl_vgm_still_loads_as_the_song_it_always_was` (line 261), where it serves as the parity oracle for the routing pin. The function is legitimate as a test oracle, but its doc describes a role it lost.

*Fix:* Rewrite the doc to state its actual role ('the original OPL-only opener, kept as the oracle the loaded-song routing test compares against'), or move it into the test module if no external caller is intended.

**[LOW · maintainability] `crates/vgms-app/src/bin/vgmstudio.rs:2`** — Subcommand lists in docs drifted: optimize and retrowave-probe missing from three places

bin/vgmstudio.rs:2-3 says 'the play, render and split subcommands'; Cargo.toml:3 (description) likewise names only play/render/split; lib.rs:2-3 names optimize but not retrowave-probe. The Command enum (cli/mod.rs:47-59) has five subcommands. The smoke test named help_lists_every_subcommand (tests/cli_smoke.rs:67-76) also only asserts three, so the one mechanism that could catch the drift drifted with it. Nothing keeps these lists in step with the enum.

*Fix:* Update the three doc lists (or replace them with 'its subcommands -- see vgms_app::cli'), and extend the smoke test's loop to cover optimize and retrowave-probe so future additions fail it.

**[LOW · idiom] `crates/vgms-app/src/cli/optimize.rs:102`** — The non-vgz output path clones the entire optimised file; the vgz path re-assigns a name read() already set

Line 102 does `result.bytes.clone()` -- a full copy of a potentially multi-megabyte VGM -- only because `result` is borrowed again by report() afterwards; report needs just result.bytes.len() and result.original_len, which could be captured before the move. In the vgz branch, line 97-99 calls `vgms_core::vgm::file::read(name, &result.bytes)` and then sets `optimised.name = name.to_owned()`, assigning the same name read() was already given.

*Fix:* Capture the lengths report() needs before consuming result.bytes (or have report take lengths instead of &Optimised), write without cloning, and drop the redundant name assignment.

*Verifier narrowed this:* The redundant name assignment is confirmed: vgm::file::read sets name from its argument (file.rs:848), so optimize.rs:99 is a no-op. But the clone half misreads report(): it needs result.changed()/saved(), result.stages, and result.bytes itself (passthrough_chips_in scans it), so 'capture the lengths before the move' does not work as suggested -- avoiding the clone requires branch-local writes instead. Claim narrowed; low stands mainly on the redundant assignment.

**[LOW · consistency] `crates/vgms-app/src/cli/optimize.rs:87`** — strip-unused-chips reporting is spliced around report(), printing FAILED above the summary but success below it

The vgm_ptch failure line prints at line 87, inside run() before report() emits the 'Optimised ... -> ...' header, while the success line prints at lines 108-113 after the per-stage list -- so the same stage appears in different places depending on outcome, breaking the summary-then-stages shape report()'s doc describes. The literal "vgm_ptch" and the `{:<9}` column format are duplicated at both sites rather than flowing through the StageOutcome reporting every other stage uses.

*Fix:* Thread the strip outcome into report() (e.g. as an extra Stage row or an Option parameter) so all stage lines print in one place with one format string.

**[LOW · confusing] `crates/vgms-app/src/cli/split.rs:67`** — Register-encoded channel decoded with unexplained magic that duplicates vgms-synth's commented version

Lines 67-68 do `let bank = channel >> 8; let channel_num = (channel & 0xFF) - 0xAF;` with no comment; the reader must know the on_skip callback passes a bank-tagged register (0xB0..=0xB8, 0xBD). vgms-synth/src/split.rs:111-112 performs the identical decode and carries the explaining comment (`// 0xB0 -> 1, 0xB8 -> 9, 0xBD -> 14`). Two copies of the encoding knowledge, only one of them documented, and both must change together if the callback's encoding ever does.

*Fix:* Have vgms_synth::split's on_skip pass decoded (bank, channel_num) or a preformatted label -- it already computes both for file names -- or at minimum copy the decode comment to the CLI site.

**[LOW · bug-risk] `crates/vgms-app/src/services/file.rs:313`** — Case-only rename detection is ASCII-only, so accented case-only renames fail as clobbers on NTFS

same_file_case_only (lines 310-313) uses eq_ignore_ascii_case. Renaming '01 étude.vgz' to '01 Étude.vgz' fails that test (é/É are non-ASCII), falls through to `dest.exists()` -- true on NTFS, whose case folding is Unicode-wide -- and errors '01 Étude.vgz already exists' instead of doing the rename_via_temp bounce built for exactly this case. European VGMRips track titles make this reachable from pack mode's rename.

*Fix:* Compare with Unicode-aware case folding (e.g. compare char-by-char via to_lowercase(), or check whether from and dest resolve to the same file via fs::metadata handle/ID) before treating an existing dest as a clobber.

**[LOW · other] `crates/vgms-app/src/parity/mod.rs:363`** — known_gap strings carry accidental runs of embedded spaces that print into scorecard output

The YM2612 entry's known_gap (line 363) contains '...puts          Nuked-OPN2...' and the YM2413's (line 374) '...the 0.99              ideal...' -- source-wrap indentation baked into the string literals. These strings are printed verbatim by the scorecard test ('[known gap: {why}]', tests/reference_parity.rs:689) and by the tolerated-failure line, so the harness output shows ragged multi-space gaps mid-sentence. The assert message at mod.rs:427 has the same artefact.

*Fix:* Use concat! or `\` line continuations so wrapped string literals do not embed the indentation, and fix the three affected strings.

### Core providers and hardware crates

**[MEDIUM · bug-risk] `crates/vgms-cores-gpl/src/lle_opm.rs:153`** — YM2151-LLE stops asserting the YM2164 variant pin after the first bus write

reset() presents LlePins { ym2164: variant, .. } and clocks the reset, but every later set_pins call clobbers the pin: render() calls master_clock(LlePins::default()) (line 234, ym2164: false), the Bus::Idle arm presents the queued byte on those default-derived pins, and the Bus::Holding arm explicitly does `pins = LlePins::default()` (line 153) before releasing the bus with set_pins. The shim (shim/lle_opm.c:26) writes chip->input.ym2164 on every call, so from the first register write onward the die's variant input reads 0 and a YM2164-flagged VGM is simulated as a YM2151. The `pins_base` parameter looks designed to carry the variant but the only caller passes default, and Ym2151Lle stores no variant field to re-present. The Nuked OPM wrapper has a test for the variant flag (opm.rs `the_ym2164_variant_reaches_the_chip`); the LLE wrapper has none, so this fails silently in the crate whose whole purpose is being the oracle.

*Fix:* Store the variant on Ym2151Lle at reset and derive every pin set from `LlePins { ym2164: self.variant, ..LlePins::default() }` (drop the misleading `pins_base` parameter). Add a variant-reaches-the-die test mirroring opm.rs's.

**[MEDIUM · refactor] `crates/vgms-cores-gpl/src/lle_opn2.rs:95`** — LLE bus state machine is triplicated in-crate, with a bit-31 flag packed into a counter

BusByte + the Bus {Idle/Holding/Recovering} enum + the master_clock bus arm exist three times in vgms-cores-gpl: lle_opm.rs (lines 50-180), lle_opn2.rs (lines 44-122) and lle_opna.rs (lines 53-226). The opn2 and opna copies are line-for-line identical, including the trick of packing the "was this a value byte" flag into bit 31 of the Holding counter (`Bus::Holding(WRITE_HOLD | (u32::from(byte.a0) << 31))`, then unpacking with `state & !(1 << 31)`), when the enum variant could simply carry `Holding { left: u32, was_value: bool }`. Three copies of a timing-critical state machine in one crate invite drift (the opm copy already differs subtly in the way that produced the ym2164 pin bug), and the bit packing obscures the hold/recover logic for no gain.

*Fix:* Extract one bus-driver helper (queue + hold/recover state machine, parameterised on hold/recover constants and a set_pins/release callback) into a shared module of the crate; give the Holding state named fields instead of bit 31.

**[MEDIUM · dead-code] `crates/vgms-cores-ymfm/src/lib.rs:5`** — vgms-cores-ymfm has zero dependents but is still compiled by every workspace build, and its docs overstate what it covers

No crate depends on vgms-cores-ymfm (grep of all Cargo.tomls finds only its own; the app registers only libvgm/nuked/gpl providers), it exports no register() like the other three provider crates, yet it remains a workspace member with a workspace-dependency alias (root Cargo.toml line 44), so `cargo build/test --workspace` compiles ymfm's C++ every time for nothing. The freeze is a documented decision (docs/vgm-multichip-2026-07/CORES-REUSE-PLAN.md: "ru-2 stays frozen. ymfm remains at its PoC"), but nothing in the crate says so; worse, its crate doc claims it covers "the chips our own cores are weakest on (YM2608, YM2610, YMF278B) and one they cannot register at all (Y8950)" while `Kind` and the shim's `ymfm_create` build only the OPN family (YM2203/2608/2610/2610B/2612/3438) — no YMF278B, no Y8950 (build.rs compiles only ymfm_opn.cpp, never ymfm_opl.cpp).

*Fix:* Either drop the crate from workspace members (keeping it on a branch/history) or mark it clearly as a frozen PoC in its lib.rs and exclude it from default-members so workspace builds stop paying the C++ compile; in any case fix the lib.rs doc to state the actual (OPN-only, unregistered) coverage.

**[MEDIUM · consistency] `crates/vgms-vgmtools/src/lib.rs:228`** — Exit-code-to-outcome interpretation exists in three copies despite command.rs claiming to hold it once

command.rs's module doc says "This module holds that interpretation once, so the native binding, the wasmi parity test and the web worker cannot drift apart on what an exit code means" — but the native binding does not call `command_outcome`: lib.rs's run_tool (lines 221-244) plus collect (251-272) re-implement the same rules (exit 0 + no output = Unchanged, decline codes = Unchanged, output must pass check_output, tail quoted on failure), strip.rs re-implements them a third time for vgm_ptch (lines 157-179), and the `suffix()` helper is duplicated verbatim (lib.rs:274, command.rs:88). The decline codes are at least shared through ToolId::declines_with, but the surrounding match is exactly the drift surface the doc claims cannot exist.

*Fix:* Make the native run_tool gather (exit_code, output_bytes_if_any, tail) and call command_outcome for the Exited(Some(code)) case, keeping only TimedOut/terminated handling local; either route strip.rs's Patch case through a variant of it or note in command.rs's doc that vgm_ptch is interpreted separately.

**[MEDIUM · consistency] `crates/vgms-cores-libvgm/build.rs:433`** — libvgm C build omits the -ffp-contract=off that vgms-cores-gpl applies for the identical determinism reason

vgms-cores-gpl/build.rs:71 passes `-ffp-contract=off` with the rationale that float contraction "would fuse those into FMAs on one target but not another, breaking ChipCore's promise of identical output everywhere", and wasm_libc.rs restates that ChipCore promises identical IEEE results across targets. libvgm's build compiles ~40 devices at opt_level(2) with no contraction flag, and several enabled cores compute samples in float/double (Maxim sn76489.c volume/panning tables, emu2413, okim6258, panning.c itself). Today's targets (x86-64 without -mfma, wasm32, MSVC /fp:precise) happen not to emit FMAs, so this is latent — but an aarch64 native build (Apple Silicon) would contract under clang's default and silently break cross-target parity with no comment explaining why the flag is absent here and present next door.

*Fix:* Add `.flag_if_supported("-ffp-contract=off")` to libvgm's cc::Build with the same one-line rationale (and to vgms-cores-ymfm's if that crate survives), or document why libvgm is exempt.

**[LOW · confusing] `crates/vgms-cores-ymfm/src/ffi.rs:103`** — YmfmChip::reset's rebuild rationale contradicts the shim, and the rebuild silently discards loaded ROMs

The doc comment says the chip is rebuilt at a new clock "because a ymfm chip derives its sample rate from the clock it was constructed with rather than taking one later", but the shim's `ymfm_sample_rate(chip, clock)` (ffi.rs:29, ymfm_c.cpp:88-90) is clock-parameterised — ymfm's sample_rate is a pure function of the clock passed at query time, and the constructor's clock is stored in an unused `m_clock` field. Rebuilding also constructs a fresh chip_impl whose `m_data` ADPCM/PCM buffers are empty, so any load_rom data delivered before a clock-changing reset is silently thrown away. Today the clock only changes on the first reset (before ROMs arrive), so it works by ordering, but the stated reason is false and the ROM-loss hazard is undocumented.

*Fix:* Replace the rebuild with `self.clock = clock; self.rate = unsafe { ymfm_sample_rate(self.handle, clock) }.max(1); unsafe { ymfm_reset(self.handle) }` — or, if the rebuild is kept for pristine-state reasons, say that and note the ROM-discard ordering assumption.

**[LOW · dead-code] `crates/vgms-cores-ymfm/src/ffi.rs:122`** — write() computes an offset bit both call sites immediately mask away; shim stores an unread m_clock

`let offset = (u32::from(port) << 1) | (u32::from(addr) & 1);` is followed only by `ymfm_write(.., offset & !1, ..)` and `ymfm_write(.., offset | 1, ..)`, so the `(addr & 1)` contribution can never be observed — it reads as though addr's low bit selects address/data when it does nothing. Relatedly, shim/ymfm_c.cpp's chip_impl stores `m_clock` (line 146) that no method reads, so ymfm_create's clock parameter is effectively dead.

*Fix:* Compute `let base = u32::from(port) << 1;` and call `ymfm_write(handle, base, register)` / `ymfm_write(handle, base | 1, data)`; delete m_clock (and then the clock parameter of ymfm_create if the reset simplification lands).

**[LOW · dead-code] `crates/vgms-cores-nuked/Cargo.toml:17`** — Unused `log` dependency in vgms-cores-nuked and vgms-cores-gpl

Both provider crates declare `log.workspace = true` under [dependencies] but neither contains a single `log::` call (grep of both src trees finds none); the other provider crates (libvgm, retrowave, vgmtools) do use it. Dead dependency edges cost build parallelism clarity and mislead readers about the crates' logging behaviour.

*Fix:* Remove `log` from vgms-cores-nuked/Cargo.toml:17 and vgms-cores-gpl/Cargo.toml:17.

**[LOW · confusing] `crates/vgms-vgmtools/Cargo.toml:43`** — Stale lint comment claims FFI/`ffi.rs` in a crate that has neither unsafe code nor an ffi.rs

The [lints.rust] comment reads "The tool entry points are `extern \"C\"`, and the temp-file plumbing around them is safe Rust. Confined to `ffi.rs`, as in the provider crates" — but this crate runs the tools as child processes / wasm command modules by explicit design (lib.rs module doc), contains zero `unsafe` and zero `extern \"C\"` (verified by grep), and has no ffi.rs. The `unsafe_code = "allow"` override it justifies is therefore unnecessary and the comment describes a linked-in design that was deliberately abandoned.

*Fix:* Delete the `unsafe_code = "allow"` override (or switch to the workspace lints table like vgms-retrowave/vgms-audio-native) and drop the stale comment.

**[LOW · refactor] `crates/vgms-cores-gpl/src/ffi.rs:83`** — OpaqueChip is copy-pasted between the nuked and gpl provider crates; require_submodule between four build.rs files

crates/vgms-cores-gpl/src/ffi.rs:83-115 is a trimmed duplicate of crates/vgms-cores-nuked/src/opaque.rs (same u64-backed storage, same alignment assert, same Debug and Send rationale); the gpl copy lacks the nuked copy's unit tests, so the two can drift (e.g. a fix to the zero-size or alignment handling landing in one). Similarly `require_submodule` + the UPSTREAM constant are re-declared in vgms-cores-nuked/build.rs, vgms-cores-gpl/build.rs, vgms-cores-ymfm/build.rs and vgms-cores-libvgm/build.rs (nuked additionally has a `watch()` helper the others inline). No comment claims the duplication is deliberate; OpaqueChip is original project code, so license tiering does not force the copy — it could live in vgms-synth (MIT) or a tiny shared support crate.

*Fix:* Move OpaqueChip into vgms-synth (or a shared ffi-support module) and use it from both provider crates; consider a small shared build-support path for require_submodule, or at least converge the four copies on one wording.

**[LOW · maintainability] `crates/vgms-cores-gpl/src/lle_opn2.rs:187`** — LLE wrappers hard-code the Nuked cores' calibrated gains (21, 2) with comment-only cross-crate coupling

lle_opn2.rs render() multiplies by a bare `21` with the comment "the shipping core's calibrated OUTPUT_GAIN applies unchanged", and lle_opm.rs:239 multiplies by `2` ("matches Nuked-OPM's OUTPUT_GAIN = 2 so the oracle diff reads level 1.0"). The authoritative constants live in a different crate (vgms-cores-nuked opn2.rs OUTPUT_GAIN=21, opm.rs OUTPUT_GAIN=2); if a re-measurement moves either, the LLE oracle silently diverges by a level factor and every oracle diff reads as a volume bug. Unlike the Nuked wrappers, the LLE ones have no loudness-pinning test.

*Fix:* Name the values as crate-local constants with doc comments pointing at the Nuked constants they must track, and add a loudness-pinning test per LLE wrapper (like nuked's a_loud_patch_uses_the_range_without_clipping_it) so a drift fails a test instead of an oracle run.

**[LOW · maintainability] `crates/vgms-cores-libvgm/src/chip.rs:1`** — chip.rs is a 2516-line file carrying five separable concerns

The file holds the WriteRule/fold pure mapping (~280 lines), the Writers FFI fetcher, the LinkedDev child-device resampler/mixer, the LibVgmChip wrapper itself, the chip_specs! macro plus the 160-line spec table, ten per-chip configure_* functions, and ~860 lines of tests. Each piece is well-documented, but the table + configure functions and the fold rules are pure data/logic with no need to live beside the unsafe wrapper, and the file is at the size where navigation and review cost real time.

*Fix:* Split into specs.rs (chip_specs! table + configure_* fns + rom_space/default_option_bits/split_mute/link_gain), fold.rs (WriteRule/Bus/fold with their tests), keeping chip.rs as the LibVgmChip/Writers/LinkedDev wrapper.

**[LOW · idiom] `crates/vgms-pack-archive/src/lib.rs:43`** — PackArchive::open clones the whole zip byte buffer for no reason

`zip::ZipArchive::new(Cursor::new(zip_bytes.to_vec()))` copies the entire archive (packs can be tens of MB) when `Cursor::new(zip_bytes)` over the borrowed slice already satisfies Read + Seek and the archive does not outlive the call. Also, the crate reports errors as bare String while sibling crates (vgms-retrowave, vgms-audio-native) use thiserror enums — a minor cross-crate inconsistency in error style.

*Fix:* Use `Cursor::new(zip_bytes)` directly; optionally introduce a small error enum if callers ever need to distinguish not-a-zip from empty.

**[LOW · bug-risk] `crates/vgms-pack-archive/src/lib.rs:121`** — rename()'s case-only branch can silently swallow a distinct entry

The map is case-sensitive and a zip may legitimately contain both "Song.VGM" and "song.vgm" (open() inserts both). rename("Song.VGM", "song.vgm") takes the case_only branch, which skips the contains_key collision check, removes the source and inserts over the existing distinct "song.vgm" — silently destroying its bytes. The native filesystem semantics being mirrored cannot exhibit this (NTFS cannot hold both names), but the in-memory model can, so the "fails rather than overwrites" guarantee in the doc comment does not hold in that corner.

*Fix:* In the case_only branch, still refuse (or explicitly document the overwrite) when `from != to && self.entries.contains_key(to)`.

**[LOW · other] `crates/vgms-cores-libvgm/src/chip.rs:772`** — Log and assert message strings carry embedded line-wrap space runs

Several long string literals were wrapped without a `\` continuation, so the output contains runs of ~14 spaces mid-sentence: chip.rs:772 (`"...its registers will be                  silently dropped"` — a user-facing warn!), vgms-cores-nuked/src/opn2.rs:282, :512 and :543 (assert messages). The `\`-continuation used elsewhere in the same files (e.g. opn2.rs:359) strips leading whitespace correctly, so these are formatting accidents, not style.

*Fix:* Rewrap with `\` line continuations (or concat!) so the rendered messages read normally; a quick grep for `"  ` inside string literals catches the rest.

### Web target — wasm glue and JS host

**[HIGH · bug-risk] `crates/vgms-web/src/services/audio.rs:169`** — load() supersedes the current AudioWorkletNode without disconnecting it, clearing pending, or guarding overlapping setups

The reset block in `load` (lines 166-173) sets `inner.node = None` with the comment "drop the old node so its handler stops updating our state", but dropping the web_sys handle neither disconnects the node from the destination nor unhooks its port handler: the old AudioWorkletNode stays in the audio graph (only `unload` calls `disconnect()`, and the app calls `load` without `unload` from `ensure_audio` (vgms-ui/src/app.rs:4500) and the pack preview (app.rs:2908-2909)), and its `onmessage` still points at the closure in `inner._on_message`, which keeps overwriting the freshly-reset state until the new `setup` replaces it -- and once replaced, the dropped closure is still installed on the old node's port, so its ~23 ms state posts start throwing wasm-bindgen "closure invoked after being dropped" errors. Additionally `inner.pending` is not cleared (unload clears it; load does not), so commands queued against the superseded song (a `play`, a seek) flush into the next song's node; and two rapid `load`s run two concurrent `setup()` futures with no generation counter -- whichever finishes last installs its node as `inner.node`, which can be the older song, with both nodes left connected and audible.

*Fix:* In load(): take and `disconnect()` the old node, clear `pending`, and drop/replace `_on_message` immediately; add a generation counter captured by each spawned setup and checked (against the current generation) before installing the node, flushing pending, or reporting errors.

**[MEDIUM · maintainability] `tools/build-web.ps1:61`** — Build copies the whole web/ tree into the servable dist, including 27 MB of e2e node_modules

`Copy-Item (Join-Path $root "web\*") $dist -Recurse -Force` copies everything under web/ into target/web-dist, which the script and serve.mjs treat as the deployable, HTTP-served directory. web/e2e/ contains node_modules (19 MB of Playwright), package-lock.json, playwright.config.js, and even test-results. The current target/web-dist confirms it: it holds a 27 MB e2e/ directory. Every build re-copies the test harness into the artifact you would upload, bloats the dist by an order of magnitude, slows the build, and serves the test infrastructure (and stale test-results) over HTTP alongside the app.

*Fix:* Copy an explicit allowlist (index.html, task_worker.js, pack_worker.js, worklet-processor.js, wasi-shim/) or add -Exclude e2e; also consider deleting the stray e2e/ from an existing target/web-dist so old deploy dirs do not keep shipping it.

*Verifier narrowed this:* Facts confirmed: line 61 copies web\* recursively; target/web-dist/e2e exists and is 27 MB (19 MB node_modules plus package-lock, playwright.config.js, test-results); serve.mjs serves the whole dist. But severity drops to medium: the genuine payload is ~18 MB (12.1 MB vgms_web_bg.wasm + 4.3 MB font + tools), so e2e roughly 2.5x's the dist, not 'an order of magnitude'; and the dist is currently consumed only by the local e2e server (branch unmerged, no deploy pipeline), so 'the artifact you would upload' is prospective. Still a real build-script defect worth the one-line fix, especially given the script's own '-E2e ... the hook never ships' posture.

**[MEDIUM · bug-risk] `web/worklet-processor.js:256`** — No processor teardown: process() always returns true, so every loaded song leaks a live wasm instance on the audio thread

`process()` unconditionally returns true ("Keep the processor alive across the whole song"), and there is no dispose command in `_onCommand`. Per the Web Audio spec an AudioWorkletNode whose processor returns true is an actively-processing source that is kept alive and keeps being processed even after it is disconnected. The page side creates one node per loaded song (services/audio.rs) and never tells the old processor to stop, so every song ever loaded in a session -- including cleanly unload()ed ones -- keeps its WebAssembly.Instance (module + full song + engine state) resident on the audio thread, keeps executing process() every 128 frames, and keeps posting a state message every 8 quanta into a handler the page has since dropped.

*Fix:* Add a `dispose` command that sets a flag making process() return false and drops `this.instance`/`this.ex` references; have WebAudioService::unload (and load's supersede path) post it before disconnecting.

**[MEDIUM · refactor] `crates/vgms-web/src/pack_zip.rs:64`** — The web pack-zip builder duplicates vgms-app's pack_zip almost line for line

build_pack_zip, process_entry, gzip, has_extension, to_vgz_name, PackZipOutput, and the test scaffolding (read_zip, song(), never(), optimizable_vgm_bytes) all exist twice: here and in crates/vgms-app/src/pack_zip.rs (lines 30-161). The only real deltas are the PNG arm (oxipng vs kept-as-is), the error type (String vs anyhow), the optimizer parameter (Option<&dyn SongOptimizer> vs bool+NativeTools), and the heartbeat callback. The .vgz renaming rules, the already-gzipped short-circuit, the log line formats, and the zip-writing loop must be changed in two places or the native and web packs drift -- there is no test tying them together.

*Fix:* The web version is already the more general one (caller-supplied SongOptimizer, progress callback). Move it to a crate both targets reach (vgms-ui::platform or a small shared module), add an image-optimizer hook (native passes oxipng, web passes None), and delete the vgms-app copy.

**[MEDIUM · maintainability] `crates/vgms-synth-worklet/src/player.rs:48`** — install_web_cores mirrors vgms-app::install_cores by hand with no shared code or enforcing test

install_web_cores (lines 48-61) repeats vgms-app::install_cores (crates/vgms-app/src/lib.rs:33-58) exactly -- same provider registration order (libvgm, nuked, gpl) and the same three promotions (ym2612/ym2151/ym2413 to nuked) -- differing only in the native-only vgms_retrowave::register call. The comment documents the mirroring but nothing enforces it: a new provider crate, a fourth promotion, or a reorder made in one place silently gives web and native users different default cores for the same file.

*Fix:* Extract the shared part into one function taking &mut CoreRegistry (e.g. `register_common_cores` in a crate both depend on, or have vgms-app call into this crate's registration before adding retrowave), so the promotion list exists once.

**[LOW · consistency] `crates/vgms-web/src/services/task.rs:94`** — A task Worker that fails to spawn or receive its request dies silently; the pack service surfaces the same failure to the user

In WorkerTaskService::spawn, a Worker::new_with_options error (line 94) or post_message error (line 134) only logs to the browser console and returns; is_busy_kind then reports idle, so the app shows no spinner, no error, and the waveform/render/scan simply never arrives. WebPackService::submit handles the identical situation by pushing PackJobOutcome::Failed so the user sees the failure (services/pack.rs:149-153). The same fault class is surfaced two different ways by two sibling services in the same crate.

*Fix:* On spawn/post failure, synthesize a failed TaskResult where the request kind has an error-carrying variant (Wav/Split), or at minimum route the message through the app's alert channel rather than only console.error.

**[LOW · dead-code] `web/worklet-processor.js:91`** — The autoplay processorOption is never set by anything

`this.playing = !!opts.autoplay` and its justifying comment ("lets an offline render ... start sounding") reference a capability that does not exist: node_options() in crates/vgms-web/src/services/audio.rs is the only producer of processorOptions and never sets autoplay, and a workspace grep finds no OfflineAudioContext or other setter (only unrelated Playwright autoplay-policy flags). The line always evaluates to false, and the comment describes a feature that was never built.

*Fix:* Replace with `this.playing = false;` and delete the comment (or implement the offline render it anticipates when that feature actually arrives).

**[LOW · confusing] `web/task_worker.js:27`** — "Transferring the buffer so nothing is copied" sits directly above an explicit full copy

The emit callback does `const copy = bytes.slice(); self.postMessage(copy, [copy.buffer]);` under the comment "transferring the buffer so nothing is copied". `bytes` is already a JS-owned Uint8Array (worker.rs builds it with js_sys::Uint8Array::from, which copies out of wasm memory into a fresh buffer), so the slice() is a second full copy of the result -- for a RenderWav result that can be tens of megabytes -- and the comment contradicts the code. web/pack_worker.js:100-101 has the same slice-then-"zero-copy" pattern on the encoded PackJobOutcome.

*Fix:* Transfer `bytes` directly (`self.postMessage(bytes, [bytes.buffer])`) in both workers, or if the defensive copy is intentional, correct the comments to say one copy is made to detach from the emitter.

**[LOW · consistency] `crates/vgms-web/src/optimize_tools.rs:117`** — Two private helpers in the same crate extract a message from a JsValue with different names and shapes

optimize_tools::describe(&JsValue) -> String (as_string, else .message, else "wasm error") and services/file.rs::js_error(JsValue) -> String (line 210: as_string, else .message, else format!("{value:?}")) implement the same JS-error-to-text conversion twice with different fallbacks and ownership conventions. A third variant of the pattern (per-field defaults) lives in services/audio.rs get_string/get_f64.

*Fix:* Fold describe/js_error into one crate-private helper (e.g. in a small js_util module) and use it from both call sites.

**[LOW · bug-risk] `web/worklet-processor.js:217`** — process() renders left.length frames into fixed 128-frame scratch buffers without clamping

leftPtr/rightPtr are allocated once at QUANTUM*4 bytes (lines 85-86), but process() passes `frames = left.length` straight to vgmsw_render, which writes `frames` f32s at each pointer. Today the render quantum is fixed at 128, but the Web Audio spec has since added AudioContextRenderSizeCategory/renderSizeHint and Chrome ships it; any future context configuration (or engine change) that delivers a larger block would silently overwrite unrelated wasm heap memory past the scratch buffers.

*Fix:* Clamp (`const frames = Math.min(left.length, QUANTUM)`) or (re)allocate the scratch when `left.length` exceeds the allocated capacity.

**[LOW · confusing] `crates/vgms-web/src/services/pack.rs:81`** — _on_timeout is underscore-named but load-bearing

The leading-underscore convention in this codebase (and Rust generally) marks fields kept alive but never read -- which is true of `_on_message` (line 79) but not of `_on_timeout`: rearm_watchdog() borrows it and clones the inner function to arm every setTimeout (lines 100-102), and terminate() clears it. A reader skimming the struct is told the field is inert when it is actually the watchdog's callback source.

*Fix:* Rename to `on_timeout` (keeping the comment about it also needing to outlive armed timers); leave `_on_message` as is.

**[LOW · consistency] `crates/vgms-web/src/services/audio.rs:181`** — load() reports a synchronous context failure via last_error()+Ok while an equally synchronous encode failure returns Err

`source_bytes` failure returns Err from load() (line 151), but an ensure_context failure two statements later stores the message in last_error and returns Ok(()) (lines 181-184). Both failures are synchronous and known before load() returns; the module doc justifies last_error only for faults "away from a call" (the async setup). The caller therefore sees the same immediate failure through two different channels depending on which step broke, and the context error is only surfaced whenever the app next polls last_error.

*Fix:* Return Err(message) for the ensure_context failure, reserving last_error for the genuinely asynchronous setup() path.

**[LOW · maintainability] `tools/build-web.ps1:71`** — CJK font cache is never validated and a .ttf ships under a .otf name

The font is downloaded once to target/cjk-font.otf and thereafter `Test-Path` short-circuits the download forever -- an interrupted Invoke-WebRequest that leaves a partial file makes every future build silently ship a corrupt font (the runtime degrades to box glyphs with no build-time signal, so it looks like the feature quietly stopped working). Separately, the URL fetches NotoSansJP_400Regular.ttf (a TrueType file) but caches and serves it as cjk-font.otf, and serve.mjs then labels it font/otf by extension; it works only because the runtime sniffs bytes, and the mismatch will puzzle anyone inspecting the dist.

*Fix:* Download to a temp name and rename only on success (or sanity-check the file size/magic before trusting the cache), and name the asset cjk-font.ttf end to end (CJK_FONT_URL in runner.rs, the copy here, and the e2e server type map already handles .ttf).

**[LOW · idiom] `crates/vgms-web/src/runner.rs:86`** — build_app constructs LocalStorageStore twice and loads the config once just for the theme

Line 67 builds a store and calls store.load() solely to read config.ui.theme; line 86 then boxes a second LocalStorageStore::new() for the app, which will re-load the same INI from localStorage. The type is zero-sized so the cost is only the duplicate parse plus reader head-scratching about whether the two stores can disagree (they cannot).

*Fix:* Build one store, read the theme from its load(), and pass that same store (Box::new(store)) to VgmStudioApp::new.

*Checked and dismissed here:* __vgms_run_tool reads the vendored shim's private internals to collect out.vgm.

### Workspace architecture and docs

**[HIGH · dead-code] `.github/workflows/check.yaml:24`** — Python CI workflow runs on every push but the Python project was removed, so it fails every time

check.yaml triggers on every push and runs `pip install -r requirements.txt`, black/mypy over ./src and ./tests, and `python -m unittest discover`. Commit 5e9ece7 'chore: remove the superseded Python project' deleted requirements.txt and every .py source; src/ now contains only vgmstudio.ico and vgmstudio.ini, and tests/ contains only Rust fixtures. The pip-install step fails immediately, so this workflow paints a red X on every push alongside the real rust.yaml results, training people to ignore CI failures.

*Fix:* Delete .github/workflows/check.yaml (rust.yaml already covers fmt/clippy/test on the Rust workspace).

**[HIGH · maintainability] `DEVELOPMENT.md:3`** — DEVELOPMENT.md claims the Python sources 'stay put' and keeps whole sections instructing against removed files

Line 3-4 says 'the Python sources under src/ stay put during the transition, for parity comparison. Both suites run.' -- false since commit 5e9ece7 removed the Python project. The Setup section (lines 127-155: install Python 3.13, pip install -r requirements.txt / requirements_dev.txt, Black in IntelliJ), Build .exe (lines 193-197: cd src; python setup.py), Format code (line 202: black src/ tests/), Type-check (lines 207-210: mypy) and Run tests (line 215) all reference files that no longer exist. This is the repo's primary onboarding document.

*Fix:* Rewrite DEVELOPMENT.md around the Rust workspace only: drop lines 3-4's transition claim and delete the Python Setup/Build .exe/Format/Type-check/Run-tests sections.

**[HIGH · maintainability] `licenses/README.md:8`** — The licensing README's crate tables omit four crates, including the one with genuinely unresolved upstream terms

The application row (line 8) lists vgms-app, vgms-ui, vgms-audio-native, vgms-retrowave, vgms-web, vgms-synth-worklet but omits vgms-pack-archive and vgms-vgmtools (both GPL-2.0-or-later). The provider table (lines 32-35) lists only vgms-cores-nuked and vgms-cores-gpl, omitting vgms-cores-libvgm -- whose Cargo.toml documents that libvgm ships no licence grant at all, exactly the kind of fact this file exists to surface -- and vgms-cores-ymfm (MIT OR Apache-2.0, complicating the 'two halves' framing). DEVELOPMENT.md line 21 promises 'licenses/README.md has the full split', and this is a project that treats licensing as load-bearing.

*Fix:* Add vgms-pack-archive and vgms-vgmtools to the application row, add vgms-cores-libvgm (with a pointer to its unresolved-grant note) and vgms-cores-ymfm to the provider table, and note that a permissive provider exists outside the reusable pair.

**[MEDIUM · dead-code] `.github/workflows/build.yaml:36`** — build.yaml packages the app with `cd src; python setup.py` against files that no longer exist

The workflow_dispatch packaging workflow installs the removed requirements files and runs `python ./setup.py` inside src/, which now holds only an .ico and an .ini. Anyone triggering it gets a guaranteed failure; there is no Rust release-packaging replacement in .github/workflows, so the repo currently has no working release workflow at all.

*Fix:* Delete build.yaml, or replace it with a cargo-based release build (cargo build --release -p vgms-app plus the tools/build-web.ps1 web bundle).

**[MEDIUM · maintainability] `DEVELOPMENT.md:84`** — DEVELOPMENT.md's CLI and wasm-check details drifted from the code, and the completed web target is undocumented

Line 84 says a file named 'play', 'render', 'split', 'convert' or 'help' parses as a subcommand -- but crates/vgms-app/src/cli/mod.rs defines Play/Render/Split/Optimize/RetrowaveProbe: 'convert' is not a subcommand and 'optimize'/'retrowave-probe' are missing from the list. Line 54's wasm-cleanliness command checks only vgms-core and vgms-synth, while rust.yaml line 68 checks seven crates (adding vgms-ui, vgms-web, vgms-synth-worklet, vgms-pack-archive, vgms-retrowave). And although the wasm web target is complete (tools/build-web.ps1, web/, Playwright e2e, the wasip1 optimiser modules), DEVELOPMENT.md never mentions how to build, serve or test it.

*Fix:* Fix the subcommand list, align the wasm-check command with rust.yaml's crate set, and add a short 'Web build' section pointing at tools/build-web.ps1, tools/build-wasi-tools.ps1 and web/e2e.

**[MEDIUM · maintainability] `TODO.md:180`** — TODO.md's any-chip-playback entry describes the pre-libvgm world: 'the chips are not [built] ... today it renders silence'

Lines 180-252 state the engine 'has no implementations of the chips themselves, so today it renders silence', that 'the first chip is in: an SN76489' written clean-room (line 197), and that 'Still to come: more cores (the YM2612 and YM2413 are the ones that would open up the most rips)' (line 249). In reality the clean-room cores were deleted, vgms-cores-libvgm serves every non-OPL chip (registered first by install_cores in crates/vgms-app/src/lib.rs:38), and Nuked YM2612/YM2151/YM2413 cores are promoted as defaults (lib.rs:50-52). Anyone using TODO.md to plan work is misled about roughly a third of the file.

*Fix:* Rewrite the 'Any-chip playback' entry (and the Phase A-C remainders at lines 253-275) to record the libvgm outcome, the way the other entries record theirs, and remove line 197's clean-room SN76489 story.

**[MEDIUM · terminology] `crates/vgms-vgmtools/src/pipeline.rs:157`** — Public identifiers mix 'optimise' and 'optimize' spellings, sometimes within one API

vgms-vgmtools exports `pub struct Optimised` (pipeline.rs:157) beside `optimize_vgm_with` (line 205) and `optimize_song_logged` (line 261) -- the return type of a function spelled the other way. vgms-core's VgmFile has `pub fn optimize` (src/vgm/file.rs:588) and `pub fn unoptimised_chips` (file.rs:643) in the same impl. Module names split the same way: vgms-core `pub mod optimize` and vgms-web `pub mod optimize_tools` vs vgms-ui `mod optimise` (src/lib.rs:21). Prose comments consistently use British spelling (fine, that's the dialect); the flag is only that code identifiers use both, so callers must remember which spelling each item took.

*Fix:* Pick one spelling for identifiers (the majority is 'optimize': the CLI subcommand, config keys, optimize.rs modules) and rename `Optimised` -> `Optimized`, `unoptimised_chips` -> `unoptimized_chips`, and vgms-ui's `optimise` module to `optimize`.

**[MEDIUM · maintainability] `crates/vgms-pack-archive/Cargo.toml:20`** — The zip/flate2 pins that protect VGZ byte parity are hand-copied into three manifests instead of inherited

Root Cargo.toml defines zip 8.6 (default-features = false, deflate-flate2) and flate2 (rust_backend) in [workspace.dependencies] precisely because any feature drift silently flips flate2 to zlib-rs and breaks native-vs-wasm VGZ byte parity (root Cargo.toml:123-130). Yet vgms-pack-archive (lines 20-21) and vgms-web (Cargo.toml:40-41) restate the same version+features verbatim rather than `zip.workspace = true` / `flate2.workspace = true` -- vgms-pack-archive's comment even says it is 'the same pair (and reasoning) the workspace uses'. A future zip bump or feature change now has three places to keep in lockstep, with no enforcing mechanism, and the failure mode the comments warn about is silent byte drift.

*Fix:* Replace both crates' inline specs with workspace inheritance; workspace dep inheritance carries default-features = false and the feature list, so behaviour is identical with a single point of truth.

**[MEDIUM · refactor] `crates/vgms-app/src/pack_zip.rs:30`** — The pack-zip builder is duplicated between vgms-app and vgms-web with hand-synchronized semantics

vgms-app/src/pack_zip.rs and vgms-web/src/pack_zip.rs each define PackZipOutput, build_pack_zip, process_entry, gzip, has_extension and to_vgz_name with the same zip options, the same 'already gzipped despite the .vgm name: just rename it' rule, and the same log-line formats; the web file's own header calls itself 'the wasm-portable half of vgms-app's pack_zip'. The copies have already drifted in shape (anyhow vs String errors, an on_progress hook only on the web side, a SongOptimizer trait vs a hardcoded native call), and nothing pins the shared semantics -- a fix to the gzip/rename rule or a log format in one file silently misses the other.

*Fix:* Move the portable builder into vgms-pack-archive (already the zip-owning, wasm-clean, app-tier crate) with SongOptimizer plus an optional image-optimizer hook; vgms-app then supplies the oxipng hook and NativeTools, vgms-web supplies its Worker pipeline, and one set of round-trip tests covers both.

**[LOW · dead-code] `crates/vgms-cores-ymfm/Cargo.toml:16`** — vgms-cores-ymfm is an orphaned crate: no consumer, no register(), and an unused vgms-core dependency

Grepping the whole workspace, nothing depends on vgms-cores-ymfm: vgms-app's install_cores and vgms-synth-worklet's install_web_cores register libvgm/nuked/gpl/retrowave only, the crate exports just ffi::{Kind, YmfmChip} with no register() into the provider convention, and the [workspace.dependencies] entry (root Cargo.toml:44) has zero users. Its vgms-core dependency (line 16) is also unused -- the only cross-crate import in the whole crate is vgms_synth::chip::ChipCore in src/ffi.rs:10. Every `cargo test/clippy --workspace` still compiles its C++ submodule. docs/vgm-multichip-2026-07/CORES-REUSE-PLAN.md ru-2 froze ymfm at its PoC and stated the crate is 'removed entirely' if it does not clear the gate, but the removal never happened.

*Fix:* Either remove the crate (git history keeps the PoC; also drop the workspace-dependencies entry and the submodule) or, if it is being deliberately kept as a future accuracy tier, record that in its lib.rs and drop the unused vgms-core dependency.

*Verifier narrowed this:* The facts hold: workspace-wide grep finds no consumer, the crate exports only ffi::{Kind, YmfmChip} with no register(), and the vgms-core dependency (Cargo.toml:16) is unused (zero vgms_core references in src/). But 'the removal never happened' misreads the record: CORES-REUSE-PLAN.md:11's 2026-07-29 owner-decision block says 'ru-2 stays frozen. ymfm remains at its PoC' — deliberate, documented retention superseding the removal clause. What survives is the unused dependency and a lib.rs that calls itself 'the accuracy tier' without noting the freeze; downgraded to low.

**[LOW · consistency] `crates/vgms-web/Cargo.toml:95`** — vgms-synth-worklet is the only intra-workspace dependency that bypasses [workspace.dependencies]

Every other path dependency between crates goes through the root [workspace.dependencies] table (vgms-core, vgms-synth, vgms-ui, all four core providers, vgms-vgmtools, vgms-pack-archive...), but vgms-web declares `vgms-synth-worklet = { path = "../vgms-synth-worklet" }` inline at line 95. The surrounding comment block justifies keeping the wasm-only third-party deps (web-sys etc.) out of the table, but the worklet is a workspace crate like the rest.

*Fix:* Add vgms-synth-worklet to [workspace.dependencies] in the root manifest and use `vgms-synth-worklet.workspace = true` here.

**[LOW · confusing] `crates/vgms-synth/src/engine.rs:71`** — Doc comments across vgms-synth and vgms-core still attribute behaviour to the deleted dro_split/dro_player binaries

The standalone binaries were folded into the single vgmstudio executable (subcommands play/render/split), but doc comments still explain APIs in their terms: engine.rs:71 and :102 ('for dro_split's channel...'), split.rs:54 and :372 ('names dro_split gives their files'), wav.rs:109, :128 and :154 ('opt-in through dro_player --render --boost'), and vgms-core/src/analysis.rs:223 and :237 ('dro_split's --isolate-percussion'). These are the permissive, reusable crates whose rustdoc outsiders are most likely to read, and the names refer to programs that no longer exist anywhere in the tree.

*Fix:* Sweep the nine references, replacing dro_split/dro_player with 'the split subcommand' / 'vgmstudio render --boost' or a behaviour-first phrasing that names no binary.

**[LOW · confusing] `crates/vgms-cores-gpl/src/lib.rs:27`** — register()'s doc says Nuked-PSG sits 'behind the clean-room SN76489', a core that was deleted

The doc comment at lines 27-28 explains registration order as 'a core that should be a picker alternative rather than the default (Nuked-PSG, behind the clean-room SN76489) relies on the builtins registering first'. The clean-room tier no longer exists: CoreRegistry::with_builtins registers only the OPL row (vgms-synth/src/registry.rs:283-311, whose comment says 'the OPL row is the only registration left in here'), and Nuked-PSG actually sits behind libvgm's SN76489 -- which the crate's own test comment at lines 134-136 states correctly. The function-level doc gives a new reader a defaulting story that is wrong on both counts.

*Fix:* Reword lines 26-28 to say the alternative rows rely on vgms-cores-libvgm registering first (as install_cores orders it), matching the nuked_psg_is_offered test's description.

**[LOW · confusing] `crates/vgms-ui/Cargo.toml:44`** — Dev-dependency comment points at a workspace explanation for 'the 0.32 line' that pins 0.35

The egui_kittest dev-dependency comment says 'See the workspace manifest for why this is pinned to the 0.32 line', but the workspace manifest pins egui_kittest 0.35 (root Cargo.toml:66) and its rationale text never mentions 0.32 -- the comment survived two toolkit bumps.

*Fix:* Change the comment to say the pin moves in lock-step with egui/eframe (no version number), so it cannot rot again.

**[LOW · confusing] `Cargo.toml:142`** — Workspace manifest comment names the app icon as src/dt.ico; the file is src/vgmstudio.ico

The winresource comment (lines 139-142) says the icon is compiled from 'src/dt.ico' and 'the window/taskbar icon is set at runtime from the same src/dt.ico'. The actual asset is src/vgmstudio.ico, as vgms-app/build.rs:12-14 and src/bin/vgmstudio.rs:26 reference it -- the comment predates the VGM Studio rename.

*Fix:* Update the comment to src/vgmstudio.ico (or drop the filename and point at vgms-app/build.rs).

**[LOW · consistency] `crates/vgms-vgmtools/src/lib.rs:1`** — vgms-vgmtools is the only crate whose lib.rs lacks the SPDX-License-Identifier header

All thirteen other crates open lib.rs with '// SPDX-License-Identifier: ...' matching their Cargo.toml license key; vgms-vgmtools/src/lib.rs starts straight at the module doc. For the crate whose whole existence is a licensing decision (wrapping GPL-2.0 vgmtools), the missing tag is the odd one out.

*Fix:* Add '// SPDX-License-Identifier: GPL-2.0-or-later' as line 1.

**[LOW · consistency] `crates/vgms-app/build.rs:12`** — The root src/ directory survives only to hold two app assets reached via ../../ paths

After the Python removal, the top-level src/ contains just vgmstudio.ico and vgmstudio.ini, and the app crate reaches out of its own tree for them: build.rs:12-14 ('cargo::rerun-if-changed=../../src/vgmstudio.ico'), src/bin/vgmstudio.rs:26 (include_bytes!("../../../../src/vgmstudio.ico")), and docs/skinning/skin-engine.md:188 tells contributors that flipping the default theme 'also needs src/vgmstudio.ini'. A root directory named src/ that is not a source directory is a small standing trap for tooling and newcomers alike.

*Fix:* Move vgmstudio.ico and vgmstudio.ini into crates/vgms-app/assets/ (fixing the three references), and let the Python-era src/ directory disappear.

*Verifier narrowed this:* Core claim holds — root src/ contains only vgmstudio.ico and vgmstudio.ini, reached via ../../ paths — but the finding undercounts the references: vgms-core/src/config.rs:686 also does include_str!("../../../src/vgmstudio.ini") (the SHIPPED_INI test constant), so the move touches a fourth load-bearing reference, and the .ini's proposed home in crates/vgms-app/assets/ would have the permissive vgms-core crate reaching into an app crate — the fix needs a different home for the .ini than suggested. Severity stays low.

*Checked and dismissed here:* Three render_wav_* shorthands have no callers outside their own tests since RenderMix superseded them.

### Not-Invented-Here audit

**[MEDIUM · refactor] `crates/vgms-app/src/pack_zip.rs:30`** — Native and web pack-zip builders are line-for-line forks; the web one already has the unifying seam

crates/vgms-app/src/pack_zip.rs and crates/vgms-web/src/pack_zip.rs duplicate the whole builder: PackZipOutput, the build_pack_zip loop, the song branch of process_entry, gzip(), has_extension() and to_vgz_name() are near-identical in both crates (the web header even calls itself "the wasm-portable half of vgms-app's pack_zip"). The web version already abstracts the only genuinely target-specific song step behind a SongOptimizer trait; the sole remaining difference is the Image arm (oxipng native-only). The native header's stated rationale — "the native-only crates (zip, oxipng) live here" — is half-stale: zip demonstrably builds for wasm32 (vgms-web and vgms-pack-archive both use it, with a comment saying so), so only oxipng justifies anything, and it justifies a hook, not a fork. The forks have already drifted (on_progress heartbeat, anyhow::Result vs Result<_, String>, two PackZipOutput types with the same name).

*Fix:* Make the portable builder in vgms-web (or a small shared GPL crate beside vgms-pack-archive) the single implementation, add an image-optimizer hook alongside SongOptimizer, and have vgms-app supply the oxipng hook and vgms-web the log-a-note fallback. Delete the vgms-app copy.

**[LOW · bug-risk] `crates/vgms-pack-archive/src/lib.rs:121`** — Case-only rename can silently overwrite a distinct same-name-different-case entry

PackArchive's map is case-sensitive (line 33 comment) and open() will happily hold both "Song.VGM" and "song.vgm" if a zip contains both — zips allow that. rename() (lines 117-131) skips the contains_key collision check whenever from.eq_ignore_ascii_case(to), so renaming "Song.VGM" to "song.vgm" while a separate "song.vgm" entry exists silently replaces that entry's bytes. The case-only allowance was copied from the native NTFS decision tree, where the two names cannot coexist — the premise does not transfer to the zip-backed map, and the doc promise "fails rather than overwrites" is violated in exactly this corner.

*Fix:* In the case_only branch, still fail when `to` exists as a key distinct from `from` (i.e. check `to != from && self.entries.contains_key(to)` unconditionally); add a test with both casings present in one zip.

**[LOW · confusing] `crates/vgms-app/src/cli/play.rs:45`** — Comment claims a "1..=16 range check" but validate() enforces 0.25..=64

Both crates/vgms-app/src/cli/play.rs:45 and crates/vgms-app/src/cli/render.rs:38 say "Reuse the config's 1..=16 range check for the CLI override" above config.validate(). The actual check in crates/vgms-core/src/config.rs:458 is `(0.25..=64.0).contains(&self.audio.boost)` — the boost became a bidirectional 0.25..=64 factor (config.rs lines 56-62 document why) and these two comments were never updated. Anyone tuning CLI boost limits from the comment would be misled about both bounds.

*Fix:* Update both comments to "Reuse the config's 0.25..=64 range check", or drop the range from the comment so it cannot drift again.

**[LOW · confusing] `crates/vgms-core/src/config.rs:490`** — The output_backend legacy-key comment sits above the resampling block, not the code it describes

Lines 490-493 explain the `output_backend` legacy key ("Read it so an existing vgmstudio.ini keeps its hardware setting... Applied *before* the `core.*` keys so an explicit new-style choice wins"), but the `if let` directly beneath (line 494) reads `audio.resampling`; the actual output_backend lookup at line 505 has no comment. The resampling block was evidently inserted between the comment and its code. A reader skimming apply_ini attributes the migration semantics to the wrong setting.

*Fix:* Move the four-line comment down to sit immediately above the `lookup(&ini, "audio", "output_backend")` block at line 505.

**[LOW · consistency] `crates/vgms-synth/src/wav.rs:272`** — render_vgm_wav masks a broken never-cancelled invariant with unwrap_or_default; the OPL twin uses expect

render_uncancelled (line 203) enforces the "a render that is never cancelled always completes" invariant with .expect(...), but render_vgm_wav (line 272) writes `.map(|bytes| bytes.unwrap_or_default())` for the same always-true keep_going. If the invariant ever broke, the OPL path panics loudly while the VGM path silently returns an empty Vec — not even a valid WAV header — which a caller would write to disk as a zero-byte .wav. Same concept, two failure behaviours.

*Fix:* Use the same expect message in render_vgm_wav (or route it through a shared uncancelled wrapper) so both engines fail identically.

**[LOW · confusing] `crates/vgms-ui/src/widgets/table.rs:3`** — Module header says "Six columns" but the table builds five

The header comment (line 3) says "Six columns.", but show() adds exactly five Column entries (lines 65-69: Pos., Bank, Reg., Value, Description) and Editor::column_titles() returns [&'static str; 5] (crates/vgms-ui/src/editor.rs:1028). The sixth column was folded into the Description cell's hover text — line 107's comment even says "(formerly its own column)" — and the header count was not updated.

*Fix:* Change the header to "Five columns" (or drop the count) and mention the hover-text fold there.

**[LOW · confusing] `crates/vgms-synth/src/resample.rs:992`** — Tap-count test doc contradicts the adjacent perf test about the worst real ratio

the_tap_count_stays_bounded's doc (lines 991-992) opens with "The worst ratio this app meets is the NES APU's 40:1, and it must stay affordable", while the perf test directly above (lines 946-948) states in bold "The worst ratio in this app is 5.07:1, not 40:1" and explains the NES APU presents 55.9 kHz — and the tap-count test's own body comment (lines 1000-1002) agrees ("No core presents a ratio like this today... the bound is kept as a warning shot"). The first sentence is a leftover from before that discovery and now asserts the opposite of the two comments beside it.

*Fix:* Rewrite the doc's opening sentence to match the body: the 40:1 bound is a guard for a hypothetical future core, not a ratio the app meets.

**[LOW · idiom] `crates/vgms-pack-archive/src/lib.rs:43`** — PackArchive::open copies the entire zip before reading it

`zip::ZipArchive::new(Cursor::new(zip_bytes.to_vec()))` clones the whole archive into a fresh Vec even though ZipArchive only needs Read + Seek, which `Cursor<&[u8]>` provides. For a multi-song pack zip opened in the browser this doubles peak memory for the duration of the unpack (the entries are then copied out a second time into the BTreeMap). The same pattern appears in vgms-app/src/pack_zip.rs's test helper read_zip (line 183), where it is harmless.

*Fix:* Use `zip::ZipArchive::new(Cursor::new(zip_bytes))` borrowing the input slice; no other change needed.

**[LOW · terminology] `crates/vgms-synth/src/engine.rs:71`** — Retired binary names dro_split / dro_player persist in nine doc comments

The standalone binaries were folded into `vgmstudio split` / `vgmstudio render` (DEVELOPMENT.md documents the one-executable design), yet doc comments still attribute behaviour to them: crates/vgms-synth/src/engine.rs:71 and :102, crates/vgms-synth/src/split.rs:54 and :372, crates/vgms-synth/src/wav.rs:109, :128 and :154 (which even cites a flag spelling, `dro_player --render --boost`, that no longer parses), and crates/vgms-core/src/analysis.rs:223 and :237. New readers grep for a `dro_split` that does not exist.

*Fix:* Sweep the nine sites, replacing dro_split/dro_player with `vgmstudio split` / `vgmstudio render --boost` (or with the calling function's path where that is what is meant).

**[LOW · dead-code] `crates/vgms-core/src/io/dro.rs:22`** — WRITE_CHAR_OPL is a constant-false flag guarding a permanently dead branch

`const WRITE_CHAR_OPL: bool = false;` (line 22) exists only to feed `if WRITE_CHAR_OPL { out.push(...) } else { ...four bytes... }` in write_v1 (lines 139-143). The true arm can never execute; the doc comment on the constant already states the decision ("writing always uses four bytes"), and the reader-side one-byte detection at lines 76-89 is what actually matters. A vestigial feature flag that no cfg, test, or caller ever flips is noise pretending to be configurability.

*Fix:* Delete the constant and the `if`, keeping the four-byte write and moving the historical note ("v1 headers were once written with a one-byte type; we always write four") onto write_v1.

**[LOW · dead-code] `crates/vgms-synth/src/decompress.rs:27`** — The Compression enum is re-exported from vgms-synth but appears in no public signature

`pub enum Compression` (line 27) is matched only inside decompress() (lines 185-193, 223-232); the public API `decompress(payload, table)` neither accepts nor returns it, and a workspace grep finds no `vgms_synth::Compression` consumer. Its re-export in crates/vgms-synth/src/lib.rs:40 is inert API surface a downstream user can import but never obtain or use — while its private sibling `Recovery` correctly stays module-local.

*Fix:* Drop Compression from the lib.rs re-export and demote it to a private enum (or module-private like Recovery); nothing outside decompress.rs can notice.

**[LOW · consistency] `crates/vgms-ui/Cargo.toml:43`** — dev-dependency comment pins egui_kittest to "the 0.32 line"; the workspace pins 0.35

crates/vgms-ui/Cargo.toml lines 42-43 say "See the workspace manifest for why this is pinned to the 0.32 line", but the workspace Cargo.toml declares eframe/egui/egui_extras/egui_kittest all at 0.35 (with its own lock-step comment). The pointer is correct, the version it quotes is two lines stale — exactly the kind of drift the workspace comment exists to prevent.

*Fix:* Reword to omit the number: "pinned in lock-step with egui/eframe — see the workspace manifest".

**[LOW · maintainability] `DEVELOPMENT.md:84`** — DEVELOPMENT.md's subcommand-shadowing note lists `convert`, which is no longer a subcommand, and omits two that are

Line 84-85 warns "A file whose name is exactly `play`, `render`, `split`, `convert` or `help` parses as a subcommand." The Command enum in crates/vgms-app/src/cli/mod.rs:47-59 has no Convert variant (conversion is GUI-only, as line 71-72 of the same doc says), so `vgmstudio .\convert` would actually open the file; meanwhile `optimize` and `retrowave-probe` do shadow file names and are not listed. The doc both over- and under-warns.

*Fix:* Update the list to play/render/split/optimize/retrowave-probe/help, or phrase it as "any subcommand name" and let `vgmstudio help` be the authoritative list.

*Checked and dismissed here:* render_wav_muted, render_wav_muted_with_progress and render_wav_boosted have no non-test callers and stale docs.

### vgms-retrowave — hardware output

**[LOW · bug-risk] `crates/vgms-retrowave/src/chip.rs:148`** — materialize's NEW pre-raise ignores bank-1 key registers, so those writes can be swallowed and then marked as sent

The doc comment directly above (chip.rs:141-142) says "NEW goes on first if any bank-1 register needs writing — the chip ignores that whole array while NEW is clear", and PLAN.md §3.4 step 1 says the same ("bank 1 must be writable before bank-1 diffs"). The code says something narrower: `needs_bank_one` is `(0..=u8::MAX).any(|reg| reg != NEW_REGISTER && !is_key_register(reg) && self.differs(1, reg))`. The `!is_key_register(reg)` term excludes 0xB0..=0xB8 and 0xBD. NEW is also skipped by the main diff loop (line 156), so this guard is the *only* place NEW is raised early. If a materialize finds every bank-1 non-key register already matching `hw` but a bank-1 key register differing, and `hw[1][0x05]` is 0, the key loop at chip.rs:165-171 emits (Bank::One, 0xB0.., value) while NEW is still clear. A real YMF262 discards those writes, but `emit` (chip.rs:115) unconditionally stamps `hw[1][reg] = Some(value)` — so the model now believes the note was keyed on, `differs` returns false forever after, and the final block (chip.rs:173-176) sets NEW afterwards. The channel-10..18 note is silently lost for the rest of the session. Reachability is narrow (it needs the bank-1 operator registers to coincidentally already match, which is most plausible right after the first materialize when `hw` holds the reset image and the song enables OPL3 and keys a bank-1 channel without changing anything else there), but the failure is silent and unrecoverable, and no test covers a bank-1-key-only diff.

*Fix:* Drop the `!is_key_register(reg) &&` term so the guard reads "any bank-1 register other than NEW itself differs", matching the comment and the plan. Add a test: materialize once, then `write_reg(0x105, 0x01)` + `write_reg(0x1B0, 0x31)` only, materialize, and assert the (Bank::One, 0x05, 0x01) write precedes the (Bank::One, 0xB0, 0x31) write.

*Verifier narrowed this:* Code reading is accurate: chip.rs:148-149 computes needs_bank_one as `reg != NEW_REGISTER && !is_key_register(reg) && self.differs(1, reg)`, so a materialize whose only outstanding bank-1 diff is a key register (0xB0..=0xB8 / 0xBD) skips the pre-raise, emits the key write at chip.rs:165-171 with NEW still clear, stamps hw at chip.rs:115, and only then sets NEW at 173-176. That contradicts both the doc comment (chip.rs:140-142) and PLAN.md §3.4 step 1 ("bank 1 must be writable before bank-1 diffs"), and the one-token fix in the suggestion is correct. Two claims in the write-up are wrong, which is why severity drops to low. (a) "lost for the rest of the session" — false. `differs` only gates materialize; `write_reg_buffered` (chip.rs:249-257) emits unconditionally, so the very next buffered write to that register (the note's own key-off, or the next key-on) lands with NEW now on. The loss is one note, not the channel. (b) The reachable-and-harmful window is narrower than "narrow". It needs hw[1][0x05] != Some(0x01) while shadow[1][0x05] has bit 0 set AND every bank-1 non-key register already matching hw. hw[1][0x05] can only be Some(0x00) if a previous materialize concluded with the song wanting NEW off, or the song wrote 0x105=0 on the playback path; a song that then turns NEW on to use bank 1 will also change bank-1 operator registers, which re-arms the guard. In the one case the reviewer names (right after the first materialize, hw holding the zeroed reset image), "every other bank-1 register already matches" means 0x1C0 = 0x00, i.e. both OPL3 speaker bits clear, so the swallowed key-on would have been silent anyway. Worth fixing as a defensive one-liner plus the missing test, not as a live defect.

**[LOW · bug-risk] `crates/vgms-retrowave/src/player.rs:105`** — Thread spawn failure panics the caller on the production path, taking the open port with it

`RetroWaveAudio::new` ends the builder chain with `.expect("spawning a thread")`. This is not a test-only path: `RetroWaveAudioService::load` (crates/vgms-app/src/services/retrowave.rs:78) calls it from the GUI thread, and `cli/play.rs:114` from the CLI. `new` also takes `device: Device` by value, so an OS refusal to spawn (thread/handle exhaustion) unwinds the UI thread while the just-opened serial port is mid-flight — the chip keeps sounding whatever it holds, which is exactly the state the crate's `catch_unwind` (player.rs:228) and Drop impl exist to prevent. The failure already has a home: `AudioService::load` returns `Result<(), String>`, and both callers report errors to the user.

*Fix:* Return `Result<Self, std::io::Error>` (or hand the `Device` back in the error variant so the port can be reused/silenced), and map it into the existing error string at the two call sites.

*Verifier narrowed this:* Call-site facts confirmed: player.rs:105 is `.expect("spawning a thread")`, and RetroWaveAudio::new is reached from vgms-app/src/services/retrowave.rs:78 (GUI, inside a `Result<(), String>` load) and cli/play.rs:114. The stated harm is refuted, though. The Device handed to `new` is always silent at that instant: `acquire_device` (retrowave.rs:50-63) either returns a freshly opened port, whose `Device::open` -> `initialise` runs reset_chip + mute sweep (device.rs:219-225), or a parked one that `RetroWaveAudio::into_device` mute-swept and flushed on the way out (player.rs:247-251). So "the chip keeps sounding whatever it holds" is not the failure mode. Nor is the port stranded: `thread::Builder::spawn` consumes and drops the closure on Err, so the moved `Device` (and its serialport handle) drops and the port closes. What actually remains is: an OOM/handle-exhaustion-class spawn failure panics the egui update thread instead of returning the error string both callers already render — a genuine but very-low-probability robustness nit, and the same behaviour `std::thread::spawn` has by design. Medium is too high.

**[LOW · dead-code] `crates/vgms-retrowave/src/commands.rs:88`** — `reset_value` has no caller outside its own test, and the reset image it exists for was never wired into the chip

`pub const fn reset_value` is documented as "The value a register holds after a chip reset, for the diffing chip's benefit", but grepping the whole workspace (crates/, tools/, tests/, web/, examples/) finds no caller other than `commands.rs:230`'s own assertion. `SerialOpl3Chip` derives its target from `translate(shadow)` (chip.rs:123, 128), and after `reset()` zeroes the shadow that means total-level registers 0x40..=0x55 reconstruct as 0x00 (full volume), not the 0xFF that `reset_value`/`queue_mute_sweep` define as silence — which is the behaviour PLAN.md §3.4 specified ("registers known in hw but zero in shadow are written to their reset value (with 0x40..=0x55 → 0xFF)") and §8 does not list among the recorded divergences. It is inaudible today because key bits are off in the same image, so this is dead code plus an unrecorded plan divergence rather than a live defect. `is_total_level` (commands.rs:22) is likewise `pub` with no user outside this module.

*Fix:* Either use it — have `SerialOpl3Chip` diff against `commands::reset_value(reg)` for registers the shadow has never written — or delete `reset_value`, demote `is_total_level` to private, and add a line to PLAN.md §8 saying the reset image is the zeroed shadow.

*Verifier narrowed this:* Both halves check out factually. Workspace-wide grep for `reset_value` returns only its definition (commands.rs:88) and its own test (commands.rs:230-234) — no caller in crates/, tools/, tests/, web/, or examples/. `is_total_level` is `pub` and used only by `silent_value` (commands.rs:28) inside the same module, exactly as claimed. The plan divergence is real and unrecorded: PLAN.md §3.4 says "registers known in `hw` but zero in `shadow` are written to their reset value (with `0x40..=0x55` -> `0xFF`)", the code instead emits `translate(shadow)` (chip.rs:123, 128) so a zeroed shadow reconstructs TL as 0x00, and §8's "Design changes from the plan" list does not mention it. Severity lowered to low: the divergence is inert (a zeroed shadow also zeroes 0xB0/0xC0, so nothing is keyed or routed to a speaker), the code arguably cannot honour the plan as written since `shadow` is `[u8;256]` with no "never written" state to distinguish from a genuine 0x00, and the remainder is one unused `pub const fn` plus an over-public helper. Documentation/cleanup, not a defect.

**[LOW · confusing] `crates/vgms-retrowave/src/device.rs:229`** — Two comments promise an IC reset when the port closes; nothing performs one

`reset_chip`'s doc says it "belongs to opening and closing a port, not to seeking or loading a song", and player.rs:249 declines a reset on the error path because "a hard reset belongs to closing the device rather than unloading a song". PLAN.md §3.6 ("App exit / backend switch") promises "the owner then IC-resets and closes the port". But `Device` has no `Drop` impl, `reset_chip` is called only from `initialise` (device.rs:223), and the only close path — `RetroWaveAudioService::release_device` (crates/vgms-app/src/services/retrowave.rs:44-47) — just assigns `self.device = None`. Harmless in practice (the pump's mute sweep leaves the chip silent, and the next `open` resets it), but three separate places describe a close-time behaviour the code does not have, which is the kind of thing a later change will 'preserve'.

*Fix:* Either give `Device` a `Drop` that queues `queue_chip_reset` best-effort (no settle sleep needed on the way out), or reword the two comments and §3.6 to say the mute sweep is what silences the board and the reset is paid only on open.

**[LOW · dead-code] `crates/vgms-retrowave/src/device.rs:215`** — `Device::port_name()` and the field backing it have no caller

`Device` stores `port_name: String` (device.rs:179) and exposes `pub fn port_name(&self) -> &str`. Grepping the workspace for `.port_name()` returns no hits — every `port_name` use in vgms-app is the `PortInfo` field, and the error text in `Error::Open` carries its own copy of the name. `with_io` forces every test and the `dump_wire` example to invent a placeholder ("MOCK", "DEAD", "capture") for a value nothing reads. It survives only as `#[derive(Debug)]` output.

*Fix:* Either drop the field and the `port_name` parameter of `with_io`, or use it — e.g. include it in `Error::Write` ("lost contact with the RetroWave device" currently never says which port), which is the more useful of the two.

**[LOW · complexity] `crates/vgms-retrowave/src/player.rs:343`** — The pump wakes ~780 times a second for the whole time a song is loaded, even paused or finished

`pump_loop` advances `deadline += QUANTUM` and sleeps every iteration regardless of state, so with `playing == false` (the state a loaded-but-not-playing song sits in indefinitely — `RetroWaveAudio::new` starts paused) the thread still wakes every 1.287 ms to poll an empty `rtrb` queue and call `flush`, which returns early on an empty wire. That is ~776 timer waits and context switches per second doing nothing. The quantum is only load-bearing while the engine is being stepped; command latency has no such requirement (a control command is a UI action).

*Fix:* When `!playing`, sleep a coarser interval (say 10-20 ms) and re-base `deadline = Instant::now()` on the transition back to playing — the absolute-deadline pacing only needs to hold across a playing run.

*Verifier narrowed this:* Arithmetic and control flow confirmed: QUANTUM = 64/49716 s = 1.287 ms (~777 iterations/s), `deadline += QUANTUM` plus the sleep at player.rs:343-352 runs unconditionally, `stop` is only set by `shutdown`, and `RetroWaveAudio::new` starts with `playing = false`, so the thread does spin at the quantum for as long as a song is loaded on the hardware backend, paused or finished. But the impact is overstated. An idle pass is a failed `rtrb::pop`, `chip.seal()` on an empty payload (protocol.rs:143-148, a single `is_empty` check), a `wire().is_empty()` early return (player.rs:365), `engine.is_finished()`, and two relaxed atomic stores — sub-microsecond, so the cost is the timer wakeups themselves, not CPU, and Rust's Windows sleep uses a per-thread high-resolution waitable timer rather than raising the global timer resolution. The paused path is also not idle by accident: it is what delivers `release_all_notes` writes, mute/pan changes and the `stop` signal promptly (see the comment at player.rs:313-314 and the `a_paused_seek_sends_nothing_until_playback_resumes` / `scrubbing_while_paused_never_restarts_the_note` tests), so the suggested two-rate loop adds a deadline-rebasing edge to a loop whose uniformity is currently load-bearing. Real but marginal; low is right, and it is efficiency rather than complexity.

*Checked and dismissed here:* "seal, send, clear_wire" is written three times, once via a mem::take borrow dance; "mute" names two different things in a crate that uses both.

### vgms-audio-native — the cpal backend

**[HIGH · bug-risk] `crates/vgms-audio-native/src/lib.rs:561`** — cpal stream errors are logged and dropped — the app has a plumbed `last_error` channel this backend never feeds

The stream's error callback is `|error| log::error!("audio output stream error: {error}")`. Nothing else records it: `SharedState` (lines 63-87) has no error slot and no "the stream died" flag, and `NativeAudio` exposes no accessor, so `NativeAudioService` (crates/vgms-app/src/services/audio.rs) cannot implement `AudioService::last_error` and falls through to the trait's `None` default (crates/vgms-ui/src/platform.rs:505). A `StreamError::DeviceNotAvailable` (headphones unplugged, device removed, driver reset mid-song) therefore kills playback with zero UI feedback: the callback stops running, so `frames_rendered`/`finished` freeze at their last values, `NativeAudioService::playing` stays `true`, and `is_playing()` keeps returning true — the transport still reads "playing" with a stationary cursor and no toast. Both sibling backends do this properly: `vgms-retrowave/src/player.rs:70` keeps `error: Mutex<Option<String>>` *plus* `stopped: AtomicBool` explicitly "so the transport can leave the 'playing' state instead of showing a frozen cursor" (surfaced at crates/vgms-app/src/services/retrowave.rs:198), and the web service records worklet faults into `last_error` (crates/vgms-web/src/services/audio.rs:182). `vgms-ui/src/app.rs:1654` already polls `last_error()` every tick and raises the "playback stopped" alert — so the whole path exists and only the primary audio backend is silent on it.

*Fix:* Mirror `RetroWaveAudio`'s `SharedState`: add `error: Mutex<Option<String>>` (only ever touched from the error callback, which is not the data callback, so the no-lock rule is untouched) and a `stopped: AtomicBool`, expose `NativeAudio::take_error()` and have `is_finished()`/a new `is_stopped()` reflect it, then implement `NativeAudioService::last_error` by forwarding. That alone makes an unplugged device produce the existing toast instead of a frozen cursor.

**[LOW · bug-risk] `crates/vgms-audio-native/src/lib.rs:123`** — Sample format is read only from the default config, so a device whose default is not f32/i16 is refused outright

`sample_format` comes solely from `device.default_output_config()` (line 123), and anything other than `F32`/`I16` returns `AudioError::UnsupportedFormat` (line 245) — playback fails with "the device's sample format ... is not supported" and no fallback. cpal 0.18's `SampleFormat` also has `I8/I24/I32/I64/U8/U16/U24/U32/U64/F64`, and hosts do pick those as the default (ALSA commonly reports S32/U16 defaults; the default config is whatever the driver leads with, not the best match for us). The device may well advertise an f32 stereo config in `supported_output_configs()` — which this same function already enumerates twice for the rate and buffer size — and we would still refuse it. Related coherence gap: the negotiated triple is assembled from two different sources. `sample_rate` and `buffer_size` come from a scan constrained to `channels() == 2` (lines 579-583, 605-608), while `sample_format` comes from the default config, which may describe a different channel count entirely; nothing checks that any single supported config actually offers all three.

*Fix:* Do one scan of `supported_output_configs()` for a stereo range covering the wanted rate and take the format from *that* range (preferring `F32`, then `I16`), falling back to the default config's format only when no stereo range matches. `UnsupportedFormat` then means the device genuinely offers neither, rather than "its default happened to be i32".

*Verifier narrowed this:* The code reading is right (format solely from default_output_config at line 123; anything but F32/I16 errors at line 245; the rate/buffer scans are stereo-constrained while the format is not), but the mechanism the finding blames is wrong, and that shrinks the risk a lot. cpal 0.18 does not hand back 'whatever the driver leads with': ALSA's default_config sorts by SupportedStreamConfigRange::cmp_default_heuristics and takes the greatest (host/alsa/mod.rs:603-620), and that heuristic ranks stereo first and then F32 highest of all formats (lib.rs:824-848, rank F32=14 > F64 > I32 > U32 > I24 > U24 > I16), so 'the device advertises a stereo f32 config yet we refuse it' is impossible there — if a stereo F32 range exists, the default already IS it. WASAPI returns the shared-mode mix format (GetMixFormat, host/wasapi/device.rs:762), float in practice, and CoreAudio-iOS/webaudio/aaudio use the same max-by-heuristics. 'ALSA commonly reports S32/U16 defaults' is not true of this crate version. The residual reachable case is narrow: an ALSA hw-style stereo device with no FloatLE whose best format is I24/I32/U24/U32/F64 while it also supports S16 — refused today, playable with the suggested rescan. The coherence sub-claim is factually true but harmless: the StreamConfig always requests channels:2 anyway, and both scan misses degrade to benign fallbacks (default rate, BufferSize::Default). Downgraded to low.

**[LOW · refactor] `crates/vgms-audio-native/src/lib.rs:391`** — The `Engine` dispatch enum and the render/limit/meter sequence are duplicated in vgms-synth-worklet, with only the web copy under test

`enum Engine` (lines 391-476) is reproduced almost line for line as `vgms-synth-worklet/src/player.rs:316-420` — same two variants, same `render`/`seek_to_ms`/`seek_to_pos`/`rewind`/`set_loop`/`position`/`is_finished` arms, same OPL-arm-only `set_muting`/`set_panning` and Vgm-arm-only `set_chip_muting`/`set_chip_panning` no-ops, and the worklet's `Engine::build` (player.rs:322-348) repeats the OPL core-choice/`core_for_realtime` construction from `NativeAudio::build` (lines 166-206). `channel_peaks` exists twice too (lines 617-625 returning `(u32, u32)`; player.rs:302-310 returning `(u16, u16)`), as does the boost→limit→`min_engaged_boost` ratchet→peak sequence (lines 528-542 vs player.rs:253-262). The worklet file itself documents the copy: "the worklet's counterpart of `vgms-audio-native`'s private `Engine`". Both crates already depend on `vgms-synth`, where the shared half could live. Two consequences: (a) any engine-level change (a seventh method, a new command, a change in limiter/peak ordering) must be made twice with nothing enforcing it; (b) the worklet's copy is unit-tested (player.rs:468+ drives render/seek/peak/limiter through plain Rust), while the native copy lives inside the `build_output_stream` closure and is reachable only with a real audio device — this crate's tests cover only `channel_peaks`, `clamp_buffer_size`, and one `Engine` arm.

*Fix:* Lift `Engine` (plus its construction from `AudioSource` and `channel_peaks`) into `vgms-synth` as a shared, testable playback front-end, and reduce the cpal closure and `WebPlayer::render` to device-specific skins over it. The native callback body then becomes testable the way `WebPlayer` already is.

*Verifier narrowed this:* The duplication is real and the line ranges are right: vgms-synth-worklet/src/player.rs:316-422 mirrors Engine's two variants and every arm (including the OPL-only/Vgm-only no-ops), player.rs:253-262 repeats the boost->limit->min_engaged_boost ratchet->peak order, and channel_peaks exists at player.rs:302-310 and lib.rs:617-625. But the finding oversells on three counts. (a) The title 'only the web copy under test' is contradicted by its own detail — lib.rs:655-690 does drive Engine::Vgm through a real core and asserts the OPL no-op does not panic. (b) The copies are not line-for-line: the worklet's render returns the produced-frame count and writes planar f32, the native one converts interleaved through a generic `T: SizedSample`; channel_peaks returns u32 natively on purpose, because the value feeds AtomicU32::fetch_max. (c) Engine::build genuinely differs — the native one also falls back to `config.core(OPL_SLOT)` and reads config.resampling, the worklet takes ResampleMode as a parameter and consults only the registry. The suggested home is also not free: both crates are GPL-2.0-or-later while vgms-synth is MIT OR Apache-2.0 and deliberately dependency-light, so lifting playback front-end code there is a licence/scope decision, not a mechanical move. Real but low-severity refactor.

**[LOW · consistency] `crates/vgms-audio-native/src/lib.rs:574`** — `supported_rate` and `resolve_buffer_size` duplicate the stereo-config scan and disagree about enumeration errors

Both functions run the identical predicate over `device.supported_output_configs()` — `channels() == 2 && min_sample_rate() <= rate && rate <= max_sample_rate()` (lines 579-583 and 605-608) — differing only in what they extract. They also disagree on failure: `supported_rate` propagates the enumeration error with `?` (line 604), so a device that cannot list its configs makes `NativeAudio::new` fail outright, while `resolve_buffer_size` swallows the same error via `.into_iter().flatten()` (line 578) and quietly falls back to `BufferSize::Default`. One call path, two policies for one failure. `NativeAudio::new` also interrogates the device three separate times (`default_output_config`, then the two scans), and a fourth predicate on the same data would be needed to fix the sample-format gap.

*Fix:* Replace both with one `fn stereo_config(device, rate) -> Result<Option<SupportedStreamConfigRange>, cpal::Error>` and derive the rate acceptance, buffer range, and (see the sample-format finding) the format from the single returned range. Pick one error policy — falling back to the host default on an enumeration error is the more forgiving and matches the buffer-size path.

*Verifier narrowed this:* Factually correct: lines 579-583 and 605-608 run the identical `channels()==2 && min<=rate<=max` predicate, supported_rate propagates the enumeration error via `?` (line 604) while resolve_buffer_size swallows it with `.into_iter().flatten()` (lines 576-578), and new() does interrogate the device three times. What weakens it is that the 'two policies for one failure' divergence is close to unreachable: line 114 already does `device.default_output_config()?`, and on the hosts in play that call sits on the same underlying enumeration — ALSA's default_config literally calls supported_configs first (host/alsa/mod.rs:604), WASAPI's default_format and supported_formats share ensure_future_audio_client + GetMixFormat — so a device that cannot list its configs has almost always failed at line 114 before either scan runs. That leaves a genuine but modest 5-line duplication between two adjacent private helpers, which is a tidy-up rather than a medium-severity consistency defect. Downgraded to low.

**[LOW · maintainability] `crates/vgms-audio-native/src/lib.rs:210`** — The command queue's 64-slot capacity is a bare literal whose overflow contract is documented only in another crate

`rtrb::RingBuffer::<Command>::new(64)` carries no comment here, and `send` (lines 369-373) handles overflow by logging a warning and discarding the command. Because every `Command` is an absolute state replacement with no retry, a dropped `SetMuting`/`SetChipMuting`/`SetBoost`/`SetLoop` leaves the audio permanently disagreeing with the UI — the channel shows muted and keeps sounding — until the user happens to change that same setting again. The reason this is survivable is written down in a *different* crate: `crates/vgms-app/src/services/audio.rs:8-13` explains that "the stream's 64-slot command queue is only drained by the audio callback, which a paused cpal stream never runs", and builds the whole pause-deferral design (`stream_live`, `pending_seek`, the flush-on-play in `play()`) on that fact. Nothing in this crate states the invariant its own consumer depends on, and the literal `64` appears in prose there and again as a literal in `vgms-retrowave/src/player.rs:94` with no shared constant, so raising or lowering it here silently falsifies a sibling module's documentation.

*Fix:* Name it — `const COMMAND_SLOTS: usize = 64;` — and document on it that the queue drains only while the stream is running, that overflow drops absolute-state commands with no recovery, and that callers must defer while paused. Referencing that doc from `NativeAudioService`'s module comment turns the coupling into something a reader can follow in one hop.

*Verifier narrowed this:* The observations check out: line 210 is a bare `rtrb::RingBuffer::<Command>::new(64)` with no comment, send() (369-373) logs and discards, the invariant really is written down only in the consumer crate (services/audio.rs:8-13), and the same literal reappears unshared at vgms-retrowave/src/player.rs:94. But the stated consequence is materially overstated. Overflow needs 64 commands queued between two callback runs, and the queue is drained at the top of EVERY callback (line 502) — a few ms apart — while every setter in NativeAudioService is gated on stream_live() so nothing is pushed at all while paused, and play() flushes at most seven. 'Leaves the audio permanently disagreeing with the UI' is therefore not reachable from user input; the finding half-concedes this by explaining why the design survives. What is left is a naming/doc nit (name the constant, state the drain-only-while-running contract locally) with no behavioural risk, so medium is too high.

*Checked and dismissed here:* "Nothing locks in the audio path" reads as an invariant, but the callback touches the allocator on two paths the file itself documents.

### vgms-core — the DRO format layer

**[LOW · bug-risk] `crates/vgms-core/src/io/dro.rs:84`** — The v1 one-byte OPL-type heuristic ignores byte_length, the field that would settle it

read_v1 distinguishes an old one-byte OPL type from a modern four-byte one purely by `opl_type_code > 0xFF` (dro.rs:83-89). The comment at dro.rs:79-81 admits the failure mode -- "It *can* go the other way, if a one-byte type is followed by three zero bytes" -- but records it rather than resolving it, and nothing downstream compensates.

Concretely: a legitimate old rip (the class the code exists to support -- see the test comment at dro.rs:398 citing adplug's samurai.dro) whose stream begins `0x00 0x00 0x00` (a 1 ms short delay followed by another delay opcode) reads as a four-byte type of value 0/1/2, swallowing three stream bytes. Two outcomes, both wrong: with no trailing slop, `reader.remaining()` is now `byte_length - 3` and the file is rejected at dro.rs:96 with the misleading "header declares N bytes of data, but only N-3 remain"; with >=3 bytes of trailing slop (which dro.rs:104-112 deliberately tolerates) the take succeeds and the whole instruction stream is decoded three bytes out of phase -- silent corruption, no warning.

A second, smaller case: a one-byte-type file with fewer than 3 bytes of data makes `reader.u32_le()` at dro.rs:83 fail outright, so the file cannot open at all.

The disambiguator is already in hand and unused: `byte_length` was read two lines earlier. After the four-byte interpretation, `reader.remaining()` should be `byte_length` (plus optional slop); after the one-byte interpretation it should be `byte_length + 3`. There is also no test for the ambiguous stream -- `v1_bytes()` (dro.rs:244) always starts its data with `0x20 0x01`, so both existing v1 tests take the unambiguous path, and the repo has no real v1 fixture (tests/ holds only lsl3_score_up_dro2.dro).

*Fix:* Choose the interpretation whose leftover length matches the declared `byte_length`: read the u32, and if `reader.remaining() != byte_length` (or, more tolerantly, `< byte_length`), seek back and take one byte instead, falling back to the current magnitude heuristic only when neither matches exactly. Add a v1 test whose data begins `0x00 0x00 0x00` after a one-byte type, and one whose data is a single 2-byte register write.

*Verifier narrowed this:* Mechanism confirmed at crates/vgms-core/src/io/dro.rs:82-101. `opl_type_code > 0xFF` is the only discriminator, `byte_length` (read at :73) is never consulted, and ByteReader::u32_le fails cleanly without advancing (io/mod.rs:101, :79-90), so the <3-bytes-of-data sub-case does abort the open. Trigger is narrower than the writeup implies, hence the downgrade: the four-byte read only aliases when the *next three* stream bytes are all 0x00 (u32_le = t | b0<<8 | b1<<16 | b2<<24 <= 0xFF requires b0=b1=b2=0), i.e. an old one-byte-type rip whose stream starts 0x00 0x00 0x00 -- a 1 ms short delay immediately followed by a second short-delay opcode, which the DOSBox writer coalesces rather than emits (a leading long delay 0x01 .. reads correctly). The silent-corruption variant additionally needs >=3 bytes of trailing slop, so it is a compound rarity; the common outcome is a clean (if misleading) refusal at :96, not corruption. Our own writer can never produce the ambiguous form (WRITE_CHAR_OPL is false), and the repo has no v1 fixture (tests/ holds lsl3_score_up_dro2.dro and lsl3_score_up.vgm; v1_bytes() at dro.rs:244 always starts 0x20 0x01), so the untested-path claim stands. One caveat on the suggested fix: it is itself ambiguous -- a genuine four-byte-type file with exactly 3 trailing slop bytes also satisfies `remaining == byte_length + 3`, so any byte_length disambiguation must prefer the exact four-byte match first.

**[LOW · consistency] `crates/vgms-core/src/io/dro.rs:129`** — The v1 writer recomputes the header length while the v2 writer preserves it, silently discarding a user edit

write_v1 ignores `song.ms_length` and writes `song.total_delay_ms()` (dro.rs:129-137); write_v2 writes `song.ms_length` verbatim (dro.rs:222). The comment offers the divergence as its own justification -- "recompute it, because V1 and V2 files write this value differently" -- which is circular: both containers store the same quantity (a length in milliseconds), and nothing in the format demands different treatment.

This is user-visible. The DRO Info dialog exposes "Length (MS)" as an editable field with no version gate (crates/vgms-ui/src/dialogs/dro_info.rs:94-100), saving it through the undoable `UpdateHeader` (crates/vgms-core/src/undo.rs:397-401). On a v2 song the edit survives a save; on a v1 song it is thrown away on the very next save, while the dialog still reports "updated". The same asymmetry applies to the mismatch the loader reports (`report.delay_mismatch = song.ms_length != song.total_delay_ms()`, crates/vgms-ui/src/editor.rs:428): a v1 file's mismatch is silently normalised away on save, a v2 file's is preserved.

Every other edit path keeps `ms_length` in step for both versions -- DeleteInstructions subtracts the removed delay (undo.rs:214-216), crop recomputes it (crop.rs:118, crop.rs:171) -- so the stored field is *not* stale in normal use, which removes the only plausible reason for the v1 recompute. The two behaviours are pinned by tests (dro.rs:441 and dro.rs:467) that assert the divergence without explaining it.

*Fix:* Pick one policy for both versions and state the reason. Either trust the maintained header field in both writers (v1 then matches v2 and the DRO Info edit means something everywhere), or recompute in both and make the dialog's Length field read-only/derived. If v1 must keep recomputing for a format reason, replace the circular comment with that reason.

*Verifier narrowed this:* Every fact checks out: write_v1 uses song.total_delay_ms() (dro.rs:129-137) while write_v2 writes song.ms_length verbatim (dro.rs:222); the divergence is pinned by tests at dro.rs:441 and :467; DRO Info's "Length (MS)" is version-agnostic (vgms-ui/src/dialogs/dro_info.rs:94-100 -> save() at :143 -> Action::UpdateHeader -> undo.rs:397-401); editor.rs:428 computes delay_mismatch for both versions; DeleteInstructions (undo.rs:214-216) and crop (crop.rs:118, :171) keep ms_length in step, and convert::dro2_to_dro1 even carries ms_length across (convert.rs:305, asserted at :508) only for write_v1 to discard it. Severity lowered because the user-visible path is gated twice over: the editable field only appears when ui.dro_info_edit_enabled is set, and that is off by default (config.rs:385, src/vgmstudio.ini:31, settings.rs:767 "off by default"). Outside that opt-in expert setting the recompute and the stored field agree, so in default use the divergence shows only as a v1 file's pre-existing header lie being normalised on save while a v2 file's is preserved. This is a design-policy call plus a circular comment, not a defect.

**[LOW · dead-code] `crates/vgms-core/src/io/dro.rs:22`** — WRITE_CHAR_OPL is a permanently-false const whose branch can never run

`const WRITE_CHAR_OPL: bool = false;` (dro.rs:22) is never assigned anywhere else -- grep over crates/, web/, tests/ and tools/ finds only its definition and the single `if WRITE_CHAR_OPL` at dro.rs:139 -- so `out.push(song.opl_type.v1_code())` at dro.rs:140 is unreachable in every build and configuration. It is a leftover from the Python port: commit faae8e1 removed the trailing "as the Python's `WRITE_CHAR_OPL = False` did" from the doc comment but kept the flag, and with the Python gone the name no longer refers to anything. The doc comment above it already states the actual behaviour ("writing always uses four bytes"), so the branch adds nothing but a second, contradicting source of truth about what the writer does. The test `v1_char_opl_type_is_upgraded_on_write` (dro.rs:431) pins the four-byte behaviour, and there is no test for the flipped flag.

*Fix:* Delete the const and the `if`, keeping just the four-byte write and the doc sentence that explains why a one-byte type is upgraded on write.

**[LOW · refactor] `crates/vgms-core/src/io/dro.rs:173`** — The 128-entry codemap cap is enforced in two places with a duplicated message, and hard-coded in a third

read_v2 rejects `codemap_length > 128` with the message "DRO v2 file has too many entries in the codemap. Maximum 128, found {}. Is the file corrupt?" (dro.rs:173-178). DroDataV2::new performs the identical check with a verbatim copy of that message (crates/vgms-core/src/song/dro_data.rs:223-229). The literal 128 appears a third time in write_v2's debug_assert, whose text already names the real enforcer: "`DroDataV2::new` caps the codemap" (dro.rs:214).

The reader's check is redundant -- `codemap_length` comes from a u8 so it is at most 255, `reader.take` is bounds-checked, and DroDataV2::new (called at dro.rs:203) would reject the same input with the same wording. Its only effect is that the rule, the bound and the user-facing string must now be kept in lockstep across two files with nothing enforcing that they stay identical. The cap is also the semantic partner of the `code & 0x7F` masking in DroDataV2::get (dro_data.rs:309), which is stated nowhere near either check.

*Fix:* Name the bound once (e.g. `const MAX_CODEMAP_ENTRIES: usize = 128;` beside DroDataV2, with a comment tying it to the 7-bit code field), have DroDataV2::new be the single point that rejects and formats the message, and drop the duplicate check in read_v2.

*Verifier narrowed this:* The literal duplication is real (dro.rs:173-178 vs song/dro_data.rs:223-229, verbatim message; 128 hard-coded a third time in the debug_assert at dro.rs:214), but the finding's central justification is wrong: the reader's check is NOT redundant. It runs before `reader.take(codemap_length)` (:183) and `reader.take(data_length)` (:196), so it is the only place that can produce the codemap diagnostic. Measured on the real fixture (746 bytes = 26 header + 122 codemap + 598 data, zero slop): with the reader check removed, the existing test v2_rejects_an_oversized_codemap (dro.rs:353, which sets codemap_length = 129) would take 129 bytes, leaving 591 < 598 for the data, and fail with ByteReader's generic "file ends unexpectedly" instead of "Maximum 128" -- DroDataV2::new would never be reached, and the test would break. Conversely, given the reader's guard, DroDataV2::new's copy can never fire from the read path; it guards direct API callers. So the suggested deletion regresses both the diagnostic and a test. What survives is the low-value nit of one duplicated string and an unnamed 128 -- worth a shared const, not a behaviour change.

*Checked and dismissed here:* DroDataV2::insert_many can break the codemap invariant that get() indexes unchecked.

### web/wasi-shim — the vendored WASI host

**[MEDIUM · bug-risk] `web/task_worker.js:20`** — task_worker.js swallows every failure; an init() rejection wedges the task kind busy forever

`self.onmessage` awaits `ready` (line 20) with no guard. If `init()` rejects — a failed `vgms_web_bg.wasm` fetch, a CSP/MIME problem, an instantiation error — the await throws out of the async handler as an unhandled rejection, so `self.postMessage("done")` (line 38) never runs. `WorkerTaskService` has no watchdog on this path: `done` stays false, `poll` never reaps the slot, and `is_busy_kind`/`is_busy` (crates/vgms-web/src/services/task.rs:216-223) report that kind busy for the rest of the session. Separately, a throw from `vgms_web_run_task` IS caught (lines 31-35) but only `console.error`'d, then "done" is posted anyway — the page sees a clean completion with zero results, so e.g. a failed waveform render is indistinguishable from an empty one. The sibling worker does this correctly: web/pack_worker.js:81-88 wraps the same `await ready` in try/catch and posts `{ error }`, which crates/vgms-web/src/services/pack.rs:201-207 turns into a `PackJobOutcome::Failed` the user actually sees. Same concept, two behaviours, and the weaker one has no watchdog behind it.

*Fix:* Mirror pack_worker: wrap `await ready` in try/catch and post an error message the page can decode, and post that message (rather than only `console.error`) when `vgms_web_run_task` throws. Give `TaskResult`/the message protocol a failure variant so `WorkerTaskService`'s handler can surface it, and always post "done" in a `finally` so the slot is reaped no matter what. While there, consider `worker.set_onerror` in both services/task.rs and services/pack.rs — a worker script that fails to load (a bad deploy dropping web/wasi-shim/) currently produces silence on the task path and a 3-minute watchdog timeout on the pack path instead of an immediate honest error.

**[MEDIUM · maintainability] `web/e2e/tests/pack.spec.js:73`** — Nothing in CI fails when the vendored shim breaks — every failure mode degrades silently to the built-in pass

web/wasi-shim/ is imported by exactly one file (web/pack_worker.js:18), and the only thing that executes it is the "exporting builds a release zip in the Worker" spec — the fixture is tests/lsl3_score_up.vgm, a YM2203 file, so it is not caught by the wholly-OPL bypass and the tools really do run. But the spec asserts only that the download ends in .zip, starts with "PK", and contains "01 Alpha.vgz"/"02 Beta.vgz". Every shim failure mode is deliberately non-fatal — a trap or missing hook becomes `ToolOutcome::Failed` and the pipeline moves on (crates/vgms-web/src/optimize_tools.rs:70-74), and a module that will not compile falls back to the built-in pass (crates/vgms-web/src/worker.rs:81-96) — so a completely broken shim produces the exact same passing assertions. The two CI gates that do assert something (.github/workflows/rust.yaml:141 and :145) run `wasmi_wasi` and `node:wasi`; neither imports web/wasi-shim, so the host that actually ships is validated by nothing. web/wasi-shim/README.vgm-studio.md:13 nonetheless instructs the upgrader to validate by "re-run the web e2e suite", and OPTIMIZER-WASM-PLAN.md:356-358 claims verification "under node over the exact vendored files" — there is no script in the repo that does that.

*Fix:* Cheapest fix that closes the loop: point tools/web/vgmtools_smoke.mjs at web/wasi-shim/index.js instead of `node:wasi` (it is plain ESM, node can import it directly, no browser needed), so the existing CI gate covers the shipped file. Additionally, have pack.spec.js assert on the export log's per-tool lines (exposing the log through the e2e snapshot if it is not already there) so a silent fallback to the built-in pass fails the suite rather than passing it. Then the README's upgrade recipe is actually true.

*Verifier narrowed this:* Conclusion stands -- nothing validates the shipped host -- but two supporting claims are wrong and the real gap is bigger. REFUTED: the fixture is not YM2203. helpers.js:9-11 points FIXTURE_VGM at tests/lsl3_score_up.vgm, whose header declares YM3812 @3579545 and nothing else, so pipeline.rs:212-218 takes the wholly-OPL bypass and skips all three tools -- __vgms_run_tool is never called by any spec, so the shim's runtime behaviour is exercised by zero tests, not by one weak one. REFUTED: 'a completely broken shim produces the exact same passing assertions' -- the import at pack_worker.js:18 is top level, so a missing or unparseable shim kills the worker module, no download arrives, and the 30s waitForEvent in pack.spec.js:76 fails the spec; only behavioural breakage is invisible. Minor: more than one spec exports (zip-pack.spec.js:60-74 does too). CONFIRMED and worth adding: the CI e2e job assembles its bundle with an explicit list, .github/workflows/rust.yaml:202 `cp web/index.html web/task_worker.js web/worklet-processor.js web/pack_worker.js target/web-dist/`, which never copies web/wasi-shim/ nor the tool_*.wasm modules -- so in CI the pack worker's import 404s and the export specs would fail outright (the branch is unpushed, so this has not run yet). Also confirmed: rust.yaml:141/145 use wasmi_wasi and node:wasi, neither touching the vendored files; README.vgm-studio.md:13 and OPTIMIZER-WASM-PLAN.md:355-359 both promise validation no script performs. The suggested fix is sound -- I verified web/wasi-shim/index.js imports and runs directly under node v24 with no browser.

**[LOW · consistency] `web/pack_worker.js:30`** — The tool "tail" is computed two different ways on native and web, and the web one drops unterminated output

crates/vgms-vgmtools/src/command.rs exists precisely so "the native binding, the wasmi parity test and the web worker cannot drift apart", and its doc for `tail` (line 54) says "the last line or two the tool printed". The two producers disagree: native `run::tail` (crates/vgms-vgmtools/src/run.rs:110-120) returns the last **three** non-empty lines joined with "; ", and `str::lines()` yields a final unterminated line too. The web producer (web/pack_worker.js:30-33, 48) keeps a 16-entry ring but returns only the **last single** non-empty line, and `ConsoleStdout.lineBuffered` (web/wasi-shim/fs_mem.js) only emits on "\n", so any final `printf` without a trailing newline stays stuck in `line_buf` and is silently lost — the web `tail` is `""` where native quotes the message. Verified in the upstream C: vgm_sro.c:702/1211, vgm_cmp.c:918 and optdac.c:488 all end a run with an unconditional `printf("\t...\r")`, which never terminates a line under this shim. The 16-entry ring is then a magic number nothing consumes — only element `[-1]` is ever read. Nothing tests this: crates/vgms-vgmtools/tests/wasm_parity.rs:151 hardcodes `tail: ""`, so the web tail has no coverage at any level.

*Fix:* Make pack_worker.js reproduce `run::tail`: keep the last 3 non-empty lines, join with "; ", and flush the pending partial line (or wrap `collect` so the leftover `line_buf` is appended before the tail is taken). Drop the unexplained 16 in favour of the same 3, and cross-reference run.rs in the comment so the lockstep is visible from both sides.

*Verifier narrowed this:* The headline mechanism is refuted empirically; a narrower divergence survives. CONFIRMED: run.rs:110-120 keeps the last three non-empty lines joined with "; " while pack_worker.js:48 keeps only the last one, and wasm_parity.rs:151 passes tail: "", so the web tail has no test coverage. REFUTED: 'drops unterminated output -- the web tail is "" where native quotes the message'. I ran all three shipped modules (target/wasi-tools/tool_*.wasm) through the vendored shim under node over tests/lsl3_score_up.vgm: vgm_cmp tail="File written.", vgm_sro exit 2 tail="No chips with Sample-ROM used!", optdac tail="\t...\rFile written." -- all non-empty. The cited printf("\t...\r") calls (vgm_sro.c:702/1211, vgm_cmp.c:918, optdac.c:488) are mid-run, not end-of-run; each is followed by newline-terminated output (vgm_cmp.c ends on "Data Compression Total: ...\n", then WriteVGMFile's "File written.\n"), so ConsoleStdout.lineBuffered does emit them. The partial-line hazard exists in the shim but does not bite these tools. Also overstated: the 16-entry ring is not 'a magic number nothing consumes' -- it bounds memory against vgm_sro's thousands of progress lines; only its size is arbitrary. What is left is a cosmetic error-message divergence in an untested path, not medium.

**[LOW · complexity] `web/pack_worker.js:99`** — The pack worker copies the whole zip before transferring it, under a comment calling it zero-copy

Lines 99-101 read `// Transfer the encoded PackJobOutcome bytes back (zero-copy).` then `const copy = result.slice(); self.postMessage(copy, [copy.buffer]);`. The comment is wrong twice over: wasm-bindgen's generated glue already hands back a standalone JS-heap copy — target/web-dist/vgms_web.js:38 is `var v5 = getArrayU8FromWasm0(ret[0], ret[1]).slice();` followed by `__wbindgen_free` — so `result` is not a view into wasm memory and `.slice()` is a second full copy of a multi-megabyte pack. That doubles the Worker's peak footprint at exactly the moment the module doc says it wants to avoid one. `result` has no other referent, so `self.postMessage(result, [result.buffer])` is correct and genuinely zero-copy. The same redundancy is in web/task_worker.js:26-29: `emit` is called from Rust with `js_sys::Uint8Array::from(bytes.as_slice())`, which already allocates a fresh JS typed array, so `bytes.slice()` copies again (smaller payloads, so lower stakes).

*Fix:* Transfer `result` directly in pack_worker.js and fix the comment to say why it is safe (wasm-bindgen already copied out of linear memory, and nothing else holds the buffer). Apply the same to task_worker.js, or keep the slice there with a comment explaining what it defends against.

*Verifier narrowed this:* Facts all check out; the severity does not. Verified target/web-dist/vgms_web.js:38 is `var v5 = getArrayU8FromWasm0(ret[0], ret[1]).slice();` followed by __wbindgen_free, so the Vec<u8> from vgms_web_run_pack_job (worker.rs:58-63) is already a standalone JS-heap array and pack_worker.js:100's `result.slice()` is a second full copy under a comment that says 'zero-copy'. Nothing else references `result`, and .slice() yields an exactly-sized ArrayBuffer, so transferring it directly is safe. The task_worker.js:26-29 half also holds: worker.rs:40 uses js_sys::Uint8Array::from(bytes.as_slice()), which is new_from_slice -> `new Uint8Array(view)` (js-sys 0.3.103 lib.rs:14116-14121, 13865), i.e. already a fresh JS array. But this is a transient extra allocation plus a wrong comment with no user-visible failure mode -- a comment/cleanup item, not a medium-severity one.

**[LOW · maintainability] `tools/build-web.ps1:61`** — The web build copies the entire web/ tree into the servable dist, dragging 27 MB of Playwright harness in with it

`Copy-Item (Join-Path $root "web\*") $dist -Recurse -Force` sweeps up web/e2e/ along with the page, the workers and web/wasi-shim/. Verified on this checkout: target/web-dist/e2e is 27 MB and contains node_modules/, test-results/, package.json, package-lock.json, playwright.config.js and serve.mjs. It is re-copied on every build, and the build's own summary (lines 84-87) filters to `-File` at the top level only, so it never appears in the "web-dist contents" listing. The e2e static server then serves its own node_modules back over HTTP for the whole suite run, and anyone who deploys target/web-dist ships the test harness with the app.

*Fix:* Copy the named artefacts explicitly — index.html, pack_worker.js, task_worker.js, worklet-processor.js and the wasi-shim/ directory — rather than the whole tree. (`-Exclude` does not behave reliably with `-Recurse`, so an explicit list is the honest fix.) That also makes the shipped surface visible in the script itself, which matters because web/wasi-shim/ must keep travelling with its two licence files.

*Verifier narrowed this:* The facts are right, the framing overstates. Confirmed: tools/build-web.ps1:61 is `Copy-Item (Join-Path $root "web\*") $dist -Recurse -Force`, target/web-dist/e2e exists at 27M with node_modules/, test-results/, package*.json, playwright.config.js and serve.mjs, and the summary at :83-87 filters `-File` at the top level with an extension whitelist so it never shows. Overstated: nothing in the repo publishes target/web-dist -- there is no deploy workflow, and CI assembles its own bundle -- so 'anyone who deploys ships the harness' is hypothetical, and 'the e2e static server then serves its own node_modules back over HTTP for the whole suite run' is not what happens; serve.mjs only serves what is requested and nothing requests them. Also worth weighing against the suggestion: an explicit named list is precisely the failure already sitting in .github/workflows/rust.yaml:202, which enumerated four files and forgot web/wasi-shim/. Excluding e2e/ would be safer than enumerating the keepers.

**[LOW · confusing] `web/task_worker.js:14`** — Both Worker bootstraps document a readiness mechanism neither one uses

task_worker.js:14-16 says "Requests that arrive first are queued by the browser and delivered after `init` resolves because we only register `onmessage` afterwards", and pack_worker.js:67-69 says "Registering onmessage only after this resolves means a request that arrives first is browser-queued until everything is ready." Neither is what the code does: `self.onmessage` is assigned synchronously at module top level in both files (task_worker.js:19, immediately after `const ready = init()` on line 17; pack_worker.js:80, immediately after the `ready` IIFE on lines 70-78). Readiness is actually handled by awaiting `ready` *inside* the handler. The outcome happens to be correct, but the stated mechanism is not the implemented one — which matters here because the two mechanisms have different failure modes (see the unguarded `await ready` in task_worker.js).

*Fix:* Rewrite both comments to describe the real design: the handler is registered eagerly and each message awaits the shared `ready` promise before doing anything, so ordering is preserved without deferring registration.

**[LOW · maintainability] `web/wasi-shim/debug.js:1`** — The shim's default-on syscall logging is held off by a comment rather than by code

`debug` is a module-level singleton, and `WASI`'s constructor calls `debug.enable(options.debug)` where `enable(enabled){...enabled===undefined?true:enabled...}` — an absent option means *enable*. web/pack_worker.js:35-39 is the single thing standing between production and per-syscall `console.log`, enforced only by a prose comment ("`debug: false` must be said out loud"). Any second construction site, or an upgrade that renames the option, turns global logging back on with nothing to catch it — and per finding above, the one e2e spec that runs this code asserts nothing that would notice. Worth recording alongside it: `Debug#enable` sets `this.log` but never updates `this.isEnabled`, so the `debug.enabled` getter reports `false` forever after construction regardless of the option — meaning the heavier guarded logs in wasi.js (`args_get`, `environ_get`) and fs_mem.js (`fd_readdir_single`) are already unreachable while the unguarded `debug.log` calls are not. The flag and the logger disagree, which makes reasoning about "is logging on?" harder than the comment implies.

*Fix:* Put the option in code, not prose: a tiny local module beside the vendored dist (e.g. web/wasi-shim-host.js exporting `createToolWasi(name, input)`) that owns the argv, the fd list and `debug: false`, with pack_worker.js calling it. That keeps web/wasi-shim/ byte-identical to upstream — preserving the README's `npm pack` + copy upgrade recipe — while making the requirement impossible to forget at a future call site. Note the `isEnabled`/`log` divergence in README.vgm-studio.md so the next upgrader knows it is upstream's, not ours.

### Native integration test suites

**[MEDIUM · bug-risk] `crates/vgms-app/tests/reference_parity.rs:635`** — The scorecard swallows every reference-player error and passes green

`every_cored_chip_matches_the_reference_within_its_band` discards reference failures silently: `let Ok(bytes) = reference.at_rate(rate).render(path, &work_dir()) else { continue; };` (635) and `let Ok((samples, rate)) = parity::reference::read_wav(&bytes) else { continue; };` (638). When a chip ends with no comparisons it prints "nothing comparable" (665) and moves on, and the only assertion is `failures.is_empty()` (725) — so a run where *nothing at all* was compared reports PASS. The trigger is documented, not hypothetical: `Reference::from_env` validates `VGMSTUDIO_REF_PLAYER` is a file but never touches `VGMSTUDIO_REF_CONFIG` (parity/reference.rs:114), the config is only opened later inside `stage()` (`std::fs::read_to_string(config)?`, reference.rs:273), and docs/vgm-multichip-2026-07/parity/REFERENCE.md:28 tells the user to set it to the repo-relative path `docs/vgm-multichip-2026-07/parity/VGMPlay.ini` — which cargo resolves against the *package* dir `crates/vgms-app/`, so it does not exist, every render errors, every row skips, and the harness is green. OPL-UNGATING-PLAN.md:126 records the gotcha ("VGMSTUDIO_REF_CONFIG must be absolute") but nothing in the code enforces it. The sibling control-group test does it right — it prints the error (511) and asserts `judged > 0` (559). Same vacuum exists in `the_chip_balance_is_measured_rather_than_guessed` (771) and, in weaker form, in optimize_parity.rs:145 (`compared` can be 0) and core_audio.rs:113 (a chip with `checked == 0` cannot fail).

*Fix:* Validate `CONFIG_ENV` in `Reference::from_env` the same way `PLAYER_ENV` is validated (exists, is a file — and canonicalise it), print the error instead of `continue` in both loops, and add a `assert!(compared > 0)`-style guard to every corpus test that can otherwise finish having measured nothing.

*Verifier narrowed this:* Code facts all check out: reference_parity.rs:635/638 discard reference errors with a bare `continue`, 664-666 prints "nothing comparable" and moves on, and 725 asserts only on `failures`; `Reference::from_env` (parity/reference.rs:93-117) validates PLAYER_ENV is a file but only stores CONFIG_ENV, which `stage()` first opens at reference.rs:273; REFERENCE.md:28 does document a package-relative path and OPL-UNGATING-PLAN.md:126 records the absolute-path gotcha. The per-chip vacuum (a chip whose renders all fail prints a line and passes) is real and unmitigated. Narrowed on the headline scenario: with a bad config the *whole binary* does not go green — the sibling `the_opl_control_group_calibrates_the_pipeline` in the same file renders the same reference and hard-fails on `judged > 0` (559), and every documented invocation uses --nocapture, so the "nothing comparable" lines are visible. Severity high -> medium: this is an #[ignore]d manual harness that already tattles on stdout, and the total-blackout case is caught by a sibling assertion. The subsidiary cites are accurate (optimize_parity.rs `compared` can be 0; core_audio.rs:113 `checked > 0 &&` makes a chip with no readable file unfailable).

**[MEDIUM · bug-risk] `crates/vgms-app/tests/cli_smoke.rs:95`** — Two CLI smoke tests pass for the wrong reason, and "every subcommand" checks three of five

`an_unknown_subcommand_is_rejected` runs `["convert", "song.dro"]` and asserts only `!status.success()` — but `song.dro` does not exist in the test's working directory, so the process would exit non-zero even if `convert` were still a valid subcommand; the test cannot fail for the reason it names. `a_file_argument_is_not_mistaken_for_a_subcommand` (181-186) has the same shape with `["small.dro", "render"]`, and both duplicate parser unit tests that *do* assert the right thing (cli/mod.rs:171 `a_file_and_a_subcommand_together_are_rejected`). Separately, `help_lists_every_subcommand` (67) iterates only `["play", "render", "split"]` while `Command` has five variants (cli/mod.rs:47-59) — dropping `optimize` or `retrowave-probe` from the parser would leave this test green under a name promising otherwise.

*Fix:* Assert on the message, not just the exit code (clap prints "unrecognized subcommand" / "cannot be used with" on stderr), and drive the subcommand list in `help_lists_every_subcommand` from all five names.

**[MEDIUM · maintainability] `crates/vgms-app/Cargo.toml:24`** — ~2,200 lines of test-only harness (and the `hound` dependency) ship inside the GPL app crate

`vgms_app::corpus` (352 lines) and `vgms_app::parity` (mod 549 + metrics 799 + reference 515) are `pub` modules of the crate that builds the shipped `vgmstudio` executable, and a workspace-wide grep shows their only consumers are the five integration-test files — every one of whose tests is `#[ignore]`d and therefore never run by CI (`cargo test --workspace`, .github/workflows/rust.yaml:41). `hound` is a runtime dependency solely for them, as its own comment admits ("Reads the reference player's WAV output for the parity harness"). The code is excellent and worth keeping, but the release binary carries a subprocess-spawning reference-player driver and a corpus walker it can never use.

*Fix:* Put both modules behind a non-default feature (e.g. `parity-harness`, with `hound` optional under it) and enable it on the ignored-test command lines documented in the module headers; the modules' own `#[cfg(test)]` unit tests come along with the feature.

**[MEDIUM · consistency] `crates/vgms-app/tests/engine_corpus.rs:86`** — One concept, two environment variables: VGMSTUDIO_CORPUS and VGMSTUDIO_VGMRIPS_CORPUS

Four suites read `VGMSTUDIO_CORPUS` directly (engine_corpus.rs:86, optimize_corpus.rs:108, projection_corpus.rs:44, optimize_parity.rs:94) and four read `VGMSTUDIO_VGMRIPS_CORPUS` via `corpus::corpus_root()` (chip_index, core_audio, oracle_lle, reference_parity). Both mean "a directory tree of .vgm/.vgz files"; the only real difference is that one half also builds the chip index. Nothing in the test files says the two are meant to be different corpora, DEVELOPMENT.md mentions neither, and a maintainer who sets one and runs `cargo test -p vgms-app -- --ignored` gets half the suite returning early — reported only on stderr, which is hidden without `--nocapture`, so the run looks like a clean pass. CORES-PLAN.md:217 in fact planned for engine_corpus.rs to be "driven by the chip index, `VGMSTUDIO_VGMRIPS_CORPUS` gated like the existing corpus", which never happened.

*Fix:* Make `VGMSTUDIO_CORPUS` the one name and let `corpus_root()` accept the old one as a fallback, or state the intended distinction in `corpus.rs` and in each suite's header; either way, list the corpus variables in DEVELOPMENT.md next to the `cargo test` line.

**[LOW · dead-code] `crates/vgms-synth/tests/scratch_chip.rs:16`** — scratch_chip.rs cannot pass: vgms-synth registers no generic cores

The test builds cores from the ambient registry (`vgms_synth::registry::registry().build(kind, None)`) inside vgms-synth's own integration-test binary. Nothing installs a registry there, so `registry()` falls back to `CoreRegistry::with_builtins` (registry.rs:587), which by that module's own unit test carries only a *listed* OPL row and builds nothing (`assert!(registry.build(ChipKind::Ymf262, None).is_none())`, registry.rs:610; `assert!(!registry.has_core(ChipKind::Sn76489), "providers only")`, registry.rs:608). Every core comes back `None`, the render is silence, and `assert!(peak > 500, "the file must sound")` (36) fails. The crate's own test-support module states the rule: "the real-core end-to-end lives downstream in `vgms-app`, where the providers are linked and registered" (src/testing.rs:9), and its `ToneStub` is `pub(crate)` so an integration test cannot reach it either. `SCRATCH_FILE` appears nowhere else in the repo — no CI step, no doc — so this has never run.

*Fix:* Delete it; `crates/vgms-app/tests/core_audio.rs` already does this job in the crate that has cores. If an ad-hoc level probe is still wanted, move it to vgms-app (which calls `install_cores()`) and mark it `#[ignore]` like its neighbours.

*Verifier narrowed this:* Mechanism confirmed: vgms-synth's dev-dependencies are only `sha2` (no provider crate), `registry()` falls back to `CoreRegistry::with_builtins`, and with_builtins registers only OPL rows built through `CoreMaker::Opl` (registry.rs:283-317) — its own unit test asserts `build(Ymf262, None).is_none()` and `!has_core(Sn76489)`. So every `build(kind, None)` in scratch_chip.rs:17 returns None, the render is silence, and `assert!(peak > 500)` (36) fails. `SCRATCH_FILE` appears nowhere else in the workspace (grep: only the file itself). Two corrections: it is not dead code that "has never run" — it is compiled and executed on every `cargo test --workspace` and returns early at line 9-12 when SCRATCH_FILE is unset, and clippy --all-targets type-checks it in CI; the failure only reaches a developer who deliberately points the variable at a file. Severity medium -> low: zero effect on CI or the product, one confusing dead end for an ad-hoc probe.

**[LOW · confusing] `crates/vgms-app/tests/engine_corpus.rs:25`** — The corpus engine's `Counting` core counts writes nobody reads, and its doc claims the opposite

"A core that renders a value derived from its writes, so a file whose commands never reach a chip is distinguishable from one whose do" (23-24) — but `render` is `out.fill(0)` (42-44) and `self.writes` is incremented and never read by anything. The property the comment advertises is therefore not tested at all: a stream whose every command is routed to the wrong port, or to no chip, renders exactly like one that is routed correctly, and this test (the only one that walks the whole corpus through `VgmEngine`) cannot tell them apart. It is the one gap that matters here, since the suite explicitly renounces checking *what* things sound like and keeps only "the stream walks and the timing is right".

*Fix:* Either make `render` emit something derived from `writes` and assert the total is non-zero for files that contain writes, or drop the field and the sentence so the test's contract matches what it checks.

*Verifier narrowed this:* The doc/code mismatch is exactly as described: engine_corpus.rs:23-24 claims a core "that renders a value derived from its writes", but `render` is `out.fill(0)` (42-44) and `writes` is only ever incremented (39) — nothing reads it (the `+=` keeps rustc's dead_code lint quiet, which is why CI's `-D warnings` passes). Impact overstated. The file's own module header (3-8) already disclaims sound checking, and the routing property the reviewer says is untested is precisely what `core_audio.rs` covers with real cores — its header says verbatim "a core can be perfect and still be handed writes on the wrong port", and `a_mega_drive_rip_plays_its_fm_as_well_as_its_psg` re-renders with the FM core withheld to prove FM writes land. So "the one gap that matters" is wrong; what remains is a struct comment that promises more than the struct does. Severity medium -> low.

**[LOW · refactor] `crates/vgms-app/tests/projection_corpus.rs:21`** — vgms-app's test suites have no `tests/common/`, so the same helpers are copied 2-5 times

Each integration test file is its own crate, so sharing needs a `tests/common/mod.rs` — vgms-synth has one (tests/common/mod.rs) and vgms-app does not, despite far more duplication. The recursive .vgm/.vgz collector exists four times in tests plus once in src, all subtly different: projection_corpus.rs:21 and optimize_corpus.rs:85 (`matches!` + `to_ascii_lowercase`), engine_corpus.rs:63 (`eq_ignore_ascii_case`), optimize_parity.rs:62 (`collect`, with a limit that is checked before recursing so it can return short), corpus.rs:223. The "render N seconds of a VgmFile into interleaved i16" loop exists five times: core_audio.rs:42 and core_audio.rs:182 (byte-identical apart from the core closure), oracle_lle.rs:41, reference_parity.rs:106, optimize_parity.rs:36. `single_chip_files` is copy-pasted between reference_parity.rs:143 and oracle_lle.rs:82, the latter having silently dropped the volume-modifier/extra-header filters that the former's comment says make the comparison legitimate.

*Fix:* Add `crates/vgms-app/tests/common/mod.rs` with `collect_songs`, `render_window(path, rate, seconds, cores)` and `single_chip_files`, and have the six suites use it — the divergence in `single_chip_files` is the kind of bug this prevents.

*Verifier narrowed this:* Duplication verified, every cite correct: vgms-app/tests has no `common/` while vgms-synth does; the collector appears at projection_corpus.rs:21 and optimize_corpus.rs:85 (identical `matches!`/`to_ascii_lowercase` form), engine_corpus.rs:63 (`eq_ignore_ascii_case`), optimize_parity.rs:62 (limit checked before recursing) and src/corpus.rs:223; the render-N-seconds loop at core_audio.rs:42/182, oracle_lle.rs:41, reference_parity.rs:106, optimize_parity.rs:36. The punchline is wrong, though: the `single_chip_files` divergence is deliberate and correct, not a latent bug. reference_parity.rs:170-181 filters volume_modifier/extra volumes for a stated reason — "The reference applies the header volume modifier... our engine applies neither" — and oracle_lle has no reference player in the loop (both sides are our own engine, fast core vs die core), so the filter would only shrink its sample for no gain. That removes the finding's justification for the refactor; what's left is ordinary test-scaffolding duplication. Severity medium -> low.

**[LOW · terminology] `crates/vgms-app/tests/oracle_lle.rs:66`** — Two functions named `native_rate_of` compute the rate two different ways, and the LLE one is the way SCORECARD.md records as wrong

oracle_lle.rs:66 takes a hand-supplied `per_sample` divisor from a table in the test (64 for YM2151, 144 for YM2612/YM2608, lines 124-155) and returns `clock / per_sample`. reference_parity.rs:256 has the same name and signature-minus-one-argument but asks the core under test: `core.reset(...); core.native_rate()`, with a comment explaining why ("asking the default while rendering through a challenger puts our resampler back into the measurement"). SCORECARD.md:585 lists exactly this as one of "three ways to get a meaningless number": "The challenger measured at the incumbent's rate... Cost: -3.5 cents and 0.11 of correlation. Fixed — it now asks the core under test." The oracle bench renders the fast core and the die core at that hand-computed rate, so if either core's `native_rate()` disagrees with `clock/per_sample` the resampler is back inside the one measurement built to exclude it — and the YM2608 row (0.5829, bar 0.50) is precisely the row whose comment says "a harness gap is not yet ruled out".

*Fix:* Delete oracle_lle's copy and use the reference_parity one (shared via the proposed tests/common), asking each side's own core for its rate; if the die core and the fast core disagree on native rate, that disagreement is itself the thing to report.

*Verifier narrowed this:* The two same-named functions exist as described (oracle_lle.rs:66 takes a hand-supplied `per_sample`, reference_parity.rs:256 asks `core.native_rate()` after reset), and SCORECARD.md:578-582 does record the incumbent-rate trap. But the equivalence claim does not survive checking: oracle_lle's table values (64/144/144, lines 124-155) are exactly the LLE cores' own `CLOCKS_PER_SAMPLE` constants (lle_opm.rs:33 = 64, lle_opn2.rs:25 = 144, lle_opna.rs:33 = 144), and each LLE core sets `self.rate = (clock / CLOCKS_PER_SAMPLE).max(1)` on reset — so the die side is provably unresampled, which is the opposite of the recorded mistake (that one asked a *different* core for the rate). The residual risk is only that a *shipping* core might report a different native rate, which is asserted nowhere but also evidenced nowhere; attributing the YM2608's 0.5829 row to it is speculation the row's own comment already flags as unresolved. Real content: a name collision between sibling suites plus a hard-coded constant that should be asked for. Severity medium -> low.

**[LOW · bug-risk] `crates/vgms-app/src/corpus.rs:113`** — A cached ChipIndex reports a different `scanned` than the walk that wrote it, so the chip report's percentages are wrong on every run but the first

`build` counts files walked (`index.scanned += 1` per file, line 62) including unreadable ones; `load` recomputes it as `index.by_chip.values().map(Vec::len).sum()` (113), which is (chip, file) *pairs* — a strictly larger number whenever any rip declares two chips, and `unreadable` is silently reset to 0. chip_index.rs then prints "chip index: {n} chips over {scanned} files" (40) and computes `share = count * 100 / scanned` (46), and its own header says the first run caches "so later runs are immediate" — i.e. the cached, wrong numbers are the normal case, and the printed shares (the evidence the test exists to produce, per CORES-PLAN.md:317) are deflated by the multi-chip factor. The round-trip assertion at chip_index.rs:76-81 compares only `files(chip).len()`, so it cannot catch this. Nearby: `cache_path`'s doc says "beside the corpus if that is writable, else under `target/`" (216-217) but the body is unconditionally `root.join(...)` — there is no fallback.

*Fix:* Write `scanned` and `unreadable` into the cache header (the format already has `# {n} files scanned` as a comment and a `v1` marker to bump) and parse them back in `load`; fix or drop the `cache_path` sentence.

*Verifier narrowed this:* The line counts are exact (corpus.rs 352, parity/mod.rs 549, metrics.rs 799, reference.rs 515 = 2215) and both are `pub mod` in vgms-app/src/lib.rs:14,16 with no consumer under src/ — grep for `parity::`/`corpus::` in src returns only their own modules. Three supporting claims fail. (1) `hound` costs nothing: vgms-synth already depends on it workspace-wide (src/wav.rs, capture.rs, peak.rs, split.rs) and vgms-app depends on vgms-synth, so dropping the manifest line compiles not one crate fewer. (2) "never run by CI" is false — CI runs `cargo test --workspace`, which executes both modules' own `#[cfg(test)]` units and the non-ignored `chip_index.rs::a_missing_corpus_is_not_an_error` (86-96), and `cargo clippy --workspace --all-targets -D warnings` type-checks every ignored suite. (3) "the release binary carries" the driver is unsupported: unreferenced rlib code is not pulled into the linked executable. What survives is the organisational point that a test-only harness lives in the shipped crate's public API. Severity medium -> low.

**[LOW · complexity] `crates/vgms-app/tests/optimize_corpus.rs:34`** — The immediate-write render oracle, and the paragraph explaining it, are duplicated across two crates

`render_immediate` (optimize_corpus.rs:34-65) and `render_indices` (crates/vgms-synth/tests/optimize_parity.rs:69-100) are the same loop, constant for constant: `NukedOpl3` + `FrameClock::new(rate, VGM_SAMPLE_RATE)` + an 8192-sample scratch + the same `Bank` tracking and the same match arms. The rationale comment above each ("Nuked's buffered write path spreads queued writes a couple of samples apart... Immediate writes isolate the latched-state audio") is also duplicated near-verbatim (optimize_corpus.rs:16-22 vs optimize_parity.rs:8-16), so a correction to the reasoning has to be made in two crates. Two different files are also both called `optimize_parity.rs` (vgms-app and vgms-synth) while testing different optimisers, which makes "the optimize_parity test" ambiguous in conversation and in CI logs.

*Fix:* Expose the immediate-write render once from `vgms_synth` (its `src/testing.rs` is the natural home, promoted from `pub(crate)` or put behind a `testing` feature) and have both suites call it; consider renaming vgms-app's to `vgmtools_parity.rs` after the tools it actually gates.

**[LOW · dead-code] `tests/make_screenshot.py:1`** — make_screenshot.py is referenced by nothing

The script generates `tests/screenshot.png`, which five Rust sites embed (pack_flow.rs:11, services/pack.rs:278, pack_zip.rs:171, vgms-ui app_gui_tests.rs:4824, vgms-ui pack.rs:2713). A workspace-wide grep for `make_screenshot` returns no hits outside the file itself: no CI step, no DEVELOPMENT.md mention, and none of the fixture's consumers name it. It is the last Python file under `tests/` (DEVELOPMENT.md:214 still documents `python -m unittest discover --start-directory tests/`, which now discovers nothing), so it reads as leftover rather than as the fixture's provenance.

*Fix:* Keep it but point at it — one line in a comment beside `PNG_FIXTURE` in pack_flow.rs, or a note in DEVELOPMENT.md saying how to regenerate the fixture — or port it to a `#[ignore]`d Rust generator so a fixture regen does not need a Python interpreter.

**[LOW · maintainability] `crates/vgms-synth/tests/engine_render.rs:40`** — engine_render.rs re-embeds the DRO fixture and re-implements common's Instruction-to-Op mapping

`fixture_ops` (40-56) is the same match, arm for arm and `expect` message for `expect` message, as the closure inside `common::decode_fixture` (tests/common/mod.rs:65-79) — including the `unreachable!` on `BankSwitch | DelaySamples`. The file also has its own `include_bytes!("../../../tests/lsl3_score_up_dro2.dro")` (17) while `common` already embeds the same fixture as a private `FIXTURE` (common/mod.rs:9), so the bytes are linked twice into one test binary and a fixture rename touches two places.

*Fix:* Add `pub(crate) fn ops_from(data: &SongData) -> Vec<Op>` and `pub(crate) const FIXTURE` to tests/common/mod.rs and have both callers use them; `decode_fixture` keeps its extra header assertions on top.

**[LOW · confusing] `crates/vgms-app/tests/reference_parity.rs:970`** — The level report's min/max depend on an undocumented side effect of `median`

`let (low, high) = (ratios[at][0], ratios[at][n - 1]);` reads the ends of the slice as if they were the minimum and maximum, which is true only because the line above it (`median(&mut ratios[at])`, 969) sorts in place — `fn median(values: &mut [f64])` (1011) sorts and returns the middle, and neither its name nor its doc (it has none) says so. The `spread` computed from them (971) is what decides whether a core's level mismatch is reported as "one scalar's worth of difference" or failed with a suggested `level` constant, so re-ordering those two lines, or switching to a non-mutating median, would silently change which cores fail.

*Fix:* Either compute `low`/`high` explicitly with `fold`/`total_cmp`, or rename to `sort_and_median` / document the in-place sort on it.

*Verifier narrowed this:* no verdict returned; kept unverified

### Untrusted-input robustness sweep

**[HIGH · bug-risk] `crates/vgms-synth/src/decompress.rs:240`** — A compressed data block declaring more than 32 bits per value panics the render thread

`bits_out` (payload[5], `bits_decompressed`) comes straight from the file and is only checked for `width == 0`. `width = value_bytes(bits_out) = ceil(bits_out/8)` can therefore be 5..=32, while `value` is a `u32`, so line 240's `&value.to_le_bytes()[..width]` slices a 4-byte array with an end index of up to 32 -> `range end index 5 out of range for slice of length 4`, an unconditional panic. Line 226's `packed << bits_out.saturating_sub(bits_in)` is the same trigger (a shift of up to 254 on a u32: panic in debug, silently masked in release). The `mask` computation just above at line 208 already special-cases `bits_out >= 32`, so the >32 case was considered for the mask and missed for the width.

Concrete input: a VGM with a `0x67 0x66 0x40 ssssssss` block whose payload begins `00 08 00 00 00 28 08 00 00 00 <one data byte>` (scheme 0, uncompressed_size 8, bits_decompressed 0x28 = 40, bits_compressed 8). `count = 8/5 = 1`, `BitReader::read(8)` succeeds, and the first `extend_from_slice` panics. This is reached from `vgm_engine::data_block` (crates/vgms-synth/src/vgm_engine.rs:725) on ordinary playback and on offline WAV/waveform render, i.e. on the cpal audio callback natively and inside the wasm worker/worklet on the web -- neither has a `catch_unwind`, so it is a hard crash from a downloaded file.

*Fix:* Reject `bits_out > 32` (and `bits_in > 32`) alongside the existing `width == 0` check, or clamp `width` to `width.min(4)`. Add a test for a block declaring bits_decompressed = 40, next to `an_unknown_scheme_is_refused`.

**[HIGH · bug-risk] `crates/vgms-synth/src/decompress.rs:215`** — `Vec::with_capacity(uncompressed_size)` allocates whatever a data block's header claims

`uncompressed_size` is read verbatim from the block's sub-header (line 194) and used as the capacity of the output buffer, with no relation to how much packed data the block actually carries -- the loop itself breaks as soon as the `BitReader` runs dry (line 219-221), and the doc comment says as much ("A block whose packed data runs out early decompresses to what it did carry"), so the capacity is the only unbounded part.

Concrete input: an eleven-byte payload `00 FF FF FF FF 08 08 00 00 00 <1 byte>` inside a `0x67` type-0x40 block asks for a 4 GiB allocation and yields one byte. On wasm32 that exceeds the whole 4 GiB address space, so `handle_alloc_error` aborts the module (the web app dies); natively it is a multi-GB commit spike on a file of a few dozen bytes. `Banks::read` (crates/vgms-synth/src/banks.rs:248) already caps exactly this pattern with `length.min(0x1_0000)`, so the codebase's own convention is not followed here.

*Fix:* Cap the pre-allocation against what the payload can possibly produce, e.g. `Vec::with_capacity(uncompressed_size.min(data.len() * 8 / usize::from(bits_in).max(1) * width))`, or simply `uncompressed_size.min(SOME_CAP)` in the style of `Banks::read`.

**[HIGH · bug-risk] `crates/vgms-core/src/vgm/io.rs:80`** — VGZ gunzip has no size cap, and the stream index multiplies the result twelvefold

`GzDecoder::new(bytes).read_to_end(&mut decoded)` (also duplicated verbatim at crates/vgms-core/src/vgm/file.rs:698-702) expands a `.vgz` with no ceiling. Deflate reaches ~1000:1 on zeros, and zeros are the worst case here for a second reason: `0x00` is treated as one-byte padding by `command_size` (crates/vgms-core/src/vgm/stream.rs:281), so every decompressed zero byte becomes one command, costing 4 bytes in `offsets` and 8 in `wait_prefix` -- a 12x amplification on top of the 1000x.

Concrete input: a 1 MB `.vgz` holding ~1 GB of `0x00` after a valid 0x80-byte header. Decompression allocates ~1 GB, then `VgmStream::parse` allocates ~12 GB of index. On wasm32 the address space is 4 GiB, so this is a guaranteed abort of the web app from a file the user merely opened; natively it is an OOM. Nothing downstream re-checks the size, and neither reader consults the gzip trailer's ISIZE.

*Fix:* Decompress through `Read::take(MAX_UNCOMPRESSED)` with an explicit documented cap (a real VGM is at most a few tens of MB) and return `Error::file` when the cap is hit. Since `vgm::io::read` and `vgm::file::read` carry byte-identical gunzip preambles (and `write_gzipped` is likewise duplicated), put the capped helper in one place so the cap cannot be added to only one of them.

*Verifier narrowed this:* The primary defect is confirmed at both cited sites: io.rs:81-83 and file.rs:699-701 are byte-identical uncapped `GzDecoder::new(bytes).read_to_end(&mut decoded)`, neither consults the gzip ISIZE, and nothing downstream re-checks the size. Adjusted because the composite failure scenario does not hold at the cited line: io.rs:80's downstream is VgmData::read_from_stream (data.rs:85-113), whose command_size (data.rs:119-130) has no 0x00 arm and returns "Unsupported VGM command", so the all-zeros file is rejected right after decompression and no index is ever built. The 12x amplification is real but belongs solely to the file.rs:698 sibling, which parses through VgmStream::parse (stream.rs:604-627, offsets Vec<u32> + wait_prefix Vec<u64>) and does step over 0x00 as one-byte padding (stream.rs:281). That is the path the web codec (vgms-web/src/codec.rs:193) and the multichip model use, so the ~1 GB + ~12 GB scenario stands there; at io.rs:80 the damage is the ~1 GB decompression alone. Severity kept: the one-place fix the suggestion asks for is still the right call.

**[MEDIUM · bug-risk] `crates/vgms-pack-archive/src/lib.rs:60`** — Pack zip entries are read to end with no size cap (zip bomb)

`PackArchive::open` iterates every entry and calls `file.read_to_end(&mut bytes)` with no limit, accumulating all of them in the `entries` map. The `zip` crate does not bound decompressed output: `Decompressor::Deflated` wraps `flate2::bufread::DeflateDecoder` and `Crc32Reader` only verifies the CRC *after* the whole entry has been read into memory (zip-8.6.0/src/crc32.rs:58-70), so a mismatch is detected after the allocation, not before it.

Concrete input: a `.zip` of a few hundred KB containing one deflate-bombed entry named `01 Song.vgm` that expands to tens of GB. Opening it via File > Open pack zip (both native and the web `pick_pack_zip` path) allocates until the process/module dies. The declared uncompressed size is available on the entry and is never consulted, and the module already has a "skip a bad entry rather than fail the open" policy (lines 60-64), so a cap fits the existing design rather than changing it.

*Fix:* Check `file.size()` against a per-entry cap and skip (with a log line) when it is exceeded, and read through `Read::take(cap)` so a lying header cannot beat the check; optionally also cap the total across entries.

**[MEDIUM · bug-risk] `crates/vgms-core/src/vgm/header.rs:705`** — Header pointer arithmetic overflows `usize` on wasm32, where dro.rs already guards the same hazard

Every relative header pointer is widened with `offset + relative as usize` where `relative` is an untrusted u32: `data_start` (line 705), `gd3_offset` (line 469), `loop_offset()` (line 586), `read_extra_header` (line 797) and `parse_extra_header` (lines 829, 838), plus `declared_eof` in crates/vgms-core/src/vgm/file.rs:783. On a 64-bit target these cannot overflow and the subsequent `data_start > bytes.len()` / `seek` checks catch the nonsense value; on wasm32 `usize` is 32 bits, so `0x34 + 0xFFFF_FFFF` overflows -- a panic in a debug build (and in `cargo test --target wasm32`), and a silent wrap in release, which makes `data_start` 0x33 and passes the bounds check. The same file then parses differently in the browser than it does natively.

Concrete input: a VGM whose data-offset field at 0x34 is `FF FF FF FF`. `BitReader::read`'s `self.bytes.len() * 8` (crates/vgms-synth/src/decompress.rs:149) is the same class of 32-bit overflow for a >512 MiB payload.

The codebase already knows about this hazard and guards it one module over: crates/vgms-core/src/io/dro.rs:186-195 says "on wasm32 a `usize` is only 32 bits, so `length_pairs * 2` would wrap" and uses `checked_mul`. The VGM header reader was not given the same treatment.

*Fix:* Use `checked_add` (returning the existing "points outside the file" error) for each of these widenings, mirroring dro.rs's wording so the wasm32 rationale is stated once and followed everywhere.

**[MEDIUM · bug-risk] `crates/vgms-synth/src/dac_stream.rs:251`** — An unvalidated `0x92` stream rate makes each output frame do unbounded work

`set_rate` (line 180) stores the `0x92` command's u32 verbatim, and `advance_frame` then runs `while stream.accumulator >= rate` after adding `hz` once per output frame -- so the inner loop iterates `hz / output_rate` times per frame, each iteration pushing a `PendingWrite` that the engine turns into a real chip register write (crates/vgms-synth/src/vgm_engine.rs:831-836).

Concrete input: `0x90` setting up a stream on a clocked chip, `0x92 00 FF FF FF FF` (rate 0xFFFF_FFFF), and `0x93` with bit 7 of the flags set (loop). At a 44100 Hz output that is ~97,000 chip writes per output frame, and the loop bit means the stream never reaches its end to stop itself -- so rendering one second of audio costs ~4.3 billion emulated register writes. Playback starves the audio callback and an offline render/export never returns; on the web it wedges the worklet. Nothing between the file and the loop bounds `hz` (`stream.playing` only checks `hz > 0`).

*Fix:* Clamp the stream rate at `set_rate` to a documented ceiling (a real DAC stream is at most a few hundred kHz; anything above, say, the output rate times a small factor is a corrupt or hostile file), or bound the per-frame iteration count and drop the excess with a warning.

**[LOW · bug-risk] `crates/vgms-core/src/vgm/file.rs:906`** — `slide_pointer` underflows when a header pointer targets the middle of an embedded GD3

`relative - removed` is unguarded. The doc comment describes the intended case -- "a pointer whose target was past the tag loses exactly the bytes that were removed" -- but the guard `field + relative as usize > cut` also admits a pointer whose target lands *inside* the removed span, and for those `relative < removed`, so the subtraction underflows: panic in a debug build, wrap to a ~4 GiB relative pointer in release.

Concrete input: a VGM with data at 0x400, a GD3 embedded at 0xC0 declaring 0x300 bytes of strings (so `removed` = 0x30C), and a loop-offset field at 0x1C holding 0xA5 (target 0xC1, one byte into the tag). `read` keeps that tag (0xC0 >= `LAST_POINTER_FIELD_END`) and never validates the loop pointer's raw bytes -- `loop_index()` only returns `None` for it. Saving the file (retag, or any pack-mode export) reaches `relocate_embedded_gd3` -> `slide_pointer(header, LOOP_OFFSET, 0xC0, 0x30C)` and computes `0xA5 - 0x30C`. `field + relative as usize` on the same line is also the 32-bit overflow of the previous finding.

*Fix:* Clamp a target inside the cut region to the cut itself rather than subtracting blindly: compute the absolute target, and write back `target.max(cut).saturating_sub(removed_as_usize)` relative to `field` (or `relative.saturating_sub(removed)` at minimum). Add a round-trip test for an embedded GD3 with a loop pointer aimed into the tag.

*Verifier narrowed this:* The code claim is correct and reachable. slide_pointer (file.rs:897-908) admits a target inside the cut span -- `field + relative as usize > cut` is true for 0x1C + 0xA5 = 0xC1 > 0xC0 -- and then does an unguarded `relative - removed` (0xA5 - 0x30C). LAST_POINTER_FIELD_END is 0xBC + 4 = 0xC0 (file.rs:41), so a GD3 at exactly 0xC0 is kept by the reader (file.rs:833 tests `at < LAST_POINTER_FIELD_END`), and the loop pointer's raw bytes are never validated on read -- the writer is documented as recomputing nothing (file.rs:358-362), so a retag really does carry the raw field into relocate_embedded_gd3. Severity lowered from medium to low: in the shipped release build the consequence is a wrapped LOOP_OFFSET (or EXTRA_HEADER) written into the output header, which the reader then discards via loop_index() returning None -- self-correcting header noise, not a crash or a corrupted stream. DATA_OFFSET cannot underflow here (its target is always past the cut). The panic is debug/test-only, and the input requires both an embedded pre-data GD3 and a loop pointer aimed into it.

### Concurrency sweep

**[HIGH · bug-risk] `crates/vgms-web/src/services/task.rs:184`** — WorkerTaskService cancel/supersede does not drop results already queued, unlike the native generation filter

The native `ThreadTaskService` tags every posted result with a generation and `Slot::cancel` bumps it, precisely so that a task which already finished and queued its result has that result discarded (`crates/vgms-app/src/services/task.rs:70-77` and the test `cancel_drops_an_already_queued_result` at line 355). The web service has no such filter. Results are pushed into one shared, untagged `Rc<RefCell<Vec<TaskResult>>>` by each Worker's `onmessage` closure (line 121), and `poll` returns the whole vector wholesale (line 213). Neither `cancel` (line 184) nor `terminate` (line 148) nor `submit`'s supersede (line 168) touches `results`.

Concretely: a VolumeScan Worker finishes and its `onmessage` pushes `TaskResult::Peak` into `results` between two frames. The user opens another song; `VgmStudioApp` calls `tasks.cancel(TaskKind::VolumeScan)` (`crates/vgms-ui/src/app.rs:2285`) with the comment "its peak is the old song's, and landing late it would set this song's volume from it". On the web the terminate is a no-op (the Worker is already done) and the queued Peak is delivered on the next `poll`, so `handle_volume_scan` sets the new song's volume from the old song's measurement. The same applies to `TaskResult::Wav` (a save dialog for a song the user closed) — the exact class the native cancels exist to prevent.

*Fix:* Give the web service the same generation discipline: keep a `HashMap<TaskKind, u64>` of current generations, capture the generation in the spawn closure and push `(kind, generation, result)` into the queue, bump on `terminate`/`cancel`/`submit`, and filter in `poll`. (A cheaper variant: key `results` by `TaskKind` and clear that kind's bucket in `terminate`.) The native test `cancel_drops_an_already_queued_result` names the contract the web implementation is silently breaking.

**[HIGH · bug-risk] `crates/vgms-web/src/services/audio.rs:170`** — load() never disconnects the superseded AudioWorkletNode, so it keeps running — and the comment says the opposite

`load` says "A fresh node supersedes any current one: reset the ready flag and drop the old node so its handler stops updating our state" (line 164-165) and then only does `inner.node = None` (line 170). Dropping the Rust-side `web_sys::AudioWorkletNode` handle does neither of the things the comment claims:

1. The node is still connected to `context.destination()` (connected at line 403) and `worklet-processor.js` `process()` returns `true` unconditionally (`web/worklet-processor.js:256`), so the old processor stays in the graph and keeps being called every quantum — with its own multi-megabyte `WebAssembly.Instance` holding the previous song and chip cores. `unload` (line 199) *does* call `node.disconnect()`, so the asymmetry is plainly unintended.
2. Its handler does not stop updating shared state: `_on_message` is untouched here, so until `setup` replaces it at line 409 the old node's `state` posts (every ~8 quanta, `worklet-processor.js:236`) keep overwriting `inner.state` — the very state `load` just reset at line 169 — so `position()`/`is_finished()` report the old song while the new node is still being set up.
3. When `setup` finally assigns `inner._on_message = Some(on_message)` the previous `Closure` is dropped, but the orphaned node's `port.onmessage` still points at it, so every subsequent state post invokes a dropped wasm-bindgen closure and throws.

This is reachable on the ordinary path: `ensure_audio` calls `audio.load(...)` with no preceding `unload` (`crates/vgms-ui/src/app.rs:4500`), and `after_edit` only pauses (`app.rs:4142`), so every edit-then-play leaks one live processor. `preview_track` (`app.rs:2908-2909`) is the same shape.

*Fix:* Mirror `unload`: take the old node, `disconnect()` it, drop `_on_message`, and clear `pending` before starting the new setup — ideally by calling `self.unload()` at the top of `load`, which is exactly what the native service does (`crates/vgms-app/src/services/audio.rs:97`, "two open output streams would play over each other"). Also consider having the processor return `false` on a `dispose` command so the node is collectable.

*Verifier narrowed this:* Headline confirmed: load clears inner.node (audio.rs:169) with no disconnect while unload does disconnect (:199-201), the node is connected to destination (:403), worklet-processor.js process() ends in an unconditional `return true` (:256), and _on_message is untouched by load so the old closure is dropped only when setup reassigns it (:409) -- after which the orphan's ~21 ms state posts call a dropped wasm-bindgen closure and throw forever. Reachability confirmed for edit-then-play (after_edit only pauses, app.rs:4141; ensure_audio loads without unload, app.rs:4501) and preview_track (app.rs:2908-2909). Two corrections: (a) load_file (app.rs:2291) and reload_audio_in_place (app.rs:4016) DO unload first, so those paths are clean; (b) detail 2 overreaches -- position() and take_peaks() gate on inner.node, which load just set to None, so during the setup window they return None; what the orphan's posts actually corrupt is state.finished (read by is_finished()/is_playing() with no node gate) and accum_peak. Both cited paths also pause the old node first, so the harm is a permanent live node + multi-MB wasm instance per cycle plus the exception storm, not audible overlap.

**[MEDIUM · bug-risk] `crates/vgms-ui/src/app.rs:2281`** — A running loop search survives a song load and applies its candidates to the wrong document

Loading a song cancels the task kinds whose late results would target the wrong document — `RenderWav`, `Split`, `VolumeScan` (lines 2281-2285) — under the comment "A dialog left open across a load would edit the wrong song -- a stale Save silently corrupting it -- so anything song-bound closes with the song" (2273-2276). `TaskKind::LoopSearch` is missed on both counts:

- `close_song_dialogs` (line 3943) clears `find_reg`, `dro_info`, `gd3_tag`, `vgm_metadata`, `render_wav` and `split`, but not `dialogs.find_loop` — which is as song-bound as any of them, holding a `LoopSearchDoc` snapshot of the searched document.
- `tasks.cancel(TaskKind::LoopSearch)` is only issued from `Action::CancelLoopSearch` (line 2192), never from `open`/`close_song` (2281-2285, 2376-2378).

So a Find Loop search started on song A keeps running over A's snapshot after song B is opened, and `handle_loop_candidates` (line 3230) feeds its candidates into the still-open dialog now sitting over B. Picking a row emits `Action::SetLoopStart(candidate.loop_point)` / `SetLoopEnd(candidate.loop_end)` (`crates/vgms-ui/src/dialogs/find_loop.rs:194-195`) — raw row indices into song A — and the Apply button writes them into B's VGM header via `ApplyLoopToMetadata` (find_loop.rs:210). That is the silent corruption the load-time comment warns about. Note the split flow is protected by exactly this kind of guard (`write_split` bails unless `split_flow` is still set, line 4354), which shows the pattern the loop search is missing.

*Fix:* Add `self.dialogs.find_loop = None;` to `close_song_dialogs`, and `self.tasks.cancel(TaskKind::LoopSearch);` alongside the other cancels in the open and `close_song` paths. A gui test that opens a second song mid-search and asserts the dialog is gone would pin it.

*Verifier narrowed this:* Facts confirmed: close_song_dialogs (app.rs:3943-3950) clears six dialogs but not dialogs.find_loop, and TaskKind::LoopSearch is cancelled only from Action::CancelLoopSearch (app.rs:2192), not in load_file (:2281-2285) or close_song (:2376-2378); handle_loop_candidates (:3230) feeds a still-open dialog, and find_loop.rs:194-195/:210 emit raw row indices from the searched document. Narrowed on two counts. Reachability: the dialog is an egui::Modal, so the menu and gather_key_input are both blocked while it is up (app.rs:1471) -- the song can only be swapped underneath it by a drag-and-drop, which handle_drops (app.rs:1350) processes unconditionally, or by the startup file. Impact: markers.set_start/set_end clamp to len-1 (markers.rs:90-99), so the result is a wrong-but-in-range loop written to song B, not an out-of-range write, and it takes a deliberate click on a row in a visibly stale table. Worth fixing for consistency with the sibling dialogs, but not the silent-corruption class of a stale Save.

**[MEDIUM · bug-risk] `crates/vgms-web/src/services/audio.rs:369`** — The async worklet setup has no generation guard, and module_added is a TOCTOU across an await

`setup` (line 359) is spawned with `spawn_local` and awaits twice — `add_module` (376) and `fetch_bytes` (382) — but carries no token identifying which `load` it belongs to. Two loads in flight (two quick clicks in the Settings core picker: `preview_cores` -> `reload_audio_in_place` -> `ensure_audio` -> `load`, `crates/vgms-ui/src/app.rs:3986-4018) leave two setups racing, and whichever finishes last wins the `inner.node = Some(node)` / `ready = true` write at lines 407-416, regardless of which load is current. There is no equivalent of the native `Slot::generation` check here.

The `module_added` flag has the same shape as a TOCTOU: it is read at line 369 and only set at line 379, after the awaited `add_module`. Two concurrent setups both observe `false`, both call `worklet.add_module(PROCESSOR_URL)`; the second evaluation of the script re-runs `registerProcessor("vgms-engine", ...)` (`web/worklet-processor.js:260`), which throws for an already-registered name, so that setup fails with "the audio processor module failed to load" and the user is shown an error for a load that would otherwise have worked.

*Fix:* Carry an epoch: bump a `u64` in `Inner` on each `load`, capture it in `setup`, and bail out (disconnecting the node it just built) if it no longer matches before installing the node. For `module_added`, set the flag *before* awaiting — or better, store the `add_module` promise in `Inner` and have every setup await that one promise, so the module is added exactly once.

*Verifier narrowed this:* Split verdict. The missing epoch guard is real -- setup (audio.rs:359-419) carries no token and the last completer wins inner.node/_on_message -- and is in fact worse than stated: load does not clear inner.pending (only unload does, :204), so with two setups in flight the FIRST to finish drains the queued play/seek commands and starts sounding, while the second silently becomes inner.node; every later pause then goes to the wrong node and the audible orphan cannot be stopped. But the cited trigger is wrong: reload_audio_in_place bails when audio.position() is None (app.rs:4011-4013), and position() returns None for the entire async window because load cleared inner.node, so two quick core-picker clicks cannot produce two setups. The real triggers are preview_track (app.rs:2908-2909, pause+load with no in-flight gate) and edit-then-play via ensure_audio. The module_added half is rejected: a second addModule of the same URL resolves from the worklet global scope's module map without re-evaluating the script, so registerProcessor does not run twice and no "module failed to load" error is shown -- setting the flag early would be harmless but fixes nothing.

**[MEDIUM · complexity] `crates/vgms-ui/src/tasks.rs:456`** — The loop search emits a full ranked clone per candidate, growing the result channel quadratically

`on_candidate` (line 456-461) clones the entire accumulated candidate list, sorts it, and emits it — for *every* candidate found:

```rust
let mut on_candidate = |candidate| {
    found.push(candidate);
    let mut snapshot = found.clone();
    rank(&mut snapshot);
    emit(TaskResult::LoopCandidates(snapshot));
};
```

With N candidates that is O(N^2 log N) work and O(N^2) bytes handed to `emit`. `emit` is not a cheap sink: natively it pushes into an unbounded `std::sync::mpsc` channel drained once per frame (`crates/vgms-app/src/services/task.rs:142-149`), so a fast search queues hundreds of snapshots between frames; on the web every snapshot is bincode-encoded and structured-cloned across a `postMessage` (`crates/vgms-web/src/worker.rs:38-44`). The dialog only ever displays the latest snapshot (`FindLoopDialog::set_candidates` replaces the vector wholesale, `crates/vgms-ui/src/dialogs/find_loop.rs:155`), so every intermediate one is discarded. The minimum-length slider floors at 0.5 s (`find_loop.rs:25`), which on a dense capture is a permissive enough setting to find a great many candidates.

Its sibling streaming task does throttle: `render_waveform_progressive` emits on a stride of completed buckets (`crates/vgms-synth/src/waveform.rs:189-203`), not per unit of work.

*Fix:* Throttle the snapshot the way the waveform render does — emit at most every K candidates (or on a coarse elapsed-time tick), plus one final ranked emit when the search returns. That keeps the live-filling table without the quadratic clone or the channel flood.

**[LOW · bug-risk] `crates/vgms-retrowave/src/player.rs:196`** — The pump thread is joined from the UI thread with no bound, behind a 2 s serial write timeout

`shutdown` sets the stop flag and then blocks on `self.pump.take()?.join()` (line 198). It is reached from `into_device` (line 192) and from `Drop` (line 211), and both run on the UI thread: `RetroWaveAudioService::unload` calls `audio.into_device()` (`crates/vgms-app/src/services/retrowave.rs:85`), which every `load` and every backend switch goes through, and `on_exit` calls `audio.unload()` (`crates/vgms-ui/src/app.rs:4743`).

The pump can be inside a blocking `flush` when the stop lands (lines 315, 318, 338), and the port is opened with `WRITE_TIMEOUT = Duration::from_secs(2)` (`crates/vgms-retrowave/src/device.rs:26`). After the loop exits, `run_pump` does one more `mute_sweep` plus `flush` (lines 247-250) — a multi-kilobyte write, another timeout's worth. So against a wedged-but-not-yet-failed board the GUI freezes for up to ~4 s on a song load or on quit, with no feedback. Everything else in this design is careful about not blocking (the quantum is 1.3 ms, the queue is lock-free); this is the one unbounded wait, and it is the one on the UI thread.

*Fix:* Either bound the wait (park the handle in a `pending_joins: Vec<JoinHandle<_>>` reaped on later frames, accepting that a wedged device's port comes back late) or shorten `WRITE_TIMEOUT` to something on the order of the quantum, since a healthy USB CDC write completes in microseconds and the only thing 2 s buys is a longer freeze. At minimum, say in the doc comment that `into_device`/`Drop` can block for the write timeout.

*Verifier narrowed this:* The structure is as described: shutdown sets stop then blocks on pump.join() (player.rs:196-199), reached from into_device (:192) and Drop (:211), and RetroWaveAudioService::unload -> audio.into_device() runs on the UI thread from every load (services/retrowave.rs:67, :83-87) and from on_exit (app.rs:4744); WRITE_TIMEOUT is 2 s (device.rs:26). The quantified claim is wrong, though: if a flush inside pump_loop times out the error propagates out of pump_loop and run_pump returns None immediately (player.rs:230-238), skipping the trailing mute_sweep+flush -- so the two waits cannot compound and the realistic worst case is one write timeout, not ~4 s. It also only bites against wedged hardware that has not yet errored. A doc note on into_device/Drop is the right-sized fix; low, not medium.

**[LOW · confusing] `web/task_worker.js:14`** — Both worker bootstraps claim onmessage is registered after init resolves; it is registered synchronously

`web/task_worker.js:14-19` says "Requests that arrive first are queued by the browser and delivered after `init` resolves because we only register `onmessage` afterwards" — but `self.onmessage` is assigned on line 19, in the same synchronous turn that calls `init()` on line 17; nothing waits for the promise. The handler fires immediately on the first message and the ordering actually comes from `await ready` on line 20. `web/pack_worker.js:68-80` repeats the same claim ("Registering onmessage only after this resolves means a request that arrives first is browser-queued until everything is ready") with the same synchronous assignment on line 80 and the same real mechanism on line 83.

The behaviour is correct; only the stated reason is wrong, which matters because someone trusting the comment could 'simplify' by removing the `await ready` that is doing the actual work.

*Fix:* Reword both to name the real mechanism: the handler is registered eagerly and awaits `ready` before touching the module, so a request that arrives during init is held in the handler rather than by the browser's queue.

*Checked and dismissed here:* NativePackService::cancel leaves the live counter standing, so a cancelled export still reports itself busy.
