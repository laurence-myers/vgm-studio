# Stage K / k-5 — session handover

**Date:** 2026-08-05. **Branch:** `render-split-2026-08` (not pushed/merged).
**Status:** workspace GREEN; six commits landed this session; one large piece
(`SongData::Vgm` variant surgery) remains.

This document is self-contained: read it top to bottom and you can continue
without the prior conversation. The canonical running record is the memory file
`stage-k-projection-retire.md` (has the same facts in denser form, plus history).

---

## 1. The goal

Retire the "OPL projection". Historically an OPL VGM (and every DRO) was carried
as a `Song` whose data could be `SongData::Vgm(VgmData)` — a VGM-flavoured `Song`
that ran on a separate `PlayerEngine`. Stage K makes **an OPL VGM just a `VgmFile`
played through the one generic `VgmEngine`**, and narrows `Song` to "a DRO
document". The end state: `SongData::Vgm`, `VgmData`, the `PlayerEngine`-era OPL
path, and `VgmFile::to_song` are gone; a `Song` is always a DRO.

Owner directives in force:
- **k-5 FULL GO (2026-08-05):** delete `SongData::Vgm`; rewire RetroWave through
  `VgmEngine`; gate hardware work with MockIo wire-byte tests.
- **Retire `optimize::optimize` (2026-08-05):** the Song-based optimiser is
  superseded by `VgmFile::optimize`; this supersedes the older mg-2b "keep it"
  directive now that `SongData::Vgm` is being deleted.

---

## 2. Done this session (all committed, all green)

| Commit | Stage | What |
|---|---|---|
| `0997dd4` | k-5.2a | `opl_hardware_core` — an OPL `ChipCore` (OplCoreAdapter + a `GatedCore` whose whole-chip stand-down is **disabled**, so a full mute gates keys at the register level; a real chip has no mix to silence). New `GatedCore::stand_down_allowed` flag. |
| `3454a5a` | k-5.2b | RetroWave pump drives `VgmEngine` over a `SharedChip = Arc<Mutex<SerialOpl3Chip>>` instead of `PlayerEngine`+`Song`. Dual-OPL2's 2nd YM3812 instance routes to the YMF262 high bank (`reg\|0x100`). `RetroWaveAudio::new(device, Arc<VgmFile>, Option<OplType>)`. Vocab routing mirrors native. **Also fixed a real `VgmEngine::is_finished()` bug**: it returned `self.finished` (set when the last *command* is read) so a trailing wait reported finished while frames were still pending → the pump cut held notes; now `self.finished && self.pending == 0`, matching `PlayerEngine`. |
| `049c524` | k-5.4a | `dro_to_vgm(&Song) -> Result<VgmFile>` assembles the VGM container itself (synthesise_header + put_chip_clocks + patch EOF/TOTAL_SAMPLES + END, then `vgm::file::read`) — no `Song::vgm` intermediate. Byte-identical to the dro2vgm golden. `opl_song_to_vgm_file` collapsed (DRO → `dro_to_vgm`); `editor.convert_to_vgm` → `dro_to_vgm(song)?; load_vgm`. |
| `0fa6e00` | k-5.4b | Web worklet `read_source` sends **every** VGM as `AudioSource::Vgm(file)` (was `to_song → AudioSource::Opl` for OPL VGMs), matching native. After a+b, **`to_song` has zero production callers.** |
| `721c134` | k-5.4c | Retired `optimize::{OptimizeOutcome, optimize, redundant_write_indices, merge_delays, flush_run, delay_at}` + `From<OptimizeOutcome>` + the crate-root re-export + every optimiser oracle test (incl. deleting `optimize_corpus.rs` and `optimize_parity.rs`). **Kept** `merge_stream_delays` + `encode_wait` (live `VgmFile::optimize` deps). |
| `5c561f3` | k-5.4d-1 | Deleted `filter_vgm` (production-dead) + `VgmStream::finish()->VgmData` (its only user) + its tests. |

`k-4b` (`a218bac`) and `k-5.3` (`b0bcb98`) landed before this session — they made
an OPL VGM travel as a `VgmFile` in the editor / native CLI. This session did the
transports (RetroWave, worklet) and started the core-type deletion.

---

## 3. Build & test

Rust/LLVM are Scoop-installed at **User** scope; a long-running agent shell does
not inherit them. Prepend this to any shell call needing cargo/rustc/clang:

```bash
export CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
export RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
export PATH="$CARGO_HOME/bin:/e/Apps/Dev/Scoop/apps/llvm/current/bin:$PATH"
```

- Per-crate is fastest: `cargo test -p vgms-core --lib`, etc.
- Whole workspace compile-check: `cargo build --workspace --tests` (~2 min cold).
- **Known flake:** `vgms-ui` kittest snapshot tests can hit a GPU
  access-violation under `--workspace`; that's the documented flake, not a
  regression. Run `vgms-ui --lib` on its own to confirm (it passed 527 this
  session).
- Acceptance gates worth re-running after each step: `vgms-app --test
  opl_ab_parity` (OPL A/B, ~1 min), `--test cli_smoke`, `--test
  song_split_parity`; `vgms-retrowave` (MockIo wire-byte tests); `vgms-synth-worklet`.

---

## 4. What remains — and the structural catch

**One large piece: delete the `SongData::Vgm` variant and the Song VGM machinery.**

The catch that shapes the ordering: **exhaustiveness coupling.** Every
`match song.data() { … SongData::Vgm(_) => … }` arm (in `song.rs`'s
len/get/raw/etc., `crop::rebuild`, `split_songs::build_piece`/`append_delay`,
`io::write_song`) is *required by the compiler while the variant exists*. So those
arms **cannot** be removed in isolation — they fall together with the variant, in
one coordinated `song.rs` surgery.

Only things that *produce/consume* `Song::vgm` without exhaustively matching
`SongData` can be deleted as independent pre-steps. `filter_vgm` (done) and
`optimize` (done) were two. The remaining independent pre-steps are (a) and (b)
below; then (d) is the unavoidable finale.

### (a) Delete `to_song` (+ its coupled `vgm::io::read`)
- `VgmFile::to_song` (`vgm/file.rs:267`) → `OplProjection::to_song`
  (`vgm/projection.rs`) → uses `VgmData::from_stream`. **Delete both `to_song`s.**
  **Keep** `OplProjection` and `project()` (still used by `is_wholly_opl`, the
  editor's row cells, and the k-3 analyser `row_vgm`).
- `vgm::io::read` (`vgm/io.rs:82`) is `to_song`'s only *production* caller and is
  itself production-dead (no prod `read_song` caller ever passes a `.vgm` name —
  verified: CLI `read_any_song_from_path`, the worklet, and the web codec all
  divert VGMs to `vgm::file::read`). Delete `vgm::io::read`.
- Migrate the two test callers of `to_song`:
  - `editor.rs:1380` (`an_opl_vgm_analyses_its_rows_from_the_stream`): the oracle
    `RegisterAnalyzer::analyze_all(&file.to_song())` → rebuild it over the
    stream via the k-3 `row_vgm`/`project()` path instead.
  - `retrowave.rs:583` (`the_hardware_service_projects_an_opl_vgm_on_the_vgm_arm`):
    `assert!(file.to_song().is_some())` → `assert!(file.is_opl())`.

### (b) Delete `vgm::io::{read, write, write_gzipped}` + surgery on their tests
- **CORRECTION to the old plan:** the `vgm::io` **module stays**. `MAGIC`,
  `GD3_MAGIC`, `parse_gd3_tag`, `write_gd3_tag`, `is_gzipped`, `synthesise_header`,
  `put_chip_clocks`, `CONVERSION_VERSION`, `MINIMUM_SUPPORTED_VERSION` are used
  pervasively in production (`vgm/file.rs`, `vgm/header.rs`, `vgm/audit.rs`,
  `convert.rs`, `split_songs.rs`, pack-archive). Only the Song-based `read` /
  `write` / `write_gzipped` go.
- `read` and `write` are **coupled** in tests (the ~50-test module in `vgm/io.rs`
  does `read` → edit → `write` round-trips, loop-point resolution, GD3 whole-file
  round-trips). Delete those tests; **keep** the unit tests of the surviving
  helpers (`gd3_round_trips`, `gd3_*`, `a_synthesised_header_matches_the_fixtures_shape`,
  `opl_type_selects_the_chip_clocks`, `a_synthesised_header…`, `put_chip_clocks`
  errors). This is surgery, not wholesale deletion of the test module.
- Loop-marker/GD3 *behaviour* coverage isn't lost: `vgm/file.rs` carries the
  parallel byte-parity, loop-index, delete-command and GD3 tests for the VgmFile
  path (the live one).

### (c) `read_song` DRO-only; `write_song` VGM-arm → error
- `io/mod.rs`: `read_song` drops its `.vgm/.vgz → vgm::io::read` branch (VGM names
  become "unsupported", which no production caller hits). `write_song`'s
  `SongData::Vgm(_)` arm must stay *as an arm* until the variant is gone — make it
  `SongData::Vgm(_) => Err(…)` (or `unreachable!`), then it disappears in (d).
- Migrate `io/mod.rs`'s own VGM round-trip tests onto `vgm::file::read/write`
  (`write_song_round_trips_every_format`, `write_song_compresses_a_vgz_by_name`).

### (d) The `song.rs` finale — delete the variant + all coupled arms + machinery
Delete in one coordinated pass (compiler-driven):
- `SongData::Vgm` variant + its 7 delegating match arms in `song.rs`
  (`len`/`is_empty`/`get`/`raw`/`raw_instruction`/`delete_many`/`insert_many`) +
  `delays_in_samples` (becomes const false → remove).
- `Song.vgm` field, `Song::vgm` ctor, `is_vgm`, `vgm_meta`, `vgm_meta_mut`,
  `total_delay_samples`, `samples_before`, `delay_samples_prefix`,
  `loop_num_samples`, `move_loop_markers_past_deletion`, the samples branch of
  `rebuild_delay_prefix`, `StreamSnapshot.loop_point`/`loop_end`, the VGM handling
  in `capture_stream`/`replace_data`, and fold `with_vgm_meta` back into `new`.
- `crop::rebuild` `SongData::Vgm` arm + `remap_loop`; `split_songs::build_piece`
  and `append_delay` `SongData::Vgm` arms (keep the DRO arms — they're live).
- `undo::DeleteInstructions` loop-marker fields + capture/restore + the
  `if !song.is_vgm()` guard (→ always the DRO path).
- `VgmData` (`vgm/data.rs`) + its ~15-test module + `VgmData::from_stream`.
- `write_song`'s now-removable `SongData::Vgm` arm; `read_song` already DRO-only.
- **Keep:** `Instruction::DelaySamples` (still produced by `project()` for VGM row
  display), `project()`, `OplProjection` (minus `to_song`),
  `slide_index_past_deletion` (used by `VgmFile` + UI markers),
  `merge_stream_delays`, `encode_wait`.
- **Test fixtures to migrate → raw bytes / `VgmFile`** (they build `Song::vgm`):
  `test_song.rs::{redundant_vgm_song, multi_song_capture, looping_vgm}` (and their
  `app_gui_tests.rs` / `dialogs` consumers — many, e.g. find_loop tests take
  `Arc<Song>` so may need a `VgmFile`→`Song` shim removed at the callsite),
  `undo.rs::optimizable_vgm` + `replace_stream_restores_loop_markers`,
  `state_patch.rs` `vgm_of` tests (append_patch is format-agnostic → port to DRO),
  `pack-archive/lib.rs:627`, `song/dro_data.rs:586` (a test match arm),
  `wasm_roundtrip.rs` VGM tests.

---

## 5. Traps & corrections learned this session (don't re-discover)

- **`Instruction::DelaySamples` STAYS.** The old plan said "the only VGM-only
  variant, delete it" — wrong. `project()` (kept) decodes VGM waits into
  `DelaySamples` for row display, so the variant + all its helpers + the three
  `song.rs` display arms are load-bearing.
- **`vgm::io` mostly survives** (see (b)). Only `read`/`write`/`write_gzipped` go.
- **`to_song` was still production-live on the web worklet** until k-5.4b — not
  just the legacy `vgm::io::read` wrapper. The old plan missed this.
- **`write_song`'s VGM arm + `vgm::io::write` were load-bearing** via the DRO→VGM
  conversion (`opl_song_to_vgm_file`, `convert_to_vgm`) until k-5.4a reshaped
  `dro_to_vgm` to build a `VgmFile` directly. Don't delete them before the
  reshape (done) is in.
- **`VgmFile::body.raw()` includes the trailing `0x66` END marker**; the old
  `VgmData::raw()` did not. Adjust byte-exact test expectations accordingly.
- **`VgmFile::set_loop_rows(start, end)` re-resolves the end to a command
  boundary** (a delay must have time before it), so it does not necessarily equal
  the raw row you passed; assert against `file.loop_end_index()`, not the input.
- **`RetroWave hardware muting`**: a whole-chip mute must gate keys at the register
  level (`opl_hardware_core`'s no-stand-down gate) because the chip *is* the audio;
  the engine's `Voice::silenced` (which only zeroes the discarded mix) is not
  enough. Pinned by `player.rs` MockIo tests.
- **Coverage gap (deliberate, noted):** render-parity of the *live*
  `VgmFile::optimize` on the corpus is now untested — the retired
  `optimize_corpus.rs`/`optimize_parity.rs` exercised the *Song* optimiser. Its
  unit coverage in `vgm/file.rs` remains. Consider a follow-up.

---

## 6. Pointers

- Memory: `stage-k-projection-retire.md` (running record; densest source of truth).
- Plan: `docs/render-split-2026-08/PLAN.md` §"Stage 4" (= Stage K = k-1..k-6);
  `docs/review-2026-08/PLAN.md` §12b + the mg-* table (mg-2b, superseded).
- The deletion-surface recon that drove Stages c–d was a 6-agent workflow; its
  full classified map (every `SongData::Vgm`/`VgmData`/`to_song`/`vgm::io`/
  `DelaySamples` site, production vs test, with a fate) is summarised in the
  memory file. Re-run a similar recon if you distrust any "production-dead" call.
- Related memory: `render-split-pseudo-mute-plan`, `audio-service-seam`,
  `retrowave-hardware-plan`, `volume-balance-model`.
