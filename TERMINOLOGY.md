# VGM Studio — Controlled Dictionary

A controlled vocabulary for this codebase, in the spirit of ASD-STE100
(Simplified Technical English). The two STE rules this document enforces:

1. **One term, one meaning, one part of speech.** A word on the approved list
   means exactly one thing everywhere — code, comments, UI strings, docs.
2. **One concept, one term.** Where the codebase historically uses several
   words for one concept, one is *approved* and the others are listed as
   *non-approved synonyms* with a "use instead" pointer.

Entries were harvested from the code's own doc comments (2026-08-07, branch
`chip-mixer-2026-08`). Each entry cites where the meaning is defined or best
exemplified. Terms marked ⚠ carry a collision the writer must know about;
the [Collisions](#collisions--one-word-many-meanings) section lists them all.

Writing style for docs and comments (the STE house rules, abridged): present
tense, active voice, one instruction per sentence, ≤ 25 words per sentence
where possible; prefer the approved noun to a pronoun when the referent could
be ambiguous. US spelling in identifiers and UI strings (`optimize`); British
spelling is allowed in comments and docs (`optimiser`).

---

## The first rule of the vocabulary: OPL is a chip, DRO is a file

The single most load-bearing distinction, and the one the code's own history
blurs:

| Term | Approved meaning | Never use it to mean |
|---|---|---|
| **OPL** | The Yamaha FM chip family: YM3526, YM3812 (OPL2), Y8950, YMF262 (OPL3). A *capability* a document has. | "a DRO document". `DocSource::Opl` and `LoadedSong::Opl` carry a DRO; that naming is scheduled for repair ([docs/dro-arm-2026-08/PLAN.md](docs/dro-arm-2026-08/PLAN.md)). |
| **DRO** | DOSBox Raw OPL, the file format: raw OPL register writes plus millisecond delays, in encodings v1 and v2. A DRO is *always* OPL. | any OPL-ness of a VGM. |
| **VGM** | Video Game Music, the file format: chip-clock header, command stream, GD3 tag. A VGM *may* be OPL (wholly-OPL stream) or carry any of 42 chip kinds. | "not OPL". An OPL VGM is a VGM. |
| **OPL VGM** | A VGM whose header declares only OPL chips *and* whose every command projects to an OPL instruction (`VgmFile::is_opl`). | a DRO. |

Derived predicates, each with a distinct approved meaning:

| Predicate | Question it answers |
|---|---|
| `is_opl()` (DocSource/Editor) | "Does this document belong on the OPL board / have the OPL reading?" True for a DRO *and* an OPL VGM. |
| `VgmFile::is_opl()` | "Is this VGM wholly OPL?" (header claim + stream fact, recomputed after every edit). |
| `is_opl_only()` (header) | The weaker *header-only* claim; counts the YM3526/Y8950 cousins that `is_opl()` does not. Do not use for feature gating. |
| `has_song()` / `song()` (Editor) | "Is the document a DRO?" — a *format* question wearing a legacy name (repair planned). |

---

## Documents and formats

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **document** | n | Whatever the editor holds open: a DRO or a VGM. The neutral term — use it whenever the format is not the point. | [editor.rs:293](crates/vgms-ui/src/editor.rs) |
| **`Song`** ⚠ | n (type) | The decoded DRO document: header fields, instruction stream, millisecond delay prefix. "A VGM document is a `VgmFile`, not a `Song`." | [song.rs:20-29](crates/vgms-core/src/song.rs). Rename to `DroSong` planned. |
| **`VgmFile`** | n (type) | The one VGM document model for any chips: header + body + tag. OPL files get more features, not a different type. | [vgm/file.rs:140](crates/vgms-core/src/vgm/file.rs) |
| **`DocSource`** | n (type) | The loaded document handed to background jobs and audio backends, as one of two arms (DRO `Song` / `VgmFile`). Re-exported by vgms-synth as **`AudioSource`** — same type, historical name. | [doc_source.rs:26](crates/vgms-core/src/doc_source.rs), [lib.rs:85](crates/vgms-synth/src/lib.rs) |
| **song** (prose) ⚠ | n | Any playable piece, DRO or VGM, in UI text and CLI help ("Play a song"). Colloquial; in code-adjacent writing prefer **document** (the loaded thing) or **track** (inside a pack) or **segment** (inside a capture). | strings.rs, [lib.rs:49](crates/vgms-app/src/lib.rs) |
| **capture** ⚠ | n | One recorded session — possibly many songs end to end — as ripped from DOSBox or a console. The thing Split Songs cuts apart. | [split_songs.rs:1](crates/vgms-core/src/split_songs.rs). Prose uses "capture" for DROs and **rip** for VGMs; both are captures. Do not use *capture* as a verb for undo snapshots — say **snapshot**. |
| **rip** | n | A VGM logged from a real system ("a Mega Drive rip"). Synonym of capture on the VGM side; fine in prose. | [header.rs:8](crates/vgms-core/src/vgm/header.rs) |
| **track** | n | One song file inside a pack, with its parsed VGM and table entry. | [pack/state.rs:42](crates/vgms-ui/src/pack/state.rs) |
| **piece** | n | The standalone file lifted out of a capture by splitting or extraction. | [vgm/file.rs:441](crates/vgms-core/src/vgm/file.rs) |
| **VGZ** | n | A gzipped VGM; read transparently, written via `write_gzipped`, otherwise identical to VGM. | [vgm/mod.rs:1](crates/vgms-core/src/vgm/mod.rs) |
| **pack** | n | A VGMRips submission project: numbered `.vgz` tracks, description text, `.m3u` playlist, screenshot. Non-approved synonyms: *package* (the file format's own header word), *submission*, *archive* (reserve for the zip-opened in-memory store), *project* (reserve for the working folder). | [pack/mod.rs:1](crates/vgms-core/src/pack/mod.rs) |
| **opaque** | adj | A VGM body kept as unparsed bytes because its stream will not walk; openable for tags, not editable. | [vgm/file.rs:55](crates/vgms-core/src/vgm/file.rs) |
| **unwalkable** | adj | A readable VGM container whose command stream cannot be iterated. Distinct from **unreadable** (not a song at all). | [editor.rs:131](crates/vgms-ui/src/editor.rs) |

## Streams and rows

One concept — "an element of the document's stream" — currently has three
words, split by document kind. Approved usage keeps the split (it encodes
which model you are in) but requires the generic word **row** whenever the
document kind is not the point:

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **row** | n | One line of the instruction table, whichever document filled it. The generic term. | [editor.rs:27](crates/vgms-ui/src/editor.rs) |
| **instruction** | n | One decoded element of a *DRO* (or OPL-projected) stream: register write, bank switch, or delay. | [instruction.rs:93](crates/vgms-core/src/song/instruction.rs) |
| **command** ⚠ | n | One element of a *VGM* stream (`VgmCommand`): chip write, wait, data block, DAC-stream control, or raw opcode. Collides with the undo sense — see Collisions. | [stream.rs:107](crates/vgms-core/src/vgm/stream.rs) |
| **stream** ⚠ | n | The document's raw instruction/command bytes exactly as in the file, plus an index. Edits splice it; writing is a memcpy. Not the cpal output stream, not a DAC stream. | [stream.rs:594](crates/vgms-core/src/vgm/stream.rs) |
| **body** | n | `VgmBody`: the stream plus end-of-data marker and padding. | vgm/file.rs |
| **walk** | v | Parse a VGM body command by command using each opcode's defined length. A stream that "will not walk" becomes an opaque body. | [stream.rs:609](crates/vgms-core/src/vgm/stream.rs) |
| **projection** ⚠ | n | `OplProjection`: a zero-copy *view* reading a wholly-OPL VGM stream as OPL instructions on access. Explicitly not a conversion. (Third sense in the UI: a DRO's presentation as its generic chip set. See Collisions.) | [projection.rs:9](crates/vgms-core/src/vgm/projection.rs) |
| **project** | v | Convert a DRO's `Song` into an equivalent `VgmFile` (`opl_song_to_vgm_file`) so it plays through `VgmEngine` or reaches the OPL3 board. | [retrowave.rs:91](crates/vgms-app/src/services/retrowave.rs) |
| **bank** ⚠ | n | Which of the two OPL register files a write lands in (Low/High). DRO v1 switches it with a `BankSwitch` instruction; v2 and VGM carry it per write (VGM calls it **port**). Do not use for PCM sample banks — say **data-block bank**. | [instruction.rs:11](crates/vgms-core/src/song/instruction.rs) |
| **port** ⚠ | n | The register-bank index a VGM write targets (OPL3 high bank = port 1); also the spec's pseudo-ports (`MEMORY_PORT`, `BANK_PORT`, …). Not a serial port (say **serial port**), not a code port. | [stream.rs:30](crates/vgms-core/src/vgm/stream.rs) |
| **codemap** | n | DRO v2's per-file table mapping each 7-bit register code to a real OPL register; ≤ 128 entries. | [dro_data.rs:260](crates/vgms-core/src/song/dro_data.rs) |
| **index map** | n | DRO v1's logical-index → byte-offset table (v1 instructions are variable length); rebuilt after every splice. `VgmStream` calls the identical mechanism *offsets*. | [dro_data.rs:37](crates/vgms-core/src/song/dro_data.rs) |
| **escape** | n | DRO v1 opcode 0x04: the next byte is a register number that would otherwise collide with a delay/bank opcode. | [dro_data.rs:24](crates/vgms-core/src/song/dro_data.rs) |
| **data block** | n | A VGM 0x67 command carrying a PCM stream, ROM image, or RAM payload. Blocks are cumulative. | [stream.rs:119](crates/vgms-core/src/vgm/stream.rs) |
| **end-of-data marker** | n | VGM opcode 0x66 ending the stream; stored in the body, never indexed as a command. | [stream.rs:152](crates/vgms-core/src/vgm/stream.rs) |
| **raw** | adj | Bytes exactly as they sit in the file; the storage form that makes round trips byte-exact. Synonym *verbatim* is fine in prose. | [song.rs:80](crates/vgms-core/src/song.rs) |
| **header** | n | The file's leading fixed fields, kept verbatim and patched only where a value changed. Trap: `Song.ms_length` is the length *recorded in* the DRO header, not the header's own length. | [header.rs:427](crates/vgms-core/src/vgm/header.rs) |
| **claim vs fact** | n | Header lengths are *claims* (`total_ms`, `ms_length`); delay sums are *facts* (`stream_total_ms`, `total_delay_ms`). The timeline trusts facts; mismatches raise the trim warning. | [vgm/file.rs:168](crates/vgms-core/src/vgm/file.rs) |
| **GD3 tag** | n | A VGM's metadata block: eleven UTF-16LE strings in fixed order. Short form *tag* is approved. | [vgm/data.rs:58](crates/vgms-core/src/vgm/data.rs) |

## Time

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **delay** | n | A row that only advances the clock. In a DRO: milliseconds (short/long encodings). The app's generic word — `VgmStream::describe` even prints waits as "delay". | [instruction.rs:107](crates/vgms-core/src/song/instruction.rs) |
| **wait** | n | The VGM spec's delay command (0x61/0x62/0x63/0x7n), counted in samples. Use when speaking VGM-spec; otherwise say delay. | [vgm/data.rs:14](crates/vgms-core/src/vgm/data.rs) |
| **sample** ⚠ | n | VGM's time unit: one tick of 44100 Hz, regardless of playback rate. Distinct from an output PCM sample at the configured frequency — qualify when both are in play. | [vgm/mod.rs:14](crates/vgms-core/src/vgm/mod.rs) |
| **native unit** | n | A document's own delay unit — ms for a DRO (rate 1000), samples for a VGM (rate 44100). The splitter and `Segment` work in it. | [doc_source.rs:64](crates/vgms-core/src/doc_source.rs) |
| **delay prefix** | n | Exclusive cumulative sum of delays (len+1 entries); makes every time/position lookup a binary search. `VgmStream`'s identical mechanism is `wait_prefix`. | [song.rs:221](crates/vgms-core/src/song.rs) |
| **DLYS / DLYL / DALL** | n | Register-column tokens for short-delay, long-delay, any-delay rows — shown in the table, typed into Find Register. Short/long refer to encoding width, not duration. | [instruction.rs:74](crates/vgms-core/src/song/instruction.rs) |

## Editing operations

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **crop** | v | Keep only the marked region \[start, end) and discard the rest, splicing in a state patch. Menu: *Crop to Marked Region*. | [crop.rs:82](crates/vgms-core/src/crop.rs) |
| **delete region** | v | Cut the marked region out, bridging the seam with a state patch. Menu: *Delete Marked Region*. | [crop.rs:114](crates/vgms-core/src/crop.rs) |
| **trim** ⚠ | — | **Do not use for editing.** The heritage sense (remove an intro/tail; the product's old name) is retired in favour of **crop** / **delete region**. *Trim* now belongs to the mixer — see [Loudness](#loudness-six-words-six-different-gains). Surviving compounds: **auto-trim** (the DRO load-time removal of a bogus leading delay) and `vgm_sro`'s *ROM trim* keep their fixed names. | [crop.rs:16](crates/vgms-core/src/crop.rs) vs [chip_mix.rs:153](crates/vgms-synth/src/chip_mix.rs) |
| **splice** | v | Delete or re-insert whole rows by byte range in one O(n) pass; delete and insert are exact inverses, powering undo. Also: inserting state-patch writes at an edit's edge. | [splice.rs:1](crates/vgms-core/src/song/splice.rs) |
| **snapshot** ⚠ | n | An immutable copy handed across a boundary: the stream snapshot undo restores, the `Song` clone the audio callback owns. Four senses live in the tree — see Collisions. | [song.rs:39](crates/vgms-core/src/song.rs) |
| **repatch** | v | Rewrite a VGM header's derived fields from the edited stream. Edits repatch; the writer never recomputes. | [vgm/file.rs:757](crates/vgms-core/src/vgm/file.rs) |
| **retag** | v | Edit only a VGM's GD3 tag; header and body reproduced verbatim. | [vgm/file.rs:13](crates/vgms-core/src/vgm/file.rs) |
| **optimise** | v | Shrink a VGM by dropping audibly-redundant latch writes and merging adjacent waits, conserving playback timing exactly. (VGM-only today; a DRO must convert first.) | [optimize.rs:1](crates/vgms-core/src/optimize.rs) |
| **redundant write** | n | A write whose value equals the cell's cached value on a latch register; dropping it cannot change the audio. | [chip_state.rs:264](crates/vgms-core/src/chip_state.rs) |
| **unmodelled command** | n | A command whose state cannot be replayed (PCM RAM write, unknown opcode); crops warn instead of promising restoration. | [chip_state.rs:299](crates/vgms-core/src/chip_state.rs) |
| **merge barrier** | n | A stream position (loop point / loop end) a wait-merge run must never span. | [optimize.rs:38](crates/vgms-core/src/optimize.rs) |
| **slide** | v | Move a stored row index left past a deletion so loop and region markers survive edits identically everywhere (`slide_index_past_deletion` — a genuinely shared primitive). | [song.rs:508](crates/vgms-core/src/song.rs) |
| **selection** | n | The table's multi-row selection (click/Ctrl/Shift). Distinct from the **marked region** — only Delete acts on the selection. | [selection.rs:22](crates/vgms-ui/src/selection.rs) |
| **marked region** | n | The half-open row range between the two markers — a view, not an edit — that loop playback repeats and crop/delete act on. | [markers.rs:19](crates/vgms-ui/src/markers.rs) |

## State machinery

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **state patch** | n | Synthetic zero-delay register writes spliced across an edit's edge: the diff between chip state at two points. | [state_patch.rs:9](crates/vgms-core/src/state_patch.rs) |
| **prelude** | n | The patch from *blank* state placed before a cropped or extracted piece. The VGM/`ChipState` path calls the same thing a **restore**; pick per subsystem, define on first use. | [split_songs.rs:271](crates/vgms-core/src/split_songs.rs) |
| **seam** ⚠ | n | An audible join: the junction a deletion leaves (bridged by a patch), or the loop's end-to-start join (Play Seam auditions it). Both are approved; the architecture-boundary sense ("audio service seam") is project-notes jargon — avoid in code docs. | [crop.rs:15](crates/vgms-core/src/crop.rs), [vgm_engine.rs:542](crates/vgms-synth/src/vgm_engine.rs) |
| **fold** | v | Replay a span of stream into accumulated register state (`StateFold` for OPL/DRO, `ChipState::fold` for any-chip VGM). | [state_patch.rs:38](crates/vgms-core/src/state_patch.rs) |
| **cell** | n | One addressable unit of chip state: chip kind + instance + port + address. | [chip_state.rs:34](crates/vgms-core/src/chip_state.rs) |
| **latch** ⚠ | n | A register that holds its last written value (re-writing the same value is inaudible). The UI's *Custom pan latch* is the other sense — a toggle that stays lit; qualify as **latch button** in UI prose. | [opl_state.rs:4](crates/vgms-core/src/opl_state.rs), [pan_controls.rs:59](crates/vgms-ui/src/widgets/pan_controls.rs) |
| **register file** | n | One of the two 256-entry OPL register arrays (low/high). Same thing writes address as a **bank**; `OplState` says file, `Song` says bank. | [opl_state.rs:20](crates/vgms-core/src/opl_state.rs) |
| **shadow** | v/n | Keep a private copy of register state so it can be resynced or restored (the OPL adapter's `newm` shadow, the RetroWave chip's board shadow). | [opl_adapter.rs:69](crates/vgms-synth/src/opl_adapter.rs) |

## Loops, regions, splitting

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **loop point** | n | The row playback restarts at; the editor holds a row index, the file a byte offset + sample count. **Loop end** is the exclusive partner index. | [loopfind.rs:54](crates/vgms-core/src/loopfind.rs) |
| **candidate** | n | A discovered possible loop: a delay-stripped block at loop point repeating verbatim at loop end, ranked by quality flags. | [loopfind.rs:47](crates/vgms-core/src/loopfind.rs) |
| **quality flags** | n | vgmlpfnd's ranking notation: `e`, `f`, `!`, `-`. | [loopfind.rs:20](crates/vgms-core/src/loopfind.rs) |
| **wrap** ⚠ | v | One jump back from loop end to loop start during playback (`wraps_remaining`, `owes_a_wrap`). Do not use for core decorators (`Leveled::wrap`) in prose. | [vgm_engine.rs:542](crates/vgms-synth/src/vgm_engine.rs) |
| **segment** | n | One detected song inside a capture: its row range, start time, duration, trailing gap, in native units. | [split_songs.rs:32](crates/vgms-core/src/split_songs.rs) |
| **gap** | n | A run of pure-delay rows whose summed length reaches the threshold — the silence parting two songs. | [split_songs.rs:5](crates/vgms-core/src/split_songs.rs) |
| **decay tail** | n | Trimmed trailing silence given back to a materialised piece as one synthetic delay, capped at the gap that followed it. | [split_songs.rs:199](crates/vgms-core/src/split_songs.rs) |
| **materialise** | v | Lift one segment out into a standalone song, optionally prepending a prelude and appending a decay tail. The VGM path is `extract_region`. | [split_songs.rs:209](crates/vgms-core/src/split_songs.rs) |
| **split** ⚠ | v | *Split Channels*: one output per chip channel (WAV renders each channel soloed; Song format rewrites the stream into per-channel VGMs). *Split Songs* is the different operation that cuts a capture at silence — always use the two-word menu names when ambiguity is possible. | [split.rs:108](crates/vgms-synth/src/split.rs) |
| **stem** ⚠ | n | One split output: a channel's soloed WAV or rewritten standalone VGM. (Filesystem "file stem" is the unrelated sense — say *file stem*.) | [song_gate.rs:22](crates/vgms-synth/src/song_gate.rs) |
| **tail** ⚠ | n | The trailing end of audio (Play Tail, `ui.tail_length`, a release tail). Three more senses exist (wait-encoder shave, undo redo-tail) — qualify outside playback contexts. | [action.rs:150](crates/vgms-ui/src/action.rs) |

## Loudness — six words, six different gains

The most collision-dense corner of the vocabulary. Every word below is
approved *only* for its own row; using one for another is the local
equivalent of an STE violation. (The memory notes a standing gain-vs-lvl
trap; this table is its fence.)

| Term | What it scales | Where it lives | Persisted? |
|---|---|---|---|
| **boost** | The whole live mix, 0.25–64×, through the peak limiter | User's volume lever / `AudioConfig::boost` | No (transient; `lock_boost` keeps it across songs) |
| **volume modifier** | A whole song, on the spec's factor ladder | VGM header byte 0x7C | Yes — in the file |
| **trim** | One chip instance's listening level, 0–100% | The mixer deck's knob → `ChipTrims` | No — ear-only, never saved |
| **balance** | A voice's share of the file's cross-chip ratio (VGMPlay model) | `VgmEngine`, per voice | N/A (derived) |
| **level** | A core's measured output calibration vs the reference (8.8 fixed point, `LEVEL_UNITY`) | `CoreInfo` / `Leveled` wrapper | N/A (constant per core). Never the harness's *gain* column. |
| **gain** | The limiter's smoothed per-frame attenuation in (0,1] | `BoostLimiter` | N/A (instantaneous) |

Related approved terms: **ladder** (the geometric set of factors a modifier
byte can express), **Match Volume** (set boost so the measured peak reaches
full scale), **Measure** (the metadata dialog's twin that fills the modifier
field instead), **peak** (the loudest sample of an un-boosted render),
**album peak** (loudest across a pack), **engaged** ⚠ (the limiter actively
reducing gain — collides with stereo-ext engagement; qualify).

## Mixing and channels

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **channel** ⚠ | n | One voice of a chip, in the app's canonical per-chip order (`channels_of`) that mute masks, pan arrays and split filenames index. (Service delivery routes and mpsc channels are the other senses — avoid "channel" for those in audio-adjacent prose.) | [channels.rs:1](crates/vgms-core/src/vgm/channels.rs) |
| **`Muting`** ⚠ | n (type) | The OPL *document* vocabulary: 18-bit channel mask + per-bank 0xBD masks. **Set bit = audible.** | [clock.rs:72](crates/vgms-synth/src/clock.rs) |
| **`ChipMuting`** ⚠ | n (type) | The generic vocabulary: per-instance masks. **Set bit = muted** — opposite polarity to `Muting`; `opl_chip_muting` flips it. | [chip_mix.rs:34](crates/vgms-synth/src/chip_mix.rs) |
| **`Panning`** / **`ChipPanning`** | n (types) | Same pairing for stereo: OPL panpot-byte image (Original/Custom) vs per-chip position arrays (−0x100..0x100). `opl_chip_panning`/`pan_of` convert. | [clock.rs:224](crates/vgms-synth/src/clock.rs), [chip_mix.rs:96](crates/vgms-synth/src/chip_mix.rs) |
| **vocabulary** | n | The codebase's own word for a control family: "the OPL vocabulary" (`Muting`/`Panning`, spoken only by DRO documents) vs "the generic / any-chip vocabulary" (`ChipMuting`/`ChipPanning`/`ChipTrims`). Backends hold both and route the fitting one. | [retrowave.rs:34](crates/vgms-app/src/services/retrowave.rs) |
| **mute mask** | n | Bitmask with bit *i* set per muted channel of one instance. `mask_effective` folds in another chip's solo. | [chip_channels.rs:129](crates/vgms-ui/src/widgets/chip_channels.rs) |
| **solo** ⚠ | v | Chip solo (lamp right-click) is *additive* across chips; channel solo (toggle right-click) is *exclusive* within a chip. One word, two mechanics — say which. | [chip_channels.rs:74](crates/vgms-ui/src/widgets/chip_channels.rs) |
| **muted vs silenced** | adj | *Muted* = the user's own act on this chip/channel. *Silenced* = quieted as a side effect of another chip's solo (or the whole-chip engine stand-down `Voice::silenced`). Keep the distinction. | chip_channels.rs |
| **gate** | v | Filter register writes so a muted channel never sounds (drop key-ons, clear key bits, force volumes) instead of masking rendered output. Hosts: `ChannelGate`, `GatedCore`, song gate. Opposite: **native mute** (the core masks its own output). | [clock.rs:159](crates/vgms-synth/src/clock.rs), [channel_gate.rs:72](crates/vgms-synth/src/channel_gate.rs) |
| **pan image** | n | An array of per-channel pan bytes (0x00 L – 0xFF R) describing a whole stereo layout — the 18-slot OPL image, dual OPL2's hard-L/R image. | [chip_panels.rs:444](crates/vgms-ui/src/widgets/chip_panels.rs) |
| **panpot** | n | An OPL stereo-ext per-channel pan register (0xD0–0xD8 per bank), inert until the 0x105 stereo-ext enable lands. | [opl_adapter.rs:131](crates/vgms-synth/src/opl_adapter.rs) |
| **stereo-ext** | n | The OPL core's panpot extension (register 0x105 bit 1); the engine owns that bit. | [opl_adapter.rs:116](crates/vgms-synth/src/opl_adapter.rs) |
| **spread** | n | A stereo-width strength (−1..1, 0 = mono) whose one knob rewrites every pan byte from the alternating auto-spread template. | [pan_controls.rs:45](crates/vgms-ui/src/widgets/pan_controls.rs) |
| **Original / Custom** | n | The two pan modes: the chip's own stereo image rules, or the panel's knobs drive the output. | [chip_channels.rs:90](crates/vgms-ui/src/widgets/chip_channels.rs) |
| **roster** ⚠ | n | A chip's canonical ordered channel list — the index space of every mask and array. (The registry's set of cores is the other sense; say **core roster**.) | [channel_gate.rs:77](crates/vgms-synth/src/channel_gate.rs) |

## Engines, cores, playback

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **engine** | n | The one pull-based playback state machine: **`VgmEngine`** walks any VGM through `ChipCore` voices and mixes them. Every document plays through it — a DRO is projected to its VGM first (`opl_song_to_vgm_file`), so live playback, WAV render, peak scan, waveform and CLI render all share this path. `clock.rs` (formerly `engine.rs`) holds just the clock, loop and mix-vocabulary types it uses. | [vgm_engine.rs:157](crates/vgms-synth/src/vgm_engine.rs), [clock.rs:1](crates/vgms-synth/src/clock.rs), [vgms-audio-native/lib.rs:203](crates/vgms-audio-native/src/lib.rs) |
| **core** ⚠ | n | One emulator implementation of a chip (a `CoreInfo` row): "nuked", "cqm", "retrowave". A core *plays* a chip, never *is* one. Head-on collision: the crate `vgms-core` is the document model — write **the core crate** vs **a chip core**. | [registry.rs:75](crates/vgms-synth/src/registry.rs) |
| **chip** ⚠ | n | The sound device itself: `ChipKind` names the 42 spec kinds; `ChipUse` is one declared with clock and flags; an **instance** is the first/second of a dual chip. Legacy trap: `OplChip` is named chip but is an emulator (a core). | [header.rs:81](crates/vgms-core/src/vgm/header.rs) |
| **`ChipCore`** / **`OplChip`** | n (types) | The generic core trait (writes + ROM/RAM in, i32 frames out at native rate) and its OPL-only sibling (register policy kept outside). **`OplCoreAdapter`** presents an `OplChip` as a `ChipCore` so `VgmEngine` hosts the OPL family. | [chip.rs:23](crates/vgms-synth/src/chip.rs), [opl.rs:17](crates/vgms-synth/src/opl.rs), [opl_adapter.rs:58](crates/vgms-synth/src/opl_adapter.rs) |
| **`Voice`** ⚠ | n (type) | One chip instance inside `VgmEngine`'s mix: core, resampler, balance, stereo placement, trim, silenced flag. ("Rhythm voices" = the five OPL drums — the datasheet sense; qualify.) | [vgm_engine.rs:35](crates/vgms-synth/src/vgm_engine.rs) |
| **registry** | n | The priority-ordered `CoreInfo` table deciding which core plays which chip; registration order sets defaults; `promote` overrides one chip. A **provider** is a crate that registers cores. **Routed** marks an entry that builds no emulator (RetroWave — picking it swaps the audio service). | [registry.rs:490](crates/vgms-synth/src/registry.rs) |
| **slot** ⚠ | n | A config key grouping chips that share a core choice (`core.opl3=`); the whole OPL family shares the single `opl3` slot. Three other senses exist (pan-array index, task slot, document slot) — see Collisions. | [config.rs:109](crates/vgms-core/src/config.rs) |
| **slug** | n | A chip's stable lowercase id (`ym2612`) for ini keys, core ids, codec wire, worklet ABI — deliberately not derived from the display name. | [header.rs:137](crates/vgms-core/src/vgm/header.rs) |
| **render** ⚠ | v | Generate audio frames. Three senses (live pull, a core's native-rate frames, offline WAV/waveform export) plus egui painting; qualify when crossing layers. | [vgm_engine.rs:563](crates/vgms-synth/src/vgm_engine.rs) |
| **transport** | n | The realtime playback path. Builds the user's core choice as made (`core_for`), a below-realtime LLE die included — the picker label carries the warning, and a CPU that cannot keep up underruns audibly. | [chip.rs:170](crates/vgms-synth/src/chip.rs) |
| **seek replay** | n | Rebuilding chip state after a seek by rewriting every prior write with no samples rendered. `VgmEngine` folds state, then executes the restore set through each core's `replay_write` (the OPL adapter's immediate path). | [vgm_engine.rs:478](crates/vgms-synth/src/vgm_engine.rs), [opl_adapter.rs:171](crates/vgms-synth/src/opl_adapter.rs) |
| **buffered write** | n | A register write queued and spread a few samples apart during generation so Nuked observes every key edge. (`WriteQueue` is the separate hardware-output queue.) | [opl.rs:35](crates/vgms-synth/src/opl.rs) |
| **`ResampleMode`** | n (type) | The user's rate-conversion choice: Sinc (band-limited) or Linear (deliberate VGMPlay-era crunch). | [resample.rs:175](crates/vgms-synth/src/resample.rs) |
| **playability** | n | Whether a file's chips have cores: Full / Partial (missing chips render silence, warned) / None (refused). VGM-only — a DRO always plays. | [chip.rs:124](crates/vgms-synth/src/chip.rs) |
| **native** ⚠ | adj | Three senses: native *rate* (a chip's own render rate; `NATIVE_SAMPLE_RATE` = the OPL3's 49716 Hz), native *mute* (a core's built-in channel mute), native *build* (cpal/non-web). Always attach the noun. | [chip.rs:48](crates/vgms-synth/src/chip.rs) |
| **waveform** ⚠ | n | The min/max bucket overview of a song's amplitude for the scrolling display. Unrelated: OPL's *waveform select* register feature (the DRO v1 0x01=0x20 prime). | [waveform.rs:1](crates/vgms-synth/src/waveform.rs) |

## OPL chips and hardware

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **OPL2** | n | The YM3812: two-operator FM, AdLib and early Sound Blasters. | [song.rs:157](crates/vgms-core/src/song.rs) |
| **OPL3** ⚠ | n | The YMF262: stereo, extra waveforms. Trap: the config *slot* named `opl3` also governs OPL2/YM3526/Y8950. | [song.rs:159](crates/vgms-core/src/song.rs) |
| **Dual OPL2** | n | Two YM3812s giving Sound Blaster Pro stereo. In VGM: the dual bit plus dro2vgm's bit-31 stereo marker → hard-pan chip 1 left, chip 2 right. Distinct from the generic **dual bit** any chip can carry. | [song.rs:158](crates/vgms-core/src/song.rs), [header.rs:330-344](crates/vgms-core/src/vgm/header.rs) |
| **`OplType`** | n (type) | The OPL hardware a DRO targets — Opl2, DualOpl2, Opl3 — from the DRO header's `iHardwareType`. UI says *hardware type*. | [song.rs:156](crates/vgms-core/src/song.rs) |
| **clock** | n | A 32-bit VGM header field in Hz: non-zero declares the chip and sets its rate; bits 30/31 are the **dual bit** and **variant bit**. | [header.rs:317](crates/vgms-core/src/vgm/header.rs) |
| **CQM** | n | Creative's OPL3-clone synthesis (CT-4390, AWE64 era); the picker row `opl3.cqm`. | docs/readme.html |
| **RetroWave** | n | The RetroWave OPL3 board: a real YMF262 on a serial port, heard through its own jack; the core named `retrowave` in the `opl3` slot. Synonyms *board*, *hardware*, *device* — prefer **board**. | [config.rs:23](crates/vgms-core/src/config.rs) |
| **pump** | n | The OS thread walking a `VgmEngine` against the wall clock, sending writes to the board as they fall due. | [player.rs:4](crates/vgms-retrowave/src/player.rs) |
| **LLE** | n | A die simulation driven electrically through its pins; render/oracle only, never the live core. | PROVENANCE.md |
| **oracle** | n | A reference program (VGMPlay, Mesen2) run separately and compared against; never linked in. | PROVENANCE.md |
| **corpus** | n | The collection of real VGM/DRO files cores are measured against. | PROVENANCE.md |

## UI and theming

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **deck** ⚠ | n | The per-chip control cluster: the mixer strip (lamp, trim knob, name per chip) plus the selected chip's channel panel. (Theme *deck* = the surface the pads sit on; pack *output deck* = the export controls. Qualify.) | [chip_panels.rs:1](crates/vgms-ui/src/widgets/chip_panels.rs) |
| **strip** | n | The always-drawn row of chip cells atop the deck, wrapping instead of scrolling. | [chip_panels.rs:305](crates/vgms-ui/src/widgets/chip_panels.rs) |
| **cell** ⚠ | n | One chip instance's unit in the strip: lamp, trim knob, name. A dual chip is two cells. (Tests say "tabs" because the name draws in tab chrome — prefer cell.) | [chip_panels.rs:470](crates/vgms-ui/src/widgets/chip_panels.rs) |
| **lamp** | n | A domed LED dot reporting state; the chip lamp is also a control (left-click mutes the chip, right-click solos). Code says `led`; prose and accessibility labels say lamp. | [theme/mod.rs:343](crates/vgms-ui/src/theme/mod.rs) |
| **knob** | n | A drag-driven rotary control (270° amber arc). Three kinds: pan, spread, trim. Trim rests fully lit at 100%; pan rests unlit at centre. | [pan_knob.rs:1](crates/vgms-ui/src/widgets/pan_knob.rs) |
| **pad** | n | A button styled as a raised instrument keycap. | [theme/palette.rs:40](crates/vgms-ui/src/theme/palette.rs) |
| **plate / well** | n | The brushed-metal fascia surface panels sit on / the dark recessed readout surface (tab wells, selector wells). Opposites in the physical metaphor. | [theme/mod.rs:150](crates/vgms-ui/src/theme/mod.rs) |
| **case / skin / theme / palette** | n | Ordered precisely: a **case** is one named fascia colour set (`CaseColors`); a **skin** is a case paired with the fixed hardware colours; the **palette** is the flat composed form widgets consume; **theme** (`ThemeChoice`) is the config enum naming a case. Do not interchange. | [theme/palette.rs:6-9](crates/vgms-ui/src/theme/palette.rs) |
| **chrome** ⚠ | n | App chrome (the framing UI around the document) vs painted chrome ("tab chrome", the bevelled style). Qualify. | [action.rs:399](crates/vgms-ui/src/action.rs) |
| **action** | n | A request the UI emits during the frame and the app processes from a queue afterwards. | [action.rs:44](crates/vgms-ui/src/action.rs) |
| **tab** | n | A view-switching label. Three tiers: the AppTab strip (Editor/Pack), pack section tabs, and chip cells in tab chrome. | [action.rs:26](crates/vgms-ui/src/action.rs) |
| **focus** | n | The row the keyboard acts from (selection focus; pack focused track). Distinct from egui keyboard focus. | [selection.rs:27](crates/vgms-ui/src/selection.rs) |
| **readiness / verdict** | n | The pack's submission-check model (Error / Warning / Note) and the output deck's one-line worst-severity summary. | [pack/state.rs:719,908](crates/vgms-ui/src/pack/state.rs) |
| **latch button** | n | A toggle that stays visibly engaged until clicked off (Custom pan latch, Album latch, volume Lock). | [pan_controls.rs:59](crates/vgms-ui/src/widgets/pan_controls.rs) |

## Services and platform

| Term | POS | Approved meaning | Source / notes |
|---|---|---|---|
| **service** | n | A platform-abstraction trait (file, audio, task, pack, config) the GUI polls each frame; native and web implementations injected at boot. | [lib.rs:3](crates/vgms-app/src/lib.rs) |
| **poll / stash** | v | The result-delivery pattern: async outcomes are stashed, then drained by a poll on the next frame. | [file.rs:4](crates/vgms-app/src/services/file.rs) |
| **task** ⚠ | n | A cancellable background computation keyed by `TaskKind` (thread natively, Worker on web). The pack export calls its near-identical machinery a **job** — an unapproved synonym pair to be aware of. | [task.rs:1](crates/vgms-app/src/services/task.rs) |
| **generation** | n | A counter bumped per submission and cancel; stale-generation results are dropped. | [task.rs:46](crates/vgms-app/src/services/task.rs) |
| **backend** ⚠ | n | The audio output route — Emulated (cpal) or RetroWave. (`ArchiveBackend` is an unrelated file-service router.) | [retrowave.rs:275](crates/vgms-app/src/services/retrowave.rs) |
| **worklet vs Worker** | n | The AudioWorklet wasm module renders playback audio; the Web Worker runs background tasks and pack jobs. Different browser contexts, different rules. | [player.rs:2](crates/vgms-synth-worklet/src/player.rs), [codec.rs:5](crates/vgms-web/src/codec.rs) |
| **codec** ⚠ | n | The hand-rolled little-endian byte encoding across the Worker boundary. *Not* an audio codec. | [codec.rs:2](crates/vgms-web/src/codec.rs) |
| **runner** ⚠ | n | Three unrelated runners exist (web boot module, `run_task`, the vgmtools pipeline parameter). Always qualify. | [runner.rs:2](crates/vgms-web/src/runner.rs) |
| **tool / pipeline / command module** | n | One of vgmtools' three optimiser programs; the target-independent stage ordering; a wasm32-wasip1 build of a tool run like a process. | [vgms-vgmtools/lib.rs](crates/vgms-vgmtools/src/lib.rs) |
| **device** ⚠ | n | The open serial-port handle to the board (parked between songs). The cpal sound card is the *audio device* — qualify. | [retrowave.rs:31](crates/vgms-app/src/services/retrowave.rs) |
| **source** | n | A document handed to an engine or task as an Opl/Vgm pair. One shape, five names (`AudioSource`, `WavSource`, `SplitSource`, `SplitTaskSource`, `LoopSearchSource`) — all aliases of `DocSource`; prefer **DocSource** in new prose. | [codec.rs:462](crates/vgms-web/src/codec.rs) |
| **scan** ⚠ | v | Read a pack folder's relevant files. The *volume scan* is the unrelated peak-measuring render task — use the full compound. | [file.rs:257](crates/vgms-app/src/services/file.rs) |

---

## Collisions — one word, many meanings

The complete list. For each: the approved reading(s), and what to write
instead for the others.

| Word | Meanings found | Rule |
|---|---|---|
| **OPL** | ① chip family ② "a DRO document" (`DocSource::Opl`, `LoadedSong::Opl`, `Editor::song`) | ① only. ② is scheduled for rename ([PLAN](docs/dro-arm-2026-08/PLAN.md)). |
| **trim** | ① mixer per-chip gain ② crop/delete heritage sense ③ auto-trim on load ④ ROM trim (`vgm_sro`) | ① unqualified. ② forbidden — write **crop**/**delete region**. ③ ④ fixed compounds. |
| **song** | ① `Song` the DRO type ② any loaded document (prose) ③ a segment in a capture ④ `SplitFormat::Song` (same-format stems) | Type name as-is (rename to `DroSong` planned). Prose: prefer **document**/**track**/**segment**. ④ fixed label. |
| **command** | ① VGM stream element ② undoable edit (`UndoableCommand`) | ① unqualified; ② write **undo command**. |
| **instruction / command / row** | one stream element, three words | **row** when format-neutral; instruction = DRO, command = VGM. |
| **bank** | ① OPL register bank ② PCM data-block bank ③ MultiPCM bank registers ④ "bank of latches" | ① unqualified. ② write **data-block bank** / PCM bank. ③ spec name. ④ avoid. |
| **core** | ① chip emulator ② the `vgms-core` crate | ① unqualified; ② write **the core crate**. |
| **chip** | ① silicon model / declared use ② `OplChip` the emulator trait ③ old docs' word for an OPL bank | ① only. ② legacy type name. ③ dead — the docs need the rewrite. |
| **capture** | ① a recorded session ② taking an undo snapshot | ① noun only; ② write **snapshot** (v). |
| **delay / wait** | same concept, DRO vs VGM spelling | **delay** generically; **wait** when quoting the VGM spec. |
| **description** | ① pack description file ② undo label ③ table changed-fields column ④ a register's static name | Qualify all four: pack description, undo label, Description column, register name. |
| **tail** | ① audio tail (Play Tail, decay/release) ② wait-encoder shave ③ undo redo-tail ④ `ui.tail_length` | ① unqualified; others qualified. |
| **slot** | ① core-choice config key ② pan-array index 0..18 ③ task-service slot ④ document slot (editor field) | ① unqualified; write **pan slot**, **task slot**, **document slot**. |
| **seam** | ① deletion junction ② loop join ③ architecture boundary | ① ② approved (both are audible joins); ③ project-notes only. |
| **snapshot** | ① stream/undo snapshot ② audio's immutable Song copy ③ kittest PNG baselines ④ E2E state dumps | ① ② unqualified in their modules; ③ write **snapshot test**; ④ E2E snapshot. |
| **latch** | ① register that holds its value ② stay-lit toggle button | ① unqualified; ② write **latch button**. |
| **native** | ① native rate ② native mute ③ native (non-web) build | Always attach the noun. |
| **gain / level / balance / trim / boost / volume modifier** | six different scalers | See the [Loudness table](#loudness-six-words-six-different-gains). Never interchange. |
| **engaged** | ① stereo-ext enabled ② limiter reducing gain | Qualify: stereo-ext engaged / limiter engaged. |
| **port** | ① register bank (VGM) ② serial port ③ a Rust port of C code | ① unqualified in stream contexts; ② **serial port**; ③ avoid — write "Rust port". |
| **projection / project** | ① `OplProjection` view (VGM read as OPL rows) ② DRO→VGM conversion (`opl_song_to_vgm_file`) ③ a DRO's presentation as generic chips (UI) | ① **the OPL projection**. ② the verb **project** (a conversion, despite the name). ③ write **the DRO's chip projection**. |
| **render** | ① live pull ② core frames ③ offline export ④ egui paint | Qualify across layers; ④ write **paint**. |
| **stream** | ① document command stream ② cpal output stream ③ DAC stream (0x90–0x95) | ① unqualified; ② **output stream**; ③ **DAC stream**. |
| **channel** | ① chip voice ② file-service delivery route ③ mpsc | ① unqualified in audio contexts; ② ③ qualify. |
| **task / job** | same machinery, two names (tasks vs pack jobs) | Keep per subsystem; do not coin new "jobs". |
| **pack / package / archive / project** | the submission, its header spelling, the zip store, the working folder | **pack** / (spec quote only) / **archive** / **pack folder**. |
| **image** | ① pan image ② screenshot PNG ③ `OptimizedImage` | Qualify: pan image / screenshot. |
| **wrap** | ① loop jump ② decorating a core | ① unqualified; ② write **wraps** only in code, prose says "decorates". |
| **roster** | ① channel order ② registered cores | ① unqualified; ② **core roster**. |
| **`Bank`** (type) | vgms-core's Low/High vs vgms-retrowave's protocol type | Same name, two crates — qualify with the crate in cross-crate prose. |
| **`Muting` vs `ChipMuting` polarity** | set bit = *audible* vs set bit = *muted* | The single nastiest trap in the mix layer. Never copy a mask across without `opl_chip_muting`/`opl_muting_from_chip`. |
| **theme / case / skin / palette** | four theming layers | Use per the [UI table](#ui-and-theming). |
| **chrome** | app framing vs painted style | Qualify. |
| **document / song / source / file** | one loaded thing, four words by layer | **document** in the editor, **source** (`DocSource`) at job boundaries, **file** for on-disk bytes. |
| **VGM Studio / DRO Trimmer** | the product's current vs shipped-docs name | **VGM Studio** everywhere; the user docs still say DRO Trimmer and need the rewrite (see [DIVERGENCE.md](docs/dro-arm-2026-08/DIVERGENCE.md)). |

---

*Maintenance: when you add a public type or coin a word in a doc comment,
check this dictionary first. If the word is taken, qualify it or pick
another. If the concept already has a term, use that term. New entries go in
the section they belong to; new collisions go in both places.*
