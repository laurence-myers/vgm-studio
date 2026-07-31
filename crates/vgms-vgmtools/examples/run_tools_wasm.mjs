// The vgmtools wasm smoke: instantiate each tool module, feed it a VGM through
// the in-memory file table, and print what came back. This is the "it runs at
// all, and imports nothing" proof; byte-parity against the native exes lives in
// `tests/wasm_parity.rs` (ow-4).
//
//   node run_tools_wasm.mjs <examples-dir> <fixture.vgm>
//
// Exit 0 only if every module instantiates with no imports, runs, and returns
// either a valid `Vgm `-headed output or nothing (unchanged) -- never garbage.
import { readFileSync } from 'fs';
import { join } from 'path';

const [examplesDir, fixturePath] = process.argv.slice(2);
if (!examplesDir || !fixturePath) {
  console.error('usage: node run_tools_wasm.mjs <examples-dir> <fixture.vgm>');
  process.exit(2);
}

const input = readFileSync(fixturePath);
const tools = ['tool_vgm_cmp', 'tool_vgm_sro', 'tool_optdac'];
let ok = true;

for (const tool of tools) {
  const wasmPath = join(examplesDir, `${tool}.wasm`);
  const bytes = readFileSync(wasmPath);

  const module = new WebAssembly.Module(bytes);
  const imports = WebAssembly.Module.imports(module);
  if (imports.length !== 0) {
    console.log(`${tool}: FAIL -- imports ${JSON.stringify(imports)}`);
    ok = false;
    continue;
  }

  const instance = new WebAssembly.Instance(module, {});
  const ex = instance.exports;
  // The linear memory can grow during run(), detaching the old buffer, so fetch
  // a fresh view every time.
  const view = () => new Uint8Array(ex.memory.buffer);

  const ptr = ex.reserve_input(input.length);
  view().set(input, ptr);
  const code = ex.run();

  const outLen = ex.output_len();
  const outPtr = ex.output_ptr();
  const output = outLen > 0 ? view().slice(outPtr, outPtr + outLen) : new Uint8Array();

  const logLen = ex.log_len();
  const logPtr = ex.log_ptr();
  const log = logLen > 0 ? new TextDecoder().decode(view().slice(logPtr, logPtr + logLen)) : '';
  const lastLine = log.split(/[\r\n]+/).filter(Boolean).pop() ?? '';

  const headed = output.length >= 4 &&
    output[0] === 0x56 && output[1] === 0x67 && output[2] === 0x6d && output[3] === 0x20; // "Vgm "
  const outputOk = output.length === 0 || headed;
  if (!outputOk) {
    ok = false;
  }

  const verdict = !outputOk ? 'FAIL (output is not a VGM)'
    : output.length === 0 ? 'unchanged'
    : `${input.length} -> ${output.length} bytes`;
  console.log(`${tool}: code=${code} ${verdict}${lastLine ? ` | "${lastLine}"` : ''}`);
}

console.log(ok ? 'TOOLS WASM SMOKE PASS' : 'TOOLS WASM SMOKE FAIL');
process.exit(ok ? 0 : 1);
