# Per-channel panning for OPL2 songs on RetroWave hardware — plan

Status: **DEFERRED** (2026-07-23). Planned, not implemented — the user chose to
disable the inert controls instead for now (hardware mode greys the pan knobs,
Spread, Custom toggle and volume boost, and hides the peak meter). Keep this
document: if hardware panning is wanted later, the design below still stands, and
the controls it would re-enable are gated on one predicate,
`AudioConfig::renders_samples`.

> **Substrate update (2026-08-05, Stage K / k-5.2):** the RetroWave service no
> longer runs `PlayerEngine` over a `Song` — the board is driven by `VgmEngine`
> through `opl_hardware_core`, an OPL `ChipCore` whose write gate never stands
> down (`vgms-synth`), with the shadow+diff serial chip underneath unchanged.
> The register-mirroring design below is engine-agnostic (it operates at the
> chip-write level), but its implementation hooks now live in
> `opl_hardware_core` / the RetroWave service rather than the deleted
> `PlayerEngine` path — re-anchor the step list before implementing.

Goal: make the channel panel's pan knobs audible on RetroWave hardware for OPL2
songs, which is today the one place they are silently inert. Mechanism (the user's
design): mirror every bank-0 channel into the YMF262's second register array —
**bank 0 becomes the left copy, bank 1 the right copy** — and simulate the pan
position by scaling each copy's volume (a 0..1 multiplier per side).

## 1. Scope: exactly `OplType::Opl2`

The trick spends the chip's spare register array, so it exists only where one is
spare:

- **OPL2 (single)** — 9 song channels, 18 physical: mirror freely, **zero
  polyphony loss**. This is the feature.
- **Dual OPL2** — bank 1 already *is* the second chip. Excluded; behavior
  unchanged (both speakers, pan inert).
- **OPL3** — the song owns both banks and its own stereo image. Excluded,
  unchanged.
- **Rhythm-mode percussion** — the YMF262's second array has no functional
  rhythm set, so drums cannot be mirrored. Drums stay centred (both speakers);
  see §5.

The Princess Maker 2 file from the last bug report is a single-YM3812 VGM —
squarely in scope.

## 2. Where it lives: entirely inside `SerialOpl3Chip`

No engine, service, UI, or trait changes. The engine already broadcasts
everything the chip needs, through the register interface it already has:

- **Pan values**: `Panning::Custom` writes the stereo-ext panpots `0x0D0+ch`
  (immediate path → they land in the chip's *shadow*, never on the wire). The
  chip stops ignoring them and reads `shadow[0][0xD0+ch]` as channel *ch*'s pan
  byte (`0x00` hard left … `0x80` centre … `0xFF` hard right, the panel's
  scale).
- **Engaged/disengaged**: the engine writes `0x105` with **bit 1** set while
  Custom panning is engaged and clear otherwise. The chip currently strips that
  bit for the wire (it must — on real silicon it is not a panpot enable); now it
  *reads* it first as the engage signal. Disengaged or never-written panpots →
  centre.
- **Timing**: the pump already sets `reconcile = true` on `SetPanning`, so a pan
  change while playing materializes immediately as a small diff burst (§4). No
  pump changes.

## 3. The pan law: match the emulator exactly

The vendored Nuked core deliberately does **not** use the upstream
constant-power law. It uses a linear **balance** law
([core.rs:1421](../../vendor/nuked-opl3/src/core.rs), a documented local
deviation): with pan byte `v`,

```
right_gain = min(v / 127, 1)          left_gain = min((v ^ 0xFF) / 127, 1)
```

The active side holds **unity from the centre outward**; only the opposite side
attenuates. At centre both sides are unity — which is also why mirrored-at-centre
hardware output has the same per-speaker level as today's both-speakers routing,
and why engaging pan does not drop centred channels by 3 dB. The hardware table
must reproduce this law, not cos/sin, or A/B-ing the two backends will disagree.

Gains become **total-level attenuation offsets** in the OPL's native 0.75 dB
steps:

```
offset(gain) = 0            if gain >= 1
             = SIDE_OFF     if gain == 0   (see below)
             = round(-20 * log10(gain) / 0.75)   otherwise, clamped to 0x3F
```

A 256-entry `const`-style table (computed once at startup, or pre-generated
literally — 6-bit values, trivially reviewable) keyed by pan byte, yielding
`(left_offset, right_offset)`. Offsets add to the **6-bit TL field only**; the
KSL bits (top 2) pass through untouched; saturate at `0x3F`.

**Which operators**: the loudness of a channel is its carrier's TL. Connection
bit (`0xC0` bit 0) = 0 (FM): scale operator 2 only. = 1 (AM): both operators are
carriers — scale both. The connection bit lives in the shadow, so this is
re-derived per write; a mid-note connection change re-emits the affected TLs as
an ordinary diff.

**`gain == 0`** (hard pan): TL `0x3F` is −47.25 dB — effectively silent, but the
cleaner statement is to also clear that side's speaker bit in its `0xC0` copy.
Decide in implementation; the table carries a "fully off" marker either way.

## 4. The mechanics: `wire_target` — one pure function

Today the chip translates per-write (`translate(bank, reg, value)`) and diffs
`shadow` vs `hw` per register. Mirroring makes one song write influence several
wire registers (its bank-1 mirror; a pan change touches up to 4 TLs + 2 `0xC0`s
per channel), so the translation generalises to a single pure function:

```
wire_target(bank, reg) -> Option<u8>   // None = never written (panpots, gaps)
```

derived from `(shadow, opl_type, pan state, rhythm bit)`. Everything else
follows from it:

- `materialize()` — unchanged in shape: emit where `hw` differs from
  `wire_target`, same NEW-first/keys-last ordering. Mirroring falls out for
  free: bank-1 targets for an OPL2 song are *derived from bank-0 shadow*.
- A playback write updates the shadow, then locally diffs the small set of wire
  registers that write can affect (the reg itself, its bank-1 mirror, and for
  `0xC0`/panpot/`0x105`/`0xBD` writes the dependent TL/C0 registers of that
  channel). A helper maps "shadow write → affected wire registers" so the local
  path and `materialize()` cannot disagree — plus a test asserting a full
  materialize after any single write emits nothing further.

**Mirror rules for OPL2 songs** (the heart of `wire_target`):

| Shadow write (bank 0) | Left copy (bank 0, wire) | Right copy (bank 1, wire) |
|---|---|---|
| Operator regs `0x20..=0x35`, `0x60..=0x95`, `0xE0..=0xF5` | as written | mirrored verbatim |
| TL `0x40..=0x55` | + left offset on carrier op(s) | mirrored + right offset |
| `0xA0..=0xA8`, `0xB0..=0xB8` | as written | mirrored verbatim (keys both copies together) |
| `0xC0..=0xC8` | `(v & 0x0F) \| 0x10` (CHA only) | `(v & 0x0F) \| 0x20` (CHB only) |
| `0xBD` | as written | **never** (non-functional; depth/rhythm bits are array-0-global) |
| `< 0x20` | as written | **never** (bank 1's `0x04`/`0x05` are `0x104`/`0x105`!) |
| Panpots `0xD0..=0xD8` | never on the wire | never (they are the pan *input*) |

`0x105` stays pinned to `0x01` (the existing `opl2_compat` rule) — the mirror
requires bank 1 writable, which it already guaranteed. The C0 split **replaces**
the current OR-`0x30` rule for `Opl2` (Dual OPL2 keeps OR-`0x30`).

**Always mirror** OPL2 songs, even under `Panning::Original` (= centre offsets,
full/full). No engage/disengage mode switch in the chip, no re-materialize storm
on toggle — engaging pan just changes offsets, an ordinary diff. Phase
coherence: OPL phase generators start deterministically at key-on and both
copies key in the same SPI burst, so the copies track; each side hears exactly
one copy anyway, so even drift would be inaudible as such.

## 5. Rhythm mode

While `shadow[0][0xBD]` bit 5 is set, channels 6–8 are the drum kit:

- their `0xC6..=0xC8` wire targets stay `(v & 0x0F) | 0x30` (both speakers,
  centred);
- their `0xA6..=0xA8`/`0xB6..=0xB8` and operator regs do **not** mirror to
  bank 1 (nothing there could sound them; writing is harmless but pointless —
  pick "don't" and test it);
- pan knobs 7–9 are inert for drums (as they are on the emulator's rhythm
  channels — verify and note the emulator's actual behavior during
  implementation).

Because `wire_target` is pure over the shadow (including the rhythm bit),
toggling rhythm mode mid-song is just another diff: the C6–C8 split/unsplit and
the mirror copies appear or fall away in one materialize-shaped burst.

## 6. What composes for free

- **Muting** — the engine gates/masks bank-0 writes before the chip sees them;
  mirrors derive from the shadow, so a muted channel's key never arms either
  copy. The `mask_replay` seek fix carries over untouched.
- **Seeks / pause / resume** — shadow-only replay + materialize already handle
  it; the diff just includes bank-1 targets now.
- **The eligibility gate** (any-chip plan) — unaffected; this feature is keyed
  off `OplType`, which the chip already receives.

## 7. Tests

1. **Pan table**: unity at and past centre on the active side; monotonic
   attenuation on the passive side; `0x00`/`0xFF` fully off; values match
   `panpot()`-derived gains within half a TL step (parity guard against the
   vendored law — reference values computed from the same formula, not the
   vendor code).
2. **Chip, mirroring**: a bank-0 operator write emits both copies; C0 split
   values; TL offsets applied to carrier-only (FM) vs both (AM); KSL preserved;
   saturation; regs `< 0x20`, `0xBD`, panpots never mirrored; Dual-OPL2/OPL3
   chips emit exactly what they do today (regression).
3. **Chip, pan flow**: panpot + `0x105` bit-1 writes via the immediate path
   change targets; materialize after a pan change emits only the affected
   TL/C0 registers; disengage returns to centre; defaults centre when never
   written.
4. **Chip, rhythm**: bit-5 set → C6–C8 both-speakers, no mirror for 6–8;
   toggling emits the transition diff; channels 0–5 unaffected.
5. **Local-diff completeness**: after any single playback write, a full
   materialize emits nothing (the affected-registers helper missed nothing).
6. **Pump**: `SetPanning` while playing puts a bounded burst on the mock wire;
   while paused, nothing until resume.
7. **Hardware checklist** (user's board + the PM2 file): pan a lead hard left /
   hard right / sweep while playing; centre A/B against the emulator for
   loudness; drums stay centred with rhythm songs; toggle Custom on/off
   mid-song; mute + pan together.

## 8. Staged implementation

1. **refactor(retrowave): `wire_target`** — pure-function translation, behavior
   byte-identical (existing tests must pass unchanged; add the local-diff
   completeness test).
2. **feat(retrowave): mirror OPL2 into bank 1 at centre** — C0 split + verbatim
   mirroring, no pan input yet. Audibly equivalent to today (▶ *user
   hardware-check: OPL2 songs still sound right*).
3. **feat(retrowave): the pan law** — table + TL offsets + engage bit +
   carrier/AM selection (▶ *user hardware-check: knobs work*).
4. **feat(retrowave): rhythm-mode carve-out** — drums centred, transition diff.
5. **docs** — PLAN/PANNING updates; note in `DEVELOPMENT.md`; drop the
   "pan is inert on hardware" caveat for OPL2.

## 9. Risks and open questions

- **Loudness parity at centre** is by-construction (unity/unity balance law),
  but the final word is the §7.7 A/B on real hardware.
- **TL headroom**: a song already at TL near `0x3F` clamps early on the quiet
  side — the pan just reaches "off" sooner. Cosmetic, not a defect.
- **Six writes per note instead of three** during dense passages: still ~40 µs
  of SPI per write and far below the CDC envelope (§1.4 of the main plan);
  the quantum batches them into one frame regardless.
- **Emulator/hardware divergence for rhythm channels' pan** — confirm what the
  emulator does with panpots on rhythm channels and match the *audible*
  behavior, not an assumption.
- If the vendored balance law ever reverts to upstream constant-power, the
  hardware table must follow — the §7.1 parity test is written against the
  formula, so it will not catch a vendor change by itself. Leave a pointer
  comment in both places.
