# Web target — completion handover (2026-07-31)

The WASM web target for VGM Studio (Step 8 of the Rust rewrite) is **complete**:
all of **wt-1..wt-9** are implemented, tested, and committed. This is the durable
record. The step-by-step design is in `PLAN.md`; the one-paragraph summary is in
memory `web-target-progress.md` (auto-loaded).

## Status

- **Branch `web-target`** (off `rename-vgm-studio`). Working tree clean.
  **Nothing pushed or merged** — commit only, per repo convention.
- **The web app builds, boots, plays, and edits + exports + zip-packs** in a
  browser. Verified end-to-end on headless Chromium; the e2e suite also targets
  Firefox (run in CI).
- Plan: `PLAN.md` (steps wt-1..wt-9, all DONE). Each step's top-of-file progress
  block records what landed.

### Commits (newest first, since `b2b0f50`; wt-1..wt-5 predate this session)

```
wt-8: wire zip-backed packs into both shells, everywhere
wt-9: RetroWave over Web Serial -- investigation, verdict GO-deferred
wt-8: vgms-pack-archive -- the in-memory zip pack backend
wt-7b: build the pack release zip in a Web Worker
wt-7a: pack mode on Chromium via the File System Access API
wt-6: Playwright e2e harness + the __vgms_e2e action/state hook
... (wt-1..wt-5: the app builds + boots; see git log)
```

## Environment (CRITICAL — the agent process has a stale env)

Prepend this to EVERY PowerShell cargo/clang call (Scoop set the vars at User
scope; a long-running agent doesn't inherit them):

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

`wasm-bindgen` CLI 0.2.126 matches the `vgms-web` pin.
`wasm32-unknown-unknown`, clang, node, npm/npx all present.

## The gate (green before every commit)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
# wasm-clean + web modules:
cargo check --target wasm32-unknown-unknown -p vgms-core -p vgms-synth -p vgms-ui \
  -p vgms-web -p vgms-synth-worklet -p vgms-pack-archive -p vgms-retrowave
# wasm-only code isn't seen by native clippy — lint the e2e hook on wasm too:
cargo clippy -p vgms-web --target wasm32-unknown-unknown --features e2e --all-targets -- -D warnings
```

## What was built

**wt-1..wt-5 (app boots).** `vgms-synth-worklet` (bindgen-free AudioWorklet
module), `vgms-web` (Worker-boundary codec, four wasm platform services,
AudioWorklet playback, `WebRunner` entry, `index.html`, `tools/build-web.ps1`),
CI extended. Detail in `PLAN.md` and the git log.

**wt-6 — Playwright e2e + `__vgms_e2e` hook.** Suite under `web/e2e/` (Chromium
+ Firefox, dependency-free `serve.mjs`). The debug-only `window.__vgms_e2e` hook
(`dispatch` an Action, read a JSON `state()`) lives in `crates/vgms-web/src/runner.rs`
behind the `e2e` feature, over two `VgmStudioApp` methods
(`e2e_enqueue_action`/`e2e_snapshot`, gated `cfg(any(test, feature = "e2e"))`).
`tools/build-web.ps1 -E2e` builds the hooked bundle; CI has a `web-e2e` job. The
**OPFS picker shim** is `globalThis.__vgms_pick_dir`, landed in wt-7 where it is
first exercised.

**wt-7 — Chromium packs (File System Access) + Worker export.**
`crates/vgms-web/pack_fs.js` (a wasm-bindgen snippet module) holds the directory
handles; Rust round-trips an opaque token as a `/<token>` path.
`WebFileService` does pick/rescan/save/delete/rename over it, all saves through
one FIFO queue. The pack export runs in a Worker (`web/pack_worker.js` +
`vgms_web_run_pack_job`) over the wasm-portable builder
`crates/vgms-web/src/pack_zip.rs` (built-in optimise, PNGs kept, gzip via flate2
`rust_backend`); a new `PackJobRequest`/`PackJobOutcome` codec crosses the
boundary.

**wt-8 — zip packs everywhere (native + web).** Leaf crate `vgms-pack-archive`
(unzip → in-memory mutation map, native decision tree). A shared
`platform::ArchiveBackend` (ONE impl) is embedded in every file service — native,
web, and the test fake — and routes any op whose `/vgms-zip-N` token it holds to
the archive; a Directory pack's paths never match, so directory mode is
untouched. `PackState` gains a `PackOrigin` (detected from the token path in
`open_folder`); `.zip` routing from picker + drag-drop; dirty-on-mutation; a
**Save .zip** deck action = the wt-7b export + clear dirty.

**wt-9 — RetroWave over Web Serial (investigation).** Verdict **GO, deferred** —
see the go/no-go in `PLAN.md` (wt-9 section) and `web-serial-spike/README.md`.

## How to build, serve, and test

```bash
pwsh tools/build-web.ps1            # release bundle into target/web-dist
pwsh tools/build-web.ps1 -E2e       # + the window.__vgms_e2e hook, for the e2e suite
cd target/web-dist && python -m http.server 8199 --bind 127.0.0.1 &
```

The Playwright suite (prepend the env prelude, build the `-E2e` bundle first):

```bash
cd web/e2e
npm ci
npx playwright install chromium firefox   # + --with-deps on Linux/CI
npx playwright test                        # both projects
npm run test:chromium                      # Chromium only
```

**The egui canvas cannot be screenshotted in the in-app Browser pane** (it
renders 0x0 → an egui-wgpu "texture not allocated" panic that is an ARTIFACT).
The e2e suite drives the app through the `__vgms_e2e` hook instead, and runs in a
real headless browser where the canvas composites fine (Chromium needs the
swiftshader flags the Playwright config passes).

## Deferred (documented, not blocking)

- **In-place save to the source `.zip` on native** — Save Pack is a Save As /
  download for now; the memory-zip origin can carry the source path later.
- **The web `beforeunload` dirty guard** — the in-app discard prompt covers
  navigation within the app; the tab-close guard is the follow-up.
- **Real Nuked channel muting** (still the wt-5-era gate) and the wt-9 Web Serial
  *implementation* (a follow-up design, per the go/no-go).

## Traps (each cost real time)

- **`TextEncoder`/`TextDecoder` do NOT exist in `AudioWorkletGlobalScope`** —
  `worklet-processor.js` hand-rolls UTF-8. Never reintroduce them there.
- **Multiple wasm instances each install their own core registry**: the app
  module (`runner.rs`), the task Worker (`worker.rs`), the pack Worker, and the
  AudioWorklet. Miss one and capability gating / playback silently breaks. (The
  pack Worker needs no cores — the built-in optimise pass is stream-only.)
- **PowerShell `pwsh` is not on the Bash tool's PATH** — run `.ps1` scripts via
  the PowerShell tool (`powershell.exe`), or the Linux-native cargo steps.
- **PowerShell `2>&1` on native exes** wraps stderr as errors and sets a false
  non-zero exit — do NOT pipe cargo through `2>&1`.
- **A memory-zip pack's file-op run spans several frames** in the egui_kittest
  harness; drive it with `harness.run_steps(16)`, not `run()` (which settles
  before the batch finishes — the fake services request no repaints).
- **Firefox's bundled Playwright binary won't launch in some locked-down Windows
  sandboxes** (`spawn UNKNOWN` / permission denied). Run `npm run test:chromium`
  there; CI (Ubuntu) runs both projects.
- **Flaky pre-existing `vgms-core` proptest**
  `any_opl_file_projects_identically_to_the_opl_reader` fails ~occasionally on
  synthetic OPL2+OPL3-write files. UNRELATED to this work; re-run if it reddens.
