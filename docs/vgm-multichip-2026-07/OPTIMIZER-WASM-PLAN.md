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
