# Review remediation — staged plan

Date: 2026-08-02. Status: **PLANNED — nothing implemented.** Branch: `review-2026-08`
(forked from `web-target` @ `df3d5cf`).

**Revision 3** — every decision is answered (see [DECISIONS.md](DECISIONS.md));
§0b records where research changed the work's shape, and §12b scopes the
owner-requested follow-on: retiring the projection concept entirely.

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

The one factual conflict is now **resolved**: `tests/e2e-pack.zip` decodes to two
v1.51 **YM3812-only** tracks, so the wholly-OPL bypass always fires and the
vendored WASI shim has **zero runtime coverage anywhere** — no spec reaches it,
CI ships neither it nor the `tool_*.wasm`, and the smoke test uses `node:wasi`
instead. See D6.

---

## 0b. What the decisions changed

Six things moved once the open questions were researched. Each is folded into
the stages below; they are listed here because the *shape* of the work changed,
not just its go-ahead.

1. **The 256 MiB gunzip cap is not sufficient on its own.** The `vgm::file` path
   amplifies ~12× into the command index, so 256 MiB decompressed still implies
   ~3 GB of index against wasm32's 4 GiB. **hf-3 now also bounds the command
   count in `VgmStream::parse`.**
2. **`0x64` is a withdrawn proposal.** One spec revision (v1.70), authored with
   the note *"Am I really sure about this?"*, gone by v1.71; both libvgm and
   legacy VGMPlay classify it **invalid** and stop playback; **zero occurrences
   in 73,400 corpus files.** sw-6 becomes a five-line deletion, and the
   match-on-value bug it carried **disappears for free**.
3. **Split Songs' OPL path has an audible bug, and the review had it backwards.**
   `materialise` synthesises a v1.51 header with hard-coded clocks, so a rip with
   a non-canonical clock is split **at the wrong pitch and tempo**. Routing to
   the VGM stack is a fix, not a byte regression — and Crop already does it.
4. **The dialog registry is dropped.** Only two dialogs are modeless and both are
   already safe; the one real hole is that `handle_drops` reads raw OS drop
   events a `Modal` cannot block. st-1 is gone; sw-5 becomes a drop gate.
5. **`SongData::Vgm` is the OPL projection's carrier**, not just the legacy
   reader's output — so Stage I's target is reachable without the ~108-site
   port.
6. **No Node build system.** `web/e2e/` moves out of `web/` so the copy manifest
   stops existing, and CI calls `build-web.ps1` rather than re-implementing it.

---

## 1. How to read this

Steps carry a two-letter prefix and a number (`hf-3`, `sw-1`). Each is rated:

- **mechanical** — an obvious edit with no design choice. Just do it.
- **contained** — one module, with a micro-choice a competent implementer makes
  alone.
- **needs-design** — carries a real design step the implementer must work out
  in place (all owner decisions are answered; see [DECISIONS.md](DECISIONS.md)).
  Remaining: mg-3b's `compare_optimised` re-basing, and Stage K's k-3/k-6.

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

**Every decision is answered — all ten stages are ready.** The only ordering
constraints left are the mechanical ones (B before D's test hooks, E before the
byte-changing stages, st-4 last).

| | Stage | What it is |
|---|---|---|
| **A** | `hf-` | Hostile files — the crash class |
| **B** | `ci-` | Build and CI unblock (D11: `web/e2e/` moves out, CI calls the script) |
| **C** | `sw-` | Silent wrongness in core, synth, ui, native |
| **D** | `wb-` | Web and worklet correctness (D4: wrapper + smoke-test repoint; needs B) |
| **E** | `sn-` | Arm the safety nets (D5: parity stays; fix it) |
| **F** | `dd-` | Deletions and doc rot |
| **G** | `tm-` | Terminology, API hygiene, manifests |
| **H** | `fk-` | Unify the native/web forks |
| **I** | `mg-` | Finish the DRO→VGM migration (D21, D22 accepted) |
| **J** | `st-` | Structural splits |
| **K** | — | *Follow-on, separate programme:* retire the projection (§12b) |

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
| **hf-3** | One shared capped gunzip helper in a **new `vgm/gzip.rs`**, used by both `io.rs:80` and `file.rs:698`; cap **256 MiB, identical on both targets**. Fold `write_gzipped` (`io.rs:169`, `file.rs:761`) in at the same time or the duplication survives. **Also bound the command count in `VgmStream::parse`** — the byte cap alone leaves ~3 GB of index reachable on wasm32 (see D1). | contained |
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
| **ci-3** | **Make the copy manifest stop existing** (D11 option D, accepted). Move `web/e2e/` → `web-e2e/`, so `web/` means exactly "files the browser gets" and `Copy-Item web/*` is correct by construction. Then replace `rust.yaml:195-202`'s hand-copied block with a call to `tools/build-web.ps1` — proven feasible, since `rust.yaml:132` already runs `shell: pwsh` on Ubuntu for the sibling script. Requires normalising five Windows-separator paths in the script (lines 24, 25, 56, 61, 69) and adding `-SkipWasiTools`. Fixes the live bug: the CI copy omits `web/wasi-shim/`, which `pack_worker.js:18` imports at top level. | contained |
| **ci-5** | *(optional, free win)* `wasm-bindgen-cli` does not run `wasm-opt`, so the app module ships at 12.7 MB. Four lines plus a binaryen prerequisite typically takes 20–40% off. | mechanical |
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
| **sw-5** | **Gate file drops while a modal is open.** `handle_drops` (`app.rs:1349`, called unconditionally at `:500`) reads `ctx.input(\|i\| i.raw.dropped_files)` — raw OS events no `Modal` can block — so dropping a `.vgm` while Find Loop is up swaps the song underneath it and Apply writes the old song's row indices into the new one. Return early with a status line when `dialogs.any_open()`. Per D16 this replaces both the dialog-closing lines and the `LoopSearch` cancellation: the song can no longer change under a running search. |
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

**Block 3 — answered, but larger than the rest:**

| Step | What |
|---|---|
| **sw-1** | `vgm/file.rs:674` `repatch_header` — preserve a deliberately-short loop end through `rebuild` and all three region edits (D2 resolution b). **Must land before mg-2**, which deletes the working reference implementation. |
| **sw-1b** | *(new, from D2)* Rather than exempting the optimiser: **validate after the pass** that the loop length survives — merging delays must still yield the same total play time. A post-condition on `VgmFile::optimize`, not a second merge barrier. |
| **sw-6** | `vgm_engine.rs:623` — **stop honouring the `0x64` override** and document the divergence at `command_wait` (D3 resolution B). A five-line deletion. The match-on-value bug needs no separate fix: with the override gone, `wait_60hz`/`wait_50hz` are constants and a literal `0x61 DF 02` can no longer be remapped. **Also correct `version.rs:100`**, which maps `OverrideWait` to v1.50 — `0x64` was v1.70-only. Keep decoding the opcode; refusing it would fail an openable file for no benefit. |

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
| **wb-6** | The shim debug default — see §0 item 4. Add `web/wasi-host.js` owning argv, fds and `debug: false`, keeping the vendored files byte-identical. **And give the shim a test**: point `tools/web/vgmtools_smoke.mjs` at `web/wasi-shim/index.js` instead of `node:wasi`, so something would notice if it broke. Today nothing would — see D6. | contained |
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

> **Blocked on D5.** The owner proposed deleting the parity checks; the evidence
> says keep them (four bugs caught *after* the clean-room cull, 39 unmeasured
> chip levels still open, and nothing else covers multi-chip mixing or absolute
> level). If the answer is "delete anyway", sn-1 and sn-2 disappear along with
> standing question (d)'s `vgms-parity` crate, and sn-3..sn-6 stand alone.

| Step | What | Rated |
|---|---|---|
| **sn-1** | `tests/reference_parity.rs:635` — a reference-player error must fail the scorecard, not `continue`; assert a minimum comparison count so a run that compared nothing cannot report PASS. | contained |
| **sn-2** | `parity/reference.rs:266` — always re-copy the staged player (mechanical, land now); `:290` — the render cache key. | contained |
| **sn-3** | `tests/cli_smoke.rs:95` — `an_unknown_subcommand_is_rejected` passes because its input file does not exist, not because the subcommand is unknown; create the input first, using the file's own `temp_dir`/`small_song_bytes` helpers. Same shape at `:181`. Widen `help_lists_every_subcommand` from three subcommands to five. | mechanical |
| **sn-4** | `synth/tests/scratch_chip.rs:16` — cannot pass; `vgms-synth`'s own test binary installs no providers, so every core is `None` and the render is silence. Move it to `vgms-app` (with `install_cores()`) or delete it. | contained |
| **sn-5** | `tests/engine_corpus.rs:25` — the `Counting` core's `render` is `out.fill(0)` and `writes` is never read, so the only whole-corpus `VgmEngine` walk cannot detect misrouting. Add the cheap non-ignored test first. | contained |
| **sn-6** | The WASI shim gate. D6 is settled: the fixture is YM3812-only, so the bypass always fires and **no spec has ever reached the shim**. The node route (wb-6's smoke-test repoint) is the cheap half; a browser gate additionally needs a committed non-OPL fixture, a rebuilt `e2e-pack.zip`, and `wasi-shim/` + `tool_*.wasm` shipped into the e2e dist. Note the bypass itself is removed in Stage I, not here. | contained |

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
| **dd-2** | The dead-code list in REVIEW.md §4, **including the permissive crates' unused public API** (D7: this repo is the only consumer, so trim). **Re-grep each before deleting**; the pressure-test found three of fourteen descriptions incomplete. Note `WRITE_CHAR_OPL` *is* referenced (at `dro.rs:139`) — its branch is unreachable, which is a different claim. | contained |
| **dd-2a** | **Delete the `vgms-cores-ymfm` crate** (standing question (a)): the crate, its `[workspace.dependencies]` entry, its submodule, its `licenses/README.md` row and its `rust.yaml` wasm-check entry. | mechanical |
| **dd-2b** | **Delete the clean-room concept** (D8): `Regime`, `Threshold::regime`, `max_envelope`, the `shared()` const fn, the unreachable test branch, and the module doc still describing two live regimes. Also correct `PARITY-PLAN.md` and `LIBVGM-PLAN.md:226`, which still says "the scorecard remains the arbiter" while contradicting its own retirement header. **Order before standing question (d)'s crate move**, so less code moves. | mechanical |
| **dd-3** | Rewrite `DEVELOPMENT.md`: drop the Python transition claim and the five sections instructing against deleted files; fix the subcommand list (`convert` is not one; `optimize` and `retrowave-probe` are missing); align the wasm-check crate set with `rust.yaml`'s seven; add the web build, serve and e2e sections that have never existed. | contained |
| **dd-4** | `licenses/README.md` — add `vgms-pack-archive` and `vgms-vgmtools` to the app row, and `vgms-cores-libvgm` to the provider table, **noting that libvgm ships no explicit licence grant and is assumed GPL-2.0-or-later** (D9). Drop the `vgms-cores-ymfm` row along with the crate (dd-2a). | contained |
| **dd-5** | **Delete `TODO.md`** (D10). | mechanical |
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
| **tm-1** | The `optimise`/`optimize` identifier sweep (D12 resolution a — US in identifiers **and** user-visible strings, British stays in comments/logs/docs): `Optimised` → `Optimized`, `unoptimised_chips` (now deleted by dd-2), the `vgms-ui::optimise` module, and the strings at `strings.rs:55/:65/:364` and `app.rs:3374`. | contained |
| **tm-4** | **Collapse onto one corpus variable, `VGMSTUDIO_VGMRIPS_CORPUS`** (D13 — one tree is easier for others to set up). Repoint the four suites reading `VGMSTUDIO_CORPUS`; keep the loud skip, so a required-but-unset corpus fails naming the variable rather than `eprintln`-ing and returning. The two `vgms-vgmtools` suites cannot see `vgms-app`, so they need their own small fallback. | contained |
| **tm-6** | Add `crates/vgms-app/tests/common/mod.rs` — the recursive `.vgm`/`.vgz` collector exists four times, all subtly different, and the "render N seconds into interleaved i16" loop repeats too. `vgms-synth/tests` already has one. **After tm-4**, so the shared preamble is written once against the settled variable name. | contained |
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
| **fk-2** | Extract `register_common_cores` into a **new GPL-2.0-or-later `vgms-cores` crate** (D15 resolution A). Signature must be `fn register_common_cores(&mut CoreRegistry)` — `install` is a process-global one-shot, so a comparison test needs a registry it can build without installing. New crate means joining three hand-maintained lists: `[workspace.dependencies]`, `licenses/README.md`, and `rust.yaml`'s wasm-check line. | contained |
| **fk-1** | One pack-zip builder: **move `PackEntry`/`PackEntryKind` down into `vgms-pack-archive`** and re-export from `vgms-ui` (D14 resolution 1). The shared builder takes `Option<&dyn ImageOptimizer>`; the web supplies a null optimizer that logs its own browser-specific line, so no browser sentence lands in a target-independent crate. Highest fan-out in the programme: 13+7 tests to merge, a public API change on `vgms_app::build_pack_zip`, and three manifests other steps also edit. | contained |

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

**Target (D20 + (e)):** DRO opened by exactly one path, VGM by exactly one, no
fallback — reached via shape **(i-b)**. `SongData::Vgm` *stays*, demoted to the
OPL projection's carrier, so the ~108-site projection port is not part of this
stage.

| Step | What | Rated |
|---|---|---|
| **mg-0** | **✓ DONE (`4b3f63b`).** Reground the reader-parity assertions on checked-in goldens under `tests/golden/projection_*.opl.vgm` (each = `io::write(io::read(input))`, frozen while both readers exist; a `UPDATE_GOLDENS=1` regenerator re-derives them and proves parity at capture). `assert_projects_to_golden` compares the projection's `io::write` to the golden -- lossless, so it subsumes the old per-field checks and survives mg-1 + mg-2. Proptest keeps only its redundancy half; corpus `compare()` reader-parity link dropped. | contained |
| **mg-0b** | **✓ DONE (`7bfef5a`).** By-cause tally added to the corpus harness + a guard test; baseline captured over the canonical corpus (72,481 files) into `mg1-baseline-projection-corpus.txt`: **4,541 OPL, all agreeing; 67,922 newly openable** (15,547 old version, 51,979 non-OPL chip, 396 unsupported command); 18 unreadable by both. (Headline was ~12,533; the real figure is 67,922.) | contained |
| **mg-1** | **✓ DONE (`beaf7e3`), single commit.** Delegate `vgm::io::read` to `file::read` + `to_song`; delete the dead old path (`read_uncompressed`, `resolve_loop_point`/`resolve_loop_end`, the private `opl_type_of`, `VgmData::read_from_stream`); keep `io::write`. ✏️ **REVERSAL — the staged gate removals do not exist.** `io::read` returns the OPL projection, and `to_song` is `None` for anything not a wholly-OPL, OPL-clock file; a pre-v1.51 file has **no OPL clock field at all** (verified: forcing v1.50 on the fixture makes `to_song` None). So the v1.51 / OPL-chip / wholly-OPL checks are each **redundant with `to_song`** -- removing them opens **zero** new files, only changes the error message. They stay as faithful messages until mg-2 deletes the wrapper. The 67,922-file widening is through `file::read` (the editor path, mg-1b), not this projection wrapper. | contained |
| **mg-1b** | **✓ DONE (`0400628`).** Delete the unreachable VGM fallback in `editor.rs::load` (route non-VGM straight to `io::dro::read`) ((e)). Verified unreachable: both readers share `VgmHeader::parse`, after which `file::read` accepts a strict superset. | mechanical |
| **mg-2** | Make `read_song` DRO-only; delete `vgm::io::read`, `read_uncompressed`, `VgmData::read_from_stream`, `resolve_loop_point`, `resolve_loop_end`, and the private `opl_type_of` duplicate (at `io.rs:258`, not `:261`). **KEEP `io::write` and the whole write side** — ✏️ the original "delete `io::read`/`write`" was wrong: D20/(i-b) keeps `SongData::Vgm` a *writable* projection carrier (REVIEW.md:312). `read_song`'s many VGM callers must be re-routed to `file::read` first. **After sw-1 (done) and mg-1.** | contained |
| **mg-2b** | **⊘ CANCELLED.** Owner directive (2026-08-03): keep `optimize::optimize` (it may still serve as a `vgm_cmp` alternative). The map also found the premise false — the Song-side crop/split arms are *live* paths (not zero-caller: DRO crop/split use them, and VGM split used them until mg-6), and `merge_stream_delays` is a live `VgmFile::optimize` dependency, so `optimize.rs` cannot be deleted wholesale. Nothing safe to delete; dropped. | — |
| **mg-3b** | **⊘ OBSOLETE — superseded by the optimizer-investigation merge.** That merge already replaced the wholly-OPL bypass with the generalized `built_in_covers_all` routing behind a user-facing `OptimizerChoice` (Auto/BuiltInOnly/Tools); removing it would undo the shipped feature. Its D6 rationale is void: the WASI shim/tools path is already reached via `OptimizerChoice::Tools` (exercised by `optimize_parity.rs`, `tools.rs`), so sn-6 needs no bypass removal; and `compare_optimised` never called the pipeline (it diffs `file.optimize()` against `optimize::optimize` directly), so there is nothing to re-base. No code change. | — |
| **mg-4** | **✓ DONE (`4c9712a`).** Rewrite `loop_end_index` over a new `VgmStream::boundary_after` (`partition_point` over the private `wait_prefix`). **Not a de-dup** — mg-2 deletes `resolve_loop_end`, so these stay two functions over two prefix types. | contained |
| **mg-5** | One `Editor::doc_source()` returning a **cached `Arc<VgmFile>` rebuilt in `bump_revision`** (⚠️ also invalidate it in `record_saved` and the GD3/metadata setters, which mutate `self.vgm` without bumping the revision), plus collapsing the three task-source enums and the OPL-first `Arc::new(file.clone())` sites. The shared type goes in **`vgms-core`** (not `vgms-ui`, which is GPL, and not a second type beside `AudioSource` — alias `AudioSource` to it). `can_preview` stays in the UI. ⚠️ `doc_source()` is **OPL-first** (audio/render/loop-search play the projection until Stage K); `split_source` must stay **vgm-first** (mg-6), so it does NOT collapse into `doc_source()`. `SplitTaskSource` does not collapse (it carries option types). | contained |
| **mg-6** | **✓ DONE (`edf1ccf`).** Route Split Songs to the VGM stack — invert the match in `split_source()` to ask `editor.vgm()` first, matching Crop's `replace_stream`. **Bug fix**: the OPL path synthesised a v1.51 header with hard-coded clocks, so a rip with a non-canonical clock split at the wrong pitch/tempo. `can_preview()` now mirrors `Editor::renderable` (`to_song().is_some() || playability(chips)`), computed once at dialog construction, so OPL VGMs keep their Preview button. `opl_state.rs`/`state_patch.rs` stay. | contained |
| **mg-7** | **✓ DONE (`3bb447a`).** Renamed the OPL `redundant_indices` → `redundant_write_indices` and dropped its bare crate-root re-export. The `chip_state` one keeps its name. **Rename, not fold.** | contained |

*(mg-3 moved to Stage C. mg-1b/mg-4/mg-6/mg-7 landed 2026-08-03 on `stage-i-migration`, which also carries the optimizer-investigation merge. mg-2b and mg-3b are retired above. Remaining: mg-5, then the delegation spine mg-0 → mg-0b → mg-1 → mg-2.)*

**Evidence (mg-0b, ✓ captured 2026-08-03):** `scanned 72,481; OPL both readers
4,541, agreed 4,541 (== opl, in the thousands ✓); newly openable 67,922; split
pieces checked 12,791; unreadable by both 18` — full run in
`mg1-baseline-projection-corpus.txt`. Zero disagreements, so the projection is a
faithful replacement. Note the widening is realised through `file::read` (the
editor, mg-1b), which is what these newly-openable files open through; `io::read`
itself stays OPL-only (see the mg-1 reversal above).

---

## 12. Stage J — structural splits

Last, and each split committed **alone**. Order: **st-6 → st-7 → st-5 → st-8 →
st-2 → st-3 → st-4.**

*(st-1, the dialog registry, is **dropped** — see D16. The lockstep is not a
correctness risk because only two dialogs are modeless and both are already
safe; the one real hole is handled by sw-5's drop gate in Stage C.)*

| Step | What | Rated |
|---|---|---|
| **st-6** | `app_gui_tests.rs` (7,399) → `app_gui_tests/mod.rs` + one child per section banner; move `act`, `pack_section` and `drag_by` up beside `build`. **First**, so st-4 lands against a split test suite. | mechanical |
| **st-7** | `libvgm/chip.rs` (2,516) → `specs.rs` + `fold.rs`, leaving the unsafe wrapper. | contained |
| **st-5** | `vgms-ui/pack.rs` (3,845) → `pack/{state,tags,view}.rs`; move the four model items currently sitting inside the `-- view --` section. | contained |
| **st-8** | `vgms-core/pack.rs` (2,412) → `pack/{readiness,naming}.rs` with `pub mod` and **no re-export** (D19 resolution b); refresh the module doc. Breaks import paths for ~24 call sites, which is acceptable per D7. | contained |
| **st-2** | **A real `Footer` widget** (D17): one common basic footer offering Save or Close, used by the nine dialogs that need nothing more; anything richer (Find Loop's third button) supplies its own. **Fix DRO Info's label flipping** so its buttons sit where every other dialog puts them. Batch the caption-toggle helper and `fmt_time` de-dup findings in — they touch six of the same dialogs. | contained |
| **st-3** | Port `BulkTagDialog` onto `dialog_modal_sized`; drop the `area` parameter and its special case in `show_all`. | contained |
| **st-4** | `app.rs` (4,807) → `src/app.rs` head + `src/app/*.rs`. **Non-`mod.rs` layout**, `pub(super)` on the ~90 moved methods (D18 resolution i); splitting `handle_action` follows as its own commit (iii). Must be the last commit in the entire programme to touch this file. | contained (**highest risk** — §14) |

**Acceptance criterion for every split:** `cargo test -p <crate> -- --list`
returns an identical test-name set before and after, zero snapshot rewrites, and
the diff contains no logic change beyond visibility keywords.

---

## 12b. Stage K — retire the projection (follow-on programme)

**Requested by the owner: "consider how to remove the projection concept
entirely."** Scoped here so the earlier stages can anticipate it; it is a
**separate programme**, not part of this remediation's exit criteria, because it
is gated on two other bodies of work finishing first.

### What the projection is, and who leans on it

`Editor` holds `projection: Option<Arc<Song>>` (`editor.rs:173`), rebuilt on
every edit by `refresh_projection` — `VgmFile::to_song()`, ~22 ms on a 4 MiB OPL
VGM. `song()` and `snapshot()` prefer it, so **for an OPL VGM every consumer of
`Song` is really consuming the projection**: the OPL `PlayerEngine` (native
audio via `Engine::Opl`, the worklet's OPL arm, `peak`/`wav`/`waveform`), the
RetroWave hardware player (`player.rs:224` builds `PlayerEngine::with_chip`
directly over the `Song`), the register analyser (`RegisterAnalyzer`,
`RegisterUsage`, `initial_channel_pans` — all `&Song`-typed), the editor's OPL
row rendering, the pack preview, and the six task sources.

The end state: **an OPL VGM takes exactly the path every other VGM takes**
(`VgmEngine` + the registered nuked-opl3 core), `Song` means "a DRO document"
and nothing else, `SongData::Vgm` is deleted, and `PlayerEngine` shrinks to the
DRO engine. This is the same destination the render-split plan names as its
long-term goal ("no code that treats OPL songs differently from any other VGM").

### Gates — why K cannot start yet

1. **Stage I** must land first: one reader per format, `doc_source()` in place
   (mg-5's `Opl` arm then shrinks to meaning "DRO", which is the shape K wants).
2. **The ChannelGate programme** (`docs/render-split-2026-08/PLAN.md`) must land
   first: OPL channel muting today is `Muting::gate` inside `PlayerEngine` —
   drop muted channels' `0xB0..=0xB8`, mask `0xBD`, seek-replay rules. Routing
   OPL VGMs through `VgmEngine` before an equivalent exists on that path loses
   working mute/solo. ChannelGate *is* that equivalent, generalised.
3. **An A/B render gate**: before any routing flips, a fixture-and-corpus test
   rendering the same OPL VGM through both engines and comparing output. The
   engines differ in write pacing (nuked's buffered writes vs `VgmEngine`'s
   pacing), so the bar is "no audible difference" measured the way the parity
   harness already measures, not byte equality. This gate is also what catches
   volume-model differences (the OPL path today does not go through the
   per-chip balance the VGM path applies — verify, and calibrate if needed).

### The workstreams, once gated

| Step | What | Size |
|---|---|---|
| **k-1** | Flip playback routing: `audio_source()` / the worklet's `source()` return `Vgm` for an OPL VGM. The worklet's own projection call (`player.rs:119-126`) is deleted. Gate: the A/B render test above. | contained |
| **k-2** | **RetroWave**: the hardware player drives its serial chip from `PlayerEngine` over a `Song`. Either (a) the RetroWave service builds its own `Song` from the `VgmFile` at load — the projection becomes a private detail of one service, deleted from the editor; or (b) `VgmEngine` learns to host an external OPL chip. **(a) first** — it is ~10 lines, unblocks everything else, and (b) can follow whenever the render-split work touches that seam anyway. | contained |
| **k-3** | **Analysis**: re-host `RegisterAnalyzer` / `RegisterUsage` / `initial_channel_pans` on `VgmStream`'s OPL writes instead of `&Song`. This is the largest genuinely new code in K — the OPL row rendering keeps its regdata-grade detail, so the analyser walks `VgmCommand::Write`s rather than `Instruction`s. The projection-vs-analysis behaviour must not change: pin current row text on a fixture before porting. | needs-design |
| **k-4** | Editor: delete the `projection` field, `refresh_projection`, and the OPL branch of `snapshot()`; `song()` becomes DRO-only. Every `(snapshot, vgm)` consumer was already collapsed by mg-5. | contained |
| **k-5** | Delete `SongData::Vgm` and the `Instruction` variants only VGM produced (`DelaySamples`, the VGM-only bank forms), narrowing `Song` to the DRO document. `OplProjection`/`to_song` go, or shrink to k-2(a)'s private helper. | contained, wide |
| **k-6** | *(optional end state)* DRO playback through `VgmEngine` via `dro_to_vgm` at load — `PlayerEngine` deleted entirely, one engine in the codebase. Editing stays on `Song`; only the audio path converts. `dro_to_vgm` is already validated byte-for-byte against `dro2vgm`. Decide when k-1..k-5 are green; it is separable and the RetroWave question (k-2) resurfaces here. | needs-design |

### What Stages A–J should do differently knowing K is coming

- **mg-5** (doc_source): design the shared enum so the `Opl` arm's meaning can
  narrow to "DRO" without another rename — name it by document kind, not chip.
- **k-3 is why the analysis findings in REVIEW.md's appendix were left
  unstaged** — do not "clean up" `analysis.rs` in Stage F/G beyond comments;
  it is about to be re-hosted.
- **dd-6/dd-8**: when fixing doc comments in `engine.rs`/`split.rs`, do not
  entrench "the OPL engine" as permanent — phrase as "the DRO engine's" where
  the comment survives K.

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
| `vgms-ui/src/dialogs/mod.rs` | J | st-2 → st-3. (st-1 dropped, so sw-5 no longer gates anything here.) |
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

- **Automated releases.** Wanted, but deliberately deferred ((c)). `build.yaml`
  goes in Stage B; `DEVELOPMENT.md` records that the gap is intentional so it
  does not read as rot. A tag-triggered `release.yaml` (`cargo build --release`
  on windows-latest plus `tools/build-web.ps1`, uploading both) is roughly half
  a day whenever it is wanted.
- The `render-split-2026-08` ChannelGate programme is independent, but **it must
  rebase onto mg-5**, not the reverse, and it names `RecordingChip` as its test
  vehicle — so `RecordingChip` is not deletable (only its three false doc
  comments get fixed, in dd-8's neighbourhood). It is also now **a gate for
  Stage K** (§12b): OPL VGMs cannot leave `PlayerEngine` until ChannelGate
  replaces `Muting::gate` on the `VgmEngine` path.
- `tests/make_screenshot.py` stays. It is unreferenced but it *generates*
  `tests/screenshot.png`, which five Rust sites embed; the fix is a provenance
  pointer in the consumers, not deletion.
- Performance findings below the "silent wrongness" bar (the `0x93` double bank
  copy, `readiness_items()` running twice per frame, `encode_wait` rebuilding its
  tables per call) are left in REVIEW.md as a backlog, not staged here.
