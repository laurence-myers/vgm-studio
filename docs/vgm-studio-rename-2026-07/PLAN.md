# Rename: VGM Studio → VGM Studio

> **Status: PLANNED (2026-07-30).** Name chosen by the user: **VGM Studio**
> (crate prefix `vgms-`, binary `vgmstudio`, display string "VGM Studio").
> **Not yet executed — no code touched.** Scope (core vs full, below) to be
> confirmed at execution time. This doc records the decision and the map so the
> rename can be run in one sitting later.

## Context

The project is called "VGM Studio" — a fossil from when it only trimmed DOSBox
Raw OPL captures. On `vgm-multichip` it became a multi-chip VGM editor/player,
where **DRO is one projection of the `VgmFile` model, not a kind** (see
[the any-chip plan](../vgm-multichip-2026-07/OPL-UNGATING-PLAN.md)). The name,
the `dro-*` crates, the `vgmstudio` binary and the `Dro*` identifiers no longer
describe the thing.

The subtlety is that `dro` means two different things in the tree, and only one
of them is stale:

- **The brand / heritage** — `dro-*` crates, `vgmstudio` binary, `VgmStudioApp`,
  `Instruction`, `vgmstudio.ini`, "VGM Studio" strings. **Rename.**
- **The DRO *file format*** — `DroDataV1`/`DroDataV2`, `io/dro.rs`,
  `song/dro_data.rs`, `DroInfoDialog` (`dialogs/dro_info.rs`), `DroCapture`
  (`vgms-synth/src/capture.rs:38`, builds a DRO v2 stream), the `.dro` extension,
  the `--dro` split flag, DRO-format docs. **Keep — DRO is a real format we
  still read and write.**

The tell is `Instruction`, whose own doc says *"One decoded instruction, from
a DRO **or a VGM** stream"* (`vgms-core/src/song/instruction.rs:91`): despite the
prefix it is the shared instruction type threaded through the whole VGM path, so
it is **brand**, not format → `Instruction`. It is also the single largest churn
item (25 files, ~200 sites).

**User decisions (fixed):**
1. **Name = VGM Studio.** Rejected: OPL Studio / DRO Studio (undersell the
   multichip reach), Chipwright / Opulence (brandable but not descriptive).
2. **Crate prefix = `vgms-`**, not `vgm-`: the permissive `core`/`synth` pair is
   publishable, and bare `vgm-core` risks a crates.io collision.
3. **Keep the DRO-format identifiers** (the boundary above). Only the brand goes.

**Open decisions (confirm at execution):**
- **Scope: core vs full.** *Core* = crates, identifiers, binary, display
  strings, config back-compat. *Full* = also the 22 historical `docs/*` snapshots
  and the `VGMSTUDIO_*` env vars.
- **`vgmstudio.ini` back-compat.** Recommended: write `vgmstudio.ini`, but keep
  *reading* `vgmstudio.ini` as a fallback for one release so existing user configs
  survive (`vgm-studio/src/config.rs:17`, `services/config.rs:26`).
- **Out-of-band (user actions), not part of any commit:** rename the GitHub repo
  (`laurence-myers/vgm-studio`, auto-redirects) and optionally the checkout
  folder (still `…\Python\vgm-studio` — a fossil from the original Python
  codebase).

## Rename map

**Crates** (`dro-` → `vgms-`; 12 dirs + `package.name` + `[workspace.dependencies]`
keys + every `dep.workspace = true`):

| Now | → | Now | → |
|---|---|---|---|
| `vgms-core` | `vgms-core` | `vgms-cores-nuked` | `vgms-cores-nuked` |
| `vgms-synth` | `vgms-synth` | `vgms-cores-gpl` | `vgms-cores-gpl` |
| `vgms-synth-worklet` | `vgms-synth-worklet` | `vgms-cores-ymfm` | `vgms-cores-ymfm` |
| `vgms-audio-native` | `vgms-audio-native` | `vgms-cores-libvgm` | `vgms-cores-libvgm` |
| `vgms-retrowave` | `vgms-retrowave` | `vgms-web` | `vgms-web` |
| `vgms-ui` | `vgms-ui` | `vgms-app` | `vgms-app` |

**Identifiers, binary, files, metadata:**

| Now | → | Where |
|---|---|---|
| `vgmstudio` (binary) + `src/bin/vgmstudio.rs` | `vgmstudio` / `vgmstudio.rs` | clap `name` `cli/mod.rs:30`, ~15 test invocations |
| `VgmStudioApp` | `VgmStudioApp` | 5 files (`vgms-ui/src/{app,lib}.rs`, `bin/vgmstudio.rs`, tests) |
| `Instruction` | `Instruction` | 25 files, ~200 sites — must **not** touch `DroData*` |
| `"VGM Studio"` | `"VGM Studio"` | window title `bin/vgmstudio.rs:86`, About `app.rs:55`, **VGM `creator:` tag `vgm/io.rs:998`**, `lib.rs:2` doc-comments, font test `theme/fonts.rs:170` |
| `vgmstudio.ini` | `vgmstudio.ini` (+ old-name read fallback) | `vgms-core/src/config.rs` (`SHIPPED_INI` :688), `vgm-studio/src/{config.rs,services/config.rs}`, ~30 doc-comment mentions |
| `VGMSTUDIO_*` env vars | `VGMSTUDIO_*` | parity/corpus harness (`tests/reference_parity.rs`, `corpus.rs:36`, `projection_corpus.rs`); update the parity-harness memory note |
| `vgmstudio-*` temp prefixes, `# vgmstudio chip index` header | `vgmstudio-*` | `corpus.rs:40,228`, `services/file.rs:350`, `reference_parity.rs:403` |
| `dt.ico` | `vgmstudio.ico` | `src/`, `build.rs:15-17`, `bin/vgmstudio.rs::load_icon` |
| `.idea/vgm-studio.iml`, `repository` URL, `README.vgm-studio.md` | `vgms.*` / new repo | `Cargo.toml:33`, `vendor/nuked-opl3/` + its `[patch]` comment |

**Untouched (DRO format):** `DroDataV1`/`V2`, `Instruction`'s DRO variants,
`io/dro.rs`, `song/dro_data.rs`, `dialogs/dro_info.rs`, `DroCapture`, `.dro`,
`--dro`, DRO-format docs.

## Steps (each independently committable, workspace green)

1. **Crates.** Rename 12 dirs; `package.name`; `[workspace.dependencies]` keys +
   every `dep.workspace = true`; `[profile.dev.package.*]` names in the root
   `Cargo.toml`. `Cargo.lock` regenerates. Build green before touching code.
2. **`Instruction` → `Instruction`.** The big mechanical one; word-boundary
   replace that leaves `DroData*`/`DroInfo*`/`DroCapture` alone. Verify with a
   grep that the only surviving `Dro` identifiers are the format keepers.
3. **`VgmStudioApp` → `VgmStudioApp`** + any other brand identifiers.
4. **Binary + CLI.** `bin/vgmstudio.rs` → `bin/vgmstudio.rs`, clap `name`, the
   `Cli::try_parse_from(["vgmstudio", …])` test invocations, `lib.rs`/`build.rs`
   doc-comments.
5. **Display strings + assets.** Window title, About text, the VGM `creator:`
   output tag (`vgm/io.rs:998`), the `theme/fonts.rs` glyph test, `dt.ico` →
   `vgmstudio.ico` (+ `build.rs`, `load_icon`).
6. **`vgmstudio.ini` → `vgmstudio.ini`** with a one-release fallback read of the
   old name (guard the *write* path to the new name only).
7. **`VGMSTUDIO_*` env vars** → `VGMSTUDIO_*` (+ temp prefixes, cache header).
   *Full scope only.*
8. **Metadata + docs.** `repository` URL, `.idea` module, `DEVELOPMENT.md`,
   `docs_src/` + regenerated `docs/readme.html`, vendored README pointer. Historical
   `docs/*-2026-07/` snapshots left dated unless *full* scope.
9. **Verify** (below).

Dependencies: 1 first (everything imports the crates). 2/3 independent after 1.
4←3. 5/6/7 independent after 1. 8 last. 9 throughout.

## Effort

~½–1 focused day; ~80% mechanical (~9 commits). Real cost is care around the
keep/rename boundary (step 2) and re-baselining snapshots (step 9), not the edits.

## Verification

- Per step: `cargo test` workspace; `cargo check --target wasm32-unknown-unknown
  -p vgms-core -p vgms-synth` (new names).
- **Boundary check after step 2:** `git grep -n '\bDro[A-Z]'` returns *only*
  `DroDataV1`/`DroDataV2`/`DroInfoDialog`/`DroCapture` and their modules.
- Snapshots re-baselined after 5 (title/About/font strings change) via
  `UPDATE_SNAPSHOTS=1`; diffs eyeballed. See the snapshot-baselines memory note.
- `.dro` load/save + `--dro` split regression stays green (format untouched).
- Grep sweep: no `vgmstudio`, `dro-`, `VGM Studio`, `VgmStudioApp`, `Instruction`
  outside `vendor/` and the DRO-format keepers.
