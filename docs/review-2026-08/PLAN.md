# Review remediation — staged plan

Date: 2026-08-02. Status: **PLANNED — nothing implemented.** Branch: `review-2026-08`
(forked from `web-target` @ `df3d5cf`).

Works through the 233 findings in [REVIEW.md](REVIEW.md). Anything the plan
cannot decide on its own is in [DECISIONS.md](DECISIONS.md) rather than guessed
at — steps blocked on a decision say so and name it.

Every step below was pressure-tested by re-opening its cited code: anchors
re-verified, difficulty rated, blast radius grepped, and a test hook named. Four
anchors had drifted and several premises were wrong; those corrections are
folded in, and the ones that contradict REVIEW.md are called out in §0 rather
than quietly fixed.

---

## 0. Corrections to the review

The review was wrong about five things. They are recorded here because two of
them change what the work *is*, not just where it lands.

1. **Splitting `app.rs` is not mechanical.** Rust privacy is descendant-only, so
   roughly ninety methods moved into `app::*` submodules become invisible to the
   `handle_action` that stays behind — every one hits E0624. And the `mod.rs`
   layout the review implied would break the `#[path = "app_gui_tests.rs"]`
   mount (it would resolve to `src/app/app_gui_tests.rs`). See D18 and st-4.
2. **Finishing the DRO→VGM migration does not make the loop-widening bug
   impossible.** The review claimed it would. The defect lives in
   `repatch_header`, which Stage I never touches. sw-1 must be fixed on its own
   merits, and *before* mg-2 deletes the working reference implementation.
3. **`mg-1` destroys its own gate.** The moment `vgm::io::read` delegates to
   `vgm::file::read`, `assert_parity`, its three callers, the
   `any_opl_file_projects_identically` proptest and the corpus `compare()` all
   compare a function to itself and pass vacuously. The gate must be converted
   to golden fixtures *before* the delegation, not after.
4. **The WASI shim's debug default is not where the review said.** The singleton
   is constructed *disabled*; the `WASI` constructor enables it when the option
   is absent, and `enable()` never updates `isEnabled`, so `debug.enabled` is
   permanently stale. Same exposure, different fix site. See D4.
5. **`bits_in > 32` cannot crash.** `BitReader::read` already refuses more than
   32 bits, so the loop breaks harmlessly. Only `bits_out > 32` panics — and
   that one check closes the shift-overflow trigger too.

One factual conflict is **unresolved** and must be settled by looking, not
reasoning: whether any current e2e spec actually exercises the vendored WASI
shim. Two reviewers reached opposite conclusions about the pack fixture's chip.
See D6.

---

## 1. How to read this

Steps carry a two-letter prefix and a number (`hf-3`, `sw-1`). Each is rated:

- **mechanical** — an obvious edit with no design choice. Just do it.
- **contained** — one module, with a micro-choice a competent implementer makes
  alone.
- **needs-design** — blocked on [DECISIONS.md](DECISIONS.md), named inline.

Each step names the test that should fail before it and pass after. Where a step
is genuinely untestable (docs, comments, CI config) it says `n/a` rather than
inventing a hook.

**Standing rules for the whole programme:**

- One stage per branch-off, committed in its own commits; do not interleave
  stages that touch the same file (see §12).
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` stay green at every commit. Clippy is currently
  *silent*, and that is worth keeping as the baseline it is.
- Snapshot tests: regenerate with `UPDATE_SNAPSHOTS=1` only when a stage is
  *meant* to change pixels. Stage J must produce zero snapshot changes; if it
  does, something moved that should not have.
- Anything touching VGZ bytes, split output, or optimiser output must show a
  byte-parity run before it lands.

---

## 2. Stage order

Bug fixes and small refactors first, as asked. Two deliberate deviations are
justified below the table.

| | Stage | What it is | Blocked on |
|---|---|---|---|
| **A** | `hf-` | Hostile files — the crash class | D1 |
| **B** | `ci-` | Build and CI unblock | D11, (c) |
| **C** | `sw-` | Silent wrongness in core, synth, ui, native | D2, D3 |
| **D** | `wb-` | Web and worklet correctness | D4, needs B |
| **E** | `sn-` | Arm the safety nets | D5, D6 |
| **F** | `dd-` | Deletions and doc rot | D7, D8, D9, D10 |
| **G** | `tm-` | Terminology, API hygiene, manifests | D12, D13 |
| **H** | `fk-` | Unify the native/web forks | D14, D15 |
| **I** | `mg-` | Finish the DRO→VGM migration | D20, D21, D22 |
| **J** | `st-` | Structural splits | D16, D17, D18, D19 |

**Why Stage J is last even though it is refactoring.** `app.rs` carries 46 of
the 233 findings. Split it first and every one of those has to be re-anchored
into whichever child its code moved to — three of them turn from one-file edits
into three-file edits. It is the one step where doing the refactor first is
strictly more expensive.

**Why deletions (F) are not first even though they are cheapest.** dd-2's
cascades touch `parity/mod.rs`, `wav.rs`, both `pack.rs` files and `app.rs` —
files Stages C, D and E are actively fixing. Deleting under an in-flight bug fix
is worse than deleting after it.

**Why Stage B exists.** It was carved out of the old Stages 5 and 6 because
three later stages block on one CI line and one on manifest cleanliness. Left
buried mid-programme, they stall D and E for the price of a single commit.

---

## 3. Stage A — hostile files

**Goal:** a downloaded `.vgm`, `.vgz` or pack zip cannot crash the app, exhaust
memory, or wedge a render. This is the only class that hard-crashes the audio
callback, and it has the smallest blast radius in the programme.

| Step | What | Rated |
|---|---|---|
| **hf-1** | `decompress.rs:202` — reject `bits_out > 32` beside the existing `width == 0` guard. That single check closes both the `to_le_bytes()[..width]` slice panic at `:240` and the shift overflow at `:226`. | mechanical |
| **hf-2** | `decompress.rs:215` — cap `Vec::with_capacity(uncompressed_size)`, following `banks.rs:248`'s `length.min(0x1_0000)`. | mechanical |
| **hf-3** | One shared capped gunzip helper in a **new `vgm/gzip.rs`**, used by both `io.rs:80` and `file.rs:698`. Fold `write_gzipped` (`io.rs:169`, `file.rs:761`) in at the same time or the duplication survives. | needs-design (**D1**) |
| **hf-4** | `checked_add` for the untrusted-`u32` widenings: `header.rs:469, :586, :705, :797, :829, :838` and `file.rs:783`. Mirror the wording of the existing wasm32 guard at `io/dro.rs:186`. **Fold in the `slide_pointer` underflow at `file.rs:906`** — it is the line after hf-4's own `:905`. | contained |
| **hf-5** | `pack-archive/lib.rs:60` — cap per-entry decompressed size; check the declared size *and* read through `Read::take` so a lying header cannot beat the check. **Do sw-7's `:121` collision fix and the `:43` needless `to_vec()` in the same visit** — three ten-line changes in a 260-line file. | contained |
| **hf-6** | `dac_stream.rs:180` — clamp the `0x92` stream rate. | contained |
| **hf-7** | Check the hostile payloads from hf-1..hf-6 into `tests/` as a table-driven "returns `Err`, does not panic" corpus. **Not cargo-fuzz** — it needs nightly and the toolchain is pinned stable by policy. Last in the stage. | contained |

**Test hooks.** `a_block_declaring_more_than_thirty_two_bits_is_refused` beside
`an_unknown_scheme_is_refused`; `a_declared_size_does_not_become_an_unbounded_reservation`
— *assert on `capacity()`, not just the output, or the test passes before the
fix too*; `a_vgz_larger_than_the_ceiling_is_refused` **in both `io.rs` and
`file.rs`**, since the whole point is that a cap must not reach only one;
`an_entry_bigger_than_the_cap_is_skipped_not_read`;
`an_absurd_stream_rate_does_not_run_the_frame_forever`. hf-4's model already
exists: `a_corrupt_length_field_is_rejected_not_wrapped` in
`tests/wasm_roundtrip.rs:62` is the DRO twin.

**Watch for.** hf-1 panics at a different line in debug than release, so do not
assert on the panic message. hf-3 adds a documented failure mode to two public
functions of a permissively-licensed crate, and `Read::take` means the gzip
trailer's ISIZE is still never consulted — the error is "hit the ceiling", not
"declared too big". Say that in the doc comment.

**Exit:** every payload in hf-7's corpus returns an error; no new panic path;
`cargo test --workspace` green.

---

## 4. Stage B — build and CI unblock

**Goal:** stop CI lying, and remove the blockers Stages D and E sit behind.
About forty lines, all of it green.

| Step | What | Rated |
|---|---|---|
| **ci-1** | Delete `.github/workflows/check.yaml` — it runs on every push and fails on every push. | mechanical |
| **ci-2** | Delete `.github/workflows/build.yaml` (same dead Python pipeline, hidden by `workflow_dispatch`). Record in `DEVELOPMENT.md` whether the release gap is deliberate. | mechanical (**(c)**) |
| **ci-3** | Fix the web-dist copy manifest — `tools/build-web.ps1:61` and `rust.yaml:202` are two drifted copies of one list, and the CI copy **omits `web/wasi-shim/` entirely**, which is why the export specs are red the first time that job runs. `wasi-shim/` must travel as a directory with its two LICENSE files. | needs-design (**D11**) |
| **ci-4** | Manifest hygiene (was tm-3): workspace-inherit the `zip`/`flate2` pins in `pack-archive/Cargo.toml:20-21` and `vgms-web/Cargo.toml:40-41`; inherit `vgms-synth-worklet` at `vgms-web/Cargo.toml:95`; add the missing SPDX header to `vgms-vgmtools/src/lib.rs`; replace its `[lints.rust]` opt-out with `[lints] workspace = true`; fix the `src/dt.ico` comment in the root manifest (the file is `src/vgmstudio.ico`). | contained |

**Why ci-4 is here and not in Stage G.** The `zip`/`flate2` pins exist to keep
native and web VGZ output byte-identical; feature drift silently swaps
`flate2`'s backend. Landing the inheritance *before* any other dependency change
means its proof is an **empty `Cargo.lock` diff**. After Stage F or H removes
dependencies, that proof is gone.

**Exit:** only "Check Rust" queues on a push; `Cargo.lock` unchanged by ci-4;
`target/web-dist/` contains no `e2e/` and does contain `wasi-shim/`.

---

## 5. Stage C — silent wrongness

**Goal:** the twelve bugs where the app does the wrong thing quietly. Ten are
single-file. This is the stage the user's priority points at.

**Block 1 — mechanical, land first:**

| Step | What |
|---|---|
| **sw-5** | `app.rs:3943` `close_song_dialogs` — add `find_loop` and `split_songs`; cancel `TaskKind::LoopSearch` alongside the other cancels at `:2281` and in `close_song`. Assess `unwalkable_vgm` too. |
| **sw-10** | `chip_channels.rs:30` — `pan_to_i16(0xFF)` yields 254, not `0x100`, while the readout says R100. Match `dot_angle`'s asymmetric scaling. |
| **sw-12** | `retrowave/chip.rs:148` — drop the `!is_key_register(reg)` term so the NEW pre-raise matches its own comment and PLAN.md §3.4. |

**Block 2 — contained:**

| Step | What |
|---|---|
| **sw-2** | `audit.rs:157` — stop `fix()` rewriting loop fields the audit never reported. *Trap the review missed: `fix()` early-returns when `audit()` is empty, so the test needs an unrelated finding present.* |
| **sw-3** | `audit.rs:117` + `version.rs:38` — compute the audit's "needed" version without the writer's `FLOOR`, so genuine pre-1.50 files stop being reported as underclaimed. **Fix both call sites** (`:117` and `:168`). |
| **sw-4** | `audio-native/lib.rs:561` — record the cpal error and a `stopped` flag, mirroring `retrowave/player.rs:70`; implement `NativeAudioService::last_error`. **Verify first that the error callback is not the data callback** before proposing a `Mutex` — the crate's "nothing locks in the audio path" promise is load-bearing. Extract the recording into a free function so it is testable without a device. |
| **sw-7** | `services/file.rs:313` — Unicode-aware case-only rename detection. |
| **sw-8** | `cores-gpl/lle_opm.rs:153` — store the YM2164 variant on `Ym2151Lle` and derive every pin set from it. **Do not mirror the Nuked test** as the review suggested: `the_ym2164_variant_reaches_the_chip` only asserts both renders have energy, which passes with the pin broken. Write a real one. |
| **sw-9** | `chip_docs/opl.rs:253` — filter `documented_registers` through `register_doc`, and widen `notable_lists_exist_for_documented_chips` past `K::Ymf262`. That test fails today the moment the three other OPL chips are added to its loop. |
| **sw-11** | `tasks.rs:456` — throttle the loop search's per-candidate ranked clone, as `waveform.rs:189` already does for its stride. |

**Block 3 — decision-blocked:**

| Step | What | Blocked on |
|---|---|---|
| **sw-1** | `vgm/file.rs:674` `repatch_header` — preserve a deliberately-short loop end. **Must land before mg-2**, which deletes the working reference implementation. | **D2** |
| **sw-6** | `stream.rs` `command_wait` — the `0x64` override divergence. The match-on-value bug (a literal 735-sample `0x61` wait being re-mapped) gets fixed either way. | **D3** |

**Also pull forward:** `mg-3` (fold the four independent wait-chunkers) has no
dependency on the rest of Stage I and belongs here as a standalone.

**Exit:** each block committed separately; `cargo test -p vgms-ui` gains
`opening_a_second_song_closes_the_find_loop_dialog`; no snapshot changes.

---

## 6. Stage D — web and worklet correctness

**Goal:** stop leaking a live wasm instance per song, and stop late results
landing on the wrong document.

**This stage has no test harness.** `vgms-web`'s services are all
`#[cfg(target_arch = "wasm32")]` and the crate has no `wasm-bindgen-test`
dependency, so the only proof is Playwright — which needs ci-3 landed first.
**Decide the harness question at stage level** (Playwright-only, or add
`wasm-bindgen-test`) rather than per step.

Implement wb-1..wb-3 as **one pass over `load()`/`unload()`**, in this order:

| Step | What | Rated |
|---|---|---|
| **wb-2** | `worklet-processor.js:256` — add a `dispose` command that makes `process()` return `false` and drops the instance. Do this first; the Rust side needs something to post. | contained |
| **wb-1** | `web/services/audio.rs:166` — `load()` supersedes the node without disconnecting it. `self.unload()` alone is **not** the fix: `unload()` never calls `port.set_onmessage(None)` (leaving a ~43/s dropped-closure throw storm), `disconnect()` does not stop a processor whose `process()` returns true, dropping the state reset regresses `is_finished()`, and `unload()` must also bump the epoch. | contained |
| **wb-3** | Same file, `setup()` — add an epoch captured per load and re-checked after every await and immediately before install; the losing setup disposes its node and must not write `last_error`. Make `module_added` a stored promise every setup awaits, not a flag read across an await. | contained |
| **wb-8** | Same file, `:181` — return `Err` for a synchronous `ensure_context` failure instead of `last_error` + `Ok(())`. | mechanical |
| **wb-4** | `web/services/task.rs:184` — the generation filter the native service has and this one does not. **Extract the bookkeeping into a natively-buildable struct** (the crate already does this for `codec`) so it can be unit-tested off-target. | contained |
| **wb-5** | `task_worker.js:20` — a `finally` so an `init()` rejection cannot wedge the kind busy forever, plus `worker.set_onerror` on both services. Defer a `TaskResult::Failed` variant to whichever change fixes the native path too. | contained |
| **wb-6** | The shim debug default — see §0 item 4 and **D4**. | needs-design |
| **wb-7** | `task_worker.js:27` and `pack_worker.js:99` — transfer directly instead of `slice()`-then-transfer under a comment claiming zero-copy. | mechanical |

**Watch for.** `web/src/codec.rs` is the most-likely-forgotten file in the
programme: nothing in `cargo test --workspace` exercises the wasm decode path,
so any enum change must move encode and decode in the same commit.

**Exit:** ci-3 landed; a Playwright spec proving a second load leaves exactly one
connected node; no dropped-closure errors in the console during a load storm.

---

## 7. Stage E — arm the safety nets

**Goal:** make the machinery that reports success actually check something.
**Must precede Stages H and I**, or a parity run over their byte-changing work
means nothing.

| Step | What | Rated |
|---|---|---|
| **sn-1** | `tests/reference_parity.rs:635` — a reference-player error must fail the scorecard, not `continue`; assert a minimum comparison count so a run that compared nothing cannot report PASS. | contained |
| **sn-2** | `parity/reference.rs:266` — always re-copy the staged player (mechanical, land now); `:290` — the render cache key. | needs-design (**D5**) |
| **sn-3** | `tests/cli_smoke.rs:95` — `an_unknown_subcommand_is_rejected` passes because its input file does not exist, not because the subcommand is unknown; create the input first, using the file's own `temp_dir`/`small_song_bytes` helpers. Same shape at `:181`. Widen `help_lists_every_subcommand` from three subcommands to five. | mechanical |
| **sn-4** | `synth/tests/scratch_chip.rs:16` — cannot pass; `vgms-synth`'s own test binary installs no providers, so every core is `None` and the render is silence. Move it to `vgms-app` (with `install_cores()`) or delete it. | contained |
| **sn-5** | `tests/engine_corpus.rs:25` — the `Counting` core's `render` is `out.fill(0)` and `writes` is never read, so the only whole-corpus `VgmEngine` walk cannot detect misrouting. Add the cheap non-ignored test first. | contained |
| **sn-6** | The WASI shim gate — **but settle the D6 factual conflict first**. | needs-design (**D6**) |

**Sequencing note.** sn-1 and sn-2 must land **before** the `vgms_app::parity`
relocation in decision (d). A `git mv` after a content fix carries the fix with
it; a content fix rebased onto a moved file has to be re-anchored by hand.

**Exit:** sn-1's guard demonstrated by a deliberately broken reference path
producing a red test.

---

## 8. Stage F — deletions and doc rot

**Goal:** the largest reduction in surface for the least risk. Every later stage
gets smaller.

**Internal order matters: dd-2 before dd-6, dd-8 and dd-9** — all three
re-anchor onto lines dd-2 removes.

| Step | What | Rated |
|---|---|---|
| **dd-2** | The dead-code list in REVIEW.md §4. **Re-grep each before deleting**; the pressure-test found three of fourteen descriptions incomplete. Note `WRITE_CHAR_OPL` *is* referenced (at `dro.rs:139`) — its branch is unreachable, which is a different claim. | needs-design (**D7, D8**) |
| **dd-3** | Rewrite `DEVELOPMENT.md`: drop the Python transition claim and the five sections instructing against deleted files; fix the subcommand list (`convert` is not one; `optimize` and `retrowave-probe` are missing); align the wasm-check crate set with `rust.yaml`'s seven; add the web build, serve and e2e sections that have never existed. | contained |
| **dd-4** | `licenses/README.md` — add `vgms-pack-archive` and `vgms-vgmtools` to the app row, and `vgms-cores-libvgm` (with its absent-grant caveat) to the provider table. | needs-design (**D9**) |
| **dd-5** | `TODO.md`'s any-chip entry. | needs-design (**D10**) |
| **dd-6** | The nine `dro_split`/`dro_player` references in `vgms-synth` and `vgms-core` doc comments — the permissive crates outsiders read. dd-2 first makes it seven. | mechanical |
| **dd-7** | Reattach five misplaced doc comments (`vgm_engine.rs:326`, `tasks.rs:292`, `app.rs:53`, `config.rs:490` and `:891`). | mechanical |
| **dd-8** | The comments that contradict their code: `palette.rs:622/:674/:726` (**`:674` has drifted by one — re-locate**), `resample.rs:991`, `table.rs:3` (fold in the `header_height` lockstep fix while you are there), `credits.rs:46`, `chip_docs/mod.rs:209`. | contained |
| **dd-9** | Rewrap the string literals carrying embedded indentation: `parity/mod.rs:363/:374/:427`, `libvgm/chip.rs:772` (a user-facing `warn!`), `cores-nuked/opn2.rs:282/:512/:543`, `app_gui_tests.rs:2073`. Sweep with a grep for two spaces inside a literal. | mechanical |

**The gate for dd-2 already exists:** `cargo clippy --workspace --all-targets --
-D warnings` goes red on the orphaned imports the deletions leave behind. That
is the check, not a new test.

**Worth adding while here:** `every_crate_is_named_in_the_licence_split` — a test
walking the workspace members against `licenses/README.md`. It is the only thing
that stops dd-4 recurring.

**Exit:** clippy still silent; `cargo doc --workspace` builds; no `dro_split`
hits outside `docs/`.

---

## 9. Stage G — terminology, API hygiene, manifests

Renames land after deletions (fewer items to rename) and before file splits
(renaming inside a moved file is a worse diff).

| Step | What | Rated |
|---|---|---|
| **tm-2** | `Action::OptimizeImage` → `RecompressImage` (+ `app.rs`'s `optimize_image`), matching the UI's own deliberate distinction. **Before tm-1**, or carve `app.rs:3374`/`:3345` out of tm-1's scope explicitly. | contained |
| **tm-1** | The `optimise`/`optimize` identifier sweep: `Optimised` → `Optimized`, `unoptimised_chips` (now deleted by dd-2), the `vgms-ui::optimise` module, and the user-visible strings at `strings.rs:55/:65/:364` and `app.rs:3374`. | needs-design (**D12**) |
| **tm-4** | The corpus environment variables. | needs-design (**D13**) |
| **tm-6** | Add `crates/vgms-app/tests/common/mod.rs` — the recursive `.vgm`/`.vgz` collector exists four times, all subtly different, and the "render N seconds into interleaved i16" loop repeats too. `vgms-synth/tests` already has one. **After tm-4**, so the shared preamble is written once against the settled variable names. | needs-design |
| **tm-5** | Make `vgms-ui`'s `widgets` tree `pub(crate)` and report what the compiler then flags — rustc's `dead_code` lint currently cannot see any of it, which is how dead palette roles survived. Also `editor.rs:1006 row_analysis`. **Last of any stage that edits `widgets/`.** | contained |

**Exit:** one spelling in identifiers; `cargo clippy -p vgms-ui` reports whatever
tm-5 exposed, and that list is triaged (not necessarily deleted) before the
stage closes.

---

## 10. Stage H — unify the native/web forks

Land **fk-3 and fk-4 first** — both touch files fk-1 then relocates.

| Step | What | Rated |
|---|---|---|
| **fk-3** | Fold `optimize_tools.rs:117 describe()` and `services/file.rs:210 js_error()` into one helper. | mechanical |
| **fk-4** | `vgmtools/lib.rs:203 run_tool` + `collect` re-implement the exit-code interpretation `command.rs` exists to hold once; `strip.rs:157` does it a third time; `suffix()` is duplicated verbatim. Also reconcile the tool "tail": native returns three lines, web returns one. | contained |
| **fk-2** | Extract `register_common_cores` so the provider order and the three promotions exist once. | needs-design (**D15**) |
| **fk-1** | One pack-zip builder. Highest fan-out in the programme: a dependency-cycle trap, 13+7 tests to merge, a public API change on `vgms_app::build_pack_zip`, and three manifests other steps also edit. | needs-design (**D14**) |

**Mitigation for fk-1:** move `PackEntry` down as its own commit, then the
builder, then delete the native copy. Add the test nothing currently has — the
same entry list producing identical names, log lines and song bytes on both
targets.

**Exit:** `pack_flow.rs::scan_build_and_reopen_a_release_zip` green unchanged;
the cross-target equivalence test passing.

---

## 11. Stage I — finish the DRO→VGM migration

The payoff stage, and the one most able to go wrong. **Read Risk 2 in §13
before starting.**

| Step | What | Rated |
|---|---|---|
| **mg-0** | *(new, from the pressure-test)* Convert the parity assertions to golden-bytes / golden-`Song` comparisons against checked-in fixtures **before** any delegation. Without this the gate evaporates the moment mg-1 lands. | contained |
| **mg-1** | Route `vgm::io::read` through `vgm::file::read` + `to_song`. | needs-design (**D20**) |
| **mg-2** | Delete `VgmData::read_from_stream`, `resolve_loop_point`, `resolve_loop_end`, and the private `opl_type_of` duplicate at `io.rs:261`. **After sw-1.** | mechanical |
| **mg-4** | Rewrite `loop_end_index` over `wait_prefix` + `partition_point`. **Not a de-dup** — mg-2 deletes one of its two call sites, so it is a rewrite. Needs a new `VgmStream` method; `wait_prefix` is private. | contained |
| **mg-5** | One `Editor::doc_source()`; express the four identically-shaped task-source enums in terms of it and collapse the six repeated matches in `app.rs`. | needs-design (**D21**) |
| **mg-6** | The two state-restore stacks. | needs-design (**D22**) |
| **mg-7** | Rename one of the two `redundant_indices`; stop re-exporting the OPL one bare at the crate root. **Rename, do not fold.** | contained |

*(mg-3 moved to Stage C.)*

**Evidence required before mg-1 is considered done:** paste the corpus run's
printed line into this document — scanned, OPL files both readers accept,
agreed, newly openable, split pieces checked — with `agreed == opl` and `opl` in
the thousands. Expect new `optimize_corpus` failures from v1.00–1.50 files that
were never readable before; those are new coverage, not regressions.

---

## 12. Stage J — structural splits

Last, and each split committed **alone**. Order: **st-6 → st-7 → st-5 → st-8 →
st-2 → st-3 → st-1 → st-4.**

| Step | What | Rated |
|---|---|---|
| **st-6** | `app_gui_tests.rs` (7,399) → `app_gui_tests/mod.rs` + one child per section banner; move `act`, `pack_section` and `drag_by` up beside `build`. **First**, so st-4 lands against a split test suite. | mechanical |
| **st-7** | `libvgm/chip.rs` (2,516) → `specs.rs` + `fold.rs`, leaving the unsafe wrapper. | contained |
| **st-5** | `vgms-ui/pack.rs` (3,845) → `pack/{state,tags,view}.rs`; move the four model items currently sitting inside the `-- view --` section. | contained |
| **st-8** | `vgms-core/pack.rs` (2,412) → `pack/{readiness,naming}.rs`; refresh the module doc. | needs-design (**D19**) |
| **st-2** | The dialog footer scaffold. Batch the caption-toggle helper and `fmt_time` de-dup findings into this step — they touch six of the same ten dialogs. | needs-design (**D17**) |
| **st-3** | Port `BulkTagDialog` onto `dialog_modal_sized`; drop the `area` parameter and its special case in `show_all`. | contained |
| **st-1** | The dialog registry. Generate it from the **corrected** slot list, i.e. after sw-5. | needs-design (**D16**) |
| **st-4** | `app.rs` (4,807) → `src/app.rs` head + `src/app/*.rs`. **Non-`mod.rs` layout** (D18). Must be the last commit in the entire programme to touch this file. | needs-design (**D18**) |

**Acceptance criterion for every split:** `cargo test -p <crate> -- --list`
returns an identical test-name set before and after, zero snapshot rewrites, and
the diff contains no logic change beyond visibility keywords.

---

## 13. Conflict map

Files touched by more than one stage. Sequence these; never parallelise them.

| File | Stages | Implication |
|---|---|---|
| `vgms-ui/src/app.rs` | C, D, F, G, I, J | 46 findings. **st-4 is the last commit to touch it, and goes alone.** Everything else edits it as a flat file. |
| `vgms-core/src/vgm/file.rs` | A, C, I, G | hf-4's `:905` and the `slide_pointer` underflow at `:906` are consecutive lines in two stages — **merge into one two-line fix in Stage A.** |
| `vgms-core/src/vgm/io.rs` | A, I | Resolved by putting hf-3's helper in a new `vgm/gzip.rs`; mg-1 then only deletes a caller. |
| `vgms-core/src/vgm/stream.rs` | C, I | Coupled through `wait_prefix`. D3 resolution (B) decouples them. |
| `vgms-pack-archive/src/lib.rs` | A, C, H | **Fold hf-5, sw-7 and the `:43` clone into one Stage A visit;** fk-1 then only adds a `pub mod` line. |
| `vgms-ui/src/tasks.rs` | C, D, I | mg-5 rewrites what sw-11 patches. C → D → I. The ChannelGate programme rebases onto mg-5, not the reverse. |
| `vgms-web/src/codec.rs` | D, I | Nothing in `cargo test --workspace` exercises it. Encode and decode move in the same commit, always. |
| `parity/mod.rs` + `tests/reference_parity.rs` | E, F, (d) | E → F → relocation. D8's deletion first shrinks what moves. |
| `vgms-ui/src/dialogs/mod.rs` | J | st-2 → st-3 → st-1 internally; sw-5 (Stage C) before st-1. |
| `vgms-synth/src/wav.rs` | F | dd-2 before dd-6 turns nine references into seven. |
| Root + crate manifests | B, F, H | **ci-4 before any dependency change**, so its proof is an empty lock diff. |
| `.github/workflows/rust.yaml`, `tools/build-web.ps1` | B, E, F | All manifest work lands in B, including what was dd-10. |
| `vgms-vgmtools/src/lib.rs` | B, G, H | Three disjoint hunks. B → G → H. |

---

## 14. Risk callouts

**Risk 1 — st-4, the `app.rs` split.** Not mechanical (see §0). Take the
non-`mod.rs` layout, land st-6 first, schedule it after every other `app.rs`
step, and commit it alone. Acceptance: identical test-name list, zero snapshot
rewrites, no logic change beyond `pub(super)`.

**Risk 2 — mg-1, delegating the OPL reader.** It destroys its own gate (§0 item
3), and every acceptance-widening it introduces is invisible to that gate
because newly-openable files count as success. Do mg-0 first. Take D20
resolution (B) so nothing observable changes on the first commit. Require the
pasted corpus evidence.

**Risk 3 — wb-1 + wb-2 + wb-3, the orphaned worklet node.** Four interacting
defects across an async boundary in a crate with **no test harness at all**, and
the e2e job is red until ci-3 lands. Do ci-3 first, implement in the order given
in §6, and extract what can be tested natively (wb-4's generation filter is the
obvious candidate).

**Honourable mention — fk-1**, for fan-out. Mitigations in §10.

---

## 15. What this plan does not cover

- The `render-split-2026-08` ChannelGate programme is independent, but **it must
  rebase onto mg-5**, not the reverse, and it names `RecordingChip` as its test
  vehicle — so `RecordingChip` is not deletable (only its three false doc
  comments get fixed, in dd-8's neighbourhood).
- `tests/make_screenshot.py` stays. It is unreferenced but it *generates*
  `tests/screenshot.png`, which five Rust sites embed; the fix is a provenance
  pointer in the consumers, not deletion.
- Performance findings below the "silent wrongness" bar (the `0x93` double bank
  copy, `readiness_items()` running twice per frame, `encode_wait` rebuilding its
  tables per call) are left in REVIEW.md as a backlog, not staged here.
