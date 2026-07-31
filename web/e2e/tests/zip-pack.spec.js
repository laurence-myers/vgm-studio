// SPDX-License-Identifier: GPL-2.0-or-later
//
// wt-8 zip-backed packs, the everywhere path (no File System Access needed).
// These run on both the Chromium and Firefox projects: Firefox is the
// non-Chromium proof that a browser without a writable-directory API still opens,
// edits, and saves a pack.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test, expect } from "@playwright/test";
import { boot, dispatch, state } from "./helpers.js";

const FIXTURE_ZIP = fileURLToPath(
  new URL("../../../tests/e2e-pack.zip", import.meta.url),
);

/** Opens the fixture .zip through the file picker as an in-memory pack. */
async function openZipPack(page) {
  const chooser = page.waitForEvent("filechooser");
  await dispatch(page, "OpenFile");
  await (await chooser).setFiles(FIXTURE_ZIP);
  await expect
    .poll(() => state(page).then((s) => (s.pack ? s.pack.trackNames.length : 0)))
    .toBe(2);
}

test("opens a .zip as an in-memory pack", async ({ page }) => {
  await boot(page);
  await openZipPack(page);

  const pack = (await state(page)).pack;
  expect(pack.trackNames).toEqual(["01 Alpha.vgm", "02 Beta.vgm"]);
  expect(pack.dirty).toBe(false);
});

test("reordering a zip pack renumbers in memory and marks it dirty", async ({
  page,
}) => {
  await boot(page);
  await openZipPack(page);

  await dispatch(page, "PackMoveTrack", { index: 0, delta: 1 });
  await expect
    .poll(() => state(page).then((s) => s.pack.trackNames[0]))
    .toContain("Beta");

  const pack = (await state(page)).pack;
  expect(pack.trackNames).toEqual(["01 Beta.vgm", "02 Alpha.vgm"]);
  // A memory-backed pack's file edits are unsaved until Save Pack.
  expect(pack.dirty).toBe(true);
});

test("Save .zip re-exports the pack and clears dirty", async ({ page }) => {
  await boot(page);
  await openZipPack(page);

  await dispatch(page, "PackMoveTrack", { index: 0, delta: 1 });
  await expect.poll(() => state(page).then((s) => s.pack.dirty)).toBe(true);

  const downloadPromise = page.waitForEvent("download", { timeout: 30_000 });
  await dispatch(page, "PackSaveArchive");
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/\.zip$/);

  // Saved: the pack is clean again.
  await expect.poll(() => state(page).then((s) => s.pack.dirty)).toBe(false);

  // The saved zip is a real archive whose songs are gzipped .vgz entries.
  const stream = await download.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  const asText = Buffer.concat(chunks).toString("latin1");
  expect(asText.slice(0, 2)).toBe("PK");
  expect(asText).toContain("01 Beta.vgz");
});
