# Optimizer review — the built-in optimizer and the VGMTools optimizers

Date: 2026-08-24. Method: a two-phase agent workflow. Seven analysis agents read the
sources in parallel. Three adversarial verification agents then opened every cited file
and tried to refute each claim. Of 89 claims, 85 were confirmed against the sources and 4
were corrected. The corrections are included below and collected in §5.

Sources: `crates/vgms-core/src/{vgm/file.rs, chip_state.rs, optimize.rs}`,
`crates/vgms-vgmtools/`, `crates/vgms-synth/src/verify.rs`,
`vendor/upstream/vgmtools/{vgm_cmp.c, chip_cmp.c, vgm_sro.c, chip_srom.c, optdac.c, …}`,
`docs/optimizer-2026-08/PLAN.md`, `docs/optimizer-rework-2026-08/PLAN.md`,
`docs/vgm-multichip-2026-07/OPTIMIZER-WASM-PLAN.md`.

---

## 1. The built-in optimizer

### 1.1 Algorithm

The entry point is `VgmFile::optimize` (`crates/vgms-core/src/vgm/file.rs:660-715`). It
operates directly on the raw VGM command stream, not on the app's Song model. It runs two
phases on a clone of the file. It accepts the clone only when the body became smaller in
bytes. Otherwise the file stays byte-identical.

**Phase 1 — redundant-write removal** (`chip_state::redundant_indices`,
`chip_state.rs:412-438`). One forward scan holds a shadow-register cache: a
`BTreeMap<Cell, u16>` keyed by (chip, instance, port, address). The pass drops a register
write only when all three conditions hold:

1. The chip has a `latch_rule` (`chip_state.rs:368-393`).
2. The rule declares the address a pure latch.
3. The cached value equals the new value.

The cache is cleared at the loop point, so the loop body re-establishes its own state —
the same rule `vgm_cmp` applies. Deletion is a byte splice at command boundaries
(`stream.rs:841-850`); `delete_commands` re-patches the loop offset, loop length, and
sample total in the header (`file.rs:360-391, 781-812`).

**Phase 2 — delay merging** (`optimize::merge_stream_delays`, `optimize.rs:46-108`).
Runs of adjacent `Wait` commands are summed and re-encoded through `encode_wait`
(`optimize.rs:134-165`), a **provably byte-minimal** encoder: bulk `0x61` chunks plus an
enumerated tail of at most two single-byte waits (`0x7n`, `0x62`, `0x63`). A brute-force
test proves minimality for all waits ≤ 2000 samples (`optimize.rs:239-267`). A lone delay
is copied verbatim; a run is re-encoded only when this saves bytes. The loop point and
the short loop end are merge barriers. The total wait-sample count is conserved exactly —
a `debug_assert_eq!` checks that the total play time did not change (`file.rs:703-707`).

`VgmFile::optimize` itself takes no parameters. All behavior switches (OptimizerChoice,
`sample_roms`, `dac_runs`, `speculative`) live in the surrounding pipeline (§3).

### 1.2 Chip support

The stream decoder recognizes writes for all 42 `ChipKind`s, but phase 1 drops writes
from only **5 chips** — those with a vetted `latch_rule`. Phase 2 (delay merging) applies
to every file, for every chip.

| Chip | Rule | Notes |
|---|---|---|
| YM3812, YMF262, YM3526, Y8950 (OPL family) | Every register is a pure latch, key-on included | The `0xB0` key bit is level-sensitive; a re-write does not re-attack. OPL3's second bank is a separate port and thus a separate cell. No per-register special cases (timers, waveform select, NEW bit). Safety rests on the corpus render-parity gate `the_builtin_optimizer_never_changes_audio`, plus byte-exact OPL corpus pinning. |
| YM2413 | Every register except `0x20-0x28` | Registers `0x20-0x28` carry key-on bits; a repeat re-attacks, so repeats are kept. |
| **YM2612** | **None — deliberately disabled** | See §1.4. Falls back to `vgm_cmp`. |
| All other 36 chips | None | Default-deny policy: "Chips earn a rule by being checked, not by being present" (`chip_state.rs:361-366`). Nothing is ever dropped from an unruled chip. |

The pipeline's `built_in_covers_all` fact (`pipeline.rs:477-480`) routes a file to the
external tools when any declared chip has no rule (and when the chip list is empty).

### 1.3 DAC support

The built-in optimizer does **not** optimize DAC or PCM content. It passes all of it
through unchanged, and it processes such files safely:

- `0x8n` DAC-write-and-wait: explicitly **not** a wait for the merger — folding it would
  drop the sample (`optimize.rs:42-44`). Copied verbatim. Its implicit wait still counts
  toward the conserved play time.
- YM2612 register `0x2A` writes: never dropped (the YM2612 has no rule).
- `0x67` data blocks, `0x68` PCM-RAM writes, `0x90-0x95` DAC-stream commands, `0xE0` PCM
  seek: not `Write` commands, so phase 1 skips them; phase 2 copies their bytes verbatim.

A DAC-heavy song is not skipped. Covered-chip writes dedup and pure waits merge around
the DAC commands. DAC-run collapsing is delegated to `optdac` in the tools pipeline,
gated on `has_ym2612`. Consequence (D-orw-6): under `BuiltInOnly` no DAC cleanup happens
at all today.

Unknown-but-sized opcodes decode to `VgmCommand::Raw` and pass through. An opcode with no
defined length makes the body `Opaque`; `optimize()` then becomes a silent no-op —
never an abort, never byte loss.

### 1.4 Known gaps and issues

**Fixed or contained:**

- **YM2612 OPN commit-latch corruption** (the one real shipping bug). The per-address
  model dropped same-value `0xA0-0xA2` low-byte writes after an `0xA4-0xA6` re-latch. On
  OPN chips the low-byte write *commits* the latched pair, so it is not redundant. It
  corrupted 25/500 corpus files, audibly (peaks ~41000). Fix: the rule was disabled
  entirely (`chip_state.rs:388`); YM2612 falls back to `vgm_cmp`. Correct OPN latch
  modelling is deferred to part 3a.
- **Core non-determinism** (stage 0 of the rework): NES and Game Boy cores called C
  `rand()`; fixed with a thread-local LCG reseeded at chip reset. This is what makes the
  render gate possible at all.

**Open, by design (safety mechanisms):**

- Interactive paths are render-gated: `vgms_synth::renders_identically`
  (`verify.rs:137-157`) renders original and candidate through the real engine on **two
  threads** (one shared thread would desynchronize the thread-local RNGs — a false
  "differs" by construction), compares interleaved i16 samples **byte-exact with zero
  tolerance**, and covers the intro plus one extra loop pass (default `loop_passes` = 2),
  with a 30-minute-per-side ceiling. A difference keeps the original, never fatally.
  Caveat: with no cores installed both sides render silence and trivially "match"; the
  gate is sound only in shells that install cores (`verify.rs:132-135`).
- Measured (2026-08-24, stage 4): the gate caught **112 of 120** first-pass `vgm_cmp`
  YM2612 corruptions; 10 speculative hold-back attempts all kept the original (0
  recovered); **0 of 536** safe files were wrongly kept back — the hard invariant.
- Size results: built-in shrank 413/500 files at 19.6%; `vgm_cmp` shrank 358 at 19.7% —
  equal reduction, built-in touches more files.

**Deferred (on record):**

- Part 3a: widen built-in coverage — YM2612 OPN latch first, then SN76489 (mind the
  noise-shifter reset), then OPN2/OPM/OPNA/OPL variants. Part 3b: built-in `optdac` and
  per-chip sample-ROM trims, which would retire the external tools. Do not start without
  the owner.
- A built-in DAC stage must be inserted *before* `vgm_cmp`, not appended to the finishing
  pass (D-orw-6).
- Web verification and a web per-track action (D-orw-7). Today the web editor's
  Edit > Optimize runs `file.optimize()` ungated, and the web pack export is unverified.
- Per-stage blame bisection on a failed verification (D-orw-4); export reuse of per-track
  results; three wasm-tools items (shim `debug` default, UB fixtures, browser e2e).

**Small defects found by this review's verification pass:**

- `VgmFile::unoptimized_chips` (`file.rs:744-751`) is **dead code** — zero callers in the
  repository. Its doc comment claims an export-log role that is not wired up. The export
  log's uncovered-chip line actually comes from `passthrough_chips()`
  (`pipeline.rs:349-361, 530-539`), which is a *different* set (vgm_cmp's passthroughs).
- Stale docs: `crates/vgms-ui/src/optimize.rs:8-9` says "three chips" (the rules cover
  five); `crates/vgms-vgmtools/PROVENANCE.md:47` says "three chips";
  `docs/optimizer-2026-08/PLAN.md:27-29` cites a pre-fix line number and the pre-fix
  YM2612 rule; `lib.rs:15-16` omits `vgms-web` from the crates that link `vgms-vgmtools`.

---

## 2. The VGMTools optimizers

### 2.1 vgm_cmp — the algorithm

`vgm_cmp` (`vendor/upstream/vgmtools/vgm_cmp.c`, 1214 lines) is a multi-pass
shadow-register trace optimizer:

1. **Multi-pass fixpoint**: `CompressVGMData()` runs repeatedly until a pass no longer
   shrinks the data (`vgm_cmp.c:141-158`). Passes converge because drop decisions depend
   on forward context (look-ahead) and on whether earlier commands were dropped. The
   output is written only when the total shrank.
2. **Per-command walk**: every recognized chip-write command is decoded and dispatched to
   a per-chip handler in `chip_cmp.c` whose bool return decides keep/drop. Exceptions:
   several recognized commands bypass the handlers entirely and are always copied —
   `0xC1`/`0xC2` RF5C68/164 memory writes ("OptVgmRF works a lot better this way"),
   `0xB5`/`0xC3` MultiPCM (handlers commented out), `0xBA` K053260 (handler commented
   out), `0xC6` WonderSwan memory writes.
3. **The generic drop rule** (canonical instance `ym2413_write`, `chip_cmp.c:623-627`):
   drop when `!RegFirst[reg] && Data == RegData[reg]`; a kept write stores the value.
   All state initializes to `0xFF`, so the first write of any register is always kept.
4. **Delay pooling**: delays are never copied. All delay commands (`0x61`, `0x62`,
   `0x63`, `0x7n`) are absorbed into a pool and flushed, re-encoded, just before the next
   kept command (`VGMLib_WriteDelay`, `vgm_lib.h`). Delays merge across dropped writes;
   zero-delay runs vanish. The `0x8n` DAC+wait command is kept but rewritten: the wait
   nibble is stripped into the pool, and up to 15 samples can be OR'ed back into the
   `0x80` byte. Note: vgm_cmp's delay writer is heuristic; the built-in's `encode_wait`
   is byte-minimal — this is why the built-in always runs as the finishing pass.
5. **Loop handling**: at the loop offset `ResetAllChips()` forces a resend of everything
   after loopback, while preserving cross-loop context (RF5C68/164 channel bank, HuC6280
   channel select, C140 banking type, OKIM6258 clock registers, VSU channel registers).
   The pending delay is flushed first so pre-loop time stays before the loop point, and
   the loop offset is relocated in the header.
6. **Look-ahead**: `GetNextChipCommand` (`vgm_cmp.c:946-1214`) scans forward for the next
   command of the same chip **type** (not instance — legacy dual-chip opcodes share match
   cases, and the bit-7 chip-select is not masked out). It powers the OPN `A4/A0`
   frequency-latch pairing, SN76489 latch/data pairing, RF5C68/HuC6280 channel-select
   elision, YMF271 `0x0A` latch, and the OKIM6258 divider check.
7. **Dual chips**: legacy "cheat mode" (header clock bit 30) remaps `0x30`/`0x3F`/
   `0xA1-0xAF` aliases; modern commands use bit 7 of the first parameter byte. Two full
   shadow banks exist.
8. **Coverage**: `0x67` data blocks, `0x68`, `0xE0`, and DAC-stream `0x90/0x91/0x93/
   0x94/0x95` are always copied verbatim. Only `0x92` (set stream frequency) is deduped,
   per stream ID; `0x90` invalidates that stream's cache.
9. **Flags**: `-justtmr` strips only timer/IRQ-status writes and keeps every
   sound-relevant duplicate; `-do6258` additionally dedupes OKIM6258 pan writes.

### 2.2 vgm_cmp — is the algorithm the same for each chip?

No. One shared substrate — the shadow-register/dirty-flag dedup — underlies every
handler, but the per-chip logic differs widely. `chip_cmp.c` (2870 lines) defines **39
public handler entry points covering 37 chips** plus GG stereo and DAC Stream Control
(the count includes `c6280_write`, which is missing from the file's own prototype
block).

- **Pure generic** (plain shadow dedup, no special cases): YM2413, YMZ280B, GGStereo,
  DAC-stream frequency. Nearly generic (only a timer-register case): YM3526, Y8950,
  YMF262, YMF278B (FM side; its OPL4 PCM side, port ≥ 2, is always kept).
- **Special-cased** (everything else). The main patterns:
  - *Two-byte latch pairing with look-ahead*: OPN `A4/A0` on YM2612/2203/2608/2610;
    YMF271 `0x0A/0x09`; SN76489's latch/data protocol.
  - *Timer masking*: OPN `0x27` masked to `0xC3`, OPM `0x14` to `0x83`, OPL `0x04` to
    `0x83` — and OPL "IRQ flags clear" writes (bit 7) are unconditionally deleted, as
    are YM2608 `0x110` and YM2610 delta-T flag-control writes.
  - *Always-keep registers* (write-sensitive): YM2612 `0x2A` DAC data, SN76489 noise
    mode (resets the shifter), AY8910 envelope shape `0x0D`, ADPCM data and key-on
    registers everywhere, OKIM6295 command port, OKIM6258 start/stop and data ports,
    uPD7759 FIFO, QSound current-address registers, K054539 key-on/off, K051649
    waveform RAM, ES5503 control page, and more.
  - *Channel-select elision with look-ahead*: RF5C68/RF5C164 (`0x07`) and HuC6280
    (`0x00`), loop-aware via a sentinel.
  - *State-coupled invalidation*: SegaPCM active-channel address registers, C352
    link-loop, K007232 frequency mode, VSU/WonderSwan sweep and envelope coupling,
    QSound `0xE3` full-cache flush, YMF271 sync-group flush, GB power-off.
  - *Always-drop*: Pokey strobe/IRQ registers `0x09-0x0B`, `0x0D-0x0F`; AY8910 I/O
    ports; SCSP port-4 `0x1A-0x29` and `0x08` (SCSP is otherwise stateless and keeps
    everything); X1-010/K005289/K007232 out-of-range offsets.
- **No handler at all** (always copied through): K053260, MultiPCM (both commented out),
  PWM, GA20, ES5505, Mikey. A file made only of these comes back unchanged; the app's
  export log names them (`passthrough_chips()`).

**Bugs and disabled areas inside vgm_cmp** (all verified against the vendored source):

| Defect | Location | Effect |
|---|---|---|
| SAA1099 missing `break` | `vgm_cmp.c:537` falls into `case 0x51` | SAA1099 writes are judged by YM2413 rules; SAA `0x18/0x19` reload an envelope, so a dropped repeat is audible. The app holds SAA1099 files back from vgm_cmp. |
| YM3812 waveform-select flush doubly broken | `chip_cmp.c:1335-1343` | `for (reg = 0xFF; reg >= 0xE0; reg++)` runs once (UINT8 wrap), and the body writes `RegFirst[Register]` not `[reg]`; the intended `0xE0-0xFF` invalidation never happens. |
| YM2608 prescaler dedup inert | `chip_cmp.c:1092-1097` | Compares `PreSclCmd` but never assigns it (YM2203 does); YM2608 prescaler writes are never removed. |
| Game Boy handler disabled | `chip_cmp.c:1837` | An unconditional `return true` makes the dedup unreachable; every GB write is kept. |
| Dual-WonderSwan out-of-bounds write | `vgm_cmp.c:783` passes the register byte unmasked | For a second WonderSwan chip, bit 7 stays set and `wswan_write` indexes `RegFirst[0x84…]` past the 0x20-entry array, corrupting adjacent VSU state. First-chip writes are correct. |
| VSU `0x160` dirties wrong indices | `chip_cmp.c:2700-2706` | Dirties indices 0-5 (wave-RAM space), not the `0x100+` channel Mode registers it presumably means. |
| Instance-blind look-ahead | `vgm_cmp.c:995-1163` | `GetNextChipCommand` matches chip type, not instance; latch-pair decisions can be influenced by the other chip instance. |
| Known first-pass YM2612 corruption | measured in-repo | ~5% of files (25/500; 112/120 on the MegaDrive subtree) render differently after one vgm_cmp pass; idempotent afterwards. Caught per-file by the app's render gate; still open on the unverified export path. |

Upstream's own TODOs: "fix ResetAllChips" (`vgm_cmp.c:3`), "K053260, K054539 (for mega
size reduction)" (`chip_cmp.c:10`), commented-out K054539 key dedup ("gets modified
during playback"), commented K051649 waveform dedup, an OKIM6295 stop-removal stub, and
a `getchar()` inside the `REMOVE_NES_DPCM_0` compile flag.

### 2.3 The other VGMTools optimizers

The app builds exactly **four** tools natively (`build.rs:33-50`) and **three** for
wasm32-wasip1 (`tools/build-wasi-tools.ps1:79-83`). Each runs as its own child process /
fresh module instance — the process boundary contains chip_srom.c's leaks and hangs.

| Tool | Built? | Algorithm |
|---|---|---|
| **vgm_cmp** (+chip_cmp.c) | native + wasm | §2.1. |
| **vgm_sro** (+chip_srom.c) | native + wasm | Sample-ROM optimizer. Replays every register write through cut-down decoders (24 distinct chips, 26 write functions) to build a per-byte ROM usage mask; merges used regions separated by < 0x20 bytes; re-emits `0x67` blocks holding only used regions, preserving the declared total ROM size. Upstream warns SegaPCM is "not 100% safe". Trap: a declared ROM ≥ 0x8000_0000 spins `chip_srom.c:3268`'s power-of-two loop forever. |
| **optdac** | native + wasm | DAC-run collapser. Removes runs of ≥ 128 consecutive identical YM2612 `0x2A` writes, keeping the first write and all delays ("128 at 8 kHz ≈ 16 ms"). Written for Worms (Mega Drive). |
| **vgm_ptch** (+chip_strp.c) | native only | Not an optimizer proper: opt-in unused-chip stripping (`-Strip:` `-MinVer` `-MinHeader`), patches in place. ~10 chips have no name its parser accepts. |
| opt_oki | no | Converts X68000 OKIM6258 DMA streams into DAC-Stream drum-table commands. |
| dacopt (by ctr) | no | A different DAC optimizer: converts raw DAC writes (YM2612, HuC6280, PWM) into data blocks + stream commands with greedy repeat matching. No license header. |
| optvgm32 | no | 32X PWM to streams; upstream: "pre-alpha and not for public use". |
| optvgmrf | no | RF5C68/164 RAM writes into deduplicated data blocks. |
| optvgm + pcm_optimizer | no | Shay Green's LGPL-2.1 YM2612 PCM sharer (self-verifying). |
| vgm_ndlz / vgm_dbc / vgm_dso | no | Dual-chip splitter / data-block bit-packer / `0x93`→`0x95` DAC-stream rewrite. |
| vgm_dscmp | no | Directory size-comparison reporter; not an optimizer. |

License: the vendored tree is GPL-2.0 (Valley Bell); the wrapping crate is
GPL-2.0-or-later, linked only by the copyleft half of the workspace (vgms-ui, vgms-app,
and — omitted from the crate's own doc — vgms-web).

---

## 3. How the app runs them (context)

One shared pipeline, `optimize_vgm_with` (`pipeline.rs:225-289`), order:
**optdac → vgm_sro → vgm_cmp → built-in finishing pass** (the built-in runs
unconditionally, mostly for its byte-minimal delay encoder). Routing by
`OptimizerChoice` (default `Auto`): the tools run only when some declared chip has no
built-in rule; `BuiltInOnly` never spawns tools; `Tools` always does (an A/B control).
Settings checkboxes gate the vgm_sro and optdac stages; write dedup is not optional.

Hold-backs for the documented tool bugs: vgm_cmp denied on SAA1099; vgm_sro denied on
QSound (measured: 12 of 23 changed files play differently), K053260, and SegaPCM; a
≥ 2 GiB declared ROM is always denied (the chip_srom.c hang). `Options.speculative`
lifts the per-chip denials — never the hang guard — and only render-gated callers set
it. Every tool run gets a 120 s timeout (per tool invocation), `MSYSTEM=MSYS` (disarms
`_getch()` on exit), and output-signature validation before a result is accepted.
Unverified paths (CLI, pack export, all web paths) keep the blanket denials.

---

## 4. Direct answers

1. **Built-in algorithm**: shadow-register redundancy elimination under a per-chip
   latch-rule table, plus a provably byte-minimal delay-merge pass. Event-level, on the
   raw command stream, all-or-nothing on a clone, timing-conserving by assertion.
2. **Built-in chips**: five — YM3812, YMF262, YM3526, Y8950, YM2413. The YM2612 rule
   exists but is disabled (OPN commit-latch bug). Everything else is default-deny.
3. **Built-in DAC**: no. All DAC/PCM commands pass through verbatim, safely; `optdac`
   covers DAC runs in the tools pipeline (so `BuiltInOnly` gets no DAC cleanup).
4. **Built-in gaps**: the disabled YM2612 rule (part 3a deferred); no web render gate;
   dead `unoptimized_chips`; several stale doc comments; otherwise its known failure
   modes are contained by the render gate, the corpus parity gates, and default-deny.
5. **vgm_cmp algorithm**: multi-pass shadow-register write dedup with delay pooling,
   loop-point cache reset, and forward look-ahead for latch pairs.
6. **Same per chip?** No. One generic substrate; four handlers are pure generic; the
   rest carry chip-specific special cases (latch pairs, always-keep triggers,
   invalidation coupling); six chips have no handler at all; and several handlers are
   buggy or deliberately disabled (YM3812 flush, YM2608 prescaler, Game Boy, SAA1099
   fallthrough, dual-WonderSwan OOB).

---

## 5. Corrections produced by the verification pass

1. `VgmFile::unoptimized_chips` is dead code (zero callers); the export log's chip line
   is `passthrough_chips()` — a different set.
2. `chip_cmp.c` defines 39 handlers / 37 chips, not 38 / 36 (`c6280_write` is missing
   from its own prototype block).
3. `GetNextChipCommand` matches chip *type*, not chip *instance*.
4. Under `-justtmr`, YMF271 timer registers and OKIM6295 clock registers do **not** keep
   deduplicating (they use `JustTimerCmds`, not a forced `0x00`) — unlike OPN `0x27`,
   OPM `0x14`, OPL `0x04`, and OKIM6258 clocks.

Additional nuances surfaced: the editor's Edit > Optimize verifies at the fixed default
44 100 Hz (`VerifyOptions::default()`), while the pack path uses the configured audio
rate; the 120 s timeout is per tool invocation (~360 s worst case per file across three
stages); `optimize()` gates on *body* bytes while the pipeline stage re-checks
*whole-file* bytes; and an unreadable header sets `rom_trim_bottomless: None`, so a
hidden ≥ 2 GiB ROM on the speculative path relies on the timeout/terminate backstops.
