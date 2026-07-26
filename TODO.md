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
  (undoable), the "Optimize VGMs on export" pack checkbox (default on, runs before
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
- Pack mode (VGMRips submission prep) -- done. Open a folder as a project; the
  package .txt is re-parsed on reopen; the track list, lengths and loop times are
  computed from each file; per-track preview, open-in-editor and quick-edit
  (rename + GD3); "Save Package Files" writes the .txt + .m3u; "Export Zip..."
  builds the submission zip (songs gzipped to .vgz, PNG optimised with oxipng).
  A live "Submission checklist" (grouped Package info / Track tags / Consistency
  / Loops / Files) flags what the VGMRips wiki wants verified before submission
  -- complete and consistent GD3 tags, hyphen-separated release dates, update
  notes, verified loops -- as three tiers (errors block export, warnings prompt,
  notes never gate). Each unresolved item is a clickable line that jumps to its
  fix (a meta form field focuses; a per-track item opens that track's quick-edit),
  the track table gains a per-track status glyph, and a one-click "Convert dates
  to hyphens" fix-assist rewrites every slash date (pack + tracks) as one undoable
  batch. Pure rules live in `dro_core::pack::readiness`.
  Possible follow-ups:
  - Recursive / multi-region screenshots, and per-region game titles.
  - Preserve hand-aligned multi-line author blocks verbatim (currently reflowed
    to a greedy wrap on save; see the note in `dro-core/src/pack.rs`).
  - Extend the VGM *editor's* reader so more real PC-AT packs open for editing
    (0x67 data blocks, the "data starts at 0x60" minimal header). Pack mode
    already lists and tags them -- see the any-chip work below, whose Phase A
    made every such file readable as a container. Full command parsing is mc-4.
- Multi-song splitter (`vgm_sptd` equivalent) -- done, for VGM **and** DRO
  captures. File > Split Songs cuts one long capture -- a whole sound-test session
  logged in one go -- into its per-song files at the silent gaps. A dialog runs
  the detector live as a gap-threshold slider (seconds) is dragged, lists each
  song's start and length with an include checkbox and a Preview button, has a
  decay-tail slider (keep some of the trimmed silence after each piece, 0 s
  default), and exports `NN <stem>.vgm`/`.dro` into a chosen folder, then offers
  to open that folder as a pack project. Each piece is prepended with the OPL
  register state the capture had reached at its start -- each touched register's
  last write, reused byte for byte from the source so the encoding is exact
  whatever the format (for DRO v1 the current bank is tracked through the
  bank-switch opcodes) -- so a song taken from the middle opens on the chip state
  it would have had mid-play rather than on silence. Detection and materialisation
  are format-agnostic (native delay unit: samples for VGM, ms for DRO); a VGM
  piece also copies the source GD3 with the track title blanked. Route B (gap
  detection is one accumulator; state capture is a register-file fold), so the
  project stays LGPL-2.1; the net is a corpus-sanity test that tripling the real
  OPL2 rip with gaps splits back into three pieces each opening on the folded
  register state, plus DRO v1/v2 round-trip tests.
- Support for header features:
  - Loop points -- done. A region is marked in the editor (Shift+click and
    Shift+right-click on the waveform, `[` and `]` on the selected row, or the
    Edit menu), played back with the Loop toggle and a repeat count, auditioned
    at the join with "Seam", and written to the header with Edit > Apply Loop to
    Metadata. Both markers survive trimming as instruction indices, and the
    header's loop length is derived from them. Possible follow-ups:
    - A loop end short of the song's end is honoured here and survives a save,
      but other players restart at the end-of-data command whatever the header
      says. A crop/trim would make it universal, and the `RangeMarkers` the loop
      uses were built to be reused for exactly that.
  - Find Loop -- done. Edit > Find Loop searches the command stream for a block
    of writes that repeats later in the song -- the shape a raw rip has when the
    capture ran through the loop more than once -- and lists the repeats it
    finds, best-first, with vgmlpfnd's `e`/`f`/`!` quality flags. Clicking a
    candidate drops the editor's loop markers on it; Audition plays the seam;
    Apply writes it into the VGM metadata (VGM only, like Apply Loop). The search
    (`dro_core::find_loops`) strips delays before matching, so a body and its
    repeat match through timing jitter, buckets window starts by a rolling hash
    for near-linear candidate finding, and runs in a background task
    (`TaskKind::LoopSearch`) so the UI never blocks. On the YM3812/YMF262 corpus
    it recovers the tagged loop within a command or two, in a few milliseconds
    even for 100k-command captures.
  - Volume -- done. A bidirectional playback "volume" lever in the transport
    row, sitting on the VGM volume-modifier factor ladder (0.25x..64x, shown to
    two decimals) so every position is a real modifier value. By default the
    volume is per-song: opening a song sets it from that song's header volume
    modifier (unity for a DRO) and manual changes are transient (not written to
    drotrim.ini). A "Lock" toggle keeps the volume across songs and persists it,
    like the old behaviour; unlocking snaps back to the current song's modifier.
    Arrows step ~1.0 at unity and above, ~0.1 below; a typed value snaps to the
    ladder. Behind it the
    peak limiter has a clipping guard: the volume cannot rise past the lowest
    boost that has driven the limiter into clipping this song (it ratchets down as
    quieter boosts still clip), reset per song. "Match Volume" measures the song's
    peak (`dro_synth::measure_peak`, an internal render) and sets the lever to
    bring it to full scale. The VGM Metadata dialog's "Measure" button fills the
    header `volume_modifier` with the vgm_vol suggestion
    (`dro_core::volume::suggest_volume_modifier`) plus a decoded "= N.NNx"
    readout. Pack mode adds "Scan Volumes" (one background task over the whole
    pack) filling a Peak column, and "Apply Modifiers" writing every track's
    `volume_modifier` to level the pack -- album mode by default (one factor from
    the loudest peak) or per-track -- as one undoable batch. Follow-up: playback does NOT honour the
    header `volume_modifier` (the boost lever is the playback control); the
    `BoostLimiter::boost()` + `min_engaged_boost` plumbing is what a "playback
    applies the modifier" follow-up would build on.
- Any-chip VGM support -- **Phase A done** (the required minimum). Pack mode
  opens a folder containing *any* VGM, for any of the 42 chips the spec covers,
  versions 1.00-1.72: each track gets its title, length and loop from its
  header, and takes part in quick edit, bulk tag, Fix Names, Fix Dates, volume
  modifiers and export like any other. `dro_core::vgm::header` models every chip
  clock (version-gated, and bounded by the "header ends at the data start" rule
  that a minimal 0x60-header rip depends on), plus the v1.70 extra header;
  `dro_core::vgm::file::VgmFile` carries the command stream as an opaque span so
  a retag is byte-exact outside the GD3 block. The editor cannot open such a
  file -- it decodes commands into OPL register writes -- so it says what the
  file is and points at pack mode rather than reporting a load failure. What is
  gated, and says why: preview (no core for those chips yet) and the export's
  `vgm_cmp` optimiser (it folds an OPL register file). Remaining phases in
  `docs/vgm-multichip-2026-07/HANDOVER.md`: the full command parser (mc-4), the
  delete-only editor for any chip (mc-5), then playback (mc-6 onward).
- Emit a higher-version VGM header when converting. The Python reserved 0x100
  bytes on purpose, leaving room for the fields later versions add (the v1.70
  extra-header offset at 0xBC, and beyond). The Rust port writes a tight 0x80
  v1.51 header instead, because the padding was what stopped its output from
  round-tripping and nothing fills those fields yet. Restore the larger header
  once there is something to put in it -- and bump the version field with it,
  rather than padding a v1.51 header.
