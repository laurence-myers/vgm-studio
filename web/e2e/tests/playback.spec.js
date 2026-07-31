// SPDX-License-Identifier: GPL-2.0-or-later
//
// wt-6 playback spec: dispatching Play flips the transport into the playing
// state through the real AudioWorklet path. Headless asserts state, not sound --
// actually hearing audio stays a named manual check (see PLAN.md wt-6).

import { test, expect } from "@playwright/test";
import { boot, dispatch, state, FIXTURE_VGM } from "./helpers.js";

test("Play starts the transport, Stop halts it", async ({ page }) => {
  await boot(page);

  // Load a song first.
  const chooser = page.waitForEvent("filechooser");
  await dispatch(page, "OpenFile");
  await (await chooser).setFiles(FIXTURE_VGM);
  await expect.poll(() => state(page).then((s) => s.hasDocument)).toBe(true);

  await dispatch(page, "Play");
  await expect.poll(() => state(page).then((s) => s.playing)).toBe(true);

  await dispatch(page, "Stop");
  await expect.poll(() => state(page).then((s) => s.playing)).toBe(false);
});
