// SPDX-License-Identifier: GPL-2.0-or-later
//
// The wasm proof for `vgms-synth-worklet`: loads the compiled cdylib, asserts it
// imports *nothing* (the AudioWorkletGlobalScope offers no libc, so the module
// must be self-contained), then drives the real ABI -- init, load, render -- for
// both engine arms and requires audible output. This is the browser-free half of
// the CI TODO the placeholder crate left ("assert its import section is empty").
//
// Usage: node tools/web/worklet_smoke.mjs <path-to.wasm>
//   default path: target/wasm32-unknown-unknown/release/vgms_synth_worklet.wasm

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const wasmPath =
  process.argv[2] ??
  fileURLToPath(
    new URL(
      '../../target/wasm32-unknown-unknown/release/vgms_synth_worklet.wasm',
      import.meta.url,
    ),
  );
const fixturePath = fileURLToPath(
  new URL('../../tests/lsl3_score_up.vgm', import.meta.url),
);

const moduleBytes = readFileSync(wasmPath);
const wasmModule = new WebAssembly.Module(moduleBytes);

// -- 1. the import section must be empty ---------------------------------------
const imports = WebAssembly.Module.imports(wasmModule);
if (imports.length !== 0) {
  console.log('imports:', JSON.stringify(imports));
  console.log(
    `WORKLET SMOKE FAIL: the module imports ${imports.length} symbol(s); ` +
      'an AudioWorklet module must import nothing.',
  );
  process.exit(1);
}
console.log('imports: [] (import-free, as an AudioWorklet module must be)');

const instance = new WebAssembly.Instance(wasmModule, {});
const ex = instance.exports;

// -- helpers over the module's own linear memory -------------------------------
function copyIn(data) {
  const ptr = ex.vgmsw_alloc(data.length);
  new Uint8Array(ex.memory.buffer).set(data, ptr);
  return ptr;
}

function readError() {
  const len = ex.vgmsw_error_len();
  if (len === 0) return '';
  const ptr = ex.vgmsw_alloc(len);
  ex.vgmsw_error_copy(ptr, len);
  const text = new TextDecoder().decode(
    new Uint8Array(ex.memory.buffer, ptr, len).slice(),
  );
  ex.vgmsw_free(ptr, len);
  return text;
}

// Loads a song and returns the loudest |sample| over `quanta` render calls of
// `frames` each. Uses a DataView so it is agnostic to the buffer's alignment.
function loadAndPeak(name, songBytes, { sampleRate = 48000, frames = 128, quanta } = {}) {
  ex.vgmsw_init();
  const nameBytes = new TextEncoder().encode(name);
  const namePtr = copyIn(nameBytes);
  const songPtr = copyIn(songBytes);
  const code = ex.vgmsw_load(namePtr, nameBytes.length, songPtr, songBytes.length, sampleRate, 0);
  ex.vgmsw_free(namePtr, nameBytes.length);
  ex.vgmsw_free(songPtr, songBytes.length);
  if (code !== 0) {
    throw new Error(`load "${name}" failed (${code}): ${readError()}`);
  }

  const bytesPerBuffer = frames * 4;
  const leftPtr = ex.vgmsw_alloc(bytesPerBuffer);
  const rightPtr = ex.vgmsw_alloc(bytesPerBuffer);
  let peak = 0;
  for (let q = 0; q < quanta; q++) {
    ex.vgmsw_render(leftPtr, rightPtr, frames);
    const view = new DataView(ex.memory.buffer);
    for (let i = 0; i < frames; i++) {
      peak = Math.max(
        peak,
        Math.abs(view.getFloat32(leftPtr + i * 4, true)),
        Math.abs(view.getFloat32(rightPtr + i * 4, true)),
      );
    }
  }
  ex.vgmsw_free(leftPtr, bytesPerBuffer);
  ex.vgmsw_free(rightPtr, bytesPerBuffer);
  return peak;
}

// A bare single-chip SN76489 VGM (mirrors the crate's native test): latch a tone,
// open the volume, wait a second, end. Exercises the generic VgmEngine arm.
function sn76489Vgm() {
  const out = new Uint8Array(0x80 + 10);
  const dv = new DataView(out.buffer);
  out.set(new TextEncoder().encode('Vgm '), 0);
  dv.setUint32(0x08, 0x151, true); // version
  dv.setUint32(0x34, 0x80 - 0x34, true); // data offset (relative)
  dv.setUint32(0x0c, 3_579_545, true); // SN76489 clock
  dv.setUint32(0x18, 44_100, true); // total samples
  out.set([0x50, 0x8e, 0x50, 0x02, 0x50, 0x90, 0x61, 0x44, 0xac, 0x66], 0x80);
  dv.setUint32(0x04, out.length - 4, true); // EOF offset
  return out;
}

// -- 2. both engine arms sound -------------------------------------------------
const oplPeak = loadAndPeak('lsl3_score_up.vgm', readFileSync(fixturePath), { quanta: 375 });
console.log(`OPL engine peak: ${oplPeak.toFixed(4)}`);

const snPeak = loadAndPeak('tone.vgm', sn76489Vgm(), { quanta: 188 });
console.log(`generic engine (SN76489) peak: ${snPeak.toFixed(4)}`);

if (oplPeak > 0.01 && snPeak > 0.01) {
  console.log('WORKLET SMOKE PASS');
} else {
  console.log('WORKLET SMOKE FAIL: an engine arm produced silence');
  process.exit(1);
}
