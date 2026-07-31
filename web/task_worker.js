// SPDX-License-Identifier: GPL-2.0-or-later
//
// The Web Worker bootstrap for the background-task system. It loads the same app
// module the page runs, then for each encoded TaskRequest the page posts, calls
// the module's `vgms_web_run_task`, forwarding every encoded TaskResult back to
// the page and a final "done" once the task returns.
//
// A module worker (`{ type: "module" }`), so it can `import` the wasm-bindgen
// glue. Cancellation is the page calling `worker.terminate()` on this Worker;
// nothing here has to handle it.

import init, { vgms_web_run_task } from "./vgms_web.js";

// Compile + instantiate the module once, before handling any request. Requests
// that arrive first are queued by the browser and delivered after `init` resolves
// because we only register `onmessage` afterwards.
const ready = init();

self.onmessage = async (event) => {
  await ready;

  const request = new Uint8Array(event.data);

  // Post each result's bytes straight back to the page, transferring the buffer
  // so nothing is copied.
  const emit = (bytes) => {
    const copy = bytes.slice();
    self.postMessage(copy, [copy.buffer]);
  };

  try {
    vgms_web_run_task(request, emit);
  } catch (error) {
    console.error("vgms-web task worker:", error);
  }

  // Signal completion so the page can mark this task kind idle and reap us.
  self.postMessage("done");
};
