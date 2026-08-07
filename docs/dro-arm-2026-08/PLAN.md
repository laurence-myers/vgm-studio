# Name the DRO arm DRO

**Branch:** `dro-arm-2026-08` (proposed; branch after `chip-mixer-2026-08` merges —
this touches the same chip-panel seams) · **Status:** planned, not started.

## Why

The app began as a DRO editor, so "OPL" became the internal synonym for "the
loaded document". VGM support broke that synonym: an OPL *VGM* is carried by the
`Vgm` arm, not the `Opl` arm, so today `Opl` means "DRO" and only history
explains it. Every doc comment that touches the seam has to re-explain the
distinction (and one of them gets it wrong — see Findings). The controlled
dictionary ([TERMINOLOGY.md](../../TERMINOLOGY.md)) now fixes the rule this plan
enforces in code: **OPL names a chip family; DRO names a file format.** A name
says *Dro* when the code exists only for DRO documents; it says *Opl* only when
it fires for any OPL-chip document, DRO or VGM alike.

## Findings (verified in code)

- **`DocSource::Opl(Arc<Song>)` is always a DRO.** Its own comment says so:
  "An Opl document is always a DRO now"
  ([doc_source.rs:89](../../crates/vgms-core/src/doc_source.rs)). The `Vgm` arm
  carries every VGM, OPL included; `is_opl()` deliberately spans both arms for
  RetroWave routing ([doc_source.rs:52](../../crates/vgms-core/src/doc_source.rs)).
- **One enum, two names.** `vgms-synth` re-exports `DocSource` as `AudioSource`
  ([lib.rs:85](../../crates/vgms-synth/src/lib.rs)), so renaming the arm once
  renames it everywhere.
- **The rename is wire-safe.** The web codec serializes the arm as a numeric tag
  (0/1), not a name
  ([codec.rs:464-480](../../crates/vgms-web/src/codec.rs)).
- **`LoadedSong::Opl` repeats the pattern in the CLI**
  ([vgms-app/src/lib.rs:57](../../crates/vgms-app/src/lib.rs)); its comment
  already has to explain that "an OPL VGM takes the `Vgm` arm like every other
  VGM (Stage K retired the projection)".
- **The editor is half-converted.** Its field is already named `dro`, but the
  public accessors are `song()`/`has_song()`
  ([editor.rs:220-232](../../crates/vgms-ui/src/editor.rs)), and the
  `doc_source()` doc comment still claims "an OPL VGM through its projection,
  hands out its `Song`" ([editor.rs:253](../../crates/vgms-ui/src/editor.rs)) —
  stale since Stage K, and contradicted by the comment at editor.rs:213-218.
- **The synth has a parallel unprefixed API family that is really the DRO
  family:** `PlayerEngine`/`engine.rs` vs `VgmEngine`; `render_wav*` vs
  `render_vgm_wav*`; `measure_peak*` vs `measure_vgm_peak*`; `render_waveform*`
  vs `render_vgm_waveform*` ([lib.rs:43-71](../../crates/vgms-synth/src/lib.rs)).
  The unprefixed names read as the general case; they are the special case.
- **`PlayerEngine` is offline-only** (the divergence audit's critic pass):
  live playback of a DRO — native, web worklet, RetroWave — goes through
  `VgmEngine` over a projected VGM ("ou-2",
  [vgms-audio-native/src/lib.rs:203-217](../../crates/vgms-audio-native/src/lib.rs)).
  `PlayerEngine`'s surviving consumers are the DRO WAV render, peak scan,
  waveform, and the CLI `render` DRO arm. So the Stage 4 renames touch dead-end
  pipelines the follow-on programme may delete outright — do Stage 4 last, and
  cheaply. Stale comments telling the old story (`wav.rs:6-9`,
  `platform.rs:380`, `editor.rs:253`) fall to Stage 6.
- **`FrameClock`, `LoopConfig`, `LoopCount`, `Position` are shared** — both
  engines import them from `engine.rs`
  ([vgm_engine.rs:31](../../crates/vgms-synth/src/vgm_engine.rs)), so they must
  move to a neutral module before `engine.rs` can be renamed.
- **The channel-level `Muting`/`Panning` vocabulary is spoken only by DRO
  documents.** Every backend translates it away for VGMs, OPL VGMs included:
  "OPL document takes the OPL `Muting`/`Panning`, an OPL VGM the generic"
  ([retrowave.rs:36](../../crates/vgms-app/src/services/retrowave.rs)); same
  note in [vgms-audio-native/src/lib.rs:13](../../crates/vgms-audio-native/src/lib.rs).
- **Genuinely chip-scoped OPL names exist and must survive:** `OplChip` and the
  cores in `opl.rs`, `OplCoreAdapter`, `OplType`, `opl_hardware_core`,
  `chip_docs/opl.rs`, `VgmFile::is_opl`, `DocSource::is_opl`,
  `Editor::is_opl`, the RetroWave crate. These all fire for OPL VGMs too.
  `OplState` ([opl_state.rs](../../crates/vgms-core/src/opl_state.rs)) models
  the OPL register file (chip-accurate name) even though only the DRO
  `state_patch` path uses it today.

## Decisions

- **D-dro-1 — the rename rule.** *Dro* iff the path exists only for DRO
  documents; *Opl* iff it fires for any OPL-chip document. Unqualified `Song`
  retires from the core's public vocabulary — a VGM is also a song, so the word
  no longer identifies the DRO shape.
- **D-dro-2 — arm and accessor renames.** `DocSource::Opl` → `DocSource::Dro`,
  `DocSource::opl()` → `DocSource::dro()`, `LoadedSong::Opl` →
  `LoadedSong::Dro`. `is_opl()` keeps its name everywhere — it answers a chip
  question, and answers it `true` for OPL VGMs. The `AudioSource` alias stays.
- **D-dro-3 — type renames.** `Song` → `DroSong`, `SongData` → `DroSongData`.
  `StreamSnapshot` keeps its name (its doc already says DRO; it is
  `song`-module-internal vocabulary). Editor accessors follow: `song()` →
  `dro_song()`, `has_song()` → `has_dro()`.
- **D-dro-4 — engine renames, after extracting the shared clock.**
  `FrameClock`/`LoopConfig`/`LoopCount`/`Position` move to a neutral
  `clock.rs`; then `engine.rs` → `dro_engine.rs`, `PlayerEngine` → `DroEngine`,
  and the DRO-side free-function families gain the prefix the VGM side already
  has: `render_wav*` → `render_dro_wav*`, `measure_peak*` →
  `measure_dro_peak*`, `render_waveform*` → `render_dro_waveform*`.
- **D-dro-5 — `Muting`/`Panning`, `opl_chip_mix`, and `OplState` do not rename
  in this programme.** They are OPL-*channel*-shaped policy, so a `Dro` prefix
  would misstate their shape; the real fix is retiring them (the DRO panel
  speaking the generic `ChipMuting`/`ChipPanning`, `OplState` folding into
  `chip_state`), which belongs to the divergence-unification programme the
  DRO/OPL/VGM audit is scoping. Renaming them now would churn names the
  follow-on deletes.
- **D-dro-6 — comments are part of the rename.** Each stage corrects the stale
  doc comments in the files it touches (the `doc_source()` projection claim
  above is the known one); no stage leaves a comment describing the old name.
- **D-dro-7 — snapshot discipline.** Stages 1–5 are pure renames: **zero
  snapshot rewrites** (a diff means something moved that shouldn't have). Any
  deliberate user-visible string change found by the label audit is quarantined
  in its own stage with its snapshot updates listed in the commit message.

## Stages

One atomic commit per stage; gates per the working agreement (`cargo fmt --all`,
then `cargo clippy --workspace --all-targets -- -D warnings` plus
wasm32-unknown-unknown clippy when `vgms-ui`/`vgms-web` are touched, then tests
for the touched crates).

### Stage 1 — the core arm

`DocSource::Opl` → `Dro`, `opl()` → `dro()`, doc comments rewritten to state
the rule (Dro = the DRO song; Vgm = any VGM, OPL included). Mechanical caller
updates: `vgms-ui` (editor.rs, tasks.rs, guards.rs, pack/state.rs, app/pack.rs),
`vgms-app` (services/task.rs, services/retrowave.rs, services/audio.rs),
`vgms-web` (codec.rs, services/audio.rs), `vgms-synth-worklet` (player.rs),
`vgms-retrowave` (player.rs). Post-check:
`rg "DocSource::Opl|\.opl\(\)" crates` returns nothing.

### Stage 2 — the CLI arm

`LoadedSong::Opl` → `LoadedSong::Dro` in `vgms-app/src/lib.rs` and the CLI
modules; banner text checked (it already prints format-appropriate detail).

### Stage 3 — the core types

`Song` → `DroSong`, `SongData` → `DroSongData`; editor accessors `song()` →
`dro_song()`, `has_song()` → `has_dro()`; caller sweep across core, ui, app,
synth, web, worklet, retrowave. This is wide but mechanical; do it as one
commit so no intermediate state has both names.

### Stage 4 — the synth DRO family

4a: extract `FrameClock`/`LoopConfig`/`LoopCount`/`Position` from `engine.rs`
into `clock.rs` (refactor acceptance pattern: identical test-name set,
normalized-line diff). 4b: `engine.rs` → `dro_engine.rs`, `PlayerEngine` →
`DroEngine`, `render_wav*` → `render_dro_wav*`, `measure_peak*` →
`measure_dro_peak*`, `render_waveform*` → `render_dro_waveform*`; `Muting`/
`Panning` stay put (D-dro-5) — re-exports keep the public surface compiling in
one hop.

### Stage 5 — the module move

`song.rs` → `dro_song.rs` (with `song/` → `dro_song/`), include-path bumps per
the known module-split mechanics. Optional if Stage 3's churn already carried
the clarity; decide at the time. Acceptance: normalized-line diff, identical
test-name set, zero snapshot rewrites.

### Stage 6 — the label and comment sweep

Consume the divergence-audit report's list of user-visible strings and
remaining comments that say "OPL" where "DRO" is meant (known candidates to
*check*, not blindly change: `APP_STATUS_NEEDS_OPL` — correct if it gates on
`is_opl`, wrong if it gates on `has_dro`). String changes that move pixels get
their snapshot updates regenerated deliberately (`UPDATE_SNAPSHOTS=1`) and
listed. `DEVELOPMENT.md` and module `//!` headers join this sweep.

### Out of scope (the follow-on unification programme)

Retiring the DRO-only vocabularies rather than renaming them: the DRO panel
speaking `ChipMuting`/`ChipPanning`, folding `OplState` into `chip_state`,
retiring `PlayerEngine` by moving the DRO offline pipelines onto the projected
VGM path (which also closes the hear-vs-export gap), collapsing the paired
render/peak/waveform entry points behind `DocSource`. The full ranked backlog
is [DIVERGENCE.md §7](DIVERGENCE.md); this plan deliberately stays a naming
programme so the follow-on can delete what it would otherwise re-label.

## Acceptance (whole programme)

- `rg -i "\bopl\b" crates --type rust` survivors are all chip-scoped (the
  D-dro-5 holdouts plus the Findings allowlist); none identify a document
  format.
- Identical test-name set before/after each stage; zero snapshot rewrites
  outside Stage 6.
- The wire codec round-trip tests pass untouched (tag bytes, not names).
