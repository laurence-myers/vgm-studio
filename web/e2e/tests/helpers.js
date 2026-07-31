// SPDX-License-Identifier: GPL-2.0-or-later
//
// Shared helpers for the web e2e specs: booting the app and reaching the
// `window.__vgms_e2e` hook.

import { fileURLToPath } from "node:url";

/** A tiny committed VGM fixture (1 KB) the app can load. */
export const FIXTURE_VGM = fileURLToPath(
  new URL("../../../tests/lsl3_score_up.vgm", import.meta.url),
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
