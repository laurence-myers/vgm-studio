// SPDX-License-Identifier: GPL-2.0-or-later
//
// Owns the one way this app constructs a WASI instance for the vgmtools command
// modules: the argv, the in-memory file descriptors, and -- said out loud --
// `debug: false`. The vendored browser_wasi_shim (web/wasi-shim/) treats an
// *absent* debug option as "enable", whose per-syscall logging would flood the
// console, so stating it in exactly one place keeps that fact from drifting.
//
// Deliberately separate from the vendored shim, so those files stay byte-for-byte
// identical to upstream. tools/web/vgmtools_smoke.mjs drives this same shim under
// node, so a change that broke it would not pass silently.

import { WASI, File, PreopenDirectory, ConsoleStdout } from "./wasi-shim/index.js";

// Runs `module` as a process over `input` (the bytes of in.vgm). Returns
// { code, output, tail }: the exit code, the bytes of out.vgm (null if the tool
// wrote nothing), and the last non-empty line it printed (for error messages).
export function runTool(module, name, input) {
  const lines = [];
  const collect = ConsoleStdout.lineBuffered((line) => {
    lines.push(line);
    if (lines.length > 16) lines.shift();
  });
  const preopen = new PreopenDirectory(".", [["in.vgm", new File(input)]]);
  const wasi = new WASI([name, "in.vgm", "out.vgm"], [], [collect, collect, collect, preopen], {
    debug: false,
  });

  const instance = new WebAssembly.Instance(module, {
    wasi_snapshot_preview1: wasi.wasiImport,
  });
  const code = wasi.start(instance);

  const entry = preopen.dir.contents.get("out.vgm");
  const output = entry ? entry.data : null;
  const tail = lines.filter((l) => l.trim().length > 0).pop() ?? "";
  return { code, output, tail };
}
