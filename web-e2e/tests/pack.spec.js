// SPDX-License-Identifier: GPL-2.0-or-later
//
// wt-7 pack mode on Chromium (File System Access). The specs drive the app
// against an OPFS-backed folder (the `__vgms_pick_dir` shim) so no real OS
// picker prompt is needed, exercising the true scan / rename / delete / write
// paths of pack_fs.js.

import { readFileSync } from "node:fs";
import { test, expect } from "@playwright/test";
import { boot, dispatch, state, seedPackFolder, pngBytes, FIXTURE_VGM } from "./helpers.js";

// File System Access folder pickers are a Chromium capability; the wt-8 zip
// packs are the everywhere answer and are proved in the Firefox project.
test.describe.configure({ mode: "serial" });

const VGM = Array.from(readFileSync(FIXTURE_VGM));

// A minimal VGMRips-format description, so the pack parses a game name (required
// for export) and opens without a parse warning.
const DESCRIPTION = "Game name:  E2E Test Pack\nSystem:  Test System\n\nSong list:\n";

/** Boots, seeds an OPFS pack (two songs, a doc, a screenshot), and opens it. */
async function openSeededPack(page) {
  await boot(page);
  const png = await pngBytes(page);
  await seedPackFolder(page, [
    { name: "01 Alpha.vgm", bytes: VGM },
    { name: "02 Beta.vgm", bytes: VGM },
    { name: "Game.txt", bytes: Array.from(Buffer.from(DESCRIPTION)) },
    { name: "Shot.png", bytes: png },
  ]);
  await dispatch(page, "OpenPackFolder");
  await expect
    .poll(() => state(page).then((s) => (s.pack ? s.pack.trackNames.length : 0)))
    .toBe(2);
  return (await state(page)).pack;
}

test("opens an OPFS-backed pack folder", async ({ page }) => {
  const pack = await openSeededPack(page);
  expect(pack.trackNames).toEqual(["01 Alpha.vgm", "02 Beta.vgm"]);
  expect(pack.imageNames).toEqual(["Shot.png"]);
});

test("reordering a track renumbers the files on disk", async ({ page }) => {
  await openSeededPack(page);

  // Move the first track down one slot; the file numbers are rewritten (via the
  // temp-name dance in pack_fs.js) and the folder is rescanned.
  await dispatch(page, "PackMoveTrack", { index: 0, delta: 1 });
  await expect
    .poll(() => state(page).then((s) => s.pack.trackNames[0]))
    .toContain("Beta");

  const pack = (await state(page)).pack;
  expect(pack.trackNames).toEqual(["01 Beta.vgm", "02 Alpha.vgm"]);
});

test("deleting a screenshot and undoing restores it", async ({ page }) => {
  await openSeededPack(page);

  await dispatch(page, "ConfirmDeleteScreenshot", "Shot.png");
  await expect
    .poll(() => state(page).then((s) => s.pack.imageNames.length))
    .toBe(0);

  await dispatch(page, "Undo");
  await expect
    .poll(() => state(page).then((s) => s.pack.imageNames))
    .toEqual(["Shot.png"]);
});

test("exporting builds a release zip in the Worker and downloads it", async ({
  page,
}) => {
  await openSeededPack(page);

  const downloadPromise = page.waitForEvent("download", { timeout: 30_000 });
  // ConfirmExportZip skips the soft-warning gate (it only blocks on hard errors),
  // so a well-formed pack exports straight away.
  await dispatch(page, "ConfirmExportZip");
  const download = await downloadPromise;

  expect(download.suggestedFilename()).toMatch(/\.zip$/);
  const stream = await download.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  const bytes = Buffer.concat(chunks);

  // A real zip, built by the wasm pack builder in the Worker.
  expect(bytes.subarray(0, 2).toString("latin1")).toBe("PK");
  // gzip-on-export is the default, so the songs are .vgz entries.
  const asText = bytes.toString("latin1");
  expect(asText).toContain("01 Alpha.vgz");
  expect(asText).toContain("02 Beta.vgz");
});
