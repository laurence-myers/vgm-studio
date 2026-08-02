// SPDX-License-Identifier: GPL-2.0-or-later
//
// The pack-export Web Worker. It hosts the same app wasm the page runs and
// builds one release zip per request, off the main thread so a multi-megabyte
// pack does not jank the frame. Cancellation is the page calling
// `worker.terminate()`; there is nothing to cancel cooperatively here.
//
// It also fetches the three vgmtools optimiser modules (wasm32-wasip1 commands
// built by tools/build-wasi-tools.ps1) and hosts them through the vendored
// browser_wasi_shim (web/wasi-shim/): `__vgms_run_tool` below runs one module
// like a process -- argv, an in-memory directory holding in.vgm, an exit code,
// maybe out.vgm -- and the Rust pipeline interprets the result exactly as it
// interprets a native child process. A module that is missing or fails to
// fetch is passed as empty bytes: the Rust side then falls back to vgms_core's
// built-in pass rather than fail the export.

import init, { vgms_web_run_pack_job } from "./vgms_web.js";
import { runTool } from "./wasi-host.js";

// Runs one optimiser module over one file. Called synchronously from Rust
// (wasm-bindgen binds it as `globalThis.__vgms_run_tool`), which is fine here:
// sync instantiation is allowed off the main thread, and the pack job itself is
// synchronous. The WASI construction (argv, fds, debug: false) lives in
// wasi-host.js, one place, so its one delicate fact -- debug must be stated -- is
// not re-derived here.
globalThis.__vgms_run_tool = (module, name, input) => runTool(module, name, input);

// Best-effort: a missing optimiser module just means the built-in pass, so a
// failed fetch warns and yields empty bytes rather than throwing.
async function fetchOptimiser(url) {
  try {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
  } catch (error) {
    console.warn(
      `pack worker: optimiser module ${url} unavailable; using the built-in pass. (${error})`,
    );
    return new Uint8Array();
  }
}

// Instantiate the app module and fetch the optimiser modules once, in parallel.
// Registering onmessage only after this resolves means a request that arrives
// first is browser-queued until everything is ready.
const ready = (async () => {
  const [, cmp, sro, dac] = await Promise.all([
    init(),
    fetchOptimiser("./tool_vgm_cmp.wasm"),
    fetchOptimiser("./tool_vgm_sro.wasm"),
    fetchOptimiser("./tool_optdac.wasm"),
  ]);
  return { cmp, sro, dac };
})();

self.onmessage = async (event) => {
  let tools;
  try {
    tools = await ready;
  } catch (error) {
    console.error(error);
    self.postMessage({ error: String(error) });
    return;
  }

  const request = new Uint8Array(event.data);
  let result;
  try {
    result = vgms_web_run_pack_job(request, tools.cmp, tools.sro, tools.dac);
  } catch (error) {
    console.error(error);
    self.postMessage({ error: String(error) });
    return;
  }
  // The encoded PackJobOutcome bytes are already an owned buffer, so transfer
  // them back with no extra copy.
  self.postMessage(result, [result.buffer]);
};
