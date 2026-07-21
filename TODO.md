- Update docs (VGM, new behaviours)
- `chip_write_delay` is gone (setting, config key and engine mechanism). It
  rendered real audio time after every register write, so the Python could
  observe key-off/key-on edges that PyOPL's DBOPL would otherwise collapse --
  at the cost of stretching the song past the length its own file declares.
  nuked-opl3's write buffer solves that properly: it spaces queued writes a
  couple of samples apart *inside* the audio being generated, for everyone,
  without adding time. An ini still carrying the key loads fine; the key is
  ignored.
- Metadata edits (GD3 tag, VGM loop fields) now mark the song dirty, so the
  discard-changes prompts cover them. Tracked separately from the instruction
  stream's revision, which keys the audio snapshot and waveform render -- a tag
  edit must not reload either. The Python tracked only instruction edits.

## VGM

- VGM optimiser (`vgm_cmp` equivalent) -- done. Strips OPL register writes that
  rewrite a register with the value it already holds (inaudible on a
  level-sensitive latch) and merges the delays those drops leave adjacent,
  conserving the sample total exactly. Reached three ways: Edit > Optimize VGM
  (undoable), the "Optimize VGMs on export" rip checkbox (default on, runs before
  the gzip step), and `drotrim optimize <in> [out]`. Route B (independent
  implementation from chip facts, not a port of vgmtools), so the project stays
  LGPL-2.1; the correctness net is render parity through nuked-opl3 with
  *immediate* writes (the buffered playback path spaces writes a couple of samples
  apart, an inaudible shift that byte-parity would spuriously flag). Corpus run
  over the local OPL packs: most published packs are already `vgm_cmp`'d so only a
  minority shrink, but un-optimised captures shrink up to ~70%. Loop safety: the
  register cache resets at the loop point so the loop body re-establishes its own
  state across the seam. The delay re-encoder is byte-minimal (full 0x61 chunks
  plus a tail of up to two 0x62/0x63/0x7n commands, borrowing from the last chunk
  when that shaves a byte). Possible follow-up: a stronger timer-register rule
  (drop even value-changing writes to the inaudible 0x02-0x04 timer control).
- Rip mode (VGMRips submission prep) -- done. Open a folder as a project; the
  package .txt is re-parsed on reopen; the track list, lengths and loop times are
  computed from each file; per-track preview, open-in-editor and quick-edit
  (rename + GD3); "Save Package Files" writes the .txt + .m3u; "Export Zip..."
  builds the submission zip (songs gzipped to .vgz, PNG optimised with oxipng).
  Possible follow-ups:
  - Recursive / multi-region screenshots, and per-region game titles.
  - Preserve hand-aligned multi-line author blocks verbatim (currently reflowed
    to a greedy wrap on save; see the note in `dro-core/src/rip.rs`).
  - Extend the VGM reader so more real PC-AT packs load (0x67 data blocks, the
    "data starts at 0x60" minimal header); today those tracks show as unreadable.
- Support for header features:
  - Loop points -- done. A region is marked in the editor (Shift+click and
    Shift+right-click on the waveform, `[` and `]` on the selected row, or the
    Edit menu), played back with the Loop toggle and a repeat count, auditioned
    at the join with "Seam", and written to the header with Edit > Apply Loop to
    Metadata. Both markers survive trimming as instruction indices, and the
    header's loop length is derived from them. See
    `docs/loop-points-2026-07/HANDOVER.md`. Possible follow-ups:
    - A loop end short of the song's end is honoured here and survives a save,
      but other players restart at the end-of-data command whatever the header
      says. A crop/trim would make it universal, and the `RangeMarkers` the loop
      uses were built to be reused for exactly that.
  - Volume boost
- Emit a higher-version VGM header when converting. The Python reserved 0x100
  bytes on purpose, leaving room for the fields later versions add (the v1.70
  extra-header offset at 0xBC, and beyond). The Rust port writes a tight 0x80
  v1.51 header instead, because the padding was what stopped its output from
  round-tripping and nothing fills those fields yet. Restore the larger header
  once there is something to put in it -- and bump the version field with it,
  rather than padding a v1.51 header.
