# Gap audit: VGMPlay reference against our engine

Audit date: 2026-08-12. Updated: 2026-08-12, after the fix branch.
Updated again the same day: the OKIM6258 regression is closed, and the M7 fix landed.
Method: a 13-agent workflow. Six agents examined six areas. Six verifier agents tried to refute each finding. One critic agent searched for areas without examination.
Reference: VGMPlay 0.52 at `E:\Code\Cpp\vgmplay-libvgm`, with the pinned configuration `docs/vgm-multichip-2026-07/parity/VGMPlay.ini`.
The two libvgm pins are almost equal. Our pin is one commit newer. That commit does not change the emulation cores.

Result of the audit: 38 confirmed findings, 31 gaps after merge of duplicates.
Final state (2026-08-13, branch `vgmplay-parity-fixes-2026-08`): **29 gaps fixed, 2 gaps planned (H5 and the T6W28 half of M3), 0 gaps open, 1 regression found and fixed**. The full-roster sweep after the last batch moved no scorecard row. The fixed items are in the two records below.

Note: this report uses ASD-STE100 style. Chip names, file names, and register names are technical names.

---

## Fixed on branch `vgmplay-parity-fixes-2026-08` (2026-08-12)

Each fix has one commit, with format, lint, and test gates. The measured
results come from the reference-parity harness (n=12, or n=9 for the OKIM6258).

| Gap | Fix | Commit | Measured result |
|---|---|---|---|
| H1: DAC streams had no per-chip command translation | Full port of `dac_control.c`: PWM 12-bit pairs, SN76496 frequency/volume forms, OKIM6295 start/stop, RF5C68 and HuC6280 channel select (with an engine-side register shadow), QSound 16-bit; command-counted rates; all five length modes; the reverse bit; the prestep; `0x95` block offsets; the loop-wrap bank guard (also closes L5, L6) | `de9ec93` | Part of the YM2612 gain below. One regression: see the OKIM6258 note |
| H2: C219 ROM data got no byte swap | Pair swap before `load_rom`, as the reference's player-side patch | `749eb31` | — |
| H3: Game Gear stereo bytes went onto the PSG bus | Address 1 is now the stereo port on Nuked-PSG; each side renders through the die's own mute mask | `39e86f6` | — |
| H4: DRO v2 OPL3 detection was missing | Init-block scan promotes a mislabelled DualOPL2 file to OPL3, playback-only, so saves stay byte-exact | `9e9d309` | — |
| H6: Game Boy legacy mode was not set | `OPT_GB_DMG_LEGACY_MODE` in the default option bits | `56aa513` | — |
| M1: Y8950 ADPCM-B was fully absent | The Y8950 is served from libvgm's MAME fmopl (the reference's own core), promoted for this one chip | `9b9f097` | **1.0000** (was 0.8287) — closed |
| M2: chips below 44100 Hz did not use the reference's rate mode | Non-FM devices start in `DEVRI_SRMODE_HIGHEST` at 44100; the ten FM chips stay native | `b69a740` | WonderSwan **1.0000** (was 0.9888) — closed |
| L1: our YM2413 DC filter (the reference has none) | Filter removed; the raw rotation sum goes out | `8f18f48` | **0.9896** (was 0.9767) — most of the old "unexplained" gap |
| L2: YM2612 write timing differed from the reference | Every half-write now sits 15 cycles after the last, as `2612intf.c` paces it | `47ac036` | **0.9922** (was 0.904 before the branch, 0.9565 after H1) — at the shared ideal |
| L3: OKIM6258 12-bit option was not set | `OPT_MSM6258_FORCE_12BIT` in the default option bits | `56aa513` | — |
| L5: seven DAC-stream flag details | Fixed inside the H1 port | `de9ec93` | — |
| L6: loop wraps added data blocks again | Fixed inside the H1 port | `de9ec93` | — |
| M7: SN76489 header noise parameters | The engine's default core selection now reads the header settings (`core_for_file`). A file with non-Sega noise parameters gets the libvgm Maxim core, which maps all three fields. An explicit user selection stays the winner | `94fb401` | The corpus SN76489 sample has only Sega-default headers, so the row does not move (0.358). A unit test pins the selection |

The parity bars moved with the measurements (`4ce29f9`, `f90d268`): the
YM2612, Y8950, and WonderSwan now take the shared 0.99 bar; the YM2413 bar
rose to 0.98. The old YM2612 theory — that the gap lived in the reference
player's driver — is disproven. Both layers of that gap were ours.

## Planned, not implemented

The owner redirected these to plan documents; the designs are agreed there.

- **H5. DRO v1 opcode tests** (`0x01`/`0x04` disambiguation). A verbatim port misreads our own tool-written v1 files; the corrected design is in `GAP-H5-DRO-V1-PLAN.md`.
- **M3. T6W28 linking**. The header-aware selection seam now exists (the M7 fix, `94fb401`), but the T6W28 also needs a cross-instance device link: the second chip's config must carry the first chip's live device pointer. The design is in `GAP-SN76489-CLUSTER-PLAN.md`.

## New issue found by the fixes — now closed

**OKIM6258 regression: 0.9766 before the branch, 0.9327 after H1. Closed at
1.0000 (n=9).** Two separate causes were found, and neither was in the H1
port itself.

**Cause 1: the stream delivery order (`0e7d779`).** The H1 port's timing was
correct, but our mixer serviced the DAC streams *before* each frame's render.
The reference updates them *after* the render, so a write that falls due at
sample n reaches the chip at sample n+1. On the X68000 files the stream
supplies bytes at almost exactly the chip's ADPCM usage rate (a difference
near 128 ppm). The one-frame shift then moves the moments when the chip's
8-byte data buffer becomes empty. At each such moment the chip plays one byte
again, which changes the ADPCM decoder state — and all audio after it is
different. The old stream engine had no prestep, which cancelled the order
error by accident; that is why the bisect showed the old code as "correct".
The fix makes the mixer render the frame first, then service the streams.
Measured: the 0.69 file (Knight Arms 02) and the two other regressed files
read 1.0000; the row returned to 0.9767.

**Cause 2: the "−15-cent pitch offset" was not pitch (`d0160cb`).** It
pre-dated the branch: the pre-H1 code measures the same offset. The harness's
native-rate probe reset the core without the header chip settings, so every
OKIM6258 file probed at the default-divider rate (7813 Hz). The divider-512
files (true rate 15625 Hz) then rendered through both sides' resamplers — the
comparison the native-rate probe exists to avoid. Our accurate resampler has
a group delay near 15 samples, and the cents metric fits each window with no
lag alignment, so a constant lag of D samples reads as approximately D cents
at this render rate. With the probe configured as the engine is, eight of the
nine files read 1.0000 with cents 0 — the "offset" on Nobunaga 01 included.

**Bar restored and raised (`d0160cb`):** 0.99 / 2.0 cents on the measured
1.0000 median (the old bar was 0.95). The full-roster sweep passes with no
other row moved. One residual stays, recorded in the row's known-gap note:
Syvalion 01 reads 0.72 with our accurate default resampler. That file changes
the clock divider two times during the song and the chip never stops; the two
sides' resamplers align the rate changes differently, which moves the
buffer-empty moments and re-seeds the ADPCM decoder. With
`VGMSTUDIO_PARITY_RESAMPLER=linear` (the reference's own conversion shape,
see L14) the file reads 1.0000, so the emulation is correct and the gap is
the resampler comparison, not the core. The row's median absorbs the file.

---

## The second batch: the 16 remaining gaps, all fixed (2026-08-13)

Eleven more commits on the same branch closed every open issue. The
full-roster sweep afterwards moved no row: every at-ideal chip still reads at
or above 0.998, the fixed rows hold, and the known-gap rows are unchanged.

| Gap | Fix | Commit |
|---|---|---|
| M4: QSound old-clock key-on aid | Per-channel start-address and pitch caches on the binding; the cached address is injected on a pitch rising from zero and on phase writes, keyed on the same clock-under-5-MHz condition the clock rescue uses | `82a2dfe` |
| M5: extra-header second-chip clock | Instance 1 resets at its extra-header clock (and bit-31 variant), as `GetChipClock` resolves it | `28bdf3b` |
| M6: NES OPT_TRI_NULL | The pinned 0x3B7 option value replaces libvgm's 0x1B7 | `badd272` |
| M8: linked devices in the estimate | The estimate follows OPN SSG (0x80) and OPL4 FM (0x100) links; solo files stay at unity because the anchor includes the link too | `f5cf27a` |
| M9: the six tail chips | Their declared clocks (offsets 0xE8-0xFC) weight the estimate with the reference's volume x PB values; playback stays a roster gap | `f5cf27a` |
| M10: DRO OPL3 reset order | Conversions open with 0x105 = the scanned enable, then 0x104 = 0; a v1 OPL3 capture primes 1 | `990226d` |
| L4: 9-16 bit unpack order | Values assemble low-chunk-first as `READ_BITS` does; a hand-derived byte test pins it independently of the packer | `fbc4630` |
| L7: v1.00/1.01 FM clock | The reader scans the first FM command and reassigns the shared clock (`ParseFileForFMClocks`, over our decoded stream) | `ea36c73` |
| L8: version-gated clock reads | The gate is gone; the physical data-offset bound stays, and the mismatch is still logged | `ea36c73` |
| L9: loop base and modifier | `LoopConfig::for_vgm` scales a finite count by `GetModifiedLoopCount`'s formula, exactly when the region is the file's own loop | `f6ce31e` |
| L10: YM2612 alternate rows | The bit-31 variant reaches the GPGX/Gens rows as the YM3438 mode bit through `SetOptionBits` | `94d9018` |
| L11: bit-31 pan without the dual bit | The pan keys on the variant alone, per the reference; mono arcade twins stay centred | `e9979d1` |
| L12: paired volume overrides | Honoured in the estimate (the parent's id with bit 7, absolute or relative) | `f5cf27a` |
| L13: AY PCM3CH option | Set on the standalone AY and pushed to the OPN-linked SSG | `badd272` |
| L14: downsample shape | Linear mode's decimation is the reference's fractional box average (`Resmpl_Exec_LinearDown`); upsampling keeps the 2-tap lerp, which is the reference's `LinearUp` | `91ac833` |
| L15: DRO tolerance | A bad v2 pair plays as nothing (the file loads); versionless v0 reads through the mask detect and the shifted layout; a single OPL2's high-bank writes are dropped | `fa122f6` |

Two deliberate part-scopes, recorded in the code where they live: the YM2612
Project2612 legacy arm is not ported (it needs a file-level fact and a render
hook, exists for one archive's old trims, and the default Nuked row never
consults it -- see `start_option_bits`), and the paired volume override
reaches the estimate but not the linked child's audible gain inside the core
binding (the rare files carrying such entries -- see `linked_contribution`).

---

## Known residuals, updated state

- **Closed at 1.0000**: Y8950 (was 0.8287), WonderSwan (was 0.9888), OKIM6258 (was 0.9766, then the 0.9327 regression — both causes found and fixed, see the new-issue section; one file's resampler residual is in the known-gap note).
- **Closed at the shared ideal**: YM2612 0.9922 (was 0.904).
- **Nearly closed**: YM2413 0.9896 (was 0.977). The last 0.004 is open.
- **Unchanged, contributors known**: SN76489 0.358 — the H3 fix removes one contributor for future rips; the M7 fix (`94fb401`) removes another for files that declare non-Sega noise parameters (the current corpus sample has none, so the number does not move); M3 (planned) holds the T6W28 case; the core noise texture stays the primary cause.
- **Unchanged, explained**: YMF262 0.9898 and YM3812 0.9771 (free-running LFO and noise phase), YM3526 0.7533 (cross-core band), SAA1099 0.8471 (noise phase), ES5503 −6.5 cents (cause still open; the pin check killed the old core-revision theory).

## One refuted claim

A finder said the AY PCM3CH option suppresses hard-panned mixing. The verifier proved this wrong. With the pinned configuration, the two mixer paths make identical output. The gap is only audible with custom pans (kept as L13).

## Areas without examination (critic result)

1. **Per-channel mute plumbing.** The reference mutes inside the core mixer. Chip state continues. Our engine gates the write stream for cores without a native mute (`channel_gate.rs`). Nobody compared the state after mute and unmute.
2. **Seek and scrub.** The reference seeks with a full replay of all commands. Our engine seeks with a last-value projection. State the fold does not model lands differently after a scrub. The parity harness renders from the start, so it cannot see this.

## Data

The full finding data (claims, verdicts, evidence with line numbers) was the audit workflow's output, in the session's temporary task store. The durable records are: this document, the two plan documents beside it, the parity `THRESHOLDS` table (`crates/vgms-app/src/parity/mod.rs`), and the fix commits named above.
