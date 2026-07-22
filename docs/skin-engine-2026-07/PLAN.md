# Skin engine — implementation plan (2026-07)

Replace the current theme system with a skin engine capable of rendering the
"Variation 2" Bassoon-style skin and its case-colour sub-variations
(including the cream light case and the verdigris metal finish; the sunburst
finish is dropped — see below). Reference mock-ups:
<https://claude.ai/code/artifact/5549555a-52f1-4cff-a5d0-d74019d73243>
(sections "Variation 2" and "Variation 2C").

Two scope decisions, both subtractive:

- **The FT2 chrome is discarded, its palettes kept.** There is one chrome —
  the plate look — and the old clone-dark / ft2-classic colour schemes
  survive only as two more case colours on it. No `Ft2 | Plate` branch, no
  dual-chrome maintenance. The old snapshot baselines still earn their keep
  once: they guard the mechanical threading phase (which is visually a
  no-op), and are retired when the plate chrome lands.
- **Sunburst is dropped.** It was the only case needing a radial-burst
  texture and its cache machinery — the one genuinely new rendering
  mechanism in the plan. Without it, every plate is a simple vertical
  gradient (verdigris adds one mid-stop), and the texture system disappears
  from the plan entirely.

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
| Cases | n/a | 12 case colour sets over fixed "hardware" colours (10 Bassoon cases incl. cream + verdigris, plus the two legacy palettes recast as cases) |

Everything else (menus, dialogs, scrollbars, separators) restyles through the
same roles it uses today.

## 2. Should we switch GUI frameworks first?

Evaluated against the needs above, the ~30-file existing UI, and the
egui_kittest snapshot suite:

- **Stay on egui 0.35 (recommended).** Every V2 need reduces to painter work
  we control: `Mesh` with per-vertex colours gives gradients; glow and drop
  shadows are double-painted galleys/strokes. `bevel.rs` already proves the
  pattern — we bypassed egui's stock button chrome once and can repaint that
  same code as plates. Snapshot infra, wasm path, and the whole app carry
  over untouched.
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
}
```

There is one chrome, so what was a `Chrome` struct of shape options reduces
to constants in the paint code (3px radius, plate edges, uppercase labels,
logo strip / scope grid / VU strip always on) plus two per-case knobs that
live in `CaseColors`:

- `plate: PlateFill` — the gradient stops. A 2-stop vertical gradient for
  every case except verdigris, which uses 3 stops (the copper mid-band). No
  material enum, no textures.
- `emboss: Emboss` — `ShadowOnDark` for dark cases, `EmbossOnLight` for
  cream *and* the recast ft2-classic palette (steel-blue face, near-black
  silkscreen — it was always a light-ish case).

Structure notes:

- The 12 cases = 12 `CaseColors` consts sharing one `HARDWARE` const — the
  "case changes, displays don't" rule from the mock-ups becomes structural.
  Ten Bassoon cases (navy, moss, plum, rust, cream, petrol, slate, olive,
  wine, verdigris) plus `CLONE_TEAL` and `FT2_STEEL` carrying today's two
  palettes onto the new chrome.
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

1. **Plate painter** — fills a rect from the case's `PlateFill` stops: one
   `Mesh` quad per gradient segment with per-vertex colours (one quad for the
   normal 2-stop cases, two for verdigris's 3 stops). Then the lit top hline,
   shadow bottom hline, and a rounded 1px border stroke. At 3px radius the
   sharp-cornered gradient under a rounded border is visually clean — no
   rounded-gradient tessellation needed.
2. **Glow strokes** — helper that paints a shape 2× (wide translucent, then
   core). Used by LED digits, lit LED dots, latched-key glow.
3. **Shadow label** — lays out one galley, paints it twice (1px offset in the
   emboss colour, then ink). Emboss direction/colour from the case's
   `emboss`. Becomes the one label helper all chrome text goes through, so
   the cream / ft2-steel flip is a data change.
4. **Chrome logo** — galley painted twice with two clip rects (light upper
   half, dark lower half) plus offset shadow: a credible two-tone chrome with
   zero assets. If we later want the full multi-stop look, swap to a
   pre-rendered PNG via the already-installed image loaders. Logo colours are
   hardware (identical in every case).

`bevel.rs`'s `button`/`toggle` keep their geometry, interaction and
accessibility code and get their paint step rewritten as plates; the FT2
bevel painters themselves are deleted in the cleanup phase. The groove
separators (`groove_h`/`groove_v`, `separator*` in `theme/mod.rs`) lose
their reason to exist — plate gaps do that job — so panel boundaries become
gaps against the desk colour, and the separator helpers reduce to spacing.
Menus/popups/windows stay flat `plate_lo` — matching the mock-ups, which
only gradient the fascia plates.

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
`theme=` in `[ui]`) becomes a flat case enum — one skin, twelve cases:

```rust
pub enum CaseChoice { Navy, Moss, Plum, Rust, Cream, Petrol, Slate,
                      Olive, Wine, Verdigris, CloneTeal, Ft2Steel }
```

- Strings: new cases serialise as `navy`, `cream`, … The legacy strings
  `clone-dark` and `ft2-classic` parse as aliases for `clone-teal` /
  `ft2-steel`, so existing configs keep their colour family (on the new
  chrome). Unknown values fall back to the default (now `navy`).
- `ALL` stays and now enumerates 12 entries — preserving the "new variant ⇒
  missing-baseline test failure" mechanism.
- Settings dialog: the "Theme" dropdown becomes "Case" — still a single
  dropdown, no nesting. `theme::apply_palette` → `skin::apply(ctx, choice)`.

## 7. Snapshot strategy

- **The old baselines guard exactly one phase.** Through the mechanical
  threading phase (visually a no-op) the existing `clone-dark` /
  `ft2-classic` showcase snapshots must stay pixel-identical. When the plate
  chrome lands they are retired, not regenerated.
- **Full showcase snapshots** for three representative cases: `navy`
  (canonical), `cream` (light-case emboss rules), `verdigris` (3-stop metal
  plate).
- **One case-strip snapshot**: a compact fascia+transport strip rendered once
  per case, all 12 stacked in a single PNG (mirrors the artifact's 2C
  section). Covers the colour tables — including the two legacy palettes —
  without 12 full baselines.
- The showcase itself grows the new widgets (LED readout, VU strip, LED dots,
  logo strip, scope grid) so they are exercised for every skin.
- Regenerate with `UPDATE_SNAPSHOTS=1` per the existing workflow.

## 8. Phases (one commit each unless noted)

1. **Characterisation** — extend the showcase with any widget not yet on it;
   regenerate baselines. Green tree before touching the engine.
2. **Mechanical `Skin` introduction** — wrap `Palette` in
   `Skin { case, hardware }` with today's values; thread `&Skin` through the
   ~30 files; `style_for(&Skin)`. Zero visual change; the old snapshots prove
   it — their last job. (Largest diff, dumbest content.)
3. **Plate chrome, replacing FT2** — plate painter, rounded borders,
   button/toggle repaint, shadow labels, uppercase chrome; re-enable
   tessellation feathering (see Risks). The three cases
   that exist at this point (`navy` plus `clone-teal` / `ft2-steel`, so the
   two config values users may already have keep rendering) all use the new
   chrome. Old showcase baselines retired; `navy` baseline added.
4. **Display widgets** — LED readout, VU strip, LED dots, scope grid, logo
   strip, per-column table roles. Showcase grows; regenerate.
5. **Cases** — the remaining 9 `CaseColors` consts; cream + ft2-steel emboss
   flip exercised; verdigris 3-stop plate; Settings case dropdown;
   case-strip snapshot.
6. **Migration & cleanup** — `CaseChoice` serde with legacy aliases, default
   flip to `navy`, module rename `theme` → `skin`, delete the FT2 bevel
   painters and groove separators, docs + MEMORY update.

Estimate: 4–5 working sessions — the sunburst texture work and dual-chrome
upkeep are gone. Phases 3–5 match the artifact's own effort notes; Phase 2
is the one this plan adds honestly (the mock-ups never had to pay the
threading cost).

## 9. Risks / open questions

- **Feathering comes back on.** `theme::install` currently sets
  `feathering = false` for the DOS hard-pixel look — an FT2-chrome decision
  that dies with it. The mock-ups assume antialiasing: the browser feathered
  their rounded corners, knob circles, LED dots and glow, and only the type
  is hard-pixel. Tessellation feathering doesn't touch glyph rendering, so
  Px437 stays crisp with it on. Re-enable in Phase 3 (same commit as the
  plate chrome, when every baseline is re-cut anyway); the pixel-grid
  `hline`/`vline` tricks in the edge painters keep working feathered.
- **No FT2 fallback**: once Phase 3 lands, the old look is gone — the recast
  `clone-teal` / `ft2-steel` cases keep the colours, not the chrome. Anyone
  attached to the flat-bevel look has no escape hatch; that is the accepted
  cost of dropping the dual-chrome branch (and it's this app's own theme, not
  a shipped promise).
- **Default case flip** to `navy` is a user-visible change — do it last, in
  its own commit, so it's trivially revertable.
- **Perf**: one 4-vertex mesh per plate and a couple of extra galleys per
  label are noise next to the existing per-frame table paint. No concern.
