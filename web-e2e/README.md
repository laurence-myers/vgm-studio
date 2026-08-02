# Web target e2e suite (wt-6)

A Playwright regression suite that pins the browser-only behaviours of the web
target — the parts no native test can reach. It is **test-only**: `npm` enters
the repo here as a dev dependency, and the build pipeline (`tools/build-web.ps1`)
stays npm-free.

## How it drives the app

The app is one egui canvas with no DOM to select, so the specs do not click
pixels. An `-E2e` build installs a debug-only hook, `window.__vgms_e2e`, with:

- `dispatch(name, arg)` — queue an [`Action`] to run on the next frame (drained
  by `VgmStudioApp::e2e_enqueue_action`).
- `state()` — a JSON snapshot of the state the specs assert on
  (`VgmStudioApp::e2e_snapshot`): loaded document, row count, playing, status,
  active tab, alert/dialog, and the open pack's tracks/images.

The hook is installed inside eframe's creator, which only runs once the
wgpu/WebGL canvas is up — so `waitForFunction(() => window.__vgms_e2e)` doubles
as the honest "the app booted" signal. A **release** build never enables the
`e2e` feature, so the hook never reaches users.

Native file/save/download dialogs are driven with Playwright's own primitives
(`filechooser`, `download`). The Chromium-only `showDirectoryPicker` /
`showSaveFilePicker` used by pack mode are shimmed against OPFS in the `e2e`
build — that shim lands with wt-7, where it is first exercised.

## Running it

From the repo root, build the servable dist **with the hook**:

```powershell
pwsh tools/build-web.ps1 -E2e
```

Then, in this directory:

```bash
npm install                     # or `npm ci` in CI
npx playwright install chromium # + firefox for the full matrix
npm test                        # both projects
npm run test:chromium           # just Chromium
```

The suite starts its own static server (`serve.mjs`, dependency-free) over
`target/web-dist`; there is no dev server to run first.

## Browser matrix

- **Chromium** and **Firefox** projects. Chromium gets the wt-7 File System
  Access pack proofs; Firefox is the non-Chromium proof for wt-8's zip packs.
- Headless WebGL needs a software rasteriser: the Chromium project passes
  `--use-angle=swiftshader --enable-unsafe-swiftshader`, and autoplay is
  unblocked so `AudioContext.resume()` needs no real gesture.

## Manual residue (named, not automated)

- **Actually hearing audio** — headless asserts the transport *state*, not sound.
- **Real OS permission prompts** — the true `showDirectoryPicker` grant dialog.

## Local caveat

Some locked-down Windows environments refuse to execute Playwright's bundled
`firefox.exe` (a `spawn UNKNOWN` / "permission denied" at launch). Chromium is
unaffected; run `npm run test:chromium` there. CI (Ubuntu) runs the full matrix.
