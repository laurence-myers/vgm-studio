# The web target: VGM Studio in the browser

> **Status: IN PROGRESS (2026-07-31).** Step 8 of the original rewrite plan,
> unblocked by the libvgm wasm spike (`LIBVGM-PLAN.md` §6, commit `6d3dbef`):
> the full 38-device core build links import-free and runs on
> `wasm32-unknown-unknown`. What remained was wiring, and this is the wiring.
> Branch: `web-target`, off `rename-vgm-studio`.
>
> **Revised 2026-07-31** to fold in three things the first cut deferred: pack
> mode on Chromium via the File System Access API (wt-7); **zip-backed packs on
> every platform, native included** (wt-8); and a RetroWave-over-Web-Serial
> investigation (wt-9). wt-6 is reworked from a one-off manual pass into a
> permanent Playwright regression suite that pins the web-specific behaviours.

## What already exists, and what this plan refuses to re-decide

The codebase was built for this from Step 6 onward, and the seams are already
cut:

- **`vgms-ui` is the whole application and it is already wasm-clean** (CI
  checks it on every push). Every platform difference lives behind four traits
  in `platform.rs`/`tasks.rs` — `FileService`, `AudioService`, `TaskService`,
  `PackService` — plus `ConfigStore`. All of them are *polled, never awaited*,
  from the update loop, which is exactly the shape a browser's callback-driven
  APIs need. Their docs already state the web behaviour ("`None` on the web",
  "every save, on the web").
- **Pack `PathBuf`s are opaque round-tripped tokens.** The app layer derives
  meaning from a pack path in exactly three pure ways — `folder.join(name)`,
  `path.with_file_name(name)`, `path.parent()` — never parses, string-splits or
  walks components, and its three equality comparisons (same-folder rescan,
  editor-path match, image lookup) are self-consistent whatever the tokens are.
  Every mutation path is `<folder>/<bare-name>`. So a *virtual* path scheme
  resolved inside the file service reaches the whole pack machinery — including
  undo/redo, which only ever replays `rename`/`delete`/`save InPlace` — with no
  change to `vgms-ui`.
- **`AppConfig` already round-trips through INI text** and its own module doc
  names localStorage as the web backing store. A test already pins that a core
  id this build has never heard of loads rather than rejects.
- **Drag-and-drop already handles the web arm**: `handle_drops` takes
  `DroppedFile::bytes` when there is no path.
- **The pack export is already bytes-in/bytes-out** (`pack_zip.rs`:
  `ZipWriter<Cursor<Vec<u8>>>`, gzip via flate2 `rust_backend` — native/wasm
  VGZ byte parity is CI-tested). Only the PNG optimiser (oxipng: C + rayon) and
  the vgmtools VGM optimiser (child processes) are native-only, and `vgms-ui`
  already falls back to `vgms_core`'s built-in optimise pass on wasm.
- **Every core provider compiles for wasm32**: `vgms-cores-libvgm` proved it
  end-to-end (541 KB, zero imports, node-executed); `vgms-cores-nuked` and
  `vgms-cores-gpl` build their C with `-ffreestanding` on wasm over the libvgm
  crate's `wasm_libc` symbols. `nuked-opl3` is pure Rust.
- **The renderer needs no feature work**: eframe 0.35's default `wgpu` feature
  enables `egui-wgpu/default`, which includes `wgpu/webgl` — WebGPU with a
  WebGL2 fallback.
- **RetroWave is already layered for a swap-in transport**: `protocol.rs` /
  `commands.rs` / `chip.rs` are pure (zero `serialport`), and `Device` writes
  through a two-method write-only `SerialIo` trait injected by `with_io`. Only
  `device.rs`'s open/enumerate and `player.rs`'s OS-thread pump are native.
- The workspace lint comment, the CI TODO ("Step 9"), and the two placeholder
  crates already name the architecture: a wasm-bindgen shell (`vgms-web`) and
  a **bindgen-free** AudioWorklet module (`vgms-synth-worklet`), because
  `AudioWorkletGlobalScope` has no `TextDecoder`/`TextEncoder` for bindgen's
  glue.

## Decided here, once

1. **Two wasm modules, not one.** The app module (egui, services, offline
   renders) and the worklet module (the realtime engines + cores) have
   different hosts with different rules. The worklet module is loaded inside
   the audio thread's global scope and must import nothing; the app module is
   wasm-bindgen and DOM-facing. Each links its own copy of the cores — the
   worklet renders live playback, the app module renders waveforms/WAVs in
   Workers.
2. **Background tasks are Web Workers running the app module.** `run_task` was
   written platform-independent ("the web implementation inside a Worker" — its
   own words). A Worker gets the same wasm module, a `TaskRequest` crosses as
   bytes (documents serialise through the crate's own writers, which round-trip
   by construction; parameters through a small hand codec), and each emitted
   `TaskResult` posts back as bytes. `cancel` = `Worker.terminate()`, the
   browser's honest analogue of killing the thread — the same conclusion
   `OPTIMIZER-WASM-PLAN.md` reached for the optimisers.
3. **The worklet owns the whole audio callback body.** Mirror
   `vgms-audio-native`'s proven loop exactly — drain commands, render i16,
   boost + limit, publish peaks/position/finished — swapping the SPSC ring for
   `port.postMessage` in and a state message out. The `Engine` enum (OPL arm /
   VGM arm dispatch) is ~80 lines and gets a sibling in the worklet crate;
   cpal keeps the original.
4. **Same cores, same defaults as native, minus hardware.** The web registry is
   built-ins + libvgm + Nuked + GPL providers with the same three Nuked
   promotions (`install_cores` minus `vgms-retrowave`). One `install_web_cores`
   lives in `vgms-synth-worklet` (an rlib as well as a cdylib) and `vgms-web`
   reuses it, so the two modules cannot drift.
5. **No new build system.** A script drives `cargo` + `wasm-bindgen` (the CLI
   the CI already pins at 0.2.126) for the app module and a plain `cargo build`
   for the worklet. Trunk cannot express the second module; a script says
   exactly what happens.
6. **Pack paths stay `PathBuf`, as virtual tokens over pluggable backends.** The
   web file service holds a token→backend map (`/<name>`, uniquified on
   collision) and resolves any incoming path by `parent()` lookup + `file_name()`.
   Three backends behind one interface: the native fs (unchanged), a Chromium
   `FileSystemDirectoryHandle`, and an in-memory zip pack (all platforms).
   `pick_output_folder` (the split flow) rides the same map. The traits keep
   their shape; `PickedFolder` gains an additive origin marker so the app knows
   a memory-backed pack needs an explicit save.
7. **A zip pack is memory-backed with an explicit save.** Opening a `.zip`
   (picker or drag-drop, any browser, and native) unpacks it through the same
   `vgm/vgz/png/txt` scan filter into the pack model; mutations hit the
   in-service entry map and mark the pack dirty. **Save Pack** runs the existing
   export pipeline with its normal options — VGMs individually optimized and
   gzipped to `.vgz` exactly as a release export does (the web's optimize step
   being the built-in pass, as in wt-7) — delivered `InPlace` to the source zip
   on native and as a download (or `showSaveFilePicker`) on the web. Per-mutation
   write-through is impossible on Firefox/Safari (downloads), so the
   explicit-save model is the one that works everywhere. Re-saving is stable:
   the optimizer passes non-shrinking entries through unchanged and already-`.vgz`
   entries are renamed, not re-gzipped (both exploration-verified in
   `process_entry`).
8. **RetroWave on the web = Web Serial, investigation-only** (not WebUSB — the
   CDC interface is OS-driver-owned and WebUSB cannot claim it), low priority,
   allowed to conclude "defer".
9. **Web behaviours are pinned by a permanent Playwright suite,** not one-off
   verification. The app is one egui canvas with no DOM to select, so e2e builds
   export a debug-only action/state hook (`window.__vgms_e2e`); OS pickers are
   shimmed with OPFS-backed handles; each of wt-6/wt-7/wt-8 lands its acceptance
   checklist as specs run in CI on Chromium **and** Firefox. npm enters the repo
   as a test-only dev dependency; the build pipeline stays npm-free.

## Out of scope, by name

- **Directory-backed pack mode outside Chromium.** No writable-directory API
  exists on Firefox/Safari; `pick_folder` reports the honest "not available in
  this browser" error there. **Zip-backed packs (wt-8) are the everywhere
  answer.**
- **PNG recompression on the web.** oxipng is C + rayon; the web export keeps
  the original bytes with a log line, and `PackService::optimize` answers the
  honest error.
- **The vgmtools optimisers on the web** — that is `OPTIMIZER-WASM-PLAN.md`
  (ow-1..ow-7), a separate programme. The web pack export falls back to
  `vgms_core`'s own optimise pass, as Edit > Optimize already does on wasm.
- **Persisted directory permissions across reloads** (IndexedDB handle storage
  + re-request) — a named follow-up, not the first cut.
- **RetroWave *implementation* on the web** — wt-9 is an investigation with a
  go/no-go, not a build.
- Installing the web app as a PWA, service workers, offline caching.

## Steps

**wt-1 — `vgms-synth-worklet`: the audio module.** Crate-type `cdylib` +
`rlib`; deps: vgms-core, vgms-synth, the three core providers, log. Own
`[lints]` block (the workspace one forbids `unsafe_code`, which `no_mangle`
exports need). Surface, all `#[unsafe(no_mangle)] pub extern "C"`, prefix
`vgmsw_`:

- memory: `vgmsw_alloc(len) -> *mut u8` / `vgmsw_free(ptr, len)`;
- setup: `vgmsw_init()` (installs the web core registry via `install_web_cores`),
  `vgmsw_set_core_choice(slug_ptr, slug_len, id_ptr, id_len)` (the config's
  `audio.core.<slug>` rows, applied before load);
- load: `vgmsw_load(name_ptr, name_len, bytes_ptr, bytes_len, sample_rate,
  resample_mode) -> i32` — parses through `read_song` / `vgm::file::read` by
  file name exactly as every other entry point does, builds the OPL or VGM
  engine with the registry's realtime cores;
- render: `vgmsw_render(left_ptr, right_ptr, frames) -> u32` — planar f32 out
  (what `AudioWorkletProcessor` wants), boost + `BoostLimiter` + peak capture
  inside, exactly the native callback's order;
- control: `vgmsw_seek_ms`, `vgmsw_seek_pos`, `vgmsw_rewind`,
  `vgmsw_set_boost`, `vgmsw_set_loop(start, end, count, start_frames,
  enabled)`, `vgmsw_set_muting(channels, perc0, perc1)`,
  `vgmsw_set_panning(mode, pans_ptr)`, `vgmsw_set_chip_mute(slug…, instance,
  mask)`, `vgmsw_set_chip_pan(slug…, instance, pans_ptr, len)`;
- readback: `vgmsw_position_frames() -> f64`, `vgmsw_position_ms`,
  `vgmsw_position_row`, `vgmsw_loop_iteration`, `vgmsw_is_finished`,
  `vgmsw_take_peak(channel) -> f32`, `vgmsw_take_limited`,
  `vgmsw_min_engaged_boost`.

State is one `Mutex<Option<…>>` (wasm is single-threaded; the lock is for the
borrow checker, not contention). The same functions are plain Rust on native,
so the unit tests drive the full ABI — load the `lsl3_score_up.vgm` fixture and
a synthesised SN76489 VGM (both engine arms), render, assert sound, seek,
finish. A node script (`tools/web/worklet_smoke.mjs`) is the wasm proof: builds
must show **zero imports** (the CI TODO's assertion), then load + render both
fixtures with real sound. Gate: workspace green + node smoke passing.

**wt-2 — `vgms-web`: codec, tasks, files, config.** The crate compiles on
native (empty-ish) so workspace clippy/test cover its portable parts; the
DOM-facing modules sit behind `#[cfg(target_arch = "wasm32")]` and the wasm
deps under a target table. Pieces:

- `codec.rs` (portable, native-tested): `TaskRequest`/`TaskResult` ↔ bytes.
  Documents as their own file bytes (`write_song` / `vgm::file::write`, read
  back by name); `AudioConfig` rides its INI string; everything else is a few
  ints and bools per variant. `PackVolumeScan` is supported (packs exist on the
  web now, via wt-7/wt-8).
- `services/task.rs`: `WorkerTaskService` — debounce via `performance.now` +
  `request_repaint_after`; one Worker per running `TaskKind` (kinds run
  concurrently, matching native semantics); `cancel` terminates; results queue
  through an `Rc<RefCell<…>>` the poll drains. The Worker side is an exported
  `#[wasm_bindgen] run_task_bytes(request: &[u8], emit: &js_sys::Function)`
  plus a small `task_worker.js` that inits the module and echoes each emitted
  result buffer back through `postMessage`.
- `services/file.rs`: `WebFileService` — a hidden `<input type=file>` for
  picks; a `.zip` pick/drop routes to a pack backend (wt-8); folder methods
  error "not available in this browser" **except on Chromium (wt-7) and for zip
  packs (wt-8)**. Song saves become Blob downloads (a download cannot be
  cancelled or fail, so every such save reports `Saved { path: None }`).
- `services/config.rs`: `LocalStorageStore` — the INI text under one
  localStorage key, `AppConfig::from_ini_sources` in, `to_ini_string` out.
- `services/pack.rs`: the web `PackService` — the export job over the Worker
  infra, PNG optimise answering the honest error, `today()` via `js_sys::Date`.

Gate: workspace green (codec tests native), `cargo check` both new crates on
wasm32.

**wt-3 — the audio service and the worklet host.** `worklet-processor.js`
(`registerProcessor('vgms-engine', …)`): receives the worklet wasm bytes +
song + config in its `processorOptions` / port messages, sync-compiles once,
then per 128-frame quantum drains queued command messages into `vgmsw_*`
calls, renders planar f32 straight into the output buffers, and posts
`{frames, ms, row, loopIteration, finished, peakL, peakR, limited,
minEngagedBoost}` every few quanta. `services/audio.rs`: `WebAudioService` —
one `AudioContext` at the config's frequency (created/resumed on Play, the
user gesture), `audioWorklet.addModule` once, one node per load; `load()`
posts the song and succeeds optimistically (a later failure surfaces through
`last_error`, the trait's channel for faults away from a call); commands post
immediately (the processor drains them even while paused — no native-style
deferral needed); position/peaks/finished answer from the last state message.
Gate: workspace green + wasm checks; behaviour proof lands in wt-6's e2e suite.

**wt-4 — the runner, the page, the build.** `runner.rs`: `#[wasm_bindgen]
start(canvas)` → `eframe::WebRunner` with the same creator the native shell
uses — `theme::install`, `VgmStudioApp::new(web services…, None)` — plus a
panic hook and a console `log` bridge (hand-rolled; no new deps). A
debug/e2e-only `window.__vgms_e2e` hook (dispatch an `Action`, read state as
JSON) and the OPFS picker shims are gated behind a `cfg`/feature so release
builds never ship them. `web/index.html`: the canvas, a dark loading state,
module-script boot; favicon from the existing icon art. `tools/build-web.ps1`:
release build of both modules, `wasm-bindgen --target web` for the app, copy
worklet wasm + static files + `licenses/` into `target/web-dist/` (the bundle
is GPL-2.0-or-later, same as the native exe — the workspace manifest already
says so), print sizes. Gate: the script produces a servable directory.

**wt-5 — CI and the local gates.** Extend `.github/workflows/rust.yaml`'s wasm
job: add both new crates to the wasm-clean check; release-build the worklet
and run the node smoke (imports must be empty — retiring the Step 9 TODO
comment); build the app module and run wasm-bindgen over it so glue generation
is proven. Pin `wasm-bindgen = "=0.2.126"` in `vgms-web` so the CLI pin cannot
drift from the lock silently. Local gate for every commit stays: `cargo fmt`,
`clippy --workspace --all-targets -D warnings`, `cargo test --workspace`, the
wasm checks.

**wt-6 — the permanent web e2e harness + core suite.** Stand up Playwright
(pinned, chromium + firefox projects) under `web/e2e/` with a tiny static
server for `target/web-dist`; land the `__vgms_e2e` action/state hook and the
OPFS picker shims in the runner (debug/e2e builds only); write the core
web-behaviour specs (see Verification), and add a `web-e2e` CI job (build dist
→ `npx playwright install --with-deps` → run). Manual residue stays named in
the step: actually *hearing* audio, and real OS permission prompts. npm arrives
here as a **test-only** dev dependency; the build pipeline stays npm-free.

**wt-7 — pack mode on Chromium (File System Access).**
- `pick_folder` = `showDirectoryPicker({mode:'readwrite', id:'vgms-pack'})`;
  scan mirroring native exactly (filters, skip-not-fail, lowercase sort, eager
  bytes); `open_folder_path(token)` = rescan of the held handle (the
  tab-switch/rescan path); picking a new folder mints a fresh token (a re-pick
  of the same folder starts a new project — native's path-equality "keep undo
  history" nicety is noted as not reproducible).
- Mutations: InPlace save = `createWritable→write→close`; delete =
  `removeEntry`; rename mirrors the native decision tree (same-name no-op;
  case-only via temp two-step; **existence check then fail rather than
  overwrite**; `move(newName)` with create+copy+delete fallback). Pack undo/redo
  then replays through the same three calls unchanged.
- `pick_image` needs only name+bytes (exploration-verified) → plain file input.
- Export job runs in the Worker infra from wt-2: gzip + zip are wasm-portable as
  pinned; `optimize_vgms` falls back to `vgms_core`'s built-in pass — the exact
  fallback Edit > Optimize already uses on wasm — with a log line naming the
  difference until OPTIMIZER-WASM lands; oxipng skipped + logged. `today()` via
  `js_sys::Date`.
- Dialog saves upgrade to `showSaveFilePicker` where present (real Cancelled
  outcomes, extension `types` mirroring native `save_filters`, the DRO↔VGM
  format-flip guard copied) — Blob download stays the fallback elsewhere.
- Acceptance lands as e2e specs (Chromium project, OPFS-shimmed pickers): open,
  reorder (temp-name dance), quick-edit rename, delete + undo, screenshot
  replace, optimise skip note, export-zip download reopened and checked,
  save-picker cancel → Cancelled. One manual pass covers the true OS prompt.

**wt-8 — zip-backed packs, every platform (native included).**
- New leaf crate `vgms-pack-archive` (GPL app tier; deps: `zip`, nothing else):
  unzip → entries through the same `vgm/vgz/png/txt` filter and lowercase sort
  the native scan uses, plus the in-memory mutation map with the native decision
  tree copied exactly (same-name no-op, case-only rename,
  fail-rather-than-overwrite, delete, write). Native unit tests pin those
  semantics against `services/file.rs`'s; the wasm proof is the same
  check-build gate as wt-5.
- Both shells mount it as a pack backend: opening a `.zip` — File > Open picker,
  or drag-drop (native path-drop and web bytes-drop both route on the
  extension) — produces a `PickedFolder` whose token backs onto the archive;
  rescans re-list it.
- `vgms-ui` additions (modest, additive): route `.zip` picks/drops into the
  pack-open flow; a `PackOrigin` marker on `PickedFolder`; a dirty flag on
  mutations to a memory-backed pack; a **Save Pack** action = the existing
  export job with its normal options (VGMs individually optimized and gzipped to
  `.vgz`), suggested name = the source zip's, `InPlace` to the source path where
  one exists (native); a discard prompt on close/switch while dirty.
- Web extra: the boot JS registers a `beforeunload` guard while a memory-backed
  pack is dirty (the runner exposes the flag), so a tab close cannot silently
  eat edits.
- Acceptance lands as e2e specs **in the Firefox project** (the non-Chromium
  proof) plus native tests: open a fixture zip via picker and via drop, retag,
  reorder, delete + undo, dirty indicator, dirty-close prompt + `beforeunload`
  dialog, Save Pack download whose songs reopen as optimized `.vgz`, second save
  stable (no re-optimize churn, no double gzip). The native round-trip of a real
  VGMRips zip is a manual pass once, then pinned by `vgms-pack-archive`'s unit
  tests.

**wt-9 — RetroWave over Web Serial (investigation, LOW PRIORITY).** Web Serial,
not WebUSB (the CDC interface is OS-driver-owned; WebUSB cannot claim it). Three
cheap, independent items:
- (a) *crate hygiene* — move `serialport` under a
  `[target.'cfg(not(target_arch = "wasm32"))']` table, gate `device.rs`'s
  open/enumerate + the serialport-typed `Error` variants + the `player.rs` pump
  native-side, and prove `cargo check -p vgms-retrowave --target
  wasm32-unknown-unknown` with the pure `protocol`/`commands`/`chip` modules
  intact.
- (b) *hardware spike, no Rust* — a standalone page that `requestPort`s with the
  known VID/PID filter (04D8:E966), opens the port, and replays a canned wire
  dump captured natively (init + test chord from `commands.rs`) — verified by
  listening, which is how this write-only board is always verified.
- (c) *timing rehearsal* — the native pump's deadline accumulator (64-frame
  ≈1.287 ms quanta, 250 ms `MAX_LAG` backlog tolerance) run in a dedicated
  Worker against a mock writer, measuring whether `setTimeout` jitter (1–4 ms)
  plus WritableStream backpressure stays inside the tolerance the native code
  already grants.

Ends with a written go/no-go in this doc. "Go" spawns a follow-up design doc for
a worker-hosted pump (the browser seam is the JS boundary, not the `Send`-bound
`SerialIo` trait) — not implementation inside this programme.

## Risks, named

- **Worklet compile on the audio thread.** `new WebAssembly.Module(bytes)` in
  the processor constructor is synchronous; the module is ~0.5–1 MB. It
  happens once per load, before playback starts. If it audibly stalls the
  graph, precompile in the main thread and structured-clone the `Module`
  (supported same-agent-cluster in Chromium/Firefox; keep the bytes path as
  the fallback).
- **Realtime budget.** 128 frames at 44.1 kHz is 2.9 ms. Native renders
  8–15× realtime at the worst resample ratio; wasm is typically 2–4× slower —
  still inside the budget, but a heavy multichip rip under Sinc resampling on
  a slow machine may not be. The settings' Linear mode is the escape hatch;
  measure at wt-6.
- **Autoplay policy.** An `AudioContext` starts suspended without a gesture.
  Play is a click, so `resume()` rides it; the risk is only ordering bugs,
  covered by resuming on every play.
- **Module size.** egui + cores in one module, twice. Expect 3–6 MB per
  module before compression. Report sizes in wt-4; brotli/gzip at serve time
  is the real answer and stays out of scope.
- **wasm-bindgen lockstep.** The generated glue and the CLI must match
  exactly; the exact-version pin plus CI actually running the CLI turns a
  drift into a red build instead of a blank page.
- **A second `Muting` constructor.** The worklet ABI needs `Muting`/`Panning`
  rebuilt from raw parts; if the existing setters don't compose them, grow a
  small constructor in `vgms-synth` (permissive tier, so keep it a plain data
  API with a test, no policy).
- **File System Access availability + `move()` version.** Chromium-only, and
  same-dir `move(newName)` is ~111+; the copy+delete fallback is named. One
  permission prompt per session (persistence deferred).
- **A zip save is a release build, not a byte copy.** Songs come back optimized
  `.vgz` and docs regenerate from meta (the same doc regeneration dir packs
  already do); a second save is stable because the optimizer passes unchanged
  entries through and gzipped entries are never re-gzipped. `zip`-on-wasm is
  unproven until wt-5's check-build.
- **Memory-backed edits are volatile.** They live only until the tab/app closes;
  the dirty prompt + the web `beforeunload` guard are the mitigations.
- **Virtual-path display cosmetics.** The only user-visible fabricated paths are
  the editor's "File saved to…" line for pack tracks and the split's output-dir
  status — both read acceptably as `/Pack Name/file.vgm`.
- **e2e stability rests on the debug-only hook and OPFS shims.** Pixel-driving an
  egui canvas is the brittleness the hook exists to avoid; the true OS prompts
  stay a named manual check; headless audio asserts state, not sound; the
  `web-e2e` CI job adds minutes and a test-only npm dependency.
- **Web Serial** (wt-9): timer clamping/background throttling, and no
  listen-back verification in a browser without the user's ears.
