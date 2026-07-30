// Runs the vgms-cores-libvgm wasm smoke: instantiates the module and calls
// both exports. Exit code 0 only if both chips genuinely sounded.
import { readFileSync } from 'fs';

const path = process.argv[2];
const bytes = readFileSync(path);
const wasmModule = new WebAssembly.Module(bytes);
console.log('imports required:', JSON.stringify(WebAssembly.Module.imports(wasmModule)));

const instance = new WebAssembly.Instance(wasmModule, {});
const { smoke_sn76489, smoke_ym2203_ssg } = instance.exports;

const sn = smoke_sn76489();
const ssg = smoke_ym2203_ssg();
console.log('sn76489 peak:', sn);
console.log('ym2203 linked-ssg peak:', ssg);

if (sn > 1000 && ssg > 1000) {
  console.log('WASM SMOKE PASS');
} else {
  console.log('WASM SMOKE FAIL');
  process.exit(1);
}
