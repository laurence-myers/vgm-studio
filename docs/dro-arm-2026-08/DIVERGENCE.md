# DRO vs OPL vs VGM — the complete divergence audit

**Audited:** 2026-08-07, working tree of branch `chip-mixer-2026-08` (after the
chip-panel and Find Register unifications). Method: eight parallel subsystem
readers over the whole workspace, findings adversarially checked by a
completeness critic; 148 findings + 9 critic corrections/gaps, condensed here.
Companion documents: [PLAN.md](PLAN.md) (the naming refactor),
[TERMINOLOGY.md](../../TERMINOLOGY.md) (the controlled vocabulary).

**Status update (post-audit).** Backlog item [§7.1](#7-the-unification-backlog-ranked)
(retire `PlayerEngine`) is now implemented — see branch `dro-engine-2026-08`
([PLAN](../dro-engine-2026-08/PLAN.md)). The offline `DroEngine` path described
below (the renamed `PlayerEngine`) no longer exists: every document renders and
scans through `VgmEngine` over its projection, so live and offline share one
engine. The snapshot prose is kept as audited; read it with that item ticked.

## The three axes

Every divergence in the tree falls on one of three axes, and conflating them
is exactly the legacy confusion:

- **dro-format** — gated on the *document being a DRO* (`DocSource::Opl`,
  `Editor::song()`, `LoadedSong::Opl`). Does **not** fire for an OPL VGM.
- **opl-chip** — gated on the *chips being OPL-family*. Fires for a DRO
  **and** for a VGM whose stream is wholly OPL (`is_opl()`).
- **vgm-only** — exists only for VGM documents (usually because the DRO
  format has no such data; sometimes just because nobody built the DRO half).

Each item below is marked **inherent** (forced by the file format) or
**incidental** (historical seam that could be unified).

## The architecture in one paragraph

There are two parallel document models joined only at a thin enum:
a DRO is a `Song` (ms-based, byte-exact v1/v2 encodings, an `OplType` baked
in); a VGM — OPL or not — is a `VgmFile` (sample-based, header + stream +
GD3). `DocSource` unifies them for background jobs but exposes only six
operations; everything else matches on the arm. **Live playback is already
unified**: every document — DRO included — plays through `VgmEngine`
(the DRO is projected to a VGM at load, "ou-2") in the native service, the
web worklet, and the RetroWave pump. The old OPL `PlayerEngine` survives
**offline only**: DRO WAV render, DRO peak scan, DRO waveform, and the CLI
`render` DRO arm. Editing, undo, analysis, and most dialogs run as parallel
per-arm implementations behind mostly-unified UI.

*(An earlier reading of the synth crate mistook `PlayerEngine` for the live
DRO path; the critic pass corrected it against `vgms-audio-native`
[lib.rs:203-217](../../crates/vgms-audio-native/src/lib.rs) — stale comments
in [wav.rs:6-9](../../crates/vgms-synth/src/wav.rs),
[platform.rs:380](../../crates/vgms-ui/src/platform.rs) and
[editor.rs:253](../../crates/vgms-ui/src/editor.rs) still tell the old
story.)*

---

## 1. What a DRO user has that a VGM user lacks (dro-format)

| Difference | Status |
|---|---|
| **DRO Info dialog** (Ctrl+I): version, hardware type, ms length; view-only unless the `ui.dro_info_edit_enabled` settings latch is on; saves undoably and stays open. The only dialog with an edit gate, the only header dialog with a shortcut. | incidental UX asymmetry ([dro_info.rs](../../crates/vgms-ui/src/dialogs/dro_info.rs), [guards.rs:6-17](../../crates/vgms-ui/src/app/guards.rs)) |
| **Convert submenu**: DRO→VGM always, DRO v2→v1 for v2 files. Conversion is strictly one-way — no VGM→DRO exists, even for a wholly-OPL VGM. | one-way is incidental ([convert.rs](../../crates/vgms-core/src/convert.rs)) |
| **Load-time checks**: bogus leading delay silently auto-trimmed (non-undoable, alerted); header-vs-stream ms mismatch raises a modal with per-version advice. A VGM load is "always all-clear". | inherent checks, divergent UX (see §3 header row) |
| **Unconditional capability**: a DRO is always playable, renderable, muteable, and always offered the Song split format; it can never be open-but-silent. | inherent (built-in OPL core) |
| **Dual-OPL2 default pan image**: hard-L/R (SB Pro) seeded by the widget layer for DROs only; a VGM declaring dual YM3812 gets it from the *engine* (bit-31 clock convention) instead. | incidental split of one behaviour across two layers ([chip_panels.rs:148-180](../../crates/vgms-ui/src/widgets/chip_panels.rs), [vgm_engine.rs:1273-1334](../../crates/vgms-synth/src/vgm_engine.rs)) |
| **Live chip-type change**: editing the DRO's hardware type rebuilds the deck; a VGM's chip set is fixed by its header. | inherent |
| **DRO v1 waveform-select prime** (0x01=0x20 before playback) — the only document-format branch in the whole synth crate. | inherent ([engine.rs:766-776](../../crates/vgms-synth/src/engine.rs)) |

## 2. What a VGM user has that a DRO user lacks (vgm-only)

Mostly inherent — the DRO format has no header fields for any of it:

- **GD3 tag editing** (11 fields), **VGM Metadata** (loop start/end, loop
  base/modifier, volume modifier with Measure), **Fix Header** (one-click
  audit-and-repair), **Apply Loop to Metadata**. A DRO user is told to
  convert first.
- **Optimize** — VGM-only in GUI *and* CLI (a DRO is refused: "Only VGMs can
  be optimized"). *Incidental*: redundant-write stripping is not conceptually
  VGM-specific; the OPL latch model exists, and `OplState::is_set` — the
  DRO-side hook — is dead code with a stale doc claiming the optimiser uses
  it ([opl_state.rs:10-14,79](../../crates/vgms-core/src/opl_state.rs)).
- **Pack mode** — pack folders scan only `vgm/vgz/png/txt`; `.dro` files are
  silently invisible, with no warning and no offered conversion
  ([file.rs:20-21](../../crates/vgms-app/src/services/file.rs)).
- **Header volume modifier honoured at load**; a DRO always starts at unity.
- **Unwalkable-VGM courtesy**: a rich explainer dialog with an
  open-as-pack escape hatch; a broken DRO gets the generic unreadable alert.
- **VGZ** compression handling, **playability reporting** (Full/Partial/None
  with named silent chips), the richer region-edit feedback (`RegionReport`
  counting unmodelled commands — the DRO crop reports nothing comparable).
- **Loop storage**: both formats can *search* for loops and audition them,
  only a VGM can *store* one. (The Find Loop "Apply" hint that explains this
  is attached with `on_hover_text` to a disabled button, so the DRO user it
  addresses likely never sees it — [find_loop.rs:548-553](../../crates/vgms-ui/src/dialogs/find_loop.rs);
  same pattern in [split_songs.rs:215-227](../../crates/vgms-ui/src/dialogs/split_songs.rs).)

## 3. One feature, two implementations (shared-but-divergent)

The structural residue: a DRO lives as a decoded `Song` with its own undo
stack, so every operation exists twice even when the UI is unified.

| Feature | DRO path | VGM path (incl. OPL VGMs) | Divergence that matters |
|---|---|---|---|
| Crop / delete region | `crop.rs` over `Song` + `OplState`/`StateFold` | `VgmFile::crop_to_region` + `ChipState` | Same OPL music gets different restore engines by container; state-emission *order* philosophies disagree — the generic path deliberately preserves causal order for OPL's NEW/banking modes, the OPL-specific path emits ascending ([chip_state.rs:13-16](../../crates/vgms-core/src/chip_state.rs) vs [opl_state.rs:88-97](../../crates/vgms-core/src/opl_state.rs)) |
| Undo | `DeleteInstructions`/`ReplaceStream`/`UpdateHeader` | `DeleteCommands`/`ReplaceVgm` | Two stacks in the editor; user-visible label differs for the same action: "Undo Delete **Instruction(s)**" vs "Undo Delete **Command(s)**" |
| Loop search | `find_loops(&Song)` — exact packed-u32 keys | `find_loops_in_stream` — FNV-1a hashed keys | Different correctness guarantee: the VGM side can (vanishingly rarely) surface a hash-collision candidate |
| Song-split | `materialise()` (OplState prelude, DRO encodings, stale doc still claims a VGM arm) | `materialise_vgm()` → `extract_region` | Output keeps its container (.dro / .vgm) — unlike Split Channels, where a DRO's stems come out as VGMs |
| Split Channels | DRO: panel vocab translated + projected to canonical-clock VGM | VGM: own bytes, header clocks verbatim | Unified on one splitter ("ou-4"); the stale comment at [split.rs:104-105](../../crates/vgms-ui/src/app/split.rs) still claims OPL VGMs have a projection |
| WAV render / peak / waveform | `render_wav*`, `measure_peak*`, `render_waveform*` over `PlayerEngine` | `render_vgm_*` / `measure_vgm_*` + `ResampleMode` + core choices | **The hear-vs-export gap** — see below |
| Find Register | `FindQuery::Dro(FindTarget)`, low-byte match, no instance scoping | `FindQuery::Vgm(VgmFindTarget)`, port- and instance-scoped | One dialog (0c9bf7b), two target grammars; a dual-OPL2 DRO cannot scope chip #1 vs #2 while the same music as a VGM can |
| Register analysis | `RegisterAnalyzer` (DRO + OPL VGM via projection) | `ChipAnalyzer` (mixed VGMs) | Changed-bits wording written twice, kept in sync only by a test; OPL register docs stated twice (regdata vs chip_docs), drift-pinned by test |
| Header info | `Song::pretty_string` ("OPL Type: …") | header accessors, UI assembles ("Chips: …") | CLI banner formats the same concepts two ways |
| Info-vs-stream mismatch | load-time modal + manual chore behind a settings latch | on-demand Fix Header, one confirm | Same problem, warning-and-chore vs one-click fix |
| Mute/pan plumbing | OPL `Muting`/`Panning` vocabulary | generic `ChipMuting`/`ChipPanning` | Every audio service, the worklet ABI, and the wire codec carry **both** vocabularies and route by document; a past switcher bug (dropped generic pair, all non-OPL mutes dead, both ends green) proves the hazard ([services/retrowave.rs](../../crates/vgms-app/src/services/retrowave.rs), test `the_switching_service_forwards_the_any_chip_controls`) |
| Format detection | GUI: content-sniff, VGM reader first | CLI + worklet: extension only | The same misnamed file lands in different document models depending on the shell |
| Seek (web) | time-based for a DRO | row-exact for a VGM | worklet-only wrinkle |

### The hear-vs-export gap (the critic's headline find)

Live playback of a DRO is the projected `VgmEngine` sound: resampled by the
user's Sinc/Linear choice, VGMPlay-style cross-chip balance, dual-OPL2
hard-panned. But the DRO's *offline* pipelines still use `PlayerEngine`:

- CLI `render` of a dual-OPL2 DRO exports **centred** audio the user never
  hears, ignores the resampling setting, and skips the missing-core check —
  while the VGM arm's own comment promises "an export sounds like what the
  user hears" ([cli/render.rs:73-99](../../crates/vgms-app/src/cli/render.rs)).
- The GUI's DRO peak scan (Match Volume) and waveform display measure that
  same differently-mixed signal.
- The DRO render honours only the per-render core override, never the
  process-wide Settings core; DRO peak/waveform honour no override at all.

An OPL VGM of the same music has none of these gaps. This is the strongest
argument for retiring `PlayerEngine` entirely (follow-on programme).

## 4. Genuinely chip-gated behaviour (fires for OPL VGMs too)

These are *correctly* OPL-scoped — the audit confirms each fires for a VGM
carrying OPL chips, not just DROs:

- **RetroWave routing**: `is_opl()` documents (DRO or OPL VGM) go to the
  board; non-OPL VGMs are refused (CLI) or silently rerouted to the emulator
  (GUI switcher). On hardware: meters/boost/trims/pans inert, and the
  DRO-vs-OPL-VGM difference *inside* the gate is only which mute/pan
  vocabulary the pump translates. OPL2 silicon fix-ups (NEW bit, speaker
  bits) key on the file's chip type, both containers alike.
  The projection/refusal logic is duplicated near-verbatim between
  [cli/play.rs:59-72](../../crates/vgms-app/src/cli/play.rs) and
  [services/retrowave.rs:89-112](../../crates/vgms-app/src/services/retrowave.rs).
- **Instruction table**: Bank column + OPL register descriptions for any
  document with an OPL reading; Chip column otherwise.
- **Split dialog**: OPL documents always offered Song format and the
  percussion option.
- **The `opl3` config slot** shared by the whole OPL family (only slot that
  can name hardware); the settings Frequency/Buffer rows grey under it.
- **Optimiser redundancy rules**: OPL family has full rules; routing
  (built-in vs vgmtools) keys on the chip set.
- **Pack**: any OPL track flips the PC-pack metadata prefill; curated
  marketing hardware strings exist only for OPL (console rips get raw chip
  lists even where a preset exists — incidental).
- **OPL gate-coverage edge**: `ChannelGate` covers YM3812/YM3526/YMF262 but
  **not Y8950**, so a Y8950 VGM builds un-muteable channels while the same
  registers in a DRO mute fine ([channel_gate.rs:695-700](../../crates/vgms-synth/src/channel_gate.rs)).
- **Two OPL definitions disagree**: `is_opl_only()` (header claim) counts
  YM3526/Y8950; `opl_type_of` (the feature gate) does not — a Y8950-only VGM
  answers `is_opl_only() == true`, `is_opl() == false`
  ([header.rs:668-680](../../crates/vgms-core/src/vgm/header.rs)).

## 5. Naming and documentation debt (the refactor plan's targets)

- `DocSource::Opl`, `LoadedSong::Opl`, `Editor::song()/has_song()`,
  `SongFileType::Vgm` (a vestigial variant no `Song` can be, kept alive as a
  UI tag) — [PLAN.md](PLAN.md) stages 1–3.
- The unprefixed synth family (`PlayerEngine`, `render_wav`, `measure_peak`,
  `render_waveform`, `Muting`, `Panning`) — stages 4–5 / follow-on.
- Strings that frame VGM as the exception and DRO as "the song":
  "This needs an OPL song" (gates a *DRO*-only feature), "no song is
  loaded", the stale resampling hover ("How non-OPL chips are resampled" —
  wrong twice over post-ou-2). Undo labels "Instruction(s)"/"Command(s)".
- Stale comments telling the pre-ou-2 story: `editor.rs:253` (projection),
  `wav.rs:6-9` (PlayerEngine "for an OPL VGM"), `platform.rs:380`,
  `split.rs:104-105`, `settings.rs:144-155`, `abi.rs` OPL-setter docs,
  `DocSource::opl()`'s RetroWave claim, `convert.rs:162`'s impossible error,
  `materialise()`'s VGM arm, `opl_state.rs`'s optimiser claim.
- Dead code: `OplState::is_set`; `initial_channel_pans(_vgm)` and
  `RegisterUsage` (exported, zero consumers — orphaned by the chip-panel
  unification).
- `PlayerEngine` still carries a loaded trap: a `DelaySamples` arm feeding a
  ms-unit clock (44.1× mistiming if ever reached)
  ([engine.rs:761-762,950-957](../../crates/vgms-synth/src/engine.rs)).
- **The shipped user documentation is still the DRO-Trimmer manual**:
  `docs/readme.html` ("DRO Trimmer v5 r1") and `docs_src/Home.wiki` document
  `drotrim.exe`, `drotrim.ini`, retired flags and the old Find Register; no
  VGM editing, packs, tags, loops, chip mixer, or RetroWave appear. The
  single largest DRO-era artefact in the tree.

## 6. What is already unified (the rule, not the residue)

For balance — the audit confirms these are format-neutral today: delete,
crop/delete-region entry points, goto, selection, markers, loop audition,
transport, waveform display, boost stepper (both formats priced on the VGM
modifier ladder), Render to WAV dialog, Split dialogs, Find Loop dialog,
Find Register dialog chrome, the chip deck (dual OPL2 = two YM3812 cells —
the old single-chip behaviour is gone), trims (generic for both), pack
volume scan (the historic OPL-only leak is fixed and regression-pinned),
open/save pickers and drag-drop, web/native task parity, and
`slide_index_past_deletion` as a genuinely shared primitive.

## 7. The unification backlog (ranked)

1. ✅ **Done — Retire `PlayerEngine`**: DRO WAV/peak/waveform/CLI-render moved
   onto the projected VGM pipeline (branch `dro-engine-2026-08`). `DroEngine`
   (the renamed `PlayerEngine`) and the `render_dro_*`/`measure_dro_peak*`
   family are deleted; this killed the hear-vs-export gap, the core-choice
   asymmetries, and the loaded `DelaySamples` trap.
2. **One mixer vocabulary**: DRO panel speaks `ChipMuting`/`ChipPanning`
   natively; delete `opl_chip_mix` round-trips, the double-send in every
   service, and the worklet's dual ABI.
3. **One state-restore engine**: fold `OplState`/`StateFold` into
   `ChipState` with per-format emitters (also resolves the emission-order
   disagreement and revives a DRO optimise path).
4. **Merge the undo command pairs** behind the two models (or at least the
   labels).
5. **Header dialog parity**: same edit-gating, undoability, shortcut story
   for DRO Info and VGM Metadata.
6. **Format detection**: pick content-sniffing everywhere.
7. **Rewrite the user docs** as VGM Studio.
8. Small fixes: `on_disabled_hover_text` for the two disabled-button hints;
   Y8950 gate coverage; `is_opl_only` vs `opl_type_of` reconciliation;
   dedupe the RetroWave projection/refusal logic; delete dead analysis
   helpers; the pack preview's unreachable Opl arm.
