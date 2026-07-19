# Code review — 2026-07 (`rust` branch)

A report-only review of the `rust` branch (@ `2cb3d0f`), plus a remediation plan
and an implementation handover. **The review itself changed no code.**

Read in this order:

1. **`review-report.md`** — the consolidated findings: 3 High / 10 Medium bugs,
   ~16 Low items, simplification/duplication folds, feature-parity gaps, and
   test-coverage notes. Start here.
2. **`remediation-plan.md`** — per-issue fix directions, test hooks, effort/risk,
   and a 12-batch implementation order. All fork decisions are resolved.
3. **`HANDOVER-remediation.md`** — a self-contained brief for a fresh session to
   *implement* the plan: environment/PATH prelude, global constraints, locked
   decisions, and per-batch code anchors (file → function → change shape → test).

Supporting evidence — the raw per-agent findings with full traces:

| File | Scope |
|------|-------|
| `findings-core.md` | dro-core song / undo / io / analysis / config |
| `findings-vgmrip.md` | dro-core vgm / convert / rip |
| `findings-synth.md` | dro-synth + dro-audio-native |
| `findings-uishell.md` | dro-ui app / editor / actions / platform |
| `findings-uiwidget.md` | dro-ui rip / dialogs / widgets / theme |
| `findings-native.md` | dro-trimmer shell / services / bins |
| `findings-ux.md` | cross-cutting UI/UX behaviour bugs |
| `findings-parity.md` | Python-vs-Rust feature parity |

Method: a `cargo clippy --workspace` baseline (zero warnings) plus eight parallel
review agents, with every High/Medium finding re-verified against source. Line
numbers cited throughout are as of `2cb3d0f` and will drift once edits begin —
anchor by function name.
