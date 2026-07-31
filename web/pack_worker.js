// SPDX-License-Identifier: GPL-2.0-or-later
//
// The pack-export Web Worker. It hosts the same app wasm the page runs and
// builds one release zip per request, off the main thread so a multi-megabyte
// pack does not jank the frame. Cancellation is the page calling
// `worker.terminate()`; there is nothing to cancel cooperatively here.
//
// It also fetches the three vgmtools optimiser modules (built beside the app by
// tools/build-web.ps1) and hands their bytes to the job, which instantiates one
// per song to run vgm_cmp/vgm_sro/optdac. A module that is missing or fails to
// fetch is passed as empty bytes: the Rust side then falls back to vgms_core's
// built-in pass rather than fail the export.

import init, { vgms_web_run_pack_job } from "./vgms_web.js";

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
  // Transfer the encoded PackJobOutcome bytes back (zero-copy).
  const copy = result.slice();
  self.postMessage(copy, [copy.buffer]);
};
