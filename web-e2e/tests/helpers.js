// SPDX-License-Identifier: GPL-2.0-or-later
//
// Shared helpers for the web e2e specs: booting the app and reaching the
// `window.__vgms_e2e` hook.

import { fileURLToPath } from "node:url";

/** A tiny committed VGM fixture (1 KB) the app can load. */
export const FIXTURE_VGM = fileURLToPath(
  new URL("../../tests/lsl3_score_up.vgm", import.meta.url),
);

/**
 * Navigates to the app and waits for it to boot. The `__vgms_e2e` hook is
 * installed inside eframe's creator, which only runs once the wgpu/WebGL canvas
 * is up, so awaiting it is the honest "the app booted" signal.
 *
 * @returns the initial state snapshot.
 */
export async function boot(page) {
  // Everything the page says during boot, so a failed boot explains itself
  // (eframe logs through `console`, and the panic hook reports there too).
  const log = [];
  page.on("console", (message) => log.push(`[${message.type()}] ${message.text()}`));
  page.on("pageerror", (error) => log.push(`[pageerror] ${error}`));
  page.on("requestfailed", (request) =>
    log.push(`[requestfailed] ${request.url()} ${request.failure()?.errorText ?? ""}`),
  );

  await page.goto("/index.html");
  try {
    await page.waitForFunction(() => Boolean(window.__vgms_e2e), null, {
      timeout: 45_000,
    });
  } catch (error) {
    // The hook lives inside eframe's creator, so "never appeared" nearly always
    // means the canvas never came up. Say what the browser had to offer.
    const probe = await page.evaluate(probeRenderer).catch((e) => `probe failed: ${e}`);
    error.message += `\n\n--- renderer probe ---\n${JSON.stringify(probe, null, 2)}`;
    error.message += `\n\n--- page console (${log.length}) ---\n${log.join("\n") || "(nothing)"}`;
    throw error;
  }
  return page.evaluate(() => window.__vgms_e2e.state());
}

/** Runs in-page: what WebGL/WebGPU this browser can give eframe. */
function probeRenderer() {
  const renderer = (gl) => {
    if (!gl) return null;
    const debug = gl.getExtension("WEBGL_debug_renderer_info");
    return debug
      ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL)
      : gl.getParameter(gl.RENDERER);
  };
  const canvas = document.createElement("canvas");
  return {
    userAgent: navigator.userAgent,
    webgl2: renderer(canvas.getContext("webgl2")),
    webgl1: renderer(document.createElement("canvas").getContext("webgl")),
    webgpu: "gpu" in navigator,
    crossOriginIsolated: globalThis.crossOriginIsolated,
    loadingText: document.getElementById("vgms-loading")?.textContent ?? null,
  };
}

/** Dispatches an action through the hook. */
export function dispatch(page, name, arg) {
  return page.evaluate(
    ([name, arg]) => window.__vgms_e2e.dispatch(name, arg),
    [name, arg ?? null],
  );
}

/** Reads the current state snapshot. */
export function state(page) {
  return page.evaluate(() => window.__vgms_e2e.state());
}

/**
 * Seeds an OPFS-backed directory with `files` ([{ name, bytes: number[] }]) and
 * installs `window.__vgms_pick_dir` so the app's folder picker resolves to it
 * (the wt-7 OPFS shim) instead of prompting. A fresh valid 1x1 PNG is generated
 * in-page so screenshot entries decode. Returns nothing; call before dispatching
 * OpenPackFolder.
 */
export async function seedPackFolder(page, files, dirName = "vgms-e2e-pack") {
  await page.evaluate(
    async ([files, dirName]) => {
      const root = await navigator.storage.getDirectory();
      try {
        await root.removeEntry(dirName, { recursive: true });
      } catch {
        /* first run: nothing to remove */
      }
      const dir = await root.getDirectoryHandle(dirName, { create: true });
      for (const file of files) {
        const handle = await dir.getFileHandle(file.name, { create: true });
        const writable = await handle.createWritable();
        await writable.write(new Uint8Array(file.bytes));
        await writable.close();
      }
      window.__vgms_pick_dir = async () => dir;
    },
    [files, dirName],
  );
}

/** A valid 1x1 PNG's bytes, generated in-page, as a number[]. */
export async function pngBytes(page) {
  return page.evaluate(async () => {
    const canvas = new OffscreenCanvas(1, 1);
    const context = canvas.getContext("2d");
    context.fillStyle = "#123456";
    context.fillRect(0, 0, 1, 1);
    const blob = await canvas.convertToBlob({ type: "image/png" });
    return Array.from(new Uint8Array(await blob.arrayBuffer()));
  });
}
