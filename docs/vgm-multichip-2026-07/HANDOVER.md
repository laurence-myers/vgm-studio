# HANDOVER — Any-chip VGM support (plan complete, implementation not started)

**For:** a fresh Claude Code session implementing multi-chip VGM support.
**From:** the planning session, 2026-07-20.
**Repo:** `I:\Code\Python\dro-trimmer` · **branch `rust`** (main/master is the Python original — parity oracle only, never modify `src/`).
**Status:** plan complete; **no code written.** Updated 2026-07-20 after user
follow-ups: GPL relicense approved, libvgm assumed GPL and promoted to default
porting source, generic editor moved ahead of playback (all locked in §2.1).
Confirm the remaining §2.2 recommendations with the user, then begin at mc-1
(§6), following the workflow rules in §4.

---

## 1 · The feature

Today the app only opens VGM files that declare an OPL2/OPL3 clock; everything
else fails with "No OPL2 or OPL3 data detected." The user's requirements:

1. **Minimum (required):** open a Pack containing *any* VGM — all 42 chips the
   spec covers, versions 1.00–1.72 — and edit each file's tags/metadata (GD3).
2. **Ideal:** preview (play) any track. Chip emulators **must compile to WASM**
   (the wasm32-unknown-unknown web build is a first-class target).
3. The instruction **editor view is disabled/hidden** for non-OPL VGMs until
   the generic command editor (mc-5) lands.
4. Extend the editor to other chips — **deleting instructions only**, plus
   basic playback once cores exist. OPL-specific extras (volume boost,
   panning, channel muting) explicitly need **not** be generalised.
   Originally a stretch goal; the user promoted it ahead of playback (§2.1).
5. When writing a VGM, emit the **minimum version** the content requires
   (e.g. WonderSwan forces v1.71, a YMF262-only file needs only v1.51).

Spec: <https://vgmrips.net/wiki/VGM_Specification> (§3 digests everything the
implementation needs; trust the live spec over §3 if they disagree).

This plan also subsumes two existing `TODO.md` bullets: extending the reader
for real PC-AT packs (0x67 data blocks, the "data starts at 0x60" minimal
header) and emitting a higher-version header when there is something to put in
it.

## 2 · Decisions

### 2.1 Locked by the user (do not re-litigate)

1. Metadata editing for any VGM is the bar for "done"; playback is desirable,
   editing optional.
2. Editor view hidden/disabled for non-OPL tracks until the generic command
   editor lands.
3. All playback code must work on wasm32 (the future AudioWorklet inherits it).
4. Generic editor scope is delete + basic playback only.
5. Minimum-version headers on write.
6. The project relicenses to **GPL** (approved 2026-07-20; §2.2 recommends
   GPL-2.0-or-later specifically).
7. **libvgm is assumed GPL wholesale** (2026-07-20): treat it as a compatible
   porting source in planning. Confirming each vendored file's header at port
   time is routine diligence, not a planning gate.
8. **The generic editor comes before any playback/emulation** (2026-07-20):
   it is mc-5 in §6, so any-chip trimming works with zero emulators.
9. **Non-commercial-clause code is acceptable to the user** (2026-07-20) —
   dro-trimmer is itself a non-commercial project. Caveat recorded in §7:
   a non-commercial clause is a *further restriction* under the GPL, so
   Genesis-Plus-GX-derived code cannot ship in the same binary as the GPL
   cores; it serves as behaviour reference and test oracle instead.

### 2.2 Recommended (confirm with the user before mc-1)

1. **Pure-Rust-first emulator policy.** Cores are vendored Rust (hand-ported
   like `vendor/nuked-opl3`) or wasm-clean crates. Rationale: the workspace is
   deliberately no-C-toolchain, `wasm32-unknown-unknown`, wasm-bindgen; the
   proven C++/ymfm route (h1romas4/libymfm.wasm) needs wasi-sdk + nightly +
   `wasm32-wasi` and is incompatible with wasm-bindgen — a second toolchain and
   a second wasm module for the same job. Small *freestanding* C cores can
   compile to wasm32-unknown-unknown via clang (h1romas4/rust-synth-emulation
   proved it with Nuked-OPN2), so C-via-`cc` is the documented fallback for a
   core that resists porting — but it reintroduces clang into every build.
   Assessed in depth in §7.1: per-core choice, gated on a toolchain
   proof-of-concept.
2. **License policy.** The user approved relicensing the project to GPL
   (2026-07-20), which widens the sourcing pool. Recommend **GPL-2.0-or-later**
   for the workspace's own code, *not* GPL-3-only: most retro-emulation GPL
   code is v2-flavoured, and a v3-only choice would lock out GPL-2.0-only
   sources such as rust-synth-emulation. The vendored LGPL-2.1 nuked-opl3
   folds into a GPL work cleanly. Acceptable core licenses become MIT / BSD-3
   / LGPL / GPL-2-compatible — including libvgm per §2.1. While any
   GPL-2.0-only core is included, the combined work distributes as GPL-2.0.
   The relicensing chore (workspace `license` fields, README, vendor notes)
   lands with the
   first GPL-licensed vendored core. §7 has the per-chip audit table (and the
   Genesis Plus GX linking caveat, §2.1.9).
3. **The pack retag path stays byte-exact** outside the GD3 block. Header
   version normalisation (min-version rewrite) is *opt-in* — applied when the
   app synthesises a header anyway (DRO→VGM conversion, editor save of a
   restructured file) and offered as an explicit "normalise header" action at
   pack export; never silently applied to a foreign file being retagged.
4. **Foreign VGMs are a separate type**, not forced through `Song`.
   `Song`/`DroInstruction` stay the OPL editing model; a new chip-agnostic
   `VgmFile` carries everything else. Pack tracks hold an enum of the two.
5. **Corpus-ordered core rollout** (§5, mc-8/mc-9): SN76489 + YM2413 + YM2612
   + AY8910 + Game Boy + NES first — that covers the overwhelming majority of
   VGMRips packs — then FM heavies, then wavetable/PCM exotics. "Every chip
   playable" is the end state of an incremental programme, not one milestone.

## 3 · Domain facts (spec digest, verified 2026-07-20)

### 3.1 Header

Little-endian throughout; all pointer fields are *relative to their own
position*. Absolute data start = `0x34 + data_offset` (assume 0x40 when
version < 1.50). **The header ends at the data start: any field at or past it
does not exist and reads as 0.** This rule is what the current reader gets
wrong for the "data at 0x60" packs. Header size buckets by version: 0x40
(≤1.50), 0x80 (1.51–1.60), 0xC0 (1.61–1.70), 0x100 (1.71+); pad unused space
with zeros.

Chip clock fields (offset · chip · version introduced · quirk bits):

| Off | Chip | Ver | Notes |
|-----|------|-----|-------|
| 0x0C | SN76489 | 1.00 | bit 31 = T6W28 (paired with dual bit) |
| 0x10 | YM2413 | 1.00 | |
| 0x2C | YM2612 | 1.10 | bit 31 = YM3438 |
| 0x30 | YM2151 | 1.10 | bit 31 = YM2164 |
| 0x38/0x3C | Sega PCM / its interface reg | 1.51 | |
| 0x40 | RF5C68 | 1.51 | |
| 0x44 | YM2203 | 1.51 | AY flags at 0x7A |
| 0x48 | YM2608 | 1.51 | AY flags at 0x7B |
| 0x4C | YM2610/B | 1.51 | bit 31 = 2610B |
| 0x50 | YM3812 | 1.51 | (current OPL2 path) |
| 0x54 | YM3526 | 1.51 | |
| 0x58 | Y8950 | 1.51 | |
| 0x5C | YMF262 | 1.51 | (current OPL3 path) |
| 0x60 | YMF278B | 1.51 | |
| 0x64 | YMF271 | 1.51 | |
| 0x68 | YMZ280B | 1.51 | |
| 0x6C | RF5C164 | 1.51 | |
| 0x70 | PWM | 1.51 | |
| 0x74 | AY8910 | 1.51 | type byte 0x78, flags 0x79 |
| 0x80 | Game Boy DMG | 1.61 | |
| 0x84 | NES APU | 1.61 | bit 31 = FDS add-on |
| 0x88 | MultiPCM | 1.61 | |
| 0x8C | uPD7759 | 1.61 | |
| 0x90 | OKIM6258 | 1.61 | flags byte 0x94 |
| 0x98 | OKIM6295 | 1.61 | |
| 0x9C | K051649/K052539 | 1.61 | bit 31 = K052539 (SCC+) |
| 0xA0 | K054539 | 1.61 | flags byte 0x95 |
| 0xA4 | HuC6280 | 1.61 | |
| 0xA8 | C140 | 1.61 | type byte 0x96 (C140/C219 variants) |
| 0xAC | K053260 | 1.61 | |
| 0xB0 | Pokey | 1.61 | |
| 0xB4 | QSound | 1.61 | no dual support |
| 0xB8 | SCSP | 1.71 | |
| 0xBC | **extra-header offset** | 1.70 | |
| 0xC0 | WonderSwan | 1.71 | |
| 0xC4 | VSU | 1.71 | |
| 0xC8 | SAA1099 | 1.71 | |
| 0xCC | ES5503 | 1.71 | channel count 0xD4 |
| 0xD0 | ES5505/06 | 1.71 | bit 31 = 5506; channels 0xD5 |
| 0xD6 | C352 clock divider | 1.71 | |
| 0xD8 | X1-010 | 1.71 | |
| 0xDC | C352 | 1.71 | |
| 0xE0 | GA20 | 1.71 | |
| 0xE4 | Mikey | 1.72 | |

Non-clock fields the app already knows: EOF 0x04, version 0x08 (BCD), GD3
0x14, total samples 0x18, loop offset 0x1C, loop samples 0x20, rate 0x24
(v1.01), SN76489 feedback/shift 0x28/0x2A (v1.10), SN flags 0x2B (v1.51),
volume modifier 0x7C + loop base 0x7E (v1.60), loop modifier 0x7F (v1.51).

**Extra header (v1.70+, offset field 0xBC):** `{size, chip-clock-offset,
chip-volume-offset}`; chip-clock list = `count × {chip_id, u32 clock}` for
second instances; chip-volume list = `count × {chip_id (bit 7 = paired chip),
flags (bit 0 = second instance), u16 volume (bit 15 = relative ×/0x100)}`.

**Dual chips:** bit 30 (0x4000_0000) in the clock. Second-instance routing:
SN76489 → commands 0x30/0x3F; YM-family 0x5n → 0xAn; everything else sets
bit 7 of the first operand byte (Sega PCM: high bit of the address word).
The existing OPL code already honours this for dual OPL2 (0xAA), including the
`dro2vgm` quirk of writing 0xC000_0000.

### 3.2 Command stream

| Opcode | Operands | Meaning |
|--------|----------|---------|
| 0x30 / 0x3F | 1 | second SN76489 write / second GG stereo |
| 0x31 | 1 | AY8910 stereo mask (documented v1.71) |
| 0x32–0x3E | 1 | reserved |
| 0x40 | 2 | Mikey write (v1.72; **1 operand if version < 1.72 per spec reservation rules**) |
| 0x41–0x4E | 2 | reserved (**1 operand if version < 1.60**) |
| 0x4F / 0x50 | 1 | GG stereo / SN76489 write |
| 0x51–0x5F / 0xA1–0xAF | 2 | YM-family `aa dd` writes (2413, 2612 p0/p1, 2151, 2203, 2608 p0/p1, 2610 p0/p1, 3812, 3526, Y8950, YMZ280B, YMF262 p0/p1) / dual-chip mirrors |
| 0x61 | 2 | wait u16 samples |
| 0x62 / 0x63 | 0 | wait 735 / 882 |
| 0x64 | 3 | override 0x62/0x63 lengths (rare) |
| 0x66 | 0 | end of data |
| 0x67 | 6+n | data block, see §3.3 |
| 0x68 | 11 | PCM RAM write, see §3.3 |
| 0x70–0x7F | 0 | wait n+1 |
| 0x80–0x8F | 0 | YM2612 DAC write from data bank + wait n |
| 0x90–0x95 | 4/4/5/10/1/4 | DAC stream control, see §3.4 |
| 0xA0 | 2 | AY8910 write (bit 7 of aa = second chip) |
| 0xB0–0xBF | 2 | `aa dd` writes: RF5C68, RF5C164, PWM (0xB2 packs 12-bit data), GB DMG, NES APU, MultiPCM, uPD7759, OKIM6258, OKIM6295, HuC6280, K053260, Pokey, WonderSwan, SAA1099, ES5506 8-bit, GA20 |
| 0xC0–0xC8 | 3 | 16-bit-addressed writes: Sega PCM, RF5C68 mem, RF5C164 mem, MultiPCM bank, QSound, SCSP, WonderSwan mem, VSU, X1-010 |
| 0xC9–0xCF | 3 | reserved |
| 0xD0–0xD6 | 3 | port+reg writes: YMF278B, YMF271, SCC1(K051649), K054539, C140, ES5503, ES5505/06 16-bit |
| 0xD7–0xDF | 3 | reserved |
| 0xE0 | 4 | seek in YM2612 PCM data bank |
| 0xE1 | 4 | C352 16-bit write |
| 0xE2–0xFF | 4 | reserved |

The reserved-range operand sizes make unknown commands skippable, but a
*trimmer* must still preserve them byte-exact (never drop what it can't
re-encode — the existing `VgmData` principle).

### 3.3 Data blocks (0x67 0x66 tt ssssssss …)

- 0x00–0x3F: uncompressed streams for the DAC engine (0x00 YM2612 PCM,
  0x01/0x02 RF5C68/164, 0x03 PWM, 0x04 OKIM6258, 0x05 HuC6280, 0x06 SCSP,
  0x07 NES APU DPCM, 0x08 Mikey).
- 0x40–0x7E: same, compressed (bit-packed or DPCM; sub-header with
  decompression parameters); 0x7F = decompression table block. A decompressor
  is required (implement from spec; verify against vgm_cmp output).
- 0x80–0xBF: ROM dumps `{u32 total_rom_size, u32 start_addr, data}` per chip
  (Sega PCM, Y8950/2608/2610 ADPCM, OPL4/OPX wave, YMZ280B, MultiPCM, uPD7759,
  OKIM6295, K054539, C140, K053260, QSound, ES5505/06, X1-010, C352, GA20).
- 0xC0–0xE1: RAM writes (RF5C68/164, NES, SCSP, ES5503).
- Bit 7 of `tt`'s chip association follows the dual-chip rule via bit 31 of
  the size field (second-chip block).

### 3.4 DAC stream control (0x90–0x95)

A chip-agnostic streaming engine that auto-writes bytes from a data bank to a
target chip register at a set frequency: 0x90 setup `{stream_id, chip_type,
port, cmd}`, 0x91 bind data bank `{stream_id, bank_id, step_size, step_base}`,
0x92 frequency `{stream_id, u32 hz}`, 0x93 start `{stream_id, u32 offset,
length_mode, u32 length}`, 0x94 stop, 0x95 fast-start by block index.
Implement once in the engine; it services YM2612 DAC, OKIM6258, HuC6280, etc.

### 3.5 Minimum-version computation (requirement 5)

`version = max(floor, chips, commands, features)`:

- **floor:** 1.50 (the writer always emits a data-offset field).
- **chips:** each used chip's intro version from §3.1 (T6W28 flag → 1.51;
  YM2612/YM2151 need only 1.10 but see floor).
- **commands:** 0x67 uncompressed → 1.50; compressed blocks / 0x68 / 0x90–0x95
  → 1.60; 0x31 → 1.71; 0x40 → 1.72; 0x64 → 1.50.
- **features:** loop modifier ≠ 0 → 1.51; volume modifier or loop base ≠ 0 →
  1.60; extra header present → 1.70; dual-chip via 0xAn/0x30 → fine at the
  chip's own version (dual formalised 1.51 — floor covers it).

Header size then follows the §3.1 bucket for the computed version. Downgrading
an existing file must first verify no higher-version field is non-zero and no
higher-version command appears — otherwise keep the original version.

### 3.6 What the app already gets right (keep it)

`VgmMeta` keeps the header verbatim and patches only mutable fields on write —
already chip-neutral except `put_chip_clocks` (io.rs ~line 429) which always
stamps OPL clocks. GD3 read/write, loop offset ↔ instruction-index residency,
gzip-by-magic VGZ handling, and the byte-exact round-trip discipline all carry
over unchanged. The three OPL chokepoints to open up: the closed command table
(`vgm/data.rs` `mod command` + `command_size`), the OPL-clock gate + `data
offset ≥ 0x80` check (`vgm/io.rs` `read_uncompressed`, ~lines 220–270), and
the OPL-only playback path (`dro-synth` `PlayerEngine`/`OplChip`).

## 4 · Environment & workflow rules

### 4.1 PATH prelude (required before ANY cargo/rustc call)

```powershell
$env:CARGO_HOME='E:\Apps\Dev\Scoop\persist\rustup\.cargo'
$env:RUSTUP_HOME='E:\Apps\Dev\Scoop\persist\rustup\.rustup'
$env:PATH="$env:CARGO_HOME\bin;E:\Apps\Dev\Scoop\apps\llvm\current\bin;$env:PATH"
```

### 4.2 Working rules

- **Confirm with the user before starting each numbered step in §6** — the
  established rhythm; do not batch ahead silently.
- Keep the workspace green after every step: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets` (zero warnings), `cargo fmt --all
  --check`, plus the wasm check build for dro-core/dro-synth/dro-ui.
- dro-core / dro-synth / dro-ui stay **wasm-clean** (no `std::fs`, no cpal, no
  threads). Native-only code goes in dro-audio-native / dro-trimmer.
- New vendored cores follow the `vendor/nuked-opl3` pattern: own directory,
  `[patch.crates-io]` or path dep, a `README.dro-trimmer.md` describing origin
  + license + local changes, `[profile.dev.package.*]` opt-level override if
  hot. License files must ride along; update the workspace license notes.
- Snapshot tests: regenerate with `UPDATE_SNAPSHOTS=1`, eyeball diffs; new
  palette fields go in the per-theme showcase.
- Commit style: `feat(dro-core): parse the full VGM header chip table (mc-1)`.
- Test fixtures: pull one small VGM per chip family from VGMRips packs (SMS,
  Mega Drive, Neo Geo, PC Engine, Game Boy, NES, arcade); keep them tiny and
  note pack provenance in the fixture directory README.

## 5 · The plan

### Phase A — metadata for every VGM (the required minimum)

#### mc-1 · dro-core: full header model + foreign-VGM container

- New `vgm::header::VgmHeader`: parse **all** §3.1 fields version-gated, with
  the "header ends at data start" rule; keep the raw bytes verbatim alongside
  (the `VgmMeta` pattern). Expose `chips() -> Vec<ChipUse>` where `ChipUse =
  {kind: ChipKind (42-variant enum), clock, dual: bool, variant flags}`, plus
  display names ("YM2612", "SN76489", …) for descriptions/UI.
- Parse the v1.70 extra header (second-instance clocks, per-chip volumes) into
  the model; preserved verbatim on write.
- New `vgm::VgmFile` = `{header: VgmHeader, body: VgmBody, tag: Option<Gd3Tag>}`
  with `VgmBody::Opaque(Vec<u8>)` for now (Phase B adds `Commands`). Reader:
  accept version ≥ 1.00, any chip set, any data offset; still validate magic /
  EOF / GD3 magic. Duration + loop length come from header fields 0x18/0x20.
- Writer: `[header verbatim][body verbatim][rewritten GD3]`, patching EOF +
  GD3 offset only. If the source GD3 sits *before* the data (the vgmrip-7
  shape — only possible at v1.50+), relocate it to the end and patch the data
  and loop offsets (pure offset arithmetic; body bytes are position-
  independent). Property: retagging with an unchanged tag is byte-identical.
- The OPL path (`read`/`write` on `Song`) is untouched; internally it can
  begin delegating header parsing to `VgmHeader` where convenient.
- Tests: fixture-per-version header parse (1.00, 1.10, 1.50, 1.51, 1.61,
  1.70+extra, 1.71); data-at-0x60 minimal header; GD3-before-data relocation;
  byte-exact retag round-trips; proptest over synthetic headers.

#### mc-2 · pack mode: any VGM in, tags editable, graceful preview gating

- `PackTrack.song` becomes `PackSong::Opl(Arc<Song>) | Foreign(Arc<VgmFile>) |
  Unreadable(String)`. `from_folder` tries the OPL reader first (unchanged
  behaviour for OPL files), falls back to the foreign reader; only true parse
  failures land in `Unreadable`.
- `TrackEntry::from_song` generalises: title from GD3/filename as today;
  duration/loop from `VgmHeader` for foreign tracks (vgm_stat parity —
  vgm_stat trusts these header fields too).
- Description/preset generalisation: the chips line derives from
  `VgmHeader::chips()` joined display names; `preset_for`/`highest_opl`
  keep working for OPL packs and fall back to the derived chip string
  otherwise. `unique_authors`/prefill already read GD3 — unchanged.
- Quick-edit (GD3) works for foreign tracks via the mc-1 writer, including
  rename; `gzip_on_export` honoured (VGZ = gzip, unchanged).
- Row UI: foreign tracks get the normal title/duration cells; the preview
  (play) button is hidden/disabled with tooltip "Playback for <chips> is not
  supported yet"; "unreadable" styling now only for `Unreadable`.
- `validations()` updated: foreign tracks are full citizens (counted, listed,
  exported); keep a soft note listing not-previewable chips.
- Tests: pack open with a mixed OPL + Mega Drive + corrupt folder; quick-edit
  round-trip on a foreign track; description output for a non-OPL pack.

#### mc-3 · editor gating + open-file behaviour

- `load_file` (app.rs ~line 1260): when the OPL reader rejects but the foreign
  reader succeeds, replace the raw error alert with a friendly dialog: chip
  list, version, duration, GD3 title, and "The editor supports OPL2/OPL3 songs
  only — open this file inside a Pack to edit its tags." (No hidden partial
  editor state.)
- Pack tab: activating Editor for a foreign track stays impossible — the
  per-track "open in editor" action is hidden/disabled for `Foreign`.
  (Interim state: mc-5 flips both this and the dialog to open the generic
  editor instead.)
- Tests: kittest snapshot of the info dialog; action-gating unit tests.

**Phase A alone satisfies requirement 1 + 3.** It needs no emulators and no
command parsing, and it is safe for VGMRips submission work because foreign
files are never structurally rewritten beyond the GD3 block.

### Phase B — the generic command stream

#### mc-4 · dro-core: full-spec stream parser

- `VgmBody::Commands(VgmStream)`: parse the §3.2 opcode table completely —
  every chip write, waits, 0x64 overrides, data blocks (§3.3, decompressor
  included), 0x68, DAC stream control, 0xE0 seeks, reserved ranges by
  version-aware operand size. Model = flat `Vec<u8>` + offset index like
  `VgmData` (proven cheap), with a typed `decode(i) -> VgmCommand` view;
  blocks are single commands owning their payload spans.
- Unknown-but-skippable commands are retained as `Raw` commands (preserve
  bytes; never drop). Truly malformed streams fall back to `Opaque` with a
  warning — metadata editing keeps working.
- Loop offset resolves to a command index (reuse `resolve_loop_point`
  approach); durations re-derived from waits (+ 0x80–0x8F implicit waits) and
  cross-checked against the header (warn on mismatch, trust the stream).
- Upgrade the **OPL** reader with the same machinery so the two TODO gaps
  close for OPL files too: 0x67 blocks preserved (surfaced as rows), minimal
  headers accepted. `VgmData`'s closed table dissolves into the generic parser
  with an OPL-projection to `DroInstruction`.
- Tests: opcode-table round-trip property tests (decode→encode byte-exact);
  fixtures per family incl. a YM2612+DAC-stream file and a compressed-block
  file; the existing OPL suite must pass untouched.

### Phase C — the generic editor, brought forward (no emulation needed)

#### mc-5 · delete-only command editor for foreign VGMs

Why this slot (user decision, §2.1): with mc-4's parser in hand, *trimming* —
the app's core competency — works for every chip without a single emulator.
Delete rows, watch the derived duration move, save, A/B the result in an
external player (VGMPlay / in-game). Two extra payoffs: the table is the
parser's best inspector (eyeballs on decoded real-world packs shake out mc-4
bugs before playback builds on them), and the editor-generalisation risk gets
retired early, while the code around it is still fresh. The playback-dependent
niceties (audition, waveform, seek-to-row) bolt on later in mc-7.

- **Editor generalisation:** `Editor` currently owns `Option<Song>` +
  `UndoController<Song>`. Introduce `EditorDoc { Opl(Song), Foreign(VgmFile) }`
  behind a small internal trait exposing what the shared plumbing needs:
  `len()`, delete/splice, loop-index sliding, wait prefix sums, revision.
  Selection, undo, dirty tracking, and the save-prompt flow are shared;
  OPL-only analysis (`AnalysisCache`, register/bank/channel display) stays on
  the `Opl` arm. Only `Commands`-bodied files are editable — `Opaque`
  fallbacks keep the mc-3 info dialog.
- **Table:** give the instruction table a row-provider abstraction. OPL rows
  render exactly as today; foreign rows render index / chip label (from §3.2
  routing, second instances tagged) / command summary (`YM2612 p0 0x28 ← dd`,
  `wait 735`, `data block 0x81: YM2608 ΔT ROM 128 KiB`, `DAC stream #0
  start…`) / cumulative time from the wait prefix. Operand editing is out of
  scope (locked): display + delete only.
- **Delete + undo:** a foreign `DeleteInstructions` twin splices the stream
  (`VgmStream` keeps the `VgmData` offsets-table design, so splice/offset
  rebuild carry over), slides the loop index via the generalised
  `move_loop_point_past_deletion`, re-derives the wait prefix. Deleting data
  blocks or DAC-stream commands is allowed — explicit intent — but rows that
  later commands depend on (a bank a 0x93/0x95 references, a ROM a chip
  needs) earn a status-bar warning, never a veto.
- **Save path:** the mc-1 writer grows a `Commands` arm: emit the spliced
  stream, repatch EOF / GD3 offset / total samples (wait prefix) / loop
  offset+length (slid index), all other header bytes verbatim. Invariant: a
  no-edit save stays byte-identical.
- **Dialogs:** GD3 dialog already works; the VGM metadata dialog's loop field
  works once its prefix-sum lookup goes through the shared trait.
- **Gating flip:** mc-3's info dialog and the pack row's disabled action now
  open the generic editor. Transport / waveform / position / channels panels
  stay hidden for foreign docs via a capability-flags struct on the loaded
  doc (the mc-3 gating generalised — this is also what mc-7 later toggles).
- Tests: splice/undo/loop-slide property tests mirroring the OPL suite;
  byte-exact no-edit save; post-delete header repatch fixtures per family;
  kittest snapshots of foreign rows (chip labels, block rows, warnings); the
  existing OPL editor suite untouched.

### Phase D — playback

#### mc-6 · dro-synth: multi-chip engine

- `ChipCore` trait (dro-synth, wasm-clean):
  `reset(clock, variant_flags, out_rate)`, `write(port: u8, addr: u16, data:
  u16)`, `load_rom(block_type, total_size, start, &[u8])`,
  `write_ram(offset, &[u8])`, `render(&mut [i32; 2] frames…)` at a
  core-chosen native rate, `native_rate() -> u32`.
- `VgmEngine`: built from `VgmHeader::chips()` via a registry (`ChipKind ->
  Option<Box<dyn ChipCore>>`); instantiates up to two instances per chip;
  routes §3.2 commands; owns the data banks, ROM routing, decompression, and
  the §3.4 DAC-stream scheduler (one implementation, chip-agnostic); applies
  per-chip gain = spec volume-modifier × extra-header volumes × a default
  per-chip balance table (port libvgm's table directly — §7).
- Per-chip linear resampler → i16 stereo mixer at the output rate (linear is
  what VGMPlay ships; a windowed-sinc upgrade is a later nicety). Keep the
  pull contract `render(&mut [i16]) -> usize` identical to `PlayerEngine` so
  NativeAudio / waveform / wav / capture / the future worklet drive it
  unchanged. Seek = replay writes with waits skipped (ROM loads applied once);
  fine at preview scale.
- `PlayerEngine` (DRO + OPL editor path, with muting/panning/boost) stays
  as-is; folding OPL into `VgmEngine` is a possible later unification, not in
  scope.
- `is_playable(header) -> Playability {Full, Partial(missing chips), None}`
  drives the UI gate from mc-2 (a file is previewable iff every clocked chip
  has a registered core — offer Partial playback with missing chips silent,
  clearly labelled, if the user wants it).
- Tests: `RecordingChip`-style fake cores asserting routing (dual-chip bit 7 /
  0xAn mirrors / SegaPCM address bit), DAC-stream timing against hand-computed
  schedules, mixer determinism across pull sizes (extend
  `output_is_independent_of_the_pull_size`).

#### mc-7 · wiring playback into the app

- `AudioService::load` generalises to a source enum (OPL `Arc<Song>` |
  `Arc<VgmFile>`); NativeAudio hosts either engine behind the existing rtrb
  command/position plumbing (loop/mute/pan commands no-op for foreign
  sources). Update dro-ui mocks/test_support.
- Pack preview button enabled per `is_playable`; transport inside the pack tab
  stays the existing minimal preview UX.
- Generic editor gains its playback slice (the bolt-on deferred from mc-5):
  capability flags flip transport/position/waveform on for playable foreign
  docs; seek-to-selected-row via `VgmEngine` replay.
- The worklet stubs stay stubs; everything added lives in dro-core/dro-synth
  so Step 8/9 of the port inherit it.

#### mc-8 · first cores: prove the engine end-to-end

- **SN76489** — vendor an existing Rust port: libymfm.wasm's
  `chip_sn76496.rs` (BSD-3, MAME lineage) or rust-synth-emulation's
  `sn76489.rs` (GPL-2.0, VGMPlay lineage) — pick after an accuracy A/B; a
  fresh write from the documented behaviour stays the easy fallback. Covers
  SMS/Game Gear/BBC etc., and T6W28.
- **YM2612/YM3438** — vendor `ym3438.rs` from rust-synth-emulation (GPL-2.0;
  a plain-Rust port of Nuked-OPN2, proven on wasm32-unknown-unknown — repo
  archived, so we maintain the copy like nuked-opl3). Fallback: hand-port
  Nuked-OPN2 (LGPL-2.1). Includes the DAC + 0x80–0x8F fast path and 0xE0
  seeks.
- **YM2413** — port emu2413 (MIT, single C file) to Rust.
- Acceptance: an SMS pack and a Mega Drive pack preview correctly A/B'd against
  VGMPlay; wasm build renders identical samples (hash a short render in a
  wasm-bindgen test, mirroring the c-parity idea).

### Phase E — core rollout (repeatable per-chip recipe)

#### mc-9 · waves of cores, corpus-ordered

Each core lands as its own confirmed step: vendor/port → registry entry →
fixture → A/B render hash vs a reference player → tick the §7 table.

- **Wave 1 (huge corpora):** AY8910/YM2149 (evaluate the `psg` crate first),
  Game Boy DMG, NES APU (+FDS), HuC6280, YM2151.
- **Wave 2 (FM heavies):** YM2203, YM2608, YM2610/B (Neo Geo) — port from
  libvgm's OPN family core (standalone C) with ymfm (BSD-3, C++) as the
  cross-check reference; their ADPCM sides consume the mc-6 ROM plumbing; SSG
  side reuses the AY core. Y8950/YM3526 (OPL cousins — small deltas from
  existing OPL knowledge), YMF278B (OPL4 = OPL3 + wave table).
- **Wave 3 (PCM/wavetable):** Sega PCM, RF5C68/164, PWM,
  OKIM6258/6295, MultiPCM, uPD7759, K051649/SCC+, K054539,
  K053260, C140/C219, YMZ280B, X1-010, GA20, Pokey, WonderSwan, VSU, SAA1099,
  Mikey.
- **Wave 4 (hard/rare):** QSound (DSP16 emu — heavy), SCSP (DSP), ES5503,
  ES5505/06, C352, YMF271.
- Perf guardrails: per-core `[profile.dev.package]` opt-level overrides like
  nuked-opl3's; budget check on wasm (a QSound/SCSP render must keep up with
  real-time in the worklet — measure before shipping the core).

### Phase F — writer polish

#### mc-10 · minimum-version headers + normalisation

- Implement §3.5 as `VgmHeader::minimum_version(&VgmStream)`. Apply it where
  headers are synthesised (DRO→VGM conversion keeps emitting 1.51 — already
  minimal) and behind an explicit "Normalise headers" pack-export option that
  rewrites version + header size bucket + zero-pads, refusing (with a listed
  reason) when a higher-version field/command blocks a downgrade.
- This also delivers the TODO "emit a higher-version header" bullet: a
  restructured file that *needs* 1.60/1.71 fields gets them cleanly.
- Tests: per-chip minimum table; downgrade-refusal cases; round-trip
  normalise→read→normalise idempotence.

## 6 · Step sequence (confirm with the user before each)

| Step | Scope | Landable alone? |
|------|-------|-----------------|
| mc-1 | dro-core: full header parse, `VgmFile` (opaque body), GD3 retag writer | yes |
| mc-2 | pack mode: foreign tracks first-class, preview gated, descriptions | yes |
| mc-3 | editor gating + friendly non-OPL open dialog | yes — **Phase A done: minimum requirement met** |
| mc-4 | full command-stream parser (also closes both reader TODOs for OPL) | yes |
| mc-5 | generic delete-only editor + foreign save path (no emulation) | yes — **any-chip trimming works** |
| mc-6 | `ChipCore` trait, `VgmEngine`, DAC streams, mixer | yes (no cores yet) |
| mc-7 | AudioService source enum, pack preview + editor playback wiring | yes |
| mc-8 | SN76489 + YM2612 + YM2413 cores; SMS/MD packs play | yes |
| mc-9 | core waves 1–4, one confirmed step per core | per-core |
| mc-10 | minimum-version writer + normalise-header export option | yes |

mc-10 can land any time after mc-4 (mc-5 makes it more valuable: deleting a
chip's last write lets the normalise action drop the version). The loop-points
feature (shipped) is independent through mc-5 — with one
touchpoint: lp-1 and mc-5 both generalise `move_loop_point_past_deletion`;
whichever lands second reuses the shared helper rather than forking it. If
loop points land first, mc-6's engine mirrors its `LoopConfig` semantics for
foreign playback.

## 7 · Emulator sourcing & licensing (audit before each port)

Workspace license: relicensing to GPL-2.0-or-later approved; libvgm assumed
GPL wholesale (§2.1). ✔ = compatible with the GPL'd workspace.

**libvgm is the default porting source for the long tail.** With its licensing
settled by the §2.1 assumption, its technical case wins: cores are standalone
C files (far easier to hand-port to Rust than MAME's `device_t`-entangled
C++), they are maintained *specifically for VGM playback* (they already handle
the exact register/quirk surface VGM files exercise, with per-chip fixes from
decades of vgmrips packs), several chips offer multiple selectable cores to
pick the best from, and libvgm/VGMPlay is the de-facto reference player for
VGMRips — the very thing every A/B test targets, so porting its core makes the
parity bar reachable by construction. Two adjacent wins: its per-chip volume
table and resampler design port straight into mc-6's mixer, and its VGM loader
is the best catalogue of real-world file tolerances for mc-4. MAME and ymfm
remain the alternates where their core is more accurate or a Rust port already
exists. Spot-check each vendored file's header at port time as routine
diligence.

**Genesis Plus GX** (user decision §2.1.9): the user accepts non-commercial
code in principle, but its clause is a *further restriction* the GPL forbids —
a distributed binary cannot combine GPX-derived code with the GPL cores this
plan is built on. Practical impact ≈ zero: GPX's headline asset (its
Nemesis-calibrated YM2612) is already covered by the Nuked-OPN2 lineage. Use
GPX freely as a behaviour reference and A/B test oracle; if a GPX-only core is
ever truly needed, ask upstream for a GPL grant or isolate it out-of-process.

| Chip(s) | Primary source | License | Note |
|---------|----------------|---------|------|
| YM3812/YMF262 | vendored nuked-opl3 | LGPL ✔ | already shipped |
| YM2612/YM3438 | vendor `ym3438.rs` from rust-synth-emulation (Nuked-OPN2 Rust port) | GPL-2.0 ✔ | archived repo → we maintain the copy; fallback: hand-port Nuked-OPN2 (LGPL) |
| SN76489 | vendor `chip_sn76496.rs` (libymfm.wasm, MAME lineage) or `sn76489.rs` (rust-synth-emulation, VGMPlay lineage) | BSD-3 / GPL-2.0 ✔ | A/B for accuracy; fresh write is the fallback |
| Sega PCM, PWM, OKIM6258, OKIM6295, C140/C219 | vendor Rust ports from libymfm.wasm `src/rust/sound` | BSD-3 ✔ | plain Rust, MAME lineage — verify per-file headers |
| YM2151 | Nuked-OPM (Rust port) | LGPL-2.1 ✔ | |
| YM2413 | emu2413 (Rust port) | MIT ✔ | single file; upstream MIT preferred over libvgm's bundled copy |
| YM2203/2608/2610, Y8950, YM3526, YMF278B | libvgm OPN/OPL family cores (C) | GPL ✔ | biggest porting job either way; ymfm (BSD-3, C++) is the accuracy cross-check |
| AY8910/YM2149 | `psg` crate (evaluate) or libvgm/MAME ay8910 | MIT / GPL / BSD-3 ✔ | check crate license + accuracy first |
| GB DMG, NES APU | fresh Rust (Pan Docs / NESdev) or MIT Rust emulators | MIT ✔ | many proven Rust impls to crib from; libvgm's cores as behaviour reference |
| HuC6280, RF5C68/164, MultiPCM, uPD7759, K051649, K054539, K053260, YMZ280B, X1-010, GA20, Pokey, WonderSwan, VSU, SAA1099, ES5503, ES5505/06, C352, SCSP, QSound, Mikey | **libvgm (default)**, MAME as alternate | GPL / BSD-3 ✔ | standalone C, VGM-proven; QSound/SCSP are the heavy DSPs, schedule last |

Prior art for the WASM question:
[h1romas4/libymfm.wasm](https://github.com/h1romas4/libymfm.wasm) — its
wasi-sdk + `wasm32-wasi` ymfm C++ toolchain is incompatible with our
wasm-bindgen plan (do not adopt), but its `src/rust/sound` chip ports are
plain Rust and vendorable as noted above. The archived
[h1romas4/rust-synth-emulation](https://github.com/h1romas4/rust-synth-emulation)
(GPL-2.0) carries both freestanding C cores compiled straight to
wasm32-unknown-unknown — proof the `cc` fallback works — and the Rust ports
(`ym3438.rs`, `sn76489.rs`, `segapcm.rs`, `pwm.rs`) that the GPL relicense
makes reusable (verified in the repo tree, 2026-07-20).

### 7.1 · Compiling the C cores to WASM without porting (assessed 2026-07-20)

Feasible, with conditions, via three routes:

1. **`cc` crate + clang, target wasm32-unknown-unknown** (the
   rust-synth-emulation route — the only one compatible with our wasm-bindgen
   plan). Works for *freestanding* C: no libc beyond `mem*` builtins, no
   malloc, no stdio, no libm. `ym3438.c`-class cores qualify as-is; most
   libvgm cores *almost* qualify but heap-allocate their chip state
   (`calloc`) and build init tables with libm (`pow`/`sin`) — each needs a
   small patch (caller-provided state buffers; precomputed or Rust-fed
   tables) or tiny shims. The same C source then serves native and wasm from
   one build script.
2. **Emscripten**: full libc/libc++, would even take ymfm's C++ — but emits
   its own module + JS glue, forcing a two-wasm-module architecture wired
   together in JS. Rejected for the same reason as the wasi route.
3. **wasm32-wasip1 + wasi-sdk** (libymfm.wasm route): incompatible with the
   wasm-bindgen UI module; a WASI shim would burden the worklet. Rejected.

**Does route 1 save time?** Per core, partially: it removes the
transliteration work but keeps everything else — the `ChipCore` adapter,
fixtures, A/B validation, and now also a per-core freestanding audit + patch.
Estimate 30–60% saved per core, most valuable on the big/hairy wave-3/4 cores
(QSound's DSP16, SCSP, ES5506, C352). One-time costs: clang joins the build
for every contributor and CI (it is already in the dev PATH prelude via
Scoop's LLVM, so locally cheap), an `unsafe` FFI boundary per core (worklet-
crate-style lint opt-out), and wasm-side debugging of C is worse than of
Rust.

**Recommendation: per-core choice behind a proof-of-concept gate.** Cores
that already exist as Rust ports stay Rust (zero effort beats saved effort);
small/simple cores get ported (better long-term maintenance, keeps the
pure-Rust ethos where it is cheap); the long-tail exotics may use route 1.
Before any core commits to it, land a PoC step in mc-9: compile one mid-size
libvgm core (e.g. K053260) for native + wasm32-unknown-unknown via `cc`,
adapt it to `ChipCore`, and pass the render-hash A/B on both targets. If the
PoC sours, that core falls back to porting and the policy stays pure-Rust.
An unported vendored `.c` also tracks upstream libvgm fixes trivially —
re-syncing a Rust port is a re-porting exercise.

## 8 · Where everything lives (orientation)

| Concern | File |
|---------|------|
| VGM header read/write, OPL gate, GD3 | `crates/dro-core/src/vgm/io.rs` |
| Command table, `VgmData`, `VgmMeta`, `Gd3Tag` | `crates/dro-core/src/vgm/data.rs` |
| `Song`, `SongData`, prefix sums, deletion sliding | `crates/dro-core/src/song.rs` |
| Pack data model, description/presets | `crates/dro-core/src/pack.rs` |
| Pack UI state, track rows, quick-edit | `crates/dro-ui/src/pack.rs` |
| Track quick-edit dialog | `crates/dro-ui/src/dialogs/track_edit.rs` |
| Pull engine, `OplChip` wrap, muting/panning | `crates/dro-synth/src/engine.rs`, `opl.rs` |
| cpal callback + command queue | `crates/dro-audio-native/src/lib.rs` |
| `AudioService` trait + platform services | `crates/dro-ui/src/platform.rs` |
| App shell: tabs, `load_file`, transport | `crates/dro-ui/src/app.rs` |
| Vendored core pattern | `vendor/nuked-opl3` + root `Cargo.toml` patch |
| Future work list | `TODO.md` |

Line numbers cited are as of commit `a6aef49` — re-locate by symbol if drifted.

## 9 · Sources

- VGM spec: <https://vgmrips.net/wiki/VGM_Specification> (header/commands/data
  blocks/DAC streams digested in §3, fetched 2026-07-20)
- ymfm: <https://github.com/aaronsgiles/ymfm> (BSD-3)
- libymfm.wasm: <https://github.com/h1romas4/libymfm.wasm>
- rust-synth-emulation: <https://github.com/h1romas4/rust-synth-emulation>
- libvgm (default porting source per §2.1/§7): <https://github.com/ValleyBell/libvgm>
- emu2413: <https://github.com/digital-sound-antiques/emu2413> (MIT)
- `psg` crate: <https://crates.io/crates/psg>
