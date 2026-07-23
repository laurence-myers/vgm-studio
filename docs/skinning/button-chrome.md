# Button icons + pad chrome — handover (2026-07-22)

Design exploration for replacing the transport/pan/channel strips' text
buttons with SVG icons, and re-skinning the button chrome as backlit pads.
All decisions below came from an interactive mockup session with the user;
the mockup HTML files live next to this document and are the authoritative
spec (open them in a browser — hover/press states are live).

## Where things stand

Nothing is implemented. This folder is design output only:

- `mockup-icons.html` — the icon set, final revision ("icon pass 2" +
  seam-gap fix). Also published at
  <https://claude.ai/code/artifact/3fbde781-2e9e-4692-95fb-215677bc7c68>
  (version history holds the earlier pixel-icon pass).
- `mockup-pads.html` — the button chrome, final revision ("backlit pads
  ×4, no outer glow"). Also published at
  <https://claude.ai/code/artifact/5e13b711-3313-417f-9d0b-d43ad9ce0efb>
  (version history holds the earlier 5-direction glow exploration).

## Decisions (user-confirmed)

### Icons

- **Line icons win** over chunky pixel fills: 16×16 `currentColor` SVG
  symbols, stroke-only with occasional filled elements (play/stop/tail
  triangles, slider thumbs, the 3×3 "all" grid, lock keyhole slot).
- **Butt caps, miter joins** — no round caps anywhere.
- **Seam icon**: outward-facing brackets `] [` (region end / region start,
  matching the waveform overlay's bracket direction), arrow below crossing
  the join, **with a clear gap** between brackets and arrow (brackets span
  the top half, y1.5–8.5 on the 16 grid; arrow at y11.5–15).
- **Icon-only buttons** — no icon+text hybrid; the existing `on_hover_text`
  tooltips carry the full meaning.
- Stroke width is **not baked into the symbols** — it's a single inherited
  parameter. The mockup shows the full strip at 1.5px and 2px (undecided,
  see open questions).
- Every glyph's exact path data is in the `<defs>` block of either mockup
  file. Vocabulary: del(bin), play, stop, tail(play-against-end-bar),
  seam, loop(chasing arcs), lock/unlock(padlock), match(expand-to-rails),
  custom(mixer sliders), reset(CCW arrow), perc(drum+sticks),
  all(3×3 grid), up/dn(chevrons).

### Button chrome

- **The user's preference is P2 "cream keys"** from `mockup-pads.html`:
  bone-cream plastic keycaps (TR-808 style) on a dark rubber deck, dark
  icons, latched key lights warm amber. **No outer glows** — every effect
  paints inside the widget rect (this was an explicit revision; it also
  removes the halo-clipping problem on tight groups like the channel
  digits and the −/+ ▲/▼ steppers).
- **Must be theme-able**: implement the pad chrome as *parametrized
  geometry* (radius, borders, inset lines, cap gradient stops, ink colors,
  lit-cap stops, lit ink, deck colors) with cream as the first palette —
  not hardcoded cream. The mockup's four variants (P1 lit-cap charcoal,
  P2 cream, P3 backlit-legend, P4 edge-lit rim) share identical geometry
  and differ only in numbers, which is the proof the parametrization works.

P2 reference values (from `mockup-pads.html` CSS, `.v2` scope):

| Role | Value |
| --- | --- |
| Deck (panel face) | `#1E2929`, bevels `#324444`/`#0D1414`, keyline `#040808` |
| Pad idle | vertical gradient `#F0E8D3`→`#E5DCC5`, border `#55503C`, ink `#38352A` |
| Pad idle depth | inset top highlight `rgba(255,255,255,.55)`, inset bottom `rgba(0,0,0,.12)`, drop shadow `0 1px 2px rgba(0,0,0,.6)`, radius 3px |
| Hover | `#F8F1DF`→`#EEE6D0` |
| Held | flat `#D8CFB6`, ink `#2E2B22`, inset `0 2px 3px rgba(0,0,0,.28)`, nudge down 1px |
| Latched | radial `#FCE79A`→`#EFC658`(65%)→`#DCAE3E`, border `#8A6D28`, ink `#3F2E08`, glint inset `rgba(255,255,255,.5)` |
| Labels on deck | `#B8D0D0`, muted `#6E8888` |
| Wells | unchanged data-bg `#0C1414` + tracker yellow, borders `#070C0C`/`#324444` |

## Relationship to the skin-engine plan (RESOLVED — see [`skin-engine.md`](skin-engine.md))

> **Resolved:** shipped as resolution 1 below — the skin engine is
> **model B, plate-forward**, and the pads are the button treatment *inside*
> it. See [`skin-engine.md`](skin-engine.md) for what was built. The rest of
> this section is kept for the design reasoning.

The skin-engine plan (committed before this session) planned a
*different* chrome: the Bassoon Variation-2 plate look, replacing the FT2
theme entirely, 12 case colours. This session's pad exploration happened
after and was not reconciled with it. Plausible resolutions were:

1. Pads become the *button treatment inside* the skin engine (the plan's
   architecture — `Skin { case, hardware }`, painter helpers, phased
   migration — fits pads as well as plates; a pad painter simply replaces
   the plate button painter, and "cream" maps naturally onto the plan's
   cream case).
2. Pad chrome ships first as a third theme in the *current* small theme
   system, and the skin engine follows later.
3. The skin engine plan is revised around the pad look, dropping the plate
   chrome.

Note the plan's Phase 2 (mechanical `Skin` threading) and its snapshot
strategy are useful under any resolution.

## Implementation notes (from code reading this session)

- The single seam: every button goes through `theme::bevel::{button,
  button_sized, toggle, toggle_sized}` (`crates/dro-ui/src/theme/bevel.rs`);
  button-palette roles are consumed only there plus `pan_knob.rs` (knob
  cap). Changing the painter touches zero call sites.
- Corner radius is hardcoded `CornerRadius::ZERO` in `button_impl` /
  `toggle_impl`; the pad style needs it parametrized (3px).
- Hover currently changes fill only, never fg colour; the pad style needs
  an fg-hover role (a couple of lines).
- Without outer glows, everything fits inside the widget rect: rounded
  fill, 1px border, inset lines, lit cap (2–3 concentric fills stand in
  for the radial gradient at 26px — or flat, which reads fine). P3's
  legend bloom, if ever wanted, is the icon painted twice.
- Estimated ~250–350 lines if done in the current theme system: dro-core
  `ThemeChoice` variant (+FromStr/Display/ALL + config round-trip tests),
  third `Palette` + chrome params, pad branch in bevel.rs (~60–80 lines),
  settings-dialog entry (`dialogs/settings.rs` theme picker), showcase
  snapshot (auto via `ThemeChoice::ALL`; regenerate with
  `UPDATE_SNAPSHOTS=1`).
- **Icon buttons and tests/accessibility**: `bevel::button` reports
  `WidgetInfo::labeled(..., text)` and gui tests find buttons via
  `harness.get_by_label("Play")`. Icon-only buttons must keep reporting a
  text label (pass the label for `WidgetInfo`/accessibility even when
  drawing a glyph instead). Plan icon rendering as epaint painter code
  (line segments + at most one arc per glyph — no SVG dependency needed),
  drawn from the paths in the mockup `<defs>`.
- **Feathering**: `theme::install` sets `tessellation feathering = false`
  (hard pixels). 3px radii, the radial cap, and circular icon arcs want it
  on; the skin plan already decided to re-enable feathering when its new
  chrome lands. Same call applies here — flip it in the same commit as the
  pad chrome, when baselines are re-cut anyway.

## Answered by the user (2026-07-22)

- **Icon stroke weight: 1.5px.** (Not 2px.)
- **Loop + Reset glyphs: circular** — the versions used in the strips, not
  the rectangular alternates.
- **Icons land together with the pad chrome** — one change, not icons-first
  on the current FT2 bevel.

## Still open for the user

1. How do pads compose with the skin-engine plan (see "Relationship to the
   skin-engine plan" above)? Ask before implementing either.
