# PLAN — One executable: fold the CLI tools into `drotrim`, and surface them in the GUI

**Repo:** `I:\Code\Python\dro-trimmer` · **branch `rust`** (master is the Python
original — parity oracle only, never modify `src/`).
**Written:** 2026-07-20. **Status:** planned, implementation starting.

---

## 0 · Progress

| Step | What | State | Commit |
|------|------|-------|--------|
| 0 | This plan | — | — |
| 1 | Fold the three bins into `drotrim` subcommands | pending | |
| 2 | VGM capture (`filter_vgm` + `capture` dispatch) | pending | |
| 3 | Move `split` into `dro-synth`, `SplitFormat::Song` | pending | |
| 4 | Combined render variant + WAV regression fixtures | pending | |
| 5 | Per-kind `ThreadTaskService` + `is_busy_kind` | pending | |
| 6 | Render to WAV (GUI) | pending | |
| 7 | Convert to DRO v1 (GUI) | pending | |
| 8 | Split Channels (GUI) | pending | |
| 9 | Cancellable exports | pending | |

---

## 1 · Why

Two problems, one plan.

**One executable.** Step 5 of the rewrite produced three standalone console
binaries next to the GUI: `dro2to1`, `dro_split` and `dro_player` (whose
`--render` flag is the render-to-WAV path). Four executables to ship, four
`--help`s to discover, and no single entry point that tells you what the tool
can do. `drotrim.exe` should *be* the tool: `drotrim help` lists what it can do,
`drotrim <subcommand>` does it, and `drotrim` with no arguments (or with just a
file) opens the GUI exactly as today. The three old bins are deleted, not
aliased.

**The GUI can't do what the CLI can.** Playback and channel muting/soloing
already exist in the GUI, but rendering to WAV, splitting a song into one file
per channel, and converting DRO v2 → v1 are reachable only from a terminal —
even though the logic behind them is shared library code. This plan surfaces all
three as menu actions, so the desktop app can do everything the command line can.

`Convert to VGM` (Edit menu → `Action::ConvertToVgm` → `Editor::convert_to_vgm`)
is the established shape for a format action, and rip mode's zip export
(background job → result → `FileService::save`) is the established shape for a
long-running export. Both are mirrored rather than reinvented.

---

## 2 · Decisions (settled — do not re-litigate)

1. **`drotrim` gains four subcommands**: `play`, `render`, `split`, `convert`.
   `render` and `play` are `dro_player`'s two halves, split apart — the mode is
   now the subcommand, not a `--render` flag. clap supplies `help` for free.
   Old bin names get no aliases; `-d/--dro` survives as a hidden alias of
   `split`'s new `-s/--song`.

2. **Render to WAV (GUI) has three independent, opt-in options** — apply the
   channel toggles (muting), apply the channel panning, apply a boost — plus an
   **"All of the above"** quick-select. **All three default to off**, which is
   exact `drotrim render` parity (full mix, centered, boost 1.0). Any
   combination is valid. The boost value prefills from the current playback
   boost, so "All of the above" ≈ "render what I'm hearing".

3. **Convert to DRO v1 (GUI) renames the song** `song.dro` → `song_1.dro`,
   matching the CLI's default output name, so a subsequent Save As cannot
   silently overwrite the v2 source.

4. **Split (GUI) overwrites** existing files in the chosen output folder, as
   `fs::write` does on the CLI. Output names are derived from the song, so a
   collision is almost always the previous split of the same song.

5. **Split supports VGM input in both formats.** WAV split already worked for
   VGMs; the song-format split gains a VGM capture rather than keeping the
   "cannot capture a VGM" error. `SplitFormat::Dro` becomes `SplitFormat::Song`
   — "the same format as the input" — emitting `.out.dro` or `.out.vgm`.

6. **`anyhow` stays in `dro-trimmer`.** The split logic moving into `dro-synth`
   switches to `dro_core::Result`. Not a technical constraint (anyhow is
   wasm-clean) but library idiom: `dro-synth` has no anyhow dependency, its
   sibling API `capture` already returns `dro_core::Result`, and a library
   returning `anyhow::Error` forces the dependency on every consumer while
   erasing matchable error types. The conversion is free — split's only anyhow
   use is `use anyhow::Result`, with no `.context()` calls.

7. **Renders are pinned by byte-exact WAV fixtures** under `tests/render/`,
   blessed with `UPDATE_RENDER_FIXTURES=1` (mirroring the kittest snapshot
   workflow). The engine is deterministic — `nuked-opl3` is bit-identical to the
   C reference and the boost limiter is plain IEEE f32 — so a byte comparison is
   a legitimate regression test, and it is what guards the Step 9 refactor.

---

## 3 · Constraints

- `dro-core`, `dro-synth` and `dro-ui` stay **wasm-clean**. Everything native —
  clap, `env_logger`, console attaching, `rfd`, `windows-sys` — lives in
  `dro-trimmer`.
- **`dro-ui` cannot depend on `dro-trimmer`** (`dro-trimmer` is the binary crate
  that depends on `dro-ui`). This is why `split` must move down into `dro-synth`
  before the GUI can call it.
- **One commit per step**, with the workspace gate green each time:

  ```
  cargo test --workspace
  cargo clippy --workspace --all-targets      # zero warnings
  cargo fmt --all --check
  cargo check --target wasm32-unknown-unknown -p dro-core -p dro-synth
  ```

---

## 4 · Facts established while planning

Verified against the tree at `cc83093`; re-check before relying on a line number.

- **`capture()`** (`crates/dro-synth/src/capture.rs:179`) walks the unified
  `DroInstruction` stream, gates writes through `Muting::gate` and re-encodes
  delays. Its *only* VGM blocker is the `DelaySamples` arm. Muted key-on writes
  are dropped (and `0xBD` masked), so a filtered stream is shorter than its
  source.
- **`dro_to_vgm()`** (`crates/dro-core/src/convert.rs:48`) already holds the
  exact VGM emission machinery a VGM capture needs: `write_command(opl_type,
  bank)` per register write, `command::WAIT` + `u16` chunking per delay,
  finished through `Song::vgm(...)`.
- **`VgmMeta::loop_point` / `loop_end` are instruction indices**
  (`crates/dro-core/src/vgm/data.rs:323`), resolved from byte offsets at read
  and recomputed at write (`vgm/io.rs:138`). `slide_index_past_deletion`
  (`song.rs:629`) is the precedent for remapping an index across dropped
  instructions.
- **`ThreadTaskService`** (`crates/dro-trimmer/src/services/task.rs`) is
  single-slot with a *global* generation — its own comment anticipates the
  per-kind registry this plan builds. Note `cancel()` does **not** bump the
  generation today, so a result already queued survives a cancel; latent now
  (nothing calls `cancel`), load-bearing once exports can be cancelled.
- **`dro-web` is an empty placeholder crate** — no web service impls to keep
  compiling when `FileService` grows methods.
- **`NativeFileService::save_filters`** has no `"wav"` arm, so a WAV save dialog
  would currently offer DRO/VGM filters.
- **clap derive names the command after the *package*** (`dro-trimmer`), so
  `name = "drotrim"` must be set explicitly — today's `drotrim --help` header is
  subtly wrong. `Option<Subcommand>` and a positional `Option<PathBuf>` coexist;
  add `args_conflicts_with_subcommands = true`.
- **Windows stdio re-queries `GetStdHandle` per write**, so `AttachConsole`
  before the first print is sufficient — there is no CRT `FILE*` to rebind.
- **Workspace lints set `unsafe_code = "deny"`, not `forbid`**, so the console
  helper can carry a local `#[allow(unsafe_code)]`. (`dro-ui`'s `#![forbid]` is
  untouched — the helper lives in `dro-trimmer`.)
- **`tests/lsl3_score_up_dro2.dro` is ~99 seconds** — fine for a `convert` smoke
  test, far too slow to render in one. Render/split tests build a small song in
  memory instead; `split.rs`'s existing `small_song()` (channels 0 and 1 plus
  percussion) is the template, and `dro_to_vgm(small_song())` manufactures its
  VGM twin.
- **`Editor::convert_to_vgm`** (`crates/dro-ui/src/editor.rs:257`) is the mirror
  template for `convert_to_dro1`. DRO v2 detection is `file_type ==
  SongFileType::Dro && file_version == DRO_FILE_V2`.
- **Menu contents are pinned by tests** — `edit_menu_items()` in
  `crates/dro-ui/src/app_gui_tests.rs:2230` probes a fixed list. Existing
  snapshot PNGs draw with menus closed, so only *new* dialog snapshots are added.

---

## 5 · Steps

### Step 1 — Fold the three bins into `drotrim` subcommands

New `crates/dro-trimmer/src/cli/` (a **library** module — a file under
`src/bin/` would be auto-discovered as a fourth binary):

- `mod.rs` — `Cli { command: Option<Command>, file: Option<PathBuf> }` with
  `#[command(name = "drotrim", version, about, after_help = "Run with no
  arguments (or with just a file) to open the GUI.",
  args_conflicts_with_subcommands = true)]`; `enum Command { Play, Render,
  Split, Convert }`; `pub fn run(Command) -> anyhow::Result<()>`.
- `play.rs` / `render.rs` — `dro_player`'s two halves, verbatim. Play drives
  `NativeAudio` on the main thread with a 50 ms position line; render defaults
  boost to 1.0 (ignoring `drotrim.ini` — a render is faithful to the source
  unless `--boost` says otherwise) and **appends** `.wav`, so `song.dro` becomes
  `song.dro.wav`.
- `split.rs` — `dro_split` moved; the format flag becomes `-s/--song` ("split to
  song files — DRO or VGM, matching the input — instead of WAV") with `-d` and
  `--dro` kept as hidden aliases. The in-place `RenderProgress` printer moves
  verbatim.
- `convert.rs` — `dro2to1` moved verbatim (`<stem>_1.<ext>` default, bails if
  the output exists).
- `console.rs` — `#[cfg(windows)]`, `#[allow(unsafe_code)]`:
  `attach_parent_console()` saves the three `GetStdHandle` values, calls
  `AttachConsole(ATTACH_PARENT_PROCESS)`, then `SetStdHandle`s back any saved
  handle that was neither null nor invalid, so `> out.txt` / `2> err.txt`
  redirections survive the attach.

`bin/drotrim.rs` keeps its `windows_subsystem` attribute, returns `ExitCode`,
and attaches the console (release Windows builds only, when there is more than
one argument) **before** `env_logger::init()` and `Cli::parse()` so clap's own
errors and `help` are visible. A subcommand runs and returns; no subcommand
falls through to today's GUI body using `cli.file`.

Delete the three bin files. Add `windows-sys` (0.61, `Win32_Foundation` +
`Win32_System_Console`) as a `cfg(windows)` dependency. Update the crate
description and the `lib.rs` docs that name the old bins.

**Tests** — parser unit tests (`Cli::command().debug_assert()`; bare invocation;
file positional; each subcommand; the `-d` alias; `drotrim a.dro convert`
rejected) plus `crates/dro-trimmer/tests/cli_smoke.rs` driving the real
executable through `env!("CARGO_BIN_EXE_drotrim")`: `help` exits 0 and lists the
four subcommands; `convert` on a temp copy of the fixture writes `…_1.dro` and a
second run bails; `render` and `split --song` on a small song written to a temp
dir produce a `RIFF` WAV and `.out.dro` files.

**Docs** — `DEVELOPMENT.md` gains a CLI section covering the subcommands and the
console wart below.

### Step 2 — VGM capture

In `dro-core`: factor the VGM command emission out of `dro_to_vgm` into a small
shared stream builder, and add a gate-driven filter:

```rust
pub fn filter_vgm(
    song: &Song,
    gate: impl FnMut(Bank, u8, u8) -> Option<u8>,
    name: String,
) -> Result<Song>
```

It walks the source VGM's instructions; register writes pass through `gate`
(dropped on `None`, value-rewritten on `Some`); `DelaySamples` is re-emitted as
`0x61` chunks. The result clones the source's `VgmMeta`, so the header, GD3 tag
and version survive, with `loop_point`/`loop_end` **remapped** by counting
surviving output instructions as the walk passes each index (dropped writes are
zero-duration, so landing on the next survivor preserves musical time; check the
boundary *before* emitting so a multi-chunk delay can't overshoot). The gate is
a closure specifically so `Muting` — a `dro-synth` type — stays out of
`dro-core`.

In `dro-synth`: `capture()` dispatches on song type — the DRO path is unchanged,
the VGM path calls `filter_vgm` with `muting.gate`. The "cannot capture a VGM's
sample delays" error disappears.

**Tests** (all in memory): a pass-all gate round-trips through
`vgm::io::write`/`read` preserving `total_delay_samples`, GD3 and loop indices; a
dropping gate shrinks `len()` and slides `loop_point` correctly; multi-chunk long
waits keep their boundary; and in `dro-synth`, capturing `dro_to_vgm(small_song)`
with one channel isolated drops the other channel's key-ons and preserves timing
— mirroring the existing DRO capture tests.

### Step 3 — Move `split` into `dro-synth`

Move `crates/dro-trimmer/src/split.rs` to `crates/dro-synth/src/split.rs` with
its tests. Swap `anyhow::Result` for `dro_core::Result`, mapping the single
hound-error site through `Error::file(...)` (orphan rules forbid a `From` impl).
Rename `SplitFormat::Dro` → `Song` and `SplitData::Dro` → `Song`; `render_one`
picks `.out.dro` or `.out.vgm` from the input type. Update `cli/split.rs`'s
imports and the `[crate::split]` doc link in `rip_zip.rs`. The wasm gate now
covers split for free.

**Register-level integration tests** — round-trip each output through
`write_song` → `read_song` (bytes in memory, no temp files) and assert the shape:
the `.0.01.` output contains channel 0's key-on write (`0xB0 = 0x31`) and *no*
`0xB1` key-on, the `.0.02.` output the reverse, both keep a masked `0xBD`, and
every output preserves `total_delay_ms`. Then the same for the VGM twin —
outputs named `.out.vgm`, bytes starting `Vgm `, `total_delay_samples`
preserved, and a source `loop_point` still landing on the same musical spot.
`cli_smoke.rs` gains a `split --song` run over a small VGM.

### Step 4 — Combined render variant + regression fixtures

`dro-synth/wav.rs` gains a public render variant taking **muting + panning +
boost** (plus rate and depth) over the existing internal impl. The engine
already supports all three for playback; the current public functions cover
muting *or* boost, and never panning. A unit test pins that the defaults equal
`render_wav`.

`crates/dro-synth/tests/render_regression.rs` renders a purpose-built ~100 ms
song (distinct frequencies per channel, so scenarios differ audibly and
bytewise) through every scenario and byte-compares each against a fixture in
`tests/render/`: `full.wav`, `muted.wav` (channel 1 muted), `boosted.wav` (boost
2.0), `panned.wav` (channel 0 hard left), `combined.wav` (all three), plus the
WAV split's `split.0.01.wav`, `split.0.02.wav` and `split.0.14.wav`. With
`UPDATE_RENDER_FIXTURES=1` the test writes the fixtures instead of asserting; a
missing fixture fails with that hint.

### Step 5 — Per-kind `ThreadTaskService`

`TaskService` gains a **required** `is_busy_kind(TaskKind) -> bool` (three
in-repo impls, all updated here — a defaulted method would let a future impl lie
silently). `ThreadTaskService` becomes `HashMap<TaskKind, Slot>` where `Slot`
carries its own pending, cancel flag, live count and generation; the channel
carries the kind. Submitting supersedes only its own kind. `cancel(kind)` now
**bumps that slot's generation** (closing the queued-result-after-cancel race)
and **swaps in a fresh live counter**, so an orphaned uncancellable render stops
counting toward `is_busy` — otherwise the status bar would claim "Rendering
WAV..." after the user cancelled by loading a new song.

### Step 6 — Render to WAV (GUI)

`TaskKind::RenderWav`; `TaskRequest::RenderWav { song, muting, panning, boost,
sample_rate, bit_depth }`; `TaskResult::Wav(Result<(String, Vec<u8>), String>)`
— the output name is computed **inside the task** from the song snapshot, so an
edit mid-render can't mislabel the save dialog.

`dialogs/render_wav.rs` — three checkboxes (apply channel toggles / apply
channel panning / apply boost), the boost stepper enabled only while its
checkbox is ticked and prefilled from the live playback boost, an "All of the
above" quick-select, and a caption noting that frequency and bit depth come from
Settings. `save()` emits `Action::RenderWavSubmitted { use_toggles, use_panning,
boost }` with boost resolved to 1.0 when its checkbox is off. All three default
off (§2.2).

The handler refuses a second concurrent render (`is_busy_kind`), submits with
the resolved muting/panning/boost, and the result routes through the rip-zip
flow: `pending_saves.push_back(SavePurpose::WavExport)` then
`files.save(SaveRequest::Dialog { .. })`. Loading a song cancels the render.
`save_filters` gains its `"wav"` arm. The status bar labels each kind
separately.

### Step 7 — Convert to DRO v1 (GUI)

A `dro1_default_name(&str) -> String` helper in `dro-core` (shared with the
CLI's path-based `default_output`), `Editor::convert_to_dro1()` mirroring
`convert_to_vgm`'s reset block plus the `_1` rename, `MenuState::is_dro_v2`
gating a new Edit-menu item under "Convert to VGM", and a handler mirroring
`ConvertToVgm`'s status/`close_song_dialogs`/`after_edit` sequence.

### Step 8 — Split Channels (GUI)

`FileService` gains `pick_output_folder()` / `poll_output_folder() ->
Option<Option<PathBuf>>` (`Some(None)` = cancelled), with default no-op bodies
for the future web shell; the native impl is the established blocking-`rfd`-then-
stash pattern. Rip mode's `pick_folder` is deliberately *not* reused — it scans
and reads a folder's contents as rip input.

`TaskKind::Split` serializes song-format outputs inside the task, so results are
ready-to-write named bytes. `dialogs/split.rs` offers WAV / song-data radios (no
per-format disabling, now that VGM capture works) and an isolate-percussion
checkbox, with a caption warning that existing files are overwritten.

The app holds one `split_flow: Option<SplitFlow>` (`AwaitingFolder → Rendering →
Writing`) that doubles as the in-flight guard and the stale-result gate. Writing
reuses the existing per-file `SaveRequest::InPlace` + `pending_saves` FIFO with a
new `SavePurpose::SplitFile`, tallied to a final "Wrote N file(s) to <dir>."
exactly as rip mode's document batch does — no new batch-save API.

### Step 9 — Cancellable exports

Add a `keep_going` hook to the internal WAV render (mirroring
`render_waveform_cancellable`), thread it through `split()`, and honour
`is_cancelled` in both `run_task` arms, so an orphaned export stops within one
render chunk instead of burning CPU to completion. Step 4's fixtures must still
match byte for byte — they are the guard on this refactor.

---

## 6 · Verification

1. The workspace gate (§3) green on every commit.
2. **Release executable**, from a real PowerShell or cmd window:
   - `drotrim help` lists `play`, `render`, `split`, `convert`; `drotrim help >
     out.txt` and `| more` both capture it.
   - `drotrim convert tests\lsl3_score_up_dro2.dro` writes `…_1.dro`; a second
     run bails. `drotrim render <file>` writes `<file>.wav` with a progress
     line. `drotrim split <file>` writes per-channel WAVs; `drotrim split
     --song <file>.vgm` writes per-channel VGMs. `drotrim play <file>` plays
     with a position readout. A bad flag prints a clap error.
   - `drotrim` and `drotrim <file>` still open the GUI, with no console window
     when launched from Explorer.
3. **GUI**: the Render to WAV dialog (defaulting to none of the three options;
   each individually; "All of the above"; the boost stepper greyed until ticked),
   the Split dialog (both formats over both DRO and VGM songs, isolate
   percussion, folder pick, overwrite), Convert to DRO v1 on a v2 song (renamed
   `_1`, item hidden for v1 and VGM), per-kind status-bar labels, and loading a
   new song mid-render cancelling cleanly.

---

## 7 · Accepted trade-offs

- **Interactive shells do not wait for a GUI-subsystem executable.** After
  `drotrim render x.dro` the prompt returns immediately and output interleaves
  with it. Piping and `cmd`-style redirection capture the output; PowerShell's
  `>` writes an empty file, having moved on before the process prints. Debug
  builds are console-subsystem, so this only affects an interactively-run release
  build. (Measured on the release exe, 2026-07-20.)
- **A file literally named `play`, `render`, `split`, `convert` or `help`** parses
  as a subcommand; open it as `drotrim .\play`.
- **The old bin names are gone.** `dro_split x.dro` is now `drotrim split
  x.dro`; only the `-d/--dro` flag survives, hidden.
- **A VGM split output's waits are re-encoded canonically** as `0x61` runs.
  Timing stays sample-exact, but `0x62`/`0x63`/`0x7n` encodings in the source are
  not byte-preserved.
