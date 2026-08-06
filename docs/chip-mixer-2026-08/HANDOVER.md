# Chip mixer: finalized design, ready to build

*Handover, 2026-08-02. The design conversation is finished — every question
below marked "decided" is the owner's answer, not a proposal. A fresh session
can start at "Implementation map" and treat the rest as the spec.*

## What this is

Quick whole-chip mute/solo plus a per-chip level trim, living **inline in the
chip tab strip** on the editor deck. Three mockup rounds led here; the final
round (and the two before it, for the discarded alternatives) are published at:

- Round 3, the chosen direction: <https://claude.ai/code/artifact/a0cc5d71-2bf0-476c-9681-cc586b9110d0>
- Round 2 (drawer/cards/tall-tab treatments, superseded): <https://claude.ai/code/artifact/b510e10b-8236-474a-b359-648e31e979ba>
- Round 1 (pre-unfold alternatives, superseded): <https://claude.ai/code/artifact/0ca2015d-aaf9-4ca1-91c7-d54750f0d538>

Two mockup details are **stale** against the final decisions: the round-3 page
still shows an unfold pad (folding was dropped — the controls are always
visible) and draws the knob's arc sweeping from 12 o'clock (the final arc fills
from the 0% end). The written spec below wins.

## The design (all decided)

Each tab in the chip strip becomes, left to right: **LED · trim knob · name**.
Always visible — there is no folded state and no unfold control. The `Mute` and
`Solo` pads beside the strip are **removed**; the LED replaces them, for every
chip rather than the selected one.

**The LED** is `theme::led` (12px dome, bloom, glint) made clickable, coloured
by the existing meter roles — no new palette entries:

| state | colour | role |
|---|---|---|
| playing | green | `meter_low` |
| soloed | yellow | `meter_mid` |
| muted (by the user) | unlit | `meter_off` |
| silenced by another chip's solo | dim green | darkened `meter_low` |

The dim-green fourth state is deliberate: a chip *you* muted and a chip muted
*for* you are different facts, and one dark state would lie about the second.
Left-click toggles mute; right-click toggles solo — the channel pads'
convention exactly.

**The trim knob** is a mini knob in the pan-knob chrome with a 270-degree arc:
0% at the left extreme (7:30), 100% at the right (4:30). The lit arc fills
**from the 0% end upward** to the position — so at the 100% default the full
arc is lit, and pulling a chip down visibly shortens its ring. (This is the
opposite resting state from the pan knobs, which sit unlit at centre; it is
intentional.) Right-click **or** double-click resets to 100%. Drag is relative,
as the pan knobs' is (~64 points of travel spans the range regardless of drawn
size), so the small cap costs no precision. Hover readout is a **percentage**
(`71%`), matching the 0–100% range — the trim only attenuates.

**Semantics.** 100% means *VGMPlay's reference balance*, not raw full scale:
the engine already applies the per-voice ratio from `vgms-synth/src/balance.rs`,
and the trim is a user factor multiplied on top, per chip instance. The trim is
**listening-only**: never saved to the config, never written into the file.
(A per-chip volume written into the VGM's extra header would be a file edit and
belongs on the Edit menu — explicitly out of scope here.)

**Layout.** When the strip is too wide for the deck (a six-chip arcade set),
it **wraps to a second row**. No horizontal scrolling.

## Model changes the design forces

- **Solo needs its own flag.** Today `ChipPanels::toggle_selected_solo`
  (crates/vgms-ui/src/widgets/chip_panels.rs) encodes solo as
  `set_chip_muted(true)` on every sibling — which cannot distinguish "muted by
  the user" (unlit) from "silenced by a sibling's solo" (dim green), and only
  ever solos the *selected* chip. Give `GenericChannelPanel` an explicit
  `soloed: bool`; effective silence for the engine mask becomes
  `chip_muted || (any_solo && !soloed)`; the LED reads the two flags directly.
  `selected_is_soloed`/`toggle_selected_solo` go away with the pads.
  One judgement call left open: whether soloing a second chip *adds* it (both
  play — mixer convention, and the natural reading of per-lamp solo) or
  *moves* the solo (today's behaviour). Additive is recommended.
- **A trim needs a home in the engine.** Model it on the muting/panning pair
  in `crates/vgms-synth/src/chip_mix.rs`: a `ChipTrims` keyed by
  `(ChipKind, instance)` with `set`/`for`/`is_neutral`/`entries()` (the
  worklet ABI replays state one instance per call). Apply it in
  `vgm_engine.rs` as a multiply into the per-voice gain, on top of
  `balance.rs`'s `voice_gain` — that is what makes 100% equal the reference.

## Implementation map

The plumbing precedent is the chip mute/pan path, added in `ae5a54e`
(cores), `260485a` (UI) and `f4646d5` (the switching forward). Follow it end
to end:

1. `vgms-synth`: `ChipTrims` in `chip_mix.rs` + engine application; a
   `ChipCore`-independent gain, so it works on every core (like whole-chip
   mute masks, it must not depend on `channel_mute`/`channel_pan` support).
2. `AudioService` (crates/vgms-ui/src/platform.rs): a `set_chip_trims`
   method. **Never default an AudioService method** — the app runs behind
   `SwitchingAudioService`, and a defaulted method silently no-ops there
   (this bit us before). Implement on the native service, the web/worklet
   service (ABI: replay per instance), the switching service (forward to both
   arms), and the test fake (record, like `chip_mutings`/`chip_pannings`).
3. `theme/mod.rs`: an interactive `led` variant (click sense + hover text);
   keep the existing passive one for the pack lamp.
4. `widgets/pan_knob.rs`: a trim variant. `paint_dial` already draws an arc to
   an angle; it needs a fill-from-minimum mode and a percent readout string in
   `strings.rs`. Right-click *and* double-click reset (the pan knob handles
   both already — same block). Clamp 0..=100; no detent is needed (the reset
   gesture covers returning to the extreme).
5. `widgets/chip_panels.rs`: rewrite `selector()`. The cells outgrow
   `theme::tabs::strip` (label-only); either extend the strip widget to carry
   per-cell leading content, or draw the cells directly in the same well
   chrome — whichever, the well look must not change. Add wrapping to a
   second row. Delete the Mute/Solo pads. Decide what the **OPL tab** shows —
   see open questions.
6. `app.rs`: push trims on change alongside `Action::MutingChanged` /
   `PanningChanged` (a new action or a widened response struct from the
   panels' `show`).
7. **Help dialog** (`dialogs/help.rs`): the lamp's two clicks and the knob's
   reset are mouse gestures — `Keys::Text` rows in the deck section, same
   change as the feature (see the guard's rules in the module doc). Regenerate
   `snapshot_help_dialog`.
8. Tests: GUI tests driving the lamp (click → `chip_mutings`, right-click →
   yellow + siblings dim), the trim (drag → `chip_trims`, right-click → back
   to unity), and the wrap; unit tests for `ChipTrims`; snapshot regen
   (`UPDATE_SNAPSHOTS=1`, then eyeball). The test SN76489 stand-in in
   `widgets/chip_output.rs` is already `channel_pan: true`; trims need no
   capability flag at all.

## Decided questions (owner's answers, 2026-08-02)

1. Six-chip width → **the strip wraps to a second row**.
2. Fourth lamp state → **yes, dim green** for "silenced by solo".
3. Readout vocabulary → **percentage**.
4. Folding → **none**; controls always visible.
5. Trim persistence → **listening-only**.
6. Ordering → **LED · knob · name**; arc **0% left, 100% right, lit from 0%
   up**; reset by **right-click or double-click**.

## Still open (small, decide while building)

- **The OPL tab.** An OPL document's single tab (`ChipControls::Opl`) was
  outside every mockup. A trim there duplicates the transport's Volume lever,
  and OPL whole-chip mute/solo has nothing to solo *against* in a one-chip
  file. Recommendation: generic chips only for v1; the OPL tab keeps its plain
  label. For a *mixed* strip (never happens today — OPL entries and generic
  entries don't coexist in one document) nothing needs deciding.
- **Additive vs exclusive solo** (see "Model changes" above; additive
  recommended).
- Whether a single-chip *generic* document shows the lamp/knob (harmless and
  consistent to show them; muting one chip of one is just mute).

## Where the session's other work went

Unrelated to the mixer but from the same session, already merged upstream of
`render-split-2026-08`: `7035d15` (File > Open Pack Zip...) and `ca60a87`
(every pan-capable chip gets always-visible pan knobs + the shared
Spread/Custom/Reset group in `widgets/pan_controls.rs`, and "All" leads the
toggle rows). The mixer work builds on the second: the selector rewrite and
the panels' response plumbing touch the same files.
