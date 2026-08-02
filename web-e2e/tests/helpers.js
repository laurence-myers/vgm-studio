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
  await page.goto("/index.html");
  await page.waitForFunction(() => Boolean(window.__vgms_e2e), null, {
    timeout: 45_000,
  });
  return page.evaluate(() => window.__vgms_e2e.state());
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
