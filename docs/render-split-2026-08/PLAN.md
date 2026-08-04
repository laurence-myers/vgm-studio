# Pseudo-muting + render split — implementation plan

Date: 2026-08-02, updated 2026-08-03. Status: **Stages 0–3 unimplemented; Stage 4
(= Stage K) begun.** Branch context: rebases onto `stage-i-migration` (Stage I is
complete — mg-5's `DocSource`/`doc_source()` is the seam Stage 4 needs; the plan
must rebase onto it, not the reverse).

The request: render each chip channel to its own WAV ("render split"), built on a
"pseudo-muting" mechanism — send every instruction to a core *except* those that
would start a note on a muted channel — so per-channel muting works on every
core, not only those (like libvgm) with native mute support. Long-term goal: no
code that treats OPL songs differently from any other VGM.

This plan was produced from a six-way recon of the codebase plus an adversarial
three-way critique of the draft; the findings of both are folded in below.

**2026-08-03 update — Stage 4 is Stage K, and it has begun.** The projection
retirement in `docs/review-2026-08/PLAN.md` §12b (`k-1..k-6`) is this plan's
Stage 4 (`ou-1..ou-4`) under another name — one body of work, tracked from two
docs. Stage I finishing met Stage 4's prerequisites, and two pieces are now done
(details in Stage 4 below):

- **The OPL A/B render harness is BUILT** — `crates/vgms-app/tests/opl_ab_parity.rs`
  (commit `68214bc`), the OPL shape of pm-5 / Stage K's "gate #3". `#[ignore]`d
  until ou-1's adapter exists; it is that adapter's acceptance gate. Running it
  **empirically confirmed the ou-1 premise**: `VgmEngine` renders an OPL VGM as
  **silence** (correlation 0.0000), because `CoreInfo::build()` returns `None` for
  `CoreMaker::Opl` (`registry.rs:152`) — there is no OPL `ChipCore` to host.
- **k-2a (RetroWave self-projection) is DONE** — commit `b9c5a6d`; see ou-2's
  RetroWave bullet.

**Type-name note (mg-5):** the task-source enums collapsed. `AudioSource`,
`WavSource`, `LoopSearchSource` and `SplitSource` are now all aliases of
`vgms_core::DocSource` (the `Opl`/`Vgm` arms are unchanged), and the five
`match (snapshot, vgm)` construction sites became `Editor::doc_source()`
(OPL-first) / `Editor::vgm_arc()` (the vgm-first handle Split and Crop take).
Where this plan names those types below, read `DocSource`; the routing questions
are unchanged, only the type names moved.

---

## 0. What the recon changed about the request

Three findings reshape the original framing:

1. **A per-channel render split already exists.** `split_vgm_cancellable`
   (`crates/vgms-synth/src/split.rs:252`) renders one WAV per channel of every
   chip instance, soloed via `ChipMuting` → `render_vgm_wav_mixed_cancellable`,
   with unwritten-instance pre-filtering, a silence post-filter (peak ≤
   full/1000), and stable naming (`{stem}.{chip-slug}[#2].{NN}-{short}.wav`).
   It is wired to the CLI (`vgmstudio split`), the GUI (Split Channels dialog),
   and the web Workers. Its doc comment names the gap this plan fills: *"a
   per-channel song output would need per-chip write gating, which is out of
   scope."* Its other failure: on a core without native mute the solo masks do
   nothing, so every "solo" renders the full mix and the silence filter keeps N
   identical files. Nuked-OPM — the **default** YM2151 core on native and web —
   is such a core.

2. **The OPL path already IS pseudo-muting.** `Muting::gate`
   (`crates/vgms-synth/src/engine.rs:166`) drops muted channels' `0xB0..=0xB8`
   writes and AND-masks `0xBD`, with a seek-replay rule (`mask_replay`: replay
   everything, clear KEY_ON) and a mute-edge buffered key-off. The plan
   generalizes this proven template; it does not invent the mechanism.

3. **"Send everything except note-on" is not sufficient for most chips.** The
   per-chip survey (verified against vendored libvgm sources — see Appendix)
   shows three strategies are required:
   - **DROP** — clean key-on-only registers: OPN-family FM reg `0x28`, YM2151
     reg `0x08`, GA20, uPD7759, OKIM6258, QSound ADPCM.
   - **TRANSFORM** — the key bit shares a register with pitch/mode, or a
     bit-per-channel enable register is shared across channels (dropping would
     destroy other channels' state): YM2413 `0x2n`, GB `NRx4`, NES `0x4015`,
     HuC6280 reg 4, RF5C68 `0x08` (active-low), SegaPCM `0x86` (active-low +
     bank bits), K053260/K054539/SCC/WonderSwan/SAA/VSU/C352/SCSP/X1-010/
     YMZ280B/C140/ES5503/YMF271/YMF278B-wave, OPN ADPCM-A/B, OPL `0xBD`.
   - **VOLUME** — no key-on exists; audibility is an attenuation/volume level:
     SN76489, AY8910 (and OPN SSG), Pokey, Mikey, QSound PCM.
   Plus **stateful protocols** that need shadow state, not a pure function of
   (addr, data): OKIM6295's two-byte command latch, SN76489's volume latch,
   RF5C68 reg 7 / HuC6280 reg 0 / MultiPCM port 1+2 channel-select registers.
   And **auto-key edge cases** no write filter can stop (OPN/OPM CSM keys
   channels from timer overflows) — accepted limitation.

The core bet of the plan: the existing `ChipMuting` mask (bit *i* = entry *i*
of `channels_of(kind, variant)`, ≤32 per chip) already crosses **every**
boundary — UI panels, `AudioService`, the native command ring, the worklet
extern-C ABI, the web codec. Pseudo-muting implemented *behind*
`ChipCore::set_channel_mutes` therefore needs **zero new protocol on any
platform**.

---

## 1. Design: `ChannelGate` + `GatedCore`

### 1.1 Shape

`ChannelGate` — a per-chip write filter + shadow state, in a new
`crates/vgms-synth/src/channel_gate.rs` (+ per-family submodules mirroring the
`chip_docs` layout):

```rust
pub enum GateAction {
    Pass,
    Drop,
    Replace(u16),
    /// Replace/pass, then emit these extra writes (latch re-emit,
    /// select-restore). Needed by stateful protocols.
    Then(u16, SmallVec<(u8, u16, u16)>),
}

impl ChannelGate {
    /// Build-time existence predicate — kind only; variant is unknown until
    /// reset. Drives registry engagement and mute_capable.
    pub fn exists(kind: ChipKind) -> bool;
    pub fn new(kind: ChipKind) -> Option<Self>;
    /// Re-key the tables; clears shadows. Forwarded from ChipCore::reset —
    /// variant (NES APU FDS roster) first exists here.
    pub fn reset(&mut self, clock: u32, variant: bool);
    /// C140-vs-C219: the distinction is ChipSettings::c140_type, not a
    /// ChipKind — forwarded from ChipCore::configure.
    pub fn configure(&mut self, settings: &ChipSettings);
    /// Mask clamped to the roster; edge-triggered and idempotent (apply_mix
    /// restates masks after every reset/seek, and set_panning re-runs it).
    /// Emits mute-edge writes (key-offs / volume-force) and unmute restores
    /// into `out`.
    pub fn set_mask(&mut self, mask: u32, out: &mut Vec<(u8, u16, u16)>);
    /// Updates shadows/latches on EVERY write, muted or not.
    pub fn filter(&mut self, port: u8, addr: u16, data: u16) -> GateAction;
}
```

`GatedCore` — a `ChipCore` decorator (same pattern as the registry's `Leveled`
wrapper, which is the forwarding template):

- `write`: run `filter`; apply the action against the inner core.
- `set_channel_mutes(mask)`:
  - **Full-roster mask ⇒ the gate stands down** (passes everything, emits
    nothing). `Voice::silenced` already guarantees silence for a full mask,
    and standing down preserves today's semantics — chip state keeps evolving
    under whole-chip mute, so un-muting a chip tab resumes held notes exactly
    like a native-mute chip. (Without this rule, gate-wrapped chips would
    regress the just-shipped tab-strip Mute/Solo: notes keyed during the mute
    would stay silent until re-keyed.) On the full→partial transition,
    still-muted channels are treated as newly muted (key-offs emitted then).
  - Partial mask: store in gate, feed `set_mask` output into the inner core,
    and forward the mask to the inner core too (no-op on the cores we wrap).
- `reset` / `configure`: forward to both the gate and the inner core (mask
  does not survive reset — `apply_mix` restates it, per the trait contract).
- Everything else (`load_rom`, `write_ram*`, `render`, `native_rate`, pans)
  forwards verbatim.

### 1.2 Why a core wrapper, not an engine filter

Verified against the code — **every** note-starting path funnels through
`voice.core.write`: the command stream (`vgm_engine.rs:690`), the `0x8n` DAC
fast path (`:586`), per-frame DAC-stream due-writes (`:833-846`), and seek
fold-replay (`seek_to_row` → `execute` → `write`). So a wrapper covers all of
them with no engine changes, and:

- **Seek replay needs no playback/replay flag.** OPL needed `mask_replay` only
  because its gate DROPs a register that also carries frequency bits. The
  generic gate prefers TRANSFORM wherever a register carries non-key state, so
  the same rule is correct live and on replay. (Scope note: this equivalence
  is a property of the actual wrap set — OPM/OPN key regs carry their channel
  in the data byte, and volume-forcing self-heals via mask restatement. It is
  *not* a general theorem: `ChipState::fold` keys by `(port, addr)` only, so
  select-protocol chips would mis-attribute replayed writes. If a
  select-protocol chip ever enters the live wrap set, revisit.)
- Masks survive reset for free (`apply_mix` restatement reaches the wrapper).
- The whole-chip `Voice::silenced` fast path is untouched.
- The mute-vs-pending-key-on race that OPL solves with buffered ordering is
  resolved by construction: mask-edge writes go through the same `inner.write`
  path as everything else.
- The same gate module serves three hosts: `GatedCore` (playback/render), the
  song-format splitter (stream filter), and later the OPL adapter.

### 1.3 Fallback, not replacement

Native core mute stays authoritative where it exists (all libvgm devices,
Nuked-OPN2, Nuked-OPLL): output-masking preserves cross-channel state exactly
(shared envelopes, rhythm operators, SN noise tracking tone 3), so it isolates
*better* than write gating. `GatedCore` wraps only rows with
`channel_mute: false` **and** `ChannelGate::exists(kind)`.

The practical wrap set today: **Nuked-OPM** (default YM2151 core), Nuked-PSG,
the three LLE cores — and, in Stage 4, the OPL adapters. Consequence worth
stating: the exotic TRANSFORM tables (K053260, C352, SCSP, SegaPCM, OKIM6295,
HuC6280, …) are **dead code for playback** — those chips resolve to libvgm
with native mute. They are exercised only by the song-format split (Stage 3)
and the A/B harness (pm-5). That reorders risk (playback-facing gate bugs can
only come from four row families) and makes the harness a prerequisite for the
exotic-table work, not a nicety. Table coverage is therefore incremental:
start with OPM + PSG + OPN + OPL rows; grow the rest with rs-2.

### 1.4 License discipline

The gate lives in `vgms-synth` (permissive MIT/Apache crate). Like
`chip_docs`, its tables must be written from datasheets/public register
documentation, **not** from GPL emulator source. The libvgm-verified knowledge
in the Appendix informs *tests* (the A/B harness, app-side/GPL) — not the
table code.

### 1.5 Stateful-protocol rules (the hard 10%)

- **SN76489**: the gate sees raw latch halves on one write port. A mute-edge
  volume-force byte injected between a song's latch byte and its data byte
  would re-latch the chip — the gate must re-emit the shadowed latch byte
  after any synthesized write (`GateAction::Then` / `set_mask` ordering).
  Volume writes for a muted channel are clamped to attenuation 0xF; unmute
  restores the shadowed volume.
- **OKIM6295**: any byte while a command is latched is consumed as the
  voice-start byte. Mask-edge emissions must be *deferred* until the latch
  resolves — the gate keeps a pending-emission queue drained on the next safe
  write.
- **HuC6280 / RF5C68 / MultiPCM**: synthesized writes to a channel's register
  must save/restore the select shadow around them (select + write +
  restore-select). MultiPCM is stateful (port 1 slot select, port 2 address
  select), not a clean DROP chip; its Sample register also retriggers when the
  slot is keyed, so the muted slot must be *kept* keyed-off, not merely denied
  key-ons.
- **Restore semantics are per-register, three-valued**: restore-as-is
  (level-type enables: WS SNDMOD, SCC, X1-010, VSU, SAA, volumes),
  restore-with-bits-stripped (C352 flags restore must strip KEYON or the next
  unrelated `0x202` re-fires the voice), never-restore (edge-triggered:
  K053260's per-channel bit keys on the rising edge — a restore would
  retrigger from sample start; all pure key-on registers). RF5C68's
  enable-restore restarts the sample from its start (the chip resets the
  address on disable) — documented as intended.
- Unmute never re-emits edge-triggered key-ons — a muted channel becomes
  audible at its next natural key-on, matching today's OPL unmute behavior.

---

## 2. Stages

Stage 0 is independent and lands first; 1→2→3 are ordered; Stage 4 is a
separately-decided follow-up programme (render split does **not** wait on it).

### Stage 0 — render controls (rs-0, rc-1, rc-2)

All three steps are independent of the gate and land first.

- **rs-0 — mix opt-ins for generic VGMs.** The GUI Render-to-WAV arm for
  generic VGMs currently drops everything but boost (`tasks.rs`
  `WavSource::Vgm` → `render_vgm_wav_cancellable`), even though
  `render_vgm_wav_mixed_cancellable` + `VgmRenderMix` exist (reached only by
  the split today). The OPL render dialog already has the right shape —
  independent "Channel toggles" / "Channel panning" / "Boost" opt-ins (+ "All
  of the above") — so rs-0 extends that exact trio to generic documents:
  each opt-in disabled means its neutral value (`ChipMuting`/`ChipPanning`
  neutral, boost 1.0), so the all-off default stays the faithful,
  byte-identical render. Carry a `VgmRenderMix` through
  `TaskRequest::RenderWav`, un-hide the toggle/pan rows, extend the web codec
  (`vgms-web/src/codec.rs`) — the one place a shape crosses a wire — and
  regenerate the `render_wav_dialog` kittest snapshot.
- **rc-1 — per-render core choice, plumbing.** Renders and splits get their
  own core selection, independent of the process-wide Settings choices
  (`CHOICES` in registry.rs, persisted to vgmstudio.ini — untouched by this
  feature). Mechanism: a `CoreChoices` map (slot slug → core short-name)
  carried by the render/split requests; a registry helper
  (`CoreRegistry::build_with(&choices, kind)`) resolves via `resolve_choice`
  — the *offline* resolution, so non-realtime LLE cores are legitimate picks
  for a render even though the transport cannot play them (that asymmetry is
  the point of the feature) — and applies `Leveled` (and, after pm-3,
  `GatedCore`) uniformly, so a per-render core is indistinguishable from a
  Settings-chosen one below the registry. The engine seam already exists:
  `VgmEngine::with_cores` takes a core factory (`vgm_engine.rs:212`); the
  render/split entry points in `wav.rs`/`split.rs` grow an optional choices
  parameter that builds the engine through it (absent ⇒ today's behavior).
  The OPL arm honors the same picker via `build_opl(choice, rate)` +
  `PlayerEngine::with_chip` — a small, named OPL special case that Stage 4
  dissolves with the rest.
- **rc-2 — per-render core choice, surfaces.** Render-to-WAV and Split
  dialogs get a per-slot core picker: one row per chip slot present in the
  document (the Settings core-picker rows are the template), seeded from the
  current Settings choices, session-sticky like other dialog state, never
  written to vgmstudio.ini. The Split dialog also gains the pan/boost
  opt-ins from rs-0 plus a "skip muted channels" option (see decision 9 —
  the split owns its solo masks, so "muting" for a split means excluding
  channels the user has muted from the output set). CLI: a repeatable
  `--core <slot>=<name>` flag on `render` and `split` (the CLI has no live
  panel, so the toggle opt-ins stay GUI-only; `-b/--boost` already exists).
  Web: the choices map rides the task-request codec, which also insulates
  the worker module from any registry-choice drift between wasm instances.
  Kittest snapshots for both dialogs regenerate.

### Stage 1 — ChannelGate (pm-1, pm-2)

- **pm-1**: the module per §1.1/§1.5. Initial coverage: OPL family, OPN family
  (incl. rhythm/ADPCM), OPM, SN76489, AY8910. `exists()` = false keeps
  today's honest behavior for uncovered chips.
- **pm-2**: unit tests with `RecordingChip` behind the wrapper: suppression;
  transforms preserving other channels' bits; latch protocols (SN76489 latch
  re-emit — reachable in live playback on Nuked-PSG, OKIM6295 deferral,
  select save/restore); mask-edge key-off emission; unmute restore semantics
  (all three classes); **mask clamped to roster** (the split deliberately
  sets `u32::MAX` on other instances — out-of-roster bits are ignored);
  **restatement idempotence** (apply_mix runs on every set_panning too — a
  no-edge restate emits nothing); **post-reset restatement with empty
  shadows** emits no garbage restores; full-roster stand-down and the
  full→partial edge.

### Stage 2 — universal fallback wiring (pm-3..pm-5)

- **pm-3**: `GatedCore`; engage in registry build for `channel_mute: false`
  rows where `ChannelGate::exists(kind)`. Forward every trait method (the
  `Leveled` forwarding test is the template; wrap order Leveled(Gated(core))).
- **pm-4**: `CoreRegistry::mute_capable(chip)` returns true when the resolved
  row has native mute **or** `ChannelGate::exists(kind)`. UI toggles
  un-disable (`chip_channels.rs` tooltip path); the three web wasm modules
  pick this up by construction (same registry code). No AudioService / ABI /
  codec change anywhere. Regenerate affected kittest snapshots
  (`UPDATE_SNAPSHOTS=1`) — no GUI test pins the disabled toggles today, so
  add one for the enabled state.
- **pm-5**: A/B validation harness (vgms-app side, GPL, like the VGMPlay
  parity harness): per chip, render each channel (a) native-mute-soloed vs
  (b) gate-soloed **with mask-forwarding disabled** (test-only constructor —
  otherwise the libvgm core underneath is also native-muted and the
  comparison is vacuous). The harness builds both arms through rc-1's
  explicit-choices builder (`build_with`) so it pins the exact cores under
  test instead of whatever Settings holds. Expect small legitimate deviations
  (gating changes state evolution; native mute only masks output) — compare
  RMS/peak with tolerances, not bytes. This harness is the only meaningful
  validation the exotic tables get (§1.3) and is a prerequisite for rs-2's
  table build-out. (A working precedent for the shape now exists:
  `crates/vgms-app/tests/opl_ab_parity.rs`, the Stage 4 / gate #3 OPL harness,
  builds two renders and compares them through the same `parity::compare`.)
  Byte-parity guard: neutral mask ⇒ byte-identical render (render_regression
  fixtures unchanged in this stage).

### Stage 3 — render split v2 + song split for all chips (rs-1, rs-2)

- **rs-1**: with the gate in place, `split_vgm_cancellable` becomes correct on
  the wrapped cores with no code change. Add one guard: warn (via `on_skip`)
  for chip instances whose resolved *offline* core has neither native mute
  nor a gate table, instead of silently writing N identical full-mix files
  (the split renders through the offline core choice, which `mute_capable`'s
  realtime resolution does not govern — and when an rc-1 per-render choices
  map is supplied, the guard evaluates against *that* map, not the Settings
  resolution). Naming unchanged.
- **rs-2**: song-format split for generic VGMs — a stream filter (`SongGate`)
  layered on `ChannelGate`, replaying the command stream per (chip, instance,
  channel) solo with the mask fixed before replay (no edges, no replay
  ambiguity — the simplest gate host):
  - `VgmCommand::Write` → `ChannelGate::filter`; transformed writes re-encode.
  - **`0x8n` DacWrite carries an embedded 0-15-sample wait**: when the YM2612
    DAC channel is muted, re-encode as an equivalent plain wait (never drop —
    the split's tests pin total-delay preservation). `0xE0` seeks pass.
  - **DAC-stream commands are a correctness requirement, not an
    optimization**: stream audio is synthesized at render time and does not
    exist in the command stream, so the emitted song would play a muted
    stream untouched. The filter shadows `0x90/0x91` setup to learn each
    stream's target register/channel; `0x92/0x93/0x95` for a
    muted-channel-bound stream are dropped (or a `0x94` stop is emitted).
  - Data blocks / RAM writes always pass.
  - Output: rebuild the body into a fresh command buffer (`raw_command` bytes
    for passed commands, re-encoded bytes for transformed ones) and serialize
    via the existing `vgm::file::write` (`file.rs:722`) — the machinery is
    the editor's VGM writer + stream tooling, *not* pack mode, and
    `convert::filter_vgm` is OPL-shaped (not reusable here). Loop offset is
    recomputed from the surviving-command index map.
  - Per-chip honesty: song split refuses (with a message) for chips without a
    gate table, per-chip rather than all-or-nothing. This lifts the CLI
    `--song is OPL-only` refusal chip-by-chip and un-hides the Split dialog's
    format radio for VGMs.

### Stage 4 — OPL joins the generic path (ou-1..ou-4) — follow-up programme

**This IS Stage K (`docs/review-2026-08/PLAN.md` §12b): `ou-1..ou-4` ≡ `k-1..k-5`.**
**Status (2026-08-04): ou-1 and ou-2 are DONE; live OPL playback now runs through
`VgmEngine` on both transports.** ou-1 (`9990089`,`ad313e9`,`90a9c74`,`b40a55e`):
the `OplCoreAdapter`, built WITHOUT the rate-plumbing API change the draft below
assumed — it runs the OPL chip at its native rate and lets the Voice resampler
convert, so `CoreInfo::build` needs no rate param; the A/B gate is un-ignored and
green. ou-2 (`0025ee2`,`21ed82e`,`3034695`,`035a042`) reroutes via **Design 1**, a
deliberate deviation from the draft below:
- **Kept OPL as an `Opl` AudioSource; rerouted INSIDE the backend.** `doc_source()`
  is untouched; the native + worklet `Engine` collapsed to
  `struct { VgmEngine, opl: Option<OplType> }`, the Opl arm projecting Song→VgmFile
  (`convert::opl_song_to_vgm_file`) and building a `VgmEngine`. Chosen over "flip
  doc_source to Vgm" so **the RetroWave routing gate needs NO change** —
  `source.opl().is_some()` is still true for OPL docs, so the "still open (ou-2
  proper)" routing concern below is MOOT and the hardware path is byte-for-byte
  unaffected. PlayerEngine is no longer used by either live transport.
- **The row-index map was UNNEEDED**: there is no playing-row highlight, so the 4
  `seek_pos(row)` sites became `seek_ms(ms_offset_at(row))` (ou-2c) — both engines
  agree on ms.
- **Vocabulary translation** shipped as `vgms_synth::opl_chip_muting/panning`
  (`opl_chip_mix.rs`), covering Opl2/Opl3/DualOpl2 topology, called in the `Engine`
  wrapper.
- **Consumers left as-is** (render_wav/waveform/peak/CLI/capture/analyser): correct
  OPL output on PlayerEngine; retiring them is k-3..k-5.

**ou-4a DONE (`2c5b917`): OPL splitting rerouted to the generic splitter.** DROs
project (`opl_song_to_vgm_file`); OPL VGMs split from their own file (clocks
verbatim, fixing a latent re-projection bug); the OPL mixer translates via
`opl_chip_muting/panning`. `-i/--isolate-percussion` retired; stem names are the
roster form; drums are 5 channels. Rerouted across GUI/CLI/web; `SplitTaskSource`
collapsed to `Vgm`.

**ou-4b DONE (`61c1895`): deleted synth's now-unwired OPL split code**
(`split`/`split_cancellable`/`split_percussion`/`render_one`/`channels_to_render`/
`DRUMS`/`SplitOptions`/`SplitData::Song`, −610 lines) and `capture.rs`. On ou-3:
`render_regression::every_split_channel_is_unchanged` and its `split.0.*` fixtures
were DELETED, not re-blessed -- a split stem takes the whole-song render path
(still pinned) and the generic split is covered by song_split_parity + cli_smoke +
the OPL A/B gates. A post-change adversarial-review workflow (`8a5f...` commit)
then cleaned up stale rustdoc/test-names and fixed two substantive gaps it caught:
the CLI now reads an OPL VGM raw (clock verbatim, matching the GUI) instead of
re-projecting to a canonical clock, and the Split dialog no longer advertises DRO
output for a DRO (song-format is always VGM now). **Stage K's remaining work is
k-3..k-5: re-host the register analyser off the OPL projection, then delete the
projection field + `SongData::Vgm`.**

Original scope (2026-08-03), kept for reference — the A/B render gate (gate #3,
`opl_ab_parity.rs`, `#[ignore]`d) and k-2a (RetroWave) were done; ou-1's adapter was
the go/no-go decision and nothing of it was built. The gate empirically settled the
premise below.

The goal "no code that treats OPL songs differently" cannot be reached by
flag-flipping: `DocSource::Opl` (formerly `AudioSource::Opl`) routes DROs and
all-OPL VGMs to `PlayerEngine`; `core_for` returns `None` for OPL by design
(pinned by `listed_and_buildable_are_different_questions`, registry.rs:619, plus
prose in three doc sites) — **now empirically confirmed by the A/B gate: an OPL
VGM through `VgmEngine` today is silence, correlation 0.0000**; both audio
backends' `Engine` enums cross-no-op the two muting vocabularies; DROs exist only
as `Song`. The critique establishes this stage is substantially larger than first
drafted — it is its own programme with a go/no-go decision, and stages 0-3 do not
depend on it. Scope:

- **ou-1 — `OplCoreAdapter: ChipCore`** wrapping `Box<dyn OplChip>`:
  - **Rate plumbing is a registry API change**: `CoreMaker::Generic` is
    `fn() -> Box<dyn ChipCore>` — no rate can reach the adapter, and OplChip
    resamples internally to a caller-given rate. Add a rate-taking maker
    variant (e.g. `GenericAtRate(fn(u32) -> Box<dyn ChipCore>)`) threaded
    through `CoreInfo::build`, `core_for`/`core_for_realtime`, and both
    engines' factories, with the native transport and worklet passing their
    negotiated rate. Then `native_rate() == output rate` and the Voice
    resampler's identity bypass (verified real, `resample.rs:277`) engages —
    no double resampling.
  - **Replay vs live writes**: PlayerEngine deliberately uses
    `write_reg_buffered` live but plain `write_reg` for seek replay;
    VgmEngine has one write path. Give the adapter a replay story (e.g. a
    default-no-op `ChipCore` bulk-replay hook forwarded by Leveled/GatedCore,
    or drain the buffer after fold replay) — otherwise a seek's
    hundreds-of-writes burst trickles through Nuked's spaced write buffer.
  - **Pan is part of the adapter's duty**: OPL pan is PlayerEngine register
    policy (stereo-ext `0x105` ownership + newm shadow, panpots `0xD0-0xD8`,
    `c0_shadow` resync, song-write suppression while engaged). Port it into
    the adapter behind `set_channel_pans`/`supports_pan`, or the OPL row's
    `channel_pan: true` draws dead knobs after rerouting.
  - `set_channel_mutes` → the OPL rows of ChannelGate. Deliberately rewrite
    the pinned can_build test + the three doc sites; decide what
    `CoreMaker::Opl` becomes (RetroWave still builds OplChips).
- **ou-2 — reroute OPL documents to `VgmEngine`**, which forces:
  - **The DRO document model (the blocker-class decision)**: the editor holds
    a DRO as a `Song` document (undo stack, auto-trim, dro2→dro1, Save), and
    no VGM→DRO emitter exists. Recommended shape: **Song stays the editable
    document; audio projects Song→VgmFile per reload** (via
    `convert::dro_to_vgm`) — but `dro_to_vgm` re-encodes delays, so engine
    positions (VGM command indices) no longer line up with editor rows; a
    row-index map between Song rows and projected command indices is required
    for seek, the playing-row highlight, and loop anchoring. This is a work
    item of its own, not a subordinate clause.
  - **Vocabulary translation**: the OPL panel emits `Muting`/`Panning`, and
    `ChipPanels::chip_muting()` is deliberately empty for OPL documents —
    after rerouting, the emulated path hears only `ChipMuting`/`ChipPanning`.
    Map the 18-bit mask + percussion AND-masks onto the 23-entry
    `channels_of(Ymf262)` roster (and both WAV-render task arms, and the
    worklet's OPL messages).
  - **RetroWave is a permanent, deliberate exemption** (half addressed by k-2a):
    SwitchingAudioService and the hardware service key on `source.opl()`, and the
    pump needs Song + SerialOpl3Chip. **✓ k-2a (`b9c5a6d`)** made the *hardware
    service* robust to either arm: `RetroWaveAudioService::load` now builds its own
    Song from a `DocSource::Vgm(file)` via `file.to_song()` (falling back to the
    `Opl` arm), so it keeps working once an OPL VGM arrives on the `Vgm` arm — the
    projection is now its private detail. **Still open (ou-2 proper):** the
    *routing* decision — `SwitchingAudioService` chooses the hardware backend by
    `source.opl().is_some()`, which is `None` for an OPL VGM on the `Vgm` arm, so
    it would route to the emulator instead of the hardware. That gate must learn
    to send an OPL-projectable `Vgm` arm to the hardware too. The OPL `Muting` type
    survives for the pump. Say so in the code.
  - **Enumerate every PlayerEngine consumer and assign a fate**: waveform.rs
    and peak.rs OPL arms (reroute via projection), `render_wav_*` +
    `WavSource::Opl` (retire or reroute), CLI play emulated arm (reroute),
    capture (kept for DRO-format outputs), RetroWave pump (kept). "Survives
    only as the hardware pump" was false as first drafted.
- **ou-3 — parity + timing verification**: OPL renders now flow through
  VgmEngine — re-bless render_regression fixtures once, deliberately, in this
  stage only. The correlation check already exists: the A/B render gate
  (`opl_ab_parity.rs`, built in gate #3) is the acceptance test — it goes green
  when the ou-1 adapter makes both engines agree; run it with `--ignored`. Verify
  the Nuked retrigger test still holds through the adapter and add a post-seek
  parity check (the replay-path change above).
- **ou-4 — split arm retirement (was rs-3)**: delete the OPL split arm; OPL
  goes through the generic splitter. **User-visible migration to state in the
  changelog**: output names change (`{name}.{bank}.{NN}.wav` →
  `{stem}.{slug}.{NN}-{short}.wav`), drums become 5 ordinary channels (decide
  the fate of `-i/--isolate-percussion` and the dialog checkbox — likely:
  retire the flag, drums are just channels now), and the RegisterUsage
  pre-filter gives way to the generic silence post-filter. DRO-format capture
  output stays available for DRO inputs until then.

---

## 3. Accepted limitations (documented, not solved)

- **CSM auto-key** (OPN timer-A keys FM ch 3; OPM equivalent): unstoppable by
  write gating; native mute handles it where available.
- **OPL4 FM half**: canonical `Ymf278b` roster is the 24 wavetable channels;
  adding the 23 OPL3 channels would exceed the 32-bit mask contract. FM-half
  muting stays whole-half (pre-existing gap — the linked YMF262 child gets no
  mute mask from the libvgm binding either).
- **HuC6280 wave-RAM** loads only while a channel is off — forcing OFF can
  alter waveform loads; table entry documents it.
- **Shared envelope generators** (AY R13, SAA groups): per-channel envelope
  isolation is impossible; volume forcing is the best available.
- **QSound**: PCM voices have no key-on (volume/rate forcing), and the bank
  register applies to the *next* channel — the gate must not touch banks.
- **RF5C68 unmute restarts the sample** (chip resets the address on disable).
- **ES5505/6**: libvgm stub, no emulator — moot.
- **Unmute on gate-wrapped chips** joins mid-note only at the next natural
  key-on / level write (identical to OPL today; slightly different from
  native-mute unmute, which resumes instantly). Whole-chip mute/unmute is
  identical to today by the stand-down rule (§1.1).
- **Song-format stems rely on the player resetting chips to silence** (rs-2).
  A stem silences the non-soloed chips by *dropping* their writes and keeps the
  soloed chip's never-written channels at their power-up state; both are silent
  on VGMPlay, libvgm and this app's engine (all reset to silence), but a
  hardware-accurate core whose chip powers up loud and never gets the driver's
  own silencing burst could sound a muted chip. The same assumption every trim
  and crop in the codebase makes.

## 4. Validation

1. Per-chip gate unit tests (RecordingChip; synthetic sequences) — pm-2 list.
2. A/B harness gate-vs-native per chip over corpus VGMs (forwarding disabled;
   RMS/peak tolerances) — the volume-table-bug precedent says this class of
   harness catches real bugs.
3. Neutral-mask byte-parity: untouched renders stay byte-identical
   (render_regression fixtures unchanged until ou-3, which re-blesses once).
4. Split e2e on a multichip fixture asserting per-channel WAVs *differ* (the
   current failure mode is N identical files); song-split round-trip: emitted
   per-channel VGM re-renders ≈ the corresponding WAV-split channel.
5. UI kittest snapshots regenerated where dialogs/toggles change (Stage 0,
   pm-4, ou-4); add a GUI test pinning the *enabled* toggles.
   Stage-0 additions: the `CoreChoices` map and the three mix opt-ins
   round-trip the web task codec; a render with all opt-ins off and no core
   override stays byte-identical to today's; a render with a core override
   ignores and does not mutate the Settings choices.
6. Pinned tests that must keep passing: channels_of rosters,
   mute-mask-survives-seek (both engines), switching-service forwarding,
   worklet smoke. Pinned test deliberately rewritten in ou-1:
   `listed_and_buildable_are_different_questions`.

## 5. Decisions taken (owner should veto here, not in the diffs)

1. **Fallback, not replacement** — native mute stays where it exists; the gate
   makes the capability universal. (Better isolation, less churn.)
2. **Gate is a core wrapper** — covers all four write paths, seek replay, and
   reset restatement for free; three hosts share it.
3. **TRANSFORM-preferred over DROP** — removes the playback/replay mode split
   (scoped claim, §1.2).
4. **Full-roster mask ⇒ gate stands down** — preserves today's whole-chip
   mute/unmute semantics exactly.
5. **Stage 4 decoupled and decision-gated** — render split ships without it;
   the DRO document model (Song + row-index map vs convert-on-open) is the
   headline choice inside it, and RetroWave remains a named, deliberate OPL
   special case.
6. **Gate tables datasheet-only** (license discipline); libvgm knowledge
   drives tests.
7. **Song split refuses per-chip** where no table exists (honest, incremental
   coverage) rather than waiting for a complete table set.
8. **Per-render core choices are one-shot** — seeded from Settings,
   session-sticky in the dialogs, never persisted to vgmstudio.ini, and
   resolved through the *offline* rule so non-realtime LLE cores are valid
   render picks. Playback and its Settings are never disturbed by a render's
   choices.
9. **Mix opt-ins are three independent toggles, all-off = faithful** —
   muting/panning/boost each individually enabled, mirroring the existing
   OPL dialog; disabled means the neutral value, keeping the default render
   byte-identical. For the *split*, "muting" means **skip muted channels**
   (the split owns its per-channel solo masks; the user's live toggles,
   when enabled, exclude muted channels from the output set), while pan and
   boost apply to the rendered stems exactly as in a whole-song render.

---

## Appendix: per-chip note-on strategy table

Verified [V] against vendored libvgm sources / in-repo code; [M] = domain
knowledge. Ch = canonical `channels_of` count. Strategy: how a muted channel
is kept silent. (libvgm native mute exists for every served device, so these
rows drive the gate only where §1.3 says the gate runs.)

| Chip | Ch | Note-on / audibility | Strategy |
|---|---|---|---|
| SN76489 | 4 | volume latch `1cc1vvvv`; no key-on [V] | VOLUME (latch-aware; re-emit latch after synth writes) |
| YM2413 | 14 | reg `0x2n` bit 4 + rhythm reg `0x0E` bits 0-4 [V] | TRANSFORM (clear bit 4; AND-mask 0x0E) |
| YM2612 | 7 | reg `0x28` (pure key reg) [V]; DAC = 0x2A data (+0x8n path, streams) [V] | DROP 0x28; DAC = gate 0x2A / rewrite 0x8n (rs-2) |
| YM2203 | 6 | FM 0x28; SSG = AY semantics [V] | DROP / VOLUME |
| YM2608 | 16 | FM 0x28; rhythm reg 0x10 key bits; ADPCM-B start bit 7 [V] | DROP / TRANSFORM |
| YM2610(B) | 16 | as 2608 (ADPCM-A port 1 reg 0) [V] | DROP / TRANSFORM |
| YM2151 | 8 | reg `0x08` (pure key reg) [V]; CSM caveat | DROP |
| SegaPCM | 16 | RAM byte `(ch*8)+0x86` bit 0 active-low stop [V] | TRANSFORM (force stop bit; keep bank bits) |
| RF5C68/164 | 8 | reg 0x08 channel-off mask, active-low; reg 7 select [V] | TRANSFORM (OR the off bit; select-aware) |
| OPL2/OPL (Ym3812/3526) | 14 | 0xBn bit 5; 0xBD drums [V] | existing Muting = DROP+AND-mask (Stage 4: gate rows) |
| Y8950 | 15 | OPL + Delta-T start bit 7 [V] | as OPL + TRANSFORM |
| YMF262 | 23 | as OPL2 × 2 banks [V] | as OPL2 |
| YMF278B wave | 24 | slot ctl bit 7 KEY [V]; FM = linked YMF262 (unmutable, §3) | TRANSFORM |
| YMF271 | 12 | slot reg 0 bit 0 [V] | TRANSFORM |
| YMZ280B | 8 | voice reg bit 7 + global 0xFF enable [V] | TRANSFORM |
| PWM | 1 | pure DAC [V] | whole-chip only |
| AY8910 | 3 | R7 mixer (active-low) + R8-10 volumes [M/V] | VOLUME/TRANSFORM |
| GB DMG | 4 | NRx4 bit 7 trigger [V] | TRANSFORM (clear bit 7) |
| NES APU | 5/6 | 0x4015 enables + length reloads [V/M] | TRANSFORM ×2 (keep timer-high bits) |
| HuC6280 | 6 | reg 4 bit 7 ON (select-addressed; wave-load caveat) [V] | TRANSFORM (select-aware) |
| C140/C219 | 24 | voice reg 0x5 bit 0x80 [V]; C219 via c140_type | TRANSFORM |
| K053260 | 4 | reg 0x28 rising-edge bits [V] | TRANSFORM (AND-mask; never restore) |
| K054539 | 8 | 0x214 key-on / 0x215 key-off [V] | TRANSFORM (AND-mask 0x214) |
| K051649 SCC | 5 | keyonoff level bits [V] | TRANSFORM (AND-mask; restore-as-is) |
| OKIM6258 | 1 | cmd PLAY bit [V] | DROP (single channel) |
| OKIM6295 | 4 | 2-byte latch protocol [V] | TRANSFORM (latch-aware, deferred emission) |
| Pokey | 4 | AUDC volumes; no key-on [M] | VOLUME |
| QSound | 19 | PCM: vol/rate; ADPCM: 0xd6+i [V] | VOLUME / DROP (bank quirk §3) |
| SCSP | 32 | KYONB per slot + global KYONEX [V] | TRANSFORM (clear KYONB; KYONEX always passes) |
| WonderSwan | 4 | SNDMOD enable bits [V] | TRANSFORM (keep bits 5-7) |
| VSU | 6 | SxINT bit 7 (level) [V] | TRANSFORM |
| SAA1099 | 6 | 0x14/0x15 enables + volumes [V] | TRANSFORM both / VOLUME |
| ES5503 | 32 | osc ctl halt bit [V] | TRANSFORM (force halt) |
| ES5505/6 | 32 | no emulator [V] | moot |
| X1-010 | 16 | ch reg 0 bit 0 (level) [V] | TRANSFORM |
| C352 | 32 | flags KEYON + 0x202 execute [V] | TRANSFORM (strip KEYON on restore; 0x202 passes) |
| GA20 | 4 | ch reg 6 bit 1 [V] | DROP key-on writes (pass stops) |
| Mikey | 4 | timers + volume; no key-on [V/M] | VOLUME |
| uPD7759 | 1 | start line edge [V] | DROP (single channel) |
| MultiPCM | 28 | slot reg 4 bit 7 (select-addressed; sample retrigger) [V] | TRANSFORM (select-aware, keep keyed-off) |
