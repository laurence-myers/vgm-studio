# HANDOVER — Any-chip VGM support (Phases A–C and the unification shipped; playback next)

> **2026-07-27, later:** the core-emulation programme (what mc-8/mc-9 sketched)
> now has its own plan — **`CORES-PLAN.md`** in this directory — locking a
> license split (GPL-2.0-or-later app, MIT/Apache vgms-synth + vgms-core), a
> per-chip runtime core registry, a no-vendoring submodule policy, and
> Nuked-CQM as an OPL3 option. Read it before touching cores; the §7 audit
> table below is superseded by the licensing analysis it encodes.

> **Second revision, 2026-07-26 (evening).** After mc-1..mc-5 shipped, the
> user redirected the plan with four directives, folded in below as locked
> decisions §2.1.13–17 and a new phase (uv-1..uv-5, §Phase C2) that now
> precedes playback:
>
> 1. **No "OPL vs Foreign".** An OPL VGM is just a VGM whose chips unlock
>    *additional* features. One abstraction holds any chip's data; features
>    check the chips. The `Song`-vs-`VgmFile` split for VGMs (and the word
>    "foreign") is transitional and gets dissolved (uv-1).
> 2. **Crop / delete-marked-region is a hard requirement for every VGM**
>    (uv-2, uv-3) — with license to rewrite the OPL-specific machinery it
>    generalises, and undo kept to deltas, not per-row snapshots.
> 3. **Optimize (vgm_cmp) for every chip**, not just OPL (uv-4).
> 4. **An explicit option to correct headers that disagree with the stream**
>    (uv-5; `vgm_ptch -Check` is the reference).
>
> vgmtools was re-surveyed at source level for 2–4; findings in §3.9.

> **Progress (branch `vgm-multichip`)**
>
> | Step | State | Commit |
> |------|-------|--------|
> | mc-1 | done | `c6db120` — `vgm/header.rs` (42-chip model, extra header), `vgm/file.rs` (`VgmFile`, opaque body, byte-exact retag writer) |
> | mc-2 | done | `13237e6` — pack mode accepts a VGM for any chips (later unified to one `PackSong::Vgm`) |
> | mc-3 | done | `99d0b20` — the explaining dialog + gated editor actions (later `LoadFailure::Unwalkable`) |
> | mc-4 | done | `030805c` — `vgm/stream.rs` (full opcode table, version-aware sizing, typed decode), `VgmBody::Commands`, OPL reader accepts minimal headers |
> | mc-5 | done | `be409cd` chip-selector deck, `524df5f` delete + undo + header repatch, `428c57e` the editor for any chips, `3a1165f` chip deck visible + pack gating flipped |
> | uv-1 | **done** | `621a90c` `OplProjection` + the corpus parity gate, `83cb3fe` pack mode unified, `fa62b93` terminology, `7aba760` the Find Loop dialog + every edit gate, `5927d5c` the splitter, `4fa1914` tag/loop metadata, `8af87a6` **the document-model collapse**, `1318b4c` Convert to VGM, `186b769` the branches it made unreachable, `a2479aa` one optimiser, `8a7508c` the naming |
> | uv-2 | done | `e83d8fa` — `chip_state`: generic latch model + block/DAC-stream/seek state, fold-equivalence proptests |
> | uv-3 | done | `e83d8fa` core, `45e7a69` UI — **crop and delete-region for every VGM** |
> | uv-4 | done | `b5e839d` — optimise for every chip it has rules for, conservative default, OPL byte-parity kept |
> | uv-5 | done | `e9afd76` — `vgm::audit` + Edit > Fix Header, user-invoked only |
>
> **All four of the 2026-07-26 directives are implemented** (§2.1.13–17), and
> so is the internal half of uv-1. **Phase C2 is complete.**
>
> ### What the unification actually looks like (finished 2026-07-27)
>
> The editor holds one document: a `Song` for a DRO, a `VgmFile` for every VGM.
> OPL is a **projection** of a VGM -- a `Song` rebuilt from the stream whenever
> the stream changes, served by `Editor::song()` and read by the register
> analyser, Find Register, the waveform and the synth exactly as before. Three
> rules keep it honest:
>
> - Every edit bumps the revision through **one method**, which refreshes the
>   projection and drops the row-analysis cache. Nothing can be an edit behind.
> - Anything that asks "what does this file say" reads the **file**, never the
>   projection: the tag, the loop, the header. The projection is a view of the
>   *stream*.
> - The table branches on whether there is an OPL reading, not on which slot
>   holds the document. (It branched on the slot at first; the kittest snapshots
>   caught it.)
>
> **The user-visible payoff is fidelity.** Saving used to go through the OPL
> writer, which rebuilds a header from the decoded song -- so a round trip could
> re-stamp a clock, drop a longer header, or quietly correct a sample total that
> disagreed with the stream. A save now returns the file's own bytes, and
> correcting a header is something the user asks for by name.
>
> **A wrinkle worth carrying forward:** a VGM header stores a loop's *length in
> samples*, so a loop end sharing its instant with the rows before it comes back
> as the first of them. `apply_loop_to_metadata` re-derives the markers from what
> was stored rather than leaving them where the user put them -- otherwise the
> "unapplied" cue stays lit on a loop that has just been applied. The `Song`
> model used to keep the asked-for index in memory and lose it on save; this is
> the same information loss, made visible immediately.
>
> **Gates split in two.** `require_song` asks for an OPL stream (playback, the
> WAV render, the channel split, the register analyser, Go To's delay
> navigation); `require_document` is for everything that is not an OPL question
> (save, close, undo/redo, delete, crop, delete-region, go-to, apply-loop, both
> metadata dialogs, Find Loop, Split Songs). Every action went through the former
> before, so a VGM for other chips opened in the editor and then declined to be
> saved or edited at all. The File menu hides Render to WAV and Split Channels
> for such a document rather than offering an item that answers "please open a
> DRO file first".
>
> **What was retired with it:** the OPL-slot half of six editor operations (167
> lines), the CLI's and the pack export's separate OPL optimiser paths, and the
> word "foreign" from code and copy. `ForeignVgmDialog` became
> `UnwalkableVgmDialog` -- with every VGM openable, the only one that still needs
> an explaining dialog is one whose commands will not walk.
>
> ### mc-6: the engine is built, the cores are not (2026-07-27)
>
> `vgms-synth` gained four modules and no emulation:
>
> - **`chip.rs`** — the `ChipCore` trait (reset, write, ROM, RAM, render at your
>   own rate), `core_for` (the registry, empty), `playability`, and
>   `RecordingChip`. `OplChip` stays: the OPL player has register policy --
>   muting, panning, Nuked's buffered-write spacing -- that belongs nowhere near
>   a generic engine.
> - **`banks.rs`** — what a `0x67` type byte means, and the sample banks kept by
>   type and arrival order (`0x95` addresses "the nth block of this type"; a
>   `0x91` binding addresses the type's whole concatenated run).
> - **`dac_stream.rs`** — all six `0x90`-`0x95` commands. Chip-agnostic in the
>   spec and here: it says when a byte is due and where it goes, and the engine
>   writes it. Serviced once per *output frame*, which is what makes it
>   independent of the command stream's clock.
> - **`decompress.rs`** — the `0x40`-`0x7E` blocks. Bit packing (copy, shift,
>   table) and DPCM, against the shared `0x7F` table. Route B, from the spec; the
>   net is a packer in the tests so every scheme round-trips.
> - **`vgm_engine.rs`** — `VgmEngine`: routing (dual-chip instances, per-chip
>   ports), the banks, ROM/RAM delivery, `0x64`'s wait redefinition, per-chip
>   linear resampling into one mix, the `render(&mut [i16]) -> usize` pull
>   contract, and `seek_to_row` via `chip_state`.
>
> **`with_cores` is how it is tested.** Routing, banks, ROM delivery and DAC
> timing are assertions about what reached which chip, which a test core answers
> without an emulator -- and keeps answering when the real ones arrive and start
> disagreeing about samples.
>
> **`chip_state` now has all four intended consumers**: crop, the optimiser, the
> splitter's prelude and this seek. A seek and a crop agree by construction about
> what "the state at row N" means.
>
> **The engine corpus** (`crates/vgms-app/tests/engine_corpus.rs`) drives all
> 16461 openable files: 146 hours rendered, every one playing and seeking for
> exactly as long as its own waits say. It bounds each file at 20 s and logs how
> many ran to their end (4418) rather than leaving the cap implied.
>
> ### The first core: an SN76489, written not vendored (2026-07-27)
>
> `cores/sn76489.rs`, registered in `core_for`. Three square waves and a noise
> channel: the latch/data register protocol and its ten-bit periods, the
> counters, the zero-period case games play samples through, the 16-bit shift
> register (tapped at bits 0 and 3, white or periodic, restarted by a write), the
> noise rate that follows tone 2, and four bits of attenuation. Not modelled: the
> Game Gear's stereo register and the T6W28's split addressing.
>
> **Route B, deliberately.** The plan allowed vendoring libymfm.wasm's BSD-3 port
> instead; writing it kept the copy-and-attribution question off the table
> entirely, and this chip is simple enough that the documented behaviour *is* the
> implementation. Every number is derived in a test rather than transcribed: the
> volume table is recomputed from "2 dB a step, and the last step is off", and a
> tone's pitch is counted in rising edges against `clock / (32 x period)`.
>
> **The licensing question is still live for the next two cores, and it is the
> user's.** `rust-synth-emulation`'s `ym3438.rs` is GPL-2.0, and vendoring it
> relicenses the project — approved in principle 2026-07-20 (§2.1), but worth
> *asking* before it happens, because the alternatives keep the choice open: a
> hand-port of Nuked-OPN2 is LGPL-2.1, emu2413 (YM2413) is MIT, and libymfm.wasm's
> ports are BSD-3. Nothing so far has forced the move.
>
> **Acceptance for a core is an A/B against VGMPlay, which is a thing a person
> does.** The tests here pin documented behaviour, not fidelity; treat a new core
> as unverified until someone has listened to it.
>
> ### The WAV render went first, on purpose (2026-07-27)
>
> A render is offline, so it needs none of the real-time audio service, the
> hardware backend or the per-chip output settings. `render_vgm_wav` sits beside
> `render_wav_mixed` and they share one write loop, so the OPL render is
> byte-for-byte what it was. No muting or panning on the generic path -- those
> are OPL ideas and `VgmEngine` has no register policy to do them with.
>
> `DocCapabilities` grew `renderable` (an OPL stream, or a chip with a core)
> alongside `playable` (an OPL stream). The File menu asks both: Render to WAV
> follows the first, Split Channels the second. When live playback lands, the
> transport should follow a third question -- "can the *audio service* play
> this", which is backend-aware because RetroWave is OPL-only (§3.7).
>
> ### Where live playback got to (2026-07-27)
>
> `AudioService::load` takes a `vgms_synth::AudioSource` (OPL `Arc<Song>` |
> `Arc<VgmFile>`), and `NativeAudio` holds whichever engine that implies behind
> one callback via a private `Engine` enum. The callback needs five things from
> an engine -- render, seek, rewind, position, finished -- and everything else it
> can be told is OPL register policy, so muting and panning are no-ops on the
> generic arm rather than errors.
>
> Two routing rules, both tested: the RetroWave service refuses a source it
> cannot play (naming the file), and `SwitchingAudioService` sends a non-OPL
> source to the emulated output whatever the setting says.
>
> **It plays.** `render_vgm_waveform_progressive` unblocked the last step, so
> `DocCapabilities::playable` now means "something would be heard" -- an OPL
> stream, or a VGM with a chip there is a core for -- and a Master System rip
> opens with its transport, waveform, position readout and peak meter.
>
> **Three capability questions where one used to do**, and they were the same
> answer for every document until a non-OPL chip became playable:
> `playable` (the transport), `renderable` (the WAV export -- the same question,
> but a render needs no output device), and "is there an OPL stream"
> (Split Channels, which decides which OPL channel a register write belongs to).
> The transport actions gate on `require_playable`, between `require_song` and
> `require_document`.
>
> **mc-7 is done.** The per-chip output settings landed as
> `widgets/chip_output.rs`: one row per chip this app can play, OPL keeping the
> choice it always had (with the device picker under it), a chip with one core
> stating it, and a closing line counting the chips with none. The rows come from
> `vgms_synth::core_for`, so a new core appears without anyone adding it, and a
> test checks the rows plus the count still cover the whole chip table.
>
> One deviation from the plan, deliberate: the config key stays the flat
> `audio.output_backend` rather than `audio.output.opl`. It *is* the OPL row's
> key -- the board is an OPL3 -- and a per-chip key format while only one chip has
> a second backend would be a migration for no behaviour. It generalises when a
> second chip has somewhere else to go.
>
> Pack preview plays any track with a core, and the checklist's note changed
> shape with it: `unplayable_chips` became `silent_chips`, because a Mega Drive
> rip is *partly* playable (PSG yes, FM no) and the useful thing to say is which
> chips would be missing from the preview, not that there cannot be one.
>
> `VgmEngine` loops, with the OPL player's three decisions carried over: no chip
> reset at the seam, `frames_rendered` rewinds to the loop start, and a region
> that renders no audio is dropped rather than spun on. Auditioning a seam works
> for either document.
>
> The greying rules follow the document: hardware output means the board mixes
> its own sound and nothing here can meter or shape it, but a non-OPL document
> never reaches that board, so its meter, boost and panning stay live.
>
> ### The version computation, and the bug it found (2026-07-27)
>
> `vgms_core::vgm::version` answers what a file actually requires -- its chips,
> its commands, its header fields, against a 1.50 floor -- plus `can_downgrade_to`
> and `blockers` for the normalise operation mc-10 still wants.
>
> Pointing it at the corpus found a **reader bug**: `read_chips` took any
> non-zero clock field inside the header span, even one the declared version
> predates. A 1.70 file's header runs to 0x100 whether or not the spec had
> assigned 0xC0 yet, so ~155 rips were reported with chips their streams never
> write to. The version is now enforced as strictly as the data-start rule, and
> the module doc says why. **The old comment justifying the permissive read was
> wrong, and had been for as long as it existed** -- worth remembering when a
> tolerance is argued for on the grounds that "the bytes are unambiguous".
>
> Only the *under*-claiming direction reached the audit: 20 corpus files use a
> 1.60 command while calling themselves 1.51, which a player trusting the version
> may mishandle. Over-claiming is near-universal (109 could be stamped lower,
> 14436 exact) and would turn Edit > Fix Header into a nag. The engine corpus
> reports all three counts every run.
>
> **What is left of mc-10:** shrinking an over-claimed header to its version's
> size bucket. That means moving every relative offset in the header (GD3, loop,
> data, EOF), which is why it is separate from restamping the version field --
> readers find the data through the data-offset field, not by assuming a size.
>
> ### Porting notes, kept because they generalise
>
> Every `&Song`-shaped stream rebuilder now has a `VgmStream` equivalent:
> `merge_delays` (`b4cdc31`), the loop finder (`d9e39a7`), and the multi-song
> splitter (`5927d5c`). `convert::filter_vgm` was **not** ported and does not
> need to be: it backs the channel split, which is an OPL-only idea, and a
> projected `Song` feeds it.
>
> **Diff every port against the OPL implementation over the whole corpus, not
> just unit fixtures.** Both `merge_delays` bugs were caught that way rather than
> by reasoning, and one only at scale (`optimize()` rebuilt unconditionally, so
> 1121 of 3933 already-optimal files came back re-spelled at the same length).
> The splitter's diff is in `projection_corpus.rs::compare_split`: every segment
> boundary must match exactly, and every piece must agree on final chip state and
> length -- *not* on bytes, because the two state preludes emit the same writes
> in different orders and the generic one also restores a register explicitly
> written to zero, which the OPL fold skips as unchanged from a blank chip.
>
> Notes worth carrying into whatever comes next:
> - **`chip_state` has four intended consumers, three connected.** Crop,
>   optimise and the splitter's prelude use it; mc-6's fast seek still does not.
>   The OPL-specific `state_patch::StateFold` and `crop.rs` still serve the DRO
>   path, which is the only caller left.
> - **`optimize::optimize(&Song)` is now reference-only.** Nothing in the app
>   reaches it; it exists as the corpus oracle. Same for the OPL splitter's
>   `materialise`, which the DRO path still uses.
> - **The optimiser's rule table is the thing to grow.** Four chips have
>   rules (OPL family, YM2612, YM2413). Adding one means checking its
>   trigger registers against `chip_cmp` and adding a line; not adding one
>   costs only compression. Do this alongside the mc-9 core waves, where the
>   chip is being studied anyway.
> - **The YM2612/YM2413 exclusions are cautious, not proven.** `0x2A`, `0x28`
>   and `0x20`-`0x28` are excluded because a wrong answer there is silent.
>   Worth revisiting with a render oracle once those cores land (mc-8).
>
> **What the corpus said** (16466 files, `VGMSTUDIO_CORPUS`): 3933 the old OPL
> reader accepts, all 3933 now agreeing byte-for-byte through the
> projection; **12533 openable that were not before**; 0 unreadable by both.
> Of the OPL-accepted files, 0 carry a non-OPL command — so the strict
> "wholly OPL" gate costs nothing real, and the corpus is what caught the
> two files whose headers declare a chip their stream never writes to.
> The splitter agrees with the OPL one on every segment boundary in those
> 3933 files and on all **11388 pieces** they yield.
> | mc-6 … mc-10 | after the uv steps (playback, cores, min-version writer) | |
>
> **Phases A, B and C are complete.** A pack containing any VGM — any chip,
> versions 1.00–1.72 — opens, lists, tags, renames, levels and exports; and
> a foreign VGM now *opens in the editor* for trimming: rows named by chip,
> selection, delete, undo and save, with the header's totals and loop kept in
> step. **Any-chip trimming works, with no emulator.** What remains is
> playback (mc-6 onward) and the minimum-version writer (mc-10).
>
> Notes from the mc-5 build (the second-slot shape below is **transitional**
> — uv-1 replaces it with the `EditorDoc` enum, which becomes honest once the
> split is DRO-vs-VGM rather than OPL-vs-foreign):
> - The editor holds a **second document slot** (`foreign: Option<VgmFile>`)
>   rather than the `EditorDoc` enum this plan proposed. At most one slot is
>   filled. `UndoController`/`UndoableCommand` were already generic, so the
>   foreign document brings its own stack and the undo machinery was untouched.
> - The instruction table asks the editor for `row_cells(i)` and
>   `column_titles()` instead of reaching into a `Song`, so it does not know
>   which document kind it draws. A foreign row's second column is the chip.
> - `DocCapabilities::playable` is false **only** for a foreign document —
>   with nothing loaded it stays true, because an empty editor has always shown
>   its (greyed) transport. The controls deck itself still draws for a foreign
>   document; only its playback half is dropped, so the chip selector shows.
> - mc-3's dialog survives for one case: a stream that will not walk has no
>   rows, and saying what the file is beats an empty table.
>
> Notes from the build worth carrying forward:
> - `read_track` in `vgms-ui/src/pack.rs` tries the OPL reader first and the
>   chip-agnostic one second. An OPL VGM the *command table* rejects (a 0x67
>   data block, say) therefore lands as `Foreign` today — readable and
>   taggable, not editable. mc-4 moves those back to `Opl`.
>   That is half of the "real PC-AT packs" TODO already delivered.
> - Foreign files must never be written by `vgm::io::write`, which re-derives
>   the OPL clocks from `Song::opl_type`. `PackTrack::retagged`/`revolumed`
>   dispatch to the right writer; keep any new save path going through them.
> - The pack export's optimiser gate needed no code change (it fails safe on
>   an unreadable file) but its log line did: a foreign VGM now reads
>   "YM2612 is not optimised yet" rather than "could not read".
>
> **Two things mc-4 deliberately left for later, against the plan text below:**
> - *The compressed-block decompressor* (§3.3) moved to **mc-6**. mc-4 steps
>   over compressed blocks whole and preserves them, which is all a trimmer
>   needs; decompressing one is only required to *play* it, so it belongs with
>   the engine that consumes it.
> - *The OPL reader's 0x67 support* moved to **mc-5**. `VgmData`/`Song` model
>   every command as a `Instruction` (register write, delay, bank switch),
>   and a data block is none of those — so accepting one means growing that
>   enum, which ripples into the table, the analyser, the optimiser and the
>   state fold. mc-4 closed the *other* half of that TODO (minimal headers),
>   which needed no model change. Until mc-5, an OPL VGM carrying a 0x67 block
>   opens in pack mode as `Foreign`: readable and taggable, not editable.

**For:** a fresh Claude Code session implementing multi-chip VGM support.
**From:** the planning session, 2026-07-20; revised against the tree 2026-07-26.
**Repo:** `I:\Code\Python\vgm-studio` · **branch `vgm-multichip`** (created
2026-07-26 off `rust` at `10138c0` — the user asked for this work on its own
branch; land each step there and merge back to `rust` at agreed milestones).
The Python original is gone from the tree (removed 2026-07-21); there is no
parity oracle and no `src/` to avoid touching.
**Status:** plan complete; **no code written.** Updated 2026-07-20 after user
follow-ups: GPL relicense approved, libvgm assumed GPL and promoted to default
porting source, generic editor moved ahead of playback (all locked in §2.1).
**Revised 2026-07-26** after re-reading the codebase (~150 commits of drift)
and two new user requirements: per-chip output settings and chip-scoped
editor panels (§2.1.10/11). The big drift, folded into the sections below:
RetroWave OPL3 **hardware output** shipped — a second audio backend the
playback phases must stay compatible with (§3.7); an in-app vgmtools suite
shipped — optimiser/vgm_cmp, volume/vgm_vol, loop finder/vgmlpfnd, song
splitter/vgm_sptd (§3.8); crop/delete-region shipped on an OPL state-fold
patch mechanism (§3.8); the project **relicensed to LGPL-2.1-or-later**, not
GPL (§2.2.2); and pack mode grew sub-tabs, bulk tagging, screenshots, and a
submission-readiness checklist (mc-2 rewritten to match). Confirm the
remaining §2.2 recommendations with the user, then begin at mc-1 (§6),
following the workflow rules in §4.

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
4. Extend the editor to other chips — ~~deleting instructions only~~
   **(scope widened 2026-07-26: deleting, plus crop/delete-marked-region as a
   hard requirement — §2.1.14)**, plus basic playback once cores exist.
   OPL-specific extras (volume boost, panning, channel muting) explicitly
   need **not** be generalised.
5. When writing a VGM, emit the **minimum version** the content requires
   (e.g. WonderSwan forces v1.71, a YMF262-only file needs only v1.51).
6. **(2026-07-26)** One VGM model: OPL chips are one kind of VGM chip with
   additional feature support, never a separate document type (§2.1.13).
7. **(2026-07-26)** The `vgm_cmp`-style optimiser supports every chip it has
   rules for, and passes the rest through untouched (§2.1.15).
8. **(2026-07-26)** The user can correct a header that disagrees with its
   command stream — explicitly, never silently (§2.1.16).

Spec: <https://vgmrips.net/wiki/VGM_Specification> (§3 digests everything the
implementation needs; trust the live spec over §3 if they disagree).

This plan also subsumes two existing `TODO.md` bullets: extending the reader
for real PC-AT packs (0x67 data blocks, the "data starts at 0x60" minimal
header) and emitting a higher-version header when there is something to put in
it. Both bullets verified still open, 2026-07-26.

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
   vgm-studio is itself a non-commercial project. Caveat recorded in §7:
   a non-commercial clause is a *further restriction* under the GPL, so
   Genesis-Plus-GX-derived code cannot ship in the same binary as the GPL
   cores; it serves as behaviour reference and test oracle instead.
10. **Per-chip output settings** (2026-07-26): the Settings dialog's single
    "Output" row (Nuked OPL3 emulated vs RetroWave OPL3 hardware) is a
    chip-specific choice wearing a global name. Replace it with a per-chip
    widget: one row per chip kind, each offering that chip's available
    backends — today only OPL2/OPL3 has a hardware option; every other chip
    is emulated-only (or nothing until its core lands). Detailed in mc-7.
11. **Chip-scoped editor panels** (2026-07-26): the editor panel holding the
    channel panning knobs + channel selector/mute buttons is OPL3-specific.
    Wrap it in a widget with a **chip selector** offering one entry per chip
    present in the loaded VGM; each chip contributes its own panel (OPL
    keeps the existing panel unchanged; other chips start empty — §2.1.4
    still holds, their extras need not be generalised). Detailed in mc-5.
12. **Work on branch `vgm-multichip`** (2026-07-26), not directly on `rust`.
13. **One VGM model** (2026-07-26, supersedes §2.2.4): every VGM is one
    document type; OPL is a *capability* of its chip set, checked before an
    OPL feature enables, never a separate type. The mc-5 editor's
    song/foreign split and pack mode's `PackSong::Opl/Foreign` naming are
    transitional and get dissolved in uv-1. The word "foreign" leaves the
    codebase and the UI copy.
14. **Crop and delete-marked-region work for every VGM** (2026-07-26) — a
    hard requirement, not an OPL nicety. The OPL-only state machinery
    (`OplState`, `state_patch::StateFold`, `crop.rs`) may be rewritten or
    replaced to get there (uv-2/uv-3).
15. **Optimise every chip** (2026-07-26): the vgm_cmp equivalent gains
    per-chip redundancy rules; a chip with no rules yet passes through
    verbatim, never corrupted (uv-4).
16. **Header-vs-stream corrections are user-invoked** (2026-07-26): the app
    audits and *offers* the fix (editor action + pack checklist); the writer
    never recomputes a field the user didn't ask it to. `vgm_ptch -Check` is
    the reference behaviour (uv-5).
17. **Undo stays delta-shaped** (2026-07-26, user guidance): a row delete is
    one command holding per-row `(index, bytes)` deltas — deleting 100
    commands is 100 small deltas in one command, never 100 snapshots.
    Wholesale rebuilds (crop, optimise) use one before/after stream-snapshot
    pair, the existing `ReplaceStream` pattern. Derived state (sample
    totals, loop fields, prefix sums) is *recomputed from the stream* after
    every apply/revert rather than maintained by arithmetic — the one
    exception stays the deleted-loop-point restore, which keeps its
    12-byte verbatim header capture because no arithmetic can recover it.

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
2. **License policy — updated 2026-07-26.** The user approved a GPL relicense
   (2026-07-20), but what actually landed (commit `3256a13`, 2026-07-21) is
   **LGPL-2.1-or-later** for the whole workspace — the Python/MIT code is
   gone, and every artifact statically links the LGPL nuked-opl3 core. Since
   then the codebase has twice chosen independent implementation over
   porting GPL vgmtools code precisely "so the project stays LGPL-2.1" (the
   optimiser and the song splitter — see `TODO.md`). Read that as the
   standing preference: **stay LGPL-2.1-or-later while it is cheap** (MIT /
   BSD-3 / LGPL sources, or fresh implementations from the spec), and spend
   the approved GPL move only when a GPL-only core is genuinely needed.
   Nothing in §7 is blocked: LGPL-2.1-or-later is GPL-2-compatible, so the
   first vendored GPL-2.0 core simply makes the *combined binary* distribute
   under GPL-2.0 while the workspace's own crates keep their LGPL headers —
   that is the moment to update the README / About / `docs/LICENSE.txt`
   notices. The v2-not-v3 reasoning stands: most retro-emulation GPL code is
   v2-flavoured, and a v3-only choice would lock out GPL-2.0-only sources
   such as rust-synth-emulation. Acceptable core licenses: MIT / BSD-3 /
   LGPL / GPL-2-compatible — including libvgm per §2.1. §7 has the per-chip
   audit table (and the Genesis Plus GX linking caveat, §2.1.9).
3. **The pack retag path stays byte-exact** outside the GD3 block. Header
   version normalisation (min-version rewrite) is *opt-in* — applied when the
   app synthesises a header anyway (DRO→VGM conversion, editor save of a
   restructured file) and offered as an explicit "normalise header" action at
   pack export; never silently applied to a foreign file being retagged.
4. ~~**Foreign VGMs are a separate type**, not forced through `Song`.~~
   **Superseded 2026-07-26 by §2.1.13.** This recommendation was implemented
   through mc-5 and served as the scaffolding; the end state inverts it:
   *every* VGM is a `VgmFile`, `Song` shrinks to the DRO model, and the OPL
   editing features read the one stream through a projection (uv-1).
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
SN76489 → commands 0x30/0x3F; YM-family 0x5n → 0xAn; the 16-bit-addressed
range 0xC0–0xC8 sets **bit 15 of the address word** (so bit 7 of byte 2 for
0xC0–0xC3's little-endian address, bit 7 of byte 1 for 0xC5–0xC8's big-endian
one — upstream's `Cmd_SegaPCM_Mem` and `Cmd_Ofs16_Data8` respectively);
everything else sets bit 7 of the first operand byte. This line named Sega PCM
as the *only* address-word case until 2026-07-29, and reading byte 2 for
0xC5–0xC8 on the strength of it retargeted 43.7% of the corpus's X1-010 writes
to a second chip that is not there.
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
already chip-neutral except `put_chip_clocks` (`vgm/io.rs:467`) which always
stamps OPL clocks. A hazard to remember: every save path that funnels through
`vgm::write` — pack retag (`retagged_bytes`) and volume-apply
(`revolumed_bytes`) included — re-derives 0x50/0x5C from `opl_type` and
zeroes the unused one, so foreign files must go through the mc-1 writer,
never this one. GD3 read/write, loop offset ↔ instruction-index residency
(`resolve_loop_point`/`resolve_loop_end`, `vgm/io.rs:321`/`:356` — already
chip-neutral), gzip-by-magic VGZ handling, and the byte-exact round-trip
discipline all carry over unchanged. The three OPL chokepoints to open up:
the closed command table (`vgm/data.rs:9` `mod command` + `command_size` at
`:120`, which hard-errors on any other opcode), the OPL-clock gate + `data
offset ≥ 0x80` check (`vgm/io.rs` `read_uncompressed` at `:203` — the offset
check at `:236`, the "No OPL2 or OPL3 data detected" gate at `:257`), and
the OPL-only playback path (`vgms-synth` `PlayerEngine` at `engine.rs:332` /
the `OplChip` trait at `opl.rs:12` — see §3.7 for how that path has changed).

### 3.7 Hardware output (RetroWave OPL3, shipped 2026-07-23) — new since the plan

A second audio backend now exists, and the playback phases must stay
compatible with it:

- `crates/vgms-retrowave`: serial protocol (`protocol.rs` — write-only
  SPI-over-CDC framing, nothing is ever read back), device handling
  (`device.rs`, USB `04D8:E966` as a default hint), and `SerialOpl3Chip`
  (`chip.rs:61`) — a shadow+diff register chip that **implements the
  `OplChip` trait**: seek replay touches only the shadow, `materialize()`
  bursts the diff to the wire, `generate_samples` fills silence. `player.rs`
  runs a wall-clock pump thread that reuses **`PlayerEngine::with_chip`** —
  the same engine with a different sink, PCM discarded.
- `PlayerEngine` is therefore already generic over its chip
  (`PlayerEngine<B, C: OplChip>`, `engine.rs:332`); `new` is pinned to
  `NukedOpl3` (`engine.rs:368`) and `with_chip` (`engine.rs:382`) is the
  seam. But all OPL register *policy* (bank switching, `0x105` stereo-ext
  ownership, `0xC0` shadowing, `0xBD` mute masks) still lives in its
  `execute` (`engine.rs:653`) — mc-6's generic `VgmEngine` must not inherit
  any of it.
- Backend choice: `OutputBackend::{Emulated, RetroWave}` in
  `vgms-core/src/config.rs:15` (ini keys `audio.output_backend`,
  `audio.retrowave_port`); `AudioConfig::renders_samples()` (`config.rs:95`)
  is the single predicate the UI greys controls off (peak meter, boost
  stepper, pan knobs). `SwitchingAudioService`
  (`vgm-studio/src/services/retrowave.rs:213`) holds both services and
  swaps **inside `load()`**; everything above `AudioService` is
  backend-blind.
- Consequences for this plan: `is_playable` (mc-6) becomes a function of
  *(file, backend)* — the hardware backend can only ever play OPL songs, so
  a foreign VGM must route to the emulated engine regardless of the output
  setting; the per-chip output widget (§2.1.10, mc-7) is what makes that
  routing visible rather than surprising. The `AudioService` trait has also
  grown to 20 methods (`platform.rs:169` — incl. `list_hardware_ports`,
  `last_error`, `take_limited`), and mc-7 must keep the mocks
  (`FakeAudioService`, `test_support.rs:186`) in step.

### 3.8 The in-app vgmtools suite (shipped 2026-07-2x) — new since the plan

Four vgmtools equivalents now exist, all deliberately independent
implementations ("Route B") so the project stays LGPL (§2.2.2). Their OPL
assumptions matter to mc-2/mc-4/mc-5:

| Tool | Where | OPL assumption |
|------|-------|----------------|
| Optimiser (vgm_cmp) | `vgms-core/src/optimize.rs:99` | **OPL-only by construction** — folds an `OplState` latch to drop redundant writes; gated on `Song::is_vgm`. Reached from Edit > Optimize VGM, the pack export "Opt." toggle (default ON), and `vgmstudio optimize`. The export pipeline (`vgm-studio/src/pack_zip.rs:125`) already fails safe — an unreadable file passes through verbatim — but after mc-1 foreign files *parse*, so the pipeline must gate on OPL-ness explicitly; never widen the optimiser itself. |
| Volume (vgm_vol) | `vgms-core/src/volume.rs:62` + pack Scan/Apply | The header-byte maths is chip-neutral; the *peak scan* needs a render, so it is OPL-only until cores land. "Apply Modifiers" writes via `revolumed_bytes` (`vgms-ui/src/pack.rs:1501`) → `put_chip_clocks` — the §3.6 hazard; foreign tracks need the mc-1 writer. |
| Loop finder (vgmlpfnd) | `vgms-core/src/loopfind.rs:118` | Matches delay-stripped `Instruction::Register` writes — OPL-shaped today; generalises naturally once mc-4's command stream exists. |
| Song splitter (vgm_sptd) | `vgms-core/src/split_songs.rs:102` | Gap detection is format-agnostic; `materialise` prepends an **OPL state prelude** via the fold machinery below, so the feature stays OPL-only. |

Behind the last one sits the crop machinery (also new since the plan):
`vgms-core/src/crop.rs` (`crop_to_region:91` / `delete_region:135`) and
`vgms-core/src/state_patch.rs`, whose `StateFold` (`:38`) is hardwired to the
OPL shape (`[[Option<u8>; 256]; 2]`, via `opl_state.rs:31`) and whose
`append_patch` (`:103`) re-emits the source stream's own bytes so encodings
stay exact. Plain row deletion is chip-neutral byte splicing
(`song/splice.rs`); anything that *synthesises chip state* is OPL-only —
which is exactly why mc-5's foreign editor is delete-only and leaves the
marked-region crops gated (see mc-5). And the loop-marker slide the plan
said lp-1 and mc-5 would share landed as `slide_index_past_deletion`
(`song.rs:747` — pub, pure index arithmetic); mc-5 reuses it as-is.

### 3.9 What vgmtools actually does (source-verified 2026-07-26)

Read at source level for uv-2..uv-5; these are the reference behaviours.
All of vgmtools is GPL-2.0 — same Route-B posture as ever: reimplement from
the chip facts, use the tool as the oracle, stay LGPL (§2.2.2).

**`chip_cmp.c` (behind vgm_cmp)** — the redundancy model:
- Core: a per-register last-value mirror (`RegData`) plus a first-write flag
  (`RegFirst`); a write is droppable iff not-first and value-equal.
- Per-chip *trigger* exceptions where rewriting the same value matters:
  NES APU writes with bit 7 (length-counter/trigger resets) always kept;
  Sega PCM delta-time treated as a state-machine trigger; RF5C68 channel
  select kept only if dependent writes follow (lookahead); YM2612 DAC data
  (0x2A) bypasses optimisation entirely; OPL 0x04 flag-*clear* writes are
  dropped outright (no sonic effect) — an extra rule, not just a mirror.
- SN76489: the latch/data protocol is decoded (bit 7 = latch), mirrors kept
  per decoded register (freq LSB/MSB, volume); GG stereo cached separately.
- Loop point: **all mirrors invalidated** (`memset 0xFF`) so the loop body
  re-establishes its own state — with a handful of dynamic values (banking
  modes, envelope state) deliberately preserved across the seam.
- 0x67 / 0x90–0x95 are passed through at the vgm_cmp.c level, untouched.

**`vgm_trml.c` (behind vgm_trim)** — the trim/state-restore model:
- Pre-scan 0..start tracking per-chip register masks + values, then emit at
  the new start: **memory first** (0x67 contents re-emitted as complete
  images), **then registers**, per-chip write shapes.
- Channels keyed-on at the trim point get their key-on written — the note
  re-attacks, which is the accepted behaviour (and what our OPL fold does).
- DAC-stream control (0x90–0x95) and 0xE0 seeks are *force-copied* through
  rather than reconstructed. Most chips have real state tracking; a few
  (MultiPCM, uPD7759, GA20, Mikey) are minimal or unimplemented.

**`vgm_ptch -Check`** — the header auditor/fixer (the uv-5 reference): it
verifies and can fix total samples vs summed waits, loop offset validity +
loop samples (`-CheckO` relocates the offset), data offset, EOF, GD3
offset/length/structure, version vs used fields (bumps to the minimum
needed), a missing 0x66 end marker, and junk between EOD and the tag.

**Adjacent tools noted, out of scope for uv-4** (each its own later feature):
`vgm_sro` (strip unused sample-ROM regions), `optdac` (YM2612 DAC run
cleanup), `opt_oki` (OKIM6258 → DAC-stream conversion), `vgm_dbc`
(data-block bit-packing), `vgm_dso` (DAC-stream reordering for compression).

> **Superseded 2026-07-30 by `OPTIMIZER-PLAN.md`.** `vgm_cmp`, `vgm_sro` and
> `optdac` are now *bound* rather than ported: `crates/vgms-vgmtools` builds them
> from a pinned GPL submodule and runs them as child processes, so the CLI, the
> pack export and Edit > Optimize reach every chip `vgm_cmp` has rules for
> instead of the three `chip_state::latch_rule` covers. `opt_oki` stays out
> (upstream calls it alpha, "not for public use"), and `vgm_dbc`/`vgm_dso` stay
> out because packs gzip anyway. `vgm_ptch` -- which can strip chips a rip
> declares but never writes to -- is the next candidate.

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
- All work lands on branch **`vgm-multichip`** (§2.1.12), commit-per-step as
  usual; merge back to `rust` at milestones the user agrees to.
- Keep the workspace green after every step: `cargo test --workspace`,
  `cargo clippy --workspace --all-targets` (zero warnings), `cargo fmt --all
  --check` (toolchain pinned to 1.97.0), plus the wasm check build for
  vgms-core/vgms-synth/vgms-ui.
- vgms-core / vgms-synth / vgms-ui stay **wasm-clean** (no `std::fs`, no cpal, no
  threads). Native-only code goes in vgms-audio-native / vgms-retrowave /
  vgm-studio.
- New vendored cores follow the `vendor/nuked-opl3` pattern: own directory,
  `[patch.crates-io]` or path dep, a `README.vgm-studio.md` describing origin
  + license + local changes, `[profile.dev.package.*]` opt-level override if
  hot. License files must ride along; update the workspace license notes
  (and see §2.2.2 — the first GPL core is also the GPL-relicense trigger).
- Snapshot tests: regenerate with `UPDATE_SNAPSHOTS=1`, eyeball diffs
  (baselines are wgpu DX12 WARP renders from the maintainer's machine); new
  palette fields go in the per-theme showcase (`theme_showcase.rs` — the
  code still says "theme"; the theme→skin rename has not happened).
- Every new key or gesture goes into the Help dialog's tables
  (`vgms-ui/src/dialogs/help.rs`, `SECTIONS`) in the same change. A test
  catches bound-but-undocumented shortcuts; prose rows (mouse gestures, the
  "1–9" mute range whose copy says "OPL3") it cannot check — mind those by
  hand.
- Commit style: `feat(vgms-core): parse the full VGM header chip table (mc-1)`.
- Test fixtures: pull one small VGM per chip family from VGMRips packs (SMS,
  Mega Drive, Neo Geo, PC Engine, Game Boy, NES, arcade); keep them tiny and
  note pack provenance in the fixture directory README.

## 5 · The plan

### Phase A — metadata for every VGM (the required minimum)

#### mc-1 · vgms-core: full header model + foreign-VGM container

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

The pack surface has grown a lot since this step was drafted — sub-tabs
(Tags / Tracks / Screenshots / Checklist), bulk GD3 tagging, a volume
scan/apply batch, vgm_ren file-name fixing, date fixing, a screenshots
inspector, a submission-readiness checklist, and an export deck with gzip +
optimize toggles. The good news: nearly all of it is GD3- or file-shaped,
not OPL-shaped, so foreign tracks join cheaply. Where things live now: the
pure layer (`TrackEntry`, presets, readiness) is `vgms-core/src/pack.rs`; the
stateful model (`PackTrack`, `from_folder`, `validations`, `highest_opl`)
is `vgms-ui/src/pack.rs`.

- `PackTrack.song` (`vgms-ui/src/pack.rs:35` — today a per-track parse result
  whose failure renders "unreadable") becomes `PackSong::Opl(Arc<Song>) |
  Foreign(Arc<VgmFile>) | Unreadable(String)`. `from_folder`
  (`vgms-ui/src/pack.rs:153`) tries the OPL reader first (unchanged
  behaviour for OPL files), falls back to the foreign reader; only true parse
  failures land in `Unreadable`.
- `TrackEntry::from_song` (`vgms-core/src/pack.rs:116`) generalises: title
  from GD3/filename as today; duration/loop from `VgmHeader` for foreign
  tracks (vgm_stat parity — vgm_stat trusts these header fields too).
- Description/preset generalisation: the chips line derives from
  `VgmHeader::chips()` joined display names; `preset_for`
  (`vgms-core/src/pack.rs:323`, `const` over the three `OplType`s — the
  PC/AT-only `PRESETS` table needs at least a neutral entry) and
  `highest_opl` (`vgms-ui/src/pack.rs:1021`) keep working for OPL packs and
  fall back to the derived chip string otherwise. `unique_authors`/prefill
  already read GD3 — unchanged.
- Tag paths work for foreign tracks via the mc-1 writer: quick-edit (rename
  + GD3), **bulk tag** (`BulkTagOverlay`, `vgms-ui/src/pack.rs:1527` — a pure
  GD3 overlay, no OPL assumption), Fix File Names (vgm_ren rules,
  chip-neutral) and Fix Dates (chip-neutral). The one trap: today's rewrite
  paths (`retagged_bytes` `:1487`, `revolumed_bytes` `:1501`) round-trip
  through `vgm::write` → `put_chip_clocks` (§3.6) — route foreign tracks
  through the mc-1 writer instead. `gzip_on_export` honoured (VGZ = gzip,
  unchanged).
- Batches that need a render or OPL semantics gate per-track, not per-pack:
  **Scan Volumes** skips foreign tracks (no render until their cores land;
  the Peak cell stays empty); **Apply Modifiers** only writes tracks with a
  measured peak; the export **"Opt." toggle** passes foreign files through
  verbatim (§3.8 — gate in `pack_zip.rs`, don't widen the optimiser). The
  readiness checklist (`vgms-core/src/pack.rs:1029`) is already chip-neutral
  (`TrackFacts` carries no chip info) and needs nothing.
- Row UI (`track_table`, `vgms-ui/src/pack.rs:1882`): foreign tracks get the
  normal title/duration cells; the preview (play) button is hidden/disabled
  with tooltip "Playback for <chips> is not supported yet"; "unreadable"
  styling (`:2050`) now only for `Unreadable`. The row menu's "Open in
  editor" is mc-3's to gate.
- `validations()` (`vgms-ui/src/pack.rs:625`) updated: foreign tracks are
  full citizens (counted, listed, exported); keep a soft note listing
  not-previewable chips.
- Tests: pack open with a mixed OPL + Mega Drive + corrupt folder; quick-edit
  and bulk-tag round-trips on a foreign track; description output for a
  non-OPL pack; a foreign track survives export with "Opt." on,
  byte-identical modulo gzip.

#### mc-3 · editor gating + open-file behaviour

- `load_file` (`vgms-ui/src/app.rs:2031`; the parse itself lives in
  `Editor::load`, `editor.rs:123`): when the OPL reader rejects but the
  foreign reader succeeds, replace the raw "Failed to load file" alert with
  a friendly dialog: chip list, version, duration, GD3 title, and "The
  editor supports OPL2/OPL3 songs only — open this file inside a Pack to
  edit its tags." (No hidden partial editor state.)
- Pack tab: activating Editor for a foreign track stays impossible — the
  per-track "open in editor" action is hidden/disabled for `Foreign`.
  (Interim state: mc-5 flips both this and the dialog to open the generic
  editor instead.)
- Tests: kittest snapshot of the info dialog; action-gating unit tests.

**Phase A alone satisfies requirement 1 + 3.** It needs no emulators and no
command parsing, and it is safe for VGMRips submission work because foreign
files are never structurally rewritten beyond the GD3 block.

### Phase B — the generic command stream

#### mc-4 · vgms-core: full-spec stream parser

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
  with an OPL-projection to `Instruction`.
- Tests: opcode-table round-trip property tests (decode→encode byte-exact);
  fixtures per family incl. a YM2612+DAC-stream file and a compressed-block
  file; the existing OPL suite must pass untouched.

### Phase C — the generic editor, brought forward (no emulation needed)

#### mc-5 · delete-only command editor for foreign VGMs

> **DONE, 2026-07-26** — see the progress table at the top for the commits
> and the design notes. The section below is the plan as written; where it
> and the build differ, the build is described in those notes.
>
> **The two things scoped out of it are now owned by the C2 phase
> (2026-07-26 redirection):**
> - *Marked-region crop for any VGM* → **uv-2 + uv-3**, promoted from
>   "future work" to a hard requirement (§2.1.14).
> - *The OPL reader's 0x67 support* → **solved differently by uv-1**: the
>   projection makes a data block a non-OPL row inside an OPL document, so
>   `Instruction` never needs to grow. The old note below about growing
>   the enum is obsolete.

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
  rebuild carry over), slides the loop indices via
  `slide_index_past_deletion` (`song.rs:747` — the loop-points/crop work
  already landed the generalised helper; reuse it, don't fork), re-derives
  the wait prefix. Deleting data blocks or DAC-stream commands is allowed —
  explicit intent — but rows that later commands depend on (a bank a
  0x93/0x95 references, a ROM a chip needs) earn a status-bar warning, never
  a veto. The marked-region crops (`crop_to_region`/`delete_region`,
  `vgms-core/src/crop.rs`) stay **OPL-only**: they splice synthesised state
  patches, and the `StateFold` machinery is OPL-shaped (§3.8). Foreign docs
  get plain row deletion; a per-chip state model that would generalise the
  crops is future work, not mc-5.
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
  Today's gates to generalise: `MenuState.song_type` (`app.rs:3958`,
  DRO-vs-VGM menu hiding) and `AudioConfig::renders_samples()` (backend,
  §3.7) — the flags struct subsumes the first and composes with the second.
- **Chip-scoped channel panel (§2.1.11):** `ChannelPanel`
  (`vgms-ui/src/widgets/channels.rs:35`) is hard-coded OPL — `[bool; 18]`
  mutes, `[u8; 18]` pans, two banks, `OplType`-driven layout. Wrap it in a
  chip-strip widget: a selector across the chips present in the loaded doc,
  one panel per chip. OPL docs contribute today's panel verbatim; a
  single-chip doc (every DRO, every currently-openable VGM) hides the
  selector so the existing UI is pixel-unchanged and the snapshot suite
  stays green; foreign chips contribute an empty pane until their cores
  exist (mute/pan generalisation stays out of scope, §2.1.4). The Help
  dialog's mute-key row ("channels 10 to 18 (OPL3)") becomes per-chip copy
  when this lands (§4.2).
- Tests: splice/undo/loop-slide property tests mirroring the OPL suite;
  byte-exact no-edit save; post-delete header repatch fixtures per family;
  kittest snapshots of foreign rows (chip labels, block rows, warnings); the
  existing OPL editor suite untouched.

### Phase C2 — one VGM model (added 2026-07-26; runs before playback)

The user's redirection, §2.1.13–17, turned into five steps. The architecture,
chosen after weighing the alternatives:

**A VGM's single source of truth is its byte stream** (`VgmHeader` +
`VgmStream` + `Gd3Tag` — the `VgmFile` that exists today), for *every* VGM,
OPL included. OPL-ness is a **projection**: a cheap per-row view that decodes
an OPL write out of the generic command, available when the file's chips are
OPL-family. Everything OPL-specific — the analysis columns, register
descriptions, find-register, the synth — consumes the projection.
`Song` shrinks to what it is honest about: the DRO model.

Alternatives considered and rejected: *(a)* keep `Song` universal and grow
`Instruction` with generic arms — puts non-OPL semantics inside the OPL
type and makes every consumer learn arms it can't handle; *(b)* read DRO
files into the VGM model too — breaks DRO round-tripping (ms delays, DRO
headers); *(c)* cache a `Song` beside the `VgmFile` for OPL files — two
mutable representations of one document is a sync hazard by construction.
The projection keeps one truth and derives the view.

The safety net for the whole phase: the audio/waveform/render/peak paths all
consume **immutable `Arc<Song>` snapshots**, not the editor's document — so
`OplProjection::to_song()` materialises one and *nothing in vgms-synth or
vgms-audio-native changes at all* in this phase. Playback generalisation
stays where it was, in mc-6/mc-7.

#### uv-1 · one document model: OPL becomes a capability

- `OplProjection` on `VgmFile`: available iff every clocked chip is
  OPL-family and the body walks. Per row, `project(i) ->
  Option<Instruction>` — OPL writes and waits project; a data block or
  raw row is `None`, shown by its generic description, skipped by analysis
  and by the synth materialisation (it has no wait, so timings hold). This
  is what finally closes the 0x67-in-an-OPL-file gap: no `Instruction`
  growth needed, the block is simply a non-OPL row.
- **Parity gate before anything switches:** for every OPL VGM in the corpus,
  the old reader's `Song` and the projection agree — instruction-for-
  instruction, meta field for meta field, bytes-out for bytes-out. Property
  test plus the corpus. This is the net under the whole phase.
- Editor: the mc-5 `song`/`foreign` slots become `EditorDoc::{Dro(Song),
  Vgm(VgmFile)}` — an enum that is now *honest* (a format split, not a
  capability split) — with one undo history over the enum. Capabilities
  derive from the chips: analysable/playable = projection present;
  editable = walks; taggable = always.
- Pack: `PackSong::{Dro, Vgm, Unreadable}`; preview and "open in editor"
  gate on the projection; the word "foreign" leaves the code and the copy.
- The full OPL-VGM feature inventory routes through the doc (checklist:
  VGM metadata dialog, GD3 dialog, apply-loop-to-metadata, optimize, crop,
  find loop, split songs, split channels/filter, DRO→VGM conversion output,
  volume-modifier boost seeding, `retagged`/`revolumed`).
- End state: `SongData::Vgm` and `VgmData` (the closed 8-opcode table) are
  deleted; `Song` is DRO-only; the capture/convert paths build `VgmFile`s.
- Staged, each stage green: (a) projection + parity tests, no consumers
  switched; (b) pack switches; (c) editor switches (the big one — several
  commits); (d) retire `VgmData`/`SongData::Vgm`, port convert/capture.

#### uv-2 · chip_state: one per-chip state layer, four consumers

One module serving crop (uv-3), optimize (uv-4), the split-songs prelude,
and later the engine's fast seek (mc-6). Contents:

- **Generic register-latch model**, keyed `(chip, instance, port, addr)`,
  holding each cell's last write as its *original byte span* (the
  `StateFold` trick — re-emitting source bytes keeps encodings exact).
  Restore emits last-writes in the order those writes last occurred, which
  preserves ordering constraints (mode registers before dependents) better
  than address order can.
- **SN76489 model**: the latch/data protocol decoded as chip_cmp does, plus
  GG stereo and T6W28.
- **Stream-level state** beside the chips: the catalogue of 0x67 data
  blocks seen (for hoisting — block order is load-bearing, banks append);
  per-stream-id DAC-stream setup/data/frequency and active flag
  (0x90–0x95); the 0xE0 PCM seek position.
- `OplState` + `state_patch::StateFold` fold in as the OPL instance of the
  model (per §2.1.14's rewrite licence); the existing OPL crop/split tests
  are the parity net, so the OPL model keeps its low-file-then-high
  ascending emission order.
- **Validation that needs no cores**: fold-equivalence — `fold(original,
  0..T)` must equal `fold(prelude)` for any crop point T. Property-tested
  over synthetic streams and the corpus.

#### uv-3 · crop and delete-marked-region for every VGM (the hard requirement)

- `crop_to_region(doc, start, end)`: emit hoisted data blocks (verbatim, in
  order), DAC-stream re-setup for active streams, the 0xE0 seek if the bank
  position is nonzero, then the state-restore writes, then
  `body[start..end)`. Header repatched from the stream (totals; loop slid
  or cleared), as `delete_commands` already does.
- `delete_region(doc, start, end)`: `body[..start)` + the state *delta*
  (cells whose value differs between `fold(start)` and `fold(end)`, emitted
  from their last-write bytes) + blocks hoisted from the removed span +
  `body[end..)`.
- **Rule: region operations never silently drop a 0x67 block.** Banks are
  cumulative (a YM2612 seek indexes the concatenation of every block so
  far), so removed spans donate their blocks to the seam. Explicit row
  deletion of a block stays allowed — explicit intent, status-bar warning.
- Where vgm_trml re-emits merged memory *images*, we hoist the original
  block commands verbatim — byte-conservation over cleverness; the merged
  re-emission is a later optimisation if ever wanted.
- Undo: one before/after snapshot pair over the doc (§2.1.17). Markers UI
  already generalises (`[`/`]` work by row; waveform marking needs
  playback, which foreign docs don't have yet).
- Keyed-on notes at the crop point re-attack, as vgm_trml and the OPL fold
  both do — accepted behaviour, worth one line in the Help notes.

#### uv-4 · optimise every chip

- The optimiser walks the stream with chip_state: a write is droppable iff
  its chip's rule table says the register is a pure latch and the mirror
  matches (`RegFirst` semantics). **Redundancy defaults OFF per chip** —
  no rules, no drops, pass through verbatim — because trigger registers
  (NES APU bit-7 writes, OKIM/uPD phrase starts, key-ons) make the generic
  mirror unsafe as a default. Restore-for-crop and redundancy-for-optimise
  are therefore different strictness levels of the same model.
- At the loop point, invalidate every mirror (chip_cmp's rule and already
  ours) so the loop body stays self-contained. Delay merging is
  chip-neutral and the existing byte-minimal re-encoder generalises.
- Rollout: OPL first (must reproduce today's optimiser byte-for-byte on the
  corpus — the parity gate for the rewrite), then SN76489 + YM2612 + YM2413
  (the big corpora), then chip-by-chip alongside the §mc-9 core waves.
- Validation ladder: (a) fold-equivalence at every wait boundary between
  original and optimised streams — self-checking, no cores; (b) render
  parity where a core exists (OPL today, more after mc-8); (c) corpus A/B
  against vgm_cmp as the external oracle.
- The pack export toggle and Edit > Optimize VGM drop their OPL gate; the
  log names chips passed through, as it already does.

#### uv-5 · header audit, fixed only when asked

- `vgms_core::vgm::audit(file) -> Vec<HeaderFinding>`, mirroring
  `vgm_ptch -Check` (§3.9): total samples vs summed waits; loop offset on a
  command boundary and in range; loop samples vs the derived value; a
  missing end marker; junk between EOD and the tag (we preserve it —
  offer the strip); version vs used fields (reported here, *fixed* by
  mc-10's normalise step, which this audit feeds).
- Editor: a status-bar note on load when findings exist ("Header disagrees
  with the stream — Edit > Fix Header…"), and a dialog listing each finding
  as before → after, applied as one undoable header-patch command.
- Pack: a per-track readiness **Warning** naming the disagreement, plus a
  "Fix Headers" batch (transactional, undoable, mirrors Fix Dates).
- The writer itself keeps the Phase A promise: verbatim unless asked.

### Phase D — playback

#### mc-6 · vgms-synth: multi-chip engine

- `ChipCore` trait (vgms-synth, wasm-clean):
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
- `PlayerEngine` (DRO + OPL editor path, with muting/panning) stays as-is;
  it is already generic over its chip (`PlayerEngine<B, C: OplChip>` via
  `with_chip` — the RetroWave sink uses exactly that seam, §3.7), and all
  OPL register policy lives in its `execute` (`engine.rs:653`) — leave it
  there; `VgmEngine` routes by opcode and must not inherit OPL policy.
  Folding OPL into `VgmEngine` is a possible later unification, not in
  scope. (Boost/limiting sits outside both engines — `BoostLimiter` is
  applied by the caller after `render`; the generic path reuses it
  unchanged.)
- `is_playable(header, backend) -> Playability {Full, Partial(missing
  chips), None}` drives the UI gate from mc-2 (a file is previewable iff
  every clocked chip has a registered core — offer Partial playback with
  missing chips silent, clearly labelled, if the user wants it). It is
  backend-aware (§3.7): on the RetroWave backend only pure-OPL files are
  playable, so non-OPL files always preview through the emulated engine
  whatever the OPL output setting says — mc-7's per-chip output widget is
  what makes that routing legible.
- Seek uses uv-2's chip_state where models exist: fold `0..T`, emit the
  restore writes, and only replay verbatim for chips without a model — a
  fast seek instead of a full-stream replay, and a third consumer proving
  the state layer right.
- Tests: `RecordingChip`-style fake cores asserting routing (dual-chip bit 7 /
  0xAn mirrors / SegaPCM address bit), DAC-stream timing against hand-computed
  schedules, mixer determinism across pull sizes (extend
  `output_is_independent_of_the_pull_size`).

#### mc-7 · wiring playback into the app

- `AudioService::load` (`vgms-ui/src/platform.rs:178`, today
  `load(song: Arc<Song>, config: &AudioConfig)`; the trait has grown to 20
  methods — §3.7) generalises to a source enum (OPL `Arc<Song>` |
  `Arc<VgmFile>`); NativeAudio hosts either engine behind the existing rtrb
  command/position plumbing (loop/mute/pan commands no-op for foreign
  sources). `SwitchingAudioService` (§3.7) threads the enum through: a
  foreign source always routes to the emulated service — the hardware
  backend refuses non-OPL sources by construction. Update the vgms-ui mocks
  (`FakeAudioService`, `test_support.rs:186`, which today lets
  `list_hardware_ports`/`last_error` fall through to trait defaults).
- **Per-chip output settings (§2.1.10):** replace the Settings dialog's
  single "Output" row (`dialogs/settings.rs:92`) with a per-chip widget:
  one row per chip kind the app can play, each a combo of that chip's
  available backends — "OPL2/OPL3: Nuked OPL3 (emulated) / RetroWave OPL3
  (hardware)" (with its conditional Device row); every other chip lists
  only its emulated core, or a dash until one lands (mirroring
  `is_playable`). Config: the flat `audio.output_backend` key becomes
  per-chip (e.g. `audio.output.opl`; keep reading the old key as the OPL
  row's migration default). `renders_samples()` (`config.rs:95`) becomes a
  property of the loaded doc's chips × their chosen outputs, not of the
  config alone; the greying rules stay per-capability (meter/boost/pan grey
  when an active chip doesn't render samples — today exactly the RetroWave
  case).
- Pack preview button enabled per `is_playable`; transport inside the pack tab
  stays the existing minimal preview UX.
- Generic editor gains its playback slice (the bolt-on deferred from mc-5):
  capability flags flip transport/position/waveform on for playable foreign
  docs; seek-to-selected-row via `VgmEngine` replay.
- The worklet stubs stay stubs; everything added lives in vgms-core/vgms-synth
  so Step 8/9 of the port inherit it. (vgms-retrowave stays native-only and
  OPL-only — hardware needs no worklet story.)

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

| Order | Step | Scope | State |
|-------|------|-------|-------|
| 1 | mc-1 | vgms-core: full header parse, `VgmFile` (opaque body), GD3 retag writer | **done** |
| 2 | mc-2 | pack mode: any-VGM tracks first-class, preview gated, descriptions | **done** |
| 3 | mc-3 | editor gating + friendly non-OPL open dialog | **done — Phase A: minimum requirement met** |
| 4 | mc-4 | full command-stream parser (minimal headers for OPL too) | **done** |
| 5 | mc-5 | delete-only editor for any VGM + chip-scoped channel panel | **done — any-chip trimming works** |
| 6 | uv-1 | one document model: `OplProjection`, editor/pack unification, "foreign" terminology dies | **done** |

`VgmData`/`SongData::Vgm` were **not** retired, and should not be: the
projection materialises into one, so it is now the OPL *reading* of a VGM
stream rather than a second document. Deleting it would mean making `Song`
DRO-only and giving the synth and the analyser something else to consume --
a deep change with no payoff, since nothing edits through it any more.

| 7 | uv-2 | chip_state: generic latch model + SN76489 + stream state (blocks, DAC streams, seeks) | **done** |
| 8 | uv-3 | **crop + delete-marked-region for every VGM** (hard requirement) | **done** |
| 9 | uv-4 | optimise every chip (per-chip rules, conservative default; OPL byte-parity gate) | **done** |
| 10 | uv-5 | header audit + user-invoked fix (editor dialog + pack checklist batch) | **done** |
| 11 | mc-6 | `ChipCore` trait, `VgmEngine`, decompressor, DAC streams, mixer; chip_state fast seek | **done, bar the cores** |
| 12 | mc-7 | AudioService source enum, per-chip output settings (§2.1.10), preview + editor playback wiring | **done** |
| 13 | mc-8 | SN76489 + YM2612 + YM2413 cores; SMS/MD packs play | SN76489 **done**; YM2612 and YM2413 next |
| 14 | mc-9 | core waves 1–4, one step per core | per-core |
| 15 | mc-10 | minimum-version writer + normalise-header export option (consumes uv-5's audit) | the computation **done** (`vgm::version`); the header *shrink* is what remains |

mc-10 can land any time after mc-4 (mc-5 makes it more valuable: deleting a
chip's last write lets the normalise action drop the version). The lp-1/mc-5
touchpoint this paragraph used to flag is resolved: loop points (and the
crop feature after them) landed first, and the shared helper exists as
`slide_index_past_deletion` (`song.rs:747`) — mc-5 reuses it. mc-6's engine
mirrors the shipped `LoopConfig` semantics for playback of any chip (notably: no
chip reset across the loop seam, `frames_rendered` rewinds to
`start_frames` — `engine.rs:518`).

## 7 · Emulator sourcing & licensing (audit before each port)

Workspace license: **LGPL-2.1-or-later today** (commit `3256a13`); the GPL
move stays approved and happens with the first GPL-licensed vendored core —
prefer LGPL-compatible sources while that stays cheap (§2.2.2). libvgm
assumed GPL wholesale (§2.1). ✔ = compatible with the workspace either way.

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
| VGM header read/write, OPL gate (`read_uncompressed:203`, gate `:257`), `put_chip_clocks:467`, GD3 | `crates/vgms-core/src/vgm/io.rs` |
| Command table (`mod command:9`, `command_size:120`), `VgmData`, `VgmMeta`, `Gd3Tag` | `crates/vgms-core/src/vgm/data.rs` |
| `Song`, `SongData`, prefix sums, deletion sliding, `slide_index_past_deletion:747` | `crates/vgms-core/src/song.rs` |
| Byte splicing shared by DRO+VGM deletes | `crates/vgms-core/src/song/splice.rs` |
| Crop/delete-region + the OPL state-patch fold (§3.8) | `crates/vgms-core/src/crop.rs`, `state_patch.rs`, `opl_state.rs` |
| vgmtools suite (§3.8) | `crates/vgms-core/src/optimize.rs`, `volume.rs`, `loopfind.rs`, `split_songs.rs` |
| Pack pure layer: `TrackEntry:99`, `preset_for:323`, `readiness:1029` | `crates/vgms-core/src/pack.rs` |
| Pack state/UI: `PackTrack:35`, `from_folder:153`, `validations:625`, `highest_opl:1021`, bulk tag, track table | `crates/vgms-ui/src/pack.rs` |
| Pack export pipeline (optimize + gzip + zip) | `crates/vgms-app/src/pack_zip.rs` |
| Quick-edit / bulk-tag / VGM metadata dialogs | `crates/vgms-ui/src/dialogs/track_edit.rs`, `bulk_tag.rs`, `vgm_metadata.rs` |
| Settings dialog (Output row `:92`) + config (`OutputBackend:15`, `renders_samples:95`) | `crates/vgms-ui/src/dialogs/settings.rs`, `crates/vgms-core/src/config.rs` |
| Pull engine (`PlayerEngine<B, C>:332`, `execute:653`), `OplChip` trait, muting/panning | `crates/vgms-synth/src/engine.rs`, `opl.rs` |
| RetroWave hardware backend (§3.7) | `crates/vgms-retrowave/src/*`, `crates/vgms-app/src/services/retrowave.rs` |
| cpal callback + command queue | `crates/vgms-audio-native/src/lib.rs` |
| `AudioService` trait (`:169`) + `FakeAudioService` (`test_support.rs:186`) | `crates/vgms-ui/src/platform.rs` |
| App shell: tabs, `load_file:2031`, transport, `menu_state:3958`, `ensure_audio:3860` | `crates/vgms-ui/src/app.rs` |
| Editor state + parse (`Editor::load:123`) | `crates/vgms-ui/src/editor.rs` |
| Channel panel (18-ch OPL — §2.1.11 wraps it) | `crates/vgms-ui/src/widgets/channels.rs` |
| Help dialog key tables (§4.2) | `crates/vgms-ui/src/dialogs/help.rs` |
| Vendored core pattern | `vendor/nuked-opl3` + root `Cargo.toml` patch |
| Future work list | `TODO.md` |

Line numbers cited are as of commit `10138c0` (2026-07-26) — re-locate by
symbol if drifted.

## 9 · Sources

- VGM spec: <https://vgmrips.net/wiki/VGM_Specification> (header/commands/data
  blocks/DAC streams digested in §3, fetched 2026-07-20)
- ymfm: <https://github.com/aaronsgiles/ymfm> (BSD-3)
- libymfm.wasm: <https://github.com/h1romas4/libymfm.wasm>
- rust-synth-emulation: <https://github.com/h1romas4/rust-synth-emulation>
- libvgm (default porting source per §2.1/§7): <https://github.com/ValleyBell/libvgm>
- emu2413: <https://github.com/digital-sound-antiques/emu2413> (MIT)
- `psg` crate: <https://crates.io/crates/psg>
