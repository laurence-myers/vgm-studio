# browser_wasi_shim, vendored

The published `dist/` of [`@bjorn3/browser_wasi_shim`](https://github.com/bjorn3/browser_wasi_shim)
0.4.2, MIT OR Apache-2.0 (both texts beside this file), vendored verbatim --
plain ESM the pack Worker imports directly, so there is no bundler and no npm
step in the web build.

It is the WASI preview1 host for the three vgmtools optimiser modules
(`tool_vgm_cmp.wasm`, `tool_vgm_sro.wasm`, `tool_optdac.wasm`, built by
`tools/build-wasi-tools.ps1`): `pack_worker.js` gives each run an in-memory
`PreopenDirectory` holding `in.vgm`, collects `out.vgm` and the exit code, and
hands them to the Rust pipeline. Upgrading is `npm pack @bjorn3/browser_wasi_shim`,
copy `dist/*.js` + the two licence files over, and re-run the web e2e suite.
