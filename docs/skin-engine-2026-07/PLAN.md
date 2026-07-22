# Skin engine — implementation plan (2026-07)

Replace the current theme system with a skin engine capable of rendering the
"Variation 2" Bassoon-style skin and its eleven case-colour sub-variations
(including the cream light case and the sunburst / verdigris material
finishes). Reference mock-ups:
<https://claude.ai/code/artifact/5549555a-52f1-4cff-a5d0-d74019d73243>
(sections "Variation 2" and "Variation 2C").

The existing FT2 look is re-expressed as two skins on the new engine rather
than deleted: it costs one const each, keeps user configs valid, and — more
importantly — keeps the current snapshot baselines as the regression harness
for the refactor. Dropping them later is a one-commit decision.

---

## 1. What Variation 2 requires (inventory)

From the mock-ups, beyond what the FT2 theme can express today:

| Need | Today | V2 |
| --- | --- | --- |
| Panel fill | flat colour | vertical gradient plate, lit top edge, shadowed bottom edge, 3px corner radius, dark border |
| Buttons | flat + 2-tone bevel | gradient face, lit edge, rounded; latch state is a brass/amber gradient |
| Chrome text | plain galley | 1px drop shadow (flipped to light emboss on the cream case) |
| Numeric readouts | text in a well | LED digits with ghost "8888" underlay and a soft glow |
| Waveform well | flat bg + wave | scope: grid lines + bright centre line behind the wave |
| Peak meter | vertical segments | plus a horizontal VU LED strip in the logo band |
| Status LEDs | none | small lit/unlit dots (loop, perc) |
| Table text | 3 roles | per-column roles (pos / bank / reg / value / description) |
| Branding | none | logo strip with two-tone chrome lettering |
| Cases | n/a | 11 case colour sets over fixed "hardware" colours; 2 material finishes (radial burst, two-stop metal) |

Everything else (menus, dialogs, scrollbars, separators) restyles through the
same roles it uses today.

## 2. Should we switch GUI frameworks first?

Evaluated against the needs above, the ~30-file existing UI, and the
egui_kittest snapshot suite:

- **Stay on egui 0.35 (recommended).** Every V2 need reduces to painter work
  we control: `Mesh` with per-vertex colours gives gradients; glow and drop
  shadows are double-painted galleys/strokes; the radial burst is a small
  generated texture. `bevel.rs` already proves the pattern — we bypassed
  egui's stock button chrome once and can branch that same code on a chrome
  style. Snapshot infra, wasm path, and the whole app carry over untouched.
- **iced** — has native gradient backgrounds and a theme system, but custom
  widgets (LED readouts, scope, VU) are more ceremony than egui's painter,
  snapshot testing is DIY, and it's a full rewrite of dro-ui. Rewrite cost
  dwarfs the painter work by an order of magnitude.
- **Slint** — best-in-class declarative skinning (gradients, states, assets in
  the DSL), but the UI moves into the Slint language, canvas-style custom
  drawing (waveform, scope) is clumsier, and licensing needs care for a
  distributed build. Same rewrite objection.
- **Webview (Tauri / Dioxus)** — the honest tempter: the mock-ups are HTML/CSS
  and express the entire skin in ~200 lines of CSS, so if this app were
  greenfield, a webview UI would make *this particular feature* easiest. But
  it means shipping a webview, re-plumbing audio/timer interaction, rewriting
  every view and dialog, and losing kittest. Rejected on total cost.
- **GPUI / FLTK / relm4** — no skinning advantage over egui for this design
  language; same rewrite cost. Not considered further.

**Verdict: stay on egui.** The skin is 3 small painters + 5 small widgets on
top of code that already exists and is already snapshot-tested. A framework
switch trades ~a week of painter work for ~a month of porting with no better
end result.

## 3. Architecture

`crates/dro-ui/src/theme/` becomes `crates/dro-ui/src/skin/` (module rename
last, after content settles). The core type splits colour data by *who may
override it*:

```rust
pub struct Skin {
    pub case: CaseColors,       // per-case: plates, buttons, labels, selection
    pub hardware: HardwareColors, // fixed across cases: LED amber, scope green,
                                  // brass latch, VU zones, LED dots
    pub chrome: Chrome,          // shapes & behaviours, not colours
}

pub struct Chrome {
    pub bevel: BevelStyle,        // Ft2 | Plate
    pub material: Material,       // Flat | VGradient | Metal { mid: f32 } | RadialBurst
    pub corner_radius: u8,        // 0 for FT2, 3 for plates
    pub emboss: Emboss,           // None | ShadowOnDark | EmbossOnLight (cream)
    pub label_case: LabelCase,    // Normal | Upper (V2 uppercases chrome text)
    pub logo_strip: bool,
    pub scope_grid: bool,
    pub vu_strip: bool,
}
```

- The 11 cases = 11 `CaseColors` consts sharing one `HARDWARE` const — the
  "case changes, displays don't" rule from the mock-ups becomes structural.
- The two FT2 skins are `Skin`s with `bevel: Ft2, material: Flat,
  corner_radius: 0, emboss: None` and a `CaseColors` carrying today's palette
  values. Pixel-identical output is the Phase-1 acceptance test.
- `Palette` (the ~40-role struct) survives, reorganised into
  `CaseColors`/`HardwareColors`; widgets keep taking one borrowed argument —
  now `&Skin` — so the 30 call-site files change mechanically.

### New colour roles

CaseColors adds: `plate_hi/lo/top/btm/border`, `btn_hi/lo/edge/text`,
`sel/sel_text`, `col_bank/col_desc` (case-tinted table columns).
HardwareColors adds: `latch_hi/lo/text`, `led_well/ghost/lit/glow`,
`scope_bg/grid/center/trace`, `vu_zones: [VuZone; 3]`, `dot_on/off`,
`col_pos/col_reg/col_val`.

## 4. Painters (skin/paint.rs)

1. **Plate painter** — fills a rect per `Material`:
   - `VGradient`: one 4-vertex `Mesh` quad, top vertices `plate_hi`, bottom
     `plate_lo`.
   - `Metal { mid }`: two stacked gradient quads (the verdigris copper).
   - `RadialBurst`: generate a `ColorImage` once per (bucketed size, skin
     generation), cache as a texture, draw stretched. Buckets: panel heights
     round to 8px so the cache stays small. (Sunburst only; do last.)
   - Then the lit top hline, shadow bottom hline, and a rounded 1px border
     stroke. At 3px radius the sharp-cornered gradient under a rounded border
     is visually clean — no rounded-gradient tessellation needed.
2. **Glow strokes** — helper that paints a shape 2× (wide translucent, then
   core). Used by LED digits, lit LED dots, latched-key glow.
3. **Shadow label** — lays out one galley, paints it twice (1px offset in the
   emboss colour, then ink). Emboss direction/colour from `Chrome::emboss`.
   Becomes the one label helper all chrome text goes through, so the cream
   flip is a data change.
4. **Chrome logo** — galley painted twice with two clip rects (light upper
   half, dark lower half) plus offset shadow: a credible two-tone chrome with
   zero assets. If we later want the full multi-stop look, swap to a
   pre-rendered PNG via the already-installed image loaders. Logo colours are
   hardware (identical in every case).

`bevel.rs` keeps its FT2 painters verbatim; `button`/`toggle` gain a
`match skin.chrome.bevel` at their paint step (geometry/interaction code is
shared). Menus/popups/windows stay flat `plate_lo` — matching the mock-ups,
which only gradient the fascia plates.

## 5. Widgets

| Widget | File | Notes |
| --- | --- | --- |
| LED readout | `widgets/led_readout.rs` (new) | ghost "8888" galley + right-aligned value galley + glow pass; unit label in `muted`. Replaces the volume field and position-panel numerics when the skin enables it. |
| VU strip | `widgets/vu_strip.rs` (new) | 2×54 cells from `vu_zones`; driven by the same `PeakMeterState` maths (extract the fall/hold logic into a shared helper rather than duplicating). Lives in the logo strip. |
| LED dot | `widgets/led_dot.rs` (new) | tiny; lit = radial-ish two-tone fill + glow. |
| Scope grid | `widgets/waveform.rs` (edit) | when `chrome.scope_grid`: grid verticals/horizontals + bright centre line painted before the wave. |
| Logo strip | `widgets/logo_strip.rs` (new) | chrome logo + subtitle + VU strip; a top panel row added in `app.rs` behind `chrome.logo_strip`. |
| Table columns | `widgets/table.rs` (edit) | per-column ink from the new roles; header band via plate painter. |
| Pan knob | `widgets/pan_knob.rs` (edit) | tick colour becomes `hardware.led_lit` under Plate chrome. |

Second font slot: not needed — the mock-ups set the logo in Px437 at 2× size,
which stays pixel-crisp (16px integer multiples). `fonts.rs` keeps the slot
open but Phase 3 does not add a face.

## 6. Config and migration

`dro-core::config::ThemeChoice` (kebab strings via `Display`/parse, stored as
`theme=` in `[ui]`) becomes:

```rust
pub enum SkinChoice { CloneDark, Ft2Classic, Bassoon(BassoonCase) }
pub enum BassoonCase { Navy, Moss, Plum, Rust, Cream, Petrol, Slate,
                       Olive, Wine, Sunburst, Verdigris }
```

- Strings: existing `clone-dark` / `ft2-classic` parse unchanged; new cases
  serialise as `bassoon-navy`, `bassoon-cream`, … Unknown values keep falling
  back to the default (now `bassoon-navy`).
- `ALL` stays and now enumerates 13 entries — preserving the "new variant ⇒
  missing-baseline test failure" mechanism.
- Settings dialog: "Theme" dropdown becomes "Skin" + a "Case" dropdown that is
  enabled only for Bassoon. `theme::apply_palette` → `skin::apply(ctx, choice)`.

## 7. Snapshot strategy

- **Full showcase snapshots** (existing mechanism) for: `clone-dark`,
  `ft2-classic` (must stay pixel-identical through Phase 1–2 — the refactor's
  safety rail), `bassoon-navy` (canonical), `bassoon-cream` (light-case
  rules), `bassoon-sunburst` (texture material path).
- **One case-strip snapshot**: a compact fascia+transport strip rendered once
  per case, all 11 stacked in a single PNG (mirrors the artifact's 2C
  section). Covers the colour tables without 11 full baselines.
- The showcase itself grows the new widgets (LED readout, VU strip, LED dots,
  logo strip, scope grid) so they are exercised for every skin.
- Regenerate with `UPDATE_SNAPSHOTS=1` per the existing workflow.

## 8. Phases (one commit each unless noted)

1. **Characterisation** — extend the showcase with any widget not yet on it;
   regenerate baselines. Green tree before touching the engine.
2. **Mechanical `Skin` introduction** — wrap `Palette` in
   `Skin { case, hardware, chrome }` with FT2 values; thread `&Skin` through
   the ~30 files; `style_for(&Skin)`. Zero visual change; old snapshots prove
   it. (Largest diff, dumbest content.)
3. **Plate chrome** — plate painter (VGradient), rounded borders, button/
   toggle Plate branch, shadow labels, uppercase chrome option. Add
   `BASSOON_NAVY`; new showcase baseline.
4. **Display widgets** — LED readout, VU strip, LED dots, scope grid, logo
   strip, per-column table roles. Showcase grows; regenerate.
5. **Cases** — 10 more `CaseColors` consts; cream emboss flip; `Metal`
   material (verdigris); `RadialBurst` texture (sunburst); Settings case
   dropdown; case-strip snapshot.
6. **Migration & cleanup** — `SkinChoice` serde, default flip to
   `bassoon-navy`, module rename `theme` → `skin`, docs + MEMORY update,
   delete anything now dead.

Estimate: 4–6 working sessions. Phases 3–5 match the artifact's own effort
notes; Phase 2 is the one this plan adds honestly (the mock-ups never had to
pay the threading cost).

## 9. Risks / open questions

- **Feathering off + rounded corners**: the app disables tessellation
  feathering for hard pixels; 3px-radius strokes may alias. Mitigation:
  radius is small and the mock-ups are hard-pixeled anyway; if it looks bad,
  plates fall back to square corners (the look survives — Bassoon reads
  "plate" mostly from the gradient + lit edge).
- **Texture cache invalidation** (sunburst): key by (size bucket, skin
  generation counter); clear on skin switch. Small and bounded.
- **Default skin flip** is a user-visible change — do it last, in its own
  commit, so it's trivially revertable.
- **Perf**: one 4-vertex mesh per plate and a couple of extra galleys per
  label are noise next to the existing per-frame table paint. No concern.
