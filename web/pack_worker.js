// SPDX-License-Identifier: GPL-2.0-or-later
//
// The pack-export Web Worker. It hosts the same app wasm the page runs and
// builds one release zip per request, off the main thread so a multi-megabyte
// pack does not jank the frame. Cancellation is the page calling
// `worker.terminate()`; there is nothing to cancel cooperatively here.

import init, { vgms_web_run_pack_job } from "./vgms_web.js";

// Compile + instantiate the module once. Registering onmessage only after this
// is created means a request that arrives first is browser-queued until init
// resolves.
const ready = init();

self.onmessage = async (event) => {
  await ready;
  const request = new Uint8Array(event.data);
  let result;
  try {
    result = vgms_web_run_pack_job(request);
  } catch (error) {
    console.error(error);
    self.postMessage({ error: String(error) });
    return;
  }
  // Transfer the encoded PackJobOutcome bytes back (zero-copy).
  const copy = result.slice();
  self.postMessage(copy, [copy.buffer]);
};
