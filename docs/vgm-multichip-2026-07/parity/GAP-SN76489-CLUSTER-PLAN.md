# Plan: the SN76489 cluster's remaining gaps (audit M3, M7)

Status: **planned, not implemented.** The cluster's high-severity half (H3,
Game Gear stereo) landed on the Nuked-PSG default: the `0x4F` mask now has its
own port on the core (never the PSG bus) and each output side sums the
channels its mask nibble enables through the die's own `YMPSG_SetMute`. The
two remaining members need a seam that does not exist yet, recorded here.

## The gaps

- **M7 — header noise parameters.** The reference's Maxim core receives the
  header's feedback taps and shift-register width, so a BBC Micro (taps
  0x0003, width 15) or Tandy (taps 0x0022) rip plays that part's own noise
  sequence. Our default Nuked-PSG is a die trace of the Sega VDP part; its
  own module doc concedes a non-Sega rip plays Sega noise. The taps cannot be
  built "onto" a die trace, and the vendored submodule is never edited
  (PROVENANCE policy). Our libvgm Maxim row already maps the fields correctly
  (`specs.rs`) -- it just sits behind the promoted default.
- **M3 — T6W28 linking.** For a Neo Geo Pocket header (clock bits 30+31) the
  reference starts two Maxim halves and wires the second's
  `SN76496_CFG.t6w28_tone` to the first's live device: tone registers drive
  one side, noise-chip registers the other, and the noise channel takes its
  period from the tone half. We start two unlinked chips (every SN core row
  nulls `t6w28_tone`; Nuked-PSG does not model the variant), so NGP rips play
  wrong noise pitch and no stereo image.

## Why not now

Both need **header-aware core selection**: `CoreRegistry::core_for(kind)` and
the engine's `with_cores` factory see only the `ChipKind`, never the file's
`ChipSettings`, so "this file declares BBC taps -> build the Maxim row instead
of the promoted die trace" has nowhere to live. T6W28 additionally needs a
**cross-instance link**: the second `LibVgmChip`'s config must carry the first
instance's live device pointer before `SndEmu_Start`, and our voices start
independently (`vgm_engine::with_cores` builds each in isolation). Changing
the factory signature ripples through the registry, the engine, and the audio
service seam -- an architectural change, not a patch.

## Agreed direction

1. Extend the factory seam so core selection can consult the header:
   `core_for(kind, &ChipSettings)` (or a `CoreInfo::claims(settings)`
   predicate per row). Default rows keep today's behaviour when the settings
   say nothing special.
2. M7: the SN76489 selection prefers the libvgm Maxim row when the header
   declares non-default noise taps or shift width; the die trace stays the
   default for Sega-parameter files. The picker still allows either manually.
3. M3: on a T6W28 header, select the libvgm Maxim row for **both** instances
   and link them: give `LibVgmChip` an explicit pairing step (the engine calls
   it after both voices exist, before the first write) that fills
   `t6w28_tone` from the partner's `DEV_INFO` and forces the same core, as
   `vgmplayer.cpp`'s second-instance start does. The balance layer's existing
   T6W28 halving exemption stays.
4. Parity check afterwards: `VGMSTUDIO_PARITY_CHIPS=sn76489` over the corpus;
   the known 0.358 scorecard row should move if GG/BBC/NGP files are in the
   sample set.
