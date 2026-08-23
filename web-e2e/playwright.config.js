// SPDX-License-Identifier: GPL-2.0-or-later
//
// Playwright configuration for the web target's regression suite (wt-6..wt-8).
//
// The app is one egui canvas with no DOM to select, so the specs drive it
// through the debug-only `window.__vgms_e2e` hook (built with
// `tools/build-web.ps1 -E2e`) rather than by clicking pixels. A tiny static
// server (serve.mjs) hosts target/web-dist; Chromium and Firefox projects give
// the wt-7 (Chromium File System Access) and wt-8 (everywhere/zip) proofs.

import { defineConfig, devices } from "@playwright/test";

const HOST = process.env.VGMS_E2E_HOST ?? "127.0.0.1";
const PORT = Number(process.env.VGMS_E2E_PORT ?? "5178");
const BASE_URL = `http://${HOST}:${PORT}`;

// Headless WebGL needs a software rasteriser. SwiftShader gives eframe's wgpu
// backend a WebGL2 context in CI where there is no GPU; without it the canvas
// never initialises and the `__vgms_e2e` hook (installed inside eframe's
// creator) is never reached. Autoplay is unblocked so `AudioContext.resume()`
// does not need a real user gesture the harness cannot synthesise.
const CHROMIUM_ARGS = [
  "--use-gl=angle",
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
  "--autoplay-policy=no-user-gesture-required",
];

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
  },
  webServer: {
    command: "node serve.mjs",
    url: `${BASE_URL}/index.html`,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: { args: CHROMIUM_ARGS },
      },
    },
    {
      // Firefox has no bundled software rasteriser like Chromium's SwiftShader:
      // it needs an X display to create any GL context, even headless. Without
      // one, `getContext("webgl2")` is null, eframe's creator never runs and the
      // `__vgms_e2e` hook never appears. That is the job's problem, not this
      // file's: CI wraps the Firefox run in `xvfb-run`, after which Mesa picks
      // llvmpipe on its own. Locally a real display serves. The prefs below only
      // keep WebGL allowed once a context is possible (blocklist bypass).
      name: "firefox",
      use: {
        ...devices["Desktop Firefox"],
        launchOptions: {
          firefoxUserPrefs: {
            "media.autoplay.default": 0,
            "media.autoplay.blocking_policy": 0,
            "webgl.force-enabled": true,
            "webgl.disabled": false,
          },
        },
      },
    },
  ],
});
