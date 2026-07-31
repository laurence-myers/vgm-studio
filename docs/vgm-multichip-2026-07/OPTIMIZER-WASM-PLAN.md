# The optimisers on the web: a wasm instance is a better process than a process

> **Status: PROPOSED (2026-07-30).** Follows `OPTIMIZER-PLAN.md`, whose ot-1
> chose child processes for the desktop. This is the web half. It can be built
> and proven under node *before* `vgms-web` exists, exactly as the libvgm wasm
> spike was.

## The problem, and why it is smaller than it looks

`vgms-vgmtools` runs each tool as a child process. The browser has no processes,
no `fork`, no filesystem and no `argv`. On the face of it none of the desktop
design survives.

But go back to *why* ot-1 wanted a process. It bought four things:

| What the process boundary bought | What a fresh wasm instance gives |
|---|---|
| Fresh globals every run (re-entrancy) | Zero-initialised data + BSS on every instantiate -- by construction |
| Reclaims the ~50 leaked ROM buffers | Dropping the instance frees the whole linear memory in one go, in O(1) |
| An access violation kills only the child | A wasm trap is a catchable exception; the host never sees a bad pointer |
| A timeout can kill a hung run | **Nothing. This is the one real gap.** |

Three of the four are not merely preserved but *improved*: a wasm instance is
cheaper than a process, and `--max-memory` gives a hard allocation ceiling that
no native child has. One trap disappears entirely for free -- `DblClickWait`
only waits on `_getch()` under `_WIN32`, and `common.h`'s `#else` branch is
already a no-op ("Double-clicking a console application on Unix doesn't open a
console window"). The two minutes it cost on desktop simply cannot happen here.

So the plan is not "port the tools to the web". It is "notice that the web has
a better process, and close the one gap it leaves".

## Three keys

### Key 1 -- the entire I/O surface is eight functions

Counted across all five sources:

```
81  printf
 9  gzseek        3  fopen
 9  gzread        3  fwrite
 6  gzclose       3  fclose
 3  gzopen
```

That is everything. **No `exit`, no `abort`, no `setjmp`, no `signal`, no
`time`, no `rand`, no `system`, no threads.** Input is read-only through four
`gz*` calls; output is write-only through three `FILE*` calls; the rest is
logging.

So there is no need for WASI, no need for emscripten, and no need for a virtual
filesystem. An **in-memory file table with exactly two entries** -- the input we
preload and the output we capture -- satisfies every call the tools can make. It
is about a hundred lines of C, and it is a strict simplification of what the
desktop build already does with `shim/zshim.c`.

The freestanding libc underneath it already exists and is proven: the
`vgms-cores-libvgm` wasm spike (`6d3dbef`) linked 38 devices with
`-ffreestanding`, `shim/wasm-libc/` headers, `src/wasm_libc.rs` for the
`malloc`/`str*` family, and `shim/wasm_stubs.c` for the printf family. That work
is reusable almost verbatim; `stdio.h` needs to grow `FILE`, and `printf` should
capture rather than truncate so the log tail still works.

### Key 2 -- one module per tool, so the hardest desktop problem never arises

The expensive part of ot-1 was symbol isolation: three programs each defining
`VGMHead`, `VGMData`, `main`, with `llvm-objcopy --keep-global-symbol`
unsupported for COFF and the chip-write functions differing in return type
between tools.

Build **three separate `.wasm` modules** and all of it evaporates. Each module
is its own link unit with its own linear memory; two tools can no more collide
than two processes can. No renaming, no objcopy, no one-TU `#include` trick.
The desktop route reached this conclusion by choosing executables; wasm gets it
for free.

### Key 3 -- a Worker is the kill switch, and the known loop is defusable

The one genuine gap is the infinite loop: `chip_srom.c:3268` doubles a `UINT32`
mask past a ROM size read verbatim from a data block, and above `0x80000000` the
mask wraps to zero and spins. Wasm cannot be preempted on its own thread.

Two answers, and we take both:

1. **Host the modules in a dedicated Web Worker.** `worker.terminate()` is the
   exact analogue of killing a child process, and it is why the optimiser gets
   its *own* worker rather than sharing the app's -- terminating it must cost
   nothing but the current file.
2. **Defuse the known loop before it starts.** We already parse the file:
   `vgms_core` walks `0x67` blocks and knows each declared ROM size. A size at or
   above `0x8000_0000` is not a file we should optimise, it is a file whose
   header is wrong -- so the pipeline refuses that stage with a reason, on
   *both* targets. This is defence in depth rather than a substitute for the
   timeout: it fixes the loop we know about, and the worker covers the ones we
   do not.

A hard `--max-memory` on each module turns any runaway allocation into a trap
instead of a dead tab, which is a containment the native child does not have.

## Architecture

`ToolOutcome`, `optimize_vgm`, the stage reporting and the wholly-OPL bypass are
all target-independent and stay exactly as they are. Only the primitive
underneath changes -- "run one tool over these bytes":

```
                     pipeline.rs  (shared: order, bypass, stage report)
                            |
              +-------------+-------------+
              |                           |
   #[cfg(not(wasm32))]              #[cfg(wasm32)]
   run.rs: child process            wasm.rs: instantiate + call, in a Worker
   (temp files, MSYSTEM, kill)      (in-memory files, fresh instance, terminate)
```

**The web path is async and that is correct**, not a compromise: the app already
routes long work through its job system (`TaskKind::LoopSearch`,
`VolumeScan`, `SplitSongs`), so an optimiser that returns a future fits the
pattern it already has. Native stays synchronous.

## The oracle this gets for free

The desktop path already exists and runs the real programs. So the wasm build
has a reference implementation on day one:

> **Run both over the corpus and require byte-identical output.**

Any divergence is a shim bug, and it is localised to the ~100 lines of file
table, because everything above it is the same C. Node can drive the modules
directly -- the libvgm spike already ships
`crates/vgms-cores-libvgm/examples/run_wasm_smoke.mjs` as the pattern -- so this
gate runs in CI **without a browser and without `vgms-web`**.

That is what makes the whole thing safe to build now and wire later.

## Steps

**ow-1 -- the file table.** `shim/memfile.c`: `gzopen`/`gzread`/`gzseek`/
`gzclose`/`fopen`/`fwrite`/`fclose` over two in-memory buffers, plus a `printf`
that appends to a capped ring buffer. Exports `vgmt_set_input(ptr,len)`,
`vgmt_output_ptr/len`, `vgmt_log_ptr/len`. Used by the wasm build only at first.

**ow-2 -- build three modules.** Extend `build.rs`: on `target_arch = "wasm32"`,
compile each tool to its own `.wasm` with clang, `-ffreestanding`, the
`wasm-libc` headers, `--no-entry`, `--export=<tool>_main`, `--max-memory`.
Prove each links with no imports, as the libvgm spike did.

**ow-3 -- the node gate.** A `run_tools_wasm.mjs` example that instantiates each
module, feeds a fixture, and prints the result. This is the "it runs at all"
proof and the first place the file table is exercised.

**ow-4 -- byte-parity against the exes.** Extend `tests/corpus.rs`: for each
sampled file, run the native tool and the wasm module and require identical
bytes. The single most valuable test in this plan.

**ow-5 -- the ROM-size guard.** Refuse `vgm_sro` on any file declaring a block
size at or above `0x8000_0000`, on both targets, with a reason on the stage.

**ow-6 -- the worker host.** JS glue that owns the compiled modules, instantiates
one per call, and enforces the timeout by terminating itself. `wasm.rs` behind
the same API, `#[cfg(target_arch = "wasm32")]`.

**ow-7 -- wire it into `vgms-web`.** Waits on Step 8 of the web programme; ow-1
to ow-6 do not.

## Risks

- **`printf` with a real formatter.** The libvgm stub truncates because nothing
  reads the text; here the log tail is a user-facing error message. Either
  implement a small `vsnprintf` subset (the tools use `%u`, `%s`, `%X`, `%.1f`)
  or accept losing the message on wasm and report the exit code alone. Prefer
  the subset -- it is the difference between "vgm_sro failed" and "vgm_sro said
  RF5Cxx Memory Writes aren't supported".
- **Instantiation cost** should be microseconds once the module is compiled
  once and instantiated per run, but if it ever bites, the fallback is one
  long-lived instance plus a bump-arena allocator reset between runs -- which
  reclaims the leak just as well, only without the fresh-BSS guarantee.
- **`long` is 32-bit on wasm32** and 32-bit on Windows too, so `gzseek`'s
  `z_off_t` shim keeps the same width; a 64-bit Unix host is where that would
  need care.
- **Module size**: three C programs, no zlib, no libm to speak of -- expect
  tens of KB each, against libvgm's 541 KB. Not a concern.

---

## Re-evaluation (2026-07-31)

Checked every load-bearing claim above against the tree before building. The
plan is **sound and its central insight holds** -- a fresh wasm instance is a
better process, and the native path is a byte-exact oracle. Seven corrections,
all narrowing the design toward code that already exists and is proven, plus one
piece of good news: **Step 8 landed, so ow-7 is no longer blocked.**

**A. The I/O surface is eight functions; the *libc* surface is larger -- but
already solved.** Grepping the five sources (`vgm_cmp`, `chip_cmp`, `vgm_sro`,
`chip_srom`, `optdac`) confirms the eight I/O calls, and confirms *no* `exit`,
`abort`, `setjmp`, `signal`, `time`, `rand`, `system`, `qsort`, `atoi`,
`strtoul`, or `math.h` (the two `time` hits are a comment and a struct field; the
one `getchar` is inside an undefined `#ifdef REMOVE_NES_DPCM_0`). But the tools
*do* need `malloc`/`calloc`/`realloc`/`free`, the `str*`/`mem*` family, and
`fgets`+`stdin` (for `ReadFilename`, which is compiled-but-unreached because we
always pass argv). **Every one of these already exists, proven and node-verified,
in `vgms-cores-libvgm/src/wasm_libc.rs` + `shim/wasm-libc/`.** That is the
"reuse almost verbatim" the plan promised -- but it means reusing the *Rust
allocator*, which decides B.

**B. Build model: reuse the proven cc+cargo+`wasm_libc` path, not a standalone
`clang --no-entry` link.** ow-2 as written (invoke clang directly, three
freestanding `.wasm`) would force a hand-written C allocator -- the single
scariest new component, and the one thing the libvgm spike does *not* prove.
Instead, build each tool as its own **`[[example]] crate-type = ["cdylib"]`**,
exactly the pattern `examples/wasm_smoke.rs` uses (just re-verified: 544 KB, zero
imports, runs under node). Three examples -> three independent `.wasm`, each its
own linear memory (**Key 2 preserved**), reusing the Rust allocator + `str*` for
free. Symbol collisions between tools are avoided by `-Dmain=<tool>_main` and
archive-pull isolation (an example that references only `cmp_main` never pulls
`vgm_sro.o`). *This isolation is asserted, not assumed -- if the one-crate build
duplicate-symbols, the fallback is three sibling crates; same three `.wasm`
either way.*

**C. `memfile.c` reuses `zshim.c` unchanged, and routes by open-mode.**
`WriteVGMFile` writes output with `fopen(name,"wb")`+`fwrite`; `OpenVGMFile`
reads input with `gzopen(name,"rb")` -> `zshim` -> `fopen(...,"rb")`. So a
`FILE*` layer over two in-memory slots -- **read-open -> input, write-open ->
output** -- satisfies everything, and `zshim.c` compiles verbatim on top of it.
No filename table keyed by name (simpler and more robust than ow-1's sketch);
`stdio.h` grows `FILE` + the `f*` family as the plan foresaw.

**D. `printf` quality is off the critical path.** Byte-parity (ow-4) is
independent of `printf` output, so nothing about the log formatter can break the
key test. Ship a compact capturing `vsnprintf` subset (`%d %u %x %X %c %s %%`,
width/zero-pad, `%l`, `%.1f`) from the start for useful error tails, but its
correctness is a quality concern, never a parity one.

**E. ow-5 guard, pinned.** The loop is `chip_srom.c:3268`
`for(rom_mask=1; rom_mask<ROMSize; rom_mask*=2);` -- a `UINT32` wrap to 0 for any
`0x67` block declaring a ROM size >= `0x8000_0000`. Guard = refuse `vgm_sro`
when `vgms_core` sees such a block, on both targets, with a stage reason. The
worker-terminate covers the unknown hangs; this fixes the one we know.

**F. Parity harness = `wasmi` in a Rust test, plus the node smoke.** ow-4 stays a
Rust `#[ignore]`, env-gated test in the `corpus.rs` idiom, driving the pre-built
`.wasm` through the pure-Rust **`wasmi`** (a light dev-dependency) and comparing
against the existing native `optimize_writes`/`trim_sample_roms`/`clean_dac_runs`
-- one language, no browser, no node. ow-3's node smoke stays as the independent
"no imports, it runs" proof.

**G. ow-7 is unblocked. `crates/vgms-web` exists and ships the seam.** The pack
worker already carries an `optimize_vgms` flag into
`vgms-web/src/pack_zip.rs::optimize_song`, which today runs *only* `vgms_core`'s
built-in pass (wt-7's deliberate placeholder). ow-7 = drive the three tool
modules from that seam. The right integration keeps the plan's design intent:
the tools run as **separate modules instantiated fresh per song** (not linked
into `vgms-web`'s long-lived linear memory, which would resurrect the leak and
the re-entrancy bug), driven via the browser `WebAssembly` API through `js-sys`,
inside the dedicated pack worker whose cancel is already `terminate()`.

**Sequencing.** ow-1..ow-5 are provable under node/`wasmi` with no browser and no
`vgms-web` -- built and committed first. ow-6 (worker host) and ow-7 (the
`vgms-web` wiring) are browser-coupled and land last, on top of a green core.

**H. ow-6's "`wasm.rs` behind the same API" cannot live in `vgms-vgmtools`.** A
wasm module cannot instantiate another wasm module on its own -- that needs the
host's `WebAssembly` API, which reaches Rust only through `js-sys`/`wasm-bindgen`.
`vgms-vgmtools` is a plain library with neither, and should stay that way. So the
split is: **`vgms-vgmtools` owns the pipeline *logic*** (order, the wholly-OPL
bypass, the ROM-size guard, the chip hold-backs, the stage report) as
target-independent code driven by an injected `Tools` runner; **`vgms-web` owns
the *runner*** -- the `js-sys` glue that instantiates the three modules in a
worker and drives their `reserve_input`/`run`/`output_*` ABI. Concretely,
`optimize_vgm(bytes, options)` stays the native convenience call (a `NativeTools`
that spawns the child processes), and a new
`optimize_vgm_with(bytes, options, &dyn Tools)` carries the same logic on wasm.
This keeps the order and every safety rule in one place rather than re-spelt in
the browser.

## Status (2026-07-31) — SUPERSEDED by the wasip1 pivot below

**ow-1..ow-7 implemented on branch `web-target`.** Not pushed, not merged.
*(The freestanding-libc build described here was replaced the next day; the
section is kept as the record of what was built and proven before the pivot.)*

- **ow-1..ow-5 -- committed, proven browser-free.** The three tool `.wasm`
  modules build import-free (36-70 KB) over `shim/wasm-libc` + `memfile.c` +
  `wasm_printf.c` + `src/wasm_libc.rs`; the node smoke runs them; the
  `wasmi`-driven parity gate is byte-identical to the native exes over valid
  VGMs (the only divergences were upstream uninitialised-read UB on
  malformed `vgm_ptch` repair fixtures, verified and out of scope); the
  ROM-size guard is in `Facts`, with tests.
- **ow-6 -- committed.** The pipeline is target-independent and takes a
  [`Tools`] runner; `optimize_vgm(bytes, options)` stays the native call over
  `NativeTools`. Verified: native workspace + wasm web build both green.
- **ow-7 -- implemented.** `vgms-web` depends on the pipeline (feature off, so
  no C and no libc collision with vgms-cores-libvgm); `optimize_tools.rs`'s
  `WebTools` drives the modules through their ABI via `js-sys` (the same
  protocol the node smoke proves), and `WebPipelineOptimizer` feeds the pack
  export. `build-web.ps1` builds and ships the three modules; `pack_worker.js`
  fetches them (best-effort -- a missing module falls back to the built-in
  pass) and hands their bytes to `vgms_web_run_pack_job`. Verified: the wasm
  web build and native `pack_zip` tests are green, `build-web.ps1` assembles a
  bundle carrying the three modules, and the regenerated wasm-bindgen glue
  matches the worker call.

  **Remaining:** the `WebTools` **browser runtime** -- a real pack export
  optimising through the modules in a page -- is the one thing this environment
  cannot exercise (no browser). It should be confirmed with the Playwright
  suite (`tools/build-web.ps1 -E2e`, then `web/e2e/`), extended to assert a
  song's optimise log shows a `vgm_cmp`/`optdac` stage line rather than only
  the built-in pass.

---

## The wasip1 pivot (2026-08-01)

A post-implementation review asked whether the freestanding-libc build was
really the best shape, "particularly around the libc shims" -- and the honest
answer was no. Key 1's premise ("the entire I/O surface is eight functions,
about a hundred lines of C") had under-counted: the tools also need the
allocator family, `str*`, `sprintf`, a float-capable `printf` and `fgets`, and
the freestanding route ended up at ~1000 bespoke lines -- a hand `FILE*` layer,
a hand printf, and a verbatim copy of vgms-cores-libvgm's unsafe allocator --
plus per-tool archive isolation and a feature gate whose only job was keeping
two `#[no_mangle] malloc`s out of one link.

**Conventional wisdom for unmodified POSIX C CLI tools is WASI**, and research
confirmed every piece is standard and maintained: wasi-sdk documents the
stock-clang route (sysroot + compiler-rt builtins as separate release
artifacts), `@bjorn3/browser_wasi_shim` is the de-facto browser host
(in-memory `PreopenDirectory`, `start()` returns the exit code), and
`wasmi_wasi` 1.1 pairs with wasmi 1.1 for a browser-free parity harness. A
spike proved it end-to-end in minutes: Scoop clang 22 + the wasi-sdk-33
sysroot compiled `vgm_cmp.c` unmodified, and node ran it **byte-identical
with the native exe on the first try** -- real printf output included.

**What replaced what (commits `ow-8`..`ow-10`):**

- `tools/build-wasi-tools.ps1` compiles the three tools to `wasm32-wasip1`
  command modules in `target/wasi-tools/` (sysroot + builtins downloaded once
  and cached under `target/wasi`, the CJK-font pattern; `-nodefaultlibs`
  keeps the clang install untouched). The link carries the promised
  `--max-memory=256M` ceiling, `--stack-first`, and a 1 MiB stack. ~280-320 KB
  per module against 36-70 KB freestanding -- wasi-libc's weight, irrelevant
  at these sizes.
- **Deleted entirely:** `shim/wasm-libc/`, `memfile.c`, `wasm_printf.c`,
  `src/wasm_libc.rs`, the `wasm_tool!` macro, the three `[[example]]`
  cdylibs, the `tool-modules` feature. Only `zlib.h`/`zshim.c` remain, riding
  into the wasip1 build unchanged. The review's "extract a shared allocator
  crate" finding dissolved -- the second copy no longer exists.
- `src/command.rs` is the one shared interpretation of a finished run (argv in,
  exit code + optional `out.vgm` + printed tail out -> `ToolOutcome`), used by
  the native binding, the `wasmi` parity test and the web worker. The tools
  run as *commands everywhere*, which is what they are.
- The web: `pack_worker.js` hosts the modules through the vendored
  `web/wasi-shim/` (`__vgms_run_tool`, the `__vgms_pick_dir` pattern; note
  `debug: false` must be explicit -- the shim enables logging when the option
  is absent). `WebTools` compiles the fetched modules and calls the hook.
  Verified under node over the exact vendored files: byte-identical with the
  native exe.
- `optimize_song_logged` (vgms-vgmtools) is the one copy of the pack
  narration; the desktop pack and the web optimizer are thin calls. Extracting
  it surfaced a real drift: the web copy returned re-written plain bytes for
  an unimproved song where native keeps the original spelling.
- The watchdog (ow-10): the worker heartbeats per pack entry and the page
  terminates a job silent for 3 minutes -- the web's stand-in for the native
  120 s timeout kill, restoring ow-6's kill-switch promise.

**Still open:** the browser-runtime confirmation above (unchanged). CI now
builds the wasip1 modules and runs both gates in the wasm job -- the sysroot
cached on the pinned `$wasiVersion`, then the `wasmi` byte-parity test and the
node smoke; the modules-absent skip path remains only for local runs.
