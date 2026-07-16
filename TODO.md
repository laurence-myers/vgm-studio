- Update docs (VGM, new behaviours)
- Publish new version of PyOPL

## VGM

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
  - Loop points: the loop point now survives trimming (it is held as an
    instruction index and the byte offset is recomputed on save), and the loop
    length is derived from what is left. What is still missing is *playback*:
    looping the song when it reaches the end.
  - Volume boost
- Emit a higher-version VGM header when converting. The Python reserved 0x100
  bytes on purpose, leaving room for the fields later versions add (the v1.70
  extra-header offset at 0xBC, and beyond). The Rust port writes a tight 0x80
  v1.51 header instead, because the padding was what stopped its output from
  round-tripping and nothing fills those fields yet. Restore the larger header
  once there is something to put in it -- and bump the version field with it,
  rather than padding a v1.51 header.
- Sometimes there's a gap at the end of the waveform panel. Related to delays?
- Add VGM/VGZ to drag and drop
- On a long song, moving the playback cursor, while it's already playing, will quickly play
  a blast of the skipped music.
