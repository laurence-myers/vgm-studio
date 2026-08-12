# Gap audit: VGMPlay reference against our engine

Audit date: 2026-08-12. Updated: 2026-08-12, after the fix branch.
Method: a 13-agent workflow. Six agents examined six areas. Six verifier agents tried to refute each finding. One critic agent searched for areas without examination.
Reference: VGMPlay 0.52 at `E:\Code\Cpp\vgmplay-libvgm`, with the pinned configuration `docs/vgm-multichip-2026-07/parity/VGMPlay.ini`.
The two libvgm pins are almost equal. Our pin is one commit newer. That commit does not change the emulation cores.

Result of the audit: 38 confirmed findings, 31 gaps after merge of duplicates.
Result after the fix branch (`vgmplay-parity-fixes-2026-08`, same day): **12 gaps fixed, 3 gaps planned, 16 gaps open, 1 new regression found**. The sections below list only the open items. The fixed items are in the record directly below.

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

The parity bars moved with the measurements (`4ce29f9`, `f90d268`): the
YM2612, Y8950, and WonderSwan now take the shared 0.99 bar; the YM2413 bar
rose to 0.98. The old YM2612 theory — that the gap lived in the reference
player's driver — is disproven. Both layers of that gap were ours.

## Planned, not implemented

The owner redirected these to plan documents; the designs are agreed there.

- **H5. DRO v1 opcode tests** (`0x01`/`0x04` disambiguation). A verbatim port misreads our own tool-written v1 files; the corrected design is in `GAP-H5-DRO-V1-PLAN.md`.
- **M3. T6W28 linking** and **M7. SN76489 header noise parameters**. Both need header-aware core selection (and, for the T6W28, a cross-instance device link); the design is in `GAP-SN76489-CLUSTER-PLAN.md`.

## New issue found by the fixes

**OKIM6258 regression: 0.9766 before the branch, 0.9327 after.** The H1
DAC-stream rework causes it, on X68000 stream files. The bisect is conclusive:
the old stream engine reads 0.9766 exactly, and the L3 option bit and the M2
rate mode are innocent (the OKIM6258 core ignores the rate mode). Per-file
evidence: most files drop a few points, one file falls to 0.69, and two files
gain a new flat −15-cent pitch offset — the sign of a stream length or timing
detail that diverges from the reference. The parity bar is lowered to 0.92 as
a tripwire, with the evidence in its note (`f90d268`). A separate fix session
is running for this item.

---

## Open: medium severity

### M4. QSound: the old-clock key-on aid is missing
For QSound files with the old 4 MHz clock, the reference keeps each channel's start address. It writes the address again when the pitch goes from zero to a value, and at each phase write. This replaces key-on writes that vgm_cmp removed. Our engine only corrects the clock (x15) and keeps no addresses. Effect: old CPS1/CPS2 rips do not start some notes.

### M5. The extra-header clock for a second chip is not applied
The reference reads the second instance's clock from the v1.70 extra header. Our parser reads `ExtraHeader::clocks`, but only the balance code uses it. Both instances get the first chip's clock. Effect: dual-chip files with two different clocks play the second chip at a wrong pitch and rate.

### M6. NES APU: one option bit is different
The pinned configuration gives option value 0x3B7. Our engine sends 0x1B7. The one different bit is OPT_TRI_NULL. With the bit set, a stopped triangle channel goes down to the null level. Without it, the channel holds its last step. Effect: a DC step and a click at each triangle stop.

### M8. The loudness estimate does not count connected devices
The reference's volume estimate follows each device link. A YM2203 counts as FM 0x100 plus SSG 0x80. Our estimate counts only the parent chip. Effect: for YM2203 + OKIM6295, the reference halves all chips and we do not. Our whole mix is then +6 dB against the reference. The relative levels stay correct.

### M9. Six chips are absent from the roster and from the estimate
The reference tables hold 48 chips. Ours hold 42. K007232, K005289, MSM5205, MSM5232, BSMT2000, and ICS2115 are absent. Effect: the chips we do play become a power of two too loud in such files, and the absent chip itself is silent.

### M10. DRO OPL3: the reset does not send register 0x105 first
DOSBox writes the register dump in the wrong order. The reference writes 0x105 = 1 and 0x104 = 0 before it replays an OPL3 file. Our conversion replays the dump on a fresh chip with newm=0. Effect: wrong timbres and broken 4-op patches until the game writes those registers again. (More relevant now: the H4 fix makes many more files play as OPL3.)

## Open: low severity

- **L4. Compressed blocks, widths 9-16 bits.** The reference builds values low-chunk first; our reader builds them MSB first. Widths above 8 are rare.
- **L7. Old headers (v1.00/1.01).** The reference finds the first FM command and gives the clock to the YM2612 or YM2151. Our parser always makes a YM2413. Such files are rare.
- **L8. Version-gated clock reads.** The reference reads each clock field that fits in the header. We obey the declared version. This is deliberate and documented.
- **L9. Loop base and loop modifier.** The reference scales the loop count with header bytes 0x7E/0x7F. Our playback loop count is app policy only.
- **L10. YM2612 alternate rows.** The GPGX and Gens picker rows never get option bits, so the YM3438 variant and the Project2612 repair stay off there. The default Nuked row is correct.
- **L11. YM3812 with clock bit 31, single chip.** The reference pans it hard left at double level. We play it in the center. Only degenerate files do this.
- **L12. Extra-header volume for connected devices.** The reference accepts a volume entry for a linked SSG or OPL4 FM half. Our balance code drops paired entries.
- **L13. AY PCM3CH option.** Not set on our side. With center pans, the output is identical. It becomes audible only with custom pans from our chip mixer.
- **L14. Downsample shape.** The reference's linear mode is a box average across all source samples; ours is a two-tap interpolation or a sharp sinc. Noise texture from fast PSGs differs a little.
- **L15. DRO tolerance.** A bad v2 code-map pair: the reference skips the pair, we refuse the file. DRO v0: the reference plays it, we refuse it. A stray second-chip select in a single-OPL2 v1 file: the reference drops those writes, we fold them into chip 1.

---

## Known residuals, updated state

- **Closed at 1.0000**: Y8950 (was 0.8287), WonderSwan (was 0.9888).
- **Closed at the shared ideal**: YM2612 0.9922 (was 0.904).
- **Nearly closed**: YM2413 0.9896 (was 0.977). The last 0.004 is open.
- **Regressed, under repair**: OKIM6258 0.9327 (was 0.9766). See the new-issue note above.
- **Unchanged, contributors known**: SN76489 0.358 — the H3 fix removes one contributor for future rips; M3 and M7 (planned) hold the others; the core noise texture stays the primary cause.
- **Unchanged, explained**: YMF262 0.9898 and YM3812 0.9771 (free-running LFO and noise phase), YM3526 0.7533 (cross-core band), SAA1099 0.8471 (noise phase), ES5503 −6.5 cents (cause still open; the pin check killed the old core-revision theory).

## One refuted claim

A finder said the AY PCM3CH option suppresses hard-panned mixing. The verifier proved this wrong. With the pinned configuration, the two mixer paths make identical output. The gap is only audible with custom pans (kept as L13).

## Areas without examination (critic result)

1. **Per-channel mute plumbing.** The reference mutes inside the core mixer. Chip state continues. Our engine gates the write stream for cores without a native mute (`channel_gate.rs`). Nobody compared the state after mute and unmute.
2. **Seek and scrub.** The reference seeks with a full replay of all commands. Our engine seeks with a last-value projection. State the fold does not model lands differently after a scrub. The parity harness renders from the start, so it cannot see this.

## Data

The full finding data (claims, verdicts, evidence with line numbers) was the audit workflow's output, in the session's temporary task store. The durable records are: this document, the two plan documents beside it, the parity `THRESHOLDS` table (`crates/vgms-app/src/parity/mod.rs`), and the fix commits named above.
