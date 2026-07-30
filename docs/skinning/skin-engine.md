# Skin engine — what shipped (2026-07-23)

The skin engine and the pad/icon button chrome (design notes and mockups in
[`button-chrome.md`](button-chrome.md) and the sibling `mockup-*.html` files)
are implemented on the `rust` branch. This records what was built, the
decisions that diverged from the original plan, and what was intentionally
deferred.

## Composition decision

The plan's plate chrome and the handover's pad chrome were reconciled as
**model B, plate-forward** (see `mockup-case-models.html`, published at
<https://claude.ai/code/artifact/b4f93cdc-1b75-4ed8-82e7-f6c741cdc773>):
switching case repaints the whole fascia **plate**, and the pads/deck/silkscreen
follow. Buttons are backlit **pads** with line icons, not plate-buttons.

## Architecture (`crates/vgms-ui/src/theme/`)

- `Skin { case: CaseColors, hardware: HardwareColors }` composes (const) into the
  flat `Palette` every widget still takes. Twelve `CaseColors` share one
  `HARDWARE` const — "case changes, displays don't."
- **Surface modes.** `CaseColors` carries `pad: Surface` and `deck: Surface`,
  independent of the plate. `Surface = Light | Dark | Grey | Tint`: Tint follows
  the plate; Light/Dark/Grey are fixed presets. Resolved at paint time by
  `palette::pad_caps` / `palette::deck_stops`. There are **no** explicit
  `pad_cap_*` roles. Either can be **overridden from Settings** via
  `SurfaceChoice` (theme default / light / dark / tint), stored as
  `pad_style` / `deck_style` in `[ui]` and applied by `theme::palette_with` --
  so the app's palette is an owned per-config value, not one of the statics.
  The Light preset is a neutral white/grey for both (a cream one fought the
  cool plates); the deck sits a shade darker than the caps, so pads read raised.
  **Grey is a pad treatment only** (it reads flat and dirty as a deck): the deck
  dropdown iterates `SurfaceChoice::DECK`, and `for_deck()` folds a grey
  `deck_style` from a hand-edited ini back to the theme default.
- **Live preview.** Theme/Pad/Deck apply as they are picked in Settings and
  revert on Close (`Action::PreviewSkin` → `VgmStudioApp::preview_skin`). The preview
  is held in `VgmStudioApp::skin_preview`, which `palette()` prefers, and deliberately
  **not** in `config` -- the volume lever persists `config` from under us, so a
  preview parked there would write itself to the ini.
- `paint.rs` — colour mix + `gradient_quad`/`plate_mesh` (shared with waveform).
- `bevel.rs` — the pad painter (`button`/`toggle`/`icon_button`/`icon_toggle`,
  engage toggles latch amber) plus the old sunken bevel for wells.
- `icon.rs` — 14 line-icon glyphs as epaint segments/arcs (no SVG dep), 1.5px.
- `mod.rs` — `plate_shape`/`plate_panel`, `deck_shape`/`deck_panel`, `deck_ink`,
  `silkscreen_group`, `led`.
- `feathering` re-enabled; the DOS font stays hard-pixel.

## Cases (twelve)

`ThemeChoice` (in `vgms-core::config`): navy (**default**), cream, verdigris,
moss, plum, rust, petrol, slate, olive, wine, plus clone-dark / ft2-classic
recast as plate cases (grey pads). navy demos pad=Light on deck=Dark; the
dark-plate cases use a `dark_plate_case!` macro (eight anchor colours each).
Settings ▸ Theme iterates `ThemeChoice::ALL`.

## Widgets / chrome

- Line icons + pads across transport/pan/channel strips; icon buttons keep their
  text `WidgetInfo` label (tests use `get_by_label`).
- **Pad lighting = "active", not "latched".** Pads are unlit at rest and light
  amber when active; pressing always depresses. A momentary button is lit only
  while held; a toggle stays lit while on and, while held, previews the state it
  is about to take (press a lit toggle and it un-lights). `PadState { hovered,
  held, lit }`, with `lit` computed by the caller.
- **Complementary display ink.** `data_text` is a case role, and it is the
  *complement* of the plate rather than an echo of it — the clone-dark model
  (teal case, yellow ink) generalised: gold on navy and slate, cyan on cream and
  rust, coral on verdigris, lime on plum, amber on petrol, lilac on olive, mint
  on wine. `data_label` stays case-hued, as it is on clone-dark.
- **Per-case scope.** `wf_bg`, `wf_cursor` and `wf_loop` are case roles too, so
  every theme styles its own waveform: the screen tint plus cursor/bracket
  accents picked to stay distinct from the wave and from each other. `wf_wave`
  is *composed* from `data_text` (every case wanted them equal), so the wave and
  the table read as one display. The neutral parts (`wf_hover`, `wf_start`,
  `wf_dim`, `wf_loop_region`) stay hardware.
- **Deck ink is contrast-picked.** The deck is coloured independently of the
  plate, so a case's own label colour can be unreadable on it. `deck_panel` sets
  `noninteractive.fg_stroke` to `deck_ink`: dark ink on a light deck (by Rec.601
  luma), the case's `label` otherwise. Text that sets its own colour (the wells'
  tracker digits) is untouched.
- Channel + Perc selectors: lit engage style, **square**; audible = amber,
  muted = plain neutral pad (not recessed).
- Scope grid behind the waveform; brighter centre line.
- **Silkscreen groups take their ink from the caller.** `silkscreen_group` is a
  keyline box with the caption cut into its top edge (drawn as two segments
  either side of the caption, so it never has to know the surface behind it);
  the keyline is the caption ink dimmed, i.e. printed *on* the surface rather
  than engraved into it. Pass `label` on a plate or face, `data_label` on the
  desktop the pack view sits on — a case's `label` is dark ink meant for a light
  plate and would vanish on the dark desk. Both combinations are in the theme
  showcase.
- **Status lamps are surface-independent.** `led` bezels the dot with a black
  alpha rather than a palette role, so the amber warning lamp still reads on a
  light deck. The colour is a `meter_*` role chosen by the caller.

## Pack: the output deck (2026-07-24)

The pack header used to be one `right_to_left` row of five pads and three
sentence-long checkboxes, which overflowed and collided with the folder name.
Split by verb, per `docs/skinning/mockup-pack-header.html` (variation V3):

- The **header** keeps only batch operations that edit the folder in place, in
  two silkscreen groups: LEVELS (Scan Volumes / Apply / Album) and TAGS (Bulk
  Tag… / Fix Dates / Fix Names). "Fix Dates" moved here from the checklist
  heading and is now greyed rather than hidden when there is nothing to convert,
  so the header does not reflow the moment it is used; "Fix Names" (rename every
  drifted file from its GD3 tag, `vgm_ren`'s rules) sits beside it and greys the
  same way.
- The **output deck** (`pack::deck`, hosted by `app.rs` as a bottom
  `deck_panel`, the slot the editor's transport deck occupies on the other tab)
  carries the readiness lamp and everything that produces the submission: the
  verdict + a "view checklist" link, then Gzip / Optimize / Save Package Files /
  Export Zip…. Export stays put however far the form and track list scroll.
- The three checkboxes became lit pads. The screenshot button was renamed
  **Recompress** — the deck's Optimize pad is the VGM `vgm_cmp` step, and two
  different jobs must not share one word on the same screen.

## Pack: sub-sections (2026-07-24)

The page then split by job into **Tags / Tracks / Screenshots / Checklist**
(`PackSection`, drawn with the same `theme::tabs` strip as Editor/Pack). The
pack's name gets a row of its own above the strip; the output deck stays below,
whichever section is open. Consequences worth remembering:

- The LEVELS and TAGS tool groups draw **only on Tracks** — batch tools live
  with what they act on.
- A jump has to carry its section. The deck's verdict link selects Checklist;
  a checklist line targeting a metadata field selects **Tags** as well as
  setting `focus_field`, and `focus_field` is taken inside the Tags form rather
  than in `show`, so a request made from another section survives until that
  form actually draws.
- The per-row **Tags** button became **Edit…** — the strip now has a Tags tab.

**The scrollbar bug this fixed.** The checklist's clickable lines used
`TextWrapMode::Extend`. Several readiness messages run past 90 characters, so at
the app's own default 800pt window a line overflowed the panel and was painted
straight over the vertical scrollbar, burying the handle — reported as "the
scrollbar is overlaid on the content and has no puck". They wrap now
(`pack_checklist_narrow` guards it at 800×600). The section scroll area also
sets `auto_shrink([false, true])`: with horizontal shrink left on, a scroll area
sizes to its content and parks the bar against the right edge of the widest
widget — mid-panel on the narrow Tags form — instead of at the panel edge.
Vertical shrink stays **on**; forcing it off makes a short section claim the
whole viewport.

**Screenshots is an inspector** (mockup S5 + E, `docs/skinning/mockup-pack-screenshots.html`):
the image in a keylined sunken well beside its record — dimensions, aspect (named
as a PC display mode when familiar), colour format, size — from
`vgms_core::pack::PngInfo`, which reads the IHDR chunk directly rather than
decoding (fixed offset, no decoder, wasm-clean) and is parsed once per image at
scan. The empty state is a dashed box with an **Add Screenshot…** pad: it picks a
`.png` through `FileService::pick_image` / `poll_picked_image` (its own channel,
so a screenshot is never mistaken for a song to open; defaulted to nothing on the
trait, so the web build is unaffected), checks it parses as a PNG, and copies it
in as `<Game Name>.png` — the tooltip names that destination up front rather than
renaming silently. Saves route through `SavePurpose::ScreenshotAdded`, which
rescans and stops: it creates a file, and there is no previous version whose bytes
could serve as an inverse.

The inspector's three pads are **Recompress / Replace… / Delete**, and the last
two are reversible. Replace reuses the picker but keeps the replaced file's name,
writing with the old bytes as its inverse. Delete asks first, then runs as a pack
transaction — `PackMutation::Delete`, whose inverse is a `Write` of the bytes
still in memory — so Edit ▸ Undo puts the screenshot back while the pack stays
open. Its confirm carries the file *name*, since a rescan can reorder the list
between prompt and answer. `FileService::delete`/`poll_deleted` are defaulted to
nothing on the trait (web has no pack folder), and only the pack executor issues
deletes, so an outcome always belongs to the run in flight — no in-flight flag,
unlike renames, which the quick-edit path also issues. The mockup's **Show in
folder** pad is still not built.

**Scroll handles ink with `fg_stroke`, not `bg_fill`.** `spacing.scroll.
foreground_color = true`. A solid bar defaults to the face colour, and every
case's face sits close to its own trough — barely 1.6:1 on petrol — so the handle
read as part of the channel. `label` is the case's silkscreen ink, dark on the
light cases and light on the dark ones, so it always lands opposite the trough.
The cost is no hover tint on the handle.

**Nothing in a scrolling view may extend.** A widget laid out with
`TextWrapMode::Extend` overflows the panel and is painted *over* the vertical
scroll bar, burying the handle — diagnosed by the user by widening the window
until the text fit and the handle reappeared. Both checklist line kinds wrap
now (a plain `Label` extends inside a row, so it needs `.wrap()` explicitly),
and the loop note lists its tracks one per line rather than comma-joining them.

**Petrol is the default case** (2026-07-25), with `pad: Surface::Light` —
`dark_plate_case!` takes an optional pad surface for exactly this. Flipping the
default also needs `src/vgmstudio.ini` and the `vgms-core` config tests, plus the
Settings theme tests in `app_gui_tests.rs`, which name the default twice.

**Alert boxes scroll.** `alert.rs` caps the message at
`content_rect().height() - 180` and scrolls it, height still shrink-to-fit. The
pre-export prompt lists every failed check and used to grow a box taller than
the window with its buttons off the bottom.

## Deferred (not built; no blockers)

- Logo strip + horizontal VU LED strip (user: skip). The vertical peak meter
  stays.
- LED readout for the position/sample counters (user: volume only).
- Status LED dots (redundant with the lit Loop/Perc toggles).
- Per-column table inks (low value).
- LED-display volume readout — **attempted then reverted** (`44644aa`,
  reverted `f989c9e`). A `theme::led_well` painted a faint "88.88x" ghost behind
  the value, but the value (a centred `DragValue` galley of a different glyph
  count) didn't land on the ghost's segment cells. Needs a fixed-cell renderer
  (draw ghost + value into the same right-aligned monospace cells; use the
  DragValue only while editing) before retrying.
- `theme` → `skin` **module rename** (user: skip the churn). The module is still
  named `theme`; everything else uses "skin"/"case" vocabulary.

## Verifying

`cargo test -p vgms-ui -p vgms-core`; regenerate snapshots with
`UPDATE_SNAPSHOTS=1` (see the `snapshot-baselines` memory) and eyeball the PNGs.
Toolchain PATH prelude: the `rust-toolchain-env` memory.
