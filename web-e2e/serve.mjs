// SPDX-License-Identifier: GPL-2.0-or-later
//
// A tiny static server for the built web target (target/web-dist), used by the
// Playwright suite. Deliberately dependency-free (Node's own http/fs) so the
// e2e harness needs only `@playwright/test`.
//
// It sets the few content types the app actually depends on -- crucially
// `application/wasm` for the modules and `text/javascript` for the ESM/Worker
// scripts, without which the browser refuses to instantiate them.

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
// This harness lives at <root>/web-e2e/, so the built target is one level up.
const DIST = resolve(here, "..", "target", "web-dist");
const HOST = process.env.VGMS_E2E_HOST ?? "127.0.0.1";
const PORT = Number(process.env.VGMS_E2E_PORT ?? "5178");

const TYPES = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".json", "application/json; charset=utf-8"],
  [".otf", "font/otf"],
  [".ttf", "font/ttf"],
  [".png", "image/png"],
  [".ico", "image/x-icon"],
]);

const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url ?? "/", `http://${request.headers.host}`);
    let pathname = decodeURIComponent(url.pathname);
    if (pathname === "/" || pathname.endsWith("/")) pathname += "index.html";
    // Resolve under DIST and refuse anything that escapes it.
    const filePath = normalize(join(DIST, pathname));
    if (!filePath.startsWith(DIST)) {
      response.writeHead(403).end("Forbidden");
      return;
    }
    const info = await stat(filePath).catch(() => null);
    if (!info || !info.isFile()) {
      response.writeHead(404).end("Not Found");
      return;
    }
    const body = await readFile(filePath);
    response.writeHead(200, {
      "Content-Type": TYPES.get(extname(filePath)) ?? "application/octet-stream",
      "Cache-Control": "no-store",
    });
    response.end(body);
  } catch (error) {
    response.writeHead(500).end(String(error));
  }
});

server.listen(PORT, HOST, () => {
  // Playwright's webServer waits for this URL to answer before running specs.
  console.log(`serving ${DIST} at http://${HOST}:${PORT}/`);
});
