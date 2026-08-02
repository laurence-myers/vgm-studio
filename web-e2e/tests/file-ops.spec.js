// SPDX-License-Identifier: GPL-2.0-or-later
//
// wt-6 baseline file specs: opening a song through the browser file picker
// (Playwright drives the native <input type=file>), and saving it back as a
// browser download.

import { test, expect } from "@playwright/test";
import { boot, dispatch, state, FIXTURE_VGM } from "./helpers.js";

/** Opens the fixture VGM through the app's file picker. */
async function openFixture(page) {
  const chooser = page.waitForEvent("filechooser");
  await dispatch(page, "OpenFile");
  await (await chooser).setFiles(FIXTURE_VGM);
  await expect
    .poll(() => state(page).then((s) => s.hasDocument))
    .toBe(true);
}

test("opens a VGM through the file picker and loads it", async ({ page }) => {
  await boot(page);
  await openFixture(page);

  const snapshot = await state(page);
  expect(snapshot.documentName).toContain("lsl3_score_up");
  expect(snapshot.rowCount).toBeGreaterThan(0);
});

test("saves the loaded song back as a browser download", async ({ page }) => {
  await boot(page);
  await openFixture(page);

  const downloadPromise = page.waitForEvent("download");
  // On the web there is no in-place path, so Save As lands as a Blob download.
  await dispatch(page, "SaveAs");
  const download = await downloadPromise;

  expect(download.suggestedFilename()).toContain("lsl3_score_up");
  const stream = await download.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  expect(Buffer.concat(chunks).length).toBeGreaterThan(0);
});
