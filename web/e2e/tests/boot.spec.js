// SPDX-License-Identifier: GPL-2.0-or-later
//
// The baseline web-behaviour specs (wt-6): the app boots on a real browser
// canvas, exposes the e2e hook, and drives an action through it. wt-7/wt-8 add
// their pack specs alongside.

import { test, expect } from "@playwright/test";
import { boot, dispatch, state } from "./helpers.js";

test("boots on a canvas and installs the e2e hook", async ({ page }) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });

  const snapshot = await boot(page);

  expect(snapshot.hasDocument).toBe(false);
  expect(snapshot.activeTab).toBe("editor");
  expect(snapshot.pack).toBeNull();
  expect(
    errors,
    `unexpected console/page errors:\n${errors.join("\n")}`,
  ).toEqual([]);
});

test("dispatch drives an action and state reflects it", async ({ page }) => {
  await boot(page);

  await dispatch(page, "Status", "e2e-probe");
  await expect.poll(() => state(page).then((s) => s.status)).toBe("e2e-probe");
});
