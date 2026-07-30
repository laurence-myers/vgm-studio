# The web target: VGM Studio in the browser

> **Status: IN PROGRESS (2026-07-31).** Step 8 of the original rewrite plan,
> unblocked by the libvgm wasm spike (`LIBVGM-PLAN.md` §6, commit `6d3dbef`):
> the full 38-device core build links import-free and runs on
> `wasm32-unknown-unknown`. What remained was wiring, and this is the wiring.
> Branch: `web-target`, off `rename-vgm-studio`.

## What already exists, and what this plan refuses to re-decide

The codebase was built for this from Step 6 onward, and the seams are already
cut:

- **`vgms-ui` is the whole application and it is already wasm-clean** (CI
  checks it on every push). Every platform difference lives behind four traits
  in `platform.rs`/`tasks.rs` — `FileService`, `AudioService`, `TaskService`,
  `PackService` — plus `ConfigStore`. All of them are *polled, never awaited*,
  from the update loop, which is exactly the shape a browser's callback-driven
  APIs need. Their docs already state the web behaviour ("`None` on the web",
  "every save, on the web", "the web has no pack folder").
- **`AppConfig` already round-trips through INI text** and its own module doc
  names localStorage as the web backing store. A test already pins that a core
  id this build has never heard of loads rather than rejects — written for
  exactly this build.
- **Drag-and-drop already handles the web arm**: `handle_drops` takes
  `DroppedFile::bytes` when there is no path.
- **Every core provider compiles for wasm32**: `vgms-cores-libvgm` proved it
  end-to-end (541 KB, zero imports, node-executed); `vgms-cores-nuked` and
  `vgms-cores-gpl` build their C with `-ffreestanding` on wasm and lean on the
  libvgm crate's `wasm_libc` symbols at link time. `nuked-opl3` is pure Rust.
- **The renderer needs no feature work**: eframe 0.35's default `wgpu` feature
  enables `egui-wgpu/default`, which includes `wgpu/webgl` — WebGPU with a
  WebGL2 fallback.
- The workspace lint comment, the CI TODO ("Step 9"), and the two placeholder
  crates already name the architecture: a wasm-bindgen shell (`vgms-web`) and
  a **bindgen-free** AudioWorklet module (`vgms-synth-worklet`), because
  `AudioWorkletGlobalScope` has no `TextDecoder`/`TextEncoder` for bindgen's
  glue.

Decided here, once:

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

## Out of scope, by name

- **Pack mode on the web.** It reads *and writes back into* a real folder;
  that needs the File System Access API (Chromium-only) and its own design.
  `pick_folder` reports "not available in the browser" through the existing
  error channel.
- **The vgmtools optimisers on the web** — that is `OPTIMIZER-WASM-PLAN.md`
  (ow-1..ow-7), a separate programme. `vgms-ui` already falls back to
  `vgms_core`'s own optimise pass on wasm.
- **RetroWave hardware output** (native-only by nature) and the LLE offline
  tier's realtime use (already gated by `core_for_realtime`).
- Installing the web app as a PWA, service workers, offline caching.

## Steps

**wt-1 — `vgms-synth-worklet`: the audio module.** Crate-type `cdylib` +
`rlib`; deps: vgms-core, vgms-synth, the three core providers, log. Own
`[lints]` block (the workspace one forbids `unsafe_code`, which `no_mangle`
exports need). Surface, all `#[unsafe(no_mangle)] pub extern "C"`, prefix
`vgmsw_`:

- memory: `vgmsw_alloc(len) -> *mut u8` / `vgmsw_free(ptr, len)`;
- setup: `vgmsw_init()` (installs the web core registry),
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
  ints and bools per variant. `PackVolumeScan` is deliberately unsupported (no
  pack on the web).
- `services/task.rs`: `WorkerTaskService` — debounce via `performance.now` +
  `request_repaint_after`; one Worker per running `TaskKind` (kinds run
  concurrently, matching native semantics); `cancel` terminates; results queue
  through an `Rc<RefCell<…>>` the poll drains. The Worker side is an exported
  `#[wasm_bindgen] run_task_bytes(request: &[u8], emit: &js_sys::Function)`
  plus a small `task_worker.js` that inits the module and echoes each emitted
  result buffer back through `postMessage`.
- `services/file.rs`: `WebFileService` — a hidden `<input type=file>` for
  picks; saves become Blob downloads (a download cannot be cancelled or fail,
  so every save reports `Saved { path: None }`); folder/rename/image/output
  channels answer with the honest "not available in the browser" error.
- `services/config.rs`: `LocalStorageStore` — the INI text under one
  localStorage key, `AppConfig::from_ini_sources` in, `to_ini_string` out.
- `services/pack.rs`: the stub `PackService` (never reachable without a
  folder, present because the app takes one).

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
Gate: workspace green + wasm checks; behaviour proof lands with wt-5's browser
run.

**wt-4 — the runner, the page, the build.** `runner.rs`: `#[wasm_bindgen]
start(canvas)` → `eframe::WebRunner` with the same creator the native shell
uses — `theme::install`, `VgmStudioApp::new(web services…, None)` — plus a
panic hook and a console `log` bridge (hand-rolled; no new deps).
`web/index.html`: the canvas, a dark loading state, module-script boot;
favicon from the existing icon art. `tools/build-web.ps1`: release build of
both modules, `wasm-bindgen --target web` for the app, copy worklet wasm +
static files + `licenses/` into `target/web-dist/` (the bundle is
GPL-2.0-or-later, same as the native exe — the workspace manifest already says
so), print sizes. Gate: the script produces a servable directory.

**wt-5 — CI and the local gates.** Extend `.github/workflows/rust.yaml`'s wasm
job: add both new crates to the wasm-clean check; release-build the worklet
and run the node smoke (imports must be empty — retiring the Step 9 TODO
comment); build the app module and run wasm-bindgen over it so glue generation
is proven. Pin `wasm-bindgen = "=0.2.126"` in `vgms-web` so the CLI pin cannot
drift from the lock silently. Local gate for every commit stays: `cargo fmt`,
`clippy --workspace --all-targets -D warnings`, `cargo test --workspace`, the
wasm checks.

**wt-6 — run it, fix what only a browser shows, record.** Serve
`target/web-dist`, drive it in the browser: open the fixture through the
picker *and* by drag payload, watch the waveform arrive progressively (task
Worker), play (context resume), seek, loop, mute a channel, boost until the
limiter caps, save-as-download, reload and find the config persisted. Fix
what that surfaces. Then: mark this plan done, note the web target in the
docs, update memory.

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
