# Web target — session handover (2026-07-31)

Continues the WASM web target for VGM Studio (Step 8 of the Rust rewrite). This
is a working-state handover for a fresh session. The durable one-paragraph
summary is in memory `web-target-progress.md` (auto-loaded); this is the detail.

## Where things are

- **Branch `web-target`** (off `rename-vgm-studio` at `b2b0f50`). Working tree
  clean. **Nothing pushed or merged** — commit only, per repo convention.
- **The web app builds and boots and plays** in a browser. Verified end-to-end.
- Plan: `docs/web-target-2026-07/PLAN.md` (steps wt-1..wt-9). wt-1..wt-5 DONE.

### Commits (newest first, since `b2b0f50`)

```
9d9b680 Reapply CJK font fetch (owner confirmed after understanding it isn't embedded)
fbb936d Revert CJK font fetch (superseded by 9d9b680)
da9073d fix(web): fetch a CJK fallback font so Japanese GD3 tags render
76343b6 fix(ui): disable channel-mute toggles for cores that can't mute
e5e9fd3 fix(ui): dialog sizing, help layout, position readout, status overflow, web maximize
0a1991d fix(web): non-OPL VGMs now play/render, and playback actually sounds
7aeff66 docs: record wt-1..wt-5 done in the web-target plan
c33e63a wt-5: extend CI to build and prove the web target
f4e7360 wt-4: the web runner, the page, and the build script
cc84c10 wt-3: web audio playback through an AudioWorklet
2cc928c wt-2b: the web platform services (wasm) for vgms-web
01525dc wt-2a: the Worker-boundary codec for vgms-web
d51e3fe wt-1: implement vgms-synth-worklet, the bindgen-free AudioWorklet module
e802885 style: rustfmt the import reorder left by the brand rename
7e89531 docs: revise web-target plan (Chromium packs, zip packs, e2e, Web Serial)
dcba726 docs: plan the web target (Step 8) as wt-1..wt-6
```

## Environment (CRITICAL — the agent process has a stale env)

Prepend this to EVERY PowerShell cargo/clang call (Scoop set the vars at User
scope; a long-running agent doesn't inherit them):

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

`wasm-bindgen` CLI 0.2.126 is installed and matches the `vgms-web` pin.
`wasm32-unknown-unknown`, clang, node, python all present.

## The gate (green before every commit)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# wasm-clean + web modules:
cargo check --target wasm32-unknown-unknown -p vgms-core -p vgms-synth -p vgms-ui -p vgms-web -p vgms-synth-worklet
# wasm-only code isn't seen by native clippy — lint it on the wasm target too:
cargo clippy -p vgms-web --target wasm32-unknown-unknown --all-targets -- -D warnings
```

## Architecture (what was built, wt-1..wt-5)

Two wasm modules. `vgms-ui` is the whole app, wasm-clean, behind service traits.

- **`crates/vgms-synth-worklet`** (cdylib+rlib) — the AudioWorklet module. A
  bindgen-free `extern "C"` ABI (`abi.rs`, prefix `vgmsw_`) over the safe player
  in `player.rs` (`WebPlayer` = the native cpal callback body minus the device).
  `install_web_cores()` = the app's `install_cores` minus RetroWave. Import-free,
  proven by `tools/web/worklet_smoke.mjs` (node). **rlib is shared by vgms-web.**
- **`crates/vgms-web`** (cdylib+rlib) —
  - `codec.rs` (portable, native-tested): `TaskRequest`/`TaskResult` <-> bytes.
    Documents ride their own file writers; scalars length-prefixed.
  - `services/{config,file,task,pack,audio}.rs` (wasm-only): the four platform
    traits + audio. localStorage config; file input + Blob downloads; Web Workers
    for tasks (`worker.rs` + `web/task_worker.js`); AudioWorklet playback
    (`web/worklet-processor.js`); pack = stub until wt-7/wt-8.
  - `runner.rs`: `#[wasm_bindgen] start(canvas)` -> eframe::WebRunner. Installs
    the core registry, the console logger, panic hook; fetches the CJK font.
- **`web/`**: `index.html`, `task_worker.js`, `worklet-processor.js`.
- **`tools/build-web.ps1`**: builds both modules, runs wasm-bindgen, assembles
  `target/web-dist` (app module ~12.5 MB, worklet ~964 KB), downloads the CJK
  font. **CI** (`.github/workflows/rust.yaml`) builds both + runs the node smoke.

## How to build, serve, and verify (no dev server needed)

```bash
pwsh tools/build-web.ps1
cd target/web-dist && python -m http.server 8199 --bind 127.0.0.1 &
```
Then navigate the in-app Browser pane to `http://127.0.0.1:8199/index.html`.

**The egui canvas cannot be screenshotted in the Browser pane** (it renders 0x0
when the pane isn't composited -> an egui-wgpu "texture not allocated" panic that
is an ARTIFACT, not a bug). Verify web behaviour WITHOUT the canvas:
- **Audio**: drive `worklet-processor.js` + the worklet wasm in an
  `OfflineAudioContext` with `processorOptions.autoplay: true` (offline render
  does not deliver port messages mid-render), measure the output buffer.
- **Fonts**: load the served font via the `FontFace` API; check it applies.
- **App boot**: read console — eframe logs "event handlers installed", no errors.

## Traps (each cost real time this session)

- **`TextEncoder`/`TextDecoder` do NOT exist in `AudioWorkletGlobalScope`** —
  `worklet-processor.js` hand-rolls UTF-8. Never reintroduce them there.
- **Three separate wasm instances each install their own core registry**: the
  app module (`runner.rs`), the Worker (`worker.rs`), the AudioWorklet
  (`vgmsw_init`). Miss one and capability gating / playback silently breaks.
- **AudioContext must be created synchronously in `load()`** so `play()`'s
  `resume()` rides the user gesture (else silent).
- **PowerShell `2>&1` on native exes** wraps stderr as errors and sets a false
  non-zero exit — do NOT pipe cargo through `2>&1`; the tool captures stderr.
- **UI changes regrow egui_kittest snapshots**: `UPDATE_SNAPSHOTS=1 cargo test
  -p vgms-ui`, then `git status` should show only the intended PNGs. Read a PNG
  with the Read tool to eyeball it.
- **Flaky pre-existing `vgms-core` proptest** `any_opl_file_projects_identically_to_the_opl_reader`
  fails ~occasionally on synthetic OPL2+OPL3-write files. UNRELATED to this work
  (flagged as its own task). If `cargo test --workspace` reddens on it, re-run.

## What remains

- **wt-6** Playwright e2e harness + core specs. Needs: a debug/e2e-only
  `window.__vgms_e2e` action/state hook in `runner.rs` behind the `e2e` cargo
  feature (already declared in vgms-web's Cargo.toml), OPFS-backed picker shims,
  a static server, chromium+firefox projects, a `web-e2e` CI job. `VgmStudioApp`
  must expose action-dispatch/state-read for the hook. Design in PLAN.md wt-6.
- **wt-7** Pack mode on Chromium (File System Access API). Paths stay `PathBuf`
  as virtual tokens `/<name>` resolved against held `FileSystemDirectoryHandle`s
  inside a pluggable `WebFileService` backend. Design in PLAN.md wt-7. (Two
  earlier Explore reports mapped the pack dataflow — the app treats pack paths
  as opaque tokens; virtual paths need ~zero vgms-ui change.)
- **wt-8** Zip-backed packs everywhere (native + web) — new leaf crate
  `vgms-pack-archive`, memory-backed with an explicit Save Pack = the release
  export (VGMs individually optimized + gzipped to .vgz). Design in PLAN.md wt-8.
- **wt-9** RetroWave over Web Serial — low-priority investigation (crate
  hygiene so `vgms-retrowave` wasm-checks; a no-Rust hardware spike; a timing
  rehearsal; a written go/no-go). Design in PLAN.md wt-9.

## Deferred: real channel muting for the Nuked cores

`76343b6` only GATES the UI (disables mute toggles) for chips whose core can't
mute. The Nuked YM2612/YM2151/YM2413 cores don't implement
`ChipCore::set_channel_mutes` (they inherit the no-op). Capability lives in
`CoreInfo.channel_mute` + `CoreRegistry::mute_capable(chip)` (mirrors
`channel_pan`/`pan_capable`). To make muting actually work, the owner's options
(their call — accuracy-critical, `vendor/upstream/` is off-limits to edit):
implement `set_channel_mutes` in the Nuked wrapper via a shim over the C's
per-channel `ch_out[6]` + the DAC cycle timing (`chip->channel = cycles % 6`),
OR default those chips to the mute-capable libvgm core. Also flagged as a task.

## Watchdog

A usage-limit watchdog is armed for the PREVIOUS session
(`a62f89b7-...`); it belongs to that session and will resume it, not a new one.
A fresh session should arm its own if doing a long autonomous run (skill
`watchdog`). Resume note at that session's scratchpad `watchdog-resume.md`.
