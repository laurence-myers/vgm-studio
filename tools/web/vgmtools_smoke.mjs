// The vgmtools wasip1 smoke: run each tool module through the *vendored* WASI
// shim (web/wasi-shim/, via web/wasi-host.js -- the same code path the pack
// worker uses in the browser), feed it a VGM, and check what comes back. Driving
// the vendored shim here is deliberate: it is the only thing that exercises it
// outside a browser, so a change that broke it fails this smoke rather than
// passing silently. Byte-parity against the native exes lives in
// `crates/vgms-vgmtools/tests/wasm_parity.rs`.
//
//   node tools/web/vgmtools_smoke.mjs target/wasi-tools tests/lsl3_score_up.vgm
//
// Exit 0 only if every module runs and returns either a valid `Vgm `-headed
// output or nothing (unchanged/declined) -- never garbage.
import { readFileSync } from 'fs';
import { join } from 'path';
import { runTool } from '../../web/wasi-host.js';

const [modulesDir, fixturePath] = process.argv.slice(2);
if (!modulesDir || !fixturePath) {
  console.error('usage: node vgmtools_smoke.mjs <modules-dir> <fixture.vgm>');
  process.exit(2);
}

const input = new Uint8Array(readFileSync(fixturePath));
const tools = ['tool_vgm_cmp', 'tool_vgm_sro', 'tool_optdac'];
let ok = true;

for (const tool of tools) {
  const module = new WebAssembly.Module(readFileSync(join(modulesDir, `${tool}.wasm`)));

  // The shim presents the same two-file world the browser gives: `in.vgm` in,
  // `out.vgm` out, both in memory.
  const { code, output } = runTool(module, tool, input);

  const headed =
    output && output.length >= 4 && Buffer.from(output.slice(0, 4)).toString('latin1') === 'Vgm ';
  if (output && !headed) {
    ok = false;
  }
  const verdict = output
    ? headed
      ? `${input.length} -> ${output.length} bytes`
      : 'FAIL (output is not a VGM)'
    : `no output (unchanged/declined, exit ${code})`;
  console.log(`${tool}: exit=${code} ${verdict}`);
}

console.log(ok ? 'VGMTOOLS WASI SMOKE PASS' : 'VGMTOOLS WASI SMOKE FAIL');
process.exit(ok ? 0 : 1);
