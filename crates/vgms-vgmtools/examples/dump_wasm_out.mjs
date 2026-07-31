// Dump one tool module's output bytes to a file, for parity debugging.
//   node dump_wasm_out.mjs <tool.wasm> <input.vgm> <out.bin>
import { readFileSync, writeFileSync } from 'fs';
const [wasmPath, inPath, outPath] = process.argv.slice(2);
const input = readFileSync(inPath);
const instance = new WebAssembly.Instance(new WebAssembly.Module(readFileSync(wasmPath)), {});
const ex = instance.exports;
const view = () => new Uint8Array(ex.memory.buffer);
const ptr = ex.reserve_input(input.length);
view().set(input, ptr);
const code = ex.run();
const len = ex.output_len();
const optr = ex.output_ptr();
const out = len > 0 ? view().slice(optr, optr + len) : new Uint8Array();
writeFileSync(outPath, out);
console.log(`code=${code} wrote ${out.length} bytes to ${outPath}`);
