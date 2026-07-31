// The vgmtools wasip1 smoke: run each tool module under node's WASI, feed it a
// VGM through a preopened scratch directory, and check what comes back. This is
// the "it runs at all" proof; byte-parity against the native exes lives in
// `crates/vgms-vgmtools/tests/wasm_parity.rs`.
//
//   node tools/web/vgmtools_smoke.mjs target/wasi-tools tests/lsl3_score_up.vgm
//
// Exit 0 only if every module runs and returns either a valid `Vgm `-headed
// output or nothing (unchanged/declined) -- never garbage.
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { WASI } from 'node:wasi';

const [modulesDir, fixturePath] = process.argv.slice(2);
if (!modulesDir || !fixturePath) {
  console.error('usage: node vgmtools_smoke.mjs <modules-dir> <fixture.vgm>');
  process.exit(2);
}

const input = readFileSync(fixturePath);
const tools = ['tool_vgm_cmp', 'tool_vgm_sro', 'tool_optdac'];
let ok = true;

for (const tool of tools) {
  const module = new WebAssembly.Module(readFileSync(join(modulesDir, `${tool}.wasm`)));

  // A fresh scratch dir per tool: the wasm sees exactly `in.vgm` and writes
  // `out.vgm` beside it, the same two-file world the browser shim presents.
  const dir = mkdtempSync(join(tmpdir(), 'vgms-wasi-smoke-'));
  writeFileSync(join(dir, 'in.vgm'), input);

  const wasi = new WASI({
    version: 'preview1',
    args: [tool, 'in.vgm', 'out.vgm'],
    preopens: { '.': dir },
    returnOnExit: true,
  });
  const instance = new WebAssembly.Instance(module, wasi.getImportObject());
  const code = wasi.start(instance);

  let output = null;
  try { output = readFileSync(join(dir, 'out.vgm')); } catch {}
  rmSync(dir, { recursive: true, force: true });

  const headed = output && output.length >= 4 && output.toString('latin1', 0, 4) === 'Vgm ';
  if (output && !headed) {
    ok = false;
  }
  const verdict = output
    ? (headed ? `${input.length} -> ${output.length} bytes` : 'FAIL (output is not a VGM)')
    : `no output (unchanged/declined, exit ${code})`;
  console.log(`${tool}: exit=${code} ${verdict}`);
}

console.log(ok ? 'VGMTOOLS WASI SMOKE PASS' : 'VGMTOOLS WASI SMOKE FAIL');
process.exit(ok ? 0 : 1);
