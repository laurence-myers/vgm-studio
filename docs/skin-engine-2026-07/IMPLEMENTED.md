# Skin engine — what shipped (2026-07-23)

The skin engine and the pad/icon button chrome from `PLAN.md` and
`../button-chrome-2026-07/HANDOVER.md` are implemented on the `rust` branch.
This records what was built, the decisions that diverged from the plan, and
what was intentionally deferred.

## Composition decision

The plan's plate chrome and the handover's pad chrome were reconciled as
**model B, plate-forward** (see `mockup-case-models.html`, published at
<https://claude.ai/code/artifact/b4f93cdc-1b75-4ed8-82e7-f6c741cdc773>):
switching case repaints the whole fascia **plate**, and the pads/deck/silkscreen
follow. Buttons are backlit **pads** with line icons, not plate-buttons.

## Architecture (`crates/dro-ui/src/theme/`)

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
- `paint.rs` — colour mix + `gradient_quad`/`plate_mesh` (shared with waveform).
- `bevel.rs` — the pad painter (`button`/`toggle`/`icon_button`/`icon_toggle`,
  engage toggles latch amber) plus the old sunken bevel for wells.
- `icon.rs` — 14 line-icon glyphs as epaint segments/arcs (no SVG dep), 1.5px.
- `mod.rs` — `plate_shape`/`plate_panel`, `deck_shape`/`deck_panel`, `led_well`.
- `feathering` re-enabled; the DOS font stays hard-pixel.

## Cases (twelve)

`ThemeChoice` (in `dro-core::config`): navy (**default**), cream, verdigris,
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
- **Per-case display ink.** `data_text` is a case role, so each palette's table
  and readouts read in their own colour rather than one shared tracker yellow.
  The scope inks (wave, cursor, loop brackets) are still shared hardware.
- Channel + Perc selectors: lit engage style, **square**; audible = amber,
  muted = plain neutral pad (not recessed).
- Scope grid behind the waveform; brighter centre line.

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

`cargo test -p dro-ui -p dro-core`; regenerate snapshots with
`UPDATE_SNAPSHOTS=1` (see the `snapshot-baselines` memory) and eyeball the PNGs.
Toolchain PATH prelude: the `rust-toolchain-env` memory.
