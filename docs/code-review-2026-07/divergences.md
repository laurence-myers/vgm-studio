# Deliberate divergences from the Python (rust branch)

Behaviours where the Rust port intentionally differs from the wx/Python
original. Recorded so they stop being re-raised as bugs in reviews.

## From the 2026-07 review (parity-6/7 and related)

- **DRO Info is view-only for a VGM.** The header-edit fields (OPL type,
  length) apply only to DRO songs; for a VGM the dialog is read-only. Editing a
  VGM header through that path could desync the header from the command stream,
  so it is deliberately gated. (parity-6)

- **The 6th "all register options" column is a hover tooltip.** The Python
  packed every possible register-description into a sixth table column; here the
  row shows the resolved description and the full list is on hover, keeping the
  table readable. (parity-7)

- **Stop leaves the readout at the pause point.** Stop pauses and rewinds the
  engine but the position readout stays where playback stopped, rather than
  snapping to 0. Matches the transport's "resume from here" feel.

## Decided during the review (implemented, not divergences to revisit)

- **Pos. column stays hex, and is labelled so.** The table's `Pos (hex)` column
  is hexadecimal; Goto parses hex (optional `0x`), Find accepts `0x`, and the
  Goto field says `(hex)`. (parity-2)

- **`buffer_size` is wired** into the cpal stream, clamped to the device's
  supported range with a host-default fallback. Buffer-size-agnostic engine, so
  audio bytes are unchanged. (parity-1)

- **Save As cannot change a song's format.** Saving a DRO as `.vgm` (or vice
  versa) is rejected with a "use Convert to VGM" message rather than writing
  bytes the app can't reopen; the Save As filters only offer the song's own
  format. (M5/ux-2)

- **Pure metadata edits (GD3 tag, VGM loop point) are not tracked as "dirty".**
  They don't bump the editor revision (which keys audio/waveform staleness), so
  -- as in the Python -- they don't trigger the unsaved-changes prompts. (H2)
