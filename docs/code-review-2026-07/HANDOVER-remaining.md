# HANDOVER — remaining review-remediation work (Batch J folds + 2 follow-ups)

**For:** a fresh Claude Code session picking up the *leftover* quality work from the 2026-07 review remediation.
**Repo:** `I:\Code\Python\dro-trimmer` · **branch `rust`** (main/master is the Python original — parity oracle only, never modify `src/`).
**Status:** the review's bugs and the high-value folds/parity/perf are **done and committed** (35 commits since `cf0c2ed`, branch fully green). What is left is **optional, quality-only** work: the low-value Batch J folds, plus two partial-completion follow-ups. None of it fixes a bug or changes user-visible behaviour (except the two follow-ups, which are additive parity features).

> Authority for the fold-level detail is **`remediation-plan.md` §4 (folds) and §5 (parity)** in this folder — read the relevant entry before touching each item. This doc is the orientation layer + the current state + the two follow-ups the plan doesn't fully cover.

---

## 1 · Current state (what's already landed)

All of Batches 0/A/B/C/D/E/F/G/H/I and H2 are implemented, tested, and committed, plus part of Batch J. See `git log --oneline cf0c2ed..HEAD`. Whole workspace is green:

- `cargo test --workspace` — dro-core 211, dro-synth 57, dro-ui 137, dro-trimmer 31, + `golden_opl`, `c_parity` (feature), `panning` integration tests.
- `cargo clippy --workspace --all-targets` — **zero warnings** (workspace lints deny `unsafe_code`, warn `clippy::all`; keep it that way).
- `cargo fmt --all --check` — clean.

**Batch J already done:** dead-API prune (`1caeba3`) and the boost-stepper extract (`16429ae`, uishell-6), then a workspace `cargo fmt` pass (`302063e`). Everything below is what remains.

## 2 · Environment & commands (do this before any cargo call)

Rust/LLVM are Scoop-installed at **User** scope; a long-running agent process does not inherit them. Prepend this to **every** PowerShell tool call that runs cargo:

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

- Build: `cargo build` · Test all: `cargo test` · one crate: `cargo test -p dro-core` · Lint: `cargo clippy --workspace --all-targets` · Format: `cargo fmt --all` (run it before committing — hand-written code drifts from rustfmt).
- **c_parity** (C reference, feature-gated): `cargo test -p dro-synth --features c-parity --test c_parity`.
- **Snapshots** (egui_kittest, `crates/dro-ui/tests/snapshots/*.png`, DX12 WARP, machine-specific): regenerate on THIS machine only if a themed surface changed:
  `$env:UPDATE_SNAPSHOTS='1'; cargo test -p dro-ui; Remove-Item Env:\UPDATE_SNAPSHOTS`
  Then `git status --short crates/dro-ui/tests/snapshots/` to confirm only the intended baselines moved.
- PowerShell quirk: `cargo ... 2>&1 | Select-String ...` flips `$?`/exit code — don't chain `if ($?)` after it; run cargo plainly and filter.

## 3 · Workflow rules (observed convention this branch was built with)

- **Commit after each atomic fix.** No pushing. Branch from `rust` if you must branch.
- **Write a test alongside each change** that would fail without it — even for a fold, the guard is that all existing tests + snapshots stay green (a pure refactor needs no new test, but must not go red).
- Idiomatic, simple Rust — match the surrounding comment density (this maintainer comments deliberately).
- Keep `clippy` at zero warnings and `cargo fmt` clean per commit.

## 4 · GLOBAL CONSTRAINTS every change must preserve

1. **Byte-parity is the safety net for the rip/VGM folds.** The rip `.txt`/`.m3u` byte-match the VGMRips template; the VGM writer preserves the source header; DRO→VGM rounding is fixture-locked (`tests/lsl3_score_up.*`). The `vgmrip-*` folds below are **byte-locked** — the golden/round-trip tests (`dro-core` rip + vgm tests, `dro-trimmer/tests/rip_flow.rs`) MUST stay green.
2. **wasm-clean core:** `dro-core`/`dro-synth` must not gain native-only deps.
3. **Real-time audio:** the cpal callback stays alloc-free/lock-free; the engine renders byte-identically to `golden_opl.rs`/`c_parity.rs`.

---

## 5 · Remaining work

### A · Byte-locked `vgmrip` folds (`crates/dro-core/src/rip.rs`) — plan §4

All three are behaviour-preserving refactors guarded by the rip golden tests. Run the full `dro-core` rip tests + `dro-trimmer` `rip_flow` after each.

- **vgmrip-2 — one rip description field table.** `generate_description` (`rip.rs:~260`) and `parse_description` (`rip.rs:~322`) each hard-code the same ordered `(label, aliases, accessor)` field list. Extract one ordered table that drives both. **M.**
- **vgmrip-3 / vgmrip-4 — unify the word-wrappers + time row.** There are two wrappers (`push_wrapped_block` `rip.rs:~413`, `wrap_value` `rip.rs:~466`) doing the same `(first_width, continuation_width)` job; unify on one, and share a `push_aligned_row` for the label+value rows. **M.**
- **vgmrip-1 — collapse the VGM read double-walk** (`crates/dro-core/src/vgm/io.rs`, `read` `~95` + the offsets pass): one command-stream walker serves both the file read and the loop-offset resolution. Read-path only — no output-byte impact, but still run the vgm round-trip tests. **M.**

### B · Small folds (each ~S, quality-only) — plan §4

Verify each is a pure move/rename with the existing tests green.

- **core-2** — widen `v1_opcode` to `pub(crate)` and use the named opcodes in `convert.rs` instead of magic bytes.
- **native-4** — a shared `read_song_from_path` helper for the 3 CLI bins (`dro_player`, `dro_split`, `dro2to1`) which each duplicate the read+name logic.
- **synth-6** — one `Position::from_frames` for the elapsed-ms formula (currently duplicated).
- **uiwidget-7** — source the Find Register choice-token list from dro-core rather than re-listing it in `dialogs/find_reg.rs`.
- **uiwidget-8** — one `dual_opl2_image()` helper (the fixed hard-L/R panning image is built in >1 place).
- **core-6** — `RegisterUsage::percussion` → `BTreeSet` (dedup + ordered).
- **synth-8** — `dro-synth` `tests/common` reuse the exported `FrameClock` instead of re-deriving it.
- **native-3** — `ThreadTaskService` (`crates/dro-trimmer/src/services/task.rs`) uses per-kind `HashMap`s with a global generation; the review suggests a single-slot design (there is only ever one waveform kind in flight). **S–M.** Keep `cancel_stops_a_pending_task` and the debounce tests green.

### C · Two partial-completion follow-ups (additive parity — these change behaviour, so add tests)

These were split during remediation; the first halves shipped, the second halves are here.

**C1 · parity-3 — embed the icon into the `.exe` resource (Explorer icon).**
The window/taskbar icon is done (`drotrim.rs::load_icon` decodes `src/dt.ico` → `IconData` → `ViewportBuilder::with_icon`, commit `0048e03`). What's left is the Explorer-file icon:
- Add a **build-dependency** — `winresource` (the maintained `winres` fork) — to `crates/dro-trimmer/Cargo.toml` `[build-dependencies]` (prefer a `[workspace.dependencies]` entry for consistency). **Confirm the crate is in the offline cargo cache before starting** — it's a new external dep.
- Add `crates/dro-trimmer/build.rs`: `#[cfg(windows)]` compile a resource with `WindowsResource::new().set_icon("../../src/dt.ico").compile()`. Make it **resilient** — on failure emit a `cargo::warning` and continue, so a toolchain hiccup never blocks the build.
- Can't be unit-tested (it's a link-time resource); verify by building `drotrim.exe` and checking Explorer shows the icon. Note this in the commit.

**C2 · parity-5 — live progress + skip lines for `dro_split`.**
`dro_player --render` already shows live `MM:SS` progress via `dro_synth::render_wav_boosted_with_progress` (commit `1b20537`). `dro_split` doesn't, because `split()` renders every channel internally.
- Add `render_wav_muted_with_progress` in `crates/dro-synth/src/wav.rs` (mirror `render_wav_boosted_with_progress` — thread `on_progress: &mut dyn FnMut(u64)` through the existing private `render_wav_impl`, which already takes it; export it from `dro-synth/src/lib.rs`). Golden-safe: the no-op path is byte-identical.
- Thread a progress callback (and a skip report) through `split()` → `render_one` in `crates/dro-trimmer/src/split.rs`. `split()` is public and has tests (`splits_only_the_used_channels`, etc.) — update those call sites to pass no-op closures.
- In `crates/dro-trimmer/src/bin/dro_split.rs`, print a "Skipping channel X (unused)" line per skipped channel and a live `MM:SS` line per rendered file (reuse `dro_core::util::ms_to_timestr`). Note: the render is faster-than-realtime, so progress is only visibly useful for large inputs.
- Test the new `render_wav_muted_with_progress` like the boosted one (progress reported, monotonic, byte-identical to `render_wav_muted`).

## 6 · Definition of done (per step)

- `cargo build` + `cargo test` (and `-p dro-ui` for UI changes) green; `cargo clippy --workspace --all-targets` **zero warnings**; `cargo fmt --all --check` clean.
- Byte-parity / real-time / wasm constraints (§4) unviolated — for the `vgmrip` folds, the rip + vgm golden/round-trip tests and `rip_flow` are the proof.
- Snapshots regenerated (§2) only if a themed surface changed, and the diff is intentional (none of the remaining folds should touch a themed surface).
- One reviewable commit per atomic change, trailered:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`

## 7 · What is explicitly NOT to be done (judged fine in the review — don't re-raise)

The two sample clocks (byte-locked vs general), the delete-path triple sanitisation, the undo command shapes, enum-over-trait song dispatch, config field symmetry, the wasm placeholder crates, and the deliberate Python divergences recorded in `divergences.md` (DRO Info view-only for VGMs; the 6th column as a hover; Stop leaving the readout at the pause point). Leave them.
