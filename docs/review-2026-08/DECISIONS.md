# Review remediation — decisions

Date: 2026-08-02. Status: **ANSWERED by the owner**, except the five marked
**OPEN** below. Branch: `review-2026-08`.

Companion to [PLAN.md](PLAN.md). This was a list of questions; it is now the
record of what was decided and why. Five entries changed materially once
researched — three of them reversed the recommendation this document originally
carried. Those are marked **↺ REVERSED** so nobody re-derives the old answer
from the old reasoning.

**Still open, needing one more word from the owner:** D5, D11, D21, D22, and the
D4/D11 tension noted under D4.

---

## Answered

### D1 — gunzip ceiling · **ANSWERED: 256 MiB, identical on both targets**

A single cap, same number natively and on wasm32, so the "opens the same
everywhere" property holds.

> ⚠️ **This does not close the wasm32 case on its own, and the plan reflects
> that.** The measured amplification on the `vgm::file` path is ~12 bytes of
> index per decompressed byte in the worst case (`offsets` 4 + `wait_prefix` 8
> per command, and a `0x00` byte decodes as a one-byte command). 256 MiB
> decompressed therefore still implies ~3 GB of index against wasm32's 4 GiB
> address space. The byte cap is necessary but not sufficient.
>
> **hf-3 therefore also bounds the command count in `VgmStream::parse`**, which
> is where the amplification actually lands. That is not a new decision — it is
> the second half of what the 256 MiB cap needs in order to mean anything on the
> web. If you would rather have a single lower byte cap instead (~20 MiB would
> bound the index without a second check), say so and hf-3 shrinks.

### D2 — loop-end preservation · **ANSWERED: (b), plus post-optimise validation**

Thread a loop end through `rebuild` and the region edits. For `optimize`
specifically, rather than exempting it: **validate after the pass** that the
loop length is preserved — e.g. that merging delays still yields the same total
play time. That is a stronger answer than the original recommendation, which
simply let the optimiser widen the loop and documented it.

Recorded as sw-1 plus a new sw-1b.

### D3 — the `0x64` wait override · **ANSWERED: (B), and the evidence is decisive**

Researched with references; the summary matters because it changes how much
this is worth caring about.

| Question | Finding |
|---|---|
| Which spec? | **v1.70 only.** Absent from 1.50, 1.60, 1.61, and gone again in 1.71 beta and the current wiki (1.72 beta). |
| The spec's own words | `0x64 cc nn nn : override length of 0x62/0x63` — followed by the author's bracketed note, *"Not yet implemented. Am I really sure about this?"* |
| Reserved range? | **No.** The current spec gives `0x64` no defined length, so a conforming parser cannot even skip it. |
| libvgm (the reference) | Classified **invalid**, not unknown — its own comment draws that distinction explicitly. `Cmd_invalid` sets `PLAYSTATE_END`: **playback stops.** |
| Legacy VGMPlay | No `case 0x64`, and no `case 0x60` in the length switch either, so it hits `default: VGMEnd = true`. Playback stops. |
| ymfm's vgmrender, vgm-converter | No handling. |
| Real files | **0 occurrences in 73,400 corpus files**, via a proper stream walk (not a byte grep). Two subtrees (~15.5k files) did not finish and are excluded. |

So this is a withdrawn proposal that no player implements, that the reference
players treat as fatal, and that no real file uses. Resolution **(B)**: the
engine stops honouring the override, and `command_wait` documents the
divergence.

Two consequences worth noting:
- **The match-on-value bug disappears for free.** With the engine no longer
  applying overrides, `wait_60hz`/`wait_50hz` are constants, so a literal
  `0x61 DF 02` can no longer be remapped. No separate fix needed.
- **`version.rs:100` is wrong** and should be corrected in the same step: it
  maps `OverrideWait` to v1.50, but `0x64` was v1.70-only.

**Deliberately not doing:** removing `0x64` from the length table to match
libvgm's "invalid" treatment. Decoding it costs nothing, and refusing it would
turn an openable file into a failed open for zero real benefit.

### D6 — the WASI shim gate · **ANSWERED, and the conflict is settled: OPL-only**

The owner said the fixture is OPL-only. Verified independently by decoding
`tests/e2e-pack.zip`: `01 Alpha.vgm` and `02 Beta.vgm` are both v1.51,
**YM3812-only**. The reviewer who reported YM2203 was wrong.

Consequences, all verified:
- The wholly-OPL bypass always fires, so `__vgms_run_tool` is **never called by
  any spec**. The vendored shim has **zero runtime coverage anywhere.**
- CI copies neither `web/wasi-shim/` nor the `tool_*.wasm` into the e2e dist.
- The CI smoke test uses Node's built-in `node:wasi`, **not** the vendored shim.

The owner's further instruction: **the wholly-OPL bypass should be removed**,
as part of the OPL→VGM work rather than as a test fix.

Two corrections to where the bypass lives and what it does:
- It is **not** in `vgms-web`. It is in the shared, target-independent
  `crates/vgms-vgmtools/src/pipeline.rs:212`.
- It gates on `VgmFile::is_opl`, **not** `is_opl_only`. `opl_type_of` recognises
  only YM3812 and YMF262, so a **YM3526-only file already takes the tools
  path.** (`VgmHeader::is_opl_only` accepts YM3526/Y8950 and has no production
  callers at all.)
- Removing it requires re-basing `compare_optimised`, which today asserts
  `VgmFile::optimize()` equals the OPL optimiser byte-for-byte over 3,933
  corpus files. Feeding `vgm_cmp` output in first would make that assertion
  measure the C tool instead.

### D7 — trimming the permissive crates' API · **ANSWERED: yes, trim**

This repo is the only consumer, so "no in-tree caller" is "no caller". Covers
`TrackEntry::from_song`, `render_wav_muted(_with_progress)`, `Compression`,
`VgmFile::unoptimised_chips`, and shrinking the eight-entry `wav.rs` surface.
Subsumes the original standing question (f).

### D8 — clean-room concepts · **ANSWERED: delete the whole concept**

`Regime`, `Threshold::regime`, `max_envelope`, the `shared()` const fn, the
unreachable test branch, and the stale module doc that still describes two live
regimes in the present tense.

### D9 — libvgm's licence · **ANSWERED: note it, assume GPL-2.0-or-later**

Record in `licenses/README.md` that libvgm ships no explicit licence grant and
is **assumed GPL-2.0-or-later**. Plus the two missing app-tier crates.

### D10 — `TODO.md` · **ANSWERED: delete it**

### D12 — spelling · **ANSWERED: (a)**

US in identifiers and every user-visible string; British stays in comments,
logs and docs.

### D13 — corpus variables · **↺ REVERSED · ANSWERED: one variable, the VGMRips corpus**

This document originally recommended keeping both, on the grounds that they name
different trees. The owner's reasoning wins: **a single corpus is easier for
other people to download and set up.** Keep `VGMSTUDIO_VGMRIPS_CORPUS`, repoint
the four suites that read `VGMSTUDIO_CORPUS`, and keep the loud-skip half — a
required-but-unset corpus must fail with the variable name, not `eprintln` and
return.

One consequence to handle: the two `vgms-vgmtools` suites cannot see
`vgms-app`, so they need their own small fallback rather than
`corpus::corpus_root()`.

### D14 — pack-zip builder home · **ANSWERED: (1)**

Move `PackEntry`/`PackEntryKind` down into `vgms-pack-archive`, re-export from
`vgms-ui`. The shared builder takes `Option<&dyn ImageOptimizer>`; the web
supplies a null optimizer that logs its own browser-specific line.

### D15 — `register_common_cores` host · **ANSWERED: (A)**

A new GPL-2.0-or-later `vgms-cores` crate. Signature must be
`fn register_common_cores(&mut CoreRegistry)` — `install` is a process-global
one-shot, so a comparison test needs a registry it can build without installing.

### D16 — dialog registry · **↺ REVERSED · ANSWERED: no registry; close one real hole**

The owner rejected the finding: modal dialogs prevent closing the song or pack,
so the lockstep is not a correctness risk — just make sure the *modeless*
dialogs are covered.

**Verified, and the owner is right about the classification.** Of the sixteen
dialogs, exactly two are modeless — Find Register and Goto — and both are
already safe:
- `find_reg` is **already** cleared by `close_song_dialogs`.
- `goto` holds nothing but a `String`; validation happens in the app against the
  current song, so it is genuinely song-independent.

**One hole the modality argument does not cover.** `handle_drops`
(`app.rs:1349`, called unconditionally from `update_impl` at `:500`) reads
`ctx.input(|i| i.raw.dropped_files)` — raw OS drop events that never pass
through egui's interaction layer, so a `Modal` cannot block them. Dropping a
`.vgm` while Find Loop is open swaps the song underneath it, and Apply then
writes the old song's row indices into the new one.

**Resolution: gate drops while a modal is open** (with a status line), rather
than adding dialog-closing lines. It closes the class instead of the two
instances that exist today, and dropping a file into a modal is ambiguous UX
regardless. The `LoopSearch` cancellation then becomes unnecessary, since the
song can no longer change under a running search.

st-1 is **dropped** from the plan; sw-5 shrinks to the drop gate.

### D17 — dialog footer · **ANSWERED: a real Footer widget**

One common basic footer offering Save or Close; anything richer (Find Loop's
third button) supplies its own footer implementation. **Also fix DRO Info's
label flipping** so its buttons sit where every other dialog puts them.

### D18 — `app.rs` visibility · **ANSWERED: (i) then (iii)**

`pub(super)` for the move; splitting `handle_action` follows as its own commit.

### D19 — splitting `vgms-core::pack` · **ANSWERED: (b)**

`pub mod readiness;` with no re-export. Consistent with D7 — nothing downstream
to break.

### D20 / (e) — one path per format · **REFINED · ANSWERED: shape (i-b), no fallback**

The owner's target: DRO opened by exactly one code path, VGM by exactly one,
no fallback. Research found a cheaper route to it than either option offered
here, because the original framing was wrong on one point:

**`SongData::Vgm` is not merely the legacy reader's output — it is the carrier
type for the OPL projection.** `OplProjection::to_song` returns `Song::vgm(…)`,
and the synth, analyser, waveform, pack preview and worklet all consume it. So
"delete the OPL VGM reader" and "delete `SongData::Vgm`" are separable, and only
the second is expensive (~108 call sites across 8 crates).

**Shape (i-b), adopted:** delete `vgm::io::read`, make `read_song` DRO-only, and
keep `SongData::Vgm` purely as the projection carrier — a view, not a document,
which is what `editor.rs` already documents it as. That satisfies the target
literally and leaves the projection port optional.

Supporting facts, each verified:
- `vgm::io::read` has exactly **one** production caller (`read_song`).
- `optimize::optimize` (the OPL optimiser) has **zero** production callers; it
  survives only as a differential oracle.
- The editor's VGM fallback is **already unreachable**: both readers call the
  same `VgmHeader::parse`, after which `file::read` can only additionally fail
  on a malformed GD3 — which `io::read` rejects too. It accepts a strict
  superset. The fallback exists to serve DRO. **(e) is therefore free.**
- Corpus, 16,466 files: 3,933 accepted by the old reader, **all agreeing
  byte-for-byte**; 12,533 newly openable; **zero stop opening**.

**Rejected: converting DRO into the VGM model.** There is no `vgm_to_dro` and it
cannot be written faithfully — DRO v2 carries a codemap and delay codes with no
VGM representation, v1 carries register escaping, and DRO delays are
milliseconds against VGM samples through a *stateful* carry (two identical 16 ms
delays legitimately become 706 and 705 samples). Preserving byte-exactness would
mean keeping the DRO container alongside the stream, i.e. rebuilding `Song`
under another name.

**Still required:** the acceptance widening is invisible to the corpus gate,
because newly-openable files count as success. Take the delegation with the old
gates re-imposed first, then remove each gate in its own revertable commit.

### (a) — `vgms-cores-ymfm` · **ANSWERED: delete it**

### (b) — scheduled CI for the ignored suites · **ANSWERED: no**

Instead: fixture-scale non-ignored versions of the cheap gates, the loud skip
from D13, and a `cargo test -- --ignored` pre-release checklist in
`DEVELOPMENT.md` carrying the absolute-path warning.

### (c) — dead workflows · **ANSWERED: delete both; releases deferred**

Automated releases are **wanted but deferred**. Record that in
`DEVELOPMENT.md` so the gap reads as deliberate rather than as rot.

---

## Open — needs one more word

### D4 — the vendored WASI shim · **ANSWERED (b), but with a tension to resolve**

Adopted: keep the vendor byte-identical, add a `web/wasi-host.js` owning argv,
fds and `debug: false`. The owner added: **avoid vendoring if possible.**

Two findings bear on that, and they pull in opposite directions:

1. **The case for de-vendoring is stronger than it looked.** The shim has
   *zero* runtime coverage (D6): no spec reaches it, CI does not ship it, and
   the smoke test uses `node:wasi` instead. Nothing would notice if it broke.
2. **But de-vendoring means adding npm to the shipped dependency graph**, which
   is currently **empty** — and that emptiness is the entire argument for the
   D11 answer below. Taking `@bjorn3/browser_wasi_shim` as a real dependency
   makes the bundler question live again.

**Options, pick one:**
- **(i)** Keep it vendored, add the wrapper, and *give it a test* — point
  `tools/web/vgmtools_smoke.mjs` at `web/wasi-shim/index.js` instead of
  `node:wasi`. Near-zero cost, and it removes the "nothing would notice"
  problem, which is the real defect.
- **(ii)** De-vendor to an npm dependency, accepting a package manager in the
  shipped graph and re-opening D11.

**Recommendation: (i).** The exposure is the missing test, not the vendoring.

### D5 — the parity harness · **↺ REVERSED · OPEN**

The owner proposed deleting the parity checks, reasoning that with clean-room
cores gone we now compare libvgm to itself. **The evidence contradicts the
premise**, so this is put back for a final call.

- The `THRESHOLDS` table was already cut to six rows, **all shared-lineage, on
  purpose**. Holding the emulator constant is the experimental *control*: it
  turns every remaining difference into a readout of our binding, volume model,
  balance, resampler and mixing.
- **VGMPlay does not use our cores by default.** The pinned `VGMPlay.ini` forces
  `Core = NUKE`; stock VGMPlay would use AdLibEmu for OPL and Genesis Plus GX
  for YM2612. The pinning is why the numbers mean anything.
- **Four catches after the clean-room cull (2026-07-29):** `852436c` AY8910
  writes bypassing the IO-port latch ("every AY song rendered as digital
  noise"); `c115d42` the core picker acting as a ~6 dB volume control;
  `7191e73` the C352 ~4× too loud; `24c8138` RF5C68 RAM images misaddressed.
- The C352 commit is the direct refutation: *"the ones that correlate at 1.0000
  read `lvl` 4.0000 exactly."* Perfect correlation **proved** the core identical,
  which is what made the level unambiguous.
- **39 chips still sit at unmeasured `LEVEL_UNITY`**; the 9 measured ones each
  carry an `n=12` provenance stamp from this harness. The C352 was an unmeasured
  unity row until it turned out to clip.
- Nothing else covers **multi-chip mixing** (every engine fixture declares one
  chip) or **absolute per-chip level** (`every_core_for_a_chip_agrees_on_its_level`
  bails when a chip has one core).
- Cost figures were overstated: `hound` is a non-dev dependency of `vgms-synth`
  regardless, so deleting parity does not remove it; and it is 8 ignored tests,
  not 17.

**The cache instinct was right, though:** a cache keyed on name/size/rate but
not on the config that selects the reference's cores can serve a WAV rendered
under a different reference. Worse than no cache.

**Recommendation: keep the harness, delete the stale narrative.** Fix the three
defects (sn-1/sn-2), delete the clean-room concept (D8), and correct
`PARITY-PLAN.md` and `LIBVGM-PLAN.md:226` — the latter still says "the scorecard
remains the arbiter", contradicting its own retirement header. That stale prose
is very likely why the tool reads as dead weight. **Say the word if you still
want it deleted and the plan will stage that instead.**

*(This also gates standing question (d): if parity is deleted there is no
`vgms-parity` crate to create.)*

### D11 — the web-dist manifest · **REFINED · OPEN**

The owner asked whether a separate Node project consuming the Rust crates would
be cleaner. **Investigated; the answer is no, and there is a better fix than any
of the three options originally offered.**

Why a bundler does not fit:
- Every runtime asset is named by a **Rust string constant** (`WORKER_URL`, the
  worklet processor path, `cjk-font.otf`) or a literal inside worker code. There
  is no `new Worker(new URL(…), import.meta.url)` anywhere — the Workers are
  built from Rust via `web-sys`. Vite's worker support keys off exactly that
  form, so it would see an empty module graph.
- **Asset hashing, the main production benefit, would break the app** by
  renaming files the Rust constants still point at.
- The shipped JS has **zero** npm dependencies; the only `package.json` is
  Playwright's, test-only.
- Nothing publishes `target/web-dist`, and the e2e server sets
  `Cache-Control: no-store`, so hashing and minification currently buy nothing.
- `worklet-processor.js` must stay an opaque classic script — it hand-rolls
  UTF-8 because `AudioWorkletGlobalScope` has no `TextEncoder`, and CI asserts
  the worklet wasm imports nothing.

**Proposed instead — option (D): make the manifest stop existing.**
`web/` currently means two incompatible things, "files the browser gets" and
"the test harness". Move `web/e2e/` out to `web-e2e/`, and the copy becomes
correct by construction. Then have CI **call `tools/build-web.ps1`** instead of
re-implementing it — proven feasible, since `rust.yaml:132` already runs
`shell: pwsh` on Ubuntu for the sibling script.

That CI duplication is already broken: `rust.yaml:195` hand-copies four files
under a comment reading "Same steps as tools/build-web.ps1, but Linux-native",
and omits `web/wasi-shim/`, which `pack_worker.js:18` imports at top level.

Cost: ~half a day. `build-web.ps1` embeds Windows path separators in five places
that need normalising, and wants a `-SkipWasiTools` switch.

**Free win found on the way:** `wasm-bindgen-cli` does not run `wasm-opt`, so
the app module ships at 12.7 MB unoptimised. Four lines and a binaryen
prerequisite typically takes 20–40% off.

**The trigger that would flip this:** the first real npm dependency in the
*shipped* graph. Note D4 option (ii) would be exactly that.

**Confirm option (D)** and the plan adopts it.

### D21 — Editor `VgmFile` ownership · **REFINED · OPEN**

Measured, not estimated. A `VgmFile` clone copies the command index as well as
the body — `offsets` at 4 bytes/command plus `wait_prefix` at 8 — so a 4 MiB
command-dense rip is **20 MiB and ~8 ms per clone**. An edit-then-play pays two,
and because the source is built *before* the debounce, **holding Delete pays a
full clone per keystroke** and discards all but the last.

- **(i) cache an `Arc<VgmFile>` rebuilt in `bump_revision` — recommended.** It
  is cheaper than its own precedent: for an OPL VGM, `refresh_projection`
  already runs `to_song()` unconditionally at **21.9 ms**, three times the
  6.9 ms clone. Consider making it lazy; the reason the projection is eager
  (`Editor::song` hands out a plain reference) does not apply to a by-value doc
  handle.
- **(ii) `Arc::make_mut` — worse than today.** `after_edit` only *pauses* audio,
  so the audio service holds its `Arc` for the rest of the session after the
  first Play. The strong count is permanently ≥2, so `make_mut` deep-copies on
  every edit — inside the keystroke handler rather than the debounced
  submission, making edit latency depend on invisible state.
- **(iii) dedup only** — zero risk, keeps every clone.

**Home for the shared type: `vgms-core`. ↺ This reverses the original
recommendation of `vgms_ui::DocSource`**, which was wrong — `vgms-ui` is
**GPL-2.0-or-later**, and `vgms-synth`'s public API takes `AudioSource`
regardless, so you would end up with two types again. Three of `SplitSource`'s
four methods (`rate`, `detect`, `stem_and_extension`) are `vgms-core` knowledge;
only `can_preview` is UI policy and stays in the UI.

**Confirm (i) + (iii) with the type in `vgms-core`.**

### D22 — the OPL state fold · **↺ REVERSED · OPEN**

This document recommended (C) — keep both stacks — and warned that routing the
split path to the VGM stack "silently changes the bytes users submit to
VGMRips". **That warning was wrong in the direction that matters.**

- **The prelude difference is a permutation and is inaudible.** The prelude
  carries **no delay between any write**, so the chip advances zero samples
  across it and only the final latch state is observable — exactly what
  `compare_split` pins. (The test comment claiming the generic path restores
  explicit zeros the OPL fold skips is **stale**; both emit them.)
- **The audible difference runs the other way, and the current OPL path is the
  one with the bug.** `materialise` synthesises a fresh v1.51 header and
  re-derives clocks from hard-coded constants (`OPL2 = 3_579_545`,
  `OPL3 = 14_318_180`). A rip declaring a non-canonical clock comes out of Split
  Songs **at a different pitch and tempo** in any player honouring the header,
  and loses its rate, volume modifier and loop base. The VGM stack preserves the
  source header.
- **The two paths already disagree.** `replace_stream` checks
  `self.vgm.is_some()` first, so **Crop already uses the VGM stack** for an OPL
  VGM, while `split_source` asks `snapshot()` first and gets the OPL projection.
  Split is the outlier, not a guarantee.

**Revised recommendation: route Split Songs to prefer the VGM stack — invert one
match at `app.rs:4242`.** It is a bug fix, it matches Crop, and it reaches "one
path per format".

Two things that will bite:
- `can_preview()` returns `matches!(self, Opl(_))` and gates the Preview button,
  so OPL VGMs would lose it unless that becomes `capabilities().renderable` —
  the more honest predicate anyway.
- `opl_state.rs`/`state_patch.rs` **cannot retire** while DRO editing exists:
  `append_patch`'s DRO v1 bank-switch emission has no VGM analogue.

**Byte-identity is not safely achievable**, so do not hold out for it. Sorting
the generic prelude by register address matches OPL but breaks banked chips —
HuC6280 register 0 selects the channel, likewise RF5C68 and the MultiPCM bank
port. Source order is the only generally-correct order.

**Confirm the reversal.**

---

## Settled without asking

| Question | Answer taken |
|---|---|
| Where the shared gunzip helper lives | A new `vgm/gzip.rs`, not `io.rs` — survives the later io.rs deletion |
| `hf-1`'s `bits_in > 32` clause | Documentation only; `BitReader::read` already refuses `> 32` |
| `hf-7`'s fuzzing tool | No cargo-fuzz (needs nightly; toolchain pinned stable). A table-driven hostile-payload corpus instead |
| `sw-3`'s second call site | Fix both; the step was incomplete, not undecided |
| `sw-7`'s "shared Unicode helper" | Not achievable and not needed — an unconditional collision check removes the branch |
| `Compression` | Remove the re-export *and* privatise the enum |
| `Device::port_name` | Delete accessor, field and the `with_io` parameter — an unused field fails `clippy -D warnings` |
| `st-4`'s module layout | Non-`mod.rs` (`src/app.rs` + `src/app/*.rs`); the `mod.rs` form breaks the `#[path]` mount |
| `vgms-vgmtools`'s lint block | Delete it, inherit `[lints] workspace = true` |
| `mg-7` | Rename, do not fold |
| `0x64`'s length table entry | Keep decoding it; refusing it would fail an openable file for no benefit |
